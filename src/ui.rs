//! Rendering `App` and mapping key events to messages.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Mode, Msg, PickerKey, SnoozeChoices};
use crate::mail::Thread;
use crate::prompt::Key;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const NUMBER_WIDTH: usize = 3;
const SENDER_WIDTH: usize = 20;
const COUNT_WIDTH: usize = 3;
const AGE_WIDTH: usize = 4;
/// Everything in a row but the subject: the number and the mark column, two
/// flag columns and a space, the sender and a space, and a space before each of
/// the count and the age.
const FIXED_WIDTH: usize =
    NUMBER_WIDTH + 1 + 3 + SENDER_WIDTH + 1 + 1 + COUNT_WIDTH + 1 + AGE_WIDTH;

/// Draws the whole screen: the queue, the statusline, the completion
/// candidates while Tab is cycling through them, and the bottom line.
pub fn view(app: &App, frame: &mut Frame) {
    let completion = app.completion();
    let [queue, statusline, candidates, bottom] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(completion.is_some())),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    rows(app, frame, queue);
    frame.render_widget(
        Paragraph::new(status_bar(app, statusline.width)),
        statusline,
    );
    if let Some((all, index)) = completion {
        frame.render_widget(
            Paragraph::new(candidate_line(all, index, candidates.width)),
            candidates,
        );
    }
    match (prompt_prefix(app.mode()), app.prompt(), app.prompt_cursor()) {
        (Some(prefix), Some(text), Some(cursor)) => {
            prompt_line(frame, bottom, prefix, text, cursor)
        }
        _ => frame.render_widget(Paragraph::new(bottom_line(app)), bottom),
    }
    if let Some(choices) = app.snooze_picker() {
        snooze_picker(frame, &choices);
    }
}

const PICKER_WIDTH: u16 = 42;

/// The snooze picker, centred over the queue: each preset's letter, label,
/// and time, with the one Enter chooses reversed.
fn snooze_picker(frame: &mut Frame, choices: &SnoozeChoices) {
    let lines: Vec<Line> = choices
        .presets
        .iter()
        .enumerate()
        .map(|(i, (preset, when))| {
            let line = Line::from(vec![
                Span::styled(
                    format!(" {}  ", preset.key()),
                    Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{:<18}", preset.label())),
                Span::styled(when.clone(), Style::new().fg(Color::DarkGray)),
            ]);
            if i == choices.index {
                line.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    let title = match choices.threads {
        1 => " Snooze ".to_string(),
        n => format!(" Snooze {n} threads "),
    };
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
    let [area] = Layout::horizontal([Constraint::Length(PICKER_WIDTH)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

/// The character before the prompt in `mode`, when it is a prompt.
fn prompt_prefix(mode: Mode) -> Option<char> {
    match mode {
        Mode::Filter => Some('/'),
        Mode::Command => Some(':'),
        Mode::Normal | Mode::Visual => None,
    }
}

/// The completion candidates, two spaces apart, with the chosen one
/// reversed, starting late enough that it fits in `width`.
fn candidate_line(all: &[String], index: usize, width: u16) -> Line<'static> {
    const GAP: &str = "  ";
    let width = usize::from(width);
    let mut first = 0;
    while first < index
        && all[first..=index].iter().map(|c| c.width()).sum::<usize>() + GAP.len() * (index - first)
            > width
    {
        first += 1;
    }
    let mut spans = Vec::new();
    for (i, candidate) in all.iter().enumerate().skip(first) {
        if i > first {
            spans.push(Span::raw(GAP));
        }
        let style = if i == index {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        spans.push(Span::styled(candidate.clone(), style));
    }
    Line::from(spans)
}

/// A prompt on the bottom line, `prefix` then `text`, with the terminal
/// cursor at `cursor` characters into `text`, scrolled so the cursor stays
/// on screen.
fn prompt_line(frame: &mut Frame, area: Rect, prefix: char, text: &str, cursor: usize) {
    let line = format!("{prefix}{text}");
    let column = |chars: usize| -> usize {
        line.chars()
            .take(chars)
            .map(|c| c.width().unwrap_or(0))
            .sum()
    };
    let at = column(cursor + 1);
    let width = usize::from(area.width).max(1);
    let mut skip = 0;
    while at - column(skip) >= width {
        skip += 1;
    }
    let shown: String = line.chars().skip(skip).collect();
    frame.render_widget(Paragraph::new(shown), area);
    let x = u16::try_from(at - column(skip)).unwrap_or(area.width);
    frame.set_cursor_position((area.x + x, area.y));
}

fn rows(app: &App, frame: &mut Frame, area: Rect) {
    let queue = app.queue();
    if queue.threads().is_empty() {
        if !app.loading() {
            let empty = Line::styled("No threads", Style::new().fg(Color::DarkGray));
            frame.render_widget(Paragraph::new(empty), area);
        }
        return;
    }
    let height = usize::from(area.height);
    let first = queue.cursor().saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = queue
        .threads()
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(index, thread)| row(app, index, thread, usize::from(area.width)))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn row(app: &App, index: usize, thread: &Thread, width: usize) -> Line<'static> {
    let cursor = app.queue().cursor();
    let at_cursor = index == cursor;
    let number = if at_cursor {
        Span::styled(
            format!("{:>NUMBER_WIDTH$}", index + 1),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{:>NUMBER_WIDTH$}", index.abs_diff(cursor)),
            Style::new().fg(Color::DarkGray),
        )
    };
    let selected = app.queue().selected(index);
    let unread = thread.unread();
    let text = if unread {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let dim = Style::new().fg(Color::DarkGray);
    let hit = thread.hit();
    let sender = hit.sender.as_deref().unwrap_or("(no sender)");
    let subject = hit.subject.as_deref().unwrap_or("(no subject)");
    let subject_width = width.saturating_sub(FIXED_WIDTH);
    let count = match thread.message_count() {
        1 => String::new(),
        n => n.to_string(),
    };
    let line = Line::from(vec![
        number,
        Span::styled(
            if selected { "▌" } else { " " },
            Style::new().fg(Color::Yellow),
        ),
        Span::styled(if unread { "●" } else { " " }, Style::new().fg(Color::Blue)),
        Span::styled(
            if thread.flagged() { "⚑" } else { " " },
            Style::new().fg(Color::Red),
        ),
        Span::raw(" "),
        Span::styled(format!("{} ", fit(sender, SENDER_WIDTH)), text),
        Span::styled(fit(subject, subject_width), text),
        Span::styled(format!(" {count:>COUNT_WIDTH$}"), dim),
        Span::styled(format!(" {:>AGE_WIDTH$}", when(app, thread)), dim),
    ]);
    if at_cursor {
        line.style(Style::new().add_modifier(Modifier::REVERSED))
    } else if selected {
        line.style(Style::new().fg(Color::Yellow))
    } else {
        line
    }
}

fn status_bar(app: &App, width: u16) -> Line<'static> {
    let (mode, colour) = match app.mode() {
        Mode::Filter => (" FILTER ", Color::Green),
        Mode::Command => (" COMMAND ", Color::Green),
        Mode::Visual => (" V-LINE ", Color::Yellow),
        Mode::Normal => (" NORMAL ", Color::Blue),
    };
    let mode = Span::styled(
        mode,
        Style::new()
            .fg(Color::Black)
            .bg(colour)
            .add_modifier(Modifier::BOLD),
    );
    let name = Span::styled(
        format!(" {} ", app.queue_name()),
        Style::new().fg(Color::White).bg(Color::DarkGray),
    );
    let filter = match app.filter() {
        "" => Span::raw(""),
        filter => Span::raw(format!(" {filter} ")),
    };
    let queue = app.queue();
    let len = queue.threads().len();
    let row = if len == 0 { 0 } else { queue.cursor() + 1 };
    let position = if app.total() > len {
        format!(" {row}/{len} of {} ", app.total())
    } else {
        format!(" {row}/{len} ")
    };
    let count = match app.pending_count() {
        Some(count) => format!("{count} "),
        None => String::new(),
    };
    let mut right = vec![Span::raw(count)];
    if app.undo_depth() > 0 {
        right.push(Span::styled(
            format!("u{} ", app.undo_depth()),
            Style::new().fg(Color::DarkGray),
        ));
    }
    if app.dry_run() {
        right.push(Span::styled(
            " DRY RUN ",
            Style::new()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        right.push(Span::raw(" "));
    }
    right.push(Span::styled(
        format!(" {} ", app.sort_name()),
        Style::new().fg(Color::DarkGray),
    ));
    right.push(Span::styled(
        position,
        Style::new().fg(Color::Black).bg(Color::Blue),
    ));
    let used =
        mode.width() + name.width() + filter.width() + right.iter().map(Span::width).sum::<usize>();
    let gap = " ".repeat(usize::from(width).saturating_sub(used));
    let mut spans = vec![mode, name, filter, Span::raw(gap)];
    spans.extend(right);
    Line::from(spans)
}

fn bottom_line(app: &App) -> Line<'static> {
    if app.loading() {
        let frame = SPINNER[app.spinner() % SPINNER.len()];
        return Line::raw(format!("{frame} Loading {}", app.queue_name()));
    }
    if app.queue().visual().is_some() {
        return Line::styled(
            "-- VISUAL LINE --",
            Style::new().add_modifier(Modifier::BOLD),
        );
    }
    Line::raw(app.status().to_string())
}

/// `text` cut or padded to exactly `width` columns, ending in `…` when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        let pad = width - text.width();
        return format!("{text}{}", " ".repeat(pad));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

/// The age column: how long ago the thread's matching email arrived, or in
/// the snooze mailbox how long until the thread returns, `now` when it is
/// due or has no schedule entry.
fn when(app: &App, thread: &Thread) -> String {
    if app.in_snooze_mailbox() {
        let returns = app.schedule().get(&thread.id).map(|e| e.return_at);
        return age(returns.unwrap_or(app.now), app.now);
    }
    age(app.now, thread.hit().received_at)
}

/// How long ago `then` was, in the largest whole unit: `now`, `5m`, `3h`,
/// `4d`, `2w`, `1y`.
fn age(now: Timestamp, then: Timestamp) -> String {
    let seconds = now.as_second() - then.as_second();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if minutes < 1 {
        "now".into()
    } else if hours < 1 {
        format!("{minutes}m")
    } else if days < 1 {
        format!("{hours}h")
    } else if days < 7 {
        format!("{days}d")
    } else if days < 365 {
        format!("{}w", days / 7)
    } else {
        format!("{}y", days / 365)
    }
}

/// The message a key press stands for in `app`'s state, if any.
pub fn key_msg(app: &App, key: KeyEvent) -> Option<Msg> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if prompt_prefix(app.mode()).is_some() {
        return prompt_key(key);
    }
    if app.snooze_picker().is_some() {
        return picker_key(key);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Msg::Quit),
            KeyCode::Char('r') => Some(Msg::Redo),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('j') => Some(Msg::Down),
        KeyCode::Char('k') => Some(Msg::Up),
        KeyCode::Char('g') => Some(Msg::Top),
        KeyCode::Char('G') => Some(Msg::Bottom),
        KeyCode::Char('q') => Some(Msg::Quit),
        KeyCode::Char('e') => Some(Msg::Archive),
        KeyCode::Char('z') => Some(Msg::OpenSnooze),
        KeyCode::Char('f') => Some(Msg::ToggleFlag),
        KeyCode::Char('r') => Some(Msg::ToggleRead),
        KeyCode::Char('u') => Some(Msg::Undo),
        KeyCode::Char('x') => Some(Msg::Mark),
        KeyCode::Char('X') => Some(Msg::ClearMarks),
        KeyCode::Char('V') => Some(Msg::Visual),
        KeyCode::Char('.') => Some(Msg::Repeat),
        KeyCode::Char('D') => Some(Msg::ToggleDryRun),
        KeyCode::Char('R') => Some(Msg::Refresh),
        KeyCode::Char('/') => Some(Msg::OpenFilter),
        KeyCode::Char(':') => Some(Msg::OpenCommand),
        KeyCode::Char('t') => Some(Msg::ToggleSort),
        KeyCode::Esc | KeyCode::Enter => Some(Msg::Cancel),
        KeyCode::Char(c) => c.to_digit(10).map(|d| Msg::Digit(d as u8)),
        _ => None,
    }
}

/// The message a key press at the snooze picker stands for: `j`, `k`, and
/// the arrows move, Enter chooses, Esc cancels, and any other character
/// may be a preset's letter, with Ctrl-c still quitting.
fn picker_key(key: KeyEvent) -> Option<Msg> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Msg::Quit);
    }
    let key = match key.code {
        KeyCode::Char('j') | KeyCode::Down => PickerKey::Down,
        KeyCode::Char('k') | KeyCode::Up => PickerKey::Up,
        KeyCode::Enter => PickerKey::Choose,
        KeyCode::Esc => PickerKey::Cancel,
        KeyCode::Char(c) => PickerKey::Char(c),
        _ => return None,
    };
    Some(Msg::Picker(key))
}

/// The message a key press at a prompt stands for: the editing keys, Enter,
/// and Esc, with Ctrl-c still quitting.
fn prompt_key(key: KeyEvent) -> Option<Msg> {
    let key = if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Some(Msg::Quit),
            KeyCode::Char('a') => Key::Home,
            KeyCode::Char('e') => Key::End,
            KeyCode::Char('w') => Key::DeleteWord,
            KeyCode::Char('u') => Key::Clear,
            _ => return None,
        }
    } else {
        match key.code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            _ => return None,
        }
    };
    Some(Msg::Prompt(key))
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::harness::Harness;
    use crate::jmap::fake::{EmailSpec, Store};

    fn render(h: &Harness, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view(&h.app, frame)).unwrap();
        terminal.backend().to_string()
    }

    fn empty_inbox() -> Store {
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build()
    }

    /// Ages against the harness's 2026-10-18T12:00:00Z.
    fn busy_inbox() -> Store {
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .role_mailbox("mb-archive", "Archive", "archive")
            .email(
                EmailSpec::new("e1", "t1", "2026-10-18T11:58:00Z")
                    .in_mailbox("mb-inbox")
                    .from("Ana Lopez", "ana@example.invalid")
                    .subject("Lunch on Friday?"),
            )
            .email(
                EmailSpec::new("e2a", "t2", "2026-10-14T08:00:00Z")
                    .in_mailbox("mb-archive")
                    .seen()
                    .from("Kim Park", "kim@example.invalid")
                    .subject("Trip to Wellington"),
            )
            .email(
                EmailSpec::new("e2b", "t2", "2026-10-18T07:00:00Z")
                    .in_mailbox("mb-inbox")
                    .seen()
                    .from("Ravi Shah", "ravi@example.invalid")
                    .subject("Re: Trip to Wellington"),
            )
            .email(
                EmailSpec::new("e2c", "t2", "2026-10-18T07:30:00Z")
                    .in_mailbox("mb-archive")
                    .seen()
                    .from("Kim Park", "kim@example.invalid")
                    .subject("Re: Trip to Wellington"),
            )
            .email(
                EmailSpec::new("e3", "t3", "2026-10-15T12:00:00Z")
                    .in_mailbox("mb-inbox")
                    .seen()
                    .flagged()
                    .from("", "billing@shop.example.invalid")
                    .subject(
                        "Your order 48213 has shipped and will arrive between Tuesday and Thursday next week",
                    ),
            )
            .email(
                EmailSpec::new("e4", "t4", "2026-09-20T12:00:00Z")
                    .in_mailbox("mb-inbox")
                    .flagged()
                    .from("Maximiliana Featherstonehaugh", "max@example.invalid"),
            )
            .email(
                EmailSpec::new("e5", "t5", "2025-06-01T12:00:00Z")
                    .in_mailbox("mb-inbox")
                    .seen()
                    .from("Old Friend", "old@example.invalid")
                    .subject("Catching up"),
            )
            .build()
    }

    #[test]
    fn loading() {
        let mut h = Harness::new(empty_inbox());
        h.start();

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn empty_queue_80x24() {
        let h = Harness::loaded(empty_inbox());

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn empty_queue_120x40() {
        let h = Harness::loaded(empty_inbox());

        insta::assert_snapshot!(render(&h, 120, 40));
    }

    #[test]
    fn loaded_queue_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("2j");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn loaded_queue_120x40() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("2j");

        insta::assert_snapshot!(render(&h, 120, 40));
    }

    #[test]
    fn marks_and_a_visual_range_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("xjVj");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn marks_and_a_visual_range_120x40() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("xjVj");

        insta::assert_snapshot!(render(&h, 120, 40));
    }

    #[test]
    fn pending_count() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("12");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn dry_run_badge() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("De");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn undo_depth() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("eee");

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" u3 "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn filter_prompt() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.keys("/is:unread");
        h.resolve(effects);

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" FILTER "), "{screen}");
        assert!(screen.contains("/is:unread"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn command_line_with_candidates() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys(":filter is:");
        h.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        h.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        let screen = render(&h, 80, 24);
        let lines: Vec<&str> = screen.lines().collect();
        assert!(lines[21].contains(" COMMAND "), "{screen}");
        assert!(
            lines[22].contains("is:unread  is:read  is:flagged  is:list"),
            "{screen}"
        );
        assert!(lines[23].contains(":filter is:read"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_candidate_line_scrolls_to_the_chosen_candidate() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys(":");
        h.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

        let screen = render(&h, 20, 6);
        let candidates = screen.lines().nth(4).unwrap();
        assert!(candidates.contains("quit  q"), "{screen}");
        assert!(!candidates.contains("archive"), "{screen}");
    }

    #[test]
    fn the_command_line_puts_the_terminal_cursor_after_the_typed_text() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys(":sort");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        terminal.draw(|frame| view(&h.app, frame)).unwrap();

        terminal.backend_mut().assert_cursor_position((5, 23));
    }

    #[test]
    fn the_prompt_puts_the_terminal_cursor_after_the_typed_text() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("/from:ana");
        h.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        terminal.draw(|frame| view(&h.app, frame)).unwrap();

        terminal.backend_mut().assert_cursor_position((8, 23));
    }

    #[test]
    fn the_statusline_shows_the_filter_and_the_sort_order() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.send(Msg::SetFilter("in:archive".into()));
        h.resolve(effects);
        let effects = h.keys("t");
        h.resolve(effects);

        let screen = render(&h, 80, 24);
        let statusline = screen.lines().nth(22).unwrap();
        assert!(statusline.contains(" Archive  in:archive "), "{statusline}");
        assert!(statusline.contains(" oldest "), "{statusline}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_snooze_mailbox_shows_when_each_thread_returns_soonest_first() {
        let store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .mailbox("mb-snooze", "Snoozed (blorp)")
            .email(
                EmailSpec::new("e1", "t1", "2026-10-01T09:00:00Z")
                    .in_mailbox("mb-snooze")
                    .seen()
                    .from("Insurer", "renewals@example.invalid")
                    .subject("Renewal"),
            )
            .email(
                EmailSpec::new("e2", "t2", "2026-10-02T09:00:00Z")
                    .in_mailbox("mb-snooze")
                    .seen()
                    .from("Airline", "trips@example.invalid")
                    .subject("Flights"),
            )
            .email(
                EmailSpec::new("e3", "t3", "2026-10-03T09:00:00Z")
                    .in_mailbox("mb-snooze")
                    .seen()
                    .from("Hotel", "stay@example.invalid")
                    .subject("Booking"),
            )
            .build();
        let entry = |email: &str, return_at: &str| crate::snooze::Entry {
            emails: [(email.into(), "mb-inbox".into())].into(),
            snoozed_at: "2026-10-10T09:00:00Z".parse().unwrap(),
            return_at: return_at.parse().unwrap(),
            returned_at: None,
        };
        let mut schedule = crate::snooze::Schedule::default();
        schedule.set("t1", Some(entry("e1", "2026-10-25T08:00:00Z")));
        schedule.set("t2", Some(entry("e2", "2026-10-18T15:00:00Z")));
        let mut h = Harness::with_schedule(store, schedule);
        let effects = h.send(Msg::SetFilter("in:\"Snoozed (blorp)\"".into()));
        h.resolve(effects);

        let screen = render(&h, 80, 24);
        let statusline = screen.lines().nth(22).unwrap();
        assert!(statusline.contains(" soonest "), "{statusline}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_mailbox_larger_than_the_queue_shows_its_total() {
        let mut store = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
        for i in 0..503 {
            let received = jiff::Timestamp::from_second(1_760_000_000 - i * 60).unwrap();
            store = store.email(
                EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received.to_string())
                    .in_mailbox("mb-inbox")
                    .seen()
                    .from("Sender", "sender@example.invalid")
                    .subject(&format!("Thread {}", i + 1)),
            );
        }
        let h = Harness::loaded(store.build());

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn snooze_picker_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("jz");

        let screen = render(&h, 80, 24);
        assert!(
            screen.contains("l  Later today       today 15:00"),
            "{screen}"
        );
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn snooze_picker_on_a_selection_120x40() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("Vjjzj");

        let screen = render(&h, 120, 40);
        assert!(screen.contains(" Snooze 3 threads "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_cursor_stays_on_screen_in_a_long_queue() {
        let mut store = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
        for i in 0..30 {
            let received = format!("2026-10-17T{:02}:{:02}:00Z", 23 - i / 60, 59 - i % 60);
            store = store.email(
                EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received)
                    .in_mailbox("mb-inbox")
                    .seen()
                    .from("Sender", "sender@example.invalid")
                    .subject(&format!("Thread {}", i + 1)),
            );
        }
        let mut h = Harness::loaded(store.build());
        h.keys("25j");

        insta::assert_snapshot!(render(&h, 80, 24));
    }
}
