//! The state task: the loop that owns `App`, and the executor that carries
//! out `update`'s effects.

use std::io;
use std::ops::ControlFlow;
use std::time::Duration;

use crossterm::event::Event;
use futures::{Stream, StreamExt};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

use crate::app::{App, Effect, Msg, update};
use crate::jmap::retry::{RETRIES, retrying};
use crate::jmap::{Class, EmailUpdate, Error, Fatal, Jmap};
use crate::mail::Mailboxes;
use crate::{sync, ui};

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

/// A clock that stays at one time, for tests.
#[derive(Clone)]
pub struct TestClock(Timestamp);

impl TestClock {
    pub fn new(now: Timestamp) -> Self {
        TestClock(now)
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        self.0
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

/// The session fetch that starts every launch with a stored token, retried
/// on the usual schedule.
pub async fn connect<T, F, Fut>(fetch: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    retrying(fetch, |retry| {
        tracing::info!("retrying the session fetch ({retry}/{RETRIES})")
    })
    .await
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

/// The line printed after quitting with `n` actions Fastmail had not
/// confirmed.
pub fn unconfirmed(n: usize) -> String {
    match n {
        1 => "1 action was not confirmed by Fastmail; check the Inbox.".into(),
        n => format!("{n} actions were not confirmed by Fastmail; check the Inbox."),
    }
}

/// Runs the state task until a quit or fatal effect or the end of `keys`,
/// and says which: starts the `sync` task and the outbox, sends the launch
/// message, then turns key events, request results, push events, spinner
/// ticks, and the reconcile tick at `App`'s `next_reconcile_at` into
/// messages, redrawing once after everything ready is handled. While `q`
/// waits for the outbox, keys are not read, and the wait ends at `App`'s
/// `quit_at`.
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
    let executor = Executor {
        jmap,
        tx,
        outbox,
        dropped,
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
                Some(Ok(Event::Key(key))) => ui::key_msg(key),
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

/// Starts the outbox task: it sends each archive's `Email/set` once the one
/// before it has been answered, retrying a transient failure on the usual
/// schedule, reports it back as [`Msg::Sent`], and stops when the sender
/// is dropped. After an archive fails transiently despite its retries it
/// sends nothing more until `dropped` reaches that archive's job number,
/// and skips every job numbered up to `dropped`.
fn outbox<J: Jmap>(
    jmap: J,
    tx: UnboundedSender<Msg>,
    mut dropped: watch::Receiver<u64>,
) -> UnboundedSender<(u64, EmailUpdate)> {
    let (outbox, mut jobs) = unbounded_channel::<(u64, EmailUpdate)>();
    tokio::spawn(async move {
        while let Some((job, update)) = jobs.recv().await {
            if job <= *dropped.borrow() {
                continue;
            }
            let result = retrying(
                || jmap.set_emails(&update),
                |retry| tracing::info!("retrying Email/set ({retry}/{RETRIES})"),
            )
            .await;
            let lost = matches!(&result, Err(error) if error.class() == Class::Transient);
            if tx.send(Msg::Sent { job, result }).is_err() {
                break;
            }
            if lost && dropped.wait_for(|&through| through >= job).await.is_err() {
                break;
            }
        }
    });
    outbox
}

/// How a request is retried: on the usual schedule, reporting each retry
/// as [`Msg::Retrying`], or for a probe while disconnected not at all,
/// since the probe backoff spaces probes.
struct Retry {
    tx: UnboundedSender<Msg>,
    generation: u64,
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
                generation: self.generation,
                retry,
            });
        })
        .await
    }
}

struct Executor<J> {
    jmap: J,
    tx: UnboundedSender<Msg>,
    outbox: UnboundedSender<(u64, EmailUpdate)>,
    /// The number of the last outbox job dropped.
    dropped: watch::Sender<u64>,
}

impl<J: Jmap> Executor<J> {
    /// Sets the time, runs `update`, and carries out its effects. A queue
    /// query or reconcile runs in its own task and sends its result back;
    /// an archive goes to the outbox.
    fn dispatch(&self, app: &mut App, clock: &impl Clock, msg: Msg) -> ControlFlow<Exit> {
        app.now = clock.now();
        for effect in update(app, msg) {
            match effect {
                Effect::Query {
                    generation,
                    query,
                    retry,
                } => {
                    let jmap = self.jmap.clone();
                    let retry = self.retry(generation, retry);
                    self.spawn(async move {
                        let result = retry.run(|| jmap.query(&query)).await;
                        Msg::Loaded { generation, result }
                    });
                }
                Effect::Reconcile {
                    generation,
                    query,
                    retry,
                } => {
                    let jmap = self.jmap.clone();
                    let retry = self.retry(generation, retry);
                    self.spawn(async move {
                        let result = retry.run(|| jmap.reconcile(&query)).await;
                        Msg::Reconciled { generation, result }
                    });
                }
                Effect::SetEmails { job, update } => {
                    let _ = self.outbox.send((job, update));
                }
                Effect::DropJobs { through } => {
                    self.dropped.send_replace(through);
                }
                Effect::Open { url } => {
                    if let Err(error) = open::that_detached(&url) {
                        tracing::error!("could not open {url}: {error}");
                        let error = error.to_string();
                        let _ = self.tx.send(Msg::OpenFailed { error });
                    }
                }
                Effect::Quit => return ControlFlow::Break(Exit::Quit),
                Effect::QuitUnconfirmed(n) => return ControlFlow::Break(Exit::Unconfirmed(n)),
                Effect::Fatal(fatal) => return ControlFlow::Break(Exit::Fatal(fatal)),
            }
        }
        ControlFlow::Continue(())
    }

    /// How the request of `generation` is retried.
    fn retry(&self, generation: u64, retry: bool) -> Retry {
        Retry {
            tx: self.tx.clone(),
            generation,
            retry,
        }
    }

    /// Runs `request` in its own task and sends its result.
    fn spawn(&self, request: impl Future<Output = Msg> + Send + 'static) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(request.await);
        });
    }
}
