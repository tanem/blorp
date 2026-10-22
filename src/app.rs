//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use serde_json::Value;

use crate::jmap::SetResult;
use crate::jmap::{Class, EmailQuery, EmailUpdate, Error, Fatal, QueryResult, Reconciled};
use crate::mail::{self, Mailboxes, Thread};
use crate::queue::Queue;

/// The most threads the queue holds: the newest this many of the Inbox's
/// threads. The query fetches every email of each thread, so the cap
/// bounds the size and time of the queue fetch.
pub const QUEUE_LIMIT: usize = 500;

/// How often a reconcile runs, whether or not the push stream is up.
pub const RECONCILE_INTERVAL: SignedDuration = SignedDuration::from_secs(60);

/// The wait before the first probe once disconnected. Each probe that
/// fails doubles it, up to [`PROBE_CAP`].
pub const PROBE_START: SignedDuration = SignedDuration::from_secs(5);

/// The longest wait between probes while disconnected.
pub const PROBE_CAP: SignedDuration = SignedDuration::from_secs(60);

/// How long `q` waits for the outbox to drain.
pub const QUIT_WAIT: SignedDuration = SignedDuration::from_secs(3);

/// The status message on entering the disconnected state.
const DISCONNECTED: &str = "Disconnected from Fastmail";

/// The whole state of a running blorp, owned by the state task.
#[derive(Debug)]
pub struct App {
    /// The current time, set from the clock before each `update`.
    pub now: Timestamp,
    mailboxes: Mailboxes,
    queue: Queue,
    /// The Inbox's `Email/query`, once launch has found the Inbox.
    query: Option<EmailQuery>,
    total: usize,
    generation: u64,
    loading: bool,
    spinner: usize,
    status: String,
    /// Whether the status message is the launch query's failure, which a
    /// reconcile that succeeds clears.
    query_failed: bool,
    /// Whether the session marks the account read-only, so that no action
    /// is sent.
    read_only: bool,
    /// The last outbox job number given out.
    job: u64,
    /// The archives sent and not yet answered, oldest first.
    in_flight: Vec<InFlight>,
    /// When the next reconcile tick is due.
    next_reconcile_at: Timestamp,
    /// The reconcile in flight, if one is.
    reconciling: Option<Reconciling>,
    /// Whether a reconcile waits for the query in flight to land and the
    /// outbox to drain.
    pending_reconcile: bool,
    /// False from when a request fails despite its retries until one
    /// succeeds.
    connected: bool,
    /// While disconnected, the wait before the probe after the next one
    /// fails.
    probe_wait: SignedDuration,
    /// The retry the query in flight is on, while it is being retried.
    retrying: Option<u32>,
    /// While `q` waits for the outbox to drain.
    quitting: Option<Quitting>,
}

/// An archive in the outbox, and the row it took out of the queue, to put
/// back if it fails.
#[derive(Debug)]
struct InFlight {
    job: u64,
    thread: Thread,
}

/// A reconcile in flight.
#[derive(Debug, Clone, Copy)]
struct Reconciling {
    generation: u64,
    /// The last outbox job number given out when it was sent. The outbox
    /// was idle then, so a later number means an archive was sent
    /// meanwhile.
    job: u64,
}

/// `q` waiting for the outbox to drain.
#[derive(Debug)]
struct Quitting {
    /// When it stops waiting.
    at: Timestamp,
    /// The archives that failed while it waited.
    failed: usize,
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: load the Inbox queue and start the reconcile tick.
    Start,
    /// `j`: move the cursor down a row.
    Down,
    /// `k`: move the cursor up a row.
    Up,
    /// `e`: archive the thread under the cursor.
    Archive,
    /// An archive's `Email/set` result.
    Sent {
        job: u64,
        result: Result<SetResult, Error>,
    },
    /// `o`: open the thread under the cursor in Fastmail's web UI.
    Open,
    /// Opening a thread in the browser failed.
    OpenFailed { error: String },
    /// `q` or `Ctrl-c`: quit, once the outbox drains or after
    /// [`QUIT_WAIT`].
    Quit,
    /// `q` has waited [`QUIT_WAIT`] for the outbox, at [`App::quit_at`].
    QuitTimeout,
    /// The spinner's tick while a request is in flight.
    Tick,
    /// An event on the push stream: a change, or a ping.
    Wake,
    /// Fastmail refused to open the push stream for the token.
    StreamRefused(Fatal),
    /// The reconcile tick, at [`App::next_reconcile_at`].
    ReconcileTick,
    /// The queue query or reconcile of `generation` failed transiently and
    /// is being retried for the `retry`th time, counted from 1.
    Retrying { generation: u64, retry: u32 },
    /// A queue query's result, tagged with the generation it was sent for.
    Loaded {
        generation: u64,
        result: Result<QueryResult, Error>,
    },
    /// A reconcile's result, tagged with the generation it was sent for.
    Reconciled {
        generation: u64,
        result: Result<Reconciled, Error>,
    },
}

/// Something for the executor to do.
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Run a queue query and report it back as [`Msg::Loaded`]. With
    /// `retry`, a transient failure is retried; without, while
    /// disconnected, the query is a probe and is sent once.
    Query {
        generation: u64,
        query: EmailQuery,
        retry: bool,
    },
    /// Run a queue query with `Mailbox/get` in its first request, and
    /// report it back as [`Msg::Reconciled`], retried or not as
    /// [`Effect::Query`] is.
    Reconcile {
        generation: u64,
        query: EmailQuery,
        retry: bool,
    },
    /// Send an archive's `Email/set` through the outbox, after every job
    /// before it, and report it back as [`Msg::Sent`].
    SetEmails {
        job: u64,
        update: EmailUpdate,
    },
    /// Drop every outbox job numbered up to `through` that has not been
    /// sent. After a job fails transiently, the outbox sends nothing more
    /// until this comes.
    DropJobs {
        through: u64,
    },
    /// Open `url` in the browser. On failure, report back with
    /// [`Msg::OpenFailed`].
    Open {
        url: String,
    },
    Quit,
    /// Restore the terminal, say how many actions Fastmail did not
    /// confirm, and exit 0.
    QuitUnconfirmed(usize),
    /// Restore the terminal, print the fixed message, and exit 1.
    Fatal(Fatal),
}

impl App {
    pub fn new(mailboxes: Mailboxes, now: Timestamp) -> Self {
        App {
            now,
            mailboxes,
            queue: Queue::default(),
            query: None,
            total: 0,
            generation: 0,
            loading: false,
            spinner: 0,
            status: String::new(),
            query_failed: false,
            read_only: false,
            job: 0,
            in_flight: Vec::new(),
            next_reconcile_at: now + RECONCILE_INTERVAL,
            reconciling: None,
            pending_reconcile: false,
            connected: true,
            probe_wait: PROBE_START,
            retrying: None,
            quitting: None,
        }
    }

    /// Marks the account read-only, as the session says, so that `e` sends
    /// nothing.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The Inbox's name, as the mailbox list last fetched has it.
    pub fn queue_name(&self) -> &str {
        self.mailboxes.inbox().map_or("", |m| &m.name)
    }

    /// How many threads are in the Inbox, including any beyond
    /// [`QUEUE_LIMIT`].
    pub fn total(&self) -> usize {
        self.total
    }

    /// True while the launch's queue query is in flight.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// False while disconnected: from when a request fails despite its
    /// retries until one succeeds.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// The retry the query in flight is on, while it is being retried.
    pub fn retrying(&self) -> Option<u32> {
        self.retrying
    }

    /// Whether the spinner turns: while the queue loads or a request is
    /// being retried.
    pub fn spinning(&self) -> bool {
        self.loading || self.retrying.is_some()
    }

    /// When the state task sends the next [`Msg::ReconcileTick`].
    pub fn next_reconcile_at(&self) -> Timestamp {
        self.next_reconcile_at
    }

    /// Advances on every tick while a request is in flight.
    pub fn spinner(&self) -> usize {
        self.spinner
    }

    /// The status message on the bottom line.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// When `q` stops waiting for the outbox, while it waits.
    pub fn quit_at(&self) -> Option<Timestamp> {
        self.quitting.as_ref().map(|q| q.at)
    }

    /// Builds the Inbox's query from the mailbox list and runs it, or says
    /// there is no Inbox.
    fn start(&mut self) -> Vec<Effect> {
        let Some(inbox) = self.mailboxes.inbox() else {
            self.status = "No Inbox mailbox".into();
            return Vec::new();
        };
        let query = EmailQuery {
            filter: json!({"inMailbox": inbox.id}),
            limit: QUEUE_LIMIT,
        };
        self.query = Some(query.clone());
        self.generation += 1;
        self.loading = true;
        self.retrying = None;
        vec![Effect::Query {
            generation: self.generation,
            query,
            retry: self.connected,
        }]
    }

    /// Records what a request's reply says about the connection. A success
    /// reconnects, saying so, and puts the reconcile tick back a minute
    /// away. A transient failure, which the executor has already retried,
    /// disconnects, saying so, and while disconnected puts the next probe
    /// the backoff away and doubles the backoff. A bug or a fatal failure
    /// changes nothing.
    fn record_reply<T>(&mut self, reply: &Result<T, Error>) {
        match reply {
            Ok(_) if !self.connected => {
                self.connected = true;
                self.next_reconcile_at = self.now + RECONCILE_INTERVAL;
                self.status = "Reconnected".into();
            }
            Err(error) if error.class() == Class::Transient => {
                if self.connected {
                    self.connected = false;
                    self.probe_wait = PROBE_START;
                    self.status = DISCONNECTED.into();
                }
                self.next_reconcile_at = self.now + self.probe_wait;
                self.probe_wait = (self.probe_wait * 2).min(PROBE_CAP);
            }
            _ => {}
        }
    }

    /// While disconnected, starts the probe backoff again, for a probe that
    /// runs at once.
    fn probe_now(&mut self) {
        if !self.connected {
            self.probe_wait = PROBE_START;
        }
    }

    /// Whether a result for `generation` is still to come, so that a retry
    /// reported late, after its result landed, is ignored.
    fn awaits(&self, generation: u64) -> bool {
        generation == self.generation && self.querying()
    }

    /// Handles the launch query's result: the queue on success, or the
    /// failure on the bottom line, a transient one as being disconnected.
    fn loaded(&mut self, generation: u64, result: Result<QueryResult, Error>) -> Vec<Effect> {
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        if generation != self.generation {
            return Vec::new();
        }
        self.record_reply(&result);
        self.loading = false;
        self.retrying = None;
        match result {
            Ok(result) => {
                self.total = result.total;
                self.queue.replace(Thread::from_query(result));
            }
            Err(error) => {
                self.status = match error.class() {
                    Class::Transient => DISCONNECTED.into(),
                    _ => error.to_string(),
                };
                self.query_failed = true;
            }
        }
        self.run_pending_reconcile()
    }

    /// Re-runs the Inbox's query with `Mailbox/get`, to bring the queue and
    /// the mailbox list back in line with Fastmail. While a query or an
    /// archive is in flight it waits, and runs once both are done, or while
    /// disconnected as the next probe. None runs while `q` waits for the
    /// outbox.
    fn reconcile(&mut self) -> Vec<Effect> {
        let Some(query) = self.query.clone() else {
            return Vec::new();
        };
        if self.quitting.is_some() {
            return Vec::new();
        }
        if self.busy() {
            self.pending_reconcile = true;
            return Vec::new();
        }
        self.pending_reconcile = false;
        self.generation += 1;
        self.retrying = None;
        self.reconciling = Some(Reconciling {
            generation: self.generation,
            job: self.job,
        });
        vec![Effect::Reconcile {
            generation: self.generation,
            query,
            retry: self.connected,
        }]
    }

    /// Opens the thread under the cursor in the browser. Nothing is sent to
    /// Fastmail, so this works while disconnected.
    fn open(&self) -> Vec<Effect> {
        let Some(thread) = self.queue.current() else {
            return Vec::new();
        };
        vec![Effect::Open {
            url: mail::web_url(self.queue_name(), thread),
        }]
    }

    /// Archives the thread under the cursor: its emails in the Inbox that
    /// a move takes out go to Archive, and the row leaves at once.
    /// Refused while disconnected and for a read-only account.
    fn archive(&mut self) -> Vec<Effect> {
        let Some(thread) = self.queue.current() else {
            return Vec::new();
        };
        if self.read_only {
            self.status = "The API token is read-only".into();
            return Vec::new();
        }
        if !self.connected {
            self.status = "Disconnected, action not sent".into();
            return Vec::new();
        }
        let (Some(inbox), Some(archive)) = (self.mailboxes.inbox(), self.mailboxes.archive())
        else {
            self.status = "No Archive mailbox".into();
            return Vec::new();
        };
        let patch = [
            (format!("mailboxIds/{}", inbox.id), Value::Null),
            (format!("mailboxIds/{}", archive.id), Value::Bool(true)),
        ];
        let update: EmailUpdate = thread
            .emails
            .iter()
            .filter(|email| self.mailboxes.moves(email, &inbox.id))
            .map(|email| (email.id.clone(), patch.clone().into()))
            .collect();
        if update.is_empty() {
            self.status = "Nothing to archive".into();
            return Vec::new();
        }
        let thread = self
            .queue
            .remove_current()
            .expect("a thread under the cursor");
        self.total = self.total.saturating_sub(1);
        self.job += 1;
        self.in_flight.push(InFlight {
            job: self.job,
            thread,
        });
        self.status = "Archived".into();
        vec![Effect::SetEmails {
            job: self.job,
            update,
        }]
    }

    /// Puts back the row an archive took out of the queue.
    fn put_back(&mut self, thread: Thread) {
        self.queue.insert(thread);
        self.total += 1;
    }

    /// Handles an archive's `Email/set` result. On failure the row goes
    /// back, the bottom line says why, and a reconcile runs once the outbox
    /// drains.
    fn sent(&mut self, job: u64, result: Result<SetResult, Error>) -> Vec<Effect> {
        self.record_reply(&result);
        let failure = match result {
            Ok(set) if set.not_updated.is_empty() => None,
            Ok(set) => Some(match set.not_updated.len() {
                1 => "1 email not updated".to_string(),
                n => format!("{n} emails not updated"),
            }),
            Err(error) => match error.class() {
                Class::Fatal(fatal) => return vec![Effect::Fatal(fatal)],
                Class::Transient => return self.lost(),
                Class::Bug => Some(error.to_string()),
            },
        };
        let Some(position) = self.in_flight.iter().position(|f| f.job == job) else {
            return Vec::new();
        };
        let InFlight { thread, .. } = self.in_flight.remove(position);
        if let Some(reason) = failure {
            self.put_back(thread);
            self.status = format!("Could not archive: {reason}");
            self.pending_reconcile = true;
            if let Some(quitting) = &mut self.quitting {
                quitting.failed += 1;
            }
        }
        self.run_pending_reconcile()
    }

    /// Handles an archive failing despite its retries, which has
    /// disconnected blorp. The outbox holds back the archives behind it,
    /// and they are dropped: every row in the outbox goes back. No
    /// reconcile is sent; the next probe is one.
    fn lost(&mut self) -> Vec<Effect> {
        let n = self.in_flight.len();
        while let Some(InFlight { thread, .. }) = self.in_flight.pop() {
            self.put_back(thread);
        }
        if let Some(quitting) = &mut self.quitting {
            quitting.failed += n;
        }
        self.status = match n {
            1 => format!("{DISCONNECTED}; could not archive"),
            n => format!("{DISCONNECTED}; could not archive {n} threads"),
        };
        vec![Effect::DropJobs { through: self.job }]
    }

    /// `q`: quits at once when the outbox is empty, and otherwise waits up
    /// to [`QUIT_WAIT`] for it to drain.
    fn quit(&mut self) -> Vec<Effect> {
        if self.in_flight.is_empty() {
            return vec![Effect::Quit];
        }
        if self.quitting.is_none() {
            self.quitting = Some(Quitting {
                at: self.now + QUIT_WAIT,
                failed: 0,
            });
            self.status = match self.in_flight.len() {
                1 => "Waiting for Fastmail to confirm 1 action".into(),
                n => format!("Waiting for Fastmail to confirm {n} actions"),
            };
        }
        Vec::new()
    }

    /// The quit effect once the outbox has drained while `q` waits.
    fn drained(&self) -> Option<Effect> {
        let quitting = self.quitting.as_ref()?;
        if !self.in_flight.is_empty() {
            return None;
        }
        Some(quit_with(quitting.failed))
    }

    /// `q` has waited long enough: quits, with every archive still in the
    /// outbox and every one that failed meanwhile unconfirmed.
    fn quit_timeout(&self) -> Vec<Effect> {
        let Some(quitting) = &self.quitting else {
            return Vec::new();
        };
        vec![quit_with(quitting.failed + self.in_flight.len())]
    }

    /// Whether a queue query or a reconcile is in flight.
    fn querying(&self) -> bool {
        self.loading || self.reconciling.is_some()
    }

    /// Whether a queue query or a reconcile is in flight, or the outbox
    /// holds an archive.
    fn busy(&self) -> bool {
        self.querying() || !self.in_flight.is_empty()
    }

    /// Runs the reconcile that was waiting, once nothing holds it back.
    /// While disconnected it waits for the next probe instead, which runs
    /// as it.
    fn run_pending_reconcile(&mut self) -> Vec<Effect> {
        if self.busy() || !self.connected || !self.pending_reconcile {
            return Vec::new();
        }
        self.reconcile()
    }

    /// Handles a reconcile's result: replaces the mailbox list and the
    /// queue's rows with Fastmail's, keeping the cursor on its thread, and
    /// clears the launch query's failure from the bottom line, or says
    /// `Reconnected` when the success reconnects. A result from before an
    /// archive was sent is discarded, so that it never brings back a thread
    /// the archive took out, and the reconcile runs again once the outbox
    /// drains. A failure shows only as a change of the connection state.
    fn reconciled(&mut self, generation: u64, result: Result<Reconciled, Error>) -> Vec<Effect> {
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        let Some(reconciling) = self.reconciling.take_if(|r| r.generation == generation) else {
            return Vec::new();
        };
        if result.is_ok() && self.query_failed {
            self.status.clear();
            self.query_failed = false;
        }
        self.record_reply(&result);
        self.retrying = None;
        match result {
            Ok(_) if reconciling.job != self.job => self.pending_reconcile = true,
            Ok(reconciled) => {
                self.mailboxes = Mailboxes::from(reconciled.mailboxes);
                self.total = reconciled.result.total;
                self.queue.replace(Thread::from_query(reconciled.result));
            }
            Err(_) => {}
        }
        self.run_pending_reconcile()
    }
}

/// The quit effect with `unconfirmed` actions Fastmail did not confirm.
fn quit_with(unconfirmed: usize) -> Effect {
    match unconfirmed {
        0 => Effect::Quit,
        n => Effect::QuitUnconfirmed(n),
    }
}

/// Applies `msg` to `app` and returns what the executor should do. Pure.
/// While `q` waits for the outbox, the message that drains it ends with
/// the quit.
pub fn update(app: &mut App, msg: Msg) -> Vec<Effect> {
    let mut effects = handle(app, msg);
    let exits = effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::Quit | Effect::QuitUnconfirmed(_) | Effect::Fatal(_)
        )
    });
    if !exits {
        effects.extend(app.drained());
    }
    effects
}

fn handle(app: &mut App, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Start => {
            app.next_reconcile_at = app.now + RECONCILE_INTERVAL;
            app.start()
        }
        Msg::Down => {
            app.queue.down();
            Vec::new()
        }
        Msg::Up => {
            app.queue.up();
            Vec::new()
        }
        Msg::Archive => app.archive(),
        Msg::Sent { job, result } => app.sent(job, result),
        Msg::Open => app.open(),
        Msg::OpenFailed { error } => {
            app.status = format!("Could not open the browser: {error}");
            Vec::new()
        }
        Msg::Quit => app.quit(),
        Msg::QuitTimeout => app.quit_timeout(),
        Msg::Tick => {
            app.spinner = app.spinner.wrapping_add(1);
            Vec::new()
        }
        Msg::Wake => {
            app.probe_now();
            app.reconcile()
        }
        Msg::StreamRefused(fatal) => vec![Effect::Fatal(fatal)],
        Msg::ReconcileTick => {
            app.next_reconcile_at = app.now + RECONCILE_INTERVAL;
            app.reconcile()
        }
        Msg::Retrying { generation, retry } => {
            if app.awaits(generation) {
                app.retrying = Some(retry);
            }
            Vec::new()
        }
        Msg::Loaded { generation, result } => app.loaded(generation, result),
        Msg::Reconciled { generation, result } => app.reconciled(generation, result),
    }
}
