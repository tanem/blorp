//! `m` opens a list of every mailbox and switches the queue to the one
//! chosen.

use blorp::app::{Effect, MailboxChoices, Msg, QUEUE_LIMIT};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, Error};
use crossterm::event::KeyCode;
use serde_json::json;

/// Mailboxes listed out of path order, with Work/Receipts nested, and two
/// threads in the Inbox and two in Work/Receipts.
fn store() -> Store {
    Store::builder()
        .mailbox("mb-travel", "Travel")
        .role_mailbox("mb-archive", "Archive", "archive")
        .child_mailbox("mb-receipts", "Receipts", "mb-work")
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-work", "Work")
        .role_mailbox("mb-sent", "Sent", "sent")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-17T11:00:00Z")
                .in_mailbox("mb-receipts")
                .subject("Order 1"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-17T10:00:00Z")
                .in_mailbox("mb-receipts")
                .subject("Order 2"),
        )
        .build()
}

fn choices(typed: &str, paths: &[&str], index: usize) -> Option<MailboxChoices> {
    Some(MailboxChoices {
        typed: typed.into(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        index,
    })
}

const ALL: [&str; 6] = [
    "Inbox",
    "Archive",
    "Sent",
    "Travel",
    "Work",
    "Work/Receipts",
];

#[test]
fn m_lists_every_mailbox_inbox_first_with_the_one_shown_highlighted() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("m"), []);

    assert_eq!(h.app.mailbox_list(), choices("", &ALL, 0));
}

fn query(mailbox: &str) -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": mailbox}),
        limit: QUEUE_LIMIT,
    }
}

/// Opens the list, types `typed`, and presses Enter, returning the
/// switch's effects unanswered.
fn switch(h: &mut Harness, typed: &str) -> Vec<Effect> {
    h.keys("m");
    h.keys(typed);
    h.press(KeyCode::Enter)
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
fn the_list_highlights_the_mailbox_shown_when_it_opens() {
    let mut h = Harness::loaded(store());
    let effects = switch(&mut h, "receipts");
    h.resolve(effects);

    h.keys("m");

    assert_eq!(h.app.mailbox_list(), choices("", &ALL, 5));
}

#[test]
fn typing_narrows_the_list_ignoring_case_and_backspace_widens_it() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    h.keys("WoR");
    assert_eq!(
        h.app.mailbox_list(),
        choices("WoR", &["Work", "Work/Receipts"], 0)
    );

    h.keys("x");
    assert_eq!(h.app.mailbox_list(), choices("WoRx", &[], 0));

    h.press(KeyCode::Backspace);
    h.press(KeyCode::Backspace);
    assert_eq!(
        h.app.mailbox_list(),
        choices("Wo", &["Work", "Work/Receipts"], 0)
    );
}

#[test]
fn letters_that_are_keys_elsewhere_are_typed_into_the_filter() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.keys("jkeoqm"), []);

    assert_eq!(h.app.mailbox_list(), choices("jkeoqm", &[], 0));
    assert_eq!(h.cursor(), 0);
}

#[test]
fn the_arrows_ctrl_n_and_ctrl_p_move_the_highlight_and_stop_at_the_ends() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    h.press(KeyCode::Up);
    assert_eq!(h.app.mailbox_list().unwrap().index, 0);
    h.ctrl('p');
    assert_eq!(h.app.mailbox_list().unwrap().index, 0);

    h.press(KeyCode::Down);
    h.ctrl('n');
    assert_eq!(h.app.mailbox_list().unwrap().index, 2);
    for _ in 0..5 {
        h.ctrl('n');
    }
    assert_eq!(h.app.mailbox_list().unwrap().index, 5);
    h.press(KeyCode::Down);
    assert_eq!(h.app.mailbox_list().unwrap().index, 5);

    h.press(KeyCode::Up);
    h.ctrl('p');
    assert_eq!(h.app.mailbox_list().unwrap().index, 3);
}

#[test]
fn enter_switches_to_the_highlighted_mailbox_and_loads_it() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    h.keys("m");
    h.keys("work");
    h.press(KeyCode::Down);

    let effects = h.press(KeyCode::Enter);

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 2,
            query: query("mb-receipts"),
            retry: true,
        }]
    );
    assert_eq!(h.app.mailbox_list(), None);
    assert!(h.app.loading());
    assert_eq!(h.app.loading_path(), "Work/Receipts");
    // The queue shows the Inbox until the query lands.
    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);

    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.app.queue_path(), "Work/Receipts");
    assert_eq!(h.subjects(), ["Order 1", "Order 2"]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.app.total(), 2);
    assert!(!h.app.loading());
}

#[test]
fn enter_with_no_match_does_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("mzz");

    assert_eq!(h.press(KeyCode::Enter), []);

    assert_eq!(h.app.mailbox_list(), choices("zz", &[], 0));
}

#[test]
fn esc_closes_the_list_and_changes_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    h.keys("mtrav");

    assert_eq!(h.press(KeyCode::Esc), []);

    assert_eq!(h.app.mailbox_list(), None);
    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
}

#[test]
fn ctrl_c_quits_from_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

#[test]
fn a_failed_switch_stays_on_the_mailbox_shown_and_says_why() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    let generation = match switch(&mut h, "travel")[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };

    h.send(Msg::Loaded {
        generation,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), Error::Status(400).to_string());
    assert!(!h.app.loading());
    assert!(h.app.connected());
}

#[test]
fn a_switch_while_disconnected_is_tried_once_as_a_probe() {
    let mut h = Harness::loaded(store());
    disconnect(&mut h);

    let effects = switch(&mut h, "receipts");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: query("mb-receipts"),
            retry: false,
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert!(h.app.connected());
    assert_eq!(h.app.queue_path(), "Work/Receipts");
}

#[test]
fn a_switch_drops_a_reconcile_in_flight() {
    let mut h = Harness::loaded(store());
    let reconcile = h.send(Msg::Wake);
    let generation = match reconcile[..] {
        [Effect::Reconcile { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    let effects = switch(&mut h, "receipts");
    assert_eq!(h.resolve(effects), []);

    let result = Ok(h.store_mut().reconcile(&query("mb-inbox")).unwrap());
    assert_eq!(h.send(Msg::Reconciled { generation, result }), []);

    assert_eq!(h.subjects(), ["Order 1", "Order 2"]);
}

#[test]
fn reconciles_refresh_the_mailbox_shown() {
    let mut h = Harness::loaded(store());
    let effects = switch(&mut h, "receipts");
    h.resolve(effects);
    h.store_mut().deliver(
        EmailSpec::new("e5", "t5", "2026-10-18T09:00:00Z")
            .in_mailbox("mb-receipts")
            .subject("Order 3"),
    );

    let effects = h.send(Msg::Wake);
    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 3,
            query: query("mb-receipts"),
            retry: true,
        }]
    );
    assert_eq!(h.resolve(effects), []);

    assert_eq!(h.subjects(), ["Order 3", "Order 1", "Order 2"]);
}

#[test]
fn a_reconcile_shows_the_new_path_of_a_renamed_mailbox() {
    let mut h = Harness::loaded(store());
    let effects = switch(&mut h, "receipts");
    h.resolve(effects);
    h.store_mut().rename_mailbox("mb-work", "Job");

    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);

    assert_eq!(h.app.queue_path(), "Job/Receipts");
    assert_eq!(h.subjects(), ["Order 1", "Order 2"]);
}

#[test]
fn a_reconcile_that_finds_the_mailbox_shown_deleted_goes_back_to_the_inbox() {
    let mut h = Harness::loaded(store());
    let effects = switch(&mut h, "receipts");
    h.resolve(effects);
    h.store_mut().delete_mailbox("mb-receipts");

    let effects = h.send(Msg::Wake);
    let effects = h.resolve(effects);
    assert_eq!(
        effects,
        [Effect::Query {
            generation: 4,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.status(), "Mailbox no longer exists");

    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "Mailbox no longer exists");
}

#[test]
fn each_launch_starts_on_the_inbox() {
    let mut h = Harness::new(store());

    assert_eq!(
        h.start(),
        [Effect::Query {
            generation: 1,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
    assert_eq!(h.app.queue_path(), "Inbox");
}

#[test]
fn the_highlight_stays_on_a_match_when_a_reconcile_shortens_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m");
    for _ in 0..5 {
        h.ctrl('n');
    }
    h.store_mut().delete_mailbox("mb-receipts");

    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);

    assert_eq!(
        h.app.mailbox_list(),
        choices("", &["Inbox", "Archive", "Sent", "Travel", "Work"], 4)
    );
}
