//! `e` archives the thread under the cursor, and dry run.

use blorp::app::{Action, Effect, Msg, Step};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, SetResult};
use serde_json::{Value, json};

/// Thread t1 spreads over the Inbox, Travel, Sent, and Trash, with one
/// email in both the Inbox and Sent. Threads t2 and t3 are single Inbox
/// emails. Newest first: t1, t2, t3. Thread t4 is in Archive.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-trash", "Trash", "trash")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Trip"),
        )
        .email(EmailSpec::new("e1b", "t1", "2026-10-18T08:10:00Z").in_mailbox("mb-travel"))
        .email(
            EmailSpec::new("e1c", "t1", "2026-10-18T08:20:00Z")
                .in_mailbox("mb-inbox")
                .in_mailbox("mb-sent"),
        )
        .email(EmailSpec::new("e1d", "t1", "2026-10-18T08:30:00Z").in_mailbox("mb-trash"))
        .email(
            EmailSpec::new("e1e", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Re: Trip"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-01T09:00:00Z")
                .in_mailbox("mb-archive")
                .subject("Receipt"),
        )
        .build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        ascending: false,
        limit: 500,
        extra_threads: Vec::new(),
    }
}

fn to_archive() -> EmailPatch {
    [
        ("mailboxIds/mb-inbox".to_string(), Value::Null),
        ("mailboxIds/mb-archive".to_string(), Value::Bool(true)),
    ]
    .into()
}

fn archive(ids: &[&str]) -> EmailUpdate {
    ids.iter()
        .map(|id| (id.to_string(), to_archive()))
        .collect()
}

#[test]
fn e_moves_only_the_threads_queue_mailbox_messages_to_archive() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e1a", "e1e"]),
        }]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.status(), "Archived 1 thread");
}

#[test]
fn the_cursor_stays_on_its_row_and_the_next_thread_slides_in() {
    let mut h = Harness::loaded(store());

    h.keys("je");
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice"]);
    assert_eq!(h.cursor(), 1);

    h.keys("e");
    assert_eq!(h.subjects(), ["Re: Trip"]);
    assert_eq!(h.cursor(), 0, "the last row was archived");
}

#[test]
fn e_on_an_empty_queue_does_nothing() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
    );

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.status(), "");
}

#[test]
fn e_in_archive_does_nothing_and_says_so() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("e");
    h.resolve(effects);
    let effects = h.send(Msg::SetFilter("in:archive".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Re: Trip", "Receipt"]);

    let effects = h.keys("e");

    assert_eq!(effects, []);
    assert_eq!(h.subjects(), ["Re: Trip", "Receipt"]);
    assert_eq!(h.status(), "Already in Archive");
}

#[test]
fn without_an_archive_mailbox_e_does_nothing_and_says_so() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .email(EmailSpec::new("e1", "t1", "2026-10-18T08:00:00Z").in_mailbox("mb-inbox"))
        .build();
    let mut h = Harness::loaded(store);

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.subjects().len(), 1);
    assert_eq!(h.status(), "No Archive mailbox");
}

#[test]
fn a_confirmed_archive_changes_nothing_more() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("e");

    assert_eq!(h.resolve(effects), []);

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "Archived 1 thread");
}

#[test]
fn not_updated_rolls_the_archive_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    h.keys("e");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e2".into()],
        }),
    });

    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(
        h.status(),
        "Could not archive 1 thread: 1 email not updated"
    );
    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 2,
            query: inbox_query(),
            snoozed: None,
        }]
    );
}

#[test]
fn a_failed_request_rolls_the_archive_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(503)),
    });

    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(
        h.status(),
        "Could not archive 1 thread: Fastmail returned HTTP 503"
    );
    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 2,
            query: inbox_query(),
            snoozed: None,
        }]
    );
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn a_failed_archive_reconciles_once_the_outbox_drains() {
    let mut h = Harness::loaded(store());
    h.keys("ee");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(503)),
    });

    assert_eq!(effects, [], "the second archive is still in the outbox");
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice"]);

    let effects = h.send(Msg::Sent {
        job: 2,
        result: Err(Error::Status(503)),
    });

    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 2,
            query: inbox_query(),
            snoozed: None,
        }]
    );
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn a_rejected_token_on_an_archive_is_fatal() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(401)),
    });

    assert_eq!(effects, [Effect::Fatal(Fatal::TokenRejected)]);
}

#[test]
fn shift_d_toggles_dry_run() {
    let mut h = Harness::loaded(store());

    h.keys("D");
    assert!(h.app.dry_run());
    assert_eq!(h.status(), "Dry run on, undo history cleared");

    h.keys("D");
    assert!(!h.app.dry_run());
    assert_eq!(h.status(), "Dry run off, undo history cleared");
}

#[test]
fn a_dry_run_archive_changes_the_queue_and_logs_instead_of_sending() {
    let mut h = Harness::loaded(store());
    h.keys("D");

    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::LogDryRun {
            step: Step::Act,
            action: Action::Archive,
            thread_ids: vec!["t1".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "[dry] Archived 1 thread");
}

#[test]
fn dry_run_from_launch_marks_every_action_message() {
    let mut h = Harness::loaded_dry(store());
    assert!(h.app.dry_run());

    h.keys("e");
    assert_eq!(h.status(), "[dry] Archived 1 thread");

    let effects = h.send(Msg::SetFilter("in:archive".into()));
    h.resolve(effects);
    h.keys("e");
    assert_eq!(h.status(), "[dry] Already in Archive");
}

#[test]
fn turning_dry_run_off_sends_the_next_archive() {
    let mut h = Harness::loaded_dry(store());
    h.keys("D");

    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e1a", "e1e"]),
        }]
    );
    assert_eq!(h.status(), "Archived 1 thread");
}

#[test]
fn a_thread_whose_queue_mailbox_messages_are_all_in_sent_stays() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .in_mailbox("mb-sent"),
        )
        .build();
    let mut h = Harness::loaded(store);

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.subjects().len(), 1);
    assert_eq!(h.status(), "Nothing to archive");
}

#[test]
fn an_archive_takes_its_thread_off_the_total_and_a_rollback_puts_it_back() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.app.total(), 3);

    h.keys("e");
    assert_eq!(h.app.total(), 2);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(503)),
    });
    assert_eq!(h.app.total(), 3);
}
