//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::Timestamp;
use serde_json::json;

use crate::jmap::{EmailQuery, Error, QueryResult};
use crate::mail::{Mailboxes, Role, Thread};
use crate::queue::Queue;

/// The whole state of a running blorp, owned by the state task.
#[derive(Debug)]
pub struct App {
    /// The current time, set from the clock before each `update`.
    pub now: Timestamp,
    mailboxes: Mailboxes,
    queue: Queue,
    queue_name: String,
    generation: u64,
    loading: bool,
    pending_count: Option<usize>,
    spinner: usize,
    status: String,
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: load the Inbox queue.
    Start,
    Down,
    Up,
    Top,
    Bottom,
    /// A digit typed as part of a count prefix.
    Digit(u8),
    /// Esc or Enter: drops a pending count.
    Cancel,
    Quit,
    /// The spinner's tick while a request is in flight.
    Tick,
    /// A queue query's result, tagged with the generation it was sent for.
    Loaded {
        generation: u64,
        result: Result<QueryResult, Error>,
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
    Quit,
}

impl App {
    pub fn new(mailboxes: Mailboxes, now: Timestamp) -> Self {
        App {
            now,
            mailboxes,
            queue: Queue::default(),
            queue_name: String::new(),
            generation: 0,
            loading: false,
            pending_count: None,
            spinner: 0,
            status: String::new(),
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn queue_name(&self) -> &str {
        &self.queue_name
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
        vec![Effect::Query {
            generation: self.generation,
            query,
        }]
    }
}

/// Applies `msg` to `app` and returns what the executor should do. Pure.
pub fn update(app: &mut App, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Start => {
            let Some(inbox) = app.mailboxes.by_role(Role::Inbox) else {
                app.status = "No Inbox mailbox".into();
                return Vec::new();
            };
            app.queue_name = inbox.name.clone();
            let query = EmailQuery {
                filter: json!({"inMailbox": inbox.id}),
                ascending: false,
            };
            app.load(query)
        }
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
        Msg::Tick => {
            app.spinner = app.spinner.wrapping_add(1);
            Vec::new()
        }
        Msg::Loaded { generation, result } => {
            if generation != app.generation {
                return Vec::new();
            }
            app.loading = false;
            match result {
                Ok(result) => app.queue.replace(Thread::from_query(result)),
                Err(error) => app.status = error.to_string(),
            }
            Vec::new()
        }
    }
}
