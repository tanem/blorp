//! A reconcile brings the queue back in line with Fastmail: on a push
//! event and on the 60 s tick.

use blorp::app::{Effect, Msg};
use blorp::harness::{Harness, NOW};
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, Error, Fatal};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

/// Threads t1 to t5, one Inbox email each, newest first, and t6 in the
/// Archive, received between t2 and t3.
fn store() -> Store {
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive");
    for (n, subject) in ["One", "Two", "Three", "Four", "Five"].iter().enumerate() {
        builder = builder.email(
            EmailSpec::new(
                &format!("e{}", n + 1),
                &format!("t{}", n + 1),
                &format!("2026-10-18T{:02}:00:00Z", 10 - n),
            )
            .in_mailbox("mb-inbox")
            .subject(subject),
        );
    }
    builder
        .email(
            EmailSpec::new("e6", "t6", "2026-10-18T08:30:00Z")
                .in_mailbox("mb-archive")
                .subject("Six"),
        )
        .build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        limit: 500,
    }
}

/// Files `email` in mailbox `to` alone on the server.
fn server_move(h: &mut Harness, email: &str, to: &str) {
    h.store_mut().change_email(email, |email| {
        email.mailbox_ids = [(to.to_string(), true)].into();
    });
}

/// Archives `email` from the Inbox on the server.
fn server_archive(h: &mut Harness, email: &str) {
    server_move(h, email, "mb-archive");
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

/// The generation of the single reconcile in `effects`.
fn reconcile_generation(effects: &[Effect]) -> u64 {
    match effects {
        [Effect::Reconcile { generation, .. }] => *generation,
        other => panic!("expected one reconcile: {other:?}"),
    }
}

fn current(h: &Harness) -> String {
    h.subjects()[h.cursor()].clone()
}

#[test]
fn new_threads_enter_sorted_gone_ones_leave_and_changed_ones_update() {
    let mut h = Harness::loaded(store());
    server_move(&mut h, "e6", "mb-inbox");
    server_archive(&mut h, "e2");
    h.store_mut().change_email("e4", |email| {
        email.keywords.insert("$seen".into(), true);
    });

    reconcile(&mut h);

    assert_eq!(h.subjects(), ["One", "Six", "Three", "Four", "Five"]);
    assert_eq!(h.app.total(), 5);
    assert!(h.app.queue().threads()[3].hit().seen);
}

#[test]
fn the_cursor_stays_on_its_thread() {
    let mut h = Harness::loaded(store());
    h.keys("jj");
    server_move(&mut h, "e6", "mb-inbox");
    server_archive(&mut h, "e1");

    reconcile(&mut h);

    assert_eq!(current(&h), "Three");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn the_cursor_keeps_its_row_when_its_thread_leaves() {
    let mut h = Harness::loaded(store());
    h.keys("jj");
    server_archive(&mut h, "e3");

    reconcile(&mut h);

    assert_eq!(h.cursor(), 2);
    assert_eq!(current(&h), "Four");
}

#[test]
fn the_cursor_moves_to_the_last_row_when_its_row_is_gone() {
    let mut h = Harness::loaded(store());
    h.keys("jjjj");
    server_archive(&mut h, "e5");

    reconcile(&mut h);

    assert_eq!(current(&h), "Four");
}

#[test]
fn a_reconcile_that_loads_the_queue_clears_the_launch_query_s_error() {
    let mut h = Harness::new(store());
    h.start();
    h.send(Msg::Loaded {
        generation: 1,
        result: Err(Error::Status(400)),
    });
    assert_eq!(h.status(), "Fastmail error: HTTP 400");

    reconcile(&mut h);

    assert_eq!(h.status(), "");
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four", "Five"]);
}

#[test]
fn a_reconcile_replaces_the_mailbox_list() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.app.queue_name(), "Inbox");
    h.store_mut().rename_mailbox("mb-inbox", "In");

    reconcile(&mut h);

    assert_eq!(h.app.queue_name(), "In");
}

#[test]
fn a_push_event_during_a_reconcile_runs_another_when_it_lands() {
    let mut h = Harness::loaded(store());
    let first = h.send(Msg::Wake);

    assert_eq!(h.send(Msg::Wake), []);

    let rest = h.resolve(first);
    assert_eq!(
        rest,
        [Effect::Reconcile {
            generation: 3,
            query: inbox_query(),
            retry: true,
        }]
    );
}

#[test]
fn a_reconcile_waits_for_the_launch_query() {
    let mut h = Harness::new(store());
    let query = h.start();

    assert_eq!(h.send(Msg::Wake), []);

    let rest = h.resolve(query);
    assert!(matches!(rest[..], [Effect::Reconcile { .. }]), "{rest:?}");
}

#[test]
fn a_result_for_an_older_generation_is_dropped() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    let stale = h.store_mut().reconcile(&inbox_query());
    h.resolve(sent);

    assert_eq!(
        h.send(Msg::Reconciled {
            generation,
            result: stale
        }),
        []
    );
}

#[test]
fn the_tick_starts_at_launch_and_comes_round_every_minute() {
    let mut h = Harness::loaded(store());
    let now: Timestamp = NOW.parse().unwrap();
    assert_eq!(
        h.app.next_reconcile_at(),
        now + SignedDuration::from_secs(60)
    );
    h.app.now = now + SignedDuration::from_secs(61);

    let effects = h.send(Msg::ReconcileTick);

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert_eq!(
        h.app.next_reconcile_at(),
        now + SignedDuration::from_secs(121)
    );
    assert!(!h.app.loading(), "a tick shows no spinner");
}

#[test]
fn a_failed_reconcile_is_not_reported() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);

    h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.status(), "");
    assert_eq!(h.subjects().len(), 5);
    assert!(h.app.connected());
}

#[test]
fn a_rejected_token_on_a_reconcile_is_fatal() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);

    let effects = h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Status(401)),
    });

    assert_eq!(effects, [Effect::Fatal(Fatal::TokenRejected)]);
}

#[test]
fn a_refused_push_stream_is_fatal() {
    let mut h = Harness::loaded(store());

    let effects = h.send(Msg::StreamRefused(Fatal::NoEmailScope));

    assert_eq!(effects, [Effect::Fatal(Fatal::NoEmailScope)]);
}
