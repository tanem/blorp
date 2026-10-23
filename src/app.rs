//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use crate::jmap::{
    Class, EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, Mailbox, QueryResult, Reconciled,
    SetResult,
};
use crate::mail::{self, Mailboxes, Thread};
use crate::queue::Queue;

/// The most threads the queue holds: the newest this many of the mailbox
/// shown's threads. The query fetches every email of each thread, so the cap
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
    /// The id of the mailbox shown, whose threads the queue holds, once
    /// launch has found the Inbox.
    shown: Option<String>,
    /// Whether the queue holds only the unread threads of the mailbox
    /// shown.
    unread_only: bool,
    /// The mailbox and setting a switch or a toggle of unread only loads,
    /// while its query is in flight.
    reloading: Option<Reload>,
    /// The outbox when the queue query in flight was sent.
    loading_outbox: OutboxAt,
    /// The mailbox list, while it is open.
    mailbox_list: Option<MailboxList>,
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
    /// The actions typed and not yet answered, oldest first: the outbox.
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

/// An action in the outbox.
#[derive(Debug)]
struct InFlight {
    job: u64,
    action: Action,
}

/// What an action in the outbox does, with what undoing it in the queue
/// needs if it fails.
#[derive(Debug)]
enum Action {
    /// An archive: the row it took out of the queue, to put back.
    Archive(Row),
    /// A mark read: the ids of the emails it marks, whose dot comes back,
    /// and under unread only the row it took out of the queue, to put
    /// back.
    Read {
        emails: Vec<String>,
        row: Option<Row>,
    },
}

/// A row an action took out of the queue: its thread, and the id of the
/// mailbox shown it was taken from.
#[derive(Debug)]
struct Row {
    thread: Thread,
    from: String,
}

impl Action {
    /// The row the action took out of the queue, if it took one.
    fn row_mut(&mut self) -> Option<&mut Row> {
        match self {
            Action::Archive(row) => Some(row),
            Action::Read { row, .. } => row.as_mut(),
        }
    }

    /// What the action does, as in `Could not archive`.
    fn verb(&self) -> &'static str {
        match self {
            Action::Archive(_) => "archive",
            Action::Read { .. } => "mark read",
        }
    }
}

/// The open mailbox list: the text typed to filter the mailboxes, and the
/// highlighted match.
#[derive(Debug, Clone)]
struct MailboxList {
    typed: String,
    index: usize,
}

/// What the open mailbox list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxChoices {
    /// The text typed to filter the mailboxes.
    pub typed: String,
    /// The path of each mailbox whose path holds the typed text, the Inbox
    /// first and the rest in path order.
    pub paths: Vec<String>,
    /// The highlighted path, when any matches.
    pub index: usize,
}

/// A key at the mailbox list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKey {
    Down,
    Up,
    Choose,
    Cancel,
    Char(char),
    Backspace,
}

/// What a switch or a toggle of unread only loads: the mailbox to show,
/// and whether unread only is on.
#[derive(Debug, Clone)]
struct Reload {
    mailbox: String,
    unread_only: bool,
}

/// The outbox at the time a queue query was sent.
#[derive(Debug, Clone, Copy, Default)]
struct OutboxAt {
    /// The last outbox job number given out.
    job: u64,
    /// Whether an action was in flight.
    busy: bool,
}

/// A reconcile in flight.
#[derive(Debug, Clone, Copy)]
struct Reconciling {
    generation: u64,
    /// The last outbox job number given out when it was sent. The outbox
    /// was idle then, so a later number means an action was sent
    /// meanwhile.
    job: u64,
}

/// `q` waiting for the outbox to drain.
#[derive(Debug)]
struct Quitting {
    /// When it stops waiting.
    at: Timestamp,
    /// The actions that failed while it waited.
    failed: usize,
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: show the Inbox, load its queue, and start the reconcile
    /// tick.
    Start,
    /// `j`: move the cursor down a row.
    Down,
    /// `k`: move the cursor up a row.
    Up,
    /// `e`: archive the thread under the cursor.
    Archive,
    /// `r`: mark the thread under the cursor read.
    MarkRead,
    /// `u`: turn unread only on or off.
    UnreadOnly,
    /// An action's `Email/set` result.
    Sent {
        job: u64,
        result: Result<SetResult, Error>,
    },
    /// `o`: open the thread under the cursor in Fastmail's web UI.
    Open,
    /// `m`: open the list of mailboxes to switch to.
    OpenMailboxList,
    /// A key at the mailbox list.
    ListKey(ListKey),
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
    /// Send an action's `Email/set` through the outbox, after every job
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
            shown: None,
            unread_only: false,
            reloading: None,
            loading_outbox: OutboxAt::default(),
            mailbox_list: None,
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

    /// Marks the account read-only, as the session says, so that no
    /// action is sent.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The name of the mailbox shown, as the mailboxes last fetched have
    /// it.
    pub fn queue_name(&self) -> &str {
        self.shown_mailbox().map_or("", |m| &m.name)
    }

    /// The path of the mailbox shown, as in `Work/Receipts`.
    pub fn queue_path(&self) -> String {
        self.shown_mailbox()
            .map_or_else(String::new, |m| self.mailboxes.path(m))
    }

    /// Whether the queue holds only the unread threads of the mailbox
    /// shown.
    pub fn unread_only(&self) -> bool {
        self.unread_only
    }

    /// The path of the mailbox a switch or a toggle loads while its query
    /// is in flight, and otherwise of the mailbox shown.
    pub fn loading_path(&self) -> String {
        match self
            .reloading
            .as_ref()
            .and_then(|r| self.mailboxes.by_id(&r.mailbox))
        {
            Some(mailbox) => self.mailboxes.path(mailbox),
            None => self.queue_path(),
        }
    }

    fn shown_mailbox(&self) -> Option<&Mailbox> {
        self.mailboxes.by_id(self.shown.as_deref()?)
    }

    /// The mailbox list, while it is open.
    pub fn mailbox_list(&self) -> Option<MailboxChoices> {
        let list = self.mailbox_list.as_ref()?;
        Some(MailboxChoices {
            typed: list.typed.clone(),
            paths: self
                .mailboxes
                .matching(&list.typed)
                .into_iter()
                .map(|m| m.path)
                .collect(),
            index: list.index,
        })
    }

    /// Opens the mailbox list with every mailbox, the one shown highlighted.
    fn open_mailbox_list(&mut self) {
        let index = self
            .mailboxes
            .matching("")
            .iter()
            .position(|m| Some(&m.id) == self.shown.as_ref())
            .unwrap_or(0);
        self.mailbox_list = Some(MailboxList {
            typed: String::new(),
            index,
        });
    }

    /// Handles a key at the mailbox list: a character or Backspace changes
    /// the text the mailboxes are filtered by, highlighting the first
    /// match, Down and Up move the highlight and stop at the ends, Enter
    /// switches to the highlighted mailbox, and Esc closes the list.
    fn mailbox_list_key(&mut self, key: ListKey) -> Vec<Effect> {
        let Some(list) = &mut self.mailbox_list else {
            return Vec::new();
        };
        let matches = self.mailboxes.matching(&list.typed);
        match key {
            ListKey::Char(c) => {
                list.typed.push(c);
                list.index = 0;
            }
            ListKey::Backspace => {
                list.typed.pop();
                list.index = 0;
            }
            ListKey::Down => list.index = (list.index + 1).min(matches.len().saturating_sub(1)),
            ListKey::Up => list.index = list.index.saturating_sub(1),
            ListKey::Cancel => self.mailbox_list = None,
            ListKey::Choose => {
                let Some(chosen) = matches.into_iter().nth(list.index) else {
                    return Vec::new();
                };
                self.mailbox_list = None;
                let unread_only = self.to_show().map_or(self.unread_only, |r| r.unread_only);
                return self.reload(Reload {
                    mailbox: chosen.id,
                    unread_only,
                });
            }
        }
        Vec::new()
    }

    /// What the queue is to show once the queue query in flight lands:
    /// the mailbox and setting of the switch or toggle in flight, or else
    /// those shown.
    fn to_show(&self) -> Option<Reload> {
        self.reloading.clone().or_else(|| {
            Some(Reload {
                mailbox: self.shown.clone()?,
                unread_only: self.unread_only,
            })
        })
    }

    /// Turns unread only on or off, from what the queue is to show next,
    /// and reloads the queue.
    fn toggle_unread_only(&mut self) -> Vec<Effect> {
        let Some(next) = self.to_show() else {
            return Vec::new();
        };
        self.reload(Reload {
            mailbox: next.mailbox,
            unread_only: !next.unread_only,
        })
    }

    /// Loads `reload`'s mailbox with its setting. The queue shows what it
    /// showed until the query lands. A reconcile in flight is dropped,
    /// since the query brings the queue up to date.
    fn reload(&mut self, reload: Reload) -> Vec<Effect> {
        self.status.clear();
        self.query_failed = false;
        self.reconciling = None;
        let query = App::query_for(&reload.mailbox, reload.unread_only);
        self.reloading = Some(reload);
        self.load(query)
    }

    /// The query for the threads of the mailbox with `id`, or with
    /// `unread_only` for those of them with an email in it that lacks
    /// `$seen`.
    fn query_for(id: &str, unread_only: bool) -> EmailQuery {
        let mailbox = json!({"inMailbox": id});
        let filter = if unread_only {
            json!({"operator": "AND", "conditions": [mailbox, {"notKeyword": "$seen"}]})
        } else {
            mailbox
        };
        EmailQuery {
            filter,
            limit: QUEUE_LIMIT,
        }
    }

    /// The query for the mailbox shown, once there is one.
    fn query(&self) -> Option<EmailQuery> {
        let id = self.shown.as_deref()?;
        Some(App::query_for(id, self.unread_only))
    }

    /// Whether the queue holds `thread`: any thread, or under unread only
    /// one with an unread email in the mailbox shown.
    fn holds(&self, thread: &Thread) -> bool {
        !self.unread_only || self.shown.as_deref().is_some_and(|id| thread.unread_in(id))
    }

    /// Runs `query` as the queue query, retried unless it is a probe.
    fn load(&mut self, query: EmailQuery) -> Vec<Effect> {
        self.generation += 1;
        self.loading = true;
        self.loading_outbox = OutboxAt {
            job: self.job,
            busy: !self.in_flight.is_empty(),
        };
        self.retrying = None;
        vec![Effect::Query {
            generation: self.generation,
            query,
            retry: self.connected,
        }]
    }

    /// How many threads are in the mailbox shown, including any beyond
    /// [`QUEUE_LIMIT`].
    pub fn total(&self) -> usize {
        self.total
    }

    /// True while a queue query, the launch's or a switch's, is in flight.
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

    /// Shows the Inbox and runs its query, or says there is no Inbox.
    fn start(&mut self) -> Vec<Effect> {
        let Some(inbox) = self.mailboxes.inbox() else {
            self.status = "No Inbox mailbox".into();
            return Vec::new();
        };
        let query = App::query_for(&inbox.id, self.unread_only);
        self.shown = Some(inbox.id.clone());
        self.load(query)
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

    /// Handles a queue query's result: the queue on success, or the
    /// failure on the bottom line, a transient one as being disconnected.
    /// The cursor stays on its thread when that is still in the queue, and
    /// otherwise goes to the first row. A switch or a toggle that succeeds
    /// shows its mailbox with its setting, a switch to another mailbox with
    /// the cursor on the first row; one that fails leaves the queue as it
    /// was. Threads in the outbox, archived from the mailbox loaded, are
    /// left out, and emails a mark in the outbox marks read are shown read,
    /// or under unread only left out when that leaves no email of their
    /// thread in the mailbox unread. A result that may predate an action,
    /// one sent while an action was in flight or before one was sent, is
    /// shown and a reconcile runs once the outbox drains, so that it never
    /// keeps a thread an archive took out or a dot a mark cleared.
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
        let reloaded = self.reloading.take();
        match result {
            Ok(result) => {
                if let Some(reload) = &reloaded {
                    if Some(&reload.mailbox) != self.shown.as_ref() {
                        self.queue = Queue::default();
                    }
                    self.shown = Some(reload.mailbox.clone());
                    self.unread_only = reload.unread_only;
                }
                self.total = result.total;
                let mut threads = Thread::from_query(result);
                for f in &self.in_flight {
                    if let Action::Read { emails, .. } = &f.action {
                        for thread in &mut threads {
                            thread.set_seen(emails, true);
                        }
                    }
                }
                let before = threads.len();
                threads.retain(|t| !self.in_outbox(t) && self.holds(t));
                self.total = self.total.saturating_sub(before - threads.len());
                self.queue.load(threads);
                if self.loading_outbox.busy || self.loading_outbox.job != self.job {
                    self.pending_reconcile = true;
                }
            }
            Err(error) => {
                self.status = match error.class() {
                    Class::Transient => DISCONNECTED.into(),
                    _ => error.to_string(),
                };
                self.query_failed = reloaded.is_none();
            }
        }
        self.run_pending_reconcile()
    }

    /// Re-runs the shown mailbox's query with `Mailbox/get`, to bring the
    /// queue and the mailboxes back in line with Fastmail. While a query
    /// or an action is in flight it waits, and runs once both are done, or
    /// while disconnected as the next probe. None runs while `q` waits for
    /// the outbox.
    fn reconcile(&mut self) -> Vec<Effect> {
        let Some(query) = self.query() else {
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

    /// Archives the thread under the cursor: its emails in the mailbox
    /// shown that a move takes out go to Archive, and the row leaves at
    /// once. Refused in Archive, Drafts, and Sent, while disconnected, and
    /// for a read-only account.
    fn archive(&mut self) -> Vec<Effect> {
        let Some(thread) = self.queue.current() else {
            return Vec::new();
        };
        let Some(from) = self.shown_mailbox() else {
            return Vec::new();
        };
        match from.role.as_deref() {
            Some("archive") => {
                self.status = format!("Already in {}", self.mailboxes.path(from));
                return Vec::new();
            }
            Some("drafts" | "sent") => {
                self.status = format!("Cannot archive from {}", self.mailboxes.path(from));
                return Vec::new();
            }
            _ => {}
        }
        if self.read_only {
            self.status = "The API token is read-only".into();
            return Vec::new();
        }
        if !self.connected {
            self.status = "Disconnected, action not sent".into();
            return Vec::new();
        }
        let Some(archive) = self.mailboxes.archive() else {
            self.status = "No Archive mailbox".into();
            return Vec::new();
        };
        let patch = [
            (format!("mailboxIds/{}", from.id), Value::Null),
            (format!("mailboxIds/{}", archive.id), Value::Bool(true)),
        ];
        let update: EmailUpdate = thread
            .emails
            .iter()
            .filter(|email| self.mailboxes.moves(email, &from.id))
            .map(|email| (email.id.clone(), patch.clone().into()))
            .collect();
        if update.is_empty() {
            self.status = "Nothing to archive".into();
            return Vec::new();
        }
        let row = self.take_current();
        self.status = "Archived".into();
        self.send(Action::Archive(row), update)
    }

    /// Takes the thread under the cursor out of the queue, the cursor
    /// staying on its row number.
    fn take_current(&mut self) -> Row {
        let from = self.shown.clone().expect("a mailbox shown");
        let thread = self
            .queue
            .remove_current()
            .expect("a thread under the cursor");
        self.total = self.total.saturating_sub(1);
        Row { thread, from }
    }

    /// Marks the thread under the cursor read: every email of it that
    /// lacks `$seen`, in whatever mailbox, gets it, and the dot clears at
    /// once, or under unread only the row leaves at once. Refused while
    /// disconnected and for a read-only account.
    fn mark_read(&mut self) -> Vec<Effect> {
        let Some(thread) = self.queue.current() else {
            return Vec::new();
        };
        let emails: Vec<String> = thread
            .emails
            .iter()
            .filter(|email| !email.seen)
            .map(|email| email.id.clone())
            .collect();
        if emails.is_empty() {
            self.status = "Already read".into();
            return Vec::new();
        }
        if self.read_only {
            self.status = "The API token is read-only".into();
            return Vec::new();
        }
        if !self.connected {
            self.status = "Disconnected, action not sent".into();
            return Vec::new();
        }
        let patch: EmailPatch = [("keywords/$seen".to_string(), Value::Bool(true))].into();
        let update: EmailUpdate = emails
            .iter()
            .map(|id| (id.clone(), patch.clone()))
            .collect();
        let row = if self.unread_only {
            Some(self.take_current())
        } else {
            self.queue.set_seen(&emails, true);
            None
        };
        self.status = "Marked read".into();
        self.send(Action::Read { emails, row }, update)
    }

    /// Puts an action in the outbox and sends its `update`.
    fn send(&mut self, action: Action, update: EmailUpdate) -> Vec<Effect> {
        self.job += 1;
        self.in_flight.push(InFlight {
            job: self.job,
            action,
        });
        vec![Effect::SetEmails {
            job: self.job,
            update,
        }]
    }

    /// Whether `thread` is in the outbox, archived from the mailbox shown.
    fn in_outbox(&self, thread: &Thread) -> bool {
        self.in_flight.iter().any(|f| match &f.action {
            Action::Archive(row) => {
                row.thread.id == thread.id && Some(&row.from) == self.shown.as_ref()
            }
            Action::Read { .. } => false,
        })
    }

    /// Undoes a failed action in the queue: brings back the dot of a
    /// mark's emails, in whichever row they are and in the rows actions in
    /// the outbox took out, and puts back the row the action took out.
    fn undo(&mut self, InFlight { action, .. }: InFlight) {
        let row = match action {
            Action::Archive(row) => Some(row),
            Action::Read { emails, row } => {
                self.queue.set_seen(&emails, false);
                for f in &mut self.in_flight {
                    if let Some(taken) = f.action.row_mut() {
                        taken.thread.set_seen(&emails, false);
                    }
                }
                row
            }
        };
        if let Some(row) = row {
            self.put_back(row);
        }
    }

    /// Puts back a row an action took out, when the queue still shows the
    /// mailbox it was taken from, would hold it, and does not hold it
    /// already.
    fn put_back(&mut self, Row { thread, from }: Row) {
        let held = self.queue.threads().iter().any(|t| t.id == thread.id);
        if Some(&from) == self.shown.as_ref() && self.holds(&thread) && !held {
            self.queue.insert(thread);
            self.total += 1;
        }
    }

    /// Handles an action's `Email/set` result. On failure the action is
    /// undone in the queue, the bottom line says why, and a reconcile runs
    /// once the outbox drains.
    fn sent(&mut self, job: u64, result: Result<SetResult, Error>) -> Vec<Effect> {
        let Some(position) = self.in_flight.iter().position(|f| f.job == job) else {
            return Vec::new();
        };
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
        let done = self.in_flight.remove(position);
        if let Some(reason) = failure {
            self.status = format!("Could not {}: {reason}", done.action.verb());
            self.undo(done);
            self.pending_reconcile = true;
            if let Some(quitting) = &mut self.quitting {
                quitting.failed += 1;
            }
        }
        self.run_pending_reconcile()
    }

    /// Handles an action failing despite its retries, which has
    /// disconnected blorp. The outbox holds back the actions behind it,
    /// and they are dropped: every action in the outbox is undone in the
    /// queue. No reconcile is sent; the next probe is one.
    fn lost(&mut self) -> Vec<Effect> {
        let n = self.in_flight.len();
        let archives = self
            .in_flight
            .iter()
            .filter(|f| matches!(f.action, Action::Archive(_)))
            .count();
        self.status = match (n, archives) {
            (1, _) => format!(
                "{DISCONNECTED}; could not {}",
                self.in_flight[0].action.verb()
            ),
            (n, 0) => format!("{DISCONNECTED}; could not mark {n} threads read"),
            (n, a) if a == n => format!("{DISCONNECTED}; could not archive {n} threads"),
            (n, _) => format!("{DISCONNECTED}; could not send {n} actions"),
        };
        // Oldest first, so that a mark brings back the dot on the row of a
        // later archive before that row is put back.
        while !self.in_flight.is_empty() {
            let lost = self.in_flight.remove(0);
            self.undo(lost);
        }
        if let Some(quitting) = &mut self.quitting {
            quitting.failed += n;
        }
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

    /// `q` has waited long enough: quits, with every action still in the
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
    /// holds an action.
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

    /// Keeps the mailbox list's highlight on a match after the mailboxes
    /// change.
    fn clamp_mailbox_list(&mut self) {
        if let Some(list) = &mut self.mailbox_list {
            let last = self.mailboxes.matching(&list.typed).len().saturating_sub(1);
            list.index = list.index.min(last);
        }
    }

    /// Goes back to the Inbox after a reconcile finds the mailbox shown was
    /// deleted, saying so, and loads it.
    fn shown_deleted(&mut self) -> Vec<Effect> {
        self.queue = Queue::default();
        self.total = 0;
        self.status = "Mailbox no longer exists".into();
        self.shown = None;
        self.start()
    }

    /// Handles a reconcile's result: replaces the mailboxes and the
    /// queue's rows with Fastmail's, keeping the cursor on its thread, and
    /// clears the launch query's failure from the bottom line, or says
    /// `Reconnected` when the success reconnects. When the mailbox shown is
    /// gone from the list, the queue goes back to the Inbox. A result from
    /// before an action was sent is discarded, so that it never brings
    /// back a thread an archive took out or a dot a mark cleared, and the
    /// reconcile runs again once the outbox drains. A failure shows only
    /// as a change of the connection state.
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
                self.clamp_mailbox_list();
                if self.shown_mailbox().is_none() {
                    return self.shown_deleted();
                }
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

/// Applies `msg` to `app`, without the quit that ends `q`'s wait.
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
        Msg::MarkRead => app.mark_read(),
        Msg::UnreadOnly => app.toggle_unread_only(),
        Msg::Sent { job, result } => app.sent(job, result),
        Msg::Open => app.open(),
        Msg::OpenMailboxList => {
            app.open_mailbox_list();
            Vec::new()
        }
        Msg::ListKey(key) => app.mailbox_list_key(key),
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
            let effects = app.reconcile();
            // While disconnected, a probe held back only by an action in
            // flight is tried again after the probe wait, not a minute
            // later.
            if !app.connected && !app.querying() && !app.in_flight.is_empty() {
                app.next_reconcile_at = app.now + app.probe_wait;
            }
            effects
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
