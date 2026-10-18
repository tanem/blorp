//! `u` undoes and `Ctrl-r` redoes, restoring each email an action touched.

use std::collections::BTreeSet;

use blorp::app::{Action, Effect, Msg, Step};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailUpdate, Error, Fatal};
use blorp::undo::{Replay, Replayed, State};
use serde_json::Value;

/// Newest first in the Inbox: t1 "Trip", t2 "Lunch", t3 "Invoice". Thread
/// t1 also has an email in Travel. Thread t4 is in Travel only.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e1b", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .flagged()
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-17T09:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Hotel"),
        )
        .build()
}

fn state(mailbox: &str, seen: bool, flagged: bool) -> State {
    State {
        mailbox_ids: BTreeSet::from([mailbox.to_string()]),
        seen,
        flagged,
    }
}

fn patch(entries: &[(&str, Value)]) -> EmailPatch {
    entries
        .iter()
        .map(|(path, value)| (path.to_string(), value.clone()))
        .collect()
}

/// Archives the thread under the cursor and answers the `Email/set`.
fn archive(h: &mut Harness) {
    let effects = h.keys("e");
    assert_eq!(h.resolve(effects), []);
}

#[test]
fn u_puts_an_archived_thread_back_with_the_cursor_on_it() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    archive(&mut h);
    h.keys("k");

    let effects = h.keys("u");

    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), "Undone: archive 1 thread");
    assert_eq!(
        effects,
        [Effect::Replay {
            job: 2,
            replay: Replay {
                expect: [("e2".to_string(), state("mb-archive", true, false))].into(),
                update: [(
                    "e2".to_string(),
                    patch(&[
                        ("mailboxIds/mb-archive", Value::Null),
                        ("mailboxIds/mb-inbox", Value::Bool(true)),
                    ])
                )]
                .into(),
            },
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.status(), "Undone: archive 1 thread");
    let effects = h.send(Msg::Show("mb-inbox".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
}

fn to_travel(id: &str) -> EmailUpdate {
    [(
        id.to_string(),
        patch(&[
            ("mailboxIds/mb-archive", Value::Null),
            ("mailboxIds/mb-travel", Value::Bool(true)),
        ]),
    )]
    .into()
}

#[test]
fn an_email_changed_elsewhere_since_the_action_is_skipped_and_the_entry_pops() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    archive(&mut h);
    h.store_mut().set_emails(&to_travel("e2"));

    let effects = h.keys("u");
    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
    let effects = h.resolve(effects);

    assert_eq!(
        h.status(),
        "Undone: archive 0 of 1 thread, 1 changed elsewhere"
    );
    assert_eq!(h.app.undo_depth(), 0);
    assert!(matches!(effects[..], [Effect::Query { .. }]), "{effects:?}");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    let effects = h.send(Msg::Show("mb-travel".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Hotel"]);
}

#[test]
fn a_count_undoes_that_many_actions_newest_first() {
    let mut h = Harness::loaded(store());
    archive(&mut h);
    archive(&mut h);
    archive(&mut h);
    assert_eq!(h.subjects(), Vec::<String>::new());
    assert_eq!(h.app.undo_depth(), 3);

    let effects = h.keys("2u");

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.status(), "Undone: archive 2 threads");
    assert_eq!(h.app.undo_depth(), 1);
    let jobs: Vec<u64> = effects
        .iter()
        .map(|e| match e {
            Effect::Replay { job, .. } => *job,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(jobs, [4, 5]);
    assert_eq!(h.resolve(effects), []);

    h.keys("5u");
    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Undone: archive 1 thread");
}

#[test]
fn with_nothing_to_undo_or_redo_u_and_ctrl_r_say_so() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("u"), []);
    assert_eq!(h.status(), "Nothing to undo");
    assert_eq!(h.ctrl('r'), []);
    assert_eq!(h.status(), "Nothing to redo");
}

#[test]
fn ctrl_r_redoes_an_undone_archive() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    archive(&mut h);
    let effects = h.keys("u");
    h.resolve(effects);

    let effects = h.ctrl('r');

    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(h.status(), "Redone: archive 1 thread");
    assert_eq!(h.app.undo_depth(), 1);
    assert_eq!(
        effects,
        [Effect::Replay {
            job: 3,
            replay: Replay {
                expect: [("e2".to_string(), state("mb-inbox", true, false))].into(),
                update: [(
                    "e2".to_string(),
                    patch(&[
                        ("mailboxIds/mb-archive", Value::Bool(true)),
                        ("mailboxIds/mb-inbox", Value::Null),
                    ])
                )]
                .into(),
            },
        }]
    );
    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Show("mb-archive".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch"]);
}

#[test]
fn a_new_action_after_an_undo_discards_the_redo() {
    let mut h = Harness::loaded(store());
    archive(&mut h);
    let effects = h.keys("u");
    h.resolve(effects);

    h.keys("j");
    archive(&mut h);

    assert_eq!(h.ctrl('r'), []);
    assert_eq!(h.status(), "Nothing to redo");
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn undo_from_another_queue_restores_the_thread_and_names_its_mailbox() {
    let mut h = Harness::loaded(store());
    let effects = h.send(Msg::Show("mb-travel".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Trip", "Hotel"]);
    h.keys("j");
    archive(&mut h);
    let effects = h.send(Msg::Show("mb-inbox".into()));
    h.resolve(effects);

    let effects = h.keys("u");

    assert_eq!(h.status(), "Undone: archive 1 thread (in Travel)");
    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
    h.resolve(effects);
    let effects = h.send(Msg::Show("mb-travel".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Trip", "Hotel"]);
}

#[test]
fn undo_takes_the_thread_out_of_a_queue_it_no_longer_belongs_in() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    archive(&mut h);
    let effects = h.send(Msg::Show("mb-archive".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch"]);

    h.keys("u");

    assert_eq!(h.subjects(), Vec::<String>::new());
    assert_eq!(h.app.total(), 0);
    assert_eq!(h.status(), "Undone: archive 1 thread (in Inbox)");
}

#[test]
fn undo_puts_each_email_back_where_it_was() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(EmailSpec::new("e1", "t1", "2026-10-18T08:00:00Z").in_mailbox("mb-inbox"))
        .email(
            EmailSpec::new("e2", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .in_mailbox("mb-archive"),
        )
        .build();
    let mut h = Harness::loaded(store);
    archive(&mut h);

    let effects = h.keys("u");
    let Effect::Replay { replay, .. } = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(
        replay.update,
        [
            (
                "e1".to_string(),
                patch(&[
                    ("mailboxIds/mb-archive", Value::Null),
                    ("mailboxIds/mb-inbox", Value::Bool(true)),
                ])
            ),
            (
                "e2".to_string(),
                patch(&[("mailboxIds/mb-inbox", Value::Bool(true))])
            ),
        ]
        .into()
    );
    h.resolve(effects);

    let effects = h.send(Msg::Show("mb-archive".into()));
    h.resolve(effects);
    assert_eq!(h.subjects().len(), 1, "e2 is still in Archive");
    let effects = h.send(Msg::Show("mb-inbox".into()));
    h.resolve(effects);
    assert_eq!(h.subjects().len(), 1);
}

#[test]
fn toggling_dry_run_clears_the_history_and_says_so() {
    let mut h = Harness::loaded(store());
    archive(&mut h);

    h.keys("D");
    assert_eq!(h.status(), "Dry run on, undo history cleared");
    assert_eq!(h.app.undo_depth(), 0);
    h.keys("u");
    assert_eq!(h.status(), "[dry] Nothing to undo");

    h.keys("e");
    h.keys("D");
    assert_eq!(h.status(), "Dry run off, undo history cleared");
    assert_eq!(h.keys("u"), []);
    assert_eq!(h.status(), "Nothing to undo");
}

#[test]
fn a_dry_run_undo_and_redo_are_logged_without_a_check() {
    let mut h = Harness::loaded_dry(store());
    h.keys("j");
    h.keys("e");
    h.store_mut().set_emails(&to_travel("e2"));

    let effects = h.keys("u");

    assert_eq!(
        effects,
        [Effect::LogDryRun {
            step: Step::Undo,
            action: Action::Archive,
            thread_ids: vec!["t2".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "[dry] Undone: archive 1 thread");

    let effects = h.ctrl('r');

    assert_eq!(
        effects,
        [Effect::LogDryRun {
            step: Step::Redo,
            action: Action::Archive,
            thread_ids: vec!["t2".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(h.status(), "[dry] Redone: archive 1 thread");
}

#[test]
fn a_failed_undo_is_rolled_back_and_dropped() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    archive(&mut h);
    h.keys("u");

    let effects = h.send(Msg::Replayed {
        job: 2,
        result: Err(Error::Status(503)),
    });

    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(
        h.status(),
        "Could not undo archive 1 thread: Fastmail returned HTTP 503"
    );
    assert!(matches!(effects[..], [Effect::Query { .. }]), "{effects:?}");
    assert_eq!(h.app.undo_depth(), 0);
    h.ctrl('r');
    assert_eq!(h.status(), "Nothing to redo");
}

#[test]
fn an_undo_with_emails_not_updated_is_rolled_back() {
    let mut h = Harness::loaded(store());
    archive(&mut h);
    h.keys("u");

    h.send(Msg::Replayed {
        job: 2,
        result: Ok(Replayed {
            skipped: Vec::new(),
            not_updated: vec!["e1b".into()],
        }),
    });

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(
        h.status(),
        "Could not undo archive 1 thread: 1 email not updated"
    );
}

#[test]
fn a_rejected_token_on_an_undo_is_fatal() {
    let mut h = Harness::loaded(store());
    archive(&mut h);
    h.keys("u");

    let effects = h.send(Msg::Replayed {
        job: 2,
        result: Err(Error::Status(401)),
    });

    assert_eq!(effects, [Effect::Fatal(Fatal::TokenRejected)]);
}

#[test]
fn a_failed_action_leaves_the_history() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    assert_eq!(h.app.undo_depth(), 1);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(503)),
    });

    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn the_history_is_shared_across_queues() {
    let mut h = Harness::loaded(store());
    archive(&mut h);
    let effects = h.send(Msg::Show("mb-travel".into()));
    h.resolve(effects);

    assert_eq!(h.app.undo_depth(), 1);
}
