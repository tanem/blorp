//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use jiff::Timestamp;
use serde_json::Value;

use crate::filter;
use crate::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, QueryResult, SetResult};
use crate::mail::{Email, Mailboxes, QueueMailbox, Role, Thread};
use crate::prompt::{History as PromptHistory, Key, Prompt};
use crate::queue::Queue;
use crate::undo::{Change, Entry, History, Replay, Replayed, Side, State};

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
    /// What the queue shows, once a filter has been applied.
    shown: Option<Shown>,
    sort: Sort,
    /// The `/` prompt, while it is open.
    filter_prompt: Option<FilterPrompt>,
    filter_history: PromptHistory,
    total: usize,
    generation: u64,
    loading: bool,
    pending_count: Option<usize>,
    spinner: usize,
    status: String,
    dry_run: bool,
    history: History,
    /// The last action that recorded an undo entry, for `.` to repeat.
    last_action: Option<Repeat>,
    /// The last outbox job number given out.
    job: u64,
    /// Actions, undos, and redos sent and not yet answered, oldest first.
    in_flight: Vec<InFlight>,
}

/// The filter a queue shows and what it selects.
#[derive(Debug, Clone)]
struct Shown {
    /// The filter text, trimmed.
    filter: String,
    /// The queue mailbox's path, or `All mail` for `in:any`.
    name: String,
    mailbox: QueueMailbox,
    /// The `Email/query` the queue is loaded with, re-run after a failed
    /// action.
    query: EmailQuery,
}

/// The order a queue sorts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Newest,
    Oldest,
}

impl Sort {
    /// `newest` or `oldest`, as the statusline shows it.
    pub fn name(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
        }
    }
}

/// The open `/` prompt, and the queue as it was when it opened, for Esc to
/// put back.
#[derive(Debug)]
struct FilterPrompt {
    prompt: Prompt,
    shown: Option<Shown>,
    queue: Queue,
    total: usize,
}

/// A job in the outbox: the entry it applies, kept to put back if it
/// fails.
#[derive(Debug)]
struct InFlight {
    job: u64,
    step: Step,
    entry: Entry,
}

/// Whether a job or a dry-run log line is an action, its undo, or its redo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Act,
    Undo,
    Redo,
}

impl Step {
    /// The side of the entry the step moves the emails to.
    fn side(self) -> Side {
        match self {
            Step::Act | Step::Redo => Side::After,
            Step::Undo => Side::Before,
        }
    }

    /// The step's name before an action's, as in the dry-run log.
    pub fn prefix(self) -> &'static str {
        match self {
            Step::Act => "",
            Step::Undo => "undo ",
            Step::Redo => "redo ",
        }
    }

    /// The word an undo or redo message starts with.
    fn done(self) -> &'static str {
        match self {
            Step::Undo => "Undone",
            Step::Redo => "Redone",
            Step::Act => unreachable!("an action's message is its own"),
        }
    }
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

/// An action and the count it was given: what `.` repeats.
#[derive(Debug, Clone, Copy)]
struct Repeat {
    action: Action,
    count: Option<usize>,
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: load the Inbox queue.
    Start,
    /// Show the queue this filter text selects, or report why it selects
    /// none.
    SetFilter(String),
    /// `/`: open the filter prompt holding the current filter.
    OpenFilter,
    /// A key typed at the open prompt.
    Prompt(Key),
    /// `t`: toggle newest and oldest first.
    ToggleSort,
    Down,
    Up,
    Top,
    Bottom,
    /// A digit typed as part of a count prefix.
    Digit(u8),
    /// Esc or Enter: drops a pending count and closes the visual range.
    Cancel,
    Quit,
    /// `e`: archive the selection.
    Archive,
    /// `x`: mark or unmark the row under the cursor, or as many rows as the
    /// count, and move past them.
    Mark,
    /// `X`: clear the marks.
    ClearMarks,
    /// `V`: open a visual range on the row under the cursor, or close the
    /// open one.
    Visual,
    /// `.`: repeat the last action that recorded an undo entry on the
    /// selection, with the count typed before it, or else the count the
    /// action was given.
    Repeat,
    /// `u`: undo the latest action, or as many as the count.
    Undo,
    /// `Ctrl-r`: redo the latest undone action, or as many as the count.
    Redo,
    /// `D`: turn dry run on or off.
    ToggleDryRun,
    /// The spinner's tick while a request is in flight.
    Tick,
    /// A queue query's result, tagged with the generation it was sent for.
    Loaded {
        generation: u64,
        result: Result<QueryResult, Error>,
    },
    /// An action's or a redo's `Email/set` result.
    Sent {
        job: u64,
        result: Result<SetResult, Error>,
    },
    /// An undo job's result.
    Replayed {
        job: u64,
        result: Result<Replayed, Error>,
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
    /// Send an action's or a redo's `Email/set` through the outbox, after
    /// every job before it, and report it back as [`Msg::Sent`].
    SetEmails {
        job: u64,
        update: EmailUpdate,
    },
    /// Send an undo through the outbox, after every job before it, and
    /// report it back as [`Msg::Replayed`].
    Replay {
        job: u64,
        replay: Replay,
    },
    /// Log a dry-run action, undo, or redo to the `blorp::dry_run` target at
    /// info.
    LogDryRun {
        step: Step,
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
            shown: None,
            sort: Sort::Newest,
            filter_prompt: None,
            filter_history: PromptHistory::default(),
            total: 0,
            generation: 0,
            loading: false,
            pending_count: None,
            spinner: 0,
            status: String::new(),
            dry_run: false,
            history: History::default(),
            last_action: None,
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

    /// The queue mailbox's path, or `All mail` for `in:any`.
    pub fn queue_name(&self) -> &str {
        self.shown.as_ref().map_or("", |s| &s.name)
    }

    /// The filter text of the queue shown.
    pub fn filter(&self) -> &str {
        self.shown.as_ref().map_or("", |s| &s.filter)
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// The text at the `/` prompt, while it is open.
    pub fn prompt(&self) -> Option<&str> {
        self.filter_prompt.as_ref().map(|p| p.prompt.text())
    }

    /// The cursor at the `/` prompt, in characters, while it is open.
    pub fn prompt_cursor(&self) -> Option<usize> {
        self.filter_prompt.as_ref().map(|p| p.prompt.cursor())
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

    /// How many actions `u` can undo.
    pub fn undo_depth(&self) -> usize {
        self.history.depth()
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

    /// Shows an action's message, marked `[dry]` in dry run.
    fn report(&mut self, message: impl std::fmt::Display) {
        self.status = if self.dry_run {
            format!("[dry] {message}")
        } else {
            message.to_string()
        };
    }

    /// What filter `text` selects in the current sort, or why it selects
    /// nothing.
    fn select(&self, text: &str) -> Result<Shown, filter::Error> {
        let resolved = filter::parse(text)?.resolve(&self.mailboxes)?;
        Ok(Shown {
            filter: text.trim().into(),
            name: resolved.name,
            mailbox: resolved.mailbox,
            query: EmailQuery {
                filter: resolved.condition,
                ascending: self.sort == Sort::Oldest,
                limit: QUEUE_LIMIT,
            },
        })
    }

    /// Shows the queue filter `text` selects, or reports why it selects
    /// nothing and leaves the queue as it is.
    fn set_filter(&mut self, text: &str) -> Vec<Effect> {
        match self.select(text) {
            Ok(shown) => self.show(shown),
            Err(error) => {
                self.status = error.to_string();
                Vec::new()
            }
        }
    }

    /// Shows `shown`. When its query is not the one the queue was loaded
    /// with, the selection is cleared, the cursor goes to the first row,
    /// and the query is run.
    fn show(&mut self, shown: Shown) -> Vec<Effect> {
        let same = self.shown.as_ref().is_some_and(|s| s.query == shown.query);
        let query = shown.query.clone();
        self.shown = Some(shown);
        if same {
            return Vec::new();
        }
        self.queue.clear_selection();
        self.queue.to_first();
        self.load(query)
    }

    /// Flips the sort and reloads the queue in it, clearing the selection
    /// and putting the cursor on the first row.
    fn toggle_sort(&mut self) -> Vec<Effect> {
        self.sort = match self.sort {
            Sort::Newest => Sort::Oldest,
            Sort::Oldest => Sort::Newest,
        };
        let Some(shown) = &mut self.shown else {
            return Vec::new();
        };
        shown.query.ascending = self.sort == Sort::Oldest;
        let query = shown.query.clone();
        self.queue.clear_selection();
        self.queue.to_first();
        self.load(query)
    }

    /// Opens the `/` prompt holding the current filter, dropping a pending
    /// count and closing the visual range.
    fn open_filter(&mut self) {
        self.take_count();
        self.queue.close_visual();
        self.filter_prompt = Some(FilterPrompt {
            prompt: Prompt::new(self.filter()),
            shown: self.shown.clone(),
            queue: self.queue.clone(),
            total: self.total,
        });
    }

    /// Handles a key at the `/` prompt. Each edit shows the queue the text
    /// selects, if it selects one. Enter keeps it, or for text that selects
    /// nothing puts the earlier queue back and reports why; either way the
    /// text goes into the prompt's history. Esc puts the earlier queue back.
    fn filter_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(open) = &mut self.filter_prompt else {
            return Vec::new();
        };
        match key {
            Key::Enter => {
                let open = self.filter_prompt.take().expect("the prompt is open");
                let text = open.prompt.text();
                self.filter_history.push(text);
                match self.select(text) {
                    Ok(shown) => {
                        self.status.clear();
                        self.show(shown)
                    }
                    Err(error) => {
                        let effects = self.restore(open);
                        self.status = error.to_string();
                        effects
                    }
                }
            }
            Key::Esc => {
                let open = self.filter_prompt.take().expect("the prompt is open");
                self.restore(open)
            }
            key => {
                open.prompt.edit(key, &self.filter_history);
                let text = open.prompt.text().to_string();
                match self.select(&text) {
                    Ok(shown) => self.show(shown),
                    Err(_) => Vec::new(),
                }
            }
        }
    }

    /// Puts back the queue as it was when the prompt opened. When the
    /// prompt changed it, the earlier rows, cursor, and marks come back at
    /// once and the earlier query is run again.
    fn restore(&mut self, open: FilterPrompt) -> Vec<Effect> {
        let query = |shown: &Option<Shown>| shown.as_ref().map(|s| s.query.clone());
        let changed = query(&self.shown) != query(&open.shown);
        self.shown = open.shown;
        if !changed {
            return Vec::new();
        }
        self.queue = open.queue;
        self.total = open.total;
        match query(&self.shown) {
            Some(query) => self.load(query),
            None => Vec::new(),
        }
    }

    fn perform(&mut self, repeat: Repeat) -> Vec<Effect> {
        match repeat.action {
            Action::Archive => self.archive(repeat),
        }
    }

    /// Archives the selection, with the count's rows from the cursor when
    /// there is neither a visual range nor a mark: each thread's emails a
    /// move takes out of the queue mailbox. Closes the visual range.
    fn archive(&mut self, repeat: Repeat) -> Vec<Effect> {
        let rows = self.queue.selection(repeat.count.unwrap_or(1));
        self.queue.close_visual();
        let Some(from) = self.shown.as_ref().map(|s| s.mailbox.clone()) else {
            return Vec::new();
        };
        if rows.is_empty() {
            return Vec::new();
        }
        let Some(archive) = self.mailboxes.by_role(Role::Archive) else {
            self.report("No Archive mailbox");
            return Vec::new();
        };
        if from == QueueMailbox::One(archive.id.clone()) {
            self.report("Already in Archive");
            return Vec::new();
        }
        let mut threads = Vec::new();
        let mut update = EmailUpdate::new();
        for row in rows {
            let thread = &self.queue.threads()[row];
            let before = update.len();
            update.extend(
                thread
                    .emails
                    .iter()
                    .filter(|email| self.mailboxes.moves(email, &from))
                    .filter_map(|email| {
                        let patch = move_patch(email, &from, &archive.id)?;
                        Some((email.id.clone(), patch))
                    }),
            );
            if update.len() > before {
                threads.push(thread.clone());
            }
        }
        if update.is_empty() {
            self.report("Nothing to archive");
            return Vec::new();
        }
        self.act(repeat, threads, update)
    }

    /// Records an action on `touched` as one undo entry and as what `.`
    /// repeats, applies it to the queue, reports it, and sends `update`, or
    /// in dry run logs it.
    fn act(&mut self, repeat: Repeat, touched: Vec<Thread>, update: EmailUpdate) -> Vec<Effect> {
        let action = repeat.action;
        self.last_action = Some(repeat);
        let changes = touched
            .iter()
            .flat_map(|thread| &thread.emails)
            .filter_map(|email| {
                let patch = update.get(&email.id)?;
                let before = State::from(email);
                Some(Change {
                    id: email.id.clone(),
                    after: before.patched(patch),
                    before,
                })
            })
            .collect();
        let entry = self.history.record(action, touched, changes).clone();
        self.apply(&entry, Side::After);
        self.report(format!(
            "{} {}",
            action.done(),
            threads(entry.threads.len())
        ));
        if self.dry_run {
            return vec![log_dry_run(Step::Act, &entry)];
        }
        self.job += 1;
        let job = self.job;
        self.in_flight.push(InFlight {
            job,
            step: Step::Act,
            entry,
        });
        vec![Effect::SetEmails { job, update }]
    }

    /// Undoes or redoes up to the pending count of entries, one job each:
    /// an undo is guarded by an `Email/get`, and a redo sends the action's
    /// patch again. A redo waits until its entry's undo is answered, since
    /// that answer can narrow the entry.
    fn replay(&mut self, step: Step) -> Vec<Effect> {
        let count = self.take_count().unwrap_or(1);
        let mut effects = Vec::new();
        let mut done: Vec<(Action, usize)> = Vec::new();
        let mut restored = Vec::new();
        let mut went_to = None;
        let mut waiting = false;
        for _ in 0..count {
            if step == Step::Redo
                && let Some(next) = self.history.next_redo()
                && self
                    .in_flight
                    .iter()
                    .any(|f| f.step == Step::Undo && f.entry.id == next.id)
            {
                waiting = true;
                break;
            }
            let entry = match step {
                Step::Undo => self.history.undo(),
                _ => self.history.redo(),
            };
            let Some(entry) = entry.cloned() else {
                break;
            };
            let placed = self.apply(&entry, step.side());
            restored.extend(placed.inside);
            went_to = went_to.or(placed.went_to);
            match done.iter_mut().find(|(action, _)| *action == entry.action) {
                Some((_, n)) => *n += entry.threads.len(),
                None => done.push((entry.action, entry.threads.len())),
            }
            if self.dry_run {
                effects.push(log_dry_run(step, &entry));
                continue;
            }
            self.job += 1;
            effects.push(match step {
                Step::Undo => Effect::Replay {
                    job: self.job,
                    replay: entry.undo(),
                },
                _ => Effect::SetEmails {
                    job: self.job,
                    update: entry.redo(),
                },
            });
            self.in_flight.push(InFlight {
                job: self.job,
                step,
                entry,
            });
        }
        if done.is_empty() {
            self.status = String::from(match step {
                _ if waiting => "The undo has not reached Fastmail yet",
                Step::Undo => "Nothing to undo",
                _ => "Nothing to redo",
            });
            return effects;
        }
        if step == Step::Undo
            && let Some(row) = restored
                .iter()
                .filter_map(|id| self.queue.index_of(id))
                .min()
        {
            self.queue.to_line(row + 1);
        }
        let parts: Vec<String> = done
            .iter()
            .map(|(action, n)| format!("{} {}", action.name(), threads(*n)))
            .collect();
        let went_to = went_to
            .filter(|_| step == Step::Undo)
            .and_then(|id| self.mailboxes.by_id(&id))
            .map(|m| format!(" (in {})", m.name))
            .unwrap_or_default();
        self.report(format!("{}: {}{went_to}", step.done(), parts.join(", ")));
        effects
    }

    /// Puts each email `entry` changed in its `side` state in memory, and
    /// moves each of its threads into or out of the queue when that changes
    /// whether it belongs there. A thread belongs in the queue when a move
    /// action would take one of its emails out of the queue mailbox.
    fn apply(&mut self, entry: &Entry, side: Side) -> Placed {
        let ascending = self.sort == Sort::Oldest;
        let mut placed = Placed::default();
        for snapshot in &entry.threads {
            let index = self.queue.index_of(&snapshot.id);
            let base = match index {
                Some(i) => self.queue.threads()[i].clone(),
                None => entry.thread_at(snapshot, side.other()),
            };
            let was = self.belongs(&base);
            let thread = entry.thread_at(&base, side);
            let is = self.belongs(&thread);
            match index {
                Some(i) if was && !is => {
                    self.queue.remove(i);
                    self.total = self.total.saturating_sub(1);
                }
                Some(i) => self.queue.set(i, thread),
                None if is && !was => {
                    self.queue.insert_sorted(thread, ascending);
                    self.total += 1;
                }
                None => {}
            }
            if self.queue.index_of(&snapshot.id).is_some() {
                placed.inside.push(snapshot.id.clone());
            } else if placed.went_to.is_none() {
                placed.went_to = entry.mailbox_gained(snapshot, side);
            }
        }
        placed
    }

    /// Whether `thread` belongs in the queue: a move action would take one
    /// of its emails out of the queue mailbox.
    fn belongs(&self, thread: &Thread) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|Shown { mailbox: from, .. }| {
                thread
                    .emails
                    .iter()
                    .any(|email| self.mailboxes.moves(email, from))
            })
    }

    /// Handles an action's or a redo's `Email/set` result.
    fn sent(&mut self, job: u64, result: Result<SetResult, Error>) -> Vec<Effect> {
        let failure = match result {
            Ok(set) if set.not_updated.is_empty() => None,
            Ok(set) => Some(not_updated(set.not_updated.len())),
            Err(error) => match error.fatal() {
                Some(fatal) => return vec![Effect::Fatal(fatal)],
                None => Some(error.to_string()),
            },
        };
        self.answered(job, failure, Vec::new())
    }

    /// Handles an undo job's result.
    fn replayed(&mut self, job: u64, result: Result<Replayed, Error>) -> Vec<Effect> {
        match result {
            Ok(r) if r.not_updated.is_empty() => self.answered(job, None, r.skipped),
            Ok(r) => self.answered(job, Some(not_updated(r.not_updated.len())), Vec::new()),
            Err(error) => match error.fatal() {
                Some(fatal) => vec![Effect::Fatal(fatal)],
                None => self.answered(job, Some(error.to_string()), Vec::new()),
            },
        }
    }

    /// Finishes job `job`. On failure its change to the queue is rolled
    /// back, its entry dropped, and the failure reported. When an undo
    /// skipped emails changed elsewhere, the partial result is reported and
    /// a redo of the entry leaves those emails alone. Either way the queue's
    /// query is re-run.
    fn answered(&mut self, job: u64, failure: Option<String>, skipped: Vec<String>) -> Vec<Effect> {
        let Some(position) = self.in_flight.iter().position(|f| f.job == job) else {
            return Vec::new();
        };
        let InFlight { step, entry, .. } = self.in_flight.remove(position);
        let n = entry.threads.len();
        if let Some(reason) = failure {
            self.apply(&entry, step.side().other());
            self.history.forget(entry.id);
            self.status = format!(
                "Could not {}{} {}: {reason}",
                step.prefix(),
                entry.action.name(),
                threads(n)
            );
        } else if skipped.is_empty() {
            return Vec::new();
        } else {
            let changed = entry.threads_with(&skipped);
            self.history.narrow(entry.id, &skipped);
            self.status = format!(
                "{}: {} {} of {}, {changed} changed elsewhere",
                step.done(),
                entry.action.name(),
                n - changed,
                threads(n)
            );
        }
        match self.shown.as_ref().map(|s| s.query.clone()) {
            Some(query) => self.load(query),
            None => Vec::new(),
        }
    }
}

/// Where [`App::apply`] left an entry's threads.
#[derive(Default)]
struct Placed {
    /// The threads in the queue afterwards.
    inside: Vec<String>,
    /// The id of a mailbox the first of the threads outside the queue went
    /// into; the others are not named.
    went_to: Option<String>,
}

/// The dry-run log effect for `step` on `entry`.
fn log_dry_run(step: Step, entry: &Entry) -> Effect {
    Effect::LogDryRun {
        step,
        action: entry.action,
        thread_ids: entry.threads.iter().map(|t| t.id.clone()).collect(),
    }
}

/// `1 thread`, `3 threads`.
fn threads(n: usize) -> String {
    match n {
        1 => "1 thread".into(),
        n => format!("{n} threads"),
    }
}

/// The failure reason for `n` emails in `notUpdated`.
fn not_updated(n: usize) -> String {
    match n {
        1 => "1 email not updated".into(),
        n => format!("{n} emails not updated"),
    }
}

/// The patch that moves `email` out of each mailbox of the queue mailbox
/// `from` it is in, other than `to`, and into mailbox `to`, or none when it
/// is in no such mailbox.
fn move_patch(email: &Email, from: &QueueMailbox, to: &str) -> Option<EmailPatch> {
    let mut patch: EmailPatch = email
        .mailbox_ids
        .iter()
        .filter(|id| *id != to && from.contains(id))
        .map(|id| (format!("mailboxIds/{id}"), Value::Null))
        .collect();
    if patch.is_empty() {
        return None;
    }
    patch.insert(format!("mailboxIds/{to}"), Value::Bool(true));
    Some(patch)
}

/// Applies `msg` to `app` and returns what the executor should do. Pure.
pub fn update(app: &mut App, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Start => app.set_filter(""),
        Msg::SetFilter(text) => app.set_filter(&text),
        Msg::OpenFilter => {
            app.open_filter();
            Vec::new()
        }
        Msg::Prompt(key) => app.filter_key(key),
        Msg::ToggleSort => app.toggle_sort(),
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
            app.queue.close_visual();
            Vec::new()
        }
        Msg::Quit => vec![Effect::Quit],
        Msg::Archive => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Archive,
                count,
            })
        }
        Msg::Repeat => {
            let count = app.take_count();
            match app.last_action {
                Some(last) => app.perform(Repeat {
                    count: count.or(last.count),
                    ..last
                }),
                None => Vec::new(),
            }
        }
        Msg::Mark => {
            let rows = app.take_count().unwrap_or(1);
            app.queue.toggle_marks(rows);
            Vec::new()
        }
        Msg::ClearMarks => {
            app.take_count();
            app.queue.clear_marks();
            Vec::new()
        }
        Msg::Visual => {
            app.take_count();
            app.queue.toggle_visual();
            Vec::new()
        }
        Msg::Undo => app.replay(Step::Undo),
        Msg::Redo => app.replay(Step::Redo),
        Msg::ToggleDryRun => {
            app.dry_run = !app.dry_run;
            let mode = if app.dry_run {
                "Dry run on"
            } else {
                "Dry run off"
            };
            app.history.clear();
            app.status = format!("{mode}, undo history cleared");
            Vec::new()
        }
        Msg::Sent { job, result } => app.sent(job, result),
        Msg::Replayed { job, result } => app.replayed(job, result),
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
