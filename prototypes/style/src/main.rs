//! PROTOTYPE. Third round for wayfinder ticket 06: look and feel. The
//! structure is settled enough (a triage queue, filter bar, pinned views, bulk
//! selection, no reading pane); this round tries four skins and a set of
//! nvim-style mechanics (counts, visual line selection, a `:` command line,
//! `.` to repeat) on the same fake data. Switch with `[` and `]` or F1 to F4.
//! Nothing here talks to Fastmail. Do not grow this into the app.

use std::collections::HashSet;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

// ---------------------------------------------------------------- fake data

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
    mailbox: String,
    age: &'static str,
    ts: u32,
}

#[allow(clippy::too_many_arguments)]
fn t(
    id: u32,
    ts: u32,
    age: &'static str,
    mailbox: &str,
    from: &'static str,
    domain: &'static str,
    subject: &'static str,
    preview: &'static str,
    count: usize,
    flags: &str,
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
        mailbox: mailbox.into(),
        age,
        ts,
    }
}

fn fake_threads() -> Vec<Thread> {
    vec![
        t(1, 160, "2h", "Inbox", "Sam Reid", "example.org", "Re: Draft spec for blorp", "Thanks for the quick turnaround on the draft", 4, "ua"),
        t(2, 159, "3h", "Inbox", "GitHub", "github.com", "[tanem/blorp] Research findings: live sync (PR #4)", "html2text review requested", 1, "ul"),
        t(3, 158, "3h", "Inbox", "This Week in Rust", "this-week-in-rust.org", "This Week in Rust 621", "ratatui 0.30.2 is out", 1, "ul"),
        t(4, 157, "4h", "Inbox", "GitHub", "github.com", "[ratatui/ratatui] Table: Fill constraint ignores min width (#1870)", "mx-ferris commented", 6, "ul"),
        t(5, 156, "1d", "Inbox", "Fastmail", "fastmail.com", "Your receipt from Fastmail", "Receipt for Fastmail Standard, 1 year", 1, "a"),
        t(6, 155, "1d", "Inbox", "Kit Larrabee", "example.net", "Saturday lunch", "Are you still coming on Saturday?", 3, "uf"),
        t(7, 154, "1d", "Inbox", "Priya Nair", "example.org", "Offsite planning (Thu 14:00)", "Priya Nair has invited you", 1, "u"),
        t(8, 153, "1d", "Inbox", "Mighty Ape", "mightyape.co.nz", "Your order has shipped", "Should arrive Thursday", 1, ""),
        t(9, 152, "2d", "Inbox", "Priya Nair, Alex Chen", "example.org", "Re: Offsite agenda", "Can we move the retro to after lunch?", 7, "fa"),
        t(10, 151, "2d", "Inbox", "GitHub", "github.com", "[tanem/blorp] Add README, licence, and glossary (PR #1)", "Merged by tanem", 2, "l"),
        t(11, 150, "2d", "Inbox", "Dentist", "example.com", "Appointment reminder: Thu 15:00", "This is a reminder", 1, ""),
        t(12, 149, "3d", "Inbox", "crates.io", "crates.io", "New crate version published: html2text 0.17.3", "A crate you watch was published", 1, "l"),
        t(13, 148, "3d", "Inbox", "Alex Chen", "example.org", "Re: Keychain prompt on every rebuild", "That is the ad-hoc signature changing", 2, "u"),
        t(14, 147, "3d", "Inbox", "Alex Chen", "example.org", "Pairing on JMAP module (Fri 10:00)", "Alex Chen has invited you", 1, ""),
        t(15, 146, "4d", "Inbox", "Library", "example.com", "Items due in 3 days", "Due on 17 Oct", 1, ""),
        t(16, 145, "4d", "Inbox", "Sam Reid", "example.org", "Lunch next week?", "Tuesday or Wednesday works", 2, ""),
        t(17, 144, "4d", "Inbox", "Power Co", "example.com", "Your bill is ready", "September bill available", 1, "a"),
        t(18, 143, "5d", "Inbox", "GitHub", "github.com", "[tanem/dotfiles] Dependabot: bump actions/checkout (PR #88)", "Bumps actions/checkout from 4 to 5", 1, "l"),
        t(19, 142, "5d", "Inbox", "Rust Blog", "blog.rust-lang.org", "Announcing Rust 1.98.0", "A new version of Rust", 1, "l"),
        t(20, 141, "5d", "Inbox", "Sam Reid", "example.org", "Photos from the weekend", "Here are the ones that came out", 1, "a"),
        t(21, 140, "6d", "Newsletters", "Pragmatic Engineer", "pragmaticengineer.com", "The Pulse #112", "Tech hiring in 2026", 1, "ul"),
        t(22, 139, "6d", "Newsletters", "Bytes", "bytes.dev", "Bytes #350: the one about signals", "Signals are back, again", 1, "ul"),
        t(23, 138, "1w", "Receipts", "Apple", "apple.com", "Your receipt from Apple", "iCloud+ 200 GB", 1, "a"),
        t(24, 137, "1w", "Projects/blorp", "Alex Chen", "example.org", "Re: EventSource quirks on Fastmail", "The first event carries the state strings", 5, "f"),
        t(25, 136, "1w", "Travel", "Air NZ", "airnewzealand.co.nz", "Your itinerary: WLG to AKL", "Booking reference 7XK2QP", 1, "a"),
    ]
}

const MAILBOXES: &[&str] =
    &["Inbox", "Archive", "Newsletters", "Receipts", "Projects/blorp", "Projects/house", "Travel", "Family", "Trash", "Junk", "Snoozed"];

const VIEWS: &[(&str, &str)] = &[
    ("Inbox", "in:inbox"),
    ("Unread", "is:unread"),
    ("GitHub", "from:github"),
    ("Lists", "is:list"),
    ("Flagged", "is:flagged"),
    ("All", ""),
];

const AGENDA: &[(&str, &str)] = &[("10:30", "1:1 with Priya"), ("12:30", "Lunch with Sam"), ("15:00", "Dentist")];

const SNOOZE_PRESETS: &[(char, &str, &str)] = &[
    ('l', "Later today", "today 18:00"),
    ('t', "Tomorrow morning", "Thu 08:00"),
    ('w', "This weekend", "Sat 08:00"),
    ('n', "Next week", "Mon 08:00"),
];

// ---------------------------------------------------------------- app state

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    C64,
    Arcade,
    Nvim,
    Minimal,
}

impl Variant {
    const ALL: [Variant; 4] = [Variant::C64, Variant::Arcade, Variant::Nvim, Variant::Minimal];
    fn key(self) -> &'static str {
        match self {
            Variant::C64 => "H",
            Variant::Arcade => "I",
            Variant::Nvim => "J",
            Variant::Minimal => "K",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Variant::C64 => "C64: blue screen, uppercase, READY. prompt, directory listing",
            Variant::Arcade => "Arcade: black, HUD with score and lives, stage clear at inbox zero",
            Variant::Nvim => "Nvim: no borders, relative numbers, lualine statusline, : command line",
            Variant::Minimal => "Minimal: one column, one accent, breadcrumb, nano-style key hints",
        }
    }
    fn index(self) -> usize {
        Variant::ALL.iter().position(|v| *v == self).unwrap()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Filter,
    Cmd,
    Visual,
}

enum Picker {
    Snooze { sel: usize },
    Move { sel: usize, filter: String },
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Archive,
    Delete,
    Spam,
    Flag,
    Read,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Action::Archive => "Archived",
            Action::Delete => "Deleted",
            Action::Spam => "Marked as spam",
            Action::Flag => "Toggled flag on",
            Action::Read => "Toggled read on",
        }
    }
    fn apply(self, t: &mut Thread) {
        match self {
            Action::Archive => t.mailbox = "Archive".into(),
            Action::Delete => t.mailbox = "Trash".into(),
            Action::Spam => t.mailbox = "Junk".into(),
            Action::Flag => t.flagged = !t.flagged,
            Action::Read => t.unread = !t.unread,
        }
    }
}

struct Undo {
    label: String,
    threads: Vec<Thread>,
    cursor: usize,
    oldest_first: bool,
    score: u32,
    hi_score: u32,
}

struct App {
    threads: Vec<Thread>,
    cursor: usize,
    view: usize,
    filter: String,
    cmd: String,
    mode: Mode,
    anchor: usize,
    selected: HashSet<u32>,
    count: String,
    last_action: Option<Action>,
    undo: Vec<Undo>,
    status: String,
    dry_run: bool,
    picker: Option<Picker>,
    help: bool,
    variant: Variant,
    agenda_open: bool,
    oldest_first: bool,
    score: u32,
    hi_score: u32,
    combo: u32,
    quit: bool,
}

impl App {
    fn new() -> Self {
        App {
            threads: fake_threads(),
            cursor: 0,
            view: 0,
            filter: String::new(),
            cmd: String::new(),
            mode: Mode::Normal,
            anchor: 0,
            selected: HashSet::new(),
            count: String::new(),
            last_action: None,
            undo: Vec::new(),
            status: "Loaded 25 threads".into(),
            dry_run: true,
            picker: None,
            help: false,
            variant: Variant::C64,
            agenda_open: true,
            oldest_first: false,
            score: 0,
            hi_score: 1200,
            combo: 0,
            quit: false,
        }
    }

    fn set_variant(&mut self, v: Variant) {
        self.variant = v;
        self.count.clear();
        self.cursor = 0;
        self.selected.clear();
        self.mode = Mode::Normal;
        self.agenda_open = matches!(v, Variant::C64 | Variant::Arcade);
    }

    fn set_view(&mut self, view: usize) {
        self.view = view;
        self.count.clear();
        self.cursor = 0;
        self.selected.clear();
        self.mode = Mode::Normal;
    }

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

    fn clamp(&mut self) {
        let len = self.visible().len();
        self.cursor = if len == 0 { 0 } else { self.cursor.min(len - 1) };
    }

    fn move_by(&mut self, delta: i32) {
        let len = self.visible().len() as i32;
        if len == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, len - 1) as usize;
    }

    /// Visual range as row positions, inclusive.
    fn visual_range(&self) -> Option<(usize, usize)> {
        (self.mode == Mode::Visual).then(|| (self.anchor.min(self.cursor), self.anchor.max(self.cursor)))
    }

    fn is_marked(&self, pos: usize, id: u32) -> bool {
        self.selected.contains(&id) || self.visual_range().is_some_and(|(a, b)| pos >= a && pos <= b)
    }

    fn take_count(&mut self) -> usize {
        let n = self.count.parse().unwrap_or(1);
        self.count.clear();
        n.max(1)
    }

    /// What an action applies to, in order: visual range, then explicit
    /// selection, then `count` rows from the cursor.
    fn targets(&self, count: usize) -> Vec<usize> {
        let vis = self.visible();
        if let Some((a, b)) = self.visual_range() {
            return vis[a.min(vis.len())..(b + 1).min(vis.len())].to_vec();
        }
        if !self.selected.is_empty() {
            return vis.into_iter().filter(|&i| self.selected.contains(&self.threads[i].id)).collect();
        }
        vis.into_iter().skip(self.cursor).take(count).collect()
    }

    fn snapshot(&mut self, label: String) {
        self.undo.push(Undo {
            label,
            threads: self.threads.clone(),
            cursor: self.cursor,
            oldest_first: self.oldest_first,
            score: self.score,
            hi_score: self.hi_score,
        });
        if self.undo.len() > 20 {
            self.undo.remove(0);
        }
    }

    fn act(&mut self, action: Action, count: usize) {
        let targets = self.targets(count);
        if targets.is_empty() {
            self.status = "Nothing to act on".into();
            return;
        }
        if let Some((a, _)) = self.visual_range() {
            self.cursor = a;
        }
        let label = if targets.len() == 1 {
            format!("{} \"{}\"", action.verb(), self.threads[targets[0]].subject)
        } else {
            format!("{} {} threads", action.verb(), targets.len())
        };
        self.snapshot(label.clone());
        for &i in &targets {
            action.apply(&mut self.threads[i]);
        }
        self.combo = if self.last_action == Some(action) { self.combo + 1 } else { 1 };
        self.score += 10 * targets.len() as u32 * self.combo.min(8);
        self.hi_score = self.hi_score.max(self.score);
        self.last_action = Some(action);
        self.selected.clear();
        self.mode = Mode::Normal;
        self.clamp();
        self.status = if self.dry_run { format!("[dry run] {label}") } else { label };
    }

    fn act_with(&mut self, verb: &str, f: impl Fn(&mut Thread)) {
        let targets = self.targets(1);
        if targets.is_empty() {
            self.status = "Nothing to act on".into();
            return;
        }
        if let Some((a, _)) = self.visual_range() {
            self.cursor = a;
        }
        self.last_action = None;
        self.combo = 0;
        let label = if targets.len() == 1 {
            format!("{verb} \"{}\"", self.threads[targets[0]].subject)
        } else {
            format!("{verb} {} threads", targets.len())
        };
        self.snapshot(label.clone());
        for &i in &targets {
            f(&mut self.threads[i]);
        }
        self.score += 10 * targets.len() as u32;
        self.hi_score = self.hi_score.max(self.score);
        self.selected.clear();
        self.mode = Mode::Normal;
        self.clamp();
        self.status = if self.dry_run { format!("[dry run] {label}") } else { label };
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(u) => {
                self.threads = u.threads;
                self.oldest_first = u.oldest_first;
                self.cursor = u.cursor;
                self.score = u.score;
                self.hi_score = u.hi_score;
                self.combo = 0;
                self.selected.clear();
                self.mode = Mode::Normal;
                self.clamp();
                self.status = format!("Undid: {}", u.label);
            }
            None => self.status = "Nothing to undo".into(),
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind != KeyEventKind::Press {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match k.code {
            KeyCode::F(1) => return self.set_variant(Variant::C64),
            KeyCode::F(2) => return self.set_variant(Variant::Arcade),
            KeyCode::F(3) => return self.set_variant(Variant::Nvim),
            KeyCode::F(4) => return self.set_variant(Variant::Minimal),
            _ => {}
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.picker.is_some() {
            return self.picker_key(k);
        }
        match self.mode {
            Mode::Filter => {
                match k.code {
                    KeyCode::Enter => self.mode = Mode::Normal,
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.filter.clear();
                    }
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char('u') if ctrl => self.filter.clear(),
                    KeyCode::Char(c) if !ctrl => self.filter.push(c),
                    _ => {}
                }
                self.cursor = 0;
                self.selected.clear();
            }
            Mode::Cmd => match k.code {
                KeyCode::Enter => {
                    let cmd = std::mem::take(&mut self.cmd);
                    self.mode = Mode::Normal;
                    self.run_cmd(&cmd);
                }
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.cmd.clear();
                }
                KeyCode::Backspace => {
                    if self.cmd.pop().is_none() {
                        self.mode = Mode::Normal;
                    }
                }
                KeyCode::Char(c) if !ctrl => self.cmd.push(c),
                _ => {}
            },
            Mode::Normal | Mode::Visual => self.normal_key(k),
        }
    }

    fn normal_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char(c @ '0'..='9') if !(c == '0' && self.count.is_empty()) => {
                self.count.push(c);
                return;
            }
            _ => {}
        }
        // Every key below either consumes the count through take_count or has
        // no use for it; `take_count` reads it before this clears it.
        let pending = std::mem::take(&mut self.count);
        let uses_count = matches!(
            k.code,
            KeyCode::Char('j' | 'k' | 'G' | 'x' | 'e' | 'd' | '!' | 'f' | 'r' | '.') | KeyCode::Down | KeyCode::Up | KeyCode::End
        );
        if uses_count {
            self.count = pending;
        }
        match k.code {
            KeyCode::Char('[') => self.set_variant(Variant::ALL[(self.variant.index() + 3) % 4]),
            KeyCode::Char(']') => self.set_variant(Variant::ALL[(self.variant.index() + 1) % 4]),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char(':') => {
                self.mode = Mode::Cmd;
                self.cmd.clear();
            }
            KeyCode::Tab => self.set_view((self.view + 1) % VIEWS.len()),
            KeyCode::BackTab => self.set_view((self.view + VIEWS.len() - 1) % VIEWS.len()),
            KeyCode::Char('j') | KeyCode::Down => {
                let n = self.take_count() as i32;
                self.move_by(n);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let n = self.take_count() as i32;
                self.move_by(-n);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.count.clear();
                self.move_by(-10000);
            }
            KeyCode::Char('G') | KeyCode::End => {
                if self.count.is_empty() {
                    self.move_by(10000);
                } else {
                    let n = self.take_count();
                    self.cursor = 0;
                    self.move_by(n as i32 - 1);
                }
            }
            KeyCode::Char('V') => {
                if self.mode == Mode::Visual {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = Mode::Visual;
                    self.anchor = self.cursor;
                    self.selected.clear();
                }
            }
            KeyCode::Char('x') => {
                let n = self.take_count();
                let vis = self.visible();
                for &i in vis.iter().skip(self.cursor).take(n) {
                    let id = self.threads[i].id;
                    if !self.selected.remove(&id) {
                        self.selected.insert(id);
                    }
                }
                self.move_by(n as i32);
            }
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
                self.count.clear();
                self.mode = Mode::Normal;
                self.cursor = 0;
            }
            KeyCode::Char('e') => {
                let n = self.take_count();
                self.act(Action::Archive, n)
            }
            KeyCode::Char('d') => {
                let n = self.take_count();
                self.act(Action::Delete, n)
            }
            KeyCode::Char('!') => {
                let n = self.take_count();
                self.act(Action::Spam, n)
            }
            KeyCode::Char('f') => {
                let n = self.take_count();
                self.act(Action::Flag, n)
            }
            KeyCode::Char('r') => {
                let n = self.take_count();
                self.act(Action::Read, n)
            }
            KeyCode::Char('.') => {
                let n = self.take_count();
                match self.last_action {
                    Some(a) => self.act(a, n),
                    None => self.status = "Nothing to repeat".into(),
                }
            }
            KeyCode::Char('z') => {
                self.count.clear();
                if self.targets(1).is_empty() {
                    self.status = "Nothing to snooze".into();
                } else {
                    self.picker = Some(Picker::Snooze { sel: 0 });
                }
            }
            KeyCode::Char('m') => {
                self.count.clear();
                if self.targets(1).is_empty() {
                    self.status = "Nothing to move".into();
                } else {
                    self.picker = Some(Picker::Move { sel: 0, filter: String::new() });
                }
            }
            KeyCode::Char('o') => {
                self.count.clear();
                match self.visible().get(self.cursor) {
                    Some(&i) => {
                        self.status =
                            format!("Would open https://app.fastmail.com/mail/Inbox/T{} in the browser", self.threads[i].id)
                    }
                    None => self.status = "Nothing to open".into(),
                }
            }
            KeyCode::Char('u') => {
                self.count.clear();
                self.undo()
            }
            KeyCode::Char('t') => {
                self.oldest_first = !self.oldest_first;
                self.cursor = 0;
                self.status = if self.oldest_first { "Oldest first".into() } else { "Newest first".into() };
            }
            KeyCode::Char('a') => self.agenda_open = !self.agenda_open,
            KeyCode::Char('R') => self.status = "Refreshed (stub, nothing changed)".into(),
            KeyCode::Char('D') => {
                self.dry_run = !self.dry_run;
                self.status = if self.dry_run { "Dry run on".into() } else { "Dry run off".into() };
            }
            _ => self.count.clear(),
        }
    }

    fn run_cmd(&mut self, cmd: &str) {
        let mut parts = cmd.trim().splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match name {
            "" => {}
            "q" | "quit" => self.quit = true,
            "archive" | "e" => self.act(Action::Archive, 1),
            "delete" | "d" => self.act(Action::Delete, 1),
            "spam" => self.act(Action::Spam, 1),
            "flag" => self.act(Action::Flag, 1),
            "read" => self.act(Action::Read, 1),
            "undo" | "u" => self.undo(),
            "help" | "h" => self.help = true,
            "view" | "v" => match VIEWS.iter().position(|(n, _)| n.eq_ignore_ascii_case(arg)) {
                Some(i) => self.set_view(i),
                None => self.status = format!("No view \"{arg}\". Views: {}", VIEWS.iter().map(|v| v.0).collect::<Vec<_>>().join(", ")),
            },
            "move" | "m" => match MAILBOXES.iter().find(|m| m.eq_ignore_ascii_case(arg)) {
                Some(mb) => {
                    let mb = mb.to_string();
                    self.act_with(&format!("Moved to {mb}:"), |t| t.mailbox = mb.clone());
                }
                None => self.status = format!("No mailbox \"{arg}\""),
            },
            "snooze" | "z" => match snooze_alias(arg) {
                Some(when) => {
                    let when = when.to_string();
                    self.act_with(&format!("Snoozed until {when}:"), |t| t.mailbox = "Snoozed".into());
                }
                None => self.status = "Usage: :snooze later|tomorrow|weekend|next".into(),
            },
            "filter" | "f" => {
                self.filter = arg.to_string();
                self.cursor = 0;
                self.selected.clear();
                self.mode = Mode::Normal;
            }
            "sort" => {
                self.oldest_first = arg == "oldest";
                self.cursor = 0;
            }
            "dry" => self.dry_run = arg != "off",
            _ => self.status = format!("Not a command: {name}"),
        }
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Some(picker) = self.picker.take() else { return };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match picker {
            Picker::Snooze { mut sel } => {
                let n = SNOOZE_PRESETS.len();
                match k.code {
                    KeyCode::Esc => self.status = "Snooze cancelled".into(),
                    KeyCode::Char('j') | KeyCode::Down => {
                        sel = (sel + 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        sel = (sel + n - 1) % n;
                        self.picker = Some(Picker::Snooze { sel });
                    }
                    KeyCode::Enter => self.snooze(sel),
                    KeyCode::Char(c) if !ctrl => match SNOOZE_PRESETS.iter().position(|p| p.0 == c) {
                        Some(i) => self.snooze(i),
                        None => match c.to_digit(10) {
                            Some(d) if (1..=n as u32).contains(&d) => self.snooze(d as usize - 1),
                            _ => self.picker = Some(Picker::Snooze { sel }),
                        },
                    },
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
                            self.act_with(&format!("Moved to {mb}:"), |t| t.mailbox = mb.clone());
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
        let when = SNOOZE_PRESETS[i].2.to_string();
        self.act_with(&format!("Snoozed until {when}:"), |t| t.mailbox = "Snoozed".into());
    }
}

/// `:snooze` argument to a preset time: later, tomorrow, weekend, next.
fn snooze_alias(arg: &str) -> Option<String> {
    let i = match arg.to_lowercase().as_str() {
        "later" | "today" => 0,
        "tomorrow" => 1,
        "weekend" => 2,
        "next" | "week" => 3,
        _ => return None,
    };
    Some(SNOOZE_PRESETS[i].2.to_string())
}

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
                _ => false,
            }
        } else if tok == "has:attachment" {
            t.attachment
        } else {
            t.from.to_lowercase().contains(&tok) || t.subject.to_lowercase().contains(&tok) || t.preview.to_lowercase().contains(&tok)
        }
    })
}

fn move_matches(filter: &str) -> Vec<&'static str> {
    let f = filter.to_lowercase();
    MAILBOXES.iter().copied().filter(|m| m.to_lowercase().contains(&f)).collect()
}

// ---------------------------------------------------------------- rendering

struct Skin {
    bg: Color,
    fg: Color,
    dim: Color,
    accent: Color,
    accent2: Color,
    cursor: Style,
    mark: Color,
}

fn skin(v: Variant) -> Skin {
    match v {
        Variant::C64 => Skin {
            bg: Color::Rgb(0x40, 0x31, 0x8d),
            fg: Color::Rgb(0x78, 0x69, 0xc4),
            dim: Color::Rgb(0x60, 0x53, 0xb0),
            accent: Color::Rgb(0xff, 0xff, 0xff),
            accent2: Color::Rgb(0x9f, 0xef, 0xe0),
            cursor: Style::new().bg(Color::Rgb(0x78, 0x69, 0xc4)).fg(Color::Rgb(0x40, 0x31, 0x8d)),
            mark: Color::Rgb(0xff, 0xff, 0x55),
        },
        Variant::Arcade => Skin {
            bg: Color::Black,
            fg: Color::Rgb(0xe0, 0xe0, 0xe0),
            dim: Color::Rgb(0x70, 0x70, 0x70),
            accent: Color::Rgb(0xff, 0xd7, 0x00),
            accent2: Color::Rgb(0xff, 0x40, 0x40),
            cursor: Style::new().bg(Color::Rgb(0xff, 0xd7, 0x00)).fg(Color::Black).bold(),
            mark: Color::Rgb(0x40, 0xff, 0x80),
        },
        Variant::Nvim => Skin {
            bg: Color::Rgb(0x1f, 0x1f, 0x28),
            fg: Color::Rgb(0xdc, 0xd7, 0xba),
            dim: Color::Rgb(0x72, 0x71, 0x69),
            accent: Color::Rgb(0x7e, 0x9c, 0xd8),
            accent2: Color::Rgb(0x98, 0xbb, 0x6c),
            cursor: Style::new().bg(Color::Rgb(0x2d, 0x4f, 0x67)),
            mark: Color::Rgb(0xe6, 0xc3, 0x84),
        },
        Variant::Minimal => Skin {
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Rgb(0xd7, 0x5f, 0x5f),
            accent2: Color::Rgb(0xd7, 0x5f, 0x5f),
            cursor: Style::new().add_modifier(Modifier::REVERSED),
            mark: Color::Rgb(0xd7, 0x5f, 0x5f),
        },
    }
}

fn ui(f: &mut Frame, app: &App) {
    let s = skin(app.variant);
    f.render_widget(Block::new().style(Style::new().bg(s.bg).fg(s.fg)), f.area());
    let [body, switcher] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    match app.variant {
        Variant::C64 => variant_c64(f, app, &s, body),
        Variant::Arcade => variant_arcade(f, app, &s, body),
        Variant::Nvim => variant_nvim(f, app, &s, body),
        Variant::Minimal => variant_minimal(f, app, &s, body),
    }
    switcher_bar(f, app, switcher);
    match &app.picker {
        Some(Picker::Snooze { sel }) => snooze_picker(f, app, &s, *sel),
        Some(Picker::Move { sel, filter }) => move_picker(f, &s, *sel, filter),
        None => {}
    }
    if app.help {
        help_overlay(f, &s);
    }
}

/// One row, shared by every skin; the skin decides colours and the caller decides
/// whether to uppercase.
fn row_line(app: &App, s: &Skin, pos: usize, i: usize, width: usize, upper: bool, numbered: bool) -> Line<'static> {
    let t = &app.threads[i];
    let marked = app.is_marked(pos, t.id);
    let at_cursor = pos == app.cursor;
    let up = |x: &str| if upper { x.to_uppercase() } else { x.to_string() };
    let mut spans: Vec<Span> = Vec::new();
    if numbered {
        let rel = (pos as i32 - app.cursor as i32).abs();
        let num = if at_cursor { format!("{:>3} ", pos + 1) } else { format!("{rel:>3} ") };
        spans.push(Span::styled(num, Style::new().fg(if at_cursor { s.accent } else { s.dim })));
    }
    spans.push(Span::styled(if marked { "▌" } else { " " }, Style::new().fg(s.mark)));
    spans.push(Span::styled(if t.unread { "● " } else { "  " }, Style::new().fg(s.accent)));
    spans.push(Span::styled(if t.flagged { "⚑ " } else { "  " }, Style::new().fg(s.accent2)));
    let from_w = 18;
    let fixed = (if numbered { 4 } else { 0 }) + 1 + 2 + 2 + from_w + 1 + 4 + 4;
    let subj_w = width.saturating_sub(fixed).max(10);
    let from = format!("{:<from_w$} ", truncate(&up(t.from), from_w));
    spans.push(Span::styled(from, Style::new().fg(s.fg)));
    let subject = format!("{:<subj_w$}", truncate(&up(t.subject), subj_w));
    spans.push(Span::styled(subject, if t.unread { Style::new().fg(s.fg).bold() } else { Style::new().fg(s.fg) }));
    let count = if t.count > 1 { format!(" {:>2} ", t.count) } else { "    ".into() };
    spans.push(Span::styled(count, Style::new().fg(s.dim)));
    spans.push(Span::styled(format!("{:>4}", t.age), Style::new().fg(s.dim)));
    let line = Line::from(spans);
    if at_cursor { line.style(s.cursor) } else if marked { line.style(Style::new().fg(s.mark)) } else { line }
}

fn rows(f: &mut Frame, app: &App, s: &Skin, area: Rect, upper: bool, numbered: bool) {
    let vis = app.visible();
    let height = area.height as usize;
    let first = app.cursor.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = vis
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(pos, &i)| row_line(app, s, pos, i, area.width as usize, upper, numbered))
        .collect();
    if vis.is_empty() {
        let msg = if upper { "NOTHING HERE. ESC CLEARS THE FILTER." } else { "Nothing here. Esc clears the filter." };
        f.render_widget(Paragraph::new(Line::from(msg)).style(Style::new().fg(s.dim)), area);
    } else {
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn mode_label(app: &App) -> &'static str {
    match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Filter => "FILTER",
        Mode::Cmd => "COMMAND",
        Mode::Visual => "V-LINE",
    }
}

/// The bottom input line: a filter being typed, a command being typed, or the
/// status message.
fn input_line(app: &App, s: &Skin, prompt_filter: &str, prompt_cmd: &str) -> Line<'static> {
    match app.mode {
        Mode::Filter => Line::from(vec![
            Span::styled(prompt_filter.to_string(), Style::new().fg(s.accent)),
            Span::raw(app.filter.clone()),
            Span::styled("▏", Style::new().fg(s.accent)),
        ]),
        Mode::Cmd => Line::from(vec![
            Span::styled(prompt_cmd.to_string(), Style::new().fg(s.accent)),
            Span::raw(app.cmd.clone()),
            Span::styled("▏", Style::new().fg(s.accent)),
        ]),
        _ => {
            let count = if app.count.is_empty() { String::new() } else { format!("  {}", app.count) };
            Line::from(vec![Span::raw(format!(" {}", app.status)), Span::styled(count, Style::new().fg(s.accent).bold())])
        }
    }
}

// ---- H: C64

fn variant_c64(f: &mut Frame, app: &App, s: &Skin, area: Rect) {
    let inner = Block::new().style(Style::new().bg(s.fg)).inner(area);
    let [border_top, body, border_bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Block::new().style(Style::new().bg(s.fg)), border_top);
    f.render_widget(Block::new().style(Style::new().bg(s.fg)), border_bottom);
    let [left, mid, right] = Layout::horizontal([Constraint::Length(2), Constraint::Fill(1), Constraint::Length(2)]).areas(body);
    f.render_widget(Block::new().style(Style::new().bg(s.fg)), left);
    f.render_widget(Block::new().style(Style::new().bg(s.fg)), right);
    let _ = inner;
    let [title, free, blank, list, blank2, ready, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(mid);
    let (view, q) = VIEWS[app.view];
    let n = app.count_for(q);
    f.render_widget(
        Paragraph::new(Line::from(format!("**** BLORP 64 TRIAGE V1  {} ****", view.to_uppercase()))).centered(),
        title,
    );
    let agenda = if app.agenda_open {
        AGENDA.iter().map(|(t, w)| format!("{t} {}", w.to_uppercase())).collect::<Vec<_>>().join("  ")
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(Line::from(format!(" {n} THREADS FREE.  {} SELECTED.  {agenda}", app.selected.len()))).centered(),
        free,
    );
    let _ = blank;
    let _ = blank2;
    rows(f, app, s, list, true, false);
    let ready_text = match app.mode {
        Mode::Visual => "VISUAL.",
        Mode::Filter => "FILTER?",
        Mode::Cmd => "COMMAND?",
        Mode::Normal => "READY.",
    };
    f.render_widget(Paragraph::new(Line::from(ready_text)), ready);
    let line = input_line(app, s, "/", ":").to_string().trim_start().to_uppercase();
    let dry = if app.dry_run { "  (DRY RUN)" } else { "" };
    f.render_widget(Paragraph::new(Line::from(format!("{line}{dry}"))), input);
}

// ---- I: Arcade

fn variant_arcade(f: &mut Frame, app: &App, s: &Skin, area: Rect) {
    let [hud, hud2, blank, list, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let (view, q) = VIEWS[app.view];
    let left = app.count_for(q);
    let lives = "♥".repeat(app.undo.len().min(5)) + &"·".repeat(5usize.saturating_sub(app.undo.len()));
    let hud_line = Line::from(vec![
        Span::styled("  1UP ", Style::new().fg(s.accent2).bold()),
        Span::styled(format!("{:06}", app.score), Style::new().fg(s.accent).bold()),
        Span::styled("   HI-SCORE ", Style::new().fg(s.accent2).bold()),
        Span::styled(format!("{:06}", app.hi_score), Style::new().fg(s.accent).bold()),
        Span::styled(format!("   STAGE {}", app.view + 1), Style::new().fg(s.fg)),
        Span::styled(format!(" {}", view.to_uppercase()), Style::new().fg(s.accent)),
        Span::styled(format!("   LEFT {left:02}"), Style::new().fg(s.fg)),
        Span::styled("   UNDO ", Style::new().fg(s.fg)),
        Span::styled(lives, Style::new().fg(s.accent2)),
    ]);
    f.render_widget(Paragraph::new(hud_line), hud);
    let combo = if app.combo > 1 {
        Span::styled(format!("  COMBO x{}", app.combo.min(8)), Style::new().fg(s.mark).bold())
    } else {
        Span::raw("")
    };
    let agenda = if app.agenda_open {
        Span::styled(
            format!("  NEXT {} {}", AGENDA[0].0, AGENDA[0].1.to_uppercase()),
            Style::new().fg(s.dim),
        )
    } else {
        Span::raw("")
    };
    let dry = if app.dry_run { Span::styled("  DEMO MODE", Style::new().fg(s.dim)) } else { Span::raw("") };
    f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), combo, agenda, dry])), hud2);
    let _ = blank;
    if app.visible().is_empty() {
        let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(3), Constraint::Fill(1)]).areas(list);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("STAGE CLEAR", Style::new().fg(s.accent).bold())),
                Line::from(""),
                Line::from(Span::styled("PRESS TAB FOR NEXT STAGE  ·  ESC TO CLEAR FILTER", Style::new().fg(s.dim))),
            ])
            .centered(),
            mid,
        );
    } else {
        rows(f, app, s, list, true, false);
    }
    let line = input_line(app, s, "SEARCH> ", "CMD> ");
    f.render_widget(Paragraph::new(line).style(Style::new().fg(s.accent)), foot);
}

// ---- J: Nvim

fn variant_nvim(f: &mut Frame, app: &App, s: &Skin, area: Rect) {
    let [tabs, list, statusline, cmdline] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let mut spans: Vec<Span> = Vec::new();
    for (i, (name, q)) in VIEWS.iter().enumerate() {
        let label = format!(" {name} {} ", app.count_for(q));
        spans.push(if i == app.view {
            Span::styled(label, Style::new().bg(s.cursor.bg.unwrap_or(s.dim)).fg(s.fg).bold())
        } else {
            Span::styled(label, Style::new().fg(s.dim))
        });
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Rgb(0x16, 0x16, 0x1d))), tabs);
    rows(f, app, s, list, false, true);
    let mode_bg = match app.mode {
        Mode::Normal => s.accent,
        Mode::Visual => s.mark,
        Mode::Filter | Mode::Cmd => s.accent2,
    };
    let vis_len = app.visible().len();
    let pos = if vis_len == 0 { "0/0".to_string() } else { format!("{}/{}", app.cursor + 1, vis_len) };
    let marked = app.visual_range().map(|(a, b)| b - a + 1).unwrap_or(app.selected.len());
    let mut status_spans = vec![
        Span::styled(format!(" {} ", mode_label(app)), Style::new().bg(mode_bg).fg(s.bg).bold()),
        Span::styled(format!(" {} ", VIEWS[app.view].0), Style::new().bg(Color::Rgb(0x2a, 0x2a, 0x37)).fg(s.fg)),
        Span::styled(
            if app.filter.is_empty() { String::new() } else { format!(" /{} ", app.filter) },
            Style::new().bg(Color::Rgb(0x2a, 0x2a, 0x37)).fg(s.accent2),
        ),
        Span::styled(format!(" {} ", app.status), Style::new().bg(Color::Rgb(0x16, 0x16, 0x1d)).fg(s.dim)),
    ];
    let right = format!(
        "{}{}{}{} {pos} ",
        if marked > 0 { format!(" {marked} marked ") } else { String::new() },
        if app.count.is_empty() { String::new() } else { format!(" {} ", app.count) },
        if app.dry_run { " dry " } else { "" },
        format!(" u{} ", app.undo.len()),
    );
    let used: usize = status_spans.iter().map(|sp| sp.content.chars().count()).sum();
    let pad = (statusline.width as usize).saturating_sub(used + right.chars().count());
    status_spans.push(Span::styled(" ".repeat(pad), Style::new().bg(Color::Rgb(0x16, 0x16, 0x1d))));
    status_spans.push(Span::styled(right, Style::new().bg(Color::Rgb(0x2a, 0x2a, 0x37)).fg(s.fg)));
    f.render_widget(Paragraph::new(Line::from(status_spans)), statusline);
    let cmd = match app.mode {
        Mode::Filter | Mode::Cmd => input_line(app, s, "/", ":"),
        Mode::Visual => Line::from(Span::styled("-- VISUAL LINE --", Style::new().fg(s.fg).bold())),
        Mode::Normal => Line::from(Span::styled(
            if app.agenda_open { format!("{} {}", AGENDA[0].0, AGENDA[0].1) } else { String::new() },
            Style::new().fg(s.dim),
        )),
    };
    f.render_widget(Paragraph::new(cmd), cmdline);
}

// ---- K: Minimal

fn variant_minimal(f: &mut Frame, app: &App, s: &Skin, area: Rect) {
    let agenda_h = if app.agenda_open { 1 } else { 0 };
    let [crumb, blank, list, agenda, status, hints1, hints2] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(agenda_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let vis = app.visible();
    let mut crumbs: Vec<Span> = vec![Span::styled(" blorp", Style::new().fg(s.accent).bold()), Span::styled(" › ", Style::new().fg(s.dim))];
    crumbs.push(Span::raw(VIEWS[app.view].0));
    if !app.filter.is_empty() {
        crumbs.push(Span::styled(" › ", Style::new().fg(s.dim)));
        crumbs.push(Span::raw(app.filter.clone()));
    }
    crumbs.push(Span::styled(format!("   {}", vis.len()), Style::new().fg(s.dim)));
    if !app.selected.is_empty() || app.mode == Mode::Visual {
        let n = app.visual_range().map(|(a, b)| b - a + 1).unwrap_or(app.selected.len());
        crumbs.push(Span::styled(format!("   {n} marked"), Style::new().fg(s.accent)));
    }
    if app.dry_run {
        crumbs.push(Span::styled("   dry run", Style::new().fg(s.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(crumbs)), crumb);
    let _ = blank;
    // One column: subject, then sender and age dimmed. No alignment grid.
    let height = list.height as usize;
    let first = app.cursor.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = vis
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(pos, &i)| {
            let t = &app.threads[i];
            let marked = app.is_marked(pos, t.id);
            let gutter = if pos == app.cursor { "▎" } else if marked { "•" } else { " " };
            let line = Line::from(vec![
                Span::styled(format!(" {gutter} "), Style::new().fg(s.accent)),
                Span::styled(t.subject, if t.unread { Style::new().bold() } else { Style::new() }),
                Span::styled(format!("  {}", t.from), Style::new().fg(s.dim)),
                Span::styled(if t.count > 1 { format!(" +{}", t.count - 1) } else { String::new() }, Style::new().fg(s.dim)),
                Span::styled(format!(" · {}", t.age), Style::new().fg(s.dim)),
                Span::styled(if t.flagged { " ⚑" } else { "" }, Style::new().fg(s.accent)),
            ]);
            if pos == app.cursor { line.style(Style::new().add_modifier(Modifier::BOLD)) } else { line }
        })
        .collect();
    if vis.is_empty() {
        f.render_widget(Paragraph::new("   Nothing here.").style(Style::new().fg(s.dim)), list);
    } else {
        f.render_widget(Paragraph::new(lines), list);
    }
    if app.agenda_open {
        let a = AGENDA.iter().map(|(t, w)| format!("{t} {w}")).collect::<Vec<_>>().join("   ");
        f.render_widget(Paragraph::new(Line::from(format!("   {a}"))).style(Style::new().fg(s.dim)), agenda);
    }
    f.render_widget(Paragraph::new(input_line(app, s, " /", " :")), status);
    let hint = |k: &str, w: &str| -> Vec<Span<'static>> {
        vec![
            Span::styled(format!("{k:>2}"), Style::new().add_modifier(Modifier::REVERSED)),
            Span::styled(format!(" {w:<9}"), Style::new().fg(s.dim)),
        ]
    };
    let mut l1 = vec![Span::raw(" ")];
    for (k, w) in [("e", "archive"), ("z", "snooze"), ("f", "flag"), ("d", "delete"), ("m", "move"), ("!", "spam"), ("o", "open")] {
        l1.extend(hint(k, w));
    }
    let mut l2 = vec![Span::raw(" ")];
    for (k, w) in [("/", "filter"), (":", "command"), ("V", "select"), ("x", "mark"), ("u", "undo"), ("⇥", "view"), ("?", "help")] {
        l2.extend(hint(k, w));
    }
    f.render_widget(Paragraph::new(Line::from(l1)), hints1);
    f.render_widget(Paragraph::new(Line::from(l2)), hints2);
}

// ---- shared chrome

fn switcher_bar(f: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        " PROTOTYPE ".black().on_yellow().bold(),
        "  [ ◀  ".into(),
        format!("{} ", app.variant.key()).bold(),
        app.variant.name().into(),
        "  ▶ ]   F1-F4 jump".dim(),
    ])
    .style(Style::new().bg(Color::Rgb(40, 40, 40)).fg(Color::White));
    f.render_widget(Paragraph::new(line), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [v] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(h);
    v
}

fn popup<'a>(f: &mut Frame, s: &Skin, title: &str, lines: Vec<Line<'a>>, width: u16) {
    let area = centered(f.area(), width, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(format!(" {title} ")).style(Style::new().bg(s.bg).fg(s.fg))),
        area,
    );
}

fn snooze_picker(f: &mut Frame, app: &App, s: &Skin, sel: usize) {
    let n = app.targets(1).len();
    let lines: Vec<Line> = SNOOZE_PRESETS
        .iter()
        .enumerate()
        .map(|(i, (k, name, when))| {
            let l = Line::from(vec![
                Span::styled(format!(" {k}  "), Style::new().fg(s.accent).bold()),
                Span::raw(format!("{name:<18}")),
                Span::styled((*when).to_string(), Style::new().fg(s.dim)),
            ]);
            if i == sel { l.style(s.cursor) } else { l }
        })
        .collect();
    let title = if n > 1 { format!("Snooze {n} threads") } else { "Snooze".to_string() };
    popup(f, s, &title, lines, 42);
}

fn move_picker(f: &mut Frame, s: &Skin, sel: usize, filter: &str) {
    let matches = move_matches(filter);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled(" Move to: ", Style::new().fg(s.dim)), Span::raw(filter.to_string()), Span::styled("▏", Style::new().fg(s.accent))]),
        Line::from(""),
    ];
    for (i, m) in matches.iter().enumerate() {
        let l = Line::from(format!("  {m}"));
        lines.push(if i == sel { l.style(s.cursor) } else { l });
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled("  no match", Style::new().fg(s.dim))));
    }
    popup(f, s, "Move", lines, 42);
}

fn help_overlay(f: &mut Frame, s: &Skin) {
    let rows = [
        ("j k", "move; a count prefix repeats: 3j"),
        ("g G", "first / last; 5G goes to row 5"),
        ("e d !", "archive / delete / spam; 3e acts on 3 rows"),
        ("f r", "flag / read toggle"),
        ("z m", "snooze / move (pickers)"),
        (".", "repeat the last action on the current row"),
        ("V", "visual line: j/k extend, then any action"),
        ("x X", "mark row (count ok) / mark all shown"),
        ("/", "filter: in: from: is:unread is:list has:attachment, text"),
        (":", "command: archive delete flag read undo move <mb>"),
        ("", "snooze <preset> view <name> filter <q> sort oldest q"),
        ("Tab", "next pinned view"),
        ("u", "undo (whole batch)"),
        ("o", "open in Fastmail web"),
        ("t a", "toggle sort / agenda"),
        ("Esc", "clear marks, filter, count"),
        ("[ ] F1-F4", "switch skin (prototype only)"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, what)| Line::from(vec![Span::styled(format!(" {k:<10}"), Style::new().fg(s.accent).bold()), Span::raw(*what)]))
        .collect();
    popup(f, s, "Keys", lines, 66);
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

/// `--snapshot` prints every skin and the nvim mechanics as plain text at
/// 120x36 and exits. Used to check rendering without a terminal.
fn snapshot() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = App::new();
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    let mut shot = |label: &str, app: &App| {
        terminal.draw(|f| ui(f, app)).unwrap();
        println!("==== {label}\n{}", terminal.backend());
    };
    let press = |app: &mut App, code: KeyCode| app.key(KeyEvent::new(code, KeyModifiers::NONE));
    let type_str = |app: &mut App, text: &str| {
        for c in text.chars() {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    };
    for v in Variant::ALL {
        app.set_variant(v);
        shot(v.name(), &app);
    }
    app.set_variant(Variant::Nvim);
    type_str(&mut app, "3j");
    type_str(&mut app, "V");
    type_str(&mut app, "2j");
    shot("nvim: 3j then V then 2j", &app);
    type_str(&mut app, "e");
    shot("nvim: e on the visual range", &app);
    type_str(&mut app, "2.");
    shot("nvim: 2. repeats archive on two rows", &app);
    type_str(&mut app, ":");
    type_str(&mut app, "move Travel");
    shot("nvim: typing :move Travel", &app);
    press(&mut app, KeyCode::Enter);
    type_str(&mut app, "u");
    type_str(&mut app, "u");
    type_str(&mut app, "u");
    shot("nvim: after three undos", &app);
    app.set_variant(Variant::Arcade);
    type_str(&mut app, "eeee");
    shot("arcade: four archives, combo", &app);
    type_str(&mut app, "/");
    type_str(&mut app, "from:nobody");
    press(&mut app, KeyCode::Enter);
    shot("arcade: empty result", &app);
    press(&mut app, KeyCode::Esc);
    app.set_variant(Variant::C64);
    type_str(&mut app, "z");
    shot("c64: snooze picker", &app);
    press(&mut app, KeyCode::Esc);
    app.set_variant(Variant::Minimal);
    type_str(&mut app, "xxj");
    shot("minimal: two marked", &app);
    type_str(&mut app, "?");
    shot("help", &app);
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
