//! PROTOTYPE. Second round for wayfinder ticket 06, after the reframe: blorp
//! is a triage queue in the Octobox sense, not a mail reader. No reading pane.
//! Dense rows, a filter bar, pinned views, bulk selection, and the agenda in
//! several roles. Four layout variants on fake data, switched with `[` and `]`
//! or F1 to F4. Nothing here talks to Fastmail. Do not grow this into the app.

use std::collections::HashSet;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Padding, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

// ---------------------------------------------------------------- fake data

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mail,
    Invitation,
}

#[derive(Clone)]
struct Thread {
    id: u32,
    from: &'static str,
    domain: &'static str,
    subject: &'static str,
    preview: &'static str,
    count: usize,
    unread: bool,
    flagged: bool,
    attachment: bool,
    list: bool,
    kind: Kind,
    mailbox: String,
    date: &'static str,
    ts: u32,
    snoozed_until: Option<String>,
    rsvp: Option<&'static str>,
}

#[allow(clippy::too_many_arguments)]
fn t(
    id: u32,
    ts: u32,
    date: &'static str,
    mailbox: &str,
    from: &'static str,
    domain: &'static str,
    subject: &'static str,
    preview: &'static str,
    count: usize,
    flags: &str, // letters: u unread, f flagged, a attachment, l list, i invitation
) -> Thread {
    Thread {
        id,
        from,
        domain,
        subject,
        preview,
        count,
        unread: flags.contains('u'),
        flagged: flags.contains('f'),
        attachment: flags.contains('a'),
        list: flags.contains('l'),
        kind: if flags.contains('i') { Kind::Invitation } else { Kind::Mail },
        mailbox: mailbox.into(),
        date,
        ts,
        snoozed_until: None,
        rsvp: None,
    }
}

fn fake_threads() -> Vec<Thread> {
    vec![
        t(1, 160, "06:41", "Inbox", "Sam Reid", "example.org", "Re: Draft spec for blorp", "Thanks for the quick turnaround on the draft. A few things from my read", 4, "ua"),
        t(2, 159, "06:12", "Inbox", "GitHub", "github.com", "[tanem/blorp] Research findings: live sync (PR #4)", "html2text review requested by @tanem", 1, "ul"),
        t(3, 158, "05:55", "Inbox", "This Week in Rust", "this-week-in-rust.org", "This Week in Rust 621", "ratatui 0.30.2 is out with a fix to the table layout engine", 1, "ul"),
        t(4, 157, "05:30", "Inbox", "GitHub", "github.com", "[ratatui/ratatui] Table: Fill constraint ignores min width (#1870)", "mx-ferris commented: can you share a minimal repro?", 6, "ul"),
        t(5, 156, "Mon", "Inbox", "Fastmail", "fastmail.com", "Your receipt from Fastmail", "Receipt for Fastmail Standard, 1 year", 1, "a"),
        t(6, 155, "Mon", "Inbox", "Kit Larrabee", "example.net", "Saturday lunch", "Are you still coming on Saturday? Dee is asking about numbers", 3, "uf"),
        t(7, 154, "Mon", "Inbox", "Priya Nair", "example.org", "Invitation: Offsite planning (Thu 14:00)", "Priya Nair has invited you to Offsite planning", 1, "ui"),
        t(8, 153, "Mon", "Inbox", "Mighty Ape", "mightyape.co.nz", "Your order has shipped", "Your order has shipped and should arrive Thursday", 1, ""),
        t(9, 152, "Sun", "Inbox", "Priya Nair, Alex Chen", "example.org", "Re: Offsite agenda", "Can we move the retro to after lunch? Alex has a clash", 7, "fa"),
        t(10, 151, "Sun", "Inbox", "GitHub", "github.com", "[tanem/blorp] Add README, licence, and glossary (PR #1)", "Merged by tanem", 2, "l"),
        t(11, 150, "Sun", "Inbox", "Dentist", "example.com", "Appointment reminder: Thu 15:00", "This is a reminder of your appointment on Thursday at 15:00", 1, ""),
        t(12, 149, "Sat", "Inbox", "crates.io", "crates.io", "New crate version published: html2text 0.17.3", "A new version of a crate you watch was published", 1, "l"),
        t(13, 148, "Sat", "Inbox", "Alex Chen", "example.org", "Re: Keychain prompt on every rebuild", "That is the ad-hoc signature changing. Sign with a stable identity", 2, "u"),
        t(14, 147, "Sat", "Inbox", "Alex Chen", "example.org", "Invitation: Pairing on JMAP module (Fri 10:00)", "Alex Chen has invited you to Pairing on JMAP module", 1, "i"),
        t(15, 146, "Fri", "Inbox", "Library", "example.com", "Items due in 3 days", "The following items are due on 17 Oct", 1, ""),
        t(16, 145, "Fri", "Inbox", "Sam Reid", "example.org", "Lunch next week?", "Tuesday or Wednesday works for me", 2, ""),
        t(17, 144, "Fri", "Inbox", "Power Co", "example.com", "Your bill is ready", "Your bill for September is now available", 1, "a"),
        t(18, 143, "Thu", "Inbox", "GitHub", "github.com", "[tanem/dotfiles] Dependabot: bump actions/checkout (PR #88)", "Bumps actions/checkout from 4 to 5", 1, "l"),
        t(19, 142, "Thu", "Inbox", "Rust Blog", "blog.rust-lang.org", "Announcing Rust 1.98.0", "The Rust team is happy to announce a new version of Rust", 1, "l"),
        t(20, 141, "Thu", "Inbox", "Sam Reid", "example.org", "Photos from the weekend", "Here are the ones that came out", 1, "a"),
        t(21, 140, "Wed", "Newsletters", "Pragmatic Engineer", "pragmaticengineer.com", "The Pulse #112", "Tech hiring in 2026, what changed", 1, "ul"),
        t(22, 139, "Wed", "Newsletters", "This Week in Rust", "this-week-in-rust.org", "This Week in Rust 620", "ratatui 0.30.1, and a new edition of the book", 1, "l"),
        t(23, 138, "Wed", "Newsletters", "Bytes", "bytes.dev", "Bytes #350: the one about signals", "Signals are back, again", 1, "ul"),
        t(24, 137, "Tue", "Receipts", "Apple", "apple.com", "Your receipt from Apple", "iCloud+ 200 GB", 1, "a"),
        t(25, 136, "Tue", "Receipts", "Spark", "spark.co.nz", "Your Spark bill", "Your bill for September is ready to view", 1, "a"),
        t(26, 135, "Tue", "Projects/blorp", "Alex Chen", "example.org", "Re: EventSource quirks on Fastmail", "The first event carries the state strings, so you can skip the initial fetch", 5, "f"),
        t(27, 134, "Tue", "Projects/blorp", "Sam Reid", "example.org", "Snooze emulation idea", "What if you just used the Snoozed mailbox and a launchd job", 3, ""),
        t(28, 133, "Mon", "Travel", "Air NZ", "airnewzealand.co.nz", "Your itinerary: WLG to AKL", "Booking reference 7XK2QP", 1, "a"),
        t(29, 132, "Mon", "Travel", "Booking.com", "booking.com", "Your booking is confirmed", "Hotel Grand, 2 nights", 1, ""),
        t(30, 131, "Sun", "Family", "Kit Larrabee", "example.net", "Ladder", "Did you ever return it", 2, ""),
    ]
}

const MAILBOXES: &[&str] = &[
    "Inbox", "Archive", "Newsletters", "Receipts", "Projects/blorp", "Projects/house", "Travel", "Family", "Trash", "Junk", "Snoozed",
];

/// Pinned views: name and filter. The typed filter narrows further.
const VIEWS: &[(&str, &str)] = &[
    ("Inbox", "in:inbox"),
    ("Unread", "is:unread"),
    ("GitHub", "from:github"),
    ("Lists", "is:list"),
    ("Invites", "is:invite"),
    ("Flagged", "is:flagged"),
    ("All", ""),
];

/// Today's agenda: start, end, title.
const AGENDA: &[(&str, &str, &str)] = &[
    ("09:00", "09:30", "Standup"),
    ("10:30", "11:00", "1:1 with Priya"),
    ("12:30", "13:30", "Lunch with Sam"),
    ("15:00", "15:45", "Dentist"),
    ("18:00", "18:15", "Pick up parcel"),
];
const NOW: &str = "09:12";

const SNOOZE_PRESETS: &[(char, &str, &str)] = &[
    ('l', "Later today", "today 18:00"),
    ('t', "Tomorrow morning", "Thu 08:00"),
    ('w', "This weekend", "Sat 08:00"),
    ('n', "Next week", "Mon 08:00"),
    ('c', "Custom", "pick a time"),
];

// ---------------------------------------------------------------- app state

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Tabs,
    Sidebar,
    Grouped,
    Calendar,
}

impl Variant {
    const ALL: [Variant; 4] = [Variant::Tabs, Variant::Sidebar, Variant::Grouped, Variant::Calendar];
    fn key(self) -> &'static str {
        match self {
            Variant::Tabs => "D",
            Variant::Sidebar => "E",
            Variant::Grouped => "F",
            Variant::Calendar => "G",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Variant::Tabs => "Tabs: views across the top, one wide table, agenda as a line",
            Variant::Sidebar => "Sidebar: views and mailboxes left, table centre, agenda right",
            Variant::Grouped => "Grouped: rows grouped by sender, act on a whole group",
            Variant::Calendar => "Calendar: agenda drives snooze presets, peek pane on, invites answered inline",
        }
    }
    fn index(self) -> usize {
        Variant::ALL.iter().position(|v| *v == self).unwrap()
    }
}

enum Picker {
    Snooze { sel: usize },
    Move { sel: usize, filter: String },
}

#[derive(Clone)]
enum RowKind {
    Header { key: String, count: usize, collapsed: bool },
    Thread(usize),
}

struct Undo {
    label: String,
    threads: Vec<Thread>,
    cursor: usize,
    selected: HashSet<u32>,
    oldest_first: bool,
}

struct App {
    threads: Vec<Thread>,
    cursor: usize,
    view: usize,
    filter: String,
    typing: bool,
    selected: HashSet<u32>,
    collapsed: HashSet<String>,
    undo: Vec<Undo>,
    status: String,
    dry_run: bool,
    picker: Option<Picker>,
    help: bool,
    variant: Variant,
    agenda_open: bool,
    peek: bool,
    oldest_first: bool,
    quit: bool,
}

impl App {
    fn new() -> Self {
        App {
            threads: fake_threads(),
            cursor: 0,
            view: 0,
            filter: String::new(),
            typing: false,
            selected: HashSet::new(),
            collapsed: HashSet::new(),
            undo: Vec::new(),
            status: "Loaded 30 threads".into(),
            dry_run: true,
            picker: None,
            help: false,
            variant: Variant::Tabs,
            agenda_open: true,
            peek: false,
            oldest_first: false,
            quit: false,
        }
    }

    fn set_variant(&mut self, v: Variant) {
        self.variant = v;
        self.peek = v == Variant::Calendar;
        self.cursor = 0;
        self.selected.clear();
    }

    /// Changing what is shown drops the selection, so an action never hits
    /// rows that are no longer on screen.
    fn set_view(&mut self, view: usize) {
        self.view = view;
        self.cursor = 0;
        self.selected.clear();
    }

    /// Threads matching the current view and typed filter, in display order.
    fn visible(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.threads.len())
            .filter(|&i| matches(&self.threads[i], VIEWS[self.view].1) && matches(&self.threads[i], &self.filter))
            .collect();
        if self.oldest_first {
            v.sort_by_key(|&i| self.threads[i].ts);
        } else {
            v.sort_by_key(|&i| std::cmp::Reverse(self.threads[i].ts));
        }
        v
    }

    fn count_for(&self, query: &str) -> usize {
        self.threads.iter().filter(|t| matches(t, query)).count()
    }

    /// The rows the table shows: threads, or group headers plus threads.
    fn rows(&self) -> Vec<RowKind> {
        let vis = self.visible();
        if self.variant != Variant::Grouped {
            return vis.into_iter().map(RowKind::Thread).collect();
        }
        let mut order: Vec<&str> = Vec::new();
        for &i in &vis {
            if !order.contains(&self.threads[i].from) {
                order.push(self.threads[i].from);
            }
        }
        let mut rows = Vec::new();
        for key in order {
            let members: Vec<usize> = vis.iter().copied().filter(|&i| self.threads[i].from == key).collect();
            let collapsed = self.collapsed.contains(key);
            rows.push(RowKind::Header { key: key.to_string(), count: members.len(), collapsed });
            if !collapsed {
                rows.extend(members.into_iter().map(RowKind::Thread));
            }
        }
        rows
    }

    fn clamp(&mut self) {
        let len = self.rows().len();
        self.cursor = if len == 0 { 0 } else { self.cursor.min(len - 1) };
    }

    fn select(&mut self, delta: i32) {
        let len = self.rows().len() as i32;
        if len == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, len - 1) as usize;
    }

    fn current_thread(&self) -> Option<usize> {
        match self.rows().get(self.cursor) {
            Some(RowKind::Thread(i)) => Some(*i),
            _ => None,
        }
    }

    /// What an action applies to: the selection, else the row under the cursor
    /// (a group header stands for its members).
    fn targets(&self) -> Vec<usize> {
        if !self.selected.is_empty() {
            return (0..self.threads.len()).filter(|&i| self.selected.contains(&self.threads[i].id)).collect();
        }
        match self.rows().get(self.cursor) {
            Some(RowKind::Thread(i)) => vec![*i],
            Some(RowKind::Header { key, .. }) => {
                self.visible().into_iter().filter(|&i| self.threads[i].from == key).collect()
            }
            None => vec![],
        }
    }

    fn snapshot(&mut self, label: String) {
        self.undo.push(Undo {
            label,
            threads: self.threads.clone(),
            cursor: self.cursor,
            selected: self.selected.clone(),
            oldest_first: self.oldest_first,
        });
        if self.undo.len() > 20 {
            self.undo.remove(0);
        }
    }

    fn act(&mut self, verb: &str, f: impl Fn(&mut Thread)) {
        let targets = self.targets();
        if targets.is_empty() {
            self.status = "Nothing to act on".into();
            return;
        }
        let label = if targets.len() == 1 {
            format!("{verb} \"{}\"", self.threads[targets[0]].subject)
        } else {
            format!("{verb} {} threads", targets.len())
        };
        self.snapshot(label.clone());
        for i in targets {
            f(&mut self.threads[i]);
        }
        self.selected.clear();
        self.clamp();
        self.status = if self.dry_run { format!("[dry run] {label}") } else { label };
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(u) => {
                self.threads = u.threads;
                self.oldest_first = u.oldest_first;
                self.cursor = u.cursor;
                let visible: HashSet<u32> = self.visible().into_iter().map(|i| self.threads[i].id).collect();
                self.selected = u.selected.intersection(&visible).copied().collect();
                self.clamp();
                self.status = format!("Undid: {}", u.label);
            }
            None => self.status = "Nothing to undo".into(),
        }
    }

    fn toggle_select(&mut self) {
        match self.rows().get(self.cursor).cloned() {
            Some(RowKind::Thread(i)) => {
                let id = self.threads[i].id;
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.select(1);
            }
            Some(RowKind::Header { collapsed: true, .. }) => self.status = "Expand the group to select it".into(),
            Some(RowKind::Header { key, .. }) => {
                let ids: Vec<u32> =
                    self.visible().into_iter().filter(|&i| self.threads[i].from == key).map(|i| self.threads[i].id).collect();
                if ids.iter().all(|id| self.selected.contains(id)) {
                    for id in ids {
                        self.selected.remove(&id);
                    }
                } else {
                    self.selected.extend(ids);
                }
            }
            None => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind != KeyEventKind::Press {
            return;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match k.code {
            KeyCode::F(1) => return self.set_variant(Variant::Tabs),
            KeyCode::F(2) => return self.set_variant(Variant::Sidebar),
            KeyCode::F(3) => return self.set_variant(Variant::Grouped),
            KeyCode::F(4) => return self.set_variant(Variant::Calendar),
            _ => {}
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.picker.is_some() {
            return self.picker_key(k);
        }
        if self.typing {
            match k.code {
                KeyCode::Enter => self.typing = false,
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter.clear();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => self.filter.clear(),
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.cursor = 0;
            self.selected.clear();
            return;
        }
        match k.code {
            KeyCode::Char('[') => self.set_variant(Variant::ALL[(self.variant.index() + 3) % 4]),
            KeyCode::Char(']') => self.set_variant(Variant::ALL[(self.variant.index() + 1) % 4]),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Tab => self.set_view((self.view + 1) % VIEWS.len()),
            KeyCode::BackTab => self.set_view((self.view + VIEWS.len() - 1) % VIEWS.len()),
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n < VIEWS.len() {
                    self.set_view(n);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.select(1),
            KeyCode::Char('k') | KeyCode::Up => self.select(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select(-10000),
            KeyCode::Char('G') | KeyCode::End => self.select(10000),
            KeyCode::Char('x') => self.toggle_select(),
            KeyCode::Char('X') => {
                let vis = self.visible();
                if vis.iter().all(|&i| self.selected.contains(&self.threads[i].id)) {
                    self.selected.clear();
                } else {
                    self.selected = vis.iter().map(|&i| self.threads[i].id).collect();
                }
            }
            KeyCode::Esc => {
                self.selected.clear();
                self.filter.clear();
                self.cursor = 0;
            }
            KeyCode::Enter => {
                if let Some(RowKind::Header { key, .. }) = self.rows().get(self.cursor).cloned() {
                    if !self.collapsed.remove(&key) {
                        let members: Vec<u32> =
                            self.threads.iter().filter(|t| t.from == key).map(|t| t.id).collect();
                        for id in members {
                            self.selected.remove(&id);
                        }
                        self.collapsed.insert(key);
                    }
                }
            }
            KeyCode::Char('e') => self.act("Archived", |t| t.mailbox = "Archive".into()),
            KeyCode::Char('d') => self.act("Deleted", |t| t.mailbox = "Trash".into()),
            KeyCode::Char('!') => self.act("Marked as spam", |t| t.mailbox = "Junk".into()),
            KeyCode::Char('f') => self.act("Toggled flag on", |t| t.flagged = !t.flagged),
            KeyCode::Char('r') => self.act("Toggled read on", |t| t.unread = !t.unread),
            KeyCode::Char('z') => {
                if self.targets().is_empty() {
                    self.status = "Nothing to snooze".into();
                } else {
                    self.picker = Some(Picker::Snooze { sel: 0 });
                }
            }
            KeyCode::Char('m') => {
                if self.targets().is_empty() {
                    self.status = "Nothing to move".into();
                } else {
                    self.picker = Some(Picker::Move { sel: 0, filter: String::new() });
                }
            }
            KeyCode::Char('o') => match self.current_thread() {
                Some(i) => {
                    self.status = format!("Would open https://app.fastmail.com/mail/Inbox/T{} in the browser", self.threads[i].id)
                }
                None => self.status = "Nothing to open".into(),
            },
            KeyCode::Char('y') => self.rsvp("Accepted"),
            KeyCode::Char('n') => self.rsvp("Declined"),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('t') => {
                self.oldest_first = !self.oldest_first;
                self.cursor = 0;
                self.status = if self.oldest_first { "Oldest first".into() } else { "Newest first".into() };
            }
            KeyCode::Char('a') => self.agenda_open = !self.agenda_open,
            KeyCode::Char('p') => self.peek = !self.peek,
            KeyCode::Char('R') => self.status = "Refreshed (stub, nothing changed)".into(),
            KeyCode::Char('D') => {
                self.dry_run = !self.dry_run;
                self.status = if self.dry_run { "Dry run on".into() } else { "Dry run off".into() };
            }
            _ => {}
        }
    }

    fn rsvp(&mut self, answer: &'static str) {
        let targets = self.targets();
        if targets.iter().any(|&i| self.threads[i].kind != Kind::Invitation) {
            self.status = "Only invitations can be answered".into();
            return;
        }
        self.act(&format!("{answer} and archived"), move |t| {
            t.rsvp = Some(answer);
            t.mailbox = "Archive".into();
        });
    }

    fn snooze_options(&self) -> Vec<(char, String, String)> {
        let mut v: Vec<(char, String, String)> =
            SNOOZE_PRESETS.iter().map(|(k, n, w)| (*k, n.to_string(), w.to_string())).collect();
        if self.variant == Variant::Calendar {
            for (n, (_, end, title)) in AGENDA.iter().filter(|e| e.1 > NOW).take(3).enumerate() {
                v.push((char::from(b'6' + n as u8), format!("After {title}"), format!("today {end}")));
            }
        }
        v
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Some(picker) = self.picker.take() else { return };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match picker {
            Picker::Snooze { mut sel } => {
                let opts = self.snooze_options();
                let n = opts.len();
                match k.code {
                    KeyCode::Esc => self.status = "Snooze cancelled".into(),
                    KeyCode::Char('j') | KeyCode::Down => {
                        sel = (sel + 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Char('n') if ctrl => {
                        sel = (sel + 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        sel = (sel + n - 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Char('p') if ctrl => {
                        sel = (sel + n - 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Enter => self.snooze(sel),
                    KeyCode::Char(c) if !ctrl => {
                        if let Some(d) = c.to_digit(10) {
                            if (1..=n as u32).contains(&d) {
                                return self.snooze(d as usize - 1);
                            }
                        }
                        match opts.iter().position(|o| o.0 == c) {
                            Some(i) => self.snooze(i),
                            None => self.picker = Some(Picker::Snooze { sel }),
                        }
                    }
                    _ => self.picker = Some(Picker::Snooze { sel }),
                }
            }
            Picker::Move { mut sel, mut filter } => {
                let matches = move_matches(&filter);
                let n = matches.len();
                match k.code {
                    KeyCode::Esc => self.status = "Move cancelled".into(),
                    KeyCode::Down => {
                        if n > 0 {
                            sel = (sel + 1) % n;
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Up => {
                        if n > 0 {
                            sel = (sel + n - 1) % n;
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Char('n') | KeyCode::Char('p') if ctrl => {
                        if n > 0 {
                            sel = if k.code == KeyCode::Char('n') { (sel + 1) % n } else { (sel + n - 1) % n };
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Enter => match matches.get(sel) {
                        Some(mb) => {
                            let mb = mb.to_string();
                            self.act(&format!("Moved to {mb}:"), |t| t.mailbox = mb.clone());
                        }
                        None => self.status = "No mailbox matches".into(),
                    },
                    KeyCode::Backspace => {
                        filter.pop();
                        self.picker = Some(Picker::Move { sel: 0, filter });
                    }
                    KeyCode::Char(c) if !ctrl => {
                        filter.push(c);
                        self.picker = Some(Picker::Move { sel: 0, filter });
                    }
                    _ => self.picker = Some(Picker::Move { sel, filter }),
                }
            }
        }
    }

    fn snooze(&mut self, i: usize) {
        let opts = self.snooze_options();
        let Some((_, name, when)) = opts.get(i).cloned() else { return };
        if name == "Custom" {
            self.status = "Custom time entry is not in the prototype".into();
            return;
        }
        self.act(&format!("Snoozed until {when}:"), move |t| {
            t.mailbox = "Snoozed".into();
            t.snoozed_until = Some(when.clone());
        });
    }
}

/// Filter language: whitespace-separated tokens, all must match.
/// `in:x` mailbox, `from:x` sender or domain, `is:unread|flagged|list|invite`,
/// `has:attachment`, anything else is a substring of sender, subject, preview or mailbox.
fn matches(t: &Thread, query: &str) -> bool {
    query.split_whitespace().all(|tok| {
        let tok = tok.to_lowercase();
        if let Some(v) = tok.strip_prefix("in:") {
            t.mailbox.to_lowercase().contains(v)
        } else if let Some(v) = tok.strip_prefix("from:") {
            t.from.to_lowercase().contains(v) || t.domain.to_lowercase().contains(v)
        } else if let Some(v) = tok.strip_prefix("is:") {
            match v {
                "unread" => t.unread,
                "flagged" => t.flagged,
                "list" => t.list,
                "invite" => t.kind == Kind::Invitation,
                _ => false,
            }
        } else if tok == "has:attachment" {
            t.attachment
        } else {
            t.from.to_lowercase().contains(&tok)
                || t.subject.to_lowercase().contains(&tok)
                || t.preview.to_lowercase().contains(&tok)
                || t.mailbox.to_lowercase().contains(&tok)
        }
    })
}

fn move_matches(filter: &str) -> Vec<&'static str> {
    let f = filter.to_lowercase();
    MAILBOXES.iter().copied().filter(|m| m.to_lowercase().contains(&f)).collect()
}

// ---------------------------------------------------------------- rendering

fn ui(f: &mut Frame, app: &App) {
    let [body, status, switcher] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());

    match app.variant {
        Variant::Tabs => variant_tabs(f, app, body),
        Variant::Sidebar => variant_sidebar(f, app, body),
        Variant::Grouped => variant_grouped(f, app, body),
        Variant::Calendar => variant_calendar(f, app, body),
    }
    status_line(f, app, status);
    switcher_bar(f, app, switcher);

    match &app.picker {
        Some(Picker::Snooze { sel }) => snooze_picker(f, app, *sel),
        Some(Picker::Move { sel, filter }) => move_picker(f, *sel, filter),
        None => {}
    }
    if app.help {
        help_overlay(f);
    }
}

/// Splits off an optional peek pane at the bottom of `area`.
fn with_peek(f: &mut Frame, app: &App, area: Rect) -> Rect {
    if !app.peek {
        return area;
    }
    let [main, peek] = Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area);
    peek_pane(f, app, peek);
    main
}

fn variant_tabs(f: &mut Frame, app: &App, area: Rect) {
    let agenda_h = if app.agenda_open { 1 } else { 0 };
    let [tabs, filter, rest, agenda] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(agenda_h),
    ])
    .areas(area);
    view_tabs(f, app, tabs);
    filter_bar(f, app, filter, false);
    let table = with_peek(f, app, rest);
    queue_table(f, app, table);
    if app.agenda_open {
        agenda_line(f, agenda);
    }
}

fn variant_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let agenda_w = if app.agenda_open { 26 } else { 0 };
    let [side, main, agenda] =
        Layout::horizontal([Constraint::Length(22), Constraint::Fill(1), Constraint::Length(agenda_w)]).areas(area);
    sidebar(f, app, side);
    let [filter, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(main);
    filter_bar(f, app, filter, false);
    let table = with_peek(f, app, rest);
    queue_table(f, app, table);
    if app.agenda_open {
        agenda_pane(f, agenda, false);
    }
}

fn variant_grouped(f: &mut Frame, app: &App, area: Rect) {
    let agenda_w = if app.agenda_open { 26 } else { 0 };
    let [main, agenda] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(agenda_w)]).areas(area);
    let [filter, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(main);
    filter_bar(f, app, filter, true);
    let table = with_peek(f, app, rest);
    queue_table(f, app, table);
    if app.agenda_open {
        agenda_pane(f, agenda, false);
    }
}

fn variant_calendar(f: &mut Frame, app: &App, area: Rect) {
    let [main, agenda] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(30)]).areas(area);
    let [filter, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(main);
    filter_bar(f, app, filter, true);
    let table = with_peek(f, app, rest);
    queue_table(f, app, table);
    agenda_pane(f, agenda, true);
}

fn view_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = vec![" ".into()];
    for (i, (name, q)) in VIEWS.iter().enumerate() {
        let label = format!(" {}:{name} {} ", i + 1, app.count_for(q));
        spans.push(if i == app.view { label.black().on_cyan().bold() } else { label.dim() });
        spans.push(" ".into());
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn filter_bar(f: &mut Frame, app: &App, area: Rect, show_view: bool) {
    let mut spans: Vec<Span> = vec![" ".into()];
    if show_view {
        let (name, q) = VIEWS[app.view];
        spans.push(format!(" {name} {} ", app.count_for(q)).black().on_cyan().bold());
        spans.push("  Tab or 1-9 switches view   ".dim());
    }
    spans.push("/ ".yellow().bold());
    if app.filter.is_empty() && !app.typing {
        spans.push(
            if show_view { "filter (in: from: is: has: or text)" } else { "filter: in:x from:x is:unread is:flagged is:list is:invite has:attachment or any text" }
                .dim(),
        );
    } else {
        spans.push(app.filter.clone().into());
        if app.typing {
            spans.push("▏".yellow());
        }
    }
    let shown = app.visible().len();
    spans.push(format!("   {shown} shown").dim());
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn sidebar(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![Line::from(" Views").bold()];
    for (i, (name, q)) in VIEWS.iter().enumerate() {
        let l = Line::from(format!(" {} {:<11}{:>4}", i + 1, name, app.count_for(q)));
        lines.push(if i == app.view { l.black().on_cyan() } else { l });
    }
    lines.push(Line::from(""));
    lines.push(Line::from(" Mailboxes").bold());
    for mb in MAILBOXES {
        let q = format!("in:{}", mb.to_lowercase());
        let n = app.count_for(&q);
        let active = app.filter.to_lowercase() == q;
        let l = Line::from(format!("   {:<13}{:>4}", truncate(mb, 13), if n > 0 { n.to_string() } else { String::new() }));
        lines.push(if active { l.black().on_cyan() } else if n == 0 { l.dim() } else { l });
    }
    f.render_widget(Paragraph::new(lines).block(Block::new().borders(ratatui::widgets::Borders::RIGHT)), area);
}

fn queue_table(f: &mut Frame, app: &App, area: Rect) {
    let rows_kind = app.rows();
    let wide = area.width >= 110;
    let show_mailbox = wide && VIEWS[app.view].1 != "in:inbox";
    let rows: Vec<Row> = rows_kind
        .iter()
        .map(|r| match r {
            RowKind::Header { key, count, collapsed } => {
                let arrow = if *collapsed { "▸" } else { "▾" };
                Row::new(vec![
                    Line::from(""),
                    Line::from(""),
                    Line::from(""),
                    Line::from(""),
                    Line::from(format!("{arrow} {key}")).bold(),
                    Line::from(if *count == 1 { "1 thread".to_string() } else { format!("{count} threads") }).dim(),
                    Line::from(""),
                    Line::from(""),
                    Line::from(""),
                    Line::from(""),
                ])
                .style(Style::new().fg(Color::Cyan))
            }
            RowKind::Thread(i) => {
                let t = &app.threads[*i];
                let selected = app.selected.contains(&t.id);
                let mut style = if t.unread { Style::new().bold() } else { Style::new() };
                if selected {
                    style = style.fg(Color::Yellow);
                }
                let kind = match (t.kind, t.rsvp) {
                    (Kind::Invitation, None) => Line::from("▣").magenta(),
                    (Kind::Invitation, Some(_)) => Line::from("▣").dim(),
                    _ => Line::from(if t.attachment { "@" } else { " " }).dim(),
                };
                Row::new(vec![
                    Line::from(if selected { "✔" } else { " " }).yellow(),
                    Line::from(if t.unread { "●" } else { " " }).blue(),
                    Line::from(if t.flagged { "⚑" } else { " " }).yellow(),
                    kind,
                    Line::from(t.from),
                    Line::from(t.subject),
                    Line::from(t.preview).dim(),
                    Line::from(if t.mailbox == "Inbox" { "" } else { t.mailbox.as_str() }).cyan().dim(),
                    Line::from(if t.count > 1 { t.count.to_string() } else { String::new() }).dim(),
                    Line::from(t.date).dim(),
                ])
                .style(style)
            }
        })
        .collect();
    let widths = [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(if wide { 18 } else { 14 }),
        Constraint::Percentage(if wide { 32 } else { 40 }),
        Constraint::Fill(1),
        Constraint::Length(if show_mailbox { 14 } else { 0 }),
        Constraint::Length(2),
        Constraint::Length(5),
    ];
    let table = Table::new(rows, widths)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::new().padding(Padding::horizontal(1)));
    let mut state = TableState::default().with_selected(if rows_kind.is_empty() { None } else { Some(app.cursor) });
    f.render_stateful_widget(table, area, &mut state);
    if rows_kind.is_empty() {
        f.render_widget(Paragraph::new(" Nothing matches. Esc clears the filter and selection.").dim(), area);
    }
}

fn peek_pane(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Peek  p to hide ").padding(Padding::horizontal(1));
    let Some(i) = app.current_thread() else {
        f.render_widget(Paragraph::new("").block(block), area);
        return;
    };
    let t = &app.threads[i];
    let mut lines = vec![
        Line::from(vec![t.subject.bold(), "  ".into(), format!("{} · {}", t.from, t.domain).dim()]),
        Line::from(vec![
            "mailbox ".dim(),
            t.mailbox.clone().cyan(),
            "   messages ".dim(),
            t.count.to_string().into(),
            "   ".into(),
            if t.attachment { "has attachment".dim() } else { "".into() },
            match t.rsvp {
                Some(a) => format!("   {a}").magenta(),
                None => "".into(),
            },
        ]),
        Line::from(""),
        Line::from(t.preview),
    ];
    if t.kind == Kind::Invitation && t.rsvp.is_none() {
        lines.push(Line::from(vec!["y".yellow().bold(), " accept   ".into(), "n".yellow().bold(), " decline".into()]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn agenda_pane(f: &mut Frame, area: Rect, emphasise_next: bool) {
    let mut next_marked = false;
    let items: Vec<ListItem> = AGENDA
        .iter()
        .map(|(start, end, what)| {
            let past = *end <= NOW;
            let is_next = !past && !next_marked;
            if is_next {
                next_marked = true;
            }
            let line = Line::from(vec![(*start).cyan(), " ".into(), (*what).into()]);
            let line = if past {
                line.dim()
            } else if is_next && emphasise_next {
                line.bold()
            } else {
                line
            };
            ListItem::new(line)
        })
        .collect();
    let mut block = Block::bordered().title(format!(" Today  {NOW} ")).padding(Padding::horizontal(1));
    if emphasise_next {
        block = block.title_bottom(Line::from(" z then 6-8: after an event ").dim());
    }
    f.render_widget(List::new(items).block(block), area);
}

fn agenda_line(f: &mut Frame, area: Rect) {
    let mut v: Vec<Span> = vec![" Today ".bold()];
    for (i, (start, end, what)) in AGENDA.iter().filter(|e| e.1 > NOW).take(4).enumerate() {
        if i > 0 {
            v.push("  ·  ".dim());
        }
        let _ = end;
        v.push((*start).cyan());
        v.push(" ".into());
        v.push((*what).into());
    }
    v.push("    a hides".dim());
    f.render_widget(Paragraph::new(Line::from(v)), area);
}

fn status_line(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(48)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(vec![" ".into(), app.status.clone().into()])), left);
    let mut spans: Vec<Span> = Vec::new();
    if !app.selected.is_empty() {
        spans.push(format!(" {} selected ", app.selected.len()).black().on_yellow());
        spans.push(" ".into());
    }
    if app.dry_run {
        spans.push(" DRY RUN ".black().on_yellow());
        spans.push(" ".into());
    }
    spans.push(format!("undo {} ", app.undo.len()).dim());
    spans.push(" ? help ".dim());
    f.render_widget(Paragraph::new(Line::from(spans)).right_aligned(), right);
}

fn switcher_bar(f: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        " PROTOTYPE ".black().on_yellow().bold(),
        "  [ ◀  ".into(),
        format!("{} ", app.variant.key()).bold(),
        app.variant.name().into(),
        "  ▶ ]   F1-F4 jump".dim(),
    ])
    .style(Style::new().bg(Color::Rgb(40, 40, 40)));
    f.render_widget(Paragraph::new(line), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [v] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(h);
    v
}

fn snooze_picker(f: &mut Frame, app: &App, sel: usize) {
    let opts = app.snooze_options();
    let n = app.targets().len();
    let area = centered(f.area(), 46, opts.len() as u16 + 2);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = opts
        .iter()
        .enumerate()
        .map(|(i, (key, name, when))| {
            let line = Line::from(vec![format!(" {key}  ").yellow().bold(), format!("{name:<22}").into(), when.clone().dim()]);
            ListItem::new(if i == sel { line.reversed() } else { line })
        })
        .collect();
    let title = if n > 1 { format!(" Snooze {n} threads until ") } else { " Snooze until ".to_string() };
    f.render_widget(
        List::new(items).block(
            Block::bordered().title(title).title_bottom(Line::from(" key, j/k, Enter, Esc ").dim().right_aligned()),
        ),
        area,
    );
}

fn move_picker(f: &mut Frame, sel: usize, filter: &str) {
    let matches = move_matches(filter);
    let area = centered(f.area(), 44, MAILBOXES.len() as u16 + 4);
    f.render_widget(Clear, area);
    let mut lines: Vec<Line> = vec![Line::from(vec![" Move to: ".dim(), filter.into(), "▏".yellow()]), Line::from("")];
    for (i, m) in matches.iter().enumerate() {
        let line = Line::from(format!("  {m}"));
        lines.push(if i == sel { line.reversed() } else { line });
    }
    if matches.is_empty() {
        lines.push(Line::from("  no match").dim());
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Move ")
                .title_bottom(Line::from(" type to filter, ↑↓ or C-n C-p, Enter, Esc ").dim().right_aligned()),
        ),
        area,
    );
}

fn help_overlay(f: &mut Frame) {
    let rows = [
        ("j k ↑ ↓", "move"),
        ("g G", "first / last row"),
        ("/", "type a filter; Enter keeps it, Esc clears it"),
        ("Tab 1-9", "switch pinned view"),
        ("x X", "select row (or group) / select all shown"),
        ("Esc", "clear selection and filter"),
        ("Enter", "collapse or expand a group (F)"),
        ("e d !", "archive / delete / spam"),
        ("f r", "flag / read toggle"),
        ("z m", "snooze / move (pickers)"),
        ("y n", "accept / decline an invitation"),
        ("o", "open in Fastmail web"),
        ("u", "undo (whole batch)"),
        ("t", "toggle newest / oldest first"),
        ("a p", "toggle agenda / peek pane"),
        ("R D", "refresh / toggle dry run"),
        ("[ ] F1-F4", "switch layout variant (prototype only)"),
        ("q", "quit"),
    ];
    let area = centered(f.area(), 60, rows.len() as u16 + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> =
        rows.iter().map(|(k, what)| Line::from(vec![format!(" {k:<10}").yellow().bold(), (*what).into()])).collect();
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Keys ")), area);
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------- main loop

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    let mut app = App::new();
    while !app.quit {
        terminal.draw(|f| ui(f, &app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                app.key(k);
            }
        }
    }
    Ok(())
}

/// `--snapshot` prints every variant and the main interactions as plain text
/// at 140x40 and exits. Used to check rendering without a terminal.
fn snapshot() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = App::new();
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    let mut shot = |label: &str, app: &App| {
        terminal.draw(|f| ui(f, app)).unwrap();
        println!("==== {label}\n{}", terminal.backend());
    };
    let press = |app: &mut App, code: KeyCode| app.key(KeyEvent::new(code, KeyModifiers::NONE));
    for v in Variant::ALL {
        app.set_variant(v);
        shot(v.name(), &app);
    }
    app.set_variant(Variant::Tabs);
    press(&mut app, KeyCode::Char('/'));
    for c in "from:github".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    shot("typing a filter", &app);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('X'));
    shot("select all shown", &app);
    press(&mut app, KeyCode::Char('e'));
    shot("archived the batch", &app);
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Esc);
    shot("undone and cleared", &app);
    app.set_variant(Variant::Grouped);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('x'));
    shot("grouped: first group collapsed, second selected", &app);
    app.set_variant(Variant::Calendar);
    press(&mut app, KeyCode::Char('z'));
    shot("calendar snooze picker", &app);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('5'));
    shot("invites view", &app);
    press(&mut app, KeyCode::Char('y'));
    shot("accepted an invitation", &app);
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--snapshot") {
        snapshot();
        return Ok(());
    }
    let terminal = ratatui::init();
    let result = run(terminal);
    ratatui::restore();
    result
}
