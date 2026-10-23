//! `u` toggles unread only, under which the queue holds only the unread
//! threads of the mailbox shown: those with an email in it and an unread
//! email in any mailbox.

use blorp::app::{Effect, Msg, QUEUE_LIMIT};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailQuery, EmailUpdate, Error, SetResult};
use serde_json::{Value, json};

/// Newest first, the Inbox holds Lunch (unread), Trip (read in the Inbox,
/// with an unread reply in Archive), Invoice (unread), and Order (read).
/// Travel holds Flight (unread) and Hotel (read).
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2a", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e2b", "t2", "2026-10-18T10:30:00Z")
                .in_mailbox("mb-archive")
                .subject("Re: Trip"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Order"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-17T11:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Flight"),
        )
        .email(
            EmailSpec::new("e6", "t6", "2026-10-17T10:00:00Z")
                .in_mailbox("mb-travel")
                .seen()
                .subject("Hotel"),
        )
        .build()
}

fn query(mailbox: &str) -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": mailbox}),
        limit: QUEUE_LIMIT,
    }
}

fn unread_query(mailbox: &str) -> EmailQuery {
    EmailQuery {
        filter: json!({"operator": "AND", "conditions": [
            {"inMailbox": mailbox},
            {"operator": "NOT", "conditions": [{"allInThreadHaveKeyword": "$seen"}]},
        ]}),
        limit: QUEUE_LIMIT,
    }
}

#[test]
fn u_reloads_the_queue_with_only_the_unread_threads() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("u");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 2,
            query: unread_query("mb-inbox"),
            retry: true,
        }]
    );
    assert!(h.app.loading());
    assert!(
        !h.app.unread_only(),
        "the setting changes when the reload lands"
    );
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice", "Order"]);

    assert_eq!(h.resolve(effects), []);
    assert!(h.app.unread_only());
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert_eq!(h.app.total(), 3);
    assert_eq!(h.app.queue_path(), "Inbox");
}

#[test]
fn a_thread_read_in_the_mailbox_shown_and_unread_in_another_is_held_with_its_dot() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(h.subjects()[1], "Trip");
    assert!(h.app.queue().rows()[1].unread());
}

#[test]
fn a_thread_whose_emails_are_all_read_is_not_held() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2a", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e2b", "t2", "2026-10-18T10:30:00Z")
                .in_mailbox("mb-archive")
                .seen()
                .subject("Re: Trip"),
        )
        .build();
    let mut h = Harness::loaded(store);

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Lunch"]);
    assert_eq!(h.app.total(), 1);
}

#[test]
fn u_again_reloads_every_thread_of_the_mailbox() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);

    let effects = h.keys("u");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert!(!h.app.unread_only());
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice", "Order"]);
    assert_eq!(h.app.total(), 4);
}

#[test]
fn u_twice_before_the_reload_lands_turns_it_back_off() {
    let mut h = Harness::loaded(store());
    h.keys("u");

    let effects = h.keys("u");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert!(!h.app.unread_only());
}

#[test]
fn the_cursor_stays_on_its_thread_when_that_is_still_in_the_queue() {
    let mut h = Harness::loaded(store());
    h.keys("jj");
    assert_eq!(h.cursor(), 2);

    let effects = h.keys("u");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert_eq!(h.cursor(), 2);

    let effects = h.keys("u");
    h.resolve(effects);
    assert_eq!(h.cursor(), 2, "back on Invoice");
}

#[test]
fn the_cursor_goes_to_the_first_row_when_its_thread_leaves_the_queue() {
    let mut h = Harness::loaded(store());
    h.keys("jjj");
    assert_eq!(h.subjects()[h.cursor()], "Order");

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(h.cursor(), 0);
}

#[test]
fn the_setting_stays_on_across_a_switch() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);
    let effects = h.switch_to("travel");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: unread_query("mb-travel"),
            retry: true,
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert!(h.app.unread_only());
    assert_eq!(h.app.queue_path(), "Travel");
    assert_eq!(h.subjects(), ["Flight"]);
}

#[test]
fn a_switch_typed_after_u_before_it_lands_keeps_the_setting_typed() {
    let mut h = Harness::loaded(store());
    h.keys("u");
    let effects = h.switch_to("travel");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: unread_query("mb-travel"),
            retry: true,
        }]
    );
}

#[test]
fn each_launch_starts_with_the_setting_off() {
    let mut h = Harness::new(store());

    assert_eq!(
        h.start(),
        [Effect::Query {
            generation: 1,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
    assert!(!h.app.unread_only());
}

#[test]
fn a_failed_reload_leaves_the_queue_and_the_setting_as_they_were() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    h.keys("u");

    h.send(Msg::Loaded {
        generation: 2,
        result: Err(Error::Status(400)),
    });

    assert!(!h.app.unread_only());
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice", "Order"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), Error::Status(400).to_string());
    assert!(!h.app.loading());

    let effects = h.send(Msg::Wake);
    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 3,
            query: query("mb-inbox"),
            retry: true,
        }]
    );
}

#[test]
fn a_toggle_while_disconnected_is_tried_once_as_a_probe() {
    let mut h = Harness::loaded(store());
    h.app.now = h.app.next_reconcile_at();
    h.send(Msg::ReconcileTick);
    h.send(Msg::Reconciled {
        generation: 2,
        result: Err(Error::Transport("connection refused".into())),
    });
    assert!(!h.app.connected());

    let effects = h.keys("u");

    assert_eq!(
        effects,
        [Effect::Query {
            generation: 3,
            query: unread_query("mb-inbox"),
            retry: false,
        }]
    );
    assert_eq!(h.resolve(effects), []);
    assert!(h.app.connected());
    assert!(h.app.unread_only());
}

#[test]
fn reconciles_use_the_setting() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);
    h.keys("j");
    h.store_mut().change_email("e1", |email| {
        email.keywords.insert("$seen".into(), true);
    });
    h.store_mut().deliver(
        EmailSpec::new("e7", "t7", "2026-10-18T11:30:00Z")
            .in_mailbox("mb-inbox")
            .subject("Dinner"),
    );

    let effects = h.send(Msg::Wake);
    assert_eq!(
        effects,
        [Effect::Reconcile {
            generation: 3,
            query: unread_query("mb-inbox"),
            retry: true,
        }]
    );
    assert_eq!(h.resolve(effects), []);

    assert_eq!(h.subjects(), ["Dinner", "Trip", "Invoice"]);
    assert_eq!(h.subjects()[h.cursor()], "Trip");
    assert_eq!(h.app.total(), 3);
}

#[test]
fn u_with_no_mailbox_shown_does_nothing() {
    let mut h = Harness::loaded(Store::builder().mailbox("mb-travel", "Travel").build());

    assert_eq!(h.keys("u"), []);
    assert_eq!(h.status(), "No Inbox mailbox");
}

/// The app with unread only on over [`store`].
fn unread_only() -> Harness {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    h
}

/// The update that marks `ids` read.
fn read(ids: &[&str]) -> EmailUpdate {
    let patch: EmailPatch = [("keywords/$seen".to_string(), Value::Bool(true))].into();
    ids.iter()
        .map(|id| (id.to_string(), patch.clone()))
        .collect()
}

#[test]
fn r_takes_the_row_out_at_once_and_the_cursor_keeps_its_row_number() {
    let mut h = unread_only();

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: read(&["e1"]),
        }]
    );
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.app.total(), 2);
    assert_eq!(h.status(), "Marked read");

    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
}

#[test]
fn r_on_a_thread_whose_unread_email_is_in_another_mailbox_takes_the_row_out() {
    let mut h = unread_only();
    h.keys("j");

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: read(&["e2b"]),
        }]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.app.total(), 2);

    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Wake);
    assert_eq!(h.resolve(effects), []);
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

fn reconcile_unread(generation: u64) -> Effect {
    Effect::Reconcile {
        generation,
        query: unread_query("mb-inbox"),
        retry: true,
    }
}

#[test]
fn r_on_the_last_row_leaves_the_cursor_on_the_new_last_row() {
    let mut h = unread_only();
    h.keys("jj");

    h.keys("r");

    assert_eq!(h.subjects(), ["Lunch", "Trip"]);
    assert_eq!(h.cursor(), 1);
}

#[test]
fn a_failed_mark_puts_the_row_back_and_reconciles() {
    let mut h = unread_only();
    h.keys("r");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert!(h.app.queue().rows()[0].unread());
    assert_eq!(h.app.total(), 3);
    assert_eq!(h.status(), "Could not mark read: Fastmail error: HTTP 400");
    assert_eq!(effects, [reconcile_unread(3)]);
}

#[test]
fn not_updated_puts_the_row_back() {
    let mut h = unread_only();
    h.keys("r");

    h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e1".into()],
        }),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
}

#[test]
fn a_transient_failure_puts_back_every_row_the_outbox_took_out() {
    let mut h = unread_only();
    h.keys("rr");
    assert_eq!(h.subjects(), ["Invoice"]);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(effects, [Effect::DropJobs { through: 2 }]);
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert_eq!(h.app.total(), 3);
    assert_eq!(
        h.status(),
        "Disconnected from Fastmail; could not mark 2 threads read"
    );
}

#[test]
fn a_reload_under_the_setting_leaves_out_a_thread_a_mark_in_flight_reads() {
    let mut h = Harness::loaded(store());
    h.keys("r");

    let effects = h.keys("u");
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

    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(h.app.total(), 2);

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);
    assert_eq!(h.resolve(effects), []);
    assert_eq!(
        h.subjects(),
        ["Lunch", "Trip", "Invoice"],
        "the reconcile brings it back"
    );
}

#[test]
fn a_mark_typed_under_the_setting_that_fails_after_it_is_turned_off_brings_back_the_dot() {
    let mut h = unread_only();
    h.keys("r");

    let effects = h.keys("u");
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
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice", "Order"]);
    assert!(!h.app.queue().rows()[0].unread());

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice", "Order"]);
    assert!(h.app.queue().rows()[0].unread());
    assert_eq!(h.app.total(), 4);
}

#[test]
fn a_failed_archive_of_a_read_thread_stays_out_once_the_setting_is_on() {
    let mut h = Harness::loaded(store());
    h.keys("jjje");
    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    let effects = h.keys("u");
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

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert_eq!(h.app.total(), 3);
}

#[test]
fn a_failed_archive_of_a_thread_whose_unread_email_is_in_another_mailbox_is_put_back() {
    let mut h = Harness::loaded(store());
    h.keys("je");
    assert_eq!(h.subjects(), ["Lunch", "Invoice", "Order"]);
    let effects = h.keys("u");
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

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert!(h.app.queue().rows()[1].unread());
    assert_eq!(h.app.total(), 3);
}

#[test]
fn a_transient_failure_puts_back_a_thread_marked_read_then_archived() {
    let mut h = Harness::loaded(store());
    h.keys("re");
    let effects = h.keys("u");
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
    assert_eq!(h.subjects(), ["Trip", "Invoice"]);

    h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert_eq!(h.subjects(), ["Lunch", "Trip", "Invoice"]);
    assert!(h.app.queue().rows()[0].unread());
    assert_eq!(h.app.total(), 3);
}
