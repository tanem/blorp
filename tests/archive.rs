//! `e` archives the thread under the cursor: the row leaves at once, the
//! archive goes through the outbox, and a failure puts the row back.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, Fatal, SetResult};
use serde_json::{Value, json};

/// Thread t1 spreads over the Inbox, Travel, Sent, and Trash, with one
/// email in both the Inbox and Sent. Threads t2 and t3 are single Inbox
/// emails. Newest first: t1, t2, t3.
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
        .build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        limit: 500,
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

fn reconcile(generation: u64) -> Effect {
    Effect::Reconcile {
        generation,
        query: inbox_query(),
        retry: true,
    }
}

/// Makes the next reconcile fail transiently, which disconnects blorp.
fn disconnect(h: &mut Harness) {
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
}

#[test]
fn e_moves_only_the_threads_inbox_emails_to_archive() {
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
    assert_eq!(h.status(), "Archived");
}

#[test]
fn the_archive_reaches_the_store_and_leaves_the_other_mailboxes_alone() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("e");

    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);

    // The email in both the Inbox and Sent keeps t1 in the Inbox.
    assert_eq!(h.subjects(), ["Lunch", "Invoice", ""]);
    let thread = &h.app.queue().rows()[2];
    let mailboxes = |id: &str| {
        let email = thread.emails.iter().find(|e| e.id == id).unwrap();
        email.mailbox_ids.iter().cloned().collect::<Vec<_>>()
    };
    assert_eq!(mailboxes("e1a"), ["mb-archive"]);
    assert_eq!(mailboxes("e1b"), ["mb-travel"]);
    assert_eq!(mailboxes("e1c"), ["mb-inbox", "mb-sent"]);
    assert_eq!(mailboxes("e1d"), ["mb-trash"]);
    assert_eq!(mailboxes("e1e"), ["mb-archive"]);
}

#[test]
fn the_cursor_stays_on_its_row_number_or_the_last_row() {
    let mut h = Harness::loaded(store());

    h.keys("je");
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice"]);
    assert_eq!(h.cursor(), 1);

    h.keys("e");
    assert_eq!(h.subjects(), ["Re: Trip"]);
    assert_eq!(h.cursor(), 0);
}

#[test]
fn e_on_an_empty_queue_does_nothing() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .role_mailbox("mb-archive", "Archive", "archive")
            .build(),
    );

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.status(), "");
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
fn a_read_only_account_sends_nothing_and_says_so() {
    let mut h = Harness::loaded(store());
    h.app.set_read_only(true);

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.subjects().len(), 3);
    assert_eq!(h.status(), "The API token is read-only");
}

#[test]
fn e_is_refused_while_disconnected() {
    let mut h = Harness::loaded(store());
    disconnect(&mut h);

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.subjects().len(), 3);
    assert_eq!(h.status(), "Disconnected, action not sent");
}

#[test]
fn a_thread_whose_inbox_emails_are_all_in_sent_stays() {
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
fn not_updated_puts_the_row_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("je");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e2".into()],
        }),
    });

    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), "Could not archive: 1 email not updated");
    assert_eq!(effects, [reconcile(2)]);
}

#[test]
fn a_failed_request_puts_the_row_back_and_reconciles_keeping_the_reason() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Could not archive: Fastmail error: HTTP 400");
    assert_eq!(effects, [reconcile(2)]);
    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Could not archive: Fastmail error: HTTP 400");
    assert!(h.app.connected());
}

#[test]
fn a_failed_archive_reconciles_once_the_outbox_drains() {
    let mut h = Harness::loaded(store());
    h.keys("ee");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(effects, [], "the second archive is still in the outbox");
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice"]);

    let effects = h.send(Msg::Sent {
        job: 2,
        result: Err(Error::Status(400)),
    });

    assert_eq!(effects, [reconcile(2)]);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn a_transient_failure_puts_back_every_archive_in_the_outbox_and_disconnects() {
    let mut h = Harness::loaded(store());
    h.keys("jee");
    assert_eq!(h.subjects(), ["Re: Trip"]);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(effects, [Effect::DropJobs { through: 2 }]);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert!(!h.app.connected());
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not archive 2 threads"
    );
    assert_eq!(h.app.total(), 3);

    h.app.now = h.app.next_reconcile_at();
    assert_eq!(
        h.send(Msg::ReconcileTick),
        [Effect::Reconcile {
            generation: 2,
            query: inbox_query(),
            retry: false,
        }],
        "the probe is the reconcile"
    );
}

#[test]
fn a_probe_held_back_by_an_archive_in_flight_is_tried_again_after_the_probe_wait() {
    let mut h = Harness::loaded(store());
    h.app.now = h.app.next_reconcile_at();
    assert_eq!(h.send(Msg::ReconcileTick), [reconcile(2)]);
    h.keys("e");
    h.send(Msg::Reconciled {
        generation: 2,
        result: Err(Error::Transport("connection refused".into())),
    });
    assert!(!h.app.connected());

    h.app.now = h.app.next_reconcile_at();
    assert_eq!(h.send(Msg::ReconcileTick), []);

    assert_eq!(
        h.app.next_reconcile_at(),
        h.app.now + jiff::SignedDuration::from_secs(10)
    );
}

#[test]
fn a_transient_failure_of_one_archive_says_so() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(h.status(), "Disconnected from Fastmail; could not archive");
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
fn an_archive_takes_its_thread_off_the_total_and_a_failure_puts_it_back() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.app.total(), 3);

    h.keys("e");
    assert_eq!(h.app.total(), 2);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });
    assert_eq!(h.app.total(), 3);
}

#[test]
fn no_reconcile_is_sent_while_an_archive_is_in_flight() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    assert_eq!(h.send(Msg::Wake), []);
    h.app.now = h.app.next_reconcile_at();
    assert_eq!(h.send(Msg::ReconcileTick), []);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult::default()),
    });

    assert_eq!(effects, [reconcile(2)]);
    assert_eq!(h.status(), "Archived");
}

#[test]
fn a_reconcile_result_from_before_an_archive_was_sent_is_discarded() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.send(Msg::Wake), [reconcile(2)]);
    let snapshot = Msg::Reconciled {
        generation: 2,
        result: store().reconcile(&inbox_query()),
    };
    let archived = h.keys("je");
    assert_eq!(h.resolve(archived), []);

    let effects = h.send(snapshot);

    assert_eq!(
        h.subjects(),
        ["Re: Trip", "Invoice"],
        "t2 does not come back"
    );
    assert_eq!(effects, [reconcile(3)], "the reconcile runs again");
    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice"]);
}

#[test]
fn q_with_nothing_in_flight_quits_at_once() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("q"), [Effect::Quit]);
}

#[test]
fn q_waits_for_an_archive_in_flight_and_quits_once_it_is_confirmed() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    assert_eq!(h.keys("q"), []);
    assert_eq!(h.status(), "Waiting for Fastmail to confirm 1 action");
    assert_eq!(
        h.app.quit_at(),
        Some(h.app.now + jiff::SignedDuration::from_secs(3))
    );

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult::default()),
    });

    assert_eq!(effects, [Effect::Quit]);
}

#[test]
fn q_quits_after_its_wait_with_the_archives_still_in_flight_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("ee");
    h.keys("q");
    assert_eq!(h.status(), "Waiting for Fastmail to confirm 2 actions");
    h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult::default()),
    });

    assert_eq!(h.send(Msg::QuitTimeout), [Effect::QuitUnconfirmed(1)]);
}

#[test]
fn an_archive_that_fails_while_q_waits_is_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    h.keys("q");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(effects, [Effect::QuitUnconfirmed(1)]);
}

#[test]
fn a_transient_failure_while_q_waits_leaves_every_archive_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("ee");
    h.keys("q");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(
        effects,
        [Effect::DropJobs { through: 2 }, Effect::QuitUnconfirmed(2)]
    );
}

/// One thread in each of Travel, Trash, Junk, Archive, Sent, and Drafts.
/// In Travel, t1 has an email in the Inbox and one also in Sent; in Trash,
/// t2 has an email also in Junk.
fn mailboxes() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-drafts", "Drafts", "drafts")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-junk", "Junk", "junk")
        .mailbox("mb-travel", "Travel")
        .email(EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z").in_mailbox("mb-travel"))
        .email(EmailSpec::new("e1b", "t1", "2026-10-18T08:10:00Z").in_mailbox("mb-inbox"))
        .email(
            EmailSpec::new("e1c", "t1", "2026-10-18T08:20:00Z")
                .in_mailbox("mb-travel")
                .in_mailbox("mb-sent"),
        )
        .email(EmailSpec::new("e2a", "t2", "2026-10-18T07:00:00Z").in_mailbox("mb-trash"))
        .email(
            EmailSpec::new("e2b", "t2", "2026-10-18T07:10:00Z")
                .in_mailbox("mb-trash")
                .in_mailbox("mb-junk"),
        )
        .email(EmailSpec::new("e3", "t3", "2026-10-18T06:00:00Z").in_mailbox("mb-junk"))
        .email(EmailSpec::new("e4", "t4", "2026-10-18T05:00:00Z").in_mailbox("mb-archive"))
        .email(EmailSpec::new("e5", "t5", "2026-10-18T04:00:00Z").in_mailbox("mb-sent"))
        .email(EmailSpec::new("e6", "t6", "2026-10-18T03:00:00Z").in_mailbox("mb-drafts"))
        .build()
}

/// Switches to the only mailbox whose path holds `typed`.
fn switch(h: &mut Harness, typed: &str) {
    let effects = h.switch_to(typed);
    assert_eq!(h.resolve(effects), []);
}

/// The update that archives `ids` from mailbox `from`.
fn archive_from(from: &str, ids: &[&str]) -> EmailUpdate {
    let patch: EmailPatch = [
        (format!("mailboxIds/{from}"), Value::Null),
        ("mailboxIds/mb-archive".to_string(), Value::Bool(true)),
    ]
    .into();
    ids.iter()
        .map(|id| (id.to_string(), patch.clone()))
        .collect()
}

#[test]
fn e_in_another_mailbox_moves_the_threads_emails_there_to_archive() {
    let mut h = Harness::loaded(mailboxes());
    switch(&mut h, "travel");

    assert_eq!(
        h.keys("e"),
        [Effect::SetEmails {
            job: 1,
            update: archive_from("mb-travel", &["e1a"]),
        }]
    );
    assert_eq!(h.app.queue().rows().len(), 0);
    assert_eq!(h.status(), "Archived");
}

#[test]
fn e_in_trash_moves_the_emails_there_to_archive() {
    let mut h = Harness::loaded(mailboxes());
    switch(&mut h, "trash");

    assert_eq!(
        h.keys("e"),
        [Effect::SetEmails {
            job: 1,
            update: archive_from("mb-trash", &["e2a"]),
        }]
    );
}

#[test]
fn e_in_junk_moves_the_emails_there_to_archive() {
    let mut h = Harness::loaded(mailboxes());
    switch(&mut h, "junk");
    // t2's email in Junk is also in Trash, which keeps it.
    assert_eq!(h.keys("e"), []);
    assert_eq!(h.status(), "Nothing to archive");

    assert_eq!(
        h.keys("je"),
        [Effect::SetEmails {
            job: 1,
            update: archive_from("mb-junk", &["e3"]),
        }]
    );
}

#[test]
fn e_in_archive_does_nothing_and_says_so() {
    let mut h = Harness::loaded(mailboxes());
    switch(&mut h, "archive");

    assert_eq!(h.keys("e"), []);
    assert_eq!(h.app.queue().rows().len(), 1);
    assert_eq!(h.status(), "Already in Archive");
}

#[test]
fn e_in_sent_or_drafts_does_nothing_and_says_so() {
    for (typed, status) in [
        ("sent", "Cannot archive from Sent"),
        ("drafts", "Cannot archive from Drafts"),
    ] {
        let mut h = Harness::loaded(mailboxes());
        switch(&mut h, typed);

        assert_eq!(h.keys("e"), [], "{typed}");
        assert_eq!(h.status(), status);
    }
}

#[test]
fn a_failed_archive_from_a_mailbox_no_longer_shown_puts_no_row_back() {
    let mut h = Harness::loaded(mailboxes());
    let job = h.keys("e");
    switch(&mut h, "travel");

    h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e1b".into()],
        }),
    });

    assert!(matches!(job[..], [Effect::SetEmails { .. }]));
    let ids: Vec<&str> = h.app.queue().rows().iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1"]);
    assert_eq!(h.app.total(), 1);
    assert_eq!(h.status(), "Could not archive: 1 email not updated");
}

#[test]
fn a_switch_that_lands_while_an_archive_from_its_mailbox_is_in_flight_leaves_the_row_out() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.keys("e").len(), 1);
    let effects = h.switch_to("inbox");
    let [
        Effect::Query {
            generation, query, ..
        },
    ] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    let result = h.store_mut().query(query);
    h.send(Msg::Loaded {
        generation: *generation,
        result,
    });

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.app.total(), 2);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.app.total(), 3);
}

#[test]
fn a_switch_whose_result_predates_a_confirmed_archive_reconciles_once_it_lands() {
    let mut h = Harness::loaded(store());
    let archive = h.keys("e");
    let effects = h.switch_to("inbox");
    let [
        Effect::Query {
            generation, query, ..
        },
    ] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    // The query is answered before the archive reaches the store.
    let result = h.store_mut().query(query);
    assert_eq!(h.resolve(archive), []);

    let effects = h.send(Msg::Loaded {
        generation: *generation,
        result,
    });
    assert_eq!(effects, [reconcile(3)]);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.resolve(effects), []);

    // The email in both the Inbox and Sent now stands for t1.
    assert_eq!(h.subjects(), ["Lunch", "Invoice", ""]);
}
