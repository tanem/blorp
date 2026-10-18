//! The state task: the loop that owns `App`, and the executor that carries
//! out `update`'s effects.

use std::io;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::Event;
use futures::{Stream, StreamExt};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::app::{App, Effect, Msg, update};
use crate::jmap::{EmailUpdate, Error, Fatal, Jmap};
use crate::mail::Mailboxes;
use crate::ui;
use crate::undo::{Replay, Replayed};

const SPINNER_TICK: Duration = Duration::from_millis(250);

/// Where the state task reads the time from before each `update`.
pub trait Clock: Send + 'static {
    fn now(&self) -> Timestamp;
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that stays where it is set, for tests.
#[derive(Clone)]
pub struct TestClock(Arc<Mutex<Timestamp>>);

impl TestClock {
    pub fn new(now: Timestamp) -> Self {
        TestClock(Arc::new(Mutex::new(now)))
    }

    pub fn set(&self, now: Timestamp) {
        *self.0.lock().unwrap() = now;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap()
    }
}

/// Why the state task stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The user quit, or the key stream ended.
    Quit,
    /// Fastmail refused the token; the caller restores the terminal and
    /// prints the message.
    Fatal(Fatal),
}

/// The launch steps that need Fastmail before the TUI starts: `Mailbox/get`,
/// giving the app the state task starts from.
pub async fn launch<J: Jmap>(jmap: &J, now: Timestamp) -> Result<App, Error> {
    let mailboxes = jmap.get_mailboxes().await?;
    Ok(App::new(Mailboxes::from(mailboxes), now))
}

/// Runs the state task until a quit or fatal effect or the end of `keys`,
/// and says which: sends the
/// launch message, then turns key events, request results, and spinner ticks
/// into messages, redrawing once after everything ready is handled.
pub async fn run<B, K, J, C>(
    terminal: &mut Terminal<B>,
    mut keys: K,
    jmap: J,
    clock: C,
    mut app: App,
) -> io::Result<Exit>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
    K: Stream<Item = io::Result<Event>> + Unpin,
    J: Jmap,
    C: Clock,
{
    let (tx, mut rx) = unbounded_channel();
    let outbox = outbox(jmap.clone(), tx.clone());
    let executor = Executor { jmap, tx, outbox };
    let mut spinner = tokio::time::interval(SPINNER_TICK);
    let mut spinning = false;
    let mut next = Some(Msg::Start);
    loop {
        if let Some(msg) = next.take()
            && let ControlFlow::Break(exit) = executor.dispatch(&mut app, &clock, msg)
        {
            return Ok(exit);
        }
        while let Ok(msg) = rx.try_recv() {
            if let ControlFlow::Break(exit) = executor.dispatch(&mut app, &clock, msg) {
                return Ok(exit);
            }
        }
        if app.loading() && !spinning {
            spinner.reset();
        }
        spinning = app.loading();
        terminal
            .draw(|frame| ui::view(&app, frame))
            .map_err(io::Error::other)?;
        next = tokio::select! {
            key = keys.next() => match key {
                Some(Ok(Event::Key(key))) => ui::key_msg(key),
                Some(Ok(_)) => None,
                Some(Err(error)) => return Err(error),
                None => return Ok(Exit::Quit),
            },
            Some(msg) = rx.recv() => Some(msg),
            _ = spinner.tick(), if spinning => Some(Msg::Tick),
        };
    }
}

/// A job for the outbox.
enum Job {
    /// An action's `Email/set`.
    Set(EmailUpdate),
    /// An undo or redo.
    Replay(Replay),
}

/// Starts the outbox task: it runs each job once the job before it has
/// been answered, reports it back as [`Msg::Sent`] or [`Msg::Replayed`],
/// and stops when the sender is dropped.
fn outbox<J: Jmap>(jmap: J, tx: UnboundedSender<Msg>) -> UnboundedSender<(u64, Job)> {
    let (outbox, mut jobs) = unbounded_channel::<(u64, Job)>();
    tokio::spawn(async move {
        while let Some((job, work)) = jobs.recv().await {
            let msg = match work {
                Job::Set(update) => Msg::Sent {
                    job,
                    result: jmap.set_emails(&update).await,
                },
                Job::Replay(replay) => Msg::Replayed {
                    job,
                    result: replay_emails(&jmap, &replay).await,
                },
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    outbox
}

/// `Email/get` on the replay's emails, then one `Email/set` for those still
/// as expected, or none when no email is.
async fn replay_emails<J: Jmap>(jmap: &J, replay: &Replay) -> Result<Replayed, Error> {
    let got = jmap.get_emails(&replay.ids()).await?;
    let (update, skipped) = replay.split(&got);
    let not_updated = if update.is_empty() {
        Vec::new()
    } else {
        jmap.set_emails(&update).await?.not_updated
    };
    Ok(Replayed {
        skipped,
        not_updated,
    })
}

struct Executor<J> {
    jmap: J,
    tx: UnboundedSender<Msg>,
    outbox: UnboundedSender<(u64, Job)>,
}

impl<J: Jmap> Executor<J> {
    /// Sets the time, runs `update`, and carries out its effects.
    fn dispatch(&self, app: &mut App, clock: &impl Clock, msg: Msg) -> ControlFlow<Exit> {
        app.now = clock.now();
        for effect in update(app, msg) {
            match effect {
                Effect::Query { generation, query } => {
                    let jmap = self.jmap.clone();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let result = jmap.query(&query).await;
                        let _ = tx.send(Msg::Loaded { generation, result });
                    });
                }
                Effect::SetEmails { job, update } => {
                    let _ = self.outbox.send((job, Job::Set(update)));
                }
                Effect::Replay { job, replay } => {
                    let _ = self.outbox.send((job, Job::Replay(replay)));
                }
                Effect::LogDryRun {
                    step,
                    action,
                    thread_ids,
                } => {
                    tracing::info!(
                        target: "blorp::dry_run",
                        "{}{} {}",
                        step.prefix(),
                        action.name(),
                        thread_ids.join(" ")
                    );
                }
                Effect::Quit => return ControlFlow::Break(Exit::Quit),
                Effect::Fatal(fatal) => return ControlFlow::Break(Exit::Fatal(fatal)),
            }
        }
        ControlFlow::Continue(())
    }
}
