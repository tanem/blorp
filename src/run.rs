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
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::app::{App, Effect, Msg, update};
use crate::jmap::{EmailUpdate, Error, Fatal, Jmap};
use crate::mail::Mailboxes;
use crate::undo::{Replay, Replayed};
use crate::{snooze, sync, ui};

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
/// and says which: starts the `sync` task, sends the launch message, then
/// turns key events, request results, push events, spinner ticks, and the
/// reconcile tick at `App`'s `next_reconcile_at` into messages, redrawing
/// once after everything ready is handled.
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
    let _sync = AbortOnDropHandle::new(tokio::spawn(sync::run(jmap.clone(), tx.clone())));
    let mut executor = Executor {
        jmap,
        tx,
        outbox,
        query: CancellationToken::new(),
    };
    let mut spinner = tokio::time::interval(SPINNER_TICK);
    let mut spinning = false;
    let reconcile = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(reconcile);
    let mut reconcile_at = None;
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
        if reconcile_at != Some(app.next_reconcile_at()) {
            reconcile_at = Some(app.next_reconcile_at());
            let wait = app.next_reconcile_at().duration_since(clock.now());
            let wait = Duration::try_from(wait).unwrap_or(Duration::ZERO);
            reconcile.as_mut().reset(Instant::now() + wait);
        }
        terminal
            .draw(|frame| ui::view(&app, frame))
            .map_err(io::Error::other)?;
        next = tokio::select! {
            key = keys.next() => match key {
                Some(Ok(Event::Key(key))) => ui::key_msg(&app, key),
                Some(Ok(_)) => None,
                Some(Err(error)) => return Err(error),
                None => return Ok(Exit::Quit),
            },
            Some(msg) = rx.recv() => Some(msg),
            _ = spinner.tick(), if spinning => Some(Msg::Tick),
            () = &mut reconcile => {
                reconcile_at = None;
                Some(Msg::ReconcileTick)
            }
        };
    }
}

/// A job for the outbox.
enum Job {
    /// An action's or a redo's `Email/set`.
    Set(EmailUpdate),
    /// An undo.
    Replay(Replay),
    /// Creating a mailbox with this name.
    CreateMailbox(String),
}

/// Starts the outbox task: it runs each job once the job before it has
/// been answered, reports it back as [`Msg::Sent`], [`Msg::Replayed`], or
/// [`Msg::MailboxCreated`], and stops when the sender is dropped.
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
                Job::CreateMailbox(name) => Msg::MailboxCreated {
                    job,
                    result: jmap.create_mailbox(&name).await,
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
    /// Cancels the latest queue query, which the next one supersedes.
    query: CancellationToken,
}

impl<J: Jmap> Executor<J> {
    /// Sets the time, runs `update`, and carries out its effects. A queue
    /// query or reconcile aborts the one before it, if that is still
    /// running. A schedule write that fails cancels the outbox job it was
    /// written for.
    fn dispatch(&mut self, app: &mut App, clock: &impl Clock, msg: Msg) -> ControlFlow<Exit> {
        app.now = clock.now();
        let mut cancelled = None;
        for effect in update(app, msg) {
            match effect {
                Effect::SetEmails { job, .. } | Effect::Replay { job, .. }
                    if cancelled == Some(job) => {}
                Effect::Query { generation, query } => {
                    let jmap = self.jmap.clone();
                    self.supersede(async move {
                        let result = jmap.query(&query).await;
                        Msg::Loaded { generation, result }
                    });
                }
                Effect::Reconcile { generation, query } => {
                    let jmap = self.jmap.clone();
                    self.supersede(async move {
                        let result = jmap.reconcile(&query).await;
                        Msg::Reconciled { generation, result }
                    });
                }
                Effect::SetEmails { job, update } => {
                    let _ = self.outbox.send((job, Job::Set(update)));
                }
                Effect::Replay { job, replay } => {
                    let _ = self.outbox.send((job, Job::Replay(replay)));
                }
                Effect::CreateMailbox { job, name } => {
                    let _ = self.outbox.send((job, Job::CreateMailbox(name)));
                }
                Effect::QuerySnoozed { query } => {
                    let jmap = self.jmap.clone();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let result = jmap.query(&query).await;
                        let _ = tx.send(Msg::SnoozedLoaded { result });
                    });
                }
                Effect::WriteSchedule {
                    job,
                    path,
                    contents,
                } => {
                    if let Err(error) = snooze::save(&path, &contents) {
                        tracing::error!("could not write {}: {error}", path.display());
                        cancelled = job;
                        let error = error.to_string();
                        let _ = self.tx.send(Msg::ScheduleFailed { job, error });
                    }
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

    /// Runs a queue query, aborting the one before it if that is still
    /// running, and sends its result.
    fn supersede(&mut self, query: impl Future<Output = Msg> + Send + 'static) {
        self.query.cancel();
        self.query = CancellationToken::new();
        let cancelled = self.query.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Some(msg) = cancelled.run_until_cancelled(query).await {
                let _ = tx.send(msg);
            }
        });
    }
}
