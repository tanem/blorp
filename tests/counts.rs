//! A count typed before `j` or `k` moves the cursor on the queue, or the
//! highlight on the mailbox list, that many rows.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::Store;
use crossterm::event::KeyCode;

/// An Inbox of `n` single-email threads, "Thread 1" newest, an Archive,
/// and `n` other mailboxes, "Folder 01" first.
fn store(n: usize) -> Store {
    let mut store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .threads(n, "mb-inbox", "2025-10-09T08:52:20Z", |email| email);
    for i in 1..=n {
        store = store.mailbox(&format!("mb-{i}"), &format!("Folder {i:02}"));
    }
    store.build()
}

#[test]
fn a_count_moves_the_cursor_that_many_rows() {
    let mut h = Harness::loaded(store(30));

    assert_eq!(h.keys("12j"), []);
    assert_eq!(h.cursor(), 12);

    h.keys("5k");
    assert_eq!(h.cursor(), 7);
}

/// The highlighted row of the open mailbox list.
fn highlight(h: &Harness) -> usize {
    h.app
        .mailbox_list()
        .expect("the mailbox list is open")
        .highlight
}

#[test]
fn a_count_moves_the_highlight_that_many_rows() {
    let mut h = Harness::loaded(store(30));
    h.keys("m");

    assert_eq!(h.keys("12j"), []);
    assert_eq!(highlight(&h), 12);

    h.keys("5k");
    assert_eq!(highlight(&h), 7);
    assert_eq!(h.cursor(), 0);
}

#[test]
fn a_count_larger_than_the_rows_left_stops_at_the_first_or_last_row() {
    let mut h = Harness::loaded(store(30));

    h.keys("5j");
    h.keys("99j");
    assert_eq!(h.cursor(), 29);
    h.keys("40k");
    assert_eq!(h.cursor(), 0);

    h.keys("m");
    h.keys("99j");
    assert_eq!(highlight(&h), 31);
    h.keys("40k");
    assert_eq!(highlight(&h), 0);
}

#[test]
fn esc_and_enter_drop_a_count_on_the_queue_and_do_nothing_else() {
    let mut h = Harness::loaded(store(30));
    h.keys("/thread 1");
    h.press(KeyCode::Enter);

    assert_eq!(h.keys("3"), []);
    assert_eq!(h.press(KeyCode::Esc), []);
    h.keys("j");
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.app.search(), "thread 1");

    h.keys("4");
    assert_eq!(h.press(KeyCode::Enter), []);
    h.keys("j");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn esc_and_enter_drop_a_count_on_the_mailbox_list_and_do_nothing_else() {
    let mut h = Harness::loaded(store(30));
    h.keys("m/folder");
    h.press(KeyCode::Enter);

    h.keys("3");
    assert_eq!(h.press(KeyCode::Esc), []);
    h.keys("j");
    assert_eq!(highlight(&h), 1);
    assert_eq!(h.app.mailbox_list().unwrap().text, "folder");

    h.keys("4");
    assert_eq!(h.press(KeyCode::Enter), []);
    h.keys("j");
    assert_eq!(highlight(&h), 2);
    assert_eq!(h.app.queue_path(), "Inbox");
}

#[test]
fn another_key_drops_a_count_on_the_queue_and_acts_once() {
    let mut h = Harness::loaded(store(30));

    h.keys("3");
    let effects = h.keys("e");
    assert!(
        matches!(effects[..], [Effect::SetEmails { .. }]),
        "{effects:?}"
    );
    h.resolve(effects);
    assert_eq!(h.subjects().len(), 29);
    assert_eq!(h.subjects()[0], "Thread 2");

    h.keys("j");
    assert_eq!(h.cursor(), 1);

    h.keys("4x");
    h.keys("j");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn another_key_drops_a_count_on_the_mailbox_list_and_acts_once() {
    let mut h = Harness::loaded(store(30));
    h.keys("m");

    h.keys("3u");
    assert!(h.app.mailbox_list().unwrap().unread_mailboxes_only);
    h.keys("u");

    h.keys("j");
    assert_eq!(highlight(&h), 1);

    h.keys("4e");
    h.keys("j");
    assert_eq!(highlight(&h), 2);
}

/// Runs a reconcile on a push event and answers it from the store.
fn reconcile(h: &mut Harness) {
    let effects = h.send(Msg::Wake);
    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert_eq!(h.resolve(effects), []);
}

#[test]
fn a_count_is_kept_through_a_reconcile_and_a_spinner_tick() {
    let mut h = Harness::loaded(store(30));

    h.keys("1");
    reconcile(&mut h);
    h.send(Msg::Tick);
    h.keys("2j");
    assert_eq!(h.cursor(), 12);

    h.keys("m3");
    reconcile(&mut h);
    h.send(Msg::Tick);
    h.keys("j");
    assert_eq!(highlight(&h), 3);
}

#[test]
fn a_count_is_kept_while_the_queue_loads() {
    let mut h = Harness::new(store(30));
    let effects = h.start();

    h.keys("12");
    h.send(Msg::Tick);
    h.resolve(effects);
    h.keys("j");

    assert_eq!(h.cursor(), 12);
}

#[test]
fn digits_go_into_the_search_prompt_and_start_no_count() {
    let mut h = Harness::loaded(store(30));

    h.keys("/thread 2");
    assert_eq!(h.app.prompt(), Some("thread 2"));
    h.press(KeyCode::Enter);
    h.keys("j");

    assert_eq!(
        h.subjects(),
        [
            "Thread 2",
            "Thread 20",
            "Thread 21",
            "Thread 22",
            "Thread 23",
            "Thread 24",
            "Thread 25",
            "Thread 26",
            "Thread 27",
            "Thread 28",
            "Thread 29"
        ]
    );
    assert_eq!(h.cursor(), 1);
}

#[test]
fn digits_go_into_the_mailbox_list_prompt_and_start_no_count() {
    let mut h = Harness::loaded(store(30));

    h.keys("m/folder 2");
    assert_eq!(h.app.prompt(), Some("folder 2"));
    h.press(KeyCode::Enter);
    h.keys("j");

    assert_eq!(h.app.mailbox_list().unwrap().rows.len(), 10);
    assert_eq!(highlight(&h), 1);
}

#[test]
fn zero_alone_does_nothing_and_extends_a_count_started() {
    let mut h = Harness::loaded(store(30));

    assert_eq!(h.keys("0"), []);
    h.keys("j");
    assert_eq!(h.cursor(), 1);

    h.keys("10j");
    assert_eq!(h.cursor(), 11);

    h.keys("m");
    h.keys("0j");
    assert_eq!(highlight(&h), 1);
    h.keys("20j");
    assert_eq!(highlight(&h), 21);
}
