//! Rendering `App` and mapping key events to messages.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, ListKey, MailboxChoices, Msg};
use crate::jmap::retry::RETRIES;
use crate::keymap;
use crate::mail::Thread;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const SENDER_WIDTH: usize = 20;
const COUNT_WIDTH: usize = 3;
const AGE_WIDTH: usize = 4;
/// Everything in a row but the subject: two flag columns and a space, the
/// sender and a space, and a space before each of the count and the age.
const FIXED_WIDTH: usize = 3 + SENDER_WIDTH + 1 + 1 + COUNT_WIDTH + 1 + AGE_WIDTH;

/// The width of the mailbox list, borders included.
const LIST_WIDTH: u16 = 42;

/// The most mailboxes the mailbox list shows at once.
const LIST_ROWS: usize = 10;

/// Draws the whole screen: the queue, the statusline, and the bottom line,
/// with the mailbox list over them while it is open.
pub fn view(app: &App, frame: &mut Frame) {
    let [queue, statusline, bottom] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    rows(app, frame, queue);
    frame.render_widget(
        Paragraph::new(status_bar(app, statusline.width)),
        statusline,
    );
    frame.render_widget(Paragraph::new(bottom_line(app)), bottom);
    if let Some(choices) = app.mailbox_list() {
        mailbox_list(frame, &choices);
    }
}

/// The mailbox list, centred over the queue: the filter line, then the
/// mailboxes it matches, scrolled to keep the highlighted one in view and
/// reversed.
fn mailbox_list(frame: &mut Frame, choices: &MailboxChoices) {
    let first = (choices.index + 1).saturating_sub(LIST_ROWS);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            " > ",
            Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ),
        Span::raw(choices.typed.clone()),
    ])];
    if choices.paths.is_empty() {
        lines.push(Line::styled(
            "   No mailbox matches",
            Style::new().fg(Color::DarkGray),
        ));
    }
    let width = usize::from(LIST_WIDTH) - 2;
    lines.extend(
        choices
            .paths
            .iter()
            .enumerate()
            .skip(first)
            .take(LIST_ROWS)
            .map(|(i, path)| {
                let line = Line::raw(fit(&format!("   {path}"), width));
                if i == choices.index {
                    line.style(Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    line
                }
            }),
    );
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
    let [area] = Layout::horizontal([Constraint::Length(LIST_WIDTH)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Mailbox ")),
        area,
    );
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
        Span::styled(if unread { "●" } else { " " }, Style::new().fg(Color::Blue)),
        Span::styled(
            if thread.flagged() { "⚑" } else { " " },
            Style::new().fg(Color::Red),
        ),
        Span::raw(" "),
        Span::styled(format!("{} ", fit(sender, SENDER_WIDTH)), text),
        Span::styled(fit(subject, subject_width), text),
        Span::styled(format!(" {count:>COUNT_WIDTH$}"), dim),
        Span::styled(
            format!(" {:>AGE_WIDTH$}", age(app.now, hit.received_at)),
            dim,
        ),
    ]);
    if index == app.queue().cursor() {
        line.style(Style::new().add_modifier(Modifier::REVERSED))
    } else {
        line
    }
}

/// The path of the mailbox shown on the left; the `DISCONNECTED` badge
/// while disconnected and the cursor's position on the right.
fn status_bar(app: &App, width: u16) -> Line<'static> {
    let name = Span::styled(
        format!(" {} ", app.queue_path()),
        Style::new().fg(Color::White).bg(Color::DarkGray),
    );
    let queue = app.queue();
    let len = queue.threads().len();
    let row = if len == 0 { 0 } else { queue.cursor() + 1 };
    let position = if app.total() > len {
        format!(" {row}/{len} of {} ", app.total())
    } else {
        format!(" {row}/{len} ")
    };
    let mut right = Vec::new();
    if !app.connected() {
        right.push(Span::styled(
            " DISCONNECTED ",
            Style::new()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
        right.push(Span::raw(" "));
    }
    right.push(Span::styled(
        position,
        Style::new().fg(Color::Black).bg(Color::Blue),
    ));
    let used = name.width() + right.iter().map(Span::width).sum::<usize>();
    let gap = " ".repeat(usize::from(width).saturating_sub(used));
    let mut spans = vec![name, Span::raw(gap)];
    spans.extend(right);
    Line::from(spans)
}

fn bottom_line(app: &App) -> Line<'static> {
    let frame = SPINNER[app.spinner() % SPINNER.len()];
    if let Some(retry) = app.retrying() {
        return Line::raw(format!("{frame} Retrying Fastmail ({retry}/{RETRIES})"));
    }
    if app.loading() {
        return Line::raw(format!("{frame} Loading {}", app.loading_path()));
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

/// The message a key press stands for: at the mailbox list, a key of the
/// list's, and otherwise one in the keymap, if any.
pub fn key_msg(app: &App, key: KeyEvent) -> Option<Msg> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if app.mailbox_list().is_some() {
        return list_key(key);
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let key = if key.modifiers.contains(KeyModifiers::CONTROL) {
        keymap::Key::Ctrl(c)
    } else {
        keymap::Key::Char(c)
    };
    keymap::msg(key)
}

/// The message a key press at the mailbox list stands for: Ctrl-n, Ctrl-p,
/// and the arrows move, Enter switches, Esc closes, and characters and
/// Backspace edit the text the mailboxes are filtered by, with Ctrl-c
/// still quitting.
fn list_key(key: KeyEvent) -> Option<Msg> {
    let key = if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Some(Msg::Quit),
            KeyCode::Char('n') => ListKey::Down,
            KeyCode::Char('p') => ListKey::Up,
            _ => return None,
        }
    } else {
        match key.code {
            KeyCode::Down => ListKey::Down,
            KeyCode::Up => ListKey::Up,
            KeyCode::Enter => ListKey::Choose,
            KeyCode::Esc => ListKey::Cancel,
            KeyCode::Backspace => ListKey::Backspace,
            KeyCode::Char(c) => ListKey::Char(c),
            _ => return None,
        }
    };
    Some(Msg::ListKey(key))
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::Effect;
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
        h.keys("jj");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn loaded_queue_120x40() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("jj");

        insta::assert_snapshot!(render(&h, 120, 40));
    }

    #[test]
    fn retrying_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        let generation = match h.send(Msg::Wake)[..] {
            [Effect::Reconcile { generation, .. }] => generation,
            ref other => panic!("{other:?}"),
        };
        h.send(Msg::Retrying {
            generation,
            retry: 2,
        });

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn disconnected_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        let generation = match h.send(Msg::Wake)[..] {
            [Effect::Reconcile { generation, .. }] => generation,
            ref other => panic!("{other:?}"),
        };
        h.send(Msg::Reconciled {
            generation,
            result: Err(crate::jmap::Error::Transport("connection refused".into())),
        });

        insta::assert_snapshot!(render(&h, 80, 24));
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
        h.keys(&"j".repeat(25));

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    /// The Inbox and fourteen other mailboxes, two of them nested, with a
    /// thread in Work/Receipts.
    fn many_mailboxes() -> Store {
        let mut store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .role_mailbox("mb-archive", "Archive", "archive")
            .role_mailbox("mb-sent", "Sent", "sent")
            .role_mailbox("mb-drafts", "Drafts", "drafts")
            .role_mailbox("mb-trash", "Trash", "trash")
            .role_mailbox("mb-junk", "Junk", "junk");
        for name in [
            "Family",
            "Clubs",
            "Newsletters",
            "Receipts",
            "Travel",
            "Work",
        ] {
            store = store.mailbox(&format!("mb-{name}"), name);
        }
        store
            .child_mailbox("mb-work-reports", "Reports", "mb-Work")
            .child_mailbox("mb-work-plans", "Plans", "mb-Work")
            .email(
                EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                    .in_mailbox("mb-inbox")
                    .from("Ana Lopez", "ana@example.invalid")
                    .subject("Lunch on Friday?"),
            )
            .email(
                EmailSpec::new("e2", "t2", "2026-10-17T09:00:00Z")
                    .in_mailbox("mb-work-reports")
                    .seen()
                    .from("Kim Park", "kim@example.invalid")
                    .subject("Quarterly report"),
            )
            .build()
    }

    #[test]
    fn mailbox_list_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m");

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" Mailbox "), "{screen}");
        assert!(!screen.contains("Work/Reports"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn mailbox_list_filtered_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mWOR");
        h.ctrl('n');

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" > WOR"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn mailbox_list_scrolled_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m");
        for _ in 0..13 {
            h.ctrl('n');
        }

        let screen = render(&h, 80, 24);
        assert!(screen.contains("Work/Reports"), "{screen}");
        assert!(!screen.contains("   Archive"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn mailbox_list_with_no_match_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mzz");

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No mailbox matches"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn switching_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mreports");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains("Loading Work/Reports"), "{screen}");
        assert!(screen.contains(" Inbox "), "{screen}");
    }

    #[test]
    fn a_mailbox_switched_to_shows_its_path_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mreports");
        let effects = h.press(KeyCode::Enter);
        h.resolve(effects);

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn only_j_k_e_o_m_q_and_ctrl_c_send_a_message() {
        let h = Harness::loaded(empty_inbox());
        let press = |code, modifiers| key_msg(&h.app, KeyEvent::new(code, modifiers));

        assert!(matches!(
            press(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(Msg::Down)
        ));
        assert!(matches!(
            press(KeyCode::Char('k'), KeyModifiers::NONE),
            Some(Msg::Up)
        ));
        assert!(matches!(
            press(KeyCode::Char('e'), KeyModifiers::NONE),
            Some(Msg::Archive)
        ));
        assert!(matches!(
            press(KeyCode::Char('o'), KeyModifiers::NONE),
            Some(Msg::Open)
        ));
        assert!(matches!(
            press(KeyCode::Char('m'), KeyModifiers::NONE),
            Some(Msg::OpenMailboxList)
        ));
        assert!(matches!(
            press(KeyCode::Char('q'), KeyModifiers::NONE),
            Some(Msg::Quit)
        ));
        assert!(matches!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Msg::Quit)
        ));
        for c in "gGEd!Mzfrux XV./tR:D?0123456789Jc".chars() {
            assert!(press(KeyCode::Char(c), KeyModifiers::NONE).is_none(), "{c}");
        }
        for code in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Up,
        ] {
            assert!(press(code, KeyModifiers::NONE).is_none(), "{code:?}");
        }
        for c in ['j', 'k', 'e', 'q', 'r', 'd', 'u'] {
            assert!(
                press(KeyCode::Char(c), KeyModifiers::CONTROL).is_none(),
                "Ctrl-{c}"
            );
        }
    }
}
