use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, Error};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

/// An Inbox of `n` single-email threads, "Thread 1" newest.
fn inbox(n: usize) -> Store {
    let mut store = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
    for i in 1..=n {
        let received = format!("2026-10-{:02}T09:00:00Z", 28 - i);
        store = store.email(
            EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received)
                .in_mailbox("mb-inbox")
                .subject(&format!("Thread {i}")),
        );
    }
    store.build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        ascending: false,
    }
}

#[test]
fn launch_queries_the_inbox_newest_first() {
    let mut h = Harness::new(inbox(3));

    let effects = h.start();

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 1,
            query: inbox_query(),
        }]
    );
}

#[test]
fn the_loaded_queue_is_one_row_per_thread_newest_first() {
    let h = Harness::loaded(inbox(3));

    assert_eq!(h.subjects(), ["Thread 1", "Thread 2", "Thread 3"]);
    assert_eq!(h.cursor(), 0);
}

#[test]
fn j_and_k_move_one_row() {
    let mut h = Harness::loaded(inbox(5));

    h.keys("jj");
    assert_eq!(h.cursor(), 2);

    h.keys("k");
    assert_eq!(h.cursor(), 1);
}

#[test]
fn a_count_prefix_moves_that_many_rows() {
    let mut h = Harness::loaded(inbox(10));

    h.keys("3j");
    assert_eq!(h.cursor(), 3);

    h.keys("2k");
    assert_eq!(h.cursor(), 1);
}

#[test]
fn motions_stop_at_the_ends_of_the_queue() {
    let mut h = Harness::loaded(inbox(4));

    h.keys("k");
    assert_eq!(h.cursor(), 0);

    h.keys("20j");
    assert_eq!(h.cursor(), 3);
}

#[test]
fn g_goes_to_the_first_row_and_shift_g_to_the_last() {
    let mut h = Harness::loaded(inbox(6));

    h.keys("G");
    assert_eq!(h.cursor(), 5);

    h.keys("g");
    assert_eq!(h.cursor(), 0);
}

#[test]
fn a_count_before_shift_g_goes_to_that_line() {
    let mut h = Harness::loaded(inbox(20));

    h.keys("12G");
    assert_eq!(h.cursor(), 11);

    h.keys("99G");
    assert_eq!(h.cursor(), 19);
}

#[test]
fn a_count_is_pending_until_a_motion_uses_it() {
    let mut h = Harness::loaded(inbox(20));

    h.keys("12");
    assert_eq!(h.pending_count(), Some(12));
    assert_eq!(h.cursor(), 0);

    h.keys("j");
    assert_eq!(h.pending_count(), None);
    assert_eq!(h.cursor(), 12);
}

#[test]
fn keys_during_the_initial_load_act_on_an_empty_queue() {
    let mut h = Harness::new(inbox(3));
    h.start();

    h.keys("3jG");

    assert_eq!(h.cursor(), 0);
    assert!(h.subjects().is_empty());
}

#[test]
fn q_quits() {
    let mut h = Harness::loaded(inbox(1));

    assert_eq!(h.keys("q"), [Effect::Quit]);
}

#[test]
fn ctrl_c_quits() {
    let mut h = Harness::loaded(inbox(1));

    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

#[test]
fn a_failed_load_leaves_the_queue_empty_and_reports_the_error() {
    let mut h = Harness::new(inbox(3));
    h.start();

    h.send(Msg::Loaded {
        generation: 1,
        result: Err(Error::Status(503)),
    });

    assert!(h.subjects().is_empty());
    assert_eq!(h.status(), "Fastmail returned HTTP 503");
}

#[test]
fn a_result_for_an_older_generation_is_dropped() {
    let mut h = Harness::loaded(inbox(3));

    let stale = inbox(1).query(&inbox_query()).unwrap();

    h.send(Msg::Loaded {
        generation: 0,
        result: Ok(stale),
    });

    assert_eq!(h.subjects(), ["Thread 1", "Thread 2", "Thread 3"]);
}

#[test]
fn a_pending_count_survives_spinner_ticks_and_the_load_landing() {
    let mut h = Harness::new(inbox(20));
    let effects = h.start();
    h.keys("12");

    h.send(Msg::Tick);
    h.resolve(effects);
    assert_eq!(h.pending_count(), Some(12));

    h.keys("j");
    assert_eq!(h.cursor(), 12);
}

#[test]
fn esc_cancels_a_pending_count() {
    let mut h = Harness::loaded(inbox(20));
    h.keys("12");

    h.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(h.pending_count(), None);

    h.keys("j");
    assert_eq!(h.cursor(), 1);
}
