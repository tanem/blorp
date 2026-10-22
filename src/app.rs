//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::jmap::{Class, EmailQuery, Error, Fatal, QueryResult, Reconciled};
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
    /// When the next reconcile tick is due.
    next_reconcile_at: Timestamp,
    /// The generation of the reconcile in flight, if one is.
    reconciling: Option<u64>,
    /// Whether a reconcile waits for the query in flight to land.
    pending_reconcile: bool,
    /// False from when a request fails despite its retries until one
    /// succeeds.
    connected: bool,
    /// While disconnected, the wait before the probe after the next one
    /// fails.
    probe_wait: SignedDuration,
    /// The retry the query in flight is on, while it is being retried.
    retrying: Option<u32>,
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
    /// `o`: open the thread under the cursor in Fastmail's web UI.
    Open,
    /// Opening a thread in the browser failed.
    OpenFailed { error: String },
    /// `q` or `Ctrl-c`: quit.
    Quit,
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
    /// Open `url` in the browser. On failure, report back with
    /// [`Msg::OpenFailed`].
    Open {
        url: String,
    },
    Quit,
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
            next_reconcile_at: now + RECONCILE_INTERVAL,
            reconciling: None,
            pending_reconcile: false,
            connected: true,
            probe_wait: PROBE_START,
            retrying: None,
        }
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
        generation == self.generation && self.busy()
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
            }
        }
        self.run_pending_reconcile()
    }

    /// Re-runs the Inbox's query with `Mailbox/get`, to bring the queue and
    /// the mailbox list back in line with Fastmail. While a query is in
    /// flight it waits, and runs once that lands, or while disconnected as
    /// the next probe.
    fn reconcile(&mut self) -> Vec<Effect> {
        let Some(query) = self.query.clone() else {
            return Vec::new();
        };
        if self.busy() {
            self.pending_reconcile = true;
            return Vec::new();
        }
        self.pending_reconcile = false;
        self.generation += 1;
        self.retrying = None;
        self.reconciling = Some(self.generation);
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

    /// Whether a queue query or a reconcile is in flight.
    fn busy(&self) -> bool {
        self.loading || self.reconciling.is_some()
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
    /// clears the status message, or says `Reconnected` when the success
    /// reconnects. A failure shows only as a change of the connection
    /// state.
    fn reconciled(&mut self, generation: u64, result: Result<Reconciled, Error>) -> Vec<Effect> {
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        if self.reconciling != Some(generation) {
            return Vec::new();
        }
        self.reconciling = None;
        if result.is_ok() {
            self.status.clear();
        }
        self.record_reply(&result);
        self.retrying = None;
        if let Ok(reconciled) = result {
            self.mailboxes = Mailboxes::from(reconciled.mailboxes);
            self.total = reconciled.result.total;
            self.queue.replace(Thread::from_query(reconciled.result));
        }
        self.run_pending_reconcile()
    }
}

/// Applies `msg` to `app` and returns what the executor should do. Pure.
pub fn update(app: &mut App, msg: Msg) -> Vec<Effect> {
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
        Msg::Open => app.open(),
        Msg::OpenFailed { error } => {
            app.status = format!("Could not open the browser: {error}");
            Vec::new()
        }
        Msg::Quit => vec![Effect::Quit],
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
