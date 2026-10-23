//! Rendering `App` and mapping key events to messages.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, ListKey, MailboxChoices, Msg};
use crate::jmap::retry::RETRIES;
use crate::keymap;
use crate::mail::Thread;
use crate::prompt::PromptKey;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const SENDER_WIDTH: usize = 20;
const COUNT_WIDTH: usize = 3;
const AGE_WIDTH: usize = 4;
/// Everything in a row but the subject: two flag columns and a space, the
/// sender and a space, and a space before each of the count and the age.
const FIXED_WIDTH: usize = 3 + SENDER_WIDTH + 1 + 1 + COUNT_WIDTH + 1 + AGE_WIDTH;

/// The width of the mailbox list's unread thread counts.
const UNREAD_WIDTH: usize = 6;

/// Draws the whole screen: the queue, or the mailbox list in its place
/// while it is open, the statusline, and the bottom line, with the terminal
/// cursor after the text while a prompt is open.
pub fn view(app: &App, frame: &mut Frame) {
    let [page, statusline, bottom] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let status = match app.mailbox_list() {
        Some(choices) => {
            mailbox_list(frame, &choices, page);
            list_status_bar(app, &choices, statusline.width)
        }
        None => {
            rows(app, frame, page);
            status_bar(app, statusline.width)
        }
    };
    frame.render_widget(Paragraph::new(status), statusline);
    frame.render_widget(Paragraph::new(bottom_line(app)), bottom);
    if let Some(typed) = app.prompt() {
        let x = u16::try_from(1 + typed.width()).unwrap_or(u16::MAX);
        frame.set_cursor_position((bottom.x + x.min(bottom.width.saturating_sub(1)), bottom.y));
    }
}

/// The mailbox list over `area`: each mailbox's path, and its unread
/// thread count on the right, blank when 0, scrolled to keep the
/// highlighted row in view and reversed.
fn mailbox_list(frame: &mut Frame, choices: &MailboxChoices, area: Rect) {
    if choices.rows.is_empty() {
        let text = match (choices.unread_only, choices.text.as_str()) {
            (true, "") => "No unread mail".to_string(),
            (false, "") => "No mailboxes".to_string(),
            (true, text) => format!("No unread mailboxes match /{text}"),
            (false, text) => format!("No mailboxes match /{text}"),
        };
        let empty = Line::styled(text, Style::new().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(empty), area);
        return;
    }
    let height = usize::from(area.height);
    let width = usize::from(area.width);
    let first = choices.highlight.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = choices
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(i, row)| {
            let unread = match row.unread {
                0 => String::new(),
                n => n.to_string(),
            };
            let path = fit(&row.path, width.saturating_sub(2 + UNREAD_WIDTH));
            let line = Line::from(vec![
                Span::raw(format!(" {path}")),
                Span::styled(
                    format!(" {unread:>UNREAD_WIDTH$}"),
                    Style::new().fg(Color::DarkGray),
                ),
            ]);
            if i == choices.highlight {
                line.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn rows(app: &App, frame: &mut Frame, area: Rect) {
    let queue = app.queue();
    let threads = queue.rows();
    if threads.is_empty() {
        if !app.loading() {
            let mut text = if app.unread_only() {
                "No unread threads".to_string()
            } else {
                "No threads".to_string()
            };
            if !app.search().is_empty() {
                text.push_str(&format!(" match /{}", app.search()));
            }
            let empty = Line::styled(text, Style::new().fg(Color::DarkGray));
            frame.render_widget(Paragraph::new(empty), area);
        }
        return;
    }
    let height = usize::from(area.height);
    let first = queue.cursor().saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = threads
        .into_iter()
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

/// The path of the mailbox shown, `Unread only` while it is on, and `/` and
/// the search while one is set on the left; the `DISCONNECTED` badge while
/// disconnected and the cursor's position on the right. The count is of
/// the rows, and the mailbox's total follows it when the mailbox holds
/// more threads than the queue and no search is set.
fn status_bar(app: &App, width: u16) -> Line<'static> {
    let queue = app.queue();
    let len = queue.rows().len();
    let row = if len == 0 { 0 } else { queue.cursor() + 1 };
    let position = if app.total() > len && app.search().is_empty() {
        format!(" {row}/{len} of {} ", app.total())
    } else {
        format!(" {row}/{len} ")
    };
    statusline(
        app,
        "Unread only",
        app.unread_only(),
        app.search(),
        position,
        width,
    )
}

/// The statusline while the mailbox list is open: the path of the mailbox
/// shown, `Unread mailboxes only` while it is on, and `/` and the text
/// while one is set, with the highlighted row's position among the
/// mailboxes listed.
fn list_status_bar(app: &App, choices: &MailboxChoices, width: u16) -> Line<'static> {
    let len = choices.rows.len();
    let row = if len == 0 { 0 } else { choices.highlight + 1 };
    statusline(
        app,
        "Unread mailboxes only",
        choices.unread_only,
        &choices.text,
        format!(" {row}/{len} "),
        width,
    )
}

/// The path of the mailbox shown, the `unread` badge while `unread_only`
/// is on, and `/` and `text` while it is not empty on the left; the
/// `DISCONNECTED` badge while disconnected and `position` on the right.
fn statusline(
    app: &App,
    unread: &str,
    unread_only: bool,
    text: &str,
    position: String,
    width: u16,
) -> Line<'static> {
    let mut left = vec![Span::styled(
        format!(" {} ", app.queue_path()),
        Style::new().fg(Color::White).bg(Color::DarkGray),
    )];
    if unread_only {
        left.push(Span::raw(" "));
        left.push(Span::styled(
            format!(" {unread} "),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    if !text.is_empty() {
        left.push(Span::raw(" "));
        left.push(Span::styled(
            format!(" /{text} "),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ));
    }
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
    let used = left.iter().chain(&right).map(Span::width).sum::<usize>();
    let gap = " ".repeat(usize::from(width).saturating_sub(used));
    let mut spans = left;
    spans.push(Span::raw(gap));
    spans.extend(right);
    Line::from(spans)
}

/// The prompt while one is open, and otherwise the retry, the loading
/// spinner, or the status message.
fn bottom_line(app: &App) -> Line<'static> {
    if let Some(typed) = app.prompt() {
        return Line::raw(format!("/{typed}"));
    }
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

/// The message a key press stands for: at a prompt, a key of the
/// prompt's; at the mailbox list, a key of the list's; and otherwise one in
/// the keymap, if any.
pub fn key_msg(app: &App, key: KeyEvent) -> Option<Msg> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if let Some(choices) = app.mailbox_list() {
        if choices.typing {
            return prompt_key(key, |key| Msg::ListKey(ListKey::Prompt(key)));
        }
        return list_key(key);
    }
    if app.prompt().is_some() {
        return prompt_key(key, Msg::SearchKey);
    }
    let key = match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => keymap::Key::Ctrl(c),
        KeyCode::Char(c) => keymap::Key::Char(c),
        KeyCode::Esc => keymap::Key::Esc,
        _ => return None,
    };
    keymap::msg(key)
}

/// The message a key press at a prompt stands for, through `msg`: Enter
/// locks the text in, Esc puts it back, and characters and Backspace edit
/// it, with Ctrl-c still quitting.
fn prompt_key(key: KeyEvent, msg: impl FnOnce(PromptKey) -> Msg) -> Option<Msg> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Msg::Quit);
    }
    let key = match key.code {
        KeyCode::Enter => PromptKey::Accept,
        KeyCode::Esc => PromptKey::Cancel,
        KeyCode::Backspace => PromptKey::Backspace,
        KeyCode::Char(c) => PromptKey::Char(c),
        _ => return None,
    };
    Some(msg(key))
}

/// The message a key press at the mailbox list stands for, with its prompt
/// closed: `j` and `k` move, `u` turns unread mailboxes only on or off, `/`
/// opens the prompt, Enter switches, Esc steps back, and `q` and Ctrl-c
/// quit.
fn list_key(key: KeyEvent) -> Option<Msg> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Msg::Quit);
    }
    let key = match key.code {
        KeyCode::Char('j') => ListKey::Down,
        KeyCode::Char('k') => ListKey::Up,
        KeyCode::Char('u') => ListKey::UnreadOnly,
        KeyCode::Char('/') => ListKey::Narrow,
        KeyCode::Char('q') => return Some(Msg::Quit),
        KeyCode::Enter => ListKey::Choose,
        KeyCode::Esc => ListKey::Back,
        _ => return None,
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
    fn a_thread_marked_read_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("r");

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn unread_only_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.keys("u");
        h.resolve(effects);

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" Inbox   Unread only "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn an_empty_unread_only_queue_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.keys("u");
        h.resolve(effects);
        h.keys("rr");

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No unread threads"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_search_prompt_while_typing_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("/TRIP");

        let screen = render(&h, 80, 24);
        assert!(
            screen.lines().nth(23).unwrap().starts_with("\"/TRIP "),
            "{screen}"
        );
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_search_locked_in_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("/wellington");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" Inbox   /wellington "), "{screen}");
        assert!(screen.contains(" 1/1 "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_search_under_unread_only_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.keys("u");
        h.resolve(effects);
        h.keys("/a");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains(" Inbox   Unread only   /a "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_search_that_matches_nothing_80x24() {
        let mut h = Harness::loaded(busy_inbox());
        h.keys("/acme");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No threads match /acme"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_search_under_unread_only_that_matches_nothing_says_so() {
        let mut h = Harness::loaded(busy_inbox());
        let effects = h.keys("u");
        h.resolve(effects);
        h.keys("/trip");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No unread threads match /trip"), "{screen}");
    }

    #[test]
    fn a_search_counts_only_the_matching_threads_of_a_mailbox_larger_than_the_queue() {
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
        let mut h = Harness::loaded(store.build());
        h.keys("/thread 1");
        h.press(KeyCode::Enter);

        let statusline = render(&h, 80, 24).lines().nth(22).unwrap().to_string();
        assert!(statusline.ends_with(" 1/111 \""), "{statusline}");
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

    /// The Inbox and thirteen other mailboxes, two of them nested. Unread
    /// threads: one in the Inbox, two in Newsletters, and one in Travel;
    /// Work/Reports holds a read thread.
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
            .email(EmailSpec::new("e3", "t3", "2026-10-17T08:00:00Z").in_mailbox("mb-Newsletters"))
            .email(EmailSpec::new("e4", "t4", "2026-10-17T07:00:00Z").in_mailbox("mb-Newsletters"))
            .email(EmailSpec::new("e5", "t5", "2026-10-16T07:00:00Z").in_mailbox("mb-Travel"))
            .build()
    }

    /// The statusline of an 80x24 screen.
    fn statusline(screen: &str) -> &str {
        screen.lines().nth(22).unwrap()
    }

    #[test]
    fn mailbox_list_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m");

        let screen = render(&h, 80, 24);
        assert!(!screen.contains("Lunch on Friday?"), "{screen}");
        assert!(statusline(&screen).starts_with("\" Inbox  "), "{screen}");
        assert!(statusline(&screen).ends_with(" 1/14 \""), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_mailbox_list_prompt_while_typing_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m/WOR");

        let screen = render(&h, 80, 24);
        assert!(
            screen.lines().nth(23).unwrap().starts_with("\"/WOR "),
            "{screen}"
        );
        assert!(statusline(&screen).contains(" Inbox   /WOR "), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn a_mailbox_list_narrowing_locked_in_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m/wor");
        h.press(KeyCode::Enter);
        h.keys("j");

        let screen = render(&h, 80, 24);
        assert!(statusline(&screen).contains(" Inbox   /wor "), "{screen}");
        assert!(statusline(&screen).ends_with(" 2/3 \""), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn unread_mailboxes_only_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mu");

        let screen = render(&h, 80, 24);
        assert!(
            statusline(&screen).contains(" Inbox   Unread mailboxes only "),
            "{screen}"
        );
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn the_statusline_shows_unread_mailboxes_only_before_the_text() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mu/e");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(
            statusline(&screen).contains(" Inbox   Unread mailboxes only   /e "),
            "{screen}"
        );
        assert!(statusline(&screen).ends_with(" 1/2 \""), "{screen}");
    }

    #[test]
    fn the_statusline_leaves_out_the_queue_s_settings_while_the_mailbox_list_is_open() {
        let mut h = Harness::loaded(many_mailboxes());
        let effects = h.keys("u");
        h.resolve(effects);
        h.keys("/lunch");
        h.press(KeyCode::Enter);

        h.keys("m");

        let screen = render(&h, 80, 24);
        assert!(!statusline(&screen).contains("Unread only"), "{screen}");
        assert!(!statusline(&screen).contains("/lunch"), "{screen}");
    }

    #[test]
    fn on_the_mailbox_list_only_j_k_u_slash_enter_esc_q_and_ctrl_c_send_a_message() {
        let mut h = Harness::loaded(empty_inbox());
        h.keys("m");
        let press = |code, modifiers| key_msg(&h.app, KeyEvent::new(code, modifiers));
        let list = |code| press(code, KeyModifiers::NONE);

        assert!(matches!(
            list(KeyCode::Char('j')),
            Some(Msg::ListKey(ListKey::Down))
        ));
        assert!(matches!(
            list(KeyCode::Char('k')),
            Some(Msg::ListKey(ListKey::Up))
        ));
        assert!(matches!(
            list(KeyCode::Char('u')),
            Some(Msg::ListKey(ListKey::UnreadOnly))
        ));
        assert!(matches!(
            list(KeyCode::Char('/')),
            Some(Msg::ListKey(ListKey::Narrow))
        ));
        assert!(matches!(
            list(KeyCode::Enter),
            Some(Msg::ListKey(ListKey::Choose))
        ));
        assert!(matches!(
            list(KeyCode::Esc),
            Some(Msg::ListKey(ListKey::Back))
        ));
        assert!(matches!(list(KeyCode::Char('q')), Some(Msg::Quit)));
        assert!(matches!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Msg::Quit)
        ));
        for c in "eromgGJKUd!x .0".chars() {
            assert!(list(KeyCode::Char(c)).is_none(), "{c}");
        }
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Tab, KeyCode::Backspace] {
            assert!(list(code).is_none(), "{code:?}");
        }
        for c in ['n', 'p', 'j', 'k', 'u'] {
            assert!(
                press(KeyCode::Char(c), KeyModifiers::CONTROL).is_none(),
                "Ctrl-{c}"
            );
        }
    }

    #[test]
    fn at_the_mailbox_list_prompt_characters_backspace_enter_esc_and_ctrl_c_send_a_message() {
        let mut h = Harness::loaded(empty_inbox());
        h.keys("m/");
        let press = |code, modifiers| key_msg(&h.app, KeyEvent::new(code, modifiers));
        let prompt = |key| Some(Msg::ListKey(ListKey::Prompt(key)));

        for c in "jkuqeorm/ A".chars() {
            assert_eq!(
                format!("{:?}", press(KeyCode::Char(c), KeyModifiers::NONE)),
                format!("{:?}", prompt(PromptKey::Char(c))),
            );
        }
        for (code, key) in [
            (KeyCode::Backspace, PromptKey::Backspace),
            (KeyCode::Enter, PromptKey::Accept),
            (KeyCode::Esc, PromptKey::Cancel),
        ] {
            assert_eq!(
                format!("{:?}", press(code, KeyModifiers::NONE)),
                format!("{:?}", prompt(key)),
            );
        }
        assert!(matches!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Msg::Quit)
        ));
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Tab] {
            assert!(press(code, KeyModifiers::NONE).is_none(), "{code:?}");
        }
        for c in ['n', 'p'] {
            assert!(
                press(KeyCode::Char(c), KeyModifiers::CONTROL).is_none(),
                "Ctrl-{c}"
            );
        }
    }

    #[test]
    fn no_unread_mail_80x24() {
        let mut h = Harness::loaded(empty_inbox());
        h.keys("mu");

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No unread mail"), "{screen}");
        assert!(statusline(&screen).ends_with(" 0/0 \""), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn no_mailboxes_match_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("m/zz");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(screen.contains("No mailboxes match /zz"), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn no_unread_mailboxes_match_says_so() {
        let mut h = Harness::loaded(many_mailboxes());
        h.keys("mu/work");
        h.press(KeyCode::Enter);

        let screen = render(&h, 80, 24);
        assert!(
            screen.contains("No unread mailboxes match /work"),
            "{screen}"
        );
    }

    #[test]
    fn a_long_mailbox_list_scrolled_80x24() {
        let mut store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .mailbox("mb-clients", "Clients");
        for i in 1..=30 {
            store =
                store.child_mailbox(&format!("mb-{i}"), &format!("Project {i:02}"), "mb-clients");
        }
        let store = store
            .child_mailbox(
                "mb-notes",
                "Notes from the planning meetings, kept for the next stage of work",
                "mb-30",
            )
            .email(EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z").in_mailbox("mb-25"))
            .email(EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z").in_mailbox("mb-notes"))
            .build();
        let mut h = Harness::loaded(store);
        h.keys("m");
        h.keys(&"j".repeat(40));

        let screen = render(&h, 80, 24);
        let page: Vec<&str> = screen.lines().take(22).collect();
        assert!(page[0].starts_with("\" Clients/Project 10 "), "{screen}");
        assert!(
            page[21].contains("Clients/Project 30/Notes from"),
            "{screen}"
        );
        assert!(page[21].contains("…"), "{screen}");
        assert!(statusline(&screen).ends_with(" 33/33 \""), "{screen}");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn switching_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        h.switch_to("reports");

        let screen = render(&h, 80, 24);
        assert!(screen.contains("Loading Work/Reports"), "{screen}");
        assert!(screen.contains(" Inbox "), "{screen}");
    }

    #[test]
    fn a_mailbox_switched_to_shows_its_path_80x24() {
        let mut h = Harness::loaded(many_mailboxes());
        let effects = h.switch_to("reports");
        h.resolve(effects);

        insta::assert_snapshot!(render(&h, 80, 24));
    }

    #[test]
    fn at_the_search_prompt_characters_backspace_enter_esc_and_ctrl_c_send_a_message() {
        let mut h = Harness::loaded(empty_inbox());
        h.keys("/");
        let press = |code, modifiers| key_msg(&h.app, KeyEvent::new(code, modifiers));

        for c in "jkeqrmu/ A".chars() {
            assert!(
                matches!(
                    press(KeyCode::Char(c), KeyModifiers::NONE),
                    Some(Msg::SearchKey(PromptKey::Char(k))) if k == c
                ),
                "{c}"
            );
        }
        assert!(matches!(
            press(KeyCode::Char('A'), KeyModifiers::SHIFT),
            Some(Msg::SearchKey(PromptKey::Char('A')))
        ));
        assert!(matches!(
            press(KeyCode::Backspace, KeyModifiers::NONE),
            Some(Msg::SearchKey(PromptKey::Backspace))
        ));
        assert!(matches!(
            press(KeyCode::Enter, KeyModifiers::NONE),
            Some(Msg::SearchKey(PromptKey::Accept))
        ));
        assert!(matches!(
            press(KeyCode::Esc, KeyModifiers::NONE),
            Some(Msg::SearchKey(PromptKey::Cancel))
        ));
        assert!(matches!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Msg::Quit)
        ));
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Tab] {
            assert!(press(code, KeyModifiers::NONE).is_none(), "{code:?}");
        }
        for c in ['n', 'p', 'u'] {
            assert!(
                press(KeyCode::Char(c), KeyModifiers::CONTROL).is_none(),
                "Ctrl-{c}"
            );
        }
    }

    #[test]
    fn only_j_k_e_r_o_m_u_slash_esc_q_and_ctrl_c_send_a_message() {
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
            press(KeyCode::Char('r'), KeyModifiers::NONE),
            Some(Msg::MarkRead)
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
            press(KeyCode::Char('u'), KeyModifiers::NONE),
            Some(Msg::UnreadOnly)
        ));
        assert!(matches!(
            press(KeyCode::Char('/'), KeyModifiers::NONE),
            Some(Msg::Search)
        ));
        assert!(matches!(
            press(KeyCode::Esc, KeyModifiers::NONE),
            Some(Msg::ClearSearch)
        ));
        assert!(matches!(
            press(KeyCode::Char('q'), KeyModifiers::NONE),
            Some(Msg::Quit)
        ));
        assert!(matches!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Msg::Quit)
        ));
        for c in "gGEd!Mzfx XUV.tR:D?0123456789Jc".chars() {
            assert!(press(KeyCode::Char(c), KeyModifiers::NONE).is_none(), "{c}");
        }
        for code in [
            KeyCode::Tab,
            KeyCode::BackTab,
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
