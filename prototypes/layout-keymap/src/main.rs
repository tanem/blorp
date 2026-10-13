//! PROTOTYPE. Throwaway code answering wayfinder ticket 06: what should the
//! triage screen look like and how should the keys feel. Three layout variants
//! on fake data, switched with `[` and `]` or F1 to F3. Nothing here talks to
//! Fastmail and nothing is persisted. Do not grow this into the app.

use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, List, ListItem, Padding, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{DefaultTerminal, Frame};

// ---------------------------------------------------------------- fake data

#[derive(Clone)]
struct Thread {
    id: u32,
    subject: &'static str,
    from: &'static str,
    count: usize,
    unread: bool,
    flagged: bool,
    date: &'static str,
    ts: u32,
    preview: &'static str,
    body: &'static str,
    attachments: Vec<&'static str>,
    mailbox: String,
    snoozed_until: Option<String>,
}

fn thread(
    id: u32,
    ts: u32,
    date: &'static str,
    from: &'static str,
    subject: &'static str,
    count: usize,
    unread: bool,
    flagged: bool,
    preview: &'static str,
    body: &'static str,
    attachments: Vec<&'static str>,
) -> Thread {
    Thread {
        id,
        subject,
        from,
        count,
        unread,
        flagged,
        date,
        ts,
        preview,
        body,
        attachments,
        mailbox: "Inbox".into(),
        snoozed_until: None,
    }
}

const BODY_LONG: &str = "Hi Tane,\n\nThanks for the quick turnaround on the draft. A few things from my read:\n\n1. The intro assumes the reader already knows what JMAP is. One sentence would fix it.\n2. Section 3 has two paragraphs that say the same thing in different words.\n3. The table in section 5 overflows on a narrow screen.\n\nI have attached the marked-up PDF. Happy to go through it on a call if that is easier.\n\nSam\n\n> On 13 Oct 2026, at 17:02, Tane Morgan wrote:\n>\n> Here is the draft I promised. It is rough in places but the shape is there.\n> Let me know what you think before Friday.\n>\n> Tane";

const BODY_NEWSLETTER: &str = "This week in Rust\n\nratatui 0.30.2 is out with a fix to the table layout engine and a new termina backend. html2text 0.17.3 adds CSS display:none handling.\n\nIn the ecosystem, reqwest 0.13 moved to hyper 1.x throughout. The eventsource-stream crate had a maintenance release.\n\nCrate of the week: secrecy, for keeping tokens out of your Debug output.\n\nCall for participation: 14 issues tagged good-first-issue across tokio, serde, and clap.\n\nUnsubscribe: https://example.com/u/abc123";

const BODY_SHORT: &str = "Your order has shipped and should arrive Thursday.\n\nTracking: 1Z999AA10123456784\n\nThanks for shopping with us.";

const BODY_RECEIPT: &str = "Receipt for Fastmail Standard, 1 year.\n\nAmount: USD 60.00\nPaid: 14 Oct 2026\nCard ending: 4242\n\nA PDF copy is attached. No action is needed.";

const BODY_FAMILY: &str = "Are you still coming on Saturday? Dee is asking about numbers for lunch. Let me know by Thursday if you can.\n\nAlso, do you still have the ladder?";

const BODY_PR: &str = "tanem/blorp\n\nResearch findings: live sync #4\n\nhtml2text review requested by @tanem.\n\nThis PR records what the research subagent found about EventSource push against Fastmail. See docs/research/live-sync.md.\n\nView it on GitHub: https://github.com/tanem/blorp/pull/4";

fn fake_threads() -> Vec<Thread> {
    vec![
        thread(1, 140, "06:41", "Sam Reid", "Re: Draft spec for blorp", 4, true, false, "Thanks for the quick turnaround on the draft. A few things from my read", BODY_LONG, vec!["spec-marked-up.pdf"]),
        thread(2, 139, "06:12", "GitHub", "[tanem/blorp] Research findings: live sync (PR #4)", 1, true, false, "html2text review requested by @tanem", BODY_PR, vec![]),
        thread(3, 138, "05:55", "This Week in Rust", "This Week in Rust 621", 1, true, false, "ratatui 0.30.2 is out with a fix to the table layout engine", BODY_NEWSLETTER, vec![]),
        thread(4, 137, "Mon", "Fastmail", "Your receipt from Fastmail", 1, false, false, "Receipt for Fastmail Standard, 1 year", BODY_RECEIPT, vec!["receipt-2026-10.pdf"]),
        thread(5, 136, "Mon", "Kit Larrabee", "Saturday lunch", 3, true, true, "Are you still coming on Saturday? Dee is asking about numbers", BODY_FAMILY, vec![]),
        thread(6, 135, "Mon", "Mighty Ape", "Your order has shipped", 1, false, false, "Your order has shipped and should arrive Thursday", BODY_SHORT, vec![]),
        thread(7, 134, "Sun", "Priya Nair, Alex Chen", "Re: Offsite agenda", 7, false, true, "Can we move the retro to after lunch? Alex has a clash", BODY_LONG, vec!["agenda-v3.docx", "venue-map.png"]),
        thread(8, 133, "Sun", "Dentist", "Appointment reminder: Thu 15:00", 1, false, false, "This is a reminder of your appointment on Thursday at 15:00", BODY_SHORT, vec![]),
        thread(9, 132, "Sat", "crates.io", "New crate version published: html2text 0.17.3", 1, false, false, "A new version of a crate you watch was published", BODY_NEWSLETTER, vec![]),
        thread(10, 131, "Sat", "Alex Chen", "Re: Keychain prompt on every rebuild", 2, true, false, "That is the ad-hoc signature changing. Sign with a stable identity and it stops", BODY_LONG, vec![]),
        thread(11, 130, "Fri", "Library", "Items due in 3 days", 1, false, false, "The following items are due on 17 Oct", BODY_SHORT, vec![]),
        thread(12, 129, "Fri", "Sam Reid", "Lunch next week?", 2, false, false, "Tuesday or Wednesday works for me", BODY_FAMILY, vec![]),
        thread(13, 128, "Thu", "Fastmail", "Welcome to Fastmail", 1, false, false, "Here is how to get the most out of your account", BODY_NEWSLETTER, vec![]),
        thread(14, 127, "Thu", "Power Co", "Your bill is ready", 1, false, false, "Your bill for September is now available", BODY_RECEIPT, vec!["bill-sep-2026.pdf"]),
    ]
}

const MAILBOXES: &[&str] = &[
    "Archive", "Receipts", "Travel", "Newsletters", "Projects/blorp", "Projects/house", "Family", "Trash", "Junk",
];

const AGENDA: &[(&str, &str)] = &[
    ("09:00", "Standup"),
    ("10:30", "1:1 with Priya"),
    ("12:30", "Lunch with Sam"),
    ("15:00", "Dentist"),
    ("18:00", "Pick up parcel"),
];

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
    ThreePane,
    Stacked,
    Focus,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::ThreePane, Variant::Stacked, Variant::Focus];
    fn key(self) -> &'static str {
        match self {
            Variant::ThreePane => "A",
            Variant::Stacked => "B",
            Variant::Focus => "C",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Variant::ThreePane => "Three pane: queue left, reading right, agenda below",
            Variant::Stacked => "Stacked: dense queue on top, reading below, agenda strip right",
            Variant::Focus => "Focus: one thread at a time, queue as a strip",
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

struct Undo {
    label: String,
    threads: Vec<Thread>,
    selected: usize,
    oldest_first: bool,
}

struct App {
    threads: Vec<Thread>,
    selected: usize,
    oldest_first: bool,
    undo: Vec<Undo>,
    status: String,
    dry_run: bool,
    picker: Option<Picker>,
    help: bool,
    scroll: u16,
    variant: Variant,
    quit: bool,
}

impl App {
    fn new() -> Self {
        App {
            threads: fake_threads(),
            selected: 0,
            oldest_first: false,
            undo: Vec::new(),
            status: "Loaded 14 threads from Inbox".into(),
            dry_run: true,
            picker: None,
            help: false,
            scroll: 0,
            variant: Variant::ThreePane,
            quit: false,
        }
    }

    /// Indices into `threads` of what is in the queue, in display order.
    fn queue(&self) -> Vec<usize> {
        let mut q: Vec<usize> = (0..self.threads.len()).filter(|i| self.threads[*i].mailbox == "Inbox").collect();
        if self.oldest_first {
            q.sort_by_key(|i| self.threads[*i].ts);
        } else {
            q.sort_by_key(|i| std::cmp::Reverse(self.threads[*i].ts));
        }
        q
    }

    fn current(&self) -> Option<usize> {
        self.queue().get(self.selected).copied()
    }

    fn clamp(&mut self) {
        let len = self.queue().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    fn select(&mut self, delta: i32) {
        let len = self.queue().len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, len - 1) as usize;
        if next != self.selected {
            self.selected = next;
            self.scroll = 0;
        }
    }

    fn snapshot(&mut self, label: String) {
        self.undo.push(Undo {
            label,
            threads: self.threads.clone(),
            selected: self.selected,
            oldest_first: self.oldest_first,
        });
        if self.undo.len() > 20 {
            self.undo.remove(0);
        }
    }

    fn act(&mut self, verb: &str, f: impl FnOnce(&mut Thread)) {
        let Some(i) = self.current() else {
            self.status = "Queue is empty".into();
            return;
        };
        let subject = self.threads[i].subject.to_string();
        let label = format!("{verb} \"{subject}\"");
        self.snapshot(label.clone());
        f(&mut self.threads[i]);
        self.clamp();
        if self.current() != Some(i) {
            self.scroll = 0;
        }
        self.status = if self.dry_run { format!("[dry run] {label}") } else { label };
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(u) => {
                self.threads = u.threads;
                self.selected = u.selected;
                self.oldest_first = u.oldest_first;
                self.scroll = 0;
                self.status = format!("Undid: {}", u.label);
            }
            None => self.status = "Nothing to undo".into(),
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
        // Prototype switcher, always live.
        match k.code {
            KeyCode::F(1) => return self.variant = Variant::ThreePane,
            KeyCode::F(2) => return self.variant = Variant::Stacked,
            KeyCode::F(3) => return self.variant = Variant::Focus,
            _ => {}
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.picker.is_some() {
            return self.picker_key(k);
        }
        match k.code {
            KeyCode::Char('[') => self.variant = Variant::ALL[(self.variant.index() + 2) % 3],
            KeyCode::Char(']') => self.variant = Variant::ALL[(self.variant.index() + 1) % 3],
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('j') | KeyCode::Down => self.select(1),
            KeyCode::Char('k') | KeyCode::Up => self.select(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select(-1000),
            KeyCode::Char('G') | KeyCode::End => self.select(1000),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('e') => self.act("Archived", |t| t.mailbox = "Archive".into()),
            KeyCode::Char('d') => self.act("Deleted", |t| t.mailbox = "Trash".into()),
            KeyCode::Char('!') => self.act("Marked as spam", |t| t.mailbox = "Junk".into()),
            KeyCode::Char('f') => self.act("Toggled flag on", |t| t.flagged = !t.flagged),
            KeyCode::Char('r') => self.act("Toggled read on", |t| t.unread = !t.unread),
            KeyCode::Char('z') => match self.current() {
                Some(_) => self.picker = Some(Picker::Snooze { sel: 0 }),
                None => self.status = "Queue is empty".into(),
            },
            KeyCode::Char('m') => match self.current() {
                Some(_) => self.picker = Some(Picker::Move { sel: 0, filter: String::new() }),
                None => self.status = "Queue is empty".into(),
            },
            KeyCode::Char('o') => match self.current() {
                Some(i) => {
                    self.status = format!("Would open https://app.fastmail.com/mail/Inbox/T{} in the browser", self.threads[i].id)
                }
                None => self.status = "Queue is empty".into(),
            },
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('t') => {
                self.oldest_first = !self.oldest_first;
                self.selected = 0;
                self.scroll = 0;
                self.status = if self.oldest_first { "Oldest first".into() } else { "Newest first".into() };
            }
            KeyCode::Char('R') => self.status = "Refreshed (stub, nothing changed)".into(),
            KeyCode::Char('D') => {
                self.dry_run = !self.dry_run;
                self.status = if self.dry_run { "Dry run on".into() } else { "Dry run off".into() };
            }
            _ => {}
        }
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Some(picker) = self.picker.take() else { return };
        match picker {
            Picker::Snooze { mut sel } => match k.code {
                KeyCode::Esc => self.status = "Snooze cancelled".into(),
                KeyCode::Char('j') | KeyCode::Down => {
                    sel = (sel + 1) % SNOOZE_PRESETS.len();
                    self.picker = Some(Picker::Snooze { sel });
                }
                KeyCode::Char('n') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    sel = (sel + 1) % SNOOZE_PRESETS.len();
                    self.picker = Some(Picker::Snooze { sel });
                }
                KeyCode::Char('p') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    sel = (sel + SNOOZE_PRESETS.len() - 1) % SNOOZE_PRESETS.len();
                    self.picker = Some(Picker::Snooze { sel });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    sel = (sel + SNOOZE_PRESETS.len() - 1) % SNOOZE_PRESETS.len();
                    self.picker = Some(Picker::Snooze { sel });
                }
                KeyCode::Enter => self.snooze(sel),
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(d) = c.to_digit(10) {
                        if (1..=SNOOZE_PRESETS.len() as u32).contains(&d) {
                            return self.snooze(d as usize - 1);
                        }
                    }
                    match SNOOZE_PRESETS.iter().position(|p| p.0 == c) {
                        Some(i) => self.snooze(i),
                        None => self.picker = Some(Picker::Snooze { sel }),
                    }
                }
                _ => self.picker = Some(Picker::Snooze { sel }),
            },
            Picker::Move { mut sel, mut filter } => {
                let matches = move_matches(&filter);
                match k.code {
                    KeyCode::Esc => self.status = "Move cancelled".into(),
                    KeyCode::Down => {
                        if !matches.is_empty() {
                            sel = (sel + 1) % matches.len();
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Up => {
                        if !matches.is_empty() {
                            sel = (sel + matches.len() - 1) % matches.len();
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Char('n') | KeyCode::Char('p') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        if !matches.is_empty() {
                            sel = if k.code == KeyCode::Char('n') { (sel + 1) % matches.len() } else { (sel + matches.len() - 1) % matches.len() };
                        }
                        self.picker = Some(Picker::Move { sel, filter });
                    }
                    KeyCode::Enter => match matches.get(sel) {
                        Some(mb) => {
                            let mb = mb.to_string();
                            self.act(&format!("Moved to {mb}:"), |t| t.mailbox = mb);
                        }
                        None => self.status = "No mailbox matches".into(),
                    },
                    KeyCode::Backspace => {
                        filter.pop();
                        self.picker = Some(Picker::Move { sel: 0, filter });
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        self.picker = Some(Picker::Move { sel: 0, filter });
                    }
                    _ => self.picker = Some(Picker::Move { sel, filter }),
                }
            }
        }
    }

    fn snooze(&mut self, preset: usize) {
        let (_, name, when) = SNOOZE_PRESETS[preset];
        if name == "Custom" {
            self.status = "Custom time entry is not in the prototype".into();
            return;
        }
        let when = when.to_string();
        self.act(&format!("Snoozed until {when}:"), |t| {
            t.mailbox = "Snoozed".into();
            t.snoozed_until = Some(when);
        });
    }
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
        Variant::ThreePane => variant_three_pane(f, app, body),
        Variant::Stacked => variant_stacked(f, app, body),
        Variant::Focus => variant_focus(f, app, body),
    }
    status_line(f, app, status);
    switcher_bar(f, app, switcher);

    match &app.picker {
        Some(Picker::Snooze { sel }) => snooze_picker(f, *sel),
        Some(Picker::Move { sel, filter }) => move_picker(f, *sel, filter),
        None => {}
    }
    if app.help {
        help_overlay(f);
    }
}

fn variant_three_pane(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(42), Constraint::Fill(1)]).areas(area);
    let [reading, agenda] = Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(right);
    queue_table(f, app, left, false);
    reading_pane(f, app, reading);
    agenda_pane(f, agenda);
}

fn variant_stacked(f: &mut Frame, app: &App, area: Rect) {
    let [main, agenda] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(26)]).areas(area);
    let [queue, reading] = Layout::vertical([Constraint::Percentage(40), Constraint::Fill(1)]).areas(main);
    queue_table(f, app, queue, true);
    reading_pane(f, app, reading);
    agenda_pane(f, agenda);
}

fn variant_focus(f: &mut Frame, app: &App, area: Rect) {
    let [strip, reading, agenda] =
        Layout::vertical([Constraint::Length(5), Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let q = app.queue();
    let title = format!(" Queue {}/{} ", if q.is_empty() { 0 } else { app.selected + 1 }, q.len());
    let mut lines = Vec::new();
    for offset in -1i32..=1 {
        let idx = app.selected as i32 + offset;
        let line = if idx < 0 || idx as usize >= q.len() {
            Line::from("")
        } else {
            let t = &app.threads[q[idx as usize]];
            let mut l = queue_line(t, area.width.saturating_sub(4) as usize);
            if offset == 0 {
                l = l.bold().reversed();
            } else {
                l = l.dim();
            }
            l
        };
        lines.push(line);
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title).padding(Padding::horizontal(1))),
        strip,
    );
    reading_pane(f, app, reading);
    let agenda_line: Vec<Span> = {
        let mut v = vec![" Agenda ".bold(), " ".into()];
        for (i, (time, what)) in AGENDA.iter().take(4).enumerate() {
            if i > 0 {
                v.push("  ·  ".dim());
            }
            v.push(time.cyan());
            v.push(" ".into());
            v.push((*what).into());
        }
        v
    };
    f.render_widget(Paragraph::new(Line::from(agenda_line)), agenda);
}

fn queue_line(t: &Thread, width: usize) -> Line<'static> {
    let marker = if t.unread { "●" } else { " " };
    let flag = if t.flagged { "⚑" } else { " " };
    let from_w = 18;
    let from = truncate(t.from, from_w);
    let count = if t.count > 1 { format!(" ({})", t.count) } else { String::new() };
    let fixed = 2 + 2 + from_w + 1 + count.len() + 1 + 6;
    let subj_w = width.saturating_sub(fixed).max(10);
    let subject = truncate(t.subject, subj_w);
    Line::from(vec![
        Span::styled(format!("{marker} "), Style::new().fg(Color::Blue)),
        Span::styled(format!("{flag} "), Style::new().fg(Color::Yellow)),
        Span::raw(format!("{from:<from_w$} ")),
        Span::styled(format!("{subject:<subj_w$}"), if t.unread { Style::new().bold() } else { Style::new() }),
        Span::styled(count, Style::new().dim()),
        Span::styled(format!(" {:>6}", t.date), Style::new().dim()),
    ])
}

fn queue_table(f: &mut Frame, app: &App, area: Rect, dense: bool) {
    let q = app.queue();
    let title = format!(
        " Inbox  {} threads  {} ",
        q.len(),
        if app.oldest_first { "oldest first" } else { "newest first" }
    );
    let rows: Vec<Row> = q
        .iter()
        .map(|&i| {
            let t = &app.threads[i];
            let style = if t.unread { Style::new().bold() } else { Style::new() };
            let marker = Span::styled(if t.unread { "●" } else { " " }, Style::new().fg(Color::Blue));
            let flag = Span::styled(if t.flagged { "⚑" } else { " " }, Style::new().fg(Color::Yellow));
            let count = if t.count > 1 { format!("{}", t.count) } else { String::new() };
            if dense {
                Row::new(vec![
                    Line::from(marker),
                    Line::from(flag),
                    Line::from(t.from),
                    Line::from(t.subject),
                    Line::from(t.preview).dim(),
                    Line::from(count).dim(),
                    Line::from(t.date).dim(),
                ])
                .style(style)
            } else {
                Row::new(vec![
                    Line::from(marker),
                    Line::from(flag),
                    Line::from(t.from),
                    Line::from(t.subject),
                    Line::from(count).dim(),
                    Line::from(t.date).dim(),
                ])
                .style(style)
            }
        })
        .collect();
    let widths: Vec<Constraint> = if dense {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(18),
            Constraint::Percentage(35),
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Length(5),
        ]
    } else {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(16),
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Length(5),
        ]
    };
    let table = Table::new(rows, widths)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(title));
    let mut state = TableState::default().with_selected(if q.is_empty() { None } else { Some(app.selected) });
    f.render_stateful_widget(table, area, &mut state);
}

fn reading_pane(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Thread ").padding(Padding::horizontal(1));
    let Some(i) = app.current() else {
        f.render_widget(
            Paragraph::new("Inbox zero. Press q to quit, u to undo, t to change order.").block(block),
            area,
        );
        return;
    };
    let t = &app.threads[i];
    let mut lines: Vec<Line> = vec![
        Line::from(t.subject).bold(),
        Line::from(vec!["From: ".dim(), t.from.into()]),
        Line::from(vec!["To:   ".dim(), "Tane Morgan".into()]),
        Line::from(vec!["Date: ".dim(), full_date(t.date).into()]),
    ];
    if t.count > 1 {
        lines.push(Line::from(vec![
            "Thread: ".dim(),
            format!("{} messages, showing the latest", t.count).into(),
        ]));
    }
    if !t.attachments.is_empty() {
        let mut spans: Vec<Span> = vec!["Attachments: ".dim()];
        for (n, a) in t.attachments.iter().enumerate() {
            if n > 0 {
                spans.push(", ".into());
            }
            spans.push(Span::styled(*a, Style::new().fg(Color::Cyan).underlined()));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    for raw in t.body.lines() {
        if raw.starts_with('>') {
            lines.push(Line::from(raw.to_string()).dim());
        } else {
            lines.push(Line::from(raw.to_string()));
        }
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.scroll, 0)).block(block),
        area,
    );
}

fn agenda_pane(f: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = AGENDA
        .iter()
        .map(|(time, what)| ListItem::new(Line::from(vec![(*time).cyan(), "  ".into(), (*what).into()])))
        .collect();
    f.render_widget(
        List::new(items).block(Block::bordered().title(" Today ").padding(Padding::horizontal(1))),
        area,
    );
}

fn status_line(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(40)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(vec![" ".into(), app.status.clone().into()])), left);
    let mut spans: Vec<Span> = Vec::new();
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
        "  ▶ ]   F1 F2 F3 jump".dim(),
    ])
    .style(Style::new().bg(Color::Rgb(40, 40, 40)));
    f.render_widget(Paragraph::new(line), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [v] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(h);
    v
}

fn snooze_picker(f: &mut Frame, sel: usize) {
    let area = centered(f.area(), 44, SNOOZE_PRESETS.len() as u16 + 2);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = SNOOZE_PRESETS
        .iter()
        .enumerate()
        .map(|(i, (key, name, when))| {
            let line = Line::from(vec![
                format!(" {key}  ").yellow().bold(),
                format!("{name:<18}").into(),
                (*when).dim(),
            ]);
            ListItem::new(if i == sel { line.reversed() } else { line })
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::bordered()
                .title(" Snooze until ")
                .title_bottom(Line::from(" key, 1-5, j/k Enter, Esc ").dim().right_aligned()),
        ),
        area,
    );
}

fn move_picker(f: &mut Frame, sel: usize, filter: &str) {
    let matches = move_matches(filter);
    let area = centered(f.area(), 44, MAILBOXES.len() as u16 + 4);
    f.render_widget(Clear, area);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![" Move to: ".dim(), filter.into(), "_".into()]),
        Line::from(""),
    ];
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
                .title(" Move thread ")
                .title_bottom(Line::from(" type to filter, ↑↓ or C-n C-p, Enter, Esc ").dim().right_aligned()),
        ),
        area,
    );
}

fn help_overlay(f: &mut Frame) {
    let rows = [
        ("j k ↑ ↓", "next / previous thread"),
        ("g G", "first / last thread"),
        ("Space b", "scroll reading pane down / up"),
        ("e", "archive"),
        ("z", "snooze (picker)"),
        ("f", "flag / unflag"),
        ("d", "delete (to Trash)"),
        ("r", "mark read / unread"),
        ("m", "move (picker, type to filter)"),
        ("!", "mark as spam"),
        ("o", "open in Fastmail web"),
        ("u", "undo last action"),
        ("t", "toggle newest / oldest first"),
        ("R", "refresh"),
        ("D", "toggle dry run (prototype only)"),
        ("[ ] F1-F3", "switch layout variant (prototype only)"),
        ("?", "this help"),
        ("q", "quit"),
    ];
    let area = centered(f.area(), 52, rows.len() as u16 + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, what)| Line::from(vec![format!(" {k:<10}").yellow().bold(), (*what).into()]))
        .collect();
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Keys ")), area);
}

/// Expands the queue's short date into a header date. Fake, like the rest.
fn full_date(short: &str) -> String {
    match short {
        "Mon" => "Mon 12 Oct 2026, 09:15".into(),
        "Sun" => "Sun 11 Oct 2026, 16:40".into(),
        "Sat" => "Sat 10 Oct 2026, 11:02".into(),
        "Fri" => "Fri 9 Oct 2026, 14:30".into(),
        "Thu" => "Thu 8 Oct 2026, 08:05".into(),
        time => format!("Wed 14 Oct 2026, {time}"),
    }
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

/// `--snapshot` prints every variant plus the pickers and help as plain text
/// at 120x40 and exits. Used to check rendering without a terminal.
fn snapshot() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut shot = |label: &str, app: &App| {
        terminal.draw(|f| ui(f, app)).unwrap();
        println!("==== {label}\n{}", terminal.backend());
    };
    for v in Variant::ALL {
        app.variant = v;
        shot(v.name(), &app);
    }
    app.variant = Variant::ThreePane;
    app.picker = Some(Picker::Snooze { sel: 1 });
    shot("snooze picker", &app);
    app.picker = Some(Picker::Move { sel: 0, filter: "pro".into() });
    shot("move picker", &app);
    app.picker = None;
    app.help = true;
    shot("help", &app);
    app.help = false;
    app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    shot("after archive, j, flag", &app);
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    shot("after two undos", &app);
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
