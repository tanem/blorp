//! The due-snooze check on every reconcile, early and no-entry returns, and
//! the snooze mailbox as a queue, at the `update` level.

use blorp::app::{Action, Effect, Msg, Step};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailQuery, EmailUpdate};
use blorp::snooze::{Entry, Schedule};
use jiff::Timestamp;
use serde_json::{Value, json};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

/// The harness's clock, a Sunday, in UTC.
const NOW: &str = "2026-10-18T12:00:00Z";

/// When the threads in [`schedule`] were snoozed.
const SNOOZED_AT: &str = "2026-10-10T09:00:00Z";

const SNOOZE_QUEUE: &str = "in:\"Snoozed (blorp)\"";

/// The Inbox holds t5. The snooze mailbox holds t1, t2, and t3, which also
/// has an older email in Travel that was there when it was snoozed.
fn builder() -> blorp::jmap::fake::StoreBuilder {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-junk", "Junk", "junk")
        .role_mailbox("mb-sent", "Sent", "sent")
        .mailbox("mb-travel", "Travel")
        .mailbox("mb-snooze", "Snoozed (blorp)")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-03T09:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .subject("Renewal"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-02T09:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .subject("Flights"),
        )
        .email(
            EmailSpec::new("e3a", "t3", "2026-10-01T09:00:00Z")
                .in_mailbox("mb-travel")
                .seen()
                .subject("Hotel"),
        )
        .email(
            EmailSpec::new("e3b", "t3", "2026-10-04T09:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .subject("Re: Hotel"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
}

fn store() -> Store {
    builder().build()
}

fn entry(email: &str, origin: &str, return_at: &str) -> Entry {
    Entry {
        emails: [(email.into(), origin.into())].into(),
        snoozed_at: at(SNOOZED_AT),
        return_at: at(return_at),
        returned_at: None,
    }
}

/// t1 returns soonest, to the Inbox; t2 next, to Travel; t3 last, to
/// Travel. None is due.
fn schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set("t1", Some(entry("e1", "mb-inbox", "2026-10-18T12:00:30Z")));
    schedule.set("t2", Some(entry("e2", "mb-travel", "2026-10-20T08:00:00Z")));
    schedule.set(
        "t3",
        Some(entry("e3b", "mb-travel", "2026-10-25T08:00:00Z")),
    );
    schedule
}

fn update(entries: &[(&str, Value)]) -> EmailUpdate {
    entries
        .iter()
        .map(|(id, patch)| {
            (
                id.to_string(),
                serde_json::from_value(patch.clone()).unwrap(),
            )
        })
        .collect()
}

fn sets(effects: &[Effect]) -> Vec<&EmailUpdate> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::SetEmails { update, .. } => Some(update),
            _ => None,
        })
        .collect()
}

fn command(h: &mut Harness, text: &str) -> Vec<Effect> {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(h.send(Msg::Prompt(blorp::prompt::Key::Enter)));
    effects
}

/// Opens the snooze mailbox's queue.
fn open_snooze_queue(h: &mut Harness) {
    let effects = h.send(Msg::SetFilter(SNOOZE_QUEUE.into()));
    h.settle(effects);
}

/// The move of a returning email out of the snooze mailbox into `origin`,
/// marked unread.
fn return_patch(origin: &str) -> Value {
    json!({
        "mailboxIds/mb-snooze": null,
        format!("mailboxIds/{origin}"): true,
        "keywords/$seen": null
    })
}

#[test]
fn every_reconcile_queries_the_snooze_mailbox_and_fetches_the_due_threads_by_id() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    h.app.now = at("2026-10-18T12:01:00Z");

    let effects = h.send(Msg::ReconcileTick);

    let [Effect::Reconcile { snoozed, .. }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *snoozed,
        Some(EmailQuery {
            filter: json!({"inMailbox": "mb-snooze"}),
            ascending: false,
            limit: 500,
            extra_threads: vec!["t1".into()],
        })
    );
}

#[test]
fn a_reconcile_returns_a_thread_that_fell_due_since_the_last() {
    let (mut h, effects) = Harness::launched(store(), false, schedule());
    assert_eq!(effects, vec![], "nothing is due at launch");
    h.app.now = at("2026-10-18T12:01:00Z");

    let effects = h.send(Msg::ReconcileTick);
    let rest = h.settle(effects);

    assert!(
        matches!(rest[..], [Effect::WriteSchedule { job: Some(_), .. }]),
        "{rest:?}"
    );
    assert_eq!(
        h.app.schedule().get("t1").unwrap().returned_at,
        Some(at("2026-10-18T12:01:00Z"))
    );
    assert_eq!(h.subjects(), ["Renewal", "Lunch"]);
    assert_eq!(h.status(), "Returned 1 thread from snooze");
}

#[test]
fn r_runs_the_due_snooze_check_too() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    h.app.now = at("2026-10-18T12:01:00Z");

    let effects = h.keys("R");
    let effects = h.resolve(effects);

    assert_eq!(
        sets(&effects),
        [&update(&[("e1", return_patch("mb-inbox"))])]
    );
}

#[test]
fn a_new_email_outside_the_snooze_mailbox_returns_its_thread_early_in_any_queue() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    let effects = h.send(Msg::SetFilter("in:archive".into()));
    h.settle(effects);
    h.store_mut().deliver(
        EmailSpec::new("e2r", "t2", "2026-10-18T11:00:00Z")
            .in_mailbox("mb-inbox")
            .subject("Re: Flights"),
    );

    let effects = h.send(Msg::Wake);
    let effects = h.resolve(effects);

    assert_eq!(
        sets(&effects),
        [&update(&[("e2", return_patch("mb-travel"))])]
    );
    assert_eq!(
        h.app.schedule().get("t2").unwrap().returned_at,
        Some(at(NOW))
    );
}

#[test]
fn an_early_return_moves_the_snoozed_emails_back_to_their_origins() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    h.store_mut().deliver(
        EmailSpec::new("e3r", "t3", "2026-10-18T11:00:00Z")
            .in_mailbox("mb-travel")
            .subject("Re: Hotel"),
    );
    let effects = h.send(Msg::SetFilter("in:travel".into()));
    h.settle(effects);

    let effects = h.send(Msg::Wake);
    h.settle(effects);

    assert_eq!(h.subjects(), ["Re: Hotel"]);
    let thread = &h.app.queue().threads()[0];
    let travel: Vec<&str> = thread
        .emails
        .iter()
        .filter(|e| e.mailbox_ids.contains("mb-travel"))
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(travel, ["e3a", "e3b", "e3r"]);
}

#[test]
fn an_email_in_sent_trash_or_junk_or_one_older_than_the_snooze_does_not_return_it() {
    let mut store = store();
    for (id, mailbox, received) in [
        ("e2s", "mb-sent", "2026-10-18T11:00:00Z"),
        ("e2t", "mb-trash", "2026-10-18T11:01:00Z"),
        ("e2j", "mb-junk", "2026-10-18T11:02:00Z"),
        ("e2o", "mb-inbox", "2026-10-09T09:00:00Z"),
    ] {
        store.deliver(EmailSpec::new(id, "t2", received).in_mailbox(mailbox));
    }
    let (mut h, effects) = Harness::launched(store, false, schedule());
    assert_eq!(effects, vec![]);

    let effects = h.send(Msg::Wake);
    let effects = h.resolve(effects);

    assert_eq!(sets(&effects), Vec::<&EmailUpdate>::new());
    assert_eq!(h.app.schedule(), &schedule());
}

#[test]
fn a_thread_in_the_snooze_mailbox_with_no_entry_returns_to_the_inbox_unread_flag_untouched() {
    let store = builder()
        .email(
            EmailSpec::new("e4", "t4", "2026-10-05T09:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .flagged()
                .subject("Stray"),
        )
        .build();
    let mut h = Harness::with_schedule(store, schedule());
    let effects = h.start();
    let effects = h.resolve(effects);
    let effects = h.resolve(effects);

    assert_eq!(
        sets(&effects),
        [&update(&[("e4", return_patch("mb-inbox"))])]
    );
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Stray", "Lunch"]);
    let returned = &h.app.queue().threads()[0];
    assert!(returned.unread());
    assert!(returned.flagged());
    assert_eq!(
        h.app.schedule().get("t4"),
        Some(&Entry {
            emails: [("e4".into(), "mb-inbox".into())].into(),
            snoozed_at: at(NOW),
            return_at: at(NOW),
            returned_at: Some(at(NOW)),
        })
    );
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn the_snooze_mailbox_sorts_soonest_first_with_threads_with_no_entry_first() {
    let mut schedule = schedule();
    schedule.set("t2", None);
    // Not started, so no due-snooze check returns t2 before it is seen.
    let mut h = Harness::with_schedule(store(), schedule);
    open_snooze_queue(&mut h);

    assert_eq!(h.subjects(), ["Flights", "Renewal", "Re: Hotel"]);
    assert_eq!(h.app.sort_name(), "soonest");

    let effects = h.keys("t");
    h.settle(effects);

    assert_eq!(h.subjects(), ["Flights", "Re: Hotel", "Renewal"]);
    assert_eq!(h.app.sort_name(), "latest");
}

#[test]
fn other_queues_show_newest_and_oldest() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.app.sort_name(), "newest");
    h.keys("t");
    assert_eq!(h.app.sort_name(), "oldest");
}

#[test]
fn z_in_the_snooze_mailbox_reschedules_and_moves_nothing() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);
    assert_eq!(h.subjects(), ["Renewal", "Flights", "Re: Hotel"]);

    let effects = command(&mut h, "z 2026-10-30");

    assert!(
        matches!(effects[0], Effect::WriteSchedule { job: Some(_), .. }),
        "{effects:?}"
    );
    assert!(sets(&effects).iter().all(|u| u.is_empty()), "{effects:?}");
    assert_eq!(
        h.app.schedule().get("t1"),
        Some(&entry("e1", "mb-inbox", "2026-10-30T08:00:00Z"))
    );
    assert_eq!(h.subjects(), ["Flights", "Re: Hotel", "Renewal"]);
    assert_eq!(h.cursor(), 0, "the next thread slides into place");
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn undo_of_a_reschedule_restores_the_previous_time() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);
    let effects = command(&mut h, "z 2026-10-30");
    h.settle(effects);

    let effects = h.keys("u");
    h.settle(effects);

    assert_eq!(h.app.schedule(), &schedule());
    assert_eq!(h.subjects(), ["Renewal", "Flights", "Re: Hotel"]);
}

#[test]
fn unsnooze_returns_the_selection_now_and_undo_snoozes_it_again() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);
    h.keys("j");

    let effects = command(&mut h, "unsnooze");

    assert_eq!(
        sets(&effects),
        [&update(&[("e2", return_patch("mb-travel"))])]
    );
    assert_eq!(h.subjects(), ["Renewal", "Re: Hotel"]);
    assert_eq!(
        h.app.schedule().get("t2").unwrap().returned_at,
        Some(at(NOW))
    );
    assert_eq!(h.status(), "Unsnoozed 1 thread");
    h.resolve(effects);

    let effects = h.keys("u");
    h.settle(effects);

    assert_eq!(h.app.schedule(), &schedule());
    assert_eq!(h.subjects(), ["Renewal", "Flights", "Re: Hotel"]);
    assert!(!h.app.queue().threads()[1].unread());
}

#[test]
fn uz_unsnoozes_with_a_count() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);

    h.keys("2");
    let effects = command(&mut h, "uz");

    assert_eq!(
        sets(&effects),
        [&update(&[
            ("e1", return_patch("mb-inbox")),
            ("e2", return_patch("mb-travel")),
        ])]
    );
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn unsnooze_outside_the_snooze_mailbox_does_nothing() {
    let (mut h, _) = Harness::launched(store(), false, schedule());

    let effects = command(&mut h, "unsnooze");

    assert_eq!(effects, vec![]);
    assert_eq!(h.status(), "Nothing to unsnooze");
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn archive_in_the_snooze_mailbox_cancels_the_entry_and_undo_restores_it() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);

    let effects = h.keys("e");
    assert_eq!(
        sets(&effects),
        [&update(&[(
            "e1",
            json!({"mailboxIds/mb-snooze": null, "mailboxIds/mb-archive": true})
        )])]
    );
    h.resolve(effects);
    assert_eq!(h.app.schedule().get("t1"), None);
    assert_eq!(h.subjects(), ["Flights", "Re: Hotel"]);

    let effects = h.keys("u");
    h.settle(effects);

    assert_eq!(h.app.schedule(), &schedule());
    assert_eq!(h.subjects(), ["Renewal", "Flights", "Re: Hotel"]);
}

#[test]
fn in_dry_run_r_returns_a_due_thread_in_memory_only() {
    let (mut h, _) = Harness::launched(store(), true, schedule());
    h.app.now = at("2026-10-18T12:01:00Z");

    let effects = h.keys("R");
    let rest = h.settle(effects);

    assert_eq!(
        rest,
        vec![Effect::LogDryRun {
            step: Step::Act,
            action: Action::Return,
            thread_ids: vec!["t1".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Renewal", "Lunch"]);
    assert_eq!(h.status(), "[dry] Returned 1 thread from snooze");
}

#[test]
fn a_return_on_a_reconcile_keeps_the_cursor_on_its_thread() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    h.store_mut().deliver(
        EmailSpec::new("e2r", "t2", "2026-10-18T09:30:00Z")
            .in_mailbox("mb-inbox")
            .subject("Re: Flights"),
    );
    h.app.now = at("2026-10-18T12:01:00Z");

    let effects = h.send(Msg::Wake);
    h.settle(effects);

    assert_eq!(h.subjects(), ["Renewal", "Re: Flights", "Lunch"]);
    assert_eq!(h.subjects()[h.cursor()], "Lunch");
}

#[test]
fn the_launch_check_answered_after_a_reconcile_returned_the_thread_sends_nothing_more() {
    let mut schedule = schedule();
    schedule.set("t1", Some(entry("e1", "mb-inbox", "2026-10-18T08:00:00Z")));
    let mut h = Harness::with_schedule(store(), schedule);
    let effects = h.start();
    let effects = h.resolve(effects);
    let [Effect::QuerySnoozed { query }] = &effects[..] else {
        panic!("{effects:?}");
    };
    let stale = h.query(query);

    let effects = h.send(Msg::Wake);
    h.settle(effects);
    assert_eq!(h.subjects(), ["Renewal", "Lunch"]);
    let effects = h.send(Msg::SnoozedLoaded { result: stale });

    assert_eq!(effects, vec![]);
    assert_eq!(h.status(), "Returned 1 thread from snooze");
}

#[test]
fn f_in_the_snooze_mailbox_keeps_the_entry_and_writes_no_schedule() {
    let (mut h, _) = Harness::launched(store(), false, schedule());
    open_snooze_queue(&mut h);

    let effects = h.keys("f");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&[("e1", json!({"keywords/$flagged": true}))]),
        }]
    );
    h.resolve(effects);
    assert_eq!(h.app.schedule(), &schedule());
    assert_eq!(h.subjects(), ["Renewal", "Flights", "Re: Hotel"]);

    let effects = h.keys("u");
    h.settle(effects);
    assert_eq!(h.app.schedule(), &schedule());
}

#[test]
fn r_on_a_returned_thread_keeps_it_where_its_return_sorted_it() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-03T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Renewal"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .build();
    let mut schedule = Schedule::default();
    schedule.set(
        "t1",
        Some(Entry {
            returned_at: Some(at("2026-10-18T11:30:00Z")),
            ..entry("e1", "mb-inbox", "2026-10-18T11:30:00Z")
        }),
    );
    let (mut h, _) = Harness::launched(store, false, schedule.clone());
    assert_eq!(h.subjects(), ["Renewal", "Lunch"]);

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&[("e1", json!({"keywords/$seen": true}))]),
        }]
    );
    assert_eq!(h.subjects(), ["Renewal", "Lunch"]);
    assert_eq!(h.app.schedule(), &schedule);
}
