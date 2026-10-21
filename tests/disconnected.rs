//! blorp is disconnected once a request fails despite its retries, and
//! probes Fastmail on a backoff until a request succeeds.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::Error;
use blorp::jmap::fake::{EmailSpec, Store};
use jiff::SignedDuration;

/// Threads t1 to t3, one Inbox email each, newest first.
fn store() -> Store {
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive");
    for (n, subject) in ["One", "Two", "Three"].iter().enumerate() {
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
    builder.build()
}

fn secs(n: i64) -> SignedDuration {
    SignedDuration::from_secs(n)
}

fn unreachable() -> Error {
    Error::Transport("connection refused".into())
}

/// Seconds from now to the next reconcile or probe.
fn wait(h: &Harness) -> i64 {
    h.app
        .next_reconcile_at()
        .duration_since(h.app.now)
        .as_secs()
}

/// The generation of the single reconcile in `effects`.
fn reconcile_generation(effects: &[Effect]) -> u64 {
    match effects {
        [Effect::Reconcile { generation, .. }] => *generation,
        other => panic!("expected one reconcile: {other:?}"),
    }
}

/// Runs the reconcile tick at its time and fails the reconcile it sends.
fn failed_tick(h: &mut Harness) -> Vec<Effect> {
    h.app.now = h.app.next_reconcile_at();
    let sent = h.send(Msg::ReconcileTick);
    let generation = reconcile_generation(&sent);
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    })
}

/// A harness whose reconcile tick failed for good at launch plus 60 s.
fn disconnected() -> Harness {
    let mut h = Harness::loaded(store());
    failed_tick(&mut h);
    assert!(!h.app.connected());
    h
}

#[test]
fn a_reconcile_that_fails_for_good_disconnects() {
    let mut h = Harness::loaded(store());
    assert!(h.app.connected());

    let effects = failed_tick(&mut h);

    assert_eq!(effects, []);
    assert!(!h.app.connected());
    assert_eq!(h.status(), "Disconnected from Fastmail");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn a_bug_failure_does_not_disconnect() {
    let mut h = Harness::loaded(store());
    h.app.now = h.app.next_reconcile_at();
    let generation = reconcile_generation(&h.send(Msg::ReconcileTick));

    h.send(Msg::Reconciled {
        generation,
        result: Err(Error::Request("notRequest".into())),
    });

    assert!(h.app.connected());
}

#[test]
fn while_disconnected_the_probe_waits_5_s_doubling_to_60_s() {
    let mut h = disconnected();
    let mut waits = Vec::new();
    for _ in 0..6 {
        waits.push(wait(&h));
        failed_tick(&mut h);
    }

    assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
}

#[test]
fn a_probe_failing_changes_nothing_on_screen() {
    let mut h = disconnected();

    assert_eq!(failed_tick(&mut h), []);

    assert_eq!(h.status(), "Disconnected from Fastmail");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn a_push_event_probes_at_once_and_resets_the_backoff() {
    let mut h = disconnected();
    failed_tick(&mut h);
    failed_tick(&mut h);
    assert_eq!(wait(&h), 20);

    let generation = reconcile_generation(&h.send(Msg::Wake));
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(wait(&h), 5);
    assert_eq!(h.status(), "Disconnected from Fastmail");
}

#[test]
fn the_first_probe_that_succeeds_applies_as_a_reconcile() {
    let mut h = disconnected();
    h.store_mut().deliver(
        EmailSpec::new("e9", "t9", "2026-10-18T11:00:00Z")
            .in_mailbox("mb-inbox")
            .subject("Nine"),
    );
    h.app.now = h.app.next_reconcile_at();

    let effects = h.send(Msg::ReconcileTick);
    assert_eq!(h.resolve(effects), []);

    assert!(h.app.connected());
    assert_eq!(h.status(), "Reconnected");
    assert_eq!(h.subjects(), ["Nine", "One", "Two", "Three"]);
    assert_eq!(h.app.next_reconcile_at(), h.app.now + secs(60));
}

#[test]
fn a_probe_is_sent_once_and_a_reconcile_while_connected_is_retried() {
    let mut h = Harness::loaded(store());
    h.app.now = h.app.next_reconcile_at();
    match &h.send(Msg::ReconcileTick)[..] {
        [Effect::Reconcile { retry, .. }] => assert!(retry),
        other => panic!("{other:?}"),
    }

    let mut h = disconnected();
    h.app.now = h.app.next_reconcile_at();
    match &h.send(Msg::ReconcileTick)[..] {
        [Effect::Reconcile { retry, .. }] => assert!(!retry),
        other => panic!("{other:?}"),
    }
}

#[test]
fn retrying_shows_until_the_request_is_answered() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);

    h.send(Msg::Retrying {
        generation,
        retry: 2,
    });
    assert_eq!(h.app.retrying(), Some(2));
    assert!(h.app.spinning());

    h.resolve(sent);
    assert_eq!(h.app.retrying(), None);
    assert!(!h.app.spinning());
}

#[test]
fn a_retry_reported_after_its_answer_is_ignored() {
    let mut h = Harness::loaded(store());
    let sent = h.send(Msg::Wake);
    let generation = reconcile_generation(&sent);
    h.resolve(sent);

    h.send(Msg::Retrying {
        generation,
        retry: 1,
    });

    assert_eq!(h.app.retrying(), None);
}

#[test]
fn j_and_k_still_work() {
    let mut h = disconnected();

    h.keys("jjk");

    assert_eq!(h.cursor(), 1);
}

#[test]
fn quit_while_disconnected_is_immediate() {
    let mut h = disconnected();

    assert_eq!(h.keys("q"), [Effect::Quit]);
}

#[test]
fn a_launch_query_that_fails_for_good_leaves_the_queue_to_the_probe() {
    let mut h = Harness::new(store());
    h.start();

    let effects = h.send(Msg::Loaded {
        generation: 1,
        result: Err(unreachable()),
    });

    assert_eq!(effects, []);
    assert!(!h.app.connected());
    assert!(!h.app.loading());
    assert_eq!(h.status(), "Disconnected from Fastmail");
    assert_eq!(wait(&h), 5);

    h.app.now = h.app.next_reconcile_at();
    let effects = h.send(Msg::ReconcileTick);
    assert_eq!(h.resolve(effects), []);
    assert!(h.app.connected());
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn a_push_event_while_a_probe_is_in_flight_sends_no_second_probe_when_it_fails() {
    let mut h = disconnected();
    h.app.now = h.app.next_reconcile_at();
    let generation = reconcile_generation(&h.send(Msg::ReconcileTick));
    assert_eq!(h.send(Msg::Wake), [], "the push event waits for the probe");

    let effects = h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(effects, [], "no second probe straight after");
    assert_eq!(wait(&h), 5, "the push event reset the backoff");
}

#[test]
fn a_reconcile_waiting_while_disconnected_runs_as_the_next_probe() {
    let mut h = disconnected();
    h.app.now = h.app.next_reconcile_at();
    let generation = reconcile_generation(&h.send(Msg::ReconcileTick));
    h.send(Msg::Wake);
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });
    h.app.now = h.app.next_reconcile_at();

    let effects = h.send(Msg::ReconcileTick);
    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert_eq!(h.resolve(effects), [], "and nothing waits behind it");
}
