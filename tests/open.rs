//! `o` opens the thread under the cursor in Fastmail's web UI. Opening
//! sends nothing to Fastmail.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::Error;
use blorp::jmap::fake::{EmailSpec, Store, StoreBuilder};
use crossterm::event::KeyCode;

/// `n` single-email Inbox threads, newest first: t1 holding e1 to tn
/// holding en.
fn inbox(n: usize) -> StoreBuilder {
    let mut store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-sent", "Sent", "sent");
    for i in 1..=n {
        let received = format!("2026-10-18T11:{:02}:00Z", 59 - i);
        store = store.email(
            EmailSpec::new(&format!("e{i}"), &format!("t{i}"), &received)
                .in_mailbox("mb-inbox")
                .subject(&format!("Thread {i}")),
        );
    }
    store
}

fn open(thread: &str, email: &str) -> Effect {
    Effect::Open {
        url: format!("https://app.fastmail.com/mail/Inbox/{thread}.{email}"),
    }
}

#[test]
fn o_opens_the_thread_under_the_cursor() {
    let mut h = Harness::loaded(inbox(3).build());
    h.keys("j");

    assert_eq!(h.keys("o"), [open("t2", "e2")]);
}

#[test]
fn o_opens_a_thread_at_its_newest_email_in_the_queue_mailbox() {
    let mut h = Harness::loaded(
        inbox(0)
            .email(
                EmailSpec::new("e-old", "t-long", "2026-10-18T09:00:00Z")
                    .in_mailbox("mb-inbox")
                    .subject("Long"),
            )
            .email(EmailSpec::new("e-mid", "t-long", "2026-10-18T10:00:00Z").in_mailbox("mb-inbox"))
            .email(
                EmailSpec::new("e-reply", "t-long", "2026-10-18T11:00:00Z").in_mailbox("mb-sent"),
            )
            .build(),
    );

    assert_eq!(h.keys("o"), [open("t-long", "e-mid")]);
}

#[test]
fn o_on_an_empty_queue_does_nothing() {
    let mut h = Harness::loaded(inbox(0).build());

    assert_eq!(h.keys("o"), []);
    assert_eq!(h.status(), "");
}

#[test]
fn o_opens_while_disconnected() {
    let mut h = Harness::loaded(inbox(3).build());
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

    assert_eq!(h.keys("o"), [open("t1", "e1")]);
}

#[test]
fn a_failure_to_open_shows_on_the_bottom_line() {
    let mut h = Harness::loaded(inbox(3).build());

    assert_eq!(
        h.send(Msg::OpenFailed {
            error: "no browser found".into(),
        }),
        []
    );
    assert_eq!(h.status(), "Could not open the browser: no browser found");
}

#[test]
fn o_uses_the_name_of_the_mailbox_shown() {
    let mut h = Harness::loaded(
        inbox(1)
            .mailbox("mb-work", "Work")
            .child_mailbox("mb-receipts", "Receipts", "mb-work")
            .email(
                EmailSpec::new("e-order", "t-order", "2026-10-17T09:00:00Z")
                    .in_mailbox("mb-receipts"),
            )
            .build(),
    );
    h.keys("mreceipts");
    let effects = h.press(KeyCode::Enter);
    assert_eq!(h.resolve(effects), []);

    assert_eq!(
        h.keys("o"),
        [Effect::Open {
            url: "https://app.fastmail.com/mail/Receipts/t-order.e-order".into(),
        }]
    );
}
