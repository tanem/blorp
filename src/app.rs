//! The application state and `update`, the pure core every message goes
//! through (ADR 0001).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use crate::command::{self, Command};
use crate::filter;
use crate::jmap::{self, Class, SetResult};
use crate::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, QueryResult, Reconciled};
use crate::mail::{Email, Mailboxes, QueueMailbox, Role, SNOOZE_MAILBOX, Thread};
use crate::prompt::{History as PromptHistory, Key, Prompt};
use crate::queue::Queue;
use crate::snooze::{self, Preset, Schedule, When};
use crate::undo::{Change, Entry, History, Replay, Replayed, ScheduleChange, Side, State};

/// The most threads a queue holds: the newest, or oldest, this many of the
/// threads that match. The query fetches every email of each thread, so the
/// cap bounds the size and time of the queue fetch.
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

/// The id dry run gives the snooze mailbox it pretends to create.
const DRY_RUN_SNOOZE_MAILBOX: &str = "blorp-dry-run-snooze";

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
    /// The `:` prompt, while it is open.
    command_prompt: Option<CommandPrompt>,
    command_history: PromptHistory,
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
    /// When the next reconcile tick is due.
    next_reconcile_at: Timestamp,
    /// The reconcile in flight, if one is.
    reconciling: Option<Reconciling>,
    /// A reconcile waiting for the query in flight to land and the outbox
    /// to drain.
    pending_reconcile: Option<Trigger>,
    /// The local time zone, which snooze times are in.
    tz: TimeZone,
    /// The snooze schedule, as last written to its file.
    schedule: Schedule,
    /// The schedule dry-run actions change, while dry run is on, so that
    /// nothing they do reaches the file.
    dry_schedule: Option<Schedule>,
    /// Where the schedule is written. Without one, snooze is refused.
    schedule_file: Option<PathBuf>,
    /// The `z` picker, while it is open.
    snooze_picker: Option<SnoozePicker>,
    /// The `m` picker, while it is open.
    move_picker: Option<MovePicker>,
    /// The snooze mailbox being created, and the snooze waiting for it.
    creating: Option<Creating>,
    /// Whether the due-snooze check waits for the first queue to load.
    due_at_launch: bool,
    /// The last outbox job number given out when the launch check's query
    /// was sent, while it is in flight.
    checking: Option<u64>,
    /// The returned threads the latest queue query fetched by id.
    extra: Extra,
    /// False from when a request fails despite its retries until one
    /// succeeds.
    connected: bool,
    /// While disconnected, the wait before the probe after the next one
    /// fails.
    probe_wait: SignedDuration,
    /// The requests being retried, and the retry each is on.
    retrying: BTreeMap<Source, u32>,
    /// The filter and sort shown before the query in flight, which loads a
    /// new one, to go back to if it fails.
    previous: Option<Previous>,
    /// While `q` waits for the outbox to drain.
    quitting: Option<Quitting>,
}

/// A request the executor may retry, as [`Msg::Retrying`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// The queue query or reconcile of this generation.
    Query(u64),
    /// The outbox job with this number.
    Job(u64),
    /// The due-snooze check's query at launch.
    SnoozeCheck,
}

/// The filter and sort a queue showed before a new one was asked for.
#[derive(Debug)]
struct Previous {
    shown: Option<Shown>,
    sort: Sort,
}

/// `q` waiting for the outbox to drain.
#[derive(Debug)]
struct Quitting {
    /// When it stops waiting.
    at: Timestamp,
    /// The jobs that failed while it waited.
    failed: usize,
}

/// The returned threads a queue query fetched by id, and the last outbox
/// job number given out when it was sent.
#[derive(Debug, Default)]
struct Extra {
    threads: Vec<String>,
    job: u64,
}

/// The open `z` picker: the preset Enter chooses, and the count typed
/// before `z`.
#[derive(Debug, Clone, Copy)]
struct SnoozePicker {
    index: usize,
    count: Option<usize>,
}

/// What the open snooze picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnoozeChoices {
    /// Each preset and when it returns, as the picker lists them.
    pub presets: Vec<(Preset, String)>,
    /// The preset Enter chooses.
    pub index: usize,
    /// How many threads the snooze applies to.
    pub threads: usize,
}

/// The open `m` picker: the text typed to filter the mailboxes, the match
/// Enter chooses, and the count typed before `m`.
#[derive(Debug, Clone)]
struct MovePicker {
    typed: String,
    index: usize,
    count: Option<usize>,
}

/// What the open move picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveChoices {
    /// The text typed to filter the mailboxes.
    pub typed: String,
    /// The path of each mailbox the typed text matches, sorted.
    pub paths: Vec<String>,
    /// The path Enter chooses, when any matches.
    pub index: usize,
    /// How many threads the move applies to.
    pub threads: usize,
}

/// The outbox job creating the snooze mailbox, and the threads to snooze
/// once it exists.
#[derive(Debug)]
struct Creating {
    job: u64,
    thread_ids: Vec<String>,
    repeat: Repeat,
}

/// A key at the `z` or `m` picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKey {
    Down,
    Up,
    Choose,
    Cancel,
    Char(char),
    Backspace,
}

/// What asked for a reconcile. `Refresh` orders after `Auto`, so the
/// greater of two triggers waiting for one reconcile is the one it runs
/// as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Trigger {
    /// A push event, the tick, or the end of an action that failed or
    /// only partly applied. Ignored in dry run, and silent when it fails.
    Auto,
    /// `R` or `:refresh`: shows the spinner and reports a failure. In dry
    /// run it resets the queue to Fastmail's and clears the undo history.
    Refresh,
}

/// A reconcile in flight.
#[derive(Debug, Clone, Copy)]
struct Reconciling {
    generation: u64,
    trigger: Trigger,
    /// The last outbox job number given out when it was sent. The outbox
    /// was idle then, so a later number means a job was sent meanwhile.
    job: u64,
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
    /// Whether the filter selects the whole queue mailbox, so that the
    /// queue holds every thread with an email a move would take out of it.
    plain: bool,
}

/// The order a queue sorts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Newest,
    Oldest,
}

impl Sort {
    /// Whether the query sorts oldest first.
    fn ascending(self) -> bool {
        self == Sort::Oldest
    }

    /// `newest` or `oldest`, as the statusline shows it.
    pub fn name(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
        }
    }
}

/// What a queue sorts by, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// The later of `receivedAt` and `returnedAt`, oldest first when
    /// ascending.
    Received { ascending: bool },
    /// The snooze mailbox's order: the return time, soonest first when
    /// ascending. A thread with no schedule entry is due now and sorts
    /// first either way.
    Return { ascending: bool },
}

impl Order {
    /// Whether the lowest key sorts first.
    fn ascending(self) -> bool {
        match self {
            Order::Received { ascending } | Order::Return { ascending } => ascending,
        }
    }

    /// Sorts `threads` in this order, keeping the order between ties.
    fn sort(self, threads: &mut [Thread], schedule: &Schedule) {
        if self.ascending() {
            threads.sort_by_key(|t| self.key(schedule, t));
        } else {
            threads.sort_by_key(|t| Reverse(self.key(schedule, t)));
        }
    }

    /// Where `thread` sorts, by its entry in `schedule`.
    fn key(self, schedule: &Schedule, thread: &Thread) -> Timestamp {
        let entry = schedule.get(&thread.id);
        match self {
            Order::Received { .. } => {
                let received = thread.hit().received_at;
                match entry.and_then(|e| e.returned_at) {
                    Some(returned) => received.max(returned),
                    None => received,
                }
            }
            Order::Return { ascending } => match entry {
                Some(entry) => entry.return_at,
                None if ascending => Timestamp::MIN,
                None => Timestamp::MAX,
            },
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
    /// Whether an edit has run a query, which changes the rows.
    queried: bool,
}

/// The open `:` prompt, the count typed before the colon, and the
/// completion Tab has made.
#[derive(Debug)]
struct CommandPrompt {
    prompt: Prompt,
    count: Option<usize>,
    completion: Option<Completion>,
}

/// The candidates for a word at the `:` prompt, which one fills it, and
/// the characters it fills.
#[derive(Debug)]
struct Completion {
    candidates: Vec<String>,
    index: usize,
    range: Range<usize>,
}

/// What keys go to: normal mode, the visual range, or a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Filter,
    Command,
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

/// An action on a thread, or the automatic return of a snoozed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Archive,
    /// `d` or `:delete`: move to Trash.
    Delete,
    /// `!` or `:spam`: move to Junk.
    Spam,
    /// `m` or `:move`: move to the mailbox with this id.
    Move(String),
    /// Snooze until the time given, or in the snooze mailbox, reschedule.
    Snooze(Timestamp),
    /// `:unsnooze`: return snoozed threads now.
    Unsnooze,
    /// `f` or `:flag`: unflag every email of the selection when any is
    /// flagged, otherwise flag every email. It is not on the undo history:
    /// the [`Action::Flag`] it resolves to is.
    ToggleFlag,
    /// `r`: mark every email of the selection read when any is unread,
    /// otherwise unread. It is not on the undo history: the
    /// [`Action::Read`] it resolves to is.
    ToggleRead,
    /// Flag every email of the selection's threads, or unflag them: what
    /// `f` and `:flag` resolve to.
    Flag(bool),
    /// Mark every email of the selection's threads read, or unread: what
    /// `r` resolves to, and `:read` or `:unread`.
    Read(bool),
    /// A due snoozed thread going back to where it was snoozed from. It is
    /// not on the undo history.
    Return,
}

impl Action {
    /// The action's name, as in the dry-run log.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Archive => "archive",
            Action::Delete => "delete",
            Action::Spam => "mark spam",
            Action::Move(_) => "move",
            Action::Snooze(_) => "snooze",
            Action::Unsnooze => "unsnooze",
            Action::ToggleFlag => "toggle flag",
            Action::ToggleRead => "toggle read",
            Action::Flag(true) => "flag",
            Action::Flag(false) => "unflag",
            Action::Read(true) => "mark read",
            Action::Read(false) => "mark unread",
            Action::Return => "return",
        }
    }

    fn done(&self) -> &'static str {
        match self {
            Action::Archive => "Archived",
            Action::Delete => "Deleted",
            Action::Spam => "Marked",
            Action::Move(_) => "Moved",
            Action::Snooze(_) => "Snoozed",
            Action::Unsnooze => "Unsnoozed",
            Action::ToggleFlag | Action::ToggleRead => {
                unreachable!("a toggle is recorded as what it resolves to")
            }
            Action::Flag(true) => "Flagged",
            Action::Flag(false) => "Unflagged",
            Action::Read(_) => "Marked",
            Action::Return => "Returned",
        }
    }
}

/// An action and the count it was given: what `.` repeats.
#[derive(Debug, Clone)]
struct Repeat {
    action: Action,
    count: Option<usize>,
}

/// Something that happened: a key, a request's result, a timer.
#[derive(Debug)]
pub enum Msg {
    /// Launch: load the Inbox queue and start the reconcile tick.
    Start,
    /// Show the queue this filter text selects, or report why it selects
    /// none.
    SetFilter(String),
    /// `/`: open the filter prompt holding the current filter.
    OpenFilter,
    /// `:`: open the command line.
    OpenCommand,
    /// A key typed at the open prompt.
    Prompt(Key),
    /// `t`: toggle newest and oldest first.
    ToggleSort,
    /// `:sort newest` or `:sort oldest`.
    SetSort(Sort),
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
    /// `d`: delete the selection.
    Delete,
    /// `!`: mark the selection spam.
    Spam,
    /// `:move` or a move picker choice: move the selection to the mailbox
    /// with this id.
    Move(String),
    /// `z`: open the snooze picker for the selection.
    OpenSnooze,
    /// `m`: open the move picker for the selection.
    OpenMove,
    /// A key at the open snooze or move picker.
    Picker(PickerKey),
    /// `:snooze <time>` or a picker choice: snooze the selection.
    Snooze(When),
    /// `:unsnooze`: return the selection from the snooze mailbox now.
    Unsnooze,
    /// `f` or `:flag`: unflag the selection when any of it is flagged,
    /// otherwise flag it.
    ToggleFlag,
    /// `r`: mark the selection read when any of it is unread, otherwise
    /// unread.
    ToggleRead,
    /// `:read` or `:unread`: mark the selection read, or unread.
    SetRead(bool),
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
    /// `:dry` or `:dry off`: turn dry run on, or off.
    SetDryRun(bool),
    /// The spinner's tick while a request is in flight.
    Tick,
    /// An event on the push stream: a change, or a ping.
    Wake,
    /// The reconcile tick, at [`App::next_reconcile_at`].
    ReconcileTick,
    /// `R` or `:refresh`: reconcile at once.
    Refresh,
    /// A request failed transiently and is being retried for the `retry`th
    /// time, counted from 1.
    Retrying {
        from: Source,
        retry: u32,
    },
    /// `q` has waited [`QUIT_WAIT`] for the outbox, at
    /// [`App::quit_at`].
    QuitTimeout,
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
    /// The result of creating the snooze mailbox.
    MailboxCreated {
        job: u64,
        result: Result<jmap::Mailbox, Error>,
    },
    /// The snooze mailbox's threads, for the due-snooze check at launch.
    SnoozedLoaded {
        result: Result<QueryResult, Error>,
    },
    /// Writing the schedule failed. The job it was written for, if any, was
    /// not sent.
    ScheduleFailed {
        job: Option<u64>,
        error: String,
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
    /// Run a queue query with `Mailbox/get` in its first request, and the
    /// snooze mailbox's query too when there is one, and report it back as
    /// [`Msg::Reconciled`]. It supersedes a query in flight, and is
    /// retried or not, as [`Effect::Query`] is.
    Reconcile {
        generation: u64,
        query: EmailQuery,
        snoozed: Option<EmailQuery>,
        retry: bool,
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
    /// Create a top-level mailbox with no role through the outbox, and
    /// report it back as [`Msg::MailboxCreated`].
    CreateMailbox {
        job: u64,
        name: String,
    },
    /// Run a query on the snooze mailbox, beside any queue query, and
    /// report it back as [`Msg::SnoozedLoaded`], retried or not as
    /// [`Effect::Query`] is.
    QuerySnoozed {
        query: EmailQuery,
        retry: bool,
    },
    /// Replace the schedule file with `contents` at once, before any effect
    /// after this one. On failure, skip the effects for `job` and report
    /// back with [`Msg::ScheduleFailed`].
    WriteSchedule {
        job: Option<u64>,
        path: PathBuf,
        contents: String,
    },
    /// Log a dry-run action, undo, or redo to the `blorp::dry_run` target at
    /// info.
    LogDryRun {
        step: Step,
        action: Action,
        thread_ids: Vec<String>,
    },
    /// Drop every outbox job numbered up to `through` that has not been
    /// sent. After a job fails transiently, the outbox sends nothing more
    /// until this comes.
    DropJobs {
        through: u64,
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
            sort: Sort::Newest,
            filter_prompt: None,
            filter_history: PromptHistory::default(),
            command_prompt: None,
            command_history: PromptHistory::default(),
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
            next_reconcile_at: now + RECONCILE_INTERVAL,
            reconciling: None,
            pending_reconcile: None,
            tz: TimeZone::UTC,
            schedule: Schedule::default(),
            dry_schedule: None,
            schedule_file: None,
            snooze_picker: None,
            move_picker: None,
            creating: None,
            due_at_launch: false,
            checking: None,
            extra: Extra::default(),
            connected: true,
            probe_wait: PROBE_START,
            retrying: BTreeMap::new(),
            previous: None,
            quitting: None,
        }
    }

    /// The app with dry run on or off, as `--dry-run` sets it at launch.
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self.dry_schedule = dry_run.then(|| self.schedule.clone());
        self
    }

    /// The app in time zone `tz`, UTC until set.
    pub fn with_time_zone(mut self, tz: TimeZone) -> Self {
        self.tz = tz;
        self
    }

    /// The app with the snooze schedule read from `file`, where changes to
    /// it are written.
    pub fn with_schedule(mut self, file: PathBuf, schedule: Schedule) -> Self {
        self.schedule_file = Some(file);
        self.dry_schedule = self.dry_run.then(|| schedule.clone());
        self.schedule = schedule;
        self
    }

    /// The app with `status` on the bottom line, as launch reports a
    /// problem.
    pub fn with_status(mut self, status: String) -> Self {
        self.status = status;
        self
    }

    /// The snooze schedule as actions have left it, dry-run ones included.
    pub fn schedule(&self) -> &Schedule {
        self.dry_schedule.as_ref().unwrap_or(&self.schedule)
    }

    fn schedule_mut(&mut self) -> &mut Schedule {
        self.dry_schedule.as_mut().unwrap_or(&mut self.schedule)
    }

    /// The snooze picker, while it is open.
    pub fn snooze_picker(&self) -> Option<SnoozeChoices> {
        let picker = self.snooze_picker?;
        let now = self.now.to_zoned(self.tz.clone());
        let presets = Preset::ALL
            .into_iter()
            .map(|preset| (preset, snooze::describe(&preset.at(&now), &now)))
            .collect();
        Some(SnoozeChoices {
            presets,
            index: picker.index,
            threads: self.queue.selection(picker.count.unwrap_or(1)).len(),
        })
    }

    /// The move picker, while it is open.
    pub fn move_picker(&self) -> Option<MoveChoices> {
        let picker = self.move_picker.as_ref()?;
        Some(MoveChoices {
            typed: picker.typed.clone(),
            paths: self
                .move_matches(&picker.typed)
                .into_iter()
                .map(|(_, path)| path)
                .collect(),
            index: picker.index,
            threads: self.queue.selection(picker.count.unwrap_or(1)).len(),
        })
    }

    /// The id and path of each mailbox a move can go to whose path holds
    /// `typed`, matched case-insensitively, sorted by path.
    fn move_matches(&self, typed: &str) -> Vec<(String, String)> {
        let typed = typed.to_lowercase();
        let mut matches: Vec<(String, String)> = self
            .mailboxes
            .iter()
            .filter(|m| self.mailboxes.is_move_target(m))
            .map(|m| (m.id.clone(), self.mailboxes.path(m)))
            .filter(|(_, path)| path.to_lowercase().contains(&typed))
            .collect();
        matches.sort_by(|a, b| a.1.cmp(&b.1));
        matches
    }

    /// How snooze time `at` reads from now.
    fn describe(&self, at: Timestamp) -> String {
        snooze::describe(
            &at.to_zoned(self.tz.clone()),
            &self.now.to_zoned(self.tz.clone()),
        )
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

    /// The sort order as the statusline shows it: `newest` or `oldest`, or
    /// in the snooze mailbox `soonest` or `latest`.
    pub fn sort_name(&self) -> &'static str {
        match (self.in_snooze_mailbox(), self.sort) {
            (false, sort) => sort.name(),
            (true, Sort::Newest) => "soonest",
            (true, Sort::Oldest) => "latest",
        }
    }

    /// Whether the queue shown is the snooze mailbox's, whose rows show
    /// and sort by when each thread returns.
    pub fn in_snooze_mailbox(&self) -> bool {
        match (&self.shown, self.mailboxes.snooze()) {
            (Some(shown), Some(snooze)) => shown.mailbox == QueueMailbox::One(snooze.id.clone()),
            _ => false,
        }
    }

    /// How the queue shown sorts.
    fn order(&self) -> Order {
        let ascending = self.sort.ascending();
        if self.in_snooze_mailbox() {
            Order::Return {
                ascending: !ascending,
            }
        } else {
            Order::Received { ascending }
        }
    }

    /// What keys go to.
    pub fn mode(&self) -> Mode {
        if self.filter_prompt.is_some() {
            Mode::Filter
        } else if self.command_prompt.is_some() {
            Mode::Command
        } else if self.queue.visual().is_some() {
            Mode::Visual
        } else {
            Mode::Normal
        }
    }

    /// The prompt that is open, if one is.
    fn open_prompt(&self) -> Option<&Prompt> {
        match (&self.filter_prompt, &self.command_prompt) {
            (Some(open), _) => Some(&open.prompt),
            (_, Some(open)) => Some(&open.prompt),
            _ => None,
        }
    }

    /// The text at the `/` or `:` prompt, while one is open.
    pub fn prompt(&self) -> Option<&str> {
        self.open_prompt().map(Prompt::text)
    }

    /// The cursor at the `/` or `:` prompt, in characters, while one is
    /// open.
    pub fn prompt_cursor(&self) -> Option<usize> {
        self.open_prompt().map(Prompt::cursor)
    }

    /// The completion candidates at the `:` prompt, and the one filling the
    /// word, while Tab is cycling through them.
    pub fn completion(&self) -> Option<(&[String], usize)> {
        let completion = self.command_prompt.as_ref()?.completion.as_ref()?;
        Some((&completion.candidates, completion.index))
    }

    /// How many threads match the queue's query, including any beyond
    /// [`QUEUE_LIMIT`].
    pub fn total(&self) -> usize {
        self.total
    }

    /// True while a queue query is in flight, other than a reconcile `R`
    /// did not ask for.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// False while disconnected: from when a request fails despite its
    /// retries until one succeeds.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// The highest retry a request is on, while any is being retried.
    pub fn retrying(&self) -> Option<u32> {
        self.retrying.values().max().copied()
    }

    /// Whether the spinner turns: while the queue loads or a request is
    /// being retried.
    pub fn spinning(&self) -> bool {
        self.loading || !self.retrying.is_empty()
    }

    /// When `q` stops waiting for the outbox, while it waits.
    pub fn quit_at(&self) -> Option<Timestamp> {
        self.quitting.as_ref().map(|q| q.at)
    }

    /// When the state task sends the next [`Msg::ReconcileTick`].
    pub fn next_reconcile_at(&self) -> Timestamp {
        self.next_reconcile_at
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

    /// Runs `query`, superseding any query in flight, a reconcile
    /// included.
    fn load(&mut self, query: EmailQuery) -> Vec<Effect> {
        self.next_generation();
        self.loading = true;
        self.reconciling = None;
        vec![Effect::Query {
            generation: self.generation,
            query: self.with_extra(query),
            retry: self.connected,
        }]
    }

    /// Starts a new generation of queue query, whose result supersedes any
    /// in flight, so that one's retry is over.
    fn next_generation(&mut self) {
        self.generation += 1;
        self.retrying
            .retain(|from, _| !matches!(from, Source::Query(_)));
    }

    /// `query` fetching by id, too, the returned threads the queue shown
    /// should hold, recorded as the query whose result comes next. The
    /// server's sort by `receivedAt` and its limit leave out a returned
    /// thread whose emails are older than the queue's last. These are
    /// fetched for a queue whose filter is its mailbox alone, which holds
    /// every thread that went back to that mailbox; a narrower filter
    /// cannot be checked without the server.
    fn with_extra(&mut self, mut query: EmailQuery) -> EmailQuery {
        query.extra_threads = match &self.shown {
            Some(shown) if shown.plain => self
                .schedule()
                .iter()
                .filter(|(_, e)| {
                    e.returned_at.is_some()
                        && !e.emails.is_empty()
                        && e.emails.values().all(|id| shown.mailbox.contains(id))
                })
                .map(|(id, _)| id.clone())
                .collect(),
            _ => Vec::new(),
        };
        self.extra = Extra {
            threads: query.extra_threads.clone(),
            job: self.job,
        };
        query
    }

    /// The queue's threads from `result`, in order: the query's, and each
    /// returned thread fetched by id that still belongs in the queue. The
    /// entry of a returned thread fetched by id that no longer belongs is
    /// dropped, unless an action was sent since the query, which the
    /// result may not show.
    fn queue_threads(&mut self, mut result: QueryResult) -> (Vec<Thread>, Vec<Effect>) {
        let extra = std::mem::take(&mut result.extra_threads);
        let extra_emails: Vec<jmap::Email> = result
            .emails
            .iter()
            .filter(|e| extra.iter().any(|t| t.email_ids.contains(&e.id)))
            .cloned()
            .collect();
        let mut threads = Thread::from_query(result);
        let Extra {
            threads: requested,
            job,
        } = std::mem::take(&mut self.extra);
        let mut left = Vec::new();
        for id in requested {
            if threads.iter().any(|t| t.id == id) {
                continue;
            }
            match extra
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| self.extra_thread(t, &extra_emails))
            {
                Some(thread) => threads.push(thread),
                None => left.push(id),
            }
        }
        let threads = self.ordered(threads);
        let effects = if job == self.job && self.in_flight.is_empty() {
            self.forget_entries(&left)
        } else {
            Vec::new()
        };
        (threads, effects)
    }

    /// `thread` as a queue row, as [`App::row`] picks its email.
    fn extra_thread(&self, thread: &jmap::Thread, emails: &[jmap::Email]) -> Option<Thread> {
        let emails: Vec<Email> = thread
            .email_ids
            .iter()
            .filter_map(|id| emails.iter().find(|e| e.id == *id))
            .cloned()
            .map(Email::from)
            .collect();
        self.row(thread.id.clone(), emails)
    }

    /// The thread `id` of `emails` as a queue row, standing for itself by
    /// its newest email a move would take out of the queue mailbox, or its
    /// oldest in an oldest-first queue, or none when it has no such email.
    fn row(&self, id: String, emails: Vec<Email>) -> Option<Thread> {
        let from = &self.shown.as_ref()?.mailbox;
        let moved = emails.iter().filter(|e| self.mailboxes.moves(e, from));
        let hit = if self.sort.ascending() {
            moved.min_by_key(|e| e.received_at)
        } else {
            moved.max_by_key(|e| e.received_at)
        }?
        .id
        .clone();
        Thread::new(id, emails, &hit)
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

    /// Whether an action must be refused because it would send to
    /// Fastmail while disconnected, saying so if it is. Dry run sends
    /// nothing, so its actions go ahead.
    fn refused(&mut self) -> bool {
        if self.connected || self.dry_run {
            return false;
        }
        self.status = "Disconnected, action not sent".into();
        true
    }

    /// Reports a queue query's failure: a transient one as being
    /// disconnected, and a bug as Fastmail's error.
    fn query_failed(&mut self, error: &Error) {
        self.status = match error.class() {
            Class::Transient => DISCONNECTED.into(),
            _ => error.to_string(),
        };
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
            plain: resolved.plain,
            query: EmailQuery {
                filter: resolved.condition,
                ascending: self.sort.ascending(),
                limit: QUEUE_LIMIT,
                extra_threads: Vec::new(),
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
    /// with, the query is run; once it loads, the selection is cleared and
    /// the cursor goes to the first row.
    fn show(&mut self, shown: Shown) -> Vec<Effect> {
        let same = self.shown.as_ref().is_some_and(|s| s.query == shown.query);
        if !same {
            self.remember();
        }
        let query = shown.query.clone();
        self.shown = Some(shown);
        if same {
            return Vec::new();
        }
        self.load(query)
    }

    /// Records the filter and sort shown, to go back to if the query for a
    /// new one fails, unless a query for a new one is already loading, in
    /// which case the record from before that one stands.
    fn remember(&mut self) {
        if self.previous.is_none() {
            self.previous = Some(Previous {
                shown: self.shown.clone(),
                sort: self.sort,
            });
        }
    }

    /// Goes back to the filter and sort shown before a new one's query
    /// failed. The first queue, with none before it, stays, so that a
    /// probe can load it.
    fn go_back(&mut self) {
        if let Some(Previous {
            shown: Some(shown),
            sort,
        }) = self.previous.take()
        {
            self.shown = Some(shown);
            self.sort = sort;
        }
    }

    /// Runs the shown queue's query again, if a queue is shown.
    fn reload(&mut self) -> Vec<Effect> {
        match self.shown.as_ref().map(|s| s.query.clone()) {
            Some(query) => self.load(query),
            None => Vec::new(),
        }
    }

    /// Sets the sort and, when it changes, reloads the queue in it; once it
    /// loads, the selection is cleared and the cursor goes to the first
    /// row.
    fn set_sort(&mut self, sort: Sort) -> Vec<Effect> {
        if sort == self.sort {
            return Vec::new();
        }
        if self.shown.is_some() {
            self.remember();
        }
        self.sort = sort;
        let Some(shown) = &mut self.shown else {
            return Vec::new();
        };
        shown.query.ascending = self.sort.ascending();
        let query = shown.query.clone();
        self.load(query)
    }

    /// Turns dry run on or off. A change clears the undo history, so real
    /// and simulated entries never share it, and makes a reconcile `R`
    /// asked for before it an automatic one, so it neither resets a dry-run
    /// queue nor clears the history again.
    fn set_dry_run(&mut self, on: bool) {
        let mode = if on { "on" } else { "off" };
        if on == self.dry_run {
            self.status = format!("Dry run already {mode}");
            return;
        }
        self.dry_run = on;
        self.history.clear();
        self.dry_schedule = on.then(|| self.schedule.clone());
        if !on && self.mailboxes.by_id(DRY_RUN_SNOOZE_MAILBOX).is_some() {
            self.mailboxes.remove(DRY_RUN_SNOOZE_MAILBOX);
            self.refresh_queue_mailbox();
        }
        self.pending_reconcile = self.pending_reconcile.min(Some(Trigger::Auto));
        if let Some(reconciling) = &mut self.reconciling {
            reconciling.trigger = Trigger::Auto;
        }
        self.status = format!("Dry run {mode}, undo history cleared");
    }

    /// Opens the `/` prompt holding the current filter, dropping a pending
    /// count, closing the visual range, and clearing the status message.
    fn open_filter(&mut self) {
        self.take_count();
        self.queue.close_visual();
        self.status.clear();
        self.filter_prompt = Some(FilterPrompt {
            prompt: Prompt::new(self.filter()),
            shown: self.shown.clone(),
            queue: self.queue.clone(),
            total: self.total,
            queried: false,
        });
    }

    /// Handles a key at the `/` prompt. Each edit shows the queue the text
    /// selects, if it selects one. Enter keeps it, or for text that selects
    /// nothing puts the earlier queue back and reports why; either way the
    /// text goes into the prompt's history. Esc puts the earlier queue back.
    fn filter_key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Enter => {
                let Some(open) = self.filter_prompt.take() else {
                    return Vec::new();
                };
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
            Key::Esc => match self.filter_prompt.take() {
                Some(open) => self.restore(open),
                None => Vec::new(),
            },
            key => {
                let Some(open) = &mut self.filter_prompt else {
                    return Vec::new();
                };
                open.prompt.edit(key, &self.filter_history);
                let text = open.prompt.text().to_string();
                let Ok(shown) = self.select(&text) else {
                    return Vec::new();
                };
                let effects = self.show(shown);
                if let Some(open) = &mut self.filter_prompt {
                    open.queried |= !effects.is_empty();
                }
                effects
            }
        }
    }

    /// Puts back the queue as it was when the prompt opened. When an edit
    /// ran a query, the earlier rows, cursor, and marks come back at once
    /// and the earlier query is run again.
    fn restore(&mut self, open: FilterPrompt) -> Vec<Effect> {
        self.shown = open.shown;
        self.previous = None;
        if !open.queried {
            return Vec::new();
        }
        self.queue = open.queue;
        self.total = open.total;
        self.reload()
    }

    /// Opens the `:` prompt, keeping the visual range open and the pending
    /// count for the command, and clearing the status message.
    fn open_command(&mut self) {
        let count = self.take_count();
        self.status.clear();
        self.command_prompt = Some(CommandPrompt {
            prompt: Prompt::new(""),
            count,
            completion: None,
        });
    }

    /// Handles a key at the `:` prompt. Enter runs the command, which goes
    /// into the prompt's history whether or not it runs; Enter and Esc
    /// return to normal mode.
    fn command_key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Enter => {
                let Some(open) = self.command_prompt.take() else {
                    return Vec::new();
                };
                let text = open.prompt.text();
                self.command_history.push(text);
                let effects = match command::parse(text) {
                    Ok(Some(command)) => self.run(command, open.count),
                    Ok(None) => Vec::new(),
                    Err(error) => {
                        self.status = error.to_string();
                        Vec::new()
                    }
                };
                self.queue.close_visual();
                effects
            }
            Key::Esc => {
                self.command_prompt = None;
                self.queue.close_visual();
                Vec::new()
            }
            Key::Tab | Key::BackTab => {
                self.complete(key == Key::Tab);
                Vec::new()
            }
            key => {
                if let Some(open) = &mut self.command_prompt {
                    open.completion = None;
                    open.prompt.edit(key, &self.command_history);
                }
                Vec::new()
            }
        }
    }

    /// Fills the word under the cursor at the `:` prompt with the next
    /// candidate, or the previous one when not `forward`. Without a
    /// completion under way, it starts one at the first candidate, or the
    /// last when not `forward`.
    fn complete(&mut self, forward: bool) {
        let Some(open) = &mut self.command_prompt else {
            return;
        };
        let completion = match open.completion.take() {
            Some(mut c) => {
                let n = c.candidates.len();
                c.index = if forward {
                    (c.index + 1) % n
                } else {
                    (c.index + n - 1) % n
                };
                c
            }
            None => {
                let (range, candidates) =
                    command::complete(open.prompt.text(), open.prompt.cursor(), &self.mailboxes);
                if candidates.is_empty() {
                    return;
                }
                let index = if forward { 0 } else { candidates.len() - 1 };
                Completion {
                    candidates,
                    index,
                    range,
                }
            }
        };
        let candidate = &completion.candidates[completion.index];
        open.prompt.replace(completion.range.clone(), candidate);
        let start = completion.range.start;
        open.completion = Some(Completion {
            range: start..start + candidate.chars().count(),
            ..completion
        });
    }

    /// Runs `command` on the selection, with `count` as the count typed
    /// before the colon.
    fn run(&mut self, command: Command, count: Option<usize>) -> Vec<Effect> {
        let msg = match command {
            Command::Archive => return self.run_on_selection(Msg::Archive, count),
            Command::Delete => return self.run_on_selection(Msg::Delete, count),
            Command::Spam => return self.run_on_selection(Msg::Spam, count),
            Command::Move(name) => {
                if self.queue.selection(count.unwrap_or(1)).is_empty() {
                    self.status = "Nothing selected".into();
                    return Vec::new();
                }
                let to = match self.mailboxes.resolve(&name) {
                    Ok(mailbox) => mailbox.id.clone(),
                    Err(_) if name.to_lowercase() == SNOOZE_MAILBOX.to_lowercase() => {
                        self.status = "Use z to snooze".into();
                        return Vec::new();
                    }
                    Err(error) => {
                        self.status = error.to_string();
                        return Vec::new();
                    }
                };
                return self.run_on_selection(Msg::Move(to), count);
            }
            Command::Snooze(when) => return self.run_on_selection(Msg::Snooze(when), count),
            Command::Unsnooze => return self.run_on_selection(Msg::Unsnooze, count),
            Command::Flag => return self.run_on_selection(Msg::ToggleFlag, count),
            Command::Read => return self.run_on_selection(Msg::SetRead(true), count),
            Command::Unread => return self.run_on_selection(Msg::SetRead(false), count),
            Command::Undo(n) => {
                self.pending_count = n.or(count);
                Msg::Undo
            }
            Command::Redo => {
                self.pending_count = count;
                Msg::Redo
            }
            Command::Filter(text) => Msg::SetFilter(text),
            Command::Sort(None) => Msg::ToggleSort,
            Command::Sort(Some(sort)) => Msg::SetSort(sort),
            Command::Refresh => Msg::Refresh,
            Command::Dry(on) => Msg::SetDryRun(on),
            Command::Quit => Msg::Quit,
        };
        handle(self, msg)
    }

    /// Runs `msg`, an action on the selection, with `count` as the count
    /// typed before the colon, or says nothing is selected.
    fn run_on_selection(&mut self, msg: Msg, count: Option<usize>) -> Vec<Effect> {
        if self.queue.selection(count.unwrap_or(1)).is_empty() {
            self.status = "Nothing selected".into();
            return Vec::new();
        }
        self.pending_count = count;
        handle(self, msg)
    }

    /// Carries out an action on the selection, unless it is refused.
    fn perform(&mut self, repeat: Repeat) -> Vec<Effect> {
        if self.refused() {
            return Vec::new();
        }
        match repeat.action {
            Action::Archive | Action::Delete | Action::Spam | Action::Move(_) => {
                self.move_selection(repeat)
            }
            Action::Snooze(until) => self.snooze(repeat, until),
            Action::Unsnooze => self.unsnooze(repeat),
            Action::ToggleFlag | Action::ToggleRead | Action::Flag(_) | Action::Read(_) => {
                self.set_keyword(repeat)
            }
            Action::Return => Vec::new(),
        }
    }

    /// Opens the snooze picker on the first preset, keeping the count and
    /// the visual range for the snooze, when there is a selection.
    fn open_snooze(&mut self) {
        let count = self.take_count();
        if self.queue.selection(count.unwrap_or(1)).is_empty() {
            return;
        }
        self.status.clear();
        self.snooze_picker = Some(SnoozePicker { index: 0, count });
    }

    /// Opens the move picker with every mailbox a move can go to, keeping
    /// the count and the visual range for the move, when there is a
    /// selection.
    fn open_move(&mut self) {
        let count = self.take_count();
        if self.queue.selection(count.unwrap_or(1)).is_empty() {
            return;
        }
        self.status.clear();
        self.move_picker = Some(MovePicker {
            typed: String::new(),
            index: 0,
            count,
        });
    }

    /// Handles a key at the move picker: a character or Backspace changes
    /// the text the mailboxes are filtered by, putting Enter on the first
    /// match, Down and Up move, Enter moves to the mailbox under the
    /// cursor, and Esc closes the picker and the visual range.
    fn move_picker_key(&mut self, key: PickerKey) -> Vec<Effect> {
        let Some(typed) = self.move_picker.as_ref().map(|p| p.typed.clone()) else {
            return Vec::new();
        };
        let matches = self.move_matches(&typed);
        let Some(picker) = &mut self.move_picker else {
            return Vec::new();
        };
        let n = matches.len();
        match key {
            PickerKey::Char(c) => {
                picker.typed.push(c);
                picker.index = 0;
            }
            PickerKey::Backspace => {
                picker.typed.pop();
                picker.index = 0;
            }
            PickerKey::Down if n > 0 => picker.index = (picker.index + 1) % n,
            PickerKey::Up if n > 0 => picker.index = (picker.index + n - 1) % n,
            PickerKey::Down | PickerKey::Up => {}
            PickerKey::Cancel => {
                self.move_picker = None;
                self.queue.close_visual();
            }
            PickerKey::Choose => {
                let Some((to, _)) = matches.into_iter().nth(picker.index) else {
                    return Vec::new();
                };
                self.pending_count = picker.count;
                self.move_picker = None;
                return handle(self, Msg::Move(to));
            }
        }
        Vec::new()
    }

    /// Handles a key at the snooze picker: `j` and `k` move, Enter snoozes
    /// with the preset under the cursor, a preset's letter snoozes with it,
    /// and Esc closes the picker and the visual range.
    fn picker_key(&mut self, key: PickerKey) -> Vec<Effect> {
        let Some(picker) = &mut self.snooze_picker else {
            return Vec::new();
        };
        let n = Preset::ALL.len();
        let preset = match key {
            PickerKey::Down => {
                picker.index = (picker.index + 1) % n;
                return Vec::new();
            }
            PickerKey::Up => {
                picker.index = (picker.index + n - 1) % n;
                return Vec::new();
            }
            PickerKey::Cancel => {
                self.snooze_picker = None;
                self.queue.close_visual();
                return Vec::new();
            }
            PickerKey::Choose => Preset::ALL[picker.index],
            PickerKey::Char(c) => match Preset::ALL.into_iter().find(|p| p.key() == c) {
                Some(preset) => preset,
                None => return Vec::new(),
            },
            PickerKey::Backspace => return Vec::new(),
        };
        self.pending_count = picker.count;
        self.snooze_picker = None;
        handle(self, Msg::Snooze(When::Preset(preset)))
    }

    /// Snoozes the selection until `until`: each thread's emails a move
    /// takes out of the queue mailbox go to the snooze mailbox, and the
    /// schedule records where each came from. The snooze mailbox is created
    /// first if it is missing. Closes the visual range.
    fn snooze(&mut self, repeat: Repeat, until: Timestamp) -> Vec<Effect> {
        let rows = self.queue.selection(repeat.count.unwrap_or(1));
        self.queue.close_visual();
        if rows.is_empty() || self.shown.is_none() {
            return Vec::new();
        }
        if until <= self.now {
            self.report(format!("Snooze time has passed: {}", self.describe(until)));
            return Vec::new();
        }
        if self.schedule_file.is_none() && !self.dry_run {
            self.report("No state directory to keep the snooze schedule in");
            return Vec::new();
        }
        let snooze = match self.mailboxes.snooze() {
            Some(snooze) => snooze.id.clone(),
            None if self.dry_run => {
                self.mailboxes.add(jmap::Mailbox {
                    id: DRY_RUN_SNOOZE_MAILBOX.into(),
                    name: SNOOZE_MAILBOX.into(),
                    parent_id: None,
                    role: None,
                });
                self.refresh_queue_mailbox();
                DRY_RUN_SNOOZE_MAILBOX.into()
            }
            None => {
                if self.creating.is_some() {
                    self.report(format!("Still creating {SNOOZE_MAILBOX}"));
                    return Vec::new();
                }
                self.job += 1;
                let thread_ids = rows
                    .iter()
                    .map(|&row| self.queue.threads()[row].id.clone())
                    .collect();
                self.creating = Some(Creating {
                    job: self.job,
                    thread_ids,
                    repeat,
                });
                self.report(format!("Creating {SNOOZE_MAILBOX}"));
                return vec![Effect::CreateMailbox {
                    job: self.job,
                    name: SNOOZE_MAILBOX.into(),
                }];
            }
        };
        self.snooze_rows(rows, repeat, until, &snooze)
    }

    /// Snoozes the threads at `rows` into the mailbox `snooze`.
    fn snooze_rows(
        &mut self,
        rows: Vec<usize>,
        repeat: Repeat,
        until: Timestamp,
        snooze: &str,
    ) -> Vec<Effect> {
        let Some(from) = self.shown.as_ref().map(|s| s.mailbox.clone()) else {
            return Vec::new();
        };
        if from == QueueMailbox::One(snooze.into()) {
            return self.reschedule(rows, repeat, until, snooze);
        }
        let inbox = self.mailboxes.by_role(Role::Inbox).map(|m| m.id.clone());
        let mut threads = Vec::new();
        let mut update = EmailUpdate::new();
        let mut entries = BTreeMap::new();
        for row in rows {
            let thread = &self.queue.threads()[row];
            let mut emails = BTreeMap::new();
            for email in &thread.emails {
                if !self.mailboxes.moves(email, &from) {
                    continue;
                }
                let Some(patch) = move_patch(email, &from, snooze) else {
                    continue;
                };
                let mut origins = email.mailbox_ids.iter().filter(|id| from.contains(id));
                let first = origins.next().cloned();
                let origin = match &inbox {
                    Some(inbox) if email.mailbox_ids.contains(inbox) && from.contains(inbox) => {
                        Some(inbox.clone())
                    }
                    _ => first,
                };
                if let Some(origin) = origin {
                    emails.insert(email.id.clone(), origin);
                    update.insert(email.id.clone(), patch);
                }
            }
            if !emails.is_empty() {
                threads.push(thread.clone());
                let entry = snooze::Entry {
                    emails,
                    snoozed_at: self.now,
                    return_at: until,
                    returned_at: None,
                };
                entries.insert(thread.id.clone(), entry);
            }
        }
        if update.is_empty() {
            self.report("Nothing to snooze");
            return Vec::new();
        }
        self.act(repeat.clone(), repeat.action, threads, update, entries)
    }

    /// Sets the return time of the threads at `rows`, in the snooze mailbox
    /// `snooze`, to `until`, moving nothing. A thread with no entry gets one
    /// that returns its emails to the Inbox.
    fn reschedule(
        &mut self,
        rows: Vec<usize>,
        repeat: Repeat,
        until: Timestamp,
        snooze: &str,
    ) -> Vec<Effect> {
        let inbox = self.mailboxes.by_role(Role::Inbox).map(|m| m.id.clone());
        let mut threads = Vec::new();
        let mut entries = BTreeMap::new();
        for row in rows {
            let thread = &self.queue.threads()[row];
            let entry = match self.schedule().get(&thread.id) {
                Some(entry) => snooze::Entry {
                    return_at: until,
                    returned_at: None,
                    ..entry.clone()
                },
                None => snooze::Entry {
                    emails: thread
                        .emails
                        .iter()
                        .filter(|e| e.mailbox_ids.contains(snooze))
                        .filter_map(|e| Some((e.id.clone(), inbox.clone()?)))
                        .collect(),
                    snoozed_at: self.now,
                    return_at: until,
                    returned_at: None,
                },
            };
            threads.push(thread.clone());
            entries.insert(thread.id.clone(), entry);
        }
        self.act(
            repeat.clone(),
            repeat.action,
            threads,
            EmailUpdate::new(),
            entries,
        )
    }

    /// Returns the selection from the snooze mailbox now, as a due return
    /// would, as one undo entry. Closes the visual range.
    fn unsnooze(&mut self, repeat: Repeat) -> Vec<Effect> {
        let rows = self.queue.selection(repeat.count.unwrap_or(1));
        self.queue.close_visual();
        if rows.is_empty() {
            return Vec::new();
        }
        let mut threads = Vec::new();
        let mut update = EmailUpdate::new();
        let mut entries = BTreeMap::new();
        if self.in_snooze_mailbox() {
            for row in rows {
                let thread = &self.queue.threads()[row];
                let entry = self.schedule().get(&thread.id);
                let Some((patches, entry)) = self.return_update(thread, entry) else {
                    continue;
                };
                threads.push(thread.clone());
                update.extend(patches);
                entries.insert(thread.id.clone(), entry);
            }
        }
        if update.is_empty() {
            self.report("Nothing to unsnooze");
            return Vec::new();
        }
        self.act(repeat.clone(), repeat.action, threads, update, entries)
    }

    /// The return of `thread` from the snooze mailbox, whose schedule entry
    /// is `entry`: each email the entry records that is still there goes
    /// back to the mailbox it came from, or the Inbox when that is gone, and
    /// with no entry each of its emails there goes to the Inbox, marked
    /// unread with the flag untouched. Gives the patches and the entry
    /// recording the return, or none when no email goes back.
    fn return_update(
        &self,
        thread: &Thread,
        entry: Option<&snooze::Entry>,
    ) -> Option<(EmailUpdate, snooze::Entry)> {
        let snooze = &self.mailboxes.snooze()?.id;
        let inbox = &self.mailboxes.by_role(Role::Inbox)?.id;
        let mut update = EmailUpdate::new();
        let mut origins = BTreeMap::new();
        for email in &thread.emails {
            if !email.mailbox_ids.contains(snooze) {
                continue;
            }
            let origin = match entry {
                Some(entry) => match entry.emails.get(&email.id) {
                    Some(origin) if self.mailboxes.by_id(origin).is_some() => origin,
                    Some(_) => inbox,
                    None => continue,
                },
                None => inbox,
            };
            let before = State::from(email);
            let mut after = before.clone();
            after.mailbox_ids.remove(snooze);
            after.mailbox_ids.insert(origin.clone());
            after.seen = false;
            update.insert(email.id.clone(), before.patch_to(&after));
            origins.insert(email.id.clone(), origin.clone());
        }
        if update.is_empty() {
            return None;
        }
        let returned = match entry {
            Some(entry) => snooze::Entry {
                returned_at: Some(self.now),
                ..entry.clone()
            },
            None => snooze::Entry {
                emails: origins,
                snoozed_at: self.now,
                return_at: self.now,
                returned_at: Some(self.now),
            },
        };
        Some((update, returned))
    }

    /// Re-resolves an `in:any` queue's mailbox, whose excluded mailboxes
    /// include the snooze mailbox once it exists.
    fn refresh_queue_mailbox(&mut self) {
        let Some(shown) = &self.shown else {
            return;
        };
        if let QueueMailbox::Any(_) = shown.mailbox
            && let Ok(refreshed) = self.select(&shown.filter.clone())
        {
            self.shown = Some(refreshed);
        }
    }

    /// Handles the result of creating the snooze mailbox: on success, the
    /// snooze that waited for it goes ahead on those of its threads still
    /// in the queue.
    fn mailbox_created(&mut self, job: u64, result: Result<jmap::Mailbox, Error>) -> Vec<Effect> {
        self.record_reply(&result);
        if let Err(error) = &result {
            match error.class() {
                Class::Fatal(fatal) => return vec![Effect::Fatal(fatal)],
                Class::Transient => return self.lost(job),
                Class::Bug => {}
            }
        }
        self.retrying.remove(&Source::Job(job));
        let Some(creating) = self.creating.take_if(|c| c.job == job) else {
            return Vec::new();
        };
        let mailbox = match result {
            Ok(mailbox) => mailbox,
            Err(error) => {
                self.status = format!("Could not create {SNOOZE_MAILBOX}: {error}");
                if let Some(quitting) = &mut self.quitting {
                    quitting.failed += 1;
                }
                return self.reconcile(Trigger::Auto);
            }
        };
        let id = mailbox.id.clone();
        self.mailboxes.add(mailbox);
        self.refresh_queue_mailbox();
        let rows = creating
            .thread_ids
            .iter()
            .filter_map(|id| self.queue.index_of(id))
            .collect::<Vec<_>>();
        let Action::Snooze(until) = creating.repeat.action else {
            return Vec::new();
        };
        let mut effects = if rows.is_empty() {
            self.report("Nothing to snooze");
            Vec::new()
        } else {
            self.snooze_rows(rows, creating.repeat, until, &id)
        };
        effects.extend(self.run_pending_reconcile());
        effects
    }

    /// Moves the selection, with the count's rows from the cursor when
    /// there is neither a visual range nor a mark, to the action's mailbox:
    /// each thread's emails a move takes out of the queue mailbox. Closes
    /// the visual range.
    fn move_selection(&mut self, repeat: Repeat) -> Vec<Effect> {
        let rows = self.queue.selection(repeat.count.unwrap_or(1));
        self.queue.close_visual();
        let Some(from) = self.shown.as_ref().map(|s| s.mailbox.clone()) else {
            return Vec::new();
        };
        if rows.is_empty() {
            return Vec::new();
        }
        let to = match self.move_target(&repeat.action, &from) {
            Ok(to) => to,
            Err(message) => {
                self.report(message);
                return Vec::new();
            }
        };
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
                        let patch = move_patch(email, &from, &to)?;
                        Some((email.id.clone(), patch))
                    }),
            );
            if update.len() > before {
                threads.push(thread.clone());
            }
        }
        if update.is_empty() {
            self.report(format!("Nothing to {}", repeat.action.name()));
            return Vec::new();
        }
        self.act(
            repeat.clone(),
            repeat.action,
            threads,
            update,
            BTreeMap::new(),
        )
    }

    /// The id of the mailbox move `action` moves to out of the queue
    /// mailbox `from`, or why it moves nothing. Delete and spam are
    /// disabled in Trash and Junk.
    fn move_target(&self, action: &Action, from: &QueueMailbox) -> Result<String, String> {
        let (role, name) = match action {
            Action::Archive => (Role::Archive, "Archive"),
            Action::Delete => (Role::Trash, "Trash"),
            Action::Spam => (Role::Junk, "Junk"),
            Action::Move(to) => return self.move_to(to, from),
            _ => unreachable!("only a move reaches move_target"),
        };
        let in_role = |role| {
            self.mailboxes
                .by_role(role)
                .is_some_and(|m| *from == QueueMailbox::One(m.id.clone()))
        };
        if matches!(action, Action::Delete | Action::Spam)
            && (in_role(Role::Trash) || in_role(Role::Junk))
        {
            let verb = if *action == Action::Delete {
                "Delete"
            } else {
                "Spam"
            };
            return Err(format!("{verb} is disabled in {}", self.queue_name()));
        }
        let Some(to) = self.mailboxes.by_role(role) else {
            return Err(format!("No {name} mailbox"));
        };
        if *from == QueueMailbox::One(to.id.clone()) {
            return Err(format!("Already in {name}"));
        }
        Ok(to.id.clone())
    }

    /// The id `to` as the mailbox a move moves to out of the queue mailbox
    /// `from`, or why it cannot be. The snooze mailbox is not one, since
    /// a thread moved there would have no schedule entry, and neither are
    /// Drafts and Sent, whose emails no action moves out again.
    fn move_to(&self, to: &str, from: &QueueMailbox) -> Result<String, String> {
        let Some(mailbox) = self.mailboxes.by_id(to) else {
            return Err("No mailbox to move to".into());
        };
        let path = self.mailboxes.path(mailbox);
        if !self.mailboxes.is_move_target(mailbox) {
            return Err(match mailbox.role {
                Some(Role::Drafts | Role::Sent) => format!("Cannot move to {path}"),
                _ => "Use z to snooze".into(),
            });
        }
        if *from == QueueMailbox::One(to.into()) {
            return Err(format!("Already in {path}"));
        }
        Ok(to.into())
    }

    /// Flags or unflags, or marks read or unread, every email of the
    /// selection's threads in every mailbox, a toggle deciding which by the
    /// selection as it is now. Each thread stays in the queue and keeps its
    /// schedule entry. Closes the visual range.
    fn set_keyword(&mut self, repeat: Repeat) -> Vec<Effect> {
        let rows = self.queue.selection(repeat.count.unwrap_or(1));
        self.queue.close_visual();
        if rows.is_empty() {
            return Vec::new();
        }
        let selected = || rows.iter().map(|&row| &self.queue.threads()[row]);
        let action = match &repeat.action {
            Action::ToggleFlag => Action::Flag(!selected().any(Thread::flagged)),
            Action::ToggleRead => Action::Read(selected().any(Thread::unread)),
            action => action.clone(),
        };
        let (keyword, on, has): (&str, bool, fn(&Email) -> bool) = match action {
            Action::Flag(on) => ("$flagged", on, |email| email.flagged),
            Action::Read(on) => ("$seen", on, |email| email.seen),
            _ => return Vec::new(),
        };
        let value = if on { Value::Bool(true) } else { Value::Null };
        let mut threads = Vec::new();
        let mut update = EmailUpdate::new();
        for thread in selected() {
            let before = update.len();
            update.extend(
                thread
                    .emails
                    .iter()
                    .filter(|email| has(email) != on)
                    .map(|email| {
                        let patch = [(format!("keywords/{keyword}"), value.clone())].into();
                        (email.id.clone(), patch)
                    }),
            );
            if update.len() > before {
                threads.push(thread.clone());
            }
        }
        if update.is_empty() {
            self.report(format!("Nothing to {}", action.name()));
            return Vec::new();
        }
        let entries = threads
            .iter()
            .filter_map(|thread| {
                let entry = self.schedule().get(&thread.id)?.clone();
                Some((thread.id.clone(), entry))
            })
            .collect();
        self.act(repeat, action, threads, update, entries)
    }

    /// Records `action` on `touched` as one undo entry and `repeat` as what
    /// `.` repeats, applies it to the queue and the schedule, reports it,
    /// and sends `update`, or in dry run logs it. The two differ when
    /// `repeat` is a toggle, which `.` decides again. Each thread's
    /// schedule entry becomes its entry in `entries`, or goes when it has
    /// none there.
    fn act(
        &mut self,
        repeat: Repeat,
        action: Action,
        touched: Vec<Thread>,
        update: EmailUpdate,
        mut entries: BTreeMap<String, snooze::Entry>,
    ) -> Vec<Effect> {
        self.last_action = Some(repeat);
        let schedule = touched
            .iter()
            .filter_map(|thread| {
                let before = self.schedule().get(&thread.id).cloned();
                let after = entries.remove(&thread.id);
                (before != after).then(|| ScheduleChange {
                    thread_id: thread.id.clone(),
                    before,
                    after,
                })
            })
            .collect();
        let changes = changes(&touched, &update);
        let entry = self
            .history
            .record(action.clone(), touched, changes, schedule)
            .clone();
        let suffix = match &action {
            Action::Snooze(until) => format!(" until {}", self.describe(*until)),
            Action::Read(true) => " read".into(),
            Action::Read(false) => " unread".into(),
            Action::Spam => " as spam".into(),
            Action::Move(to) => match self.mailboxes.by_id(to) {
                Some(mailbox) => format!(" to {}", self.mailboxes.path(mailbox)),
                None => String::new(),
            },
            _ => String::new(),
        };
        self.report(format!(
            "{} {}{suffix}",
            action.done(),
            threads(entry.threads.len())
        ));
        self.send(entry, update)
    }

    /// Applies `entry` to the queue and the schedule and sends `update`
    /// for it, after writing the schedule if the entry changes it, or in
    /// dry run logs it.
    fn send(&mut self, entry: Entry, update: EmailUpdate) -> Vec<Effect> {
        self.apply(&entry, Side::After);
        if self.dry_run {
            return vec![log_dry_run(Step::Act, &entry)];
        }
        self.job += 1;
        let job = self.job;
        let mut effects: Vec<Effect> = self.write_schedule(Some(job), &entry).into_iter().collect();
        effects.push(Effect::SetEmails { job, update });
        self.in_flight.push(InFlight {
            job,
            step: Step::Act,
            entry,
        });
        effects
    }

    /// The write of the schedule after `entry` changed it, for `job`, or
    /// none when it did not or in dry run.
    fn write_schedule(&self, job: Option<u64>, entry: &Entry) -> Option<Effect> {
        if entry.schedule.is_empty() {
            return None;
        }
        self.schedule_write(job)
    }

    /// The write of the schedule as it now is, for `job`, or none in dry run
    /// or with no file to write.
    fn schedule_write(&self, job: Option<u64>) -> Option<Effect> {
        if self.dry_run {
            return None;
        }
        Some(Effect::WriteSchedule {
            job,
            path: self.schedule_file.clone()?,
            contents: self.schedule.to_json(&self.tz),
        })
    }

    /// Undoes or redoes up to the pending count of entries, one job each:
    /// an undo is guarded by an `Email/get`, and a redo sends the action's
    /// patch again. A redo waits until its entry's undo is answered, since
    /// that answer can narrow the entry.
    fn replay(&mut self, step: Step) -> Vec<Effect> {
        let count = self.take_count().unwrap_or(1);
        if self.refused() {
            return Vec::new();
        }
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
            let name = entry.action.name();
            match done.iter_mut().find(|(action, _)| action.name() == name) {
                Some((_, n)) => *n += entry.threads.len(),
                None => done.push((entry.action.clone(), entry.threads.len())),
            }
            if self.dry_run {
                effects.push(log_dry_run(step, &entry));
                continue;
            }
            self.job += 1;
            effects.extend(self.write_schedule(Some(self.job), &entry));
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
        for change in &entry.schedule {
            let state = change.state(side).cloned();
            self.schedule_mut().set(&change.thread_id, state);
        }
        let order = self.order();
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
                    // The email that matched where the action ran may not
                    // be in this queue's mailbox.
                    let thread = self
                        .row(thread.id.clone(), thread.emails.clone())
                        .unwrap_or(thread);
                    // `self.schedule()` would borrow the queue too.
                    let schedule = self.dry_schedule.as_ref().unwrap_or(&self.schedule);
                    self.queue
                        .insert_sorted(thread, order.ascending(), |t| order.key(schedule, t));
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
        if !entry.schedule.is_empty() {
            let schedule = self.dry_schedule.as_ref().unwrap_or(&self.schedule);
            self.queue.resort(|threads| order.sort(threads, schedule));
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
        self.record_reply(&result);
        let failure = match result {
            Ok(set) if set.not_updated.is_empty() => None,
            Ok(set) => Some(not_updated(set.not_updated.len())),
            Err(error) => match error.class() {
                Class::Fatal(fatal) => return vec![Effect::Fatal(fatal)],
                Class::Transient => return self.lost(job),
                Class::Bug => Some(error.to_string()),
            },
        };
        self.answered(job, failure, Vec::new())
    }

    /// Handles an undo job's result.
    fn replayed(&mut self, job: u64, result: Result<Replayed, Error>) -> Vec<Effect> {
        self.record_reply(&result);
        let (failure, skipped) = match result {
            Ok(r) if r.not_updated.is_empty() => (None, r.skipped),
            Ok(r) => (Some(not_updated(r.not_updated.len())), Vec::new()),
            Err(error) => match error.class() {
                Class::Fatal(fatal) => return vec![Effect::Fatal(fatal)],
                Class::Transient => return self.lost(job),
                Class::Bug => (Some(error.to_string()), Vec::new()),
            },
        };
        self.answered(job, failure, skipped)
    }

    /// Handles outbox job `job` failing despite its retries, which has
    /// disconnected blorp. The outbox holds back the jobs behind it, and
    /// they are dropped: every job in the outbox is rolled back, newest
    /// first, and its entry dropped, and a snooze waiting for the snooze
    /// mailbox is abandoned. No reconcile is issued; the next probe is
    /// one.
    fn lost(&mut self, job: u64) -> Vec<Effect> {
        let mut effects = vec![Effect::DropJobs { through: self.job }];
        self.retrying
            .retain(|from, _| !matches!(from, Source::Job(_)));
        let what = match self.in_flight.iter().find(|f| f.job == job) {
            Some(f) => Some(format!(
                "{}{} {}",
                f.step.prefix(),
                f.entry.action.name(),
                threads(f.entry.threads.len())
            )),
            None => self
                .creating
                .as_ref()
                .filter(|c| c.job == job)
                .map(|_| format!("create {SNOOZE_MAILBOX}")),
        };
        let mut n = 0;
        let mut schedule = false;
        while let Some(InFlight { step, entry, .. }) = self.in_flight.pop() {
            self.apply(&entry, step.side().other());
            self.history.forget(entry.id);
            schedule |= !entry.schedule.is_empty();
            n += 1;
        }
        if self.creating.take().is_some() {
            n += 1;
        }
        if schedule {
            effects.extend(self.schedule_write(None));
        }
        if let Some(quitting) = &mut self.quitting {
            quitting.failed += n;
        }
        self.status = match what {
            Some(what) if n == 2 => {
                format!("{DISCONNECTED}; could not {what} or the action after it")
            }
            Some(what) if n > 2 => format!(
                "{DISCONNECTED}; could not {what} or the {} actions after it",
                n - 1
            ),
            Some(what) => format!("{DISCONNECTED}; could not {what}"),
            None => DISCONNECTED.into(),
        };
        effects
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
        self.retrying.remove(&Source::Job(job));
        let n = entry.threads.len();
        let mut effects = Vec::new();
        if let Some(reason) = failure {
            if let Some(quitting) = &mut self.quitting {
                quitting.failed += 1;
            }
            self.apply(&entry, step.side().other());
            effects.extend(self.write_schedule(None, &entry));
            self.history.forget(entry.id);
            self.status = format!(
                "Could not {}{} {}: {reason}",
                step.prefix(),
                entry.action.name(),
                threads(n)
            );
        } else if skipped.is_empty() {
            return self.run_pending_reconcile();
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
        effects.extend(self.reconcile(Trigger::Auto));
        effects
    }

    /// Handles a failed write of the schedule. The job it was written for
    /// was not sent, so its change is rolled back and its entry dropped
    /// with nothing to tell Fastmail.
    fn schedule_failed(&mut self, job: Option<u64>, error: &str) -> Vec<Effect> {
        let reason = format!("could not write the snooze schedule: {error}");
        let Some(position) = job.and_then(|job| self.in_flight.iter().position(|f| f.job == job))
        else {
            self.status = format!("Could not save the snooze schedule: {error}");
            return Vec::new();
        };
        let InFlight { step, entry, .. } = self.in_flight.remove(position);
        self.apply(&entry, step.side().other());
        self.history.forget(entry.id);
        self.status = format!(
            "Could not {}{} {}: {reason}",
            step.prefix(),
            entry.action.name(),
            threads(entry.threads.len())
        );
        self.run_pending_reconcile()
    }

    /// The query on the snooze mailbox that the due-snooze check runs,
    /// fetching by id too the threads due to return, since the query's
    /// limit can leave one out. None when there is no snooze mailbox on
    /// Fastmail.
    fn snoozed_query(&self) -> Option<EmailQuery> {
        let snooze = self
            .mailboxes
            .snooze()
            .filter(|m| m.id != DRY_RUN_SNOOZE_MAILBOX)?;
        Some(EmailQuery {
            filter: json!({"inMailbox": snooze.id}),
            ascending: false,
            limit: QUEUE_LIMIT,
            extra_threads: self.due(),
        })
    }

    /// The ids of the threads due to return that have not.
    fn due(&self) -> Vec<String> {
        self.schedule()
            .iter()
            .filter(|(_, e)| e.is_due(self.now))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Starts the due-snooze check at launch: a query on the snooze
    /// mailbox, or with no snooze mailbox to return threads from, dropping
    /// the due entries.
    fn check_due(&mut self) -> Vec<Effect> {
        match self.snoozed_query() {
            Some(query) => {
                self.checking = Some(self.job);
                vec![Effect::QuerySnoozed {
                    query,
                    retry: self.connected,
                }]
            }
            None => self.forget_due(),
        }
    }

    /// Handles the launch check's query on the snooze mailbox. As with a
    /// reconcile, the result is used only if the outbox stayed idle from
    /// when it was sent, so a snapshot taken before a return or an action
    /// landed never returns a thread again; otherwise the next reconcile
    /// runs the check.
    fn snoozed_loaded(&mut self, result: Result<QueryResult, Error>) -> Vec<Effect> {
        let sent = self.checking.take();
        self.retrying.remove(&Source::SnoozeCheck);
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        self.record_reply(&result);
        let mut effects = match result {
            Ok(_) if sent != Some(self.job) || !self.in_flight.is_empty() => Vec::new(),
            Ok(result) => self.return_due(Thread::all_from_query(result)),
            // The next reconcile runs the check.
            Err(error) if error.class() == Class::Transient => Vec::new(),
            Err(error) => {
                self.status = format!("Could not check snoozed threads: {error}");
                Vec::new()
            }
        };
        // A reply that reconnects lets a reconcile waiting to run go.
        effects.extend(self.run_pending_reconcile());
        effects
    }

    /// Whether a result from `from` is still to come, so that a retry it
    /// reports late, after it was superseded or dropped, is ignored.
    fn awaits(&self, from: Source) -> bool {
        match from {
            Source::Query(generation) => {
                generation == self.generation && (self.loading || self.reconciling.is_some())
            }
            Source::Job(job) => {
                self.in_flight.iter().any(|f| f.job == job)
                    || self.creating.as_ref().is_some_and(|c| c.job == job)
            }
            Source::SnoozeCheck => self.checking.is_some(),
        }
    }

    /// How many jobs the outbox holds.
    fn outbox_len(&self) -> usize {
        self.in_flight.len() + usize::from(self.creating.is_some())
    }

    /// `q` or `:quit`: quits at once when the outbox is empty, and
    /// otherwise waits up to [`QUIT_WAIT`] for it to drain.
    fn quit(&mut self) -> Vec<Effect> {
        if self.outbox_len() == 0 {
            return vec![Effect::Quit];
        }
        if self.quitting.is_none() {
            let n = self.outbox_len();
            self.quitting = Some(Quitting {
                at: self.now + QUIT_WAIT,
                failed: 0,
            });
            self.status = format!("Waiting for Fastmail to confirm {}", actions(n));
        }
        Vec::new()
    }

    /// The quit effect once the outbox has drained while `q` waits:
    /// unconfirmed when a job failed meanwhile.
    fn drained(&self) -> Option<Effect> {
        let quitting = self.quitting.as_ref()?;
        if self.outbox_len() > 0 {
            return None;
        }
        Some(quit_with(quitting.failed))
    }

    /// `q` has waited long enough: quits, with every job still in the
    /// outbox and every one that failed meanwhile unconfirmed.
    fn quit_timeout(&self) -> Vec<Effect> {
        let Some(quitting) = &self.quitting else {
            return Vec::new();
        };
        vec![quit_with(quitting.failed + self.outbox_len())]
    }

    /// Drops the entries of the threads due to return, which have no snooze
    /// mailbox to return from.
    fn forget_due(&mut self) -> Vec<Effect> {
        let due = self.due();
        self.forget_entries(&due)
    }

    /// Drops the schedule entries of the threads `ids`, writing the
    /// schedule.
    fn forget_entries(&mut self, ids: &[String]) -> Vec<Effect> {
        if ids.is_empty() {
            return Vec::new();
        }
        for id in ids {
            self.schedule_mut().set(id, None);
        }
        self.schedule_write(None).into_iter().collect()
    }

    /// Whether `thread`, in the snooze mailbox, returns now: it has no
    /// entry, its entry is due, or it returns early because it gained an
    /// email received after it was snoozed in `outside`, every mailbox but
    /// the snooze mailbox, Trash, Junk, Drafts, and Sent. A thread whose
    /// entry records a return already made does not.
    fn returns_now(
        &self,
        thread: &Thread,
        entry: Option<&snooze::Entry>,
        outside: &QueueMailbox,
    ) -> bool {
        let Some(entry) = entry else {
            return true;
        };
        if entry.returned_at.is_some() {
            return false;
        }
        if entry.is_due(self.now) {
            return true;
        }
        thread.emails.iter().any(|email| {
            email.received_at > entry.snoozed_at
                && email.mailbox_ids.iter().any(|id| outside.contains(id))
        })
    }

    /// Returns every thread among `snoozed`, the snooze mailbox's threads,
    /// that returns now, as [`App::return_update`] describes, as one outbox
    /// job per thread, and the entry records when. A due entry whose thread
    /// is gone or has no snoozed email left is dropped. In dry run the
    /// return happens in memory only.
    fn return_due(&mut self, snoozed: Vec<Thread>) -> Vec<Effect> {
        let now = self.now;
        let mut gone: Vec<String> = self
            .schedule()
            .iter()
            .filter(|(id, e)| e.is_due(now) && !snoozed.iter().any(|t| t.id == **id))
            .map(|(id, _)| id.clone())
            .collect();
        let outside = self.mailboxes.all_mail();
        let mut effects = Vec::new();
        let mut returned = 0;
        for thread in snoozed {
            let before = self.schedule().get(&thread.id).cloned();
            if !self.returns_now(&thread, before.as_ref(), &outside) {
                continue;
            }
            let Some((update, after)) = self.return_update(&thread, before.as_ref()) else {
                if before.is_some() {
                    gone.push(thread.id.clone());
                }
                continue;
            };
            let threads = vec![thread];
            let entry = Entry {
                id: 0,
                action: Action::Return,
                changes: changes(&threads, &update),
                schedule: vec![ScheduleChange {
                    thread_id: threads[0].id.clone(),
                    before,
                    after: Some(after),
                }],
                threads,
            };
            returned += 1;
            effects.extend(self.send(entry, update));
        }
        effects.extend(self.forget_entries(&gone));
        if returned > 0 {
            self.report(format!("Returned {} from snooze", threads(returned)));
        }
        effects
    }

    /// Sorts `threads`, in the server's order by `receivedAt`, as the queue
    /// shown sorts: by the later of that and when each returned from snooze,
    /// or in the snooze mailbox by when each returns, keeping the server's
    /// order between ties.
    fn ordered(&self, mut threads: Vec<Thread>) -> Vec<Thread> {
        self.order().sort(&mut threads, self.schedule());
        threads
    }

    /// Re-runs the shown queue's query with `Mailbox/get`, and the snooze
    /// mailbox's query for the due-snooze check, to bring the queue, the
    /// mailbox list, and the snoozed threads back in line with Fastmail.
    /// While a query is in flight or the outbox is busy it waits, and runs
    /// once both are done, or while disconnected as the next probe. A
    /// reconcile that runs takes the place of one waiting. In dry run only
    /// `R` runs one, and a probe while disconnected.
    fn reconcile(&mut self, trigger: Trigger) -> Vec<Effect> {
        if self.dry_run && trigger == Trigger::Auto && self.connected {
            return Vec::new();
        }
        let Some(query) = self.shown.as_ref().map(|s| s.query.clone()) else {
            return Vec::new();
        };
        if self.busy() {
            self.pending_reconcile = self.pending_reconcile.max(Some(trigger));
            return Vec::new();
        }
        let trigger = self
            .pending_reconcile
            .take()
            .map_or(trigger, |t| t.max(trigger));
        self.next_generation();
        self.loading = trigger == Trigger::Refresh;
        self.reconciling = Some(Reconciling {
            generation: self.generation,
            trigger,
            job: self.job,
        });
        let query = self.with_extra(query);
        vec![Effect::Reconcile {
            generation: self.generation,
            query,
            snoozed: self.snoozed_query(),
            retry: self.connected,
        }]
    }

    /// Whether a queue query is in flight or the outbox holds a job.
    fn busy(&self) -> bool {
        self.loading
            || self.reconciling.is_some()
            || !self.in_flight.is_empty()
            || self.creating.is_some()
    }

    /// Runs the reconcile that was waiting, once nothing holds it back.
    /// While disconnected it waits for the next probe instead, which runs
    /// as it.
    fn run_pending_reconcile(&mut self) -> Vec<Effect> {
        if self.busy() || !self.connected {
            return Vec::new();
        }
        match self.pending_reconcile.take() {
            Some(trigger) => self.reconcile(trigger),
            None => Vec::new(),
        }
    }

    /// Handles a reconcile's result. It is applied only if the outbox
    /// stayed idle from when it was sent, so a snapshot taken before an
    /// action landed never brings back a thread the action moved; otherwise
    /// it is discarded and run again once the outbox drains. A failure is
    /// reported only for `R`, apart from the connection state changing.
    fn reconciled(&mut self, generation: u64, result: Result<Reconciled, Error>) -> Vec<Effect> {
        if let Err(error) = &result
            && let Some(fatal) = error.fatal()
        {
            return vec![Effect::Fatal(fatal)];
        }
        let Some(reconciling) = self.reconciling.take_if(|r| r.generation == generation) else {
            return Vec::new();
        };
        self.record_reply(&result);
        self.retrying.remove(&Source::Query(generation));
        self.loading = false;
        let trigger = reconciling.trigger;
        match result {
            Err(error) => {
                if trigger == Trigger::Refresh {
                    self.query_failed(&error);
                }
            }
            Ok(_) if self.job != reconciling.job || !self.in_flight.is_empty() => {
                self.pending_reconcile = self.pending_reconcile.max(Some(trigger));
            }
            Ok(_) if self.dry_run && trigger == Trigger::Auto => {}
            Ok(reconciled) => {
                let mut effects = self.apply_reconciled(reconciled, trigger);
                effects.extend(self.run_pending_reconcile());
                return effects;
            }
        }
        self.run_pending_reconcile()
    }

    /// Replaces the mailbox list and the queue's rows with Fastmail's, then
    /// runs the due-snooze check on the snooze mailbox's threads, keeping
    /// the cursor on its thread. The
    /// filter keeps the mailbox ids it resolved to. Only `R` reaches here
    /// in dry run, where the undo history is cleared too.
    fn apply_reconciled(&mut self, reconciled: Reconciled, trigger: Trigger) -> Vec<Effect> {
        self.mailboxes = Mailboxes::from(reconciled.mailboxes);
        self.total = reconciled.result.total;
        if self.dry_run && trigger == Trigger::Refresh {
            self.dry_schedule = Some(self.schedule.clone());
        }
        let (threads, mut effects) = self.queue_threads(reconciled.result);
        self.queue.reconcile(threads);
        if self.dry_run && trigger == Trigger::Refresh {
            self.history.clear();
            self.report("Queue reset to Fastmail, undo history cleared");
        }
        let current = self.queue.current().map(|t| t.id.clone());
        effects.extend(match reconciled.snoozed {
            Some(snoozed) if self.mailboxes.snooze().is_some() => {
                self.return_due(Thread::all_from_query(snoozed))
            }
            _ => self.forget_due(),
        });
        if let Some(id) = current {
            self.queue.to_thread(&id);
        }
        effects
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

/// What `update` does to each email of `threads` it patches.
fn changes(threads: &[Thread], update: &EmailUpdate) -> Vec<Change> {
    threads
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
        .collect()
}

/// The dry-run log effect for `step` on `entry`.
fn log_dry_run(step: Step, entry: &Entry) -> Effect {
    Effect::LogDryRun {
        step,
        action: entry.action.clone(),
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

/// The quit effect with `unconfirmed` actions Fastmail did not confirm.
fn quit_with(unconfirmed: usize) -> Effect {
    match unconfirmed {
        0 => Effect::Quit,
        n => Effect::QuitUnconfirmed(n),
    }
}

/// `1 action`, `3 actions`.
fn actions(n: usize) -> String {
    match n {
        1 => "1 action".into(),
        n => format!("{n} actions"),
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
            app.due_at_launch = true;
            app.set_filter("")
        }
        Msg::SetFilter(text) => app.set_filter(&text),
        Msg::OpenFilter => {
            app.open_filter();
            Vec::new()
        }
        Msg::OpenCommand => {
            app.open_command();
            Vec::new()
        }
        Msg::Prompt(key) if app.command_prompt.is_some() => app.command_key(key),
        Msg::Prompt(key) => app.filter_key(key),
        Msg::ToggleSort => app.set_sort(match app.sort {
            Sort::Newest => Sort::Oldest,
            Sort::Oldest => Sort::Newest,
        }),
        Msg::SetSort(sort) => app.set_sort(sort),
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
        Msg::Quit => app.quit(),
        Msg::QuitTimeout => app.quit_timeout(),
        Msg::Archive => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Archive,
                count,
            })
        }
        Msg::Delete => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Delete,
                count,
            })
        }
        Msg::Spam => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Spam,
                count,
            })
        }
        Msg::Move(to) => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Move(to),
                count,
            })
        }
        Msg::OpenSnooze => {
            app.open_snooze();
            Vec::new()
        }
        Msg::OpenMove => {
            app.open_move();
            Vec::new()
        }
        Msg::Picker(key) if app.move_picker.is_some() => app.move_picker_key(key),
        Msg::Picker(key) => app.picker_key(key),
        Msg::Snooze(when) => {
            let count = app.take_count();
            match when.resolve(&app.now.to_zoned(app.tz.clone())) {
                Some(until) => app.perform(Repeat {
                    action: Action::Snooze(until.timestamp()),
                    count,
                }),
                None => {
                    app.status = "Snooze time out of range".into();
                    Vec::new()
                }
            }
        }
        Msg::Unsnooze => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Unsnooze,
                count,
            })
        }
        Msg::ToggleFlag => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::ToggleFlag,
                count,
            })
        }
        Msg::ToggleRead => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::ToggleRead,
                count,
            })
        }
        Msg::SetRead(on) => {
            let count = app.take_count();
            app.perform(Repeat {
                action: Action::Read(on),
                count,
            })
        }
        Msg::MailboxCreated { job, result } => app.mailbox_created(job, result),
        Msg::SnoozedLoaded { result } => app.snoozed_loaded(result),
        Msg::ScheduleFailed { job, error } => app.schedule_failed(job, &error),
        Msg::Repeat => {
            let count = app.take_count();
            match app.last_action.clone() {
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
            app.set_dry_run(!app.dry_run);
            Vec::new()
        }
        Msg::SetDryRun(on) => {
            app.set_dry_run(on);
            Vec::new()
        }
        Msg::Sent { job, result } => app.sent(job, result),
        Msg::Replayed { job, result } => app.replayed(job, result),
        Msg::Tick => {
            app.spinner = app.spinner.wrapping_add(1);
            Vec::new()
        }
        Msg::Wake => {
            app.probe_now();
            app.reconcile(Trigger::Auto)
        }
        Msg::ReconcileTick => {
            app.next_reconcile_at = app.now + RECONCILE_INTERVAL;
            app.reconcile(Trigger::Auto)
        }
        Msg::Refresh => {
            app.take_count();
            app.probe_now();
            app.reconcile(Trigger::Refresh)
        }
        Msg::Retrying { from, retry } => {
            if app.awaits(from) {
                app.retrying.insert(from, retry);
            }
            Vec::new()
        }
        Msg::Reconciled { generation, result } => app.reconciled(generation, result),
        Msg::Loaded { generation, result } => {
            if let Err(error) = &result
                && let Some(fatal) = error.fatal()
            {
                return vec![Effect::Fatal(fatal)];
            }
            if generation != app.generation {
                return Vec::new();
            }
            app.record_reply(&result);
            app.loading = false;
            app.retrying.remove(&Source::Query(generation));
            let mut effects = Vec::new();
            match result {
                Ok(result) => {
                    if app.previous.take().is_some() {
                        app.queue.clear_selection();
                        app.queue.to_first();
                    }
                    app.total = result.total;
                    let (threads, forgotten) = app.queue_threads(result);
                    app.queue.replace(threads);
                    effects.extend(forgotten);
                }
                Err(error) => {
                    app.go_back();
                    app.query_failed(&error);
                }
            }
            // While disconnected, the check is left to the next reconcile.
            if std::mem::take(&mut app.due_at_launch) && app.connected {
                effects.extend(app.check_due());
            }
            effects.extend(app.run_pending_reconcile());
            effects
        }
    }
}
