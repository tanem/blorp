//! `?` and `:help` open an overlay listing every key and command.

use std::io;
use std::time::Duration;

use blorp::app::{Effect, Mode, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, FakeJmap, Store};
use blorp::run::{TestClock, launch, run};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// An Inbox of three single-email threads.
fn store() -> Store {
    let mut store = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
    for i in 1..=3 {
        store = store.email(
            EmailSpec::new(&format!("e{i}"), &format!("t{i}"), "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject(&format!("Thread {i}")),
        );
    }
    store.build()
}

fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    h.key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn question_mark_opens_help_at_the_top_in_normal_mode() {
    let mut h = Harness::loaded(store());

    h.keys("?");

    assert_eq!(h.app.help(), Some(0));
    assert_eq!(h.app.mode(), Mode::Normal);
}

#[test]
fn help_and_h_open_help() {
    for text in ["help", "h"] {
        let mut h = Harness::loaded(store());
        h.keys(":");
        h.keys(text);

        press(&mut h, KeyCode::Enter);

        assert_eq!(h.app.help(), Some(0), "{text:?}");
        assert_eq!(h.app.mode(), Mode::Normal, "{text:?}");
    }
}

#[test]
fn q_closes_help_without_quitting() {
    let mut h = Harness::loaded(store());
    h.keys("?");

    let effects = h.keys("q");

    assert_eq!(effects, []);
    assert_eq!(h.app.help(), None);
}

#[test]
fn any_other_key_closes_help_and_does_nothing_else() {
    for key in ["e", "x", "3", ":", "?", "G"] {
        let mut h = Harness::loaded(store());
        h.keys("?");

        let effects = h.keys(key);

        assert_eq!(effects, [], "{key:?}");
        assert_eq!(h.app.help(), None, "{key:?}");
        assert_eq!(h.app.mode(), Mode::Normal, "{key:?}");
        assert_eq!(h.pending_count(), None, "{key:?}");
        assert_eq!(
            h.subjects(),
            ["Thread 1", "Thread 2", "Thread 3"],
            "{key:?}"
        );
    }
    for code in [KeyCode::Esc, KeyCode::Enter, KeyCode::Tab, KeyCode::Down] {
        let mut h = Harness::loaded(store());
        h.keys("?");

        let effects = press(&mut h, code);

        assert_eq!(effects, [], "{code:?}");
        assert_eq!(h.app.help(), None, "{code:?}");
        assert_eq!(h.app.filter(), "", "{code:?}");
    }
}

#[test]
fn ctrl_c_quits_from_help() {
    let mut h = Harness::loaded(store());
    h.keys("?");

    let effects = h.ctrl('c');

    assert_eq!(effects, [Effect::Quit]);
}

/// The harness with help open on a screen 24 rows tall, where it shows 19
/// lines: all but the tabline, the statusline, the bottom line, and its
/// own top and bottom borders.
fn help_at_24_rows() -> Harness {
    let mut h = Harness::loaded(store());
    h.send(Msg::Resize(24));
    h.keys("?");
    h
}

#[test]
fn j_and_k_scroll_help_a_line() {
    let mut h = help_at_24_rows();

    h.keys("jjj");
    assert_eq!(h.app.help(), Some(3));
    h.keys("k");
    assert_eq!(h.app.help(), Some(2));
    h.keys("kkk");
    assert_eq!(h.app.help(), Some(0));
}

#[test]
fn ctrl_d_and_ctrl_u_scroll_help_half_a_page() {
    let mut h = help_at_24_rows();

    h.ctrl('d');
    assert_eq!(h.app.help(), Some(9));
    h.ctrl('d');
    assert_eq!(h.app.help(), Some(18));
    h.ctrl('u');
    assert_eq!(h.app.help(), Some(9));
    h.ctrl('u');
    h.ctrl('u');
    assert_eq!(h.app.help(), Some(0));
}

#[test]
fn help_stops_scrolling_at_its_last_page() {
    let mut h = help_at_24_rows();
    for _ in 0..20 {
        h.ctrl('d');
    }
    let last = h.app.help().unwrap();
    assert!(last > 0);

    h.keys("j");
    assert_eq!(h.app.help(), Some(last));
    h.keys("k");
    assert_eq!(h.app.help(), Some(last - 1));
}

#[test]
fn help_that_fits_on_the_screen_does_not_scroll() {
    let mut h = Harness::loaded(store());
    h.send(Msg::Resize(200));
    h.keys("?");

    h.keys("j");
    h.ctrl('d');

    assert_eq!(h.app.help(), Some(0));
}

#[test]
fn a_taller_screen_keeps_help_on_its_last_page() {
    let mut h = help_at_24_rows();
    for _ in 0..20 {
        h.ctrl('d');
    }
    let at_24 = h.app.help().unwrap();

    h.send(Msg::Resize(40));
    let at_40 = h.app.help().unwrap();
    h.keys("k");

    assert_eq!(at_40 + 16, at_24);
    assert_eq!(h.app.help(), Some(at_40 - 1));
}

const NOW: &str = "2026-10-18T12:00:00Z";

/// Sends each event one virtual second after the last.
fn events(events: Vec<Event>) -> impl Stream<Item = io::Result<Event>> + Unpin {
    Box::pin(stream::iter(events).then(|event| async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(event)
    }))
}

fn key(c: char, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), modifiers))
}

/// Runs blorp on an 80x24 screen through `before`, then `?` and Ctrl-d ten
/// times, and returns the rows of the help overlay's last line and its
/// bottom border.
async fn help_scrolled_down(before: Vec<Event>) -> (String, String) {
    let jmap = FakeJmap::new(store());
    let app = launch(&jmap, NOW.parse().unwrap()).await.unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut script = before;
    script.push(key('?', KeyModifiers::NONE));
    script.extend((0..10).map(|_| key('d', KeyModifiers::CONTROL)));

    run(
        &mut terminal,
        events(script),
        jmap.clone(),
        TestClock::new(NOW.parse().unwrap()),
        app,
    )
    .await
    .unwrap();

    let buffer = terminal.backend().buffer();
    let row = |y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    (row(20), row(21))
}

#[tokio::test(start_paused = true)]
async fn help_scrolls_to_its_last_line_on_the_terminal_it_runs_in() {
    let (last, border) = help_scrolled_down(Vec::new()).await;

    assert!(last.contains(":quit"), "{last}");
    assert!(border.contains(" of "), "{border}");
}

#[tokio::test(start_paused = true)]
async fn help_scrolls_by_the_height_of_a_resize() {
    let (last, _) = help_scrolled_down(vec![Event::Resize(80, 200)]).await;

    assert!(last.contains("Visual line"), "{last}");
}

/// Each entry help lists: its keys or usage, and its aliases.
fn entries() -> Vec<(String, String)> {
    blorp::help::lines()
        .into_iter()
        .filter_map(|line| match line {
            blorp::help::Line::Entry { name, aliases, .. } => Some((name, aliases)),
            _ => None,
        })
        .collect()
}

#[test]
fn help_lists_every_command_with_its_usage_and_aliases() {
    let commands: Vec<(String, String)> = [
        (":archive", ":e"),
        (":delete", ":d"),
        (":spam", ""),
        (":move <mailbox>", ":m"),
        (":snooze <time>", ":z"),
        (":unsnooze", ":uz"),
        (":flag", ":f"),
        (":read", ":r"),
        (":unread", ""),
        (":open", ":o"),
        (":undo [count]", ":u"),
        (":redo", ""),
        (":view [save|delete] <name>", ":v"),
        (":filter [query]", ""),
        (":sort [newest|oldest]", ""),
        (":refresh", ""),
        (":dry [off]", ""),
        (":help", ":h"),
        (":quit", ":q"),
    ]
    .into_iter()
    .map(|(usage, aliases)| (usage.to_string(), aliases.to_string()))
    .collect();

    let listed: Vec<_> = entries()
        .into_iter()
        .filter(|(name, _)| name.starts_with(':') && name.len() > 1)
        .collect();

    assert_eq!(listed, commands);
}

#[test]
fn help_lists_every_key_normal_mode_takes() {
    let names: Vec<String> = entries()
        .into_iter()
        .flat_map(|(name, _)| name.split(' ').map(ToString::to_string).collect::<Vec<_>>())
        .collect();
    let mut keys: Vec<(KeyEvent, String)> = (' '..='~')
        .map(|c| {
            (
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                c.to_string(),
            )
        })
        .collect();
    keys.extend(('a'..='z').map(|c| {
        (
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
            format!("Ctrl-{c}"),
        )
    }));
    keys.extend([
        (
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            "Tab".into(),
        ),
        (
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            "Shift-Tab".into(),
        ),
        (
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            "Esc".into(),
        ),
        (
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            "Enter".into(),
        ),
    ]);
    let h = Harness::loaded(store());

    for (event, name) in keys {
        if blorp::ui::key_msg(&h.app, event).is_none() {
            continue;
        }
        let listed = if name.chars().all(|c| c.is_ascii_digit()) {
            names.contains(&"0-9".to_string())
        } else {
            names.contains(&name)
        };
        assert!(listed, "{name} is not in help");
    }
    for name in [
        "j", "k", "g", "G", "0-9", "e", "z", "f", "d", "r", "m", "!", "o", "u", "Ctrl-r",
    ] {
        assert!(names.contains(&name.to_string()), "{name}");
    }
    for name in [
        "x",
        "X",
        "V",
        ".",
        "/",
        "Tab",
        "Shift-Tab",
        "t",
        "R",
        ":",
        "D",
        "?",
        "q",
    ] {
        assert!(names.contains(&name.to_string()), "{name}");
    }
}
