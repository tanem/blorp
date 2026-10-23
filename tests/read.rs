//! `r` marks the thread under the cursor read: its dot clears at once, the
//! mark goes through the outbox, and a failure brings the dot back.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, SetResult};
use crossterm::event::KeyCode;
use serde_json::{Value, json};

/// Thread t1 spreads over the Inbox, Travel, Sent, Trash, and Junk, with
/// its emails in the Inbox, Travel, Trash, and Junk unread and the one in
/// Sent read. Thread t2 is a read Inbox email and t3 an unread one. Newest
/// first: t1, t2, t3.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-junk", "Junk", "junk")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Trip"),
        )
        .email(EmailSpec::new("e1b", "t1", "2026-10-18T08:10:00Z").in_mailbox("mb-travel"))
        .email(
            EmailSpec::new("e1c", "t1", "2026-10-18T08:20:00Z")
                .in_mailbox("mb-sent")
                .seen(),
        )
        .email(EmailSpec::new("e1d", "t1", "2026-10-18T08:30:00Z").in_mailbox("mb-trash"))
        .email(EmailSpec::new("e1e", "t1", "2026-10-18T08:40:00Z").in_mailbox("mb-junk"))
        .email(
            EmailSpec::new("e1f", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Re: Trip"),
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

fn reconcile(generation: u64) -> Effect {
    Effect::Reconcile {
        generation,
        query: inbox_query(),
        retry: true,
    }
}

/// The update that marks `ids` read.
fn read(ids: &[&str]) -> EmailUpdate {
    let patch: EmailPatch = [("keywords/$seen".to_string(), Value::Bool(true))].into();
    ids.iter()
        .map(|id| (id.to_string(), patch.clone()))
        .collect()
}

/// Whether each row shows the unread dot, in order.
fn dots(h: &Harness) -> Vec<bool> {
    h.app.queue().rows().iter().map(|t| t.unread()).collect()
}

#[test]
fn r_marks_every_unread_email_of_the_thread_read_in_every_mailbox() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: read(&["e1a", "e1b", "e1d", "e1e", "e1f"]),
        }]
    );
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(dots(&h), [false, false, true]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.status(), "Marked read");
}

#[test]
fn r_on_a_read_thread_does_nothing_and_says_so() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("jr"), []);
    assert_eq!(h.status(), "Already read");
}

#[test]
fn r_on_an_empty_queue_does_nothing() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
    );

    assert_eq!(h.keys("r"), []);
    assert_eq!(h.status(), "");
}

#[test]
fn a_read_only_account_sends_nothing_and_says_so() {
    let mut h = Harness::loaded(store());
    h.app.set_read_only(true);

    assert_eq!(h.keys("r"), []);
    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(h.status(), "The API token is read-only");
}

#[test]
fn r_is_refused_while_disconnected() {
    let mut h = Harness::loaded(store());
    h.app.now = h.app.next_reconcile_at();
    h.send(Msg::ReconcileTick);
    h.send(Msg::Reconciled {
        generation: 2,
        result: Err(Error::Transport("connection refused".into())),
    });
    assert!(!h.app.connected());

    assert_eq!(h.keys("r"), []);
    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(h.status(), "Disconnected, action not sent");
}

#[test]
fn the_mark_reaches_the_store() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("r");

    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);

    assert_eq!(dots(&h), [false, false, true]);
    assert_eq!(h.status(), "Marked read");
}

#[test]
fn not_updated_brings_the_dot_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("r");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e1b".into()],
        }),
    });

    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(h.status(), "Could not mark read: 1 email not updated");
    assert_eq!(effects, [reconcile(2)]);
}

#[test]
fn a_failed_request_brings_the_dot_back_and_reconciles() {
    let mut h = Harness::loaded(store());
    h.keys("jjr");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.status(), "Could not mark read: Fastmail error: HTTP 400");
    assert_eq!(effects, [reconcile(2)]);
}

#[test]
fn a_transient_failure_brings_back_every_action_in_the_outbox_and_disconnects() {
    let mut h = Harness::loaded(store());
    h.keys("rjje");
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch"]);
    assert_eq!(dots(&h), [false, false]);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(effects, [Effect::DropJobs { through: 2 }]);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(dots(&h), [true, false, true]);
    assert!(!h.app.connected());
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not send 2 actions"
    );
}

#[test]
fn a_transient_failure_of_one_mark_says_so() {
    let mut h = Harness::loaded(store());
    h.keys("r");

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not mark read"
    );
}

#[test]
fn a_transient_failure_of_several_marks_counts_the_threads() {
    let mut h = Harness::loaded(store());
    h.keys("rjjr");

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(dots(&h), [true, false, true]);
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not mark 2 threads read"
    );
}

#[test]
fn no_reconcile_is_sent_while_a_mark_is_in_flight() {
    let mut h = Harness::loaded(store());
    h.keys("r");

    assert_eq!(h.send(Msg::Wake), []);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult::default()),
    });

    assert_eq!(effects, [reconcile(2)]);
}

#[test]
fn a_reconcile_result_from_before_a_mark_was_sent_is_discarded() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.send(Msg::Wake), [reconcile(2)]);
    let snapshot = Msg::Reconciled {
        generation: 2,
        result: store().reconcile(&inbox_query()),
    };
    let marked = h.keys("r");
    assert_eq!(h.resolve(marked), []);

    let effects = h.send(snapshot);

    assert_eq!(dots(&h), [false, false, true], "the dot does not come back");
    assert_eq!(effects, [reconcile(3)]);
}

#[test]
fn a_switch_that_lands_while_a_mark_is_in_flight_shows_the_thread_read() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.keys("r").len(), 1);
    h.keys("mtravel");
    let effects = h.press(KeyCode::Enter);
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
    assert_eq!(h.app.queue_path(), "Travel");
    assert_eq!(dots(&h), [false]);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });
    assert_eq!(dots(&h), [true], "the dot comes back in the mailbox shown");
}

#[test]
fn q_waits_for_a_mark_in_flight_and_counts_it_unconfirmed() {
    let mut h = Harness::loaded(store());
    h.keys("r");

    assert_eq!(h.keys("q"), []);
    assert_eq!(h.status(), "Waiting for Fastmail to confirm 1 action");
    assert_eq!(h.send(Msg::QuitTimeout), [Effect::QuitUnconfirmed(1)]);
}

#[test]
fn q_quits_once_the_mark_in_flight_is_confirmed() {
    let mut h = Harness::loaded(store());
    h.keys("r");
    h.keys("q");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult::default()),
    });

    assert_eq!(effects, [Effect::Quit]);
}

#[test]
fn a_failed_mark_brings_the_dot_back_on_a_row_an_archive_in_flight_puts_back() {
    let mut h = Harness::loaded(store());
    h.keys("re");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });
    h.send(Msg::Sent {
        job: 2,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(dots(&h), [true, false, true]);
}
