use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, Error};
use serde_json::json;

/// An Inbox of `n` single-email threads, "Thread 1" newest.
fn inbox(n: usize) -> Store {
    let mut store = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
    for i in 1..=n {
        let received = jiff::Timestamp::from_second(1_760_000_000 - i as i64 * 60).unwrap();
        store = store.email(
            EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received.to_string())
                .in_mailbox("mb-inbox")
                .subject(&format!("Thread {i}")),
        );
    }
    store.build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        limit: 500,
    }
}

#[test]
fn launch_queries_the_inbox() {
    let mut h = Harness::new(inbox(3));

    let effects = h.start();

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 1,
            query: inbox_query(),
            retry: true,
        }]
    );
}

#[test]
fn launch_without_an_inbox_says_so() {
    let mut h = Harness::new(Store::builder().mailbox("mb-travel", "Travel").build());

    assert_eq!(h.start(), []);
    assert_eq!(h.status(), "No Inbox mailbox");
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
fn j_and_k_stop_at_the_ends_of_the_queue() {
    let mut h = Harness::loaded(inbox(4));

    h.keys("k");
    assert_eq!(h.cursor(), 0);

    h.keys(&"j".repeat(20));
    assert_eq!(h.cursor(), 3);
}

#[test]
fn digits_are_not_counts() {
    let mut h = Harness::loaded(inbox(10));

    assert_eq!(h.keys("3j"), []);

    assert_eq!(h.cursor(), 1);
}

#[test]
fn keys_during_the_initial_load_act_on_an_empty_queue() {
    let mut h = Harness::new(inbox(3));
    h.start();

    h.keys("jjj");

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
        result: Err(Error::Status(400)),
    });

    assert!(h.subjects().is_empty());
    assert_eq!(h.status(), "Fastmail error: HTTP 400");
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
fn the_queue_holds_the_newest_500_threads_and_knows_the_total() {
    let h = Harness::loaded(inbox(503));

    assert_eq!(h.subjects().len(), 500);
    assert_eq!(h.subjects()[499], "Thread 500");
    assert_eq!(h.app.total(), 503);
}
