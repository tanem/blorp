//! A reconcile brings the queue back in line with Fastmail: on a push
//! event, on the 60 s tick, and on `R`.

use blorp::app::{Effect, Mode, Msg};
use blorp::harness::{Harness, NOW};
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, EmailUpdate, Error, Fatal};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

/// Threads t1 to t5, one Inbox email each, newest first, and t6 in the
/// Archive, received between t2 and t3. Travel is empty.
fn store() -> Store {
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel");
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
        ascending: false,
        limit: 500,
    }
}

/// Moves `email` from mailbox `from` to mailbox `to` on the server.
fn server_move(h: &mut Harness, email: &str, from: &str, to: &str) {
    let update = EmailUpdate::from([(
        email.to_string(),
        [
            (format!("mailboxIds/{from}"), Value::Null),
            (format!("mailboxIds/{to}"), Value::Bool(true)),
        ]
        .into(),
    )]);
    assert!(h.store_mut().set_emails(&update).not_updated.is_empty());
}

/// Archives `email` from the Inbox on the server.
fn server_archive(h: &mut Harness, email: &str) {
    server_move(h, email, "mb-inbox", "mb-archive");
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
    server_move(&mut h, "e6", "mb-archive", "mb-inbox");
    server_archive(&mut h, "e2");
    let seen = EmailUpdate::from([(
        "e4".to_string(),
        [("keywords/$seen".to_string(), Value::Bool(true))].into(),
    )]);
    h.store_mut().set_emails(&seen);

    reconcile(&mut h);

    assert_eq!(h.subjects(), ["One", "Six", "Three", "Four", "Five"]);
    assert_eq!(h.app.total(), 5);
    assert!(h.app.queue().threads()[3].hit().seen);
}

#[test]
fn the_cursor_stays_on_its_thread() {
    let mut h = Harness::loaded(store());
    h.keys("jj");
    server_move(&mut h, "e6", "mb-archive", "mb-inbox");
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
    h.keys("G");
    server_archive(&mut h, "e5");

    reconcile(&mut h);

    assert_eq!(current(&h), "Four");
}

#[test]
fn marks_on_threads_that_left_are_dropped_and_the_rest_kept() {
    let mut h = Harness::loaded(store());
    h.keys("jxjx");
    server_archive(&mut h, "e2");

    reconcile(&mut h);

    let marked: Vec<String> = (0..h.subjects().len())
        .filter(|&i| h.app.queue().marked(i))
        .map(|i| h.subjects()[i].clone())
        .collect();
    assert_eq!(marked, ["Four"]);
}

#[test]
fn a_visual_range_keeps_its_threads_still_in_the_queue() {
    let mut h = Harness::loaded(store());
    h.keys("jVjj");
    server_archive(&mut h, "e2");

    reconcile(&mut h);

    assert_eq!(h.app.mode(), Mode::Visual);
    assert_eq!(h.app.queue().visual(), Some(1..=2));
    assert_eq!(h.subjects()[1..=2], ["Three", "Four"]);
}

#[test]
fn a_visual_range_opened_below_the_cursor_keeps_its_threads_too() {
    let mut h = Harness::loaded(store());
    h.keys("GVkk");
    server_archive(&mut h, "e5");

    reconcile(&mut h);

    assert_eq!(h.app.queue().visual(), Some(2..=3));
    assert_eq!(h.subjects()[2..=3], ["Three", "Four"]);
}

#[test]
fn a_visual_range_closes_when_all_its_threads_leave() {
    let mut h = Harness::loaded(store());
    h.keys("jVj");
    server_archive(&mut h, "e2");
    server_archive(&mut h, "e3");

    reconcile(&mut h);

    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.cursor(), 2);
    assert_eq!(current(&h), "Five");
}

#[test]
fn a_reconcile_leaves_the_undo_history_alone() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("e");
    h.resolve(effects);
    assert_eq!(h.app.undo_depth(), 1);

    reconcile(&mut h);

    assert_eq!(h.app.undo_depth(), 1);
    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
}

#[test]
fn a_reconcile_replaces_the_mailbox_list_and_the_filter_keeps_its_mailbox() {
    let mut h = Harness::loaded(store());
    let effects = h.send(Msg::SetFilter("in:Travel".into()));
    h.resolve(effects);
    server_move(&mut h, "e1", "mb-inbox", "mb-travel");
    h.store_mut().rename_mailbox("mb-travel", "Trips");
    h.store_mut().add_mailbox("mb-receipts", "Receipts");

    reconcile(&mut h);

    assert_eq!(h.subjects(), ["One"], "the queue still shows mb-travel");
    assert_eq!(h.send(Msg::SetFilter("in:Trips".into())), []);
    assert_eq!(h.app.filter(), "in:Trips");
    h.send(Msg::SetFilter("in:Travel".into()));
    assert_eq!(h.status(), "No mailbox matches \"Travel\"");
    let effects = h.send(Msg::SetFilter("in:Receipts".into()));
    assert!(matches!(effects[..], [Effect::Query { .. }]), "{effects:?}");
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
        }]
    );
}

#[test]
fn a_reconcile_waits_for_a_query_in_flight() {
    let mut h = Harness::loaded(store());
    let query = h.send(Msg::SetFilter("in:Travel".into()));

    assert_eq!(h.send(Msg::Wake), []);

    let rest = h.resolve(query);
    assert!(matches!(rest[..], [Effect::Reconcile { .. }]), "{rest:?}");
}

#[test]
fn a_result_taken_before_an_action_landed_is_discarded_and_run_again() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    let before_archive = h.reconcile(&inbox_query());
    let archive = h.keys("e");
    assert_eq!(h.resolve(archive), [], "the reconcile is still in flight");

    let rerun = h.send(Msg::Reconciled {
        generation,
        result: before_archive,
    });

    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
    assert!(matches!(rerun[..], [Effect::Reconcile { .. }]), "{rerun:?}");
    assert_eq!(h.resolve(rerun), []);
    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
}

#[test]
fn a_result_that_lands_while_the_outbox_is_busy_runs_again_once_it_drains() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    let result = h.reconcile(&inbox_query());
    let archive = h.keys("e");

    assert_eq!(h.send(Msg::Reconciled { generation, result }), []);
    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);

    let rest = h.resolve(archive);
    assert!(matches!(rest[..], [Effect::Reconcile { .. }]), "{rest:?}");
}

#[test]
fn a_reconcile_waits_while_the_outbox_is_busy() {
    let mut h = Harness::loaded(store());
    let archive = h.keys("e");

    assert_eq!(h.send(Msg::Wake), []);

    let rest = h.resolve(archive);
    assert!(matches!(rest[..], [Effect::Reconcile { .. }]), "{rest:?}");
}

#[test]
fn a_filter_change_drops_a_reconcile_in_flight() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    let result = h.reconcile(&inbox_query());
    server_archive(&mut h, "e1");
    h.send(Msg::SetFilter("in:Travel".into()));

    assert_eq!(h.send(Msg::Reconciled { generation, result }), []);
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four", "Five"]);
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
fn r_reconciles_at_once_with_the_spinner() {
    let mut h = Harness::loaded(store());
    server_archive(&mut h, "e1");

    let effects = h.keys("R");

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert!(h.app.loading());
    h.resolve(effects);
    assert!(!h.app.loading());
    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
}

#[test]
fn the_refresh_command_reconciles_at_once() {
    let mut h = Harness::loaded(store());
    h.keys(":refresh");

    let effects = h.key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Enter,
    ));

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
}

#[test]
fn a_failed_refresh_is_reported_and_a_failed_tick_is_not() {
    let mut h = Harness::loaded(store());
    let sent = h.keys("R");
    let generation = reconcile_generation(&sent);
    h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Status(503)),
    });
    assert_eq!(h.status(), "Fastmail returned HTTP 503");
    assert!(!h.app.loading());
    assert_eq!(h.subjects().len(), 5);

    h.keys(":");
    h.key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Esc,
    ));
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Status(503)),
    });
    assert_eq!(h.status(), "");
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
fn dry_run_ignores_push_events_and_the_tick() {
    let mut h = Harness::loaded_dry(store());
    let now: Timestamp = NOW.parse().unwrap();
    h.app.now = now + SignedDuration::from_secs(60);

    assert_eq!(h.send(Msg::Wake), []);
    assert_eq!(h.send(Msg::ReconcileTick), []);
    assert_eq!(
        h.app.next_reconcile_at(),
        now + SignedDuration::from_secs(120)
    );
}

#[test]
fn r_in_dry_run_resets_the_queue_to_fastmail_and_clears_the_undo_history() {
    let mut h = Harness::loaded_dry(store());
    h.keys("e");
    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
    assert_eq!(h.app.undo_depth(), 1);

    let effects = h.keys("R");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four", "Five"]);
    assert_eq!(h.app.undo_depth(), 0);
    assert_eq!(
        h.status(),
        "[dry] Queue reset to Fastmail, undo history cleared"
    );
}

#[test]
fn a_reconcile_from_before_dry_run_turned_on_is_discarded() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    h.keys("De");

    assert_eq!(h.resolve(sent), []);

    assert_eq!(h.subjects(), ["Two", "Three", "Four", "Five"]);
    assert_eq!(h.app.undo_depth(), 1);
}
