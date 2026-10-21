//! blorp is disconnected once a request fails despite its retries, probes
//! Fastmail on a backoff until a request succeeds, and refuses to send
//! actions meanwhile.

use blorp::app::{Effect, Msg};
use blorp::harness::{Harness, NOW};
use blorp::jmap::Error;
use blorp::jmap::fake::{EmailSpec, Store};
use crossterm::event::{KeyCode, KeyEvent};
use jiff::{SignedDuration, Timestamp};

/// Threads t1 to t3, one Inbox email each, newest first.
fn store() -> Store {
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel");
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

fn now() -> Timestamp {
    NOW.parse().unwrap()
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
fn a_probe_failing_is_silent() {
    let mut h = disconnected();
    h.keys(":");
    h.key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(h.status(), "");

    failed_tick(&mut h);

    assert_eq!(h.status(), "");
}

#[test]
fn r_probes_at_once_and_resets_the_backoff() {
    let mut h = disconnected();
    failed_tick(&mut h);
    failed_tick(&mut h);
    assert_eq!(wait(&h), 20);

    let generation = reconcile_generation(&h.keys("R"));
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(wait(&h), 5);
    assert_eq!(h.status(), "Disconnected from Fastmail");
}

#[test]
fn a_push_event_probes_at_once_and_resets_the_backoff() {
    let mut h = disconnected();
    failed_tick(&mut h);

    let generation = reconcile_generation(&h.send(Msg::Wake));
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(wait(&h), 5);
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
fn a_probe_in_dry_run_reconnects_but_leaves_the_queue_alone() {
    let mut h = Harness::loaded_dry(store());
    h.app.now = now() + secs(1);
    let generation = reconcile_generation(&h.keys("R"));
    h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });
    assert!(!h.app.connected());
    h.keys("e");
    assert_eq!(h.subjects(), ["Two", "Three"]);
    h.app.now = h.app.next_reconcile_at();

    let effects = h.send(Msg::ReconcileTick);
    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    h.resolve(effects);

    assert!(h.app.connected());
    assert_eq!(h.subjects(), ["Two", "Three"]);
}

#[test]
fn any_request_that_succeeds_reconnects() {
    let mut h = disconnected();

    let effects = h.send(Msg::SetFilter("in:archive".into()));
    h.resolve(effects);

    assert!(h.app.connected());
    assert_eq!(h.status(), "Reconnected");
}

#[test]
fn retrying_shows_until_the_request_is_answered() {
    let mut h = Harness::loaded(store());
    let sent = h.keys("R");
    let generation = reconcile_generation(&sent);

    h.send(Msg::Retrying {
        from: blorp::app::Source::Query(generation),
        retry: 2,
    });
    assert_eq!(h.app.retrying(), Some(2));
    assert!(h.app.spinning());

    h.resolve(sent);
    assert_eq!(h.app.retrying(), None);
}

#[test]
fn every_action_that_would_send_is_refused() {
    for keys in ["e", "d", "!", "f", "r", "u", "."] {
        let mut h = Harness::loaded(store());
        let effects = h.keys("e");
        h.resolve(effects);
        failed_tick(&mut h);

        let effects = h.keys(keys);

        assert_eq!(effects, [], "{keys}");
        assert_eq!(h.status(), "Disconnected, action not sent", "{keys}");
        assert_eq!(h.subjects(), ["Two", "Three"], "{keys}");
        assert_eq!(h.app.undo_depth(), 1, "{keys}");
    }
}

#[test]
fn redo_is_refused() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("eu");
    h.settle(effects);
    failed_tick(&mut h);

    assert_eq!(h.ctrl('r'), []);
    assert_eq!(h.status(), "Disconnected, action not sent");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn commands_that_would_send_are_refused() {
    for command in [
        "archive",
        "move Travel",
        "flag",
        "read",
        "snooze tomorrow",
        "undo",
    ] {
        let mut h = disconnected();
        h.keys(":");
        h.keys(command);

        let effects = h.key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(effects, [], "{command}");
        assert_eq!(h.status(), "Disconnected, action not sent", "{command}");
        assert_eq!(h.subjects(), ["One", "Two", "Three"], "{command}");
    }
}

#[test]
fn a_refused_pick_closes_the_picker() {
    let mut h = disconnected();
    h.keys("m");

    assert_eq!(h.key(KeyEvent::from(KeyCode::Enter)), []);

    assert!(h.app.move_picker().is_none());
    assert_eq!(h.status(), "Disconnected, action not sent");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn navigation_marks_and_the_visual_range_still_work() {
    let mut h = disconnected();

    h.keys("jxjV");

    assert_eq!(h.cursor(), 2);
    assert!(h.app.queue().marked(1));
    assert!(h.app.queue().visual().is_some());
}

#[test]
fn dry_run_actions_go_ahead_while_disconnected() {
    let mut h = disconnected();
    h.keys("D");

    let effects = h.keys("e");

    assert!(
        matches!(effects[..], [Effect::LogDryRun { .. }]),
        "{effects:?}"
    );
    assert_eq!(h.subjects(), ["Two", "Three"]);
}

#[test]
fn a_filter_change_while_disconnected_tries_and_on_failure_leaves_the_queue_and_filter() {
    let mut h = disconnected();
    h.keys("x");

    let effects = h.send(Msg::SetFilter("in:archive".into()));
    let generation = match effects[..] {
        [Effect::Query { generation, .. }] => generation,
        _ => panic!("{effects:?}"),
    };
    h.send(Msg::Loaded {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(h.app.filter(), "");
    assert_eq!(h.app.queue_name(), "Inbox");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
    assert!(h.app.queue().marked(0), "the marks stay");
    assert_eq!(h.status(), "Disconnected from Fastmail");
}

#[test]
fn a_sort_change_that_fails_leaves_the_sort() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("t");
    let generation = match effects[..] {
        [Effect::Query { generation, .. }] => generation,
        _ => panic!("{effects:?}"),
    };
    h.send(Msg::Loaded {
        generation,
        result: Err(Error::Method {
            method: "Email/query".into(),
            kind: "unsupportedSort".into(),
        }),
    });

    assert_eq!(h.app.sort_name(), "newest");
    assert_eq!(h.status(), "Fastmail error: unsupportedSort");
    assert!(h.app.connected());
}

#[test]
fn an_action_that_fails_for_good_rolls_back_every_job_in_the_outbox_and_disconnects() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    h.keys("e");
    assert_eq!(h.subjects(), ["Three"]);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(unreachable()),
    });

    assert_eq!(effects, [Effect::DropJobs { through: 2 }]);
    assert!(!h.app.connected());
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
    assert_eq!(h.app.undo_depth(), 0);
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not archive 1 thread or the action after it"
    );
    assert_eq!(wait(&h), 5, "the probe reconciles");
}

#[test]
fn an_action_that_fails_as_a_bug_rolls_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Method {
            method: "Email/set".into(),
            kind: "invalidArguments".into(),
        }),
    });

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert!(h.app.connected());
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
    assert_eq!(
        h.status(),
        "Could not archive 1 thread: Fastmail error: invalidArguments"
    );
}

#[test]
fn quit_waits_for_the_outbox_to_drain() {
    let mut h = Harness::loaded(store());
    let sent = h.keys("e");

    assert_eq!(h.keys("q"), []);
    assert_eq!(h.status(), "Waiting for Fastmail to confirm 1 action");
    assert_eq!(h.app.quit_at(), Some(now() + secs(3)));

    let effects = h.resolve(sent);
    assert_eq!(effects.last(), Some(&Effect::Quit));
}

#[test]
fn quit_gives_up_after_3_s_with_the_actions_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("ee");
    h.keys("q");

    assert_eq!(h.send(Msg::QuitTimeout), [Effect::QuitUnconfirmed(2)]);
}

#[test]
fn an_action_that_fails_while_quit_waits_is_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    h.keys("q");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(unreachable()),
    });

    assert_eq!(effects.last(), Some(&Effect::QuitUnconfirmed(1)));
}

#[test]
fn quit_while_disconnected_is_immediate() {
    let mut h = disconnected();

    assert_eq!(h.keys("q"), [Effect::Quit]);
}

#[test]
fn a_bug_answer_to_an_action_does_not_reconnect() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    let generation = match h.send(Msg::SetFilter("in:archive".into()))[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    h.send(Msg::Loaded {
        generation,
        result: Err(unreachable()),
    });
    assert!(!h.app.connected(), "the outbox job is still out");

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert!(!h.app.connected());
}

#[test]
fn a_retry_reported_after_its_query_was_superseded_is_ignored() {
    let mut h = Harness::loaded(store());
    let old = match h.send(Msg::SetFilter("in:archive".into()))[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    let new = h.send(Msg::SetFilter("in:travel".into()));

    h.send(Msg::Retrying {
        from: blorp::app::Source::Query(old),
        retry: 1,
    });
    h.resolve(new);

    assert_eq!(h.app.retrying(), None);
}

#[test]
fn a_launch_query_that_fails_for_good_leaves_the_snooze_check_to_the_probe() {
    let mut store = store();
    store.add_mailbox("mb-snoozed", "Snoozed (blorp)");
    let mut h = Harness::new(store);
    h.start();

    let effects = h.send(Msg::Loaded {
        generation: 1,
        result: Err(unreachable()),
    });

    assert_eq!(effects, []);
    assert!(!h.app.connected());
    assert_eq!(wait(&h), 5);
}

#[test]
fn r_while_a_probe_is_in_flight_sends_no_second_probe_when_it_fails() {
    let mut h = disconnected();
    h.app.now = h.app.next_reconcile_at();
    let generation = reconcile_generation(&h.send(Msg::ReconcileTick));
    assert_eq!(h.keys("R"), [], "R waits for the probe in flight");

    let effects = h.send(Msg::Reconciled {
        generation,
        result: Err(unreachable()),
    });

    assert_eq!(effects, [], "no second probe straight after");
    assert_eq!(wait(&h), 5, "R reset the backoff");
}

#[test]
fn a_failure_of_a_superseded_query_is_ignored() {
    let mut h = Harness::loaded(store());
    let old = match h.send(Msg::SetFilter("in:archive".into()))[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    h.send(Msg::SetFilter("in:travel".into()));

    h.send(Msg::Loaded {
        generation: old,
        result: Err(unreachable()),
    });

    assert!(h.app.connected());
}

#[test]
fn a_reconcile_waiting_for_the_outbox_runs_once_an_action_reconnects() {
    let mut h = Harness::loaded(store());
    let archive = h.keys("e");
    assert_eq!(h.send(Msg::Wake), [], "the push event waits for the outbox");
    let generation = match h.send(Msg::SetFilter("in:archive".into()))[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    h.send(Msg::Loaded {
        generation,
        result: Err(unreachable()),
    });
    assert!(!h.app.connected());

    let effects = h.resolve(archive);

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert!(h.app.connected());
}

#[test]
fn a_reconcile_waiting_while_disconnected_runs_as_the_next_probe() {
    let mut h = disconnected();
    h.app.now = h.app.next_reconcile_at();
    let generation = reconcile_generation(&h.send(Msg::ReconcileTick));
    h.keys("R");
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
    assert!(h.app.loading(), "it runs as the R that waited");
    assert_eq!(h.resolve(effects), [], "and nothing waits behind it");
}

#[test]
fn a_reconcile_waiting_runs_once_the_launch_snooze_check_reconnects() {
    let mut store = store();
    store.add_mailbox("mb-snoozed", "Snoozed (blorp)");
    let mut h = Harness::new(store);
    let query = match &h.start()[..] {
        [Effect::Query { query, .. }] => query.clone(),
        other => panic!("{other:?}"),
    };
    let result = h.query(&query);
    let snoozed = match &h.send(Msg::Loaded {
        generation: 1,
        result,
    })[..]
    {
        [Effect::QuerySnoozed { query, .. }] => query.clone(),
        other => panic!("{other:?}"),
    };
    h.keys("e");
    assert_eq!(h.send(Msg::Wake), [], "the push event waits for the outbox");
    h.send(Msg::Sent {
        job: 1,
        result: Err(unreachable()),
    });
    assert!(!h.app.connected());

    let result = h.query(&snoozed);
    let effects = h.send(Msg::SnoozedLoaded { result });

    assert!(h.app.connected());
    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
}
