//! `o` and `:open` open each selected thread in Fastmail's web UI, up to
//! ten at once. Opening sends nothing to Fastmail and is not undoable.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::Error;
use blorp::jmap::fake::{EmailSpec, Store, StoreBuilder};
use crossterm::event::{KeyCode, KeyEvent};

/// `n` single-email Inbox threads, newest first: t1 holding e1 to tn
/// holding en.
fn builder(n: usize) -> StoreBuilder {
    let mut store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .mailbox("mb-work", "Work")
        .child_mailbox("mb-receipts", "Receipts", "mb-work");
    for i in 1..=n {
        let received = format!("2026-10-18T11:{:02}:00Z", 59 - i);
        store = store.email(
            EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received)
                .in_mailbox("mb-inbox")
                .subject(&format!("Thread {i}")),
        );
    }
    store
}

fn store(n: usize) -> Store {
    builder(n).build()
}

fn url(mailbox: &str, thread: &str, email: &str) -> Effect {
    Effect::Open {
        url: format!("https://app.fastmail.com/mail/{mailbox}/{thread}.{email}"),
    }
}

fn inbox(ids: &[usize]) -> Vec<Effect> {
    ids.iter()
        .map(|i| url("Inbox", &format!("t{i}"), &format!("e{i}")))
        .collect()
}

fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    h.key(KeyEvent::from(code))
}

/// Types `text` at the command line and runs it.
fn run(h: &mut Harness, text: &str) -> Vec<Effect> {
    h.keys(":");
    h.keys(text);
    press(h, KeyCode::Enter)
}

#[test]
fn o_opens_the_thread_under_the_cursor() {
    let mut h = Harness::loaded(store(3));
    h.keys("j");
    assert_eq!(h.keys("o"), inbox(&[2]));
    assert_eq!(h.status(), "Opened 1 thread");
}

#[test]
fn o_opens_a_thread_at_its_newest_email_wherever_that_is() {
    let mut h = Harness::loaded(
        builder(0)
            .email(
                EmailSpec::new("e-old", "t-long", "2026-10-18T09:00:00Z")
                    .in_mailbox("mb-inbox")
                    .subject("Long"),
            )
            .email(
                EmailSpec::new("e-reply", "t-long", "2026-10-18T11:00:00Z").in_mailbox("mb-sent"),
            )
            .email(EmailSpec::new("e-mid", "t-long", "2026-10-18T10:00:00Z").in_mailbox("mb-inbox"))
            .build(),
    );
    assert_eq!(h.keys("o"), vec![url("Inbox", "t-long", "e-reply")]);
}

#[test]
fn a_count_opens_one_tab_per_thread_in_queue_order() {
    let mut h = Harness::loaded(store(5));
    h.keys("j");
    assert_eq!(h.keys("3o"), inbox(&[2, 3, 4]));
    assert_eq!(h.status(), "Opened 3 threads");
    assert_eq!(h.pending_count(), None);
}

#[test]
fn o_opens_the_marked_rows_and_keeps_the_marks() {
    let mut h = Harness::loaded(store(5));
    h.keys("xjx");
    assert_eq!(h.keys("o"), inbox(&[1, 3]));
    assert!(h.app.queue().marked(0));
    assert!(h.app.queue().marked(2));
    assert_eq!(h.app.queue().threads().len(), 5);
}

#[test]
fn o_opens_the_visual_range_and_closes_it() {
    let mut h = Harness::loaded(store(5));
    h.keys("Vjj");
    assert_eq!(h.keys("o"), inbox(&[1, 2, 3]));
    assert_eq!(h.app.queue().visual(), None);
}

#[test]
fn o_refuses_more_than_ten_threads_and_opens_none() {
    let mut h = Harness::loaded(store(30));
    h.keys("V");
    h.keys(&"j".repeat(22));
    assert!(h.keys("o").is_empty());
    assert_eq!(h.status(), "Too many to open: 23 threads (limit 10)");
    assert_eq!(h.app.queue().visual(), Some(0..=22));

    let mut h = Harness::loaded(store(30));
    assert!(h.keys("11o").is_empty());
    assert_eq!(h.status(), "Too many to open: 11 threads (limit 10)");
    assert_eq!(h.keys("10o"), inbox(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
}

#[test]
fn the_url_names_the_queue_mailbox() {
    let mut h = Harness::loaded(
        builder(0)
            .email(
                EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                    .in_mailbox("mb-work")
                    .in_mailbox("mb-receipts"),
            )
            .build(),
    );
    let loaded = h.send(Msg::SetFilter("in:Work".into()));
    h.settle(loaded);
    assert_eq!(h.keys("o"), vec![url("Work", "t1", "e1")]);

    let loaded = h.send(Msg::SetFilter("in:Work/Receipts".into()));
    h.settle(loaded);
    assert_eq!(h.keys("o"), vec![url("Receipts", "t1", "e1")]);
}

#[test]
fn the_url_names_the_inbox_in_an_in_any_queue() {
    let mut h = Harness::loaded(
        builder(0)
            .email(EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z").in_mailbox("mb-work"))
            .build(),
    );
    let loaded = h.send(Msg::SetFilter("in:any".into()));
    h.settle(loaded);
    assert_eq!(h.keys("o"), vec![url("Inbox", "t1", "e1")]);
}

#[test]
fn o_is_not_undoable_and_dot_does_not_repeat_it() {
    let mut h = Harness::loaded(store(5));
    let archived = h.keys("e");
    h.settle(archived);
    assert_eq!(h.app.undo_depth(), 1);
    assert_eq!(h.keys("o"), inbox(&[2]));
    assert_eq!(h.app.undo_depth(), 1);
    let repeated = h.keys(".");
    assert!(matches!(repeated.as_slice(), [Effect::SetEmails { .. }]));
}

#[test]
fn o_works_in_dry_run() {
    let mut h = Harness::loaded_dry(store(3));
    assert_eq!(h.keys("o"), inbox(&[1]));
}

#[test]
fn o_works_while_disconnected() {
    let mut h = Harness::loaded(store(3));
    h.app.now = h.app.next_reconcile_at();
    let generation = match h.send(Msg::ReconcileTick).as_slice() {
        [Effect::Reconcile { generation, .. }] => *generation,
        other => panic!("expected one reconcile: {other:?}"),
    };
    h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Transport("connection refused".into())),
    });
    assert!(!h.app.connected());
    assert_eq!(h.keys("o"), inbox(&[1]));
    assert_eq!(h.status(), "Opened 1 thread");
}

#[test]
fn colon_open_and_colon_o_open_the_selection_with_the_count_before_the_colon() {
    for name in ["open", "o"] {
        let mut h = Harness::loaded(store(5));
        h.keys("2");
        assert_eq!(run(&mut h, name), inbox(&[1, 2]), ":{name}");
    }
}

#[test]
fn colon_open_refuses_more_than_ten_threads() {
    let mut h = Harness::loaded(store(30));
    h.keys("12");
    assert!(run(&mut h, "open").is_empty());
    assert_eq!(h.status(), "Too many to open: 12 threads (limit 10)");
}

#[test]
fn colon_open_with_nothing_selected_says_so() {
    let mut h = Harness::loaded(store(0));
    assert!(run(&mut h, "open").is_empty());
    assert_eq!(h.status(), "Nothing selected");
}

#[test]
fn colon_open_takes_no_arguments() {
    let mut h = Harness::loaded(store(3));
    assert!(run(&mut h, "open now").is_empty());
    assert_eq!(h.status(), "Usage: :open");
}

#[test]
fn a_failure_to_open_shows_in_the_status_line() {
    let mut h = Harness::loaded(store(3));
    h.send(Msg::OpenFailed {
        error: "no browser found".into(),
    });
    assert_eq!(h.status(), "Could not open the browser: no browser found");
}
