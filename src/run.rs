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
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::app::{App, Effect, Msg, Source, update};
use crate::jmap::retry::{RETRIES, retrying};
use crate::jmap::{Class, EmailUpdate, Error, Fatal, Jmap};
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
    /// The user quit with this many actions Fastmail had not confirmed
    /// after the wait for the outbox; the caller restores the terminal and
    /// says so.
    Unconfirmed(usize),
    /// Fastmail refused the token; the caller restores the terminal and
    /// prints the message.
    Fatal(Fatal),
}

/// The launch steps that need Fastmail before the TUI starts: `Mailbox/get`,
/// retried on the usual schedule, giving the app the state task starts
/// from.
pub async fn launch<J: Jmap>(jmap: &J, now: Timestamp) -> Result<App, Error> {
    let mailboxes = retrying(
        || jmap.get_mailboxes(),
        |retry| tracing::info!("retrying Mailbox/get ({retry}/{RETRIES})"),
    )
    .await?;
    Ok(App::new(Mailboxes::from(mailboxes), now))
}

/// The unconfirmed line printed after quitting with `n` actions Fastmail
/// had not confirmed.
pub fn unconfirmed(n: usize) -> String {
    match n {
        1 => "1 action was not confirmed by Fastmail; check the Inbox.".into(),
        n => format!("{n} actions were not confirmed by Fastmail; check the Inbox."),
    }
}

/// Runs the state task until a quit or fatal effect or the end of `keys`,
/// and says which: starts the `sync` task, sends the launch message, then
/// turns key events, request results, push events, spinner ticks, and the
/// reconcile tick at `App`'s `next_reconcile_at` into messages, redrawing
/// once after everything ready is handled. While `q` waits for the
/// outbox, keys are not read, and the wait ends at `App`'s `quit_at`.
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
    let (dropped, dropped_rx) = watch::channel(0);
    let outbox = outbox(jmap.clone(), tx.clone(), dropped_rx);
    let _sync = AbortOnDropHandle::new(tokio::spawn(sync::run(jmap.clone(), tx.clone())));
    let mut executor = Executor {
        jmap,
        tx,
        outbox,
        dropped,
        query: CancellationToken::new(),
    };
    let mut spinner = tokio::time::interval(SPINNER_TICK);
    let mut spinning = false;
    let reconcile = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(reconcile);
    let mut reconcile_at = None;
    let quit = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(quit);
    let mut quit_at = None;
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
        if app.spinning() && !spinning {
            spinner.reset();
        }
        spinning = app.spinning();
        if reconcile_at != Some(app.next_reconcile_at()) {
            reconcile_at = Some(app.next_reconcile_at());
            let wait = app.next_reconcile_at().duration_since(clock.now());
            let wait = Duration::try_from(wait).unwrap_or(Duration::ZERO);
            reconcile.as_mut().reset(Instant::now() + wait);
        }
        if quit_at != app.quit_at() {
            quit_at = app.quit_at();
            if let Some(at) = quit_at {
                let wait = Duration::try_from(at.duration_since(clock.now())).unwrap_or_default();
                quit.as_mut().reset(Instant::now() + wait);
            }
        }
        terminal
            .draw(|frame| ui::view(&app, frame))
            .map_err(io::Error::other)?;
        next = tokio::select! {
            key = keys.next(), if quit_at.is_none() => match key {
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
            () = &mut quit, if quit_at.is_some() => Some(Msg::QuitTimeout),
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
/// been answered, retrying a transient failure, reports it back as
/// [`Msg::Sent`], [`Msg::Replayed`], or [`Msg::MailboxCreated`], and stops
/// when the sender is dropped. After a job fails transiently despite its
/// retries it sends nothing more until `dropped` reaches that job's
/// number, and skips every job numbered up to `dropped`.
fn outbox<J: Jmap>(
    jmap: J,
    tx: UnboundedSender<Msg>,
    mut dropped: watch::Receiver<u64>,
) -> UnboundedSender<(u64, Job)> {
    let (outbox, mut jobs) = unbounded_channel::<(u64, Job)>();
    tokio::spawn(async move {
        while let Some((job, work)) = jobs.recv().await {
            if job <= *dropped.borrow() {
                continue;
            }
            let retry = Retry {
                tx: tx.clone(),
                from: Source::Job(job),
                retry: true,
            };
            let msg = match work {
                Job::Set(update) => Msg::Sent {
                    job,
                    result: retry.run(|| jmap.set_emails(&update)).await,
                },
                Job::Replay(replay) => Msg::Replayed {
                    job,
                    result: replay_emails(&jmap, &replay, &retry).await,
                },
                Job::CreateMailbox(name) => Msg::MailboxCreated {
                    job,
                    result: retry.run(|| jmap.create_mailbox(&name)).await,
                },
            };
            let lost = match &msg {
                Msg::Sent { result, .. } => transient(result),
                Msg::Replayed { result, .. } => transient(result),
                Msg::MailboxCreated { result, .. } => transient(result),
                _ => false,
            };
            if tx.send(msg).is_err() {
                break;
            }
            if lost && dropped.wait_for(|&through| through >= job).await.is_err() {
                break;
            }
        }
    });
    outbox
}

/// Whether `result` is a transient failure.
fn transient<T>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(error) if error.class() == Class::Transient)
}

/// `Email/get` on the replay's emails, then one `Email/set` for those still
/// as expected, or none when no email is, each retried on its own.
async fn replay_emails<J: Jmap>(
    jmap: &J,
    replay: &Replay,
    retry: &Retry,
) -> Result<Replayed, Error> {
    let ids = replay.ids();
    let got = retry.run(|| jmap.get_emails(&ids)).await?;
    let (update, skipped) = replay.split(&got);
    let not_updated = if update.is_empty() {
        Vec::new()
    } else {
        retry.run(|| jmap.set_emails(&update)).await?.not_updated
    };
    Ok(Replayed {
        skipped,
        not_updated,
    })
}

/// How a request is retried: on the usual schedule, reporting each retry
/// as [`Msg::Retrying`], or for a probe while disconnected not at all,
/// since the probe backoff spaces probes.
struct Retry {
    tx: UnboundedSender<Msg>,
    from: Source,
    /// False for a probe.
    retry: bool,
}

impl Retry {
    async fn run<T, F, Fut>(&self, mut call: F) -> Result<T, Error>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        if !self.retry {
            return call().await;
        }
        retrying(call, |retry| {
            let _ = self.tx.send(Msg::Retrying {
                from: self.from,
                retry,
            });
        })
        .await
    }
}

struct Executor<J> {
    jmap: J,
    tx: UnboundedSender<Msg>,
    outbox: UnboundedSender<(u64, Job)>,
    /// The number of the last outbox job dropped.
    dropped: watch::Sender<u64>,
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
            let policy = |from, retry| Retry {
                tx: self.tx.clone(),
                from,
                retry,
            };
            match effect {
                Effect::SetEmails { job, .. } | Effect::Replay { job, .. }
                    if cancelled == Some(job) => {}
                Effect::Query {
                    generation,
                    query,
                    retry,
                } => {
                    let jmap = self.jmap.clone();
                    let retry = policy(Source::Query(generation), retry);
                    self.supersede(async move {
                        let result = retry.run(|| jmap.query(&query)).await;
                        Msg::Loaded { generation, result }
                    });
                }
                Effect::Reconcile {
                    generation,
                    query,
                    snoozed,
                    retry,
                } => {
                    let jmap = self.jmap.clone();
                    let retry = policy(Source::Query(generation), retry);
                    self.supersede(async move {
                        let result = retry.run(|| jmap.reconcile(&query, snoozed.as_ref())).await;
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
                Effect::QuerySnoozed { query, retry } => {
                    let jmap = self.jmap.clone();
                    let tx = self.tx.clone();
                    let retry = policy(Source::SnoozeCheck, retry);
                    tokio::spawn(async move {
                        let result = retry.run(|| jmap.query(&query)).await;
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
                Effect::DropJobs { through } => {
                    self.dropped.send_replace(through);
                }
                Effect::Quit => return ControlFlow::Break(Exit::Quit),
                Effect::QuitUnconfirmed(n) => return ControlFlow::Break(Exit::Unconfirmed(n)),
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
