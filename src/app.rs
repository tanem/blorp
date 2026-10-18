//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::Timestamp;
use serde_json::{Value, json};

use crate::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, QueryResult, SetResult};
use crate::mail::{Mailboxes, Role, Thread};
use crate::queue::Queue;

/// The most threads a queue holds: the newest, or oldest, this many of the
/// threads that match. The query fetches every email of each thread, so the
/// cap bounds the size and time of the queue fetch.
pub const QUEUE_LIMIT: usize = 500;

/// The whole state of a running blorp, owned by the state task.
#[derive(Debug)]
pub struct App {
    /// The current time, set from the clock before each `update`.
    pub now: Timestamp,
    mailboxes: Mailboxes,
    queue: Queue,
    queue_name: String,
    /// The id of the mailbox the queue shows.
    queue_mailbox: Option<String>,
    /// The `Email/query` the queue was loaded with, which the filter
    /// produced, re-run after a failed action.
    queue_query: Option<EmailQuery>,
    total: usize,
    generation: u64,
    loading: bool,
    pending_count: Option<usize>,
    spinner: usize,
    status: String,
    dry_run: bool,
    /// The last outbox job number given out.
    job: u64,
    /// Actions sent and not yet answered, oldest first.
    in_flight: Vec<InFlight>,
}

/// An action whose `Email/set` is in the outbox: what to put back if it
/// fails.
#[derive(Debug)]
struct InFlight {
    job: u64,
    action: Action,
    /// The row the thread was on.
    index: usize,
    thread: Thread,
}

/// An action on a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Archive,
}

impl Action {
    /// The action's name, as in the dry-run log.
    pub fn name(self) -> &'static str {
        match self {
            Action::Archive => "archive",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Action::Archive => "Archived",
        }
    }
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: load the Inbox queue.
    Start,
    /// Load the queue of the mailbox with this id, newest first.
    Show(String),
    Down,
    Up,
    Top,
    Bottom,
    /// A digit typed as part of a count prefix.
    Digit(u8),
    /// Esc or Enter: drops a pending count.
    Cancel,
    Quit,
    /// `e`: archive the thread under the cursor.
    Archive,
    /// `D`: turn dry run on or off.
    ToggleDryRun,
    /// The spinner's tick while a request is in flight.
    Tick,
    /// A queue query's result, tagged with the generation it was sent for.
    Loaded {
        generation: u64,
        result: Result<QueryResult, Error>,
    },
    /// An outbox job's `Email/set` result.
    Sent {
        job: u64,
        result: Result<SetResult, Error>,
    },
}

/// Something for the executor to do.
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Run a queue query and report it back as [`Msg::Loaded`].
    Query {
        generation: u64,
        query: EmailQuery,
    },
    /// Send an `Email/set` through the outbox, after every job before it,
    /// and report it back as [`Msg::Sent`].
    SetEmails {
        job: u64,
        update: EmailUpdate,
    },
    /// Log a dry-run action to the `blorp::dry_run` target at info.
    LogDryRun {
        action: Action,
        thread_ids: Vec<String>,
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
            queue_name: String::new(),
            queue_mailbox: None,
            queue_query: None,
            total: 0,
            generation: 0,
            loading: false,
            pending_count: None,
            spinner: 0,
            status: String::new(),
            dry_run: false,
            job: 0,
            in_flight: Vec::new(),
        }
    }

    /// The app with dry run on or off, as `--dry-run` sets it at launch.
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// True while actions are logged instead of sent.
    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn queue_name(&self) -> &str {
        &self.queue_name
    }

    /// How many threads match the queue's query, including any beyond
    /// [`QUEUE_LIMIT`].
    pub fn total(&self) -> usize {
        self.total
    }

    /// True while a queue query is in flight.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The count typed so far, before the key that uses it.
    pub fn pending_count(&self) -> Option<usize> {
        self.pending_count
    }

    /// Advances on every tick while a request is in flight.
    pub fn spinner(&self) -> usize {
        self.spinner
    }

    /// The status message on the bottom line.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The pending count, cleared by the key that uses it.
    fn take_count(&mut self) -> Option<usize> {
        self.pending_count.take()
    }

    fn load(&mut self, query: EmailQuery) -> Vec<Effect> {
        self.generation += 1;
        self.loading = true;
        self.queue_query = Some(query.clone());
        vec![Effect::Query {
            generation: self.generation,
            query,
        }]
    }

    /// Shows an action's message, marked `[dry]` in dry run.
    fn report(&mut self, message: impl std::fmt::Display) {
        self.status = if self.dry_run {
            format!("[dry] {message}")
        } else {
            message.to_string()
        };
    }

    fn show(&mut self, mailbox_id: &str) -> Vec<Effect> {
        let Some(mailbox) = self.mailboxes.by_id(mailbox_id) else {
            self.status = "No such mailbox".into();
            return Vec::new();
        };
        self.queue_name = mailbox.name.clone();
        self.queue_mailbox = Some(mailbox.id.clone());
        let query = EmailQuery {
            filter: json!({"inMailbox": mailbox.id}),
            ascending: false,
            limit: QUEUE_LIMIT,
        };
        self.load(query)
    }

    fn archive(&mut self) -> Vec<Effect> {
        self.take_count();
        let (Some(thread), Some(from)) = (self.queue.current(), &self.queue_mailbox) else {
            return Vec::new();
        };
        let Some(archive) = self.mailboxes.by_role(Role::Archive) else {
            self.report("No Archive mailbox");
            return Vec::new();
        };
        if *from == archive.id {
            self.report("Already in Archive");
            return Vec::new();
        }
        let patch = move_patch(from, &archive.id);
        let update: EmailUpdate = thread
            .emails
            .iter()
            .filter(|email| self.mailboxes.moves(email, from))
            .map(|email| (email.id.clone(), patch.clone()))
            .collect();
        if update.is_empty() {
            self.report("Nothing to archive");
            return Vec::new();
        }
        let index = self.queue.cursor();
        let thread = self
            .queue
            .remove_current()
            .expect("a thread is under the cursor");
        self.total = self.total.saturating_sub(1);
        self.act(Action::Archive, index, thread, update)
    }

    /// Reports an action whose queue mutation is done, and sends it, or in
    /// dry run logs it.
    fn act(
        &mut self,
        action: Action,
        index: usize,
        thread: Thread,
        update: EmailUpdate,
    ) -> Vec<Effect> {
        self.report(format!("{} 1 thread", action.done()));
        if self.dry_run {
            return vec![Effect::LogDryRun {
                action,
                thread_ids: vec![thread.id],
            }];
        }
        self.job += 1;
        self.in_flight.push(InFlight {
            job: self.job,
            action,
            index,
            thread,
        });
        vec![Effect::SetEmails {
            job: self.job,
            update,
        }]
    }

    /// Handles an outbox job's result: nothing more on success, and on
    /// failure the queue mutation is rolled back, reported, and the queue's
    /// query re-run.
    fn sent(&mut self, job: u64, result: Result<SetResult, Error>) -> Vec<Effect> {
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        let Some(position) = self.in_flight.iter().position(|f| f.job == job) else {
            return Vec::new();
        };
        let flight = self.in_flight.remove(position);
        let reason = match result {
            Ok(set) if set.not_updated.is_empty() => return Vec::new(),
            Ok(set) => match set.not_updated.len() {
                1 => "1 email not updated".to_string(),
                n => format!("{n} emails not updated"),
            },
            Err(error) => error.to_string(),
        };
        if self.queue.restore(flight.index, flight.thread) {
            self.total += 1;
        }
        self.status = format!("Could not {} 1 thread: {reason}", flight.action.name());
        match self.queue_query.clone() {
            Some(query) => self.load(query),
            None => Vec::new(),
        }
    }
}

/// The patch that moves an email from mailbox `from` to mailbox `to`.
fn move_patch(from: &str, to: &str) -> EmailPatch {
    EmailPatch::from([
        (format!("mailboxIds/{from}"), Value::Null),
        (format!("mailboxIds/{to}"), Value::Bool(true)),
    ])
}

/// Applies `msg` to `app` and returns what the executor should do. Pure.
pub fn update(app: &mut App, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Start => {
            let Some(inbox) = app.mailboxes.by_role(Role::Inbox) else {
                app.status = "No Inbox mailbox".into();
                return Vec::new();
            };
            let id = inbox.id.clone();
            app.show(&id)
        }
        Msg::Show(mailbox_id) => app.show(&mailbox_id),
        Msg::Digit(digit) => {
            let count = app.pending_count.unwrap_or(0);
            if count > 0 || digit > 0 {
                app.pending_count = Some(count.saturating_mul(10).saturating_add(digit.into()));
            }
            Vec::new()
        }
        Msg::Down => {
            let rows = app.take_count().unwrap_or(1);
            app.queue.down(rows);
            Vec::new()
        }
        Msg::Up => {
            let rows = app.take_count().unwrap_or(1);
            app.queue.up(rows);
            Vec::new()
        }
        Msg::Top => {
            app.take_count();
            app.queue.to_first();
            Vec::new()
        }
        Msg::Bottom => {
            match app.take_count() {
                Some(line) => app.queue.to_line(line),
                None => app.queue.to_last(),
            }
            Vec::new()
        }
        Msg::Cancel => {
            app.take_count();
            Vec::new()
        }
        Msg::Quit => vec![Effect::Quit],
        Msg::Archive => app.archive(),
        Msg::ToggleDryRun => {
            app.dry_run = !app.dry_run;
            app.status = if app.dry_run {
                "Dry run on"
            } else {
                "Dry run off"
            }
            .into();
            Vec::new()
        }
        Msg::Sent { job, result } => app.sent(job, result),
        Msg::Tick => {
            app.spinner = app.spinner.wrapping_add(1);
            Vec::new()
        }
        Msg::Loaded { generation, result } => {
            if let Err(error) = &result
                && let Some(fatal) = error.fatal()
            {
                return vec![Effect::Fatal(fatal)];
            }
            if generation != app.generation {
                return Vec::new();
            }
            app.loading = false;
            match result {
                Ok(result) => {
                    app.total = result.total;
                    app.queue.replace(Thread::from_query(result));
                }
                Err(error) => app.status = error.to_string(),
            }
            Vec::new()
        }
    }
}
