//! Snoozing threads, the schedule, return at launch, and undo, at the
//! `update` level.

use blorp::app::{Action, Effect, Msg, Step};
use blorp::harness::{Harness, SCHEDULE_FILE};
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailUpdate, Error, SetResult};
use blorp::snooze::{Entry, Schedule};
use jiff::Timestamp;
use serde_json::{Value, json};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

/// The harness's clock, a Sunday, in UTC.
const NOW: &str = "2026-10-18T12:00:00Z";

/// Newest first: t1 (in the Inbox and Travel, with a Sent reply), t2, t3.
/// Thread t4 is snoozed.
fn store() -> Store {
    builder()
        .mailbox("mb-snooze", "Snoozed (blorp)")
        .email(
            EmailSpec::new("e4", "t4", "2026-10-01T09:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .flagged()
                .subject("Renewal"),
        )
        .build()
}

/// [`store`] before anything was snoozed, with no snooze mailbox.
fn store_without_snooze_mailbox() -> Store {
    builder().build()
}

fn builder() -> blorp::jmap::fake::StoreBuilder {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-fm-snoozed", "Snoozed", "snoozed")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e1b", "t1", "2026-10-18T08:10:00Z")
                .in_mailbox("mb-travel")
                .seen(),
        )
        .email(
            EmailSpec::new("e1c", "t1", "2026-10-18T08:20:00Z")
                .in_mailbox("mb-sent")
                .seen(),
        )
        .email(
            EmailSpec::new("e1d", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .flagged()
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
}

fn command(h: &mut Harness, text: &str) -> Vec<Effect> {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(h.send(Msg::Prompt(blorp::prompt::Key::Enter)));
    effects
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

fn schedule_of(contents: &str) -> Value {
    serde_json::from_str(contents).unwrap()
}

#[test]
fn snooze_moves_the_queue_mailbox_messages_and_records_each_origin() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "snooze tomorrow");

    let until = at("2026-10-19T08:00:00Z");
    assert_eq!(
        effects,
        vec![
            Effect::WriteSchedule {
                job: Some(1),
                path: SCHEDULE_FILE.into(),
                contents: h.app.schedule().to_json(&jiff::tz::TimeZone::UTC),
            },
            Effect::SetEmails {
                job: 1,
                update: update(&[
                    (
                        "e1a",
                        json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-snooze": true})
                    ),
                    (
                        "e1d",
                        json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-snooze": true})
                    ),
                ]),
            },
        ]
    );
    assert_eq!(
        h.app.schedule().get("t1"),
        Some(&Entry {
            emails: [
                ("e1a".into(), "mb-inbox".into()),
                ("e1d".into(), "mb-inbox".into())
            ]
            .into(),
            snoozed_at: at(NOW),
            return_at: until,
            returned_at: None,
        })
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "Snoozed 1 thread until Mon 08:00");
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn the_schedule_is_written_before_the_email_set() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "z 2h");

    let Effect::WriteSchedule { contents, .. } = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(
        schedule_of(contents)["threads"]["t1"],
        json!({
            "emails": {"e1a": "mb-inbox", "e1d": "mb-inbox"},
            "snoozedAt": "2026-10-18T12:00:00+00:00",
            "returnAt": "2026-10-18T14:00:00+00:00",
            "returnedAt": null
        })
    );
    assert!(matches!(effects[1], Effect::SetEmails { job: 1, .. }));
}

#[test]
fn snooze_in_all_mail_moves_every_message_outside_the_excluded_mailboxes() {
    let mut h = Harness::loaded(store());
    let effects = h.send(Msg::SetFilter("in:any".into()));
    h.resolve(effects);

    let effects = command(&mut h, "snooze weekend");

    let Some(Effect::SetEmails { update: sent, .. }) = effects.get(1) else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *sent,
        update(&[
            (
                "e1a",
                json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-snooze": true})
            ),
            (
                "e1b",
                json!({"mailboxIds/mb-travel": null, "mailboxIds/mb-snooze": true})
            ),
            (
                "e1d",
                json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-snooze": true})
            ),
        ])
    );
    assert_eq!(
        h.app.schedule().get("t1").unwrap().emails,
        [
            ("e1a".into(), "mb-inbox".into()),
            ("e1b".into(), "mb-travel".into()),
            ("e1d".into(), "mb-inbox".into()),
        ]
        .into()
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn a_count_snoozes_that_many_threads_as_one_undo_entry() {
    let mut h = Harness::loaded(store());

    h.keys("2");
    let effects = command(&mut h, "z weekend");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Invoice"]);
    assert_eq!(h.status(), "Snoozed 2 threads until Sat 08:00");
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn a_time_not_understood_is_an_error_and_snoozes_nothing() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "snooze next tuesday");

    assert_eq!(effects, vec![]);
    assert_eq!(h.status(), "Snooze time not understood: \"next tuesday\"");
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn snooze_needs_a_time() {
    let mut h = Harness::loaded(store());

    command(&mut h, "snooze");

    assert_eq!(h.status(), "Usage: :snooze <time>");
}

#[test]
fn a_time_that_has_passed_snoozes_nothing() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "snooze 2026-10-01");

    assert_eq!(effects, vec![]);
    assert_eq!(h.status(), "Snooze time has passed: Thu 1 Oct 08:00");
}

#[test]
fn repeat_snoozes_at_the_same_time() {
    let mut h = Harness::loaded(store());
    let effects = command(&mut h, "z 2h");
    h.resolve(effects);
    h.app.now = at("2026-10-18T12:30:00Z");

    let effects = h.keys(".");
    h.resolve(effects);

    assert_eq!(
        h.app.schedule().get("t2").unwrap().return_at,
        at("2026-10-18T14:00:00Z")
    );
    assert_eq!(
        h.app.schedule().get("t2").unwrap().snoozed_at,
        at("2026-10-18T12:30:00Z")
    );
    assert_eq!(h.subjects(), ["Invoice"]);
}

#[test]
fn undo_of_a_snooze_moves_the_messages_back_and_deletes_the_entry() {
    let mut h = Harness::loaded(store());
    let effects = command(&mut h, "z tomorrow");
    h.resolve(effects);

    let effects = h.keys("u");

    assert!(
        matches!(
            &effects[..],
            [
                Effect::WriteSchedule { job: Some(2), .. },
                Effect::Replay { job: 2, .. }
            ]
        ),
        "{effects:?}"
    );
    h.resolve(effects);
    assert_eq!(h.app.schedule(), &Schedule::default());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Undone: snooze 1 thread");
}

#[test]
fn redo_of_a_snooze_records_the_entry_again() {
    let mut h = Harness::loaded(store());
    let effects = command(&mut h, "z tomorrow");
    h.resolve(effects);
    let effects = h.keys("u");
    h.resolve(effects);

    let effects = h.ctrl('r');
    h.resolve(effects);

    assert_eq!(
        h.app.schedule().get("t1").unwrap().return_at,
        at("2026-10-19T08:00:00Z")
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn a_failed_snooze_is_rolled_back_with_its_entry() {
    let mut h = Harness::loaded(store());
    command(&mut h, "z tomorrow");

    let effects = h.send(Msg::Sent {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert!(
        matches!(effects[0], Effect::WriteSchedule { job: None, .. }),
        "{effects:?}"
    );
    assert_eq!(h.app.schedule(), &Schedule::default());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Could not snooze 1 thread: connection reset");
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn a_schedule_write_that_fails_cancels_the_snooze() {
    let mut h = Harness::loaded(store());
    command(&mut h, "z tomorrow");

    let effects = h.send(Msg::ScheduleFailed {
        job: Some(1),
        error: "Permission denied".into(),
    });

    assert_eq!(effects, vec![]);
    assert_eq!(h.app.schedule(), &Schedule::default());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(
        h.status(),
        "Could not snooze 1 thread: could not write the snooze schedule: Permission denied"
    );
    assert_eq!(h.app.undo_depth(), 0);
    // A later answer for the cancelled job is ignored.
    assert_eq!(
        h.send(Msg::Sent {
            job: 1,
            result: Ok(SetResult::default())
        }),
        vec![]
    );
}

#[test]
fn snooze_in_the_snooze_mailbox_does_nothing() {
    let mut h = Harness::loaded(store());
    let effects = h.send(Msg::SetFilter("in:\"Snoozed (blorp)\"".into()));
    h.resolve(effects);

    let effects = command(&mut h, "z tomorrow");

    assert_eq!(effects, vec![]);
    assert_eq!(h.status(), "Already snoozed");
}

#[test]
fn dry_run_snoozes_in_memory_and_writes_nothing() {
    let mut h = Harness::loaded_dry(store());

    let effects = command(&mut h, "z tomorrow");

    assert_eq!(
        effects,
        vec![Effect::LogDryRun {
            step: Step::Act,
            action: Action::Snooze(at("2026-10-19T08:00:00Z")),
            thread_ids: vec!["t1".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "[dry] Snoozed 1 thread until Mon 08:00");
    assert!(h.app.schedule().get("t1").is_some());

    let effects = h.keys("D");
    assert_eq!(effects, vec![]);
    assert_eq!(h.app.schedule(), &Schedule::default());
}

#[test]
fn archiving_a_returned_thread_drops_its_entry_and_undo_restores_it() {
    let returned = Entry {
        emails: [("e2".into(), "mb-inbox".into())].into(),
        snoozed_at: at("2026-10-17T09:00:00Z"),
        return_at: at("2026-10-18T08:00:00Z"),
        returned_at: Some(at("2026-10-18T11:30:00Z")),
    };
    let mut schedule = Schedule::default();
    schedule.set("t2", Some(returned.clone()));
    let mut h = Harness::loaded_with(store(), schedule);
    assert_eq!(h.subjects(), ["Lunch", "Re: Trip", "Invoice"]);

    let effects = h.keys("e");
    assert!(
        matches!(effects[0], Effect::WriteSchedule { job: Some(1), .. }),
        "{effects:?}"
    );
    h.resolve(effects);
    assert_eq!(h.app.schedule().get("t2"), None);

    let effects = h.keys("u");
    h.resolve(effects);
    assert_eq!(h.app.schedule().get("t2"), Some(&returned));
    assert_eq!(h.subjects(), ["Lunch", "Re: Trip", "Invoice"]);
}

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn z_opens_the_picker_and_a_letter_snoozes_with_its_preset() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("z"), vec![]);
    let choices = h.app.snooze_picker().unwrap();
    assert_eq!((choices.index, choices.threads), (0, 1));
    assert_eq!(
        choices
            .presets
            .iter()
            .map(|(p, when)| format!("{} {} {when}", p.key(), p.label()))
            .collect::<Vec<_>>(),
        [
            "l Later today today 15:00",
            "t Tomorrow morning Mon 08:00",
            "w This weekend Sat 08:00",
            "n Next week Mon 08:00",
        ]
    );

    let effects = h.keys("w");
    h.resolve(effects);

    assert!(h.app.snooze_picker().is_none());
    assert_eq!(
        h.app.schedule().get("t1").unwrap().return_at,
        at("2026-10-24T08:00:00Z")
    );
    assert_eq!(h.status(), "Snoozed 1 thread until Sat 08:00");
}

#[test]
fn the_picker_moves_with_j_and_k_and_enter_chooses() {
    let mut h = Harness::loaded(store());
    h.keys("zjjjk");
    assert_eq!(h.app.snooze_picker().unwrap().index, 2);

    let effects = h.key(key(crossterm::event::KeyCode::Enter));
    h.resolve(effects);

    assert_eq!(
        h.app.schedule().get("t1").unwrap().return_at,
        at("2026-10-24T08:00:00Z")
    );
}

#[test]
fn esc_closes_the_picker_and_snoozes_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("Vjz");

    let effects = h.key(key(crossterm::event::KeyCode::Esc));

    assert_eq!(effects, vec![]);
    assert!(h.app.snooze_picker().is_none());
    assert!(h.app.queue().visual().is_none());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.keys("e").len(), 1, "keys act normally again");
}

#[test]
fn the_picker_snoozes_the_whole_selection_with_the_count_typed_before_z() {
    let mut h = Harness::loaded(store());

    h.keys("2z");
    let effects = h.keys("t");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Invoice"]);
    assert_eq!(h.status(), "Snoozed 2 threads until Mon 08:00");
}

#[test]
fn other_keys_at_the_picker_do_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("z");

    assert_eq!(h.keys("eq"), vec![]);

    assert!(h.app.snooze_picker().is_some());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn z_in_an_empty_queue_does_nothing() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
    );

    h.keys("z");

    assert!(h.app.snooze_picker().is_none());
}

#[test]
fn the_first_snooze_creates_the_snooze_mailbox_with_no_role() {
    let mut h = Harness::loaded(store_without_snooze_mailbox());

    let effects = command(&mut h, "z tomorrow");
    assert_eq!(
        effects,
        vec![Effect::CreateMailbox {
            job: 1,
            name: "Snoozed (blorp)".into(),
        }]
    );
    assert_eq!(h.status(), "Creating Snoozed (blorp)");
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);

    let effects = h.resolve(effects);

    let created = h
        .reconcile(&blorp::jmap::EmailQuery {
            filter: json!({"inMailbox": "mb-inbox"}),
            ascending: false,
            limit: 500,
            extra_threads: Vec::new(),
        })
        .unwrap()
        .mailboxes
        .into_iter()
        .find(|m| m.name == "Snoozed (blorp)")
        .unwrap();
    assert_eq!(created.role, None);
    assert_eq!(created.parent_id, None);
    let Some(Effect::SetEmails { update: sent, .. }) = effects.get(1) else {
        panic!("{effects:?}");
    };
    assert_eq!(
        sent["e1a"],
        serde_json::from_value(json!({
            "mailboxIds/mb-inbox": null,
            format!("mailboxIds/{}", created.id): true
        }))
        .unwrap()
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.status(), "Snoozed 1 thread until Mon 08:00");
}

#[test]
fn a_new_snooze_mailbox_leaves_all_mail() {
    let mut h = Harness::loaded(store_without_snooze_mailbox());
    let effects = h.send(Msg::SetFilter("in:any".into()));
    h.resolve(effects);

    let effects = command(&mut h, "z tomorrow");
    h.resolve(effects);
    let effects = h.send(Msg::Refresh);
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn a_failed_creation_snoozes_nothing() {
    let mut h = Harness::loaded(store_without_snooze_mailbox());
    command(&mut h, "z tomorrow");

    let effects = h.send(Msg::MailboxCreated {
        job: 1,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert_eq!(
        h.status(),
        "Could not create Snoozed (blorp): connection reset"
    );
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn a_snooze_mailbox_deleted_elsewhere_is_created_again() {
    let mut h = Harness::loaded(store());
    h.store_mut().remove_mailbox("mb-snooze");
    let effects = h.send(Msg::Refresh);
    h.resolve(effects);

    let effects = command(&mut h, "z tomorrow");

    assert!(
        matches!(effects[..], [Effect::CreateMailbox { .. }]),
        "{effects:?}"
    );
}

#[test]
fn fastmails_snoozed_mailbox_is_never_used() {
    let mut h = Harness::loaded(store_without_snooze_mailbox());

    let effects = command(&mut h, "z tomorrow");

    assert!(
        matches!(effects[..], [Effect::CreateMailbox { .. }]),
        "{effects:?}"
    );
}

#[test]
fn dry_run_snoozes_with_no_snooze_mailbox_and_creates_none() {
    let mut h = Harness::loaded_dry(store_without_snooze_mailbox());

    let effects = command(&mut h, "z tomorrow");

    assert!(
        matches!(effects[..], [Effect::LogDryRun { .. }]),
        "{effects:?}"
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);

    h.keys("D");
    let effects = command(&mut h, "z tomorrow");
    assert!(
        matches!(effects[..], [Effect::CreateMailbox { .. }]),
        "{effects:?}"
    );
}

/// A schedule holding t4, due at 08:00 on the harness's day, snoozed from
/// the Inbox.
fn due_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set(
        "t4",
        Some(Entry {
            emails: [("e4".into(), "mb-inbox".into())].into(),
            snoozed_at: at("2026-10-10T09:00:00Z"),
            return_at: at("2026-10-18T08:00:00Z"),
            returned_at: None,
        }),
    );
    schedule
}

#[test]
fn a_due_thread_returns_at_launch_unread_and_at_the_top() {
    let (h, effects) = Harness::launched(store(), false, due_schedule());

    let Effect::WriteSchedule {
        job: Some(1),
        contents,
        ..
    } = &effects[0]
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        schedule_of(contents)["threads"]["t4"]["returnedAt"],
        "2026-10-18T12:00:00+00:00"
    );
    assert_eq!(effects.len(), 1, "the Email/set was answered: {effects:?}");
    assert_eq!(
        h.app.schedule().get("t4").unwrap().returned_at,
        Some(at(NOW))
    );
    assert_eq!(h.subjects(), ["Renewal", "Re: Trip", "Lunch", "Invoice"]);
    let returned = &h.app.queue().threads()[0];
    assert!(returned.unread());
    assert!(returned.flagged());
    assert_eq!(h.status(), "Returned 1 thread from snooze");
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn the_return_is_one_email_set_per_thread_moving_the_snoozed_emails_back() {
    let mut schedule = due_schedule();
    schedule.set(
        "t3",
        Some(Entry {
            emails: [("e3".into(), "mb-travel".into())].into(),
            snoozed_at: at("2026-10-10T09:00:00Z"),
            return_at: at("2026-10-18T11:00:00Z"),
            returned_at: None,
        }),
    );
    let mut h = Harness::with_schedule(store(), schedule);
    // e3 is in the Inbox, not the snooze mailbox: there is nothing to
    // return, so its entry is dropped.
    let effects = h.start();
    let effects = h.resolve(effects);

    let Effect::QuerySnoozed { query } = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(query.filter, json!({"inMailbox": "mb-snooze"}));
    let effects = h.send(Msg::SnoozedLoaded {
        result: h.reconcile(query).map(|r| r.result),
    });

    let sets: Vec<&EmailUpdate> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::SetEmails { update, .. } => Some(update),
            _ => None,
        })
        .collect();
    assert_eq!(
        sets,
        [&update(&[(
            "e4",
            json!({
                "mailboxIds/mb-snooze": null,
                "mailboxIds/mb-inbox": true,
                "keywords/$seen": null
            })
        )])]
    );
    assert!(
        matches!(
            effects.last(),
            Some(Effect::WriteSchedule { job: None, .. })
        ),
        "{effects:?}"
    );
    assert_eq!(h.app.schedule().get("t3"), None);
}

#[test]
fn a_thread_not_yet_due_stays_snoozed() {
    let mut schedule = due_schedule();
    let mut entry = schedule.get("t4").unwrap().clone();
    entry.return_at = at("2026-10-18T12:00:01Z");
    schedule.set("t4", Some(entry.clone()));

    let (h, effects) = Harness::launched(store(), false, schedule);

    assert_eq!(effects, vec![]);
    assert_eq!(h.app.schedule().get("t4"), Some(&entry));
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn a_return_goes_to_the_inbox_when_its_mailbox_is_gone() {
    let mut schedule = due_schedule();
    let mut entry = schedule.get("t4").unwrap().clone();
    entry.emails = [("e4".into(), "mb-deleted".into())].into();
    schedule.set("t4", Some(entry));

    let (h, _) = Harness::launched(store(), false, schedule);

    assert_eq!(h.subjects(), ["Renewal", "Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn a_failed_return_puts_the_entry_back() {
    let mut h = Harness::with_schedule(store(), due_schedule());
    let effects = h.start();
    let effects = h.resolve(effects);
    let Effect::QuerySnoozed { query } = &effects[0] else {
        panic!("{effects:?}");
    };
    let result = h.reconcile(query).map(|r| r.result);
    let effects = h.send(Msg::SnoozedLoaded { result });
    let Some(Effect::SetEmails { job, .. }) = effects.get(1) else {
        panic!("{effects:?}");
    };

    let effects = h.send(Msg::Sent {
        job: *job,
        result: Err(Error::Transport("connection reset".into())),
    });

    assert!(
        matches!(effects[0], Effect::WriteSchedule { job: None, .. }),
        "{effects:?}"
    );
    assert_eq!(h.app.schedule(), &due_schedule());
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "Could not return 1 thread: connection reset");
}

#[test]
fn in_dry_run_a_due_thread_returns_in_memory_only() {
    let (h, effects) = Harness::launched(store(), true, due_schedule());

    assert_eq!(
        effects,
        vec![Effect::LogDryRun {
            step: Step::Act,
            action: Action::Return,
            thread_ids: vec!["t4".into()],
        }]
    );
    assert_eq!(h.subjects(), ["Renewal", "Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(h.status(), "[dry] Returned 1 thread from snooze");
}

#[test]
fn undo_of_an_action_on_a_returned_thread_puts_it_back_at_the_top() {
    let (mut h, _) = Harness::launched(store(), false, due_schedule());

    let effects = h.keys("e");
    h.resolve(effects);
    assert_eq!(h.app.schedule().get("t4"), None);
    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(
        h.app.schedule().get("t4").unwrap().returned_at,
        Some(at(NOW))
    );
    assert_eq!(h.subjects(), ["Renewal", "Re: Trip", "Lunch", "Invoice"]);
}

#[test]
fn completion_offers_the_presets_and_weekdays() {
    let mut h = Harness::loaded(store());
    h.keys(":snooze ");
    h.key(key(crossterm::event::KeyCode::Tab));

    let (candidates, _) = h.app.completion().unwrap();
    assert_eq!(
        candidates,
        [
            "later", "tomorrow", "weekend", "next", "mon", "tue", "wed", "thu", "fri", "sat", "sun"
        ]
    );

    h.key(key(crossterm::event::KeyCode::Esc));
    h.keys(":z t");
    h.key(key(crossterm::event::KeyCode::Tab));
    assert_eq!(h.app.completion().unwrap().0, ["tomorrow", "tue", "thu"]);
}

/// 500 Inbox threads newer than t1..t3, so the queue is full without them.
fn crowded() -> Store {
    let mut builder = builder().mailbox("mb-snooze", "Snoozed (blorp)");
    for i in 0..500 {
        let received = Timestamp::from_second(at("2026-10-18T11:59:59Z").as_second() - i).unwrap();
        builder = builder.email(
            EmailSpec::new(&format!("n{i}"), &format!("tn{i}"), &received.to_string())
                .in_mailbox("mb-inbox")
                .seen()
                .subject(&format!("New {i}")),
        );
    }
    builder.build()
}

/// A schedule in which t3 returned to the Inbox at 11:59:30.
fn returned_t3() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set(
        "t3",
        Some(Entry {
            emails: [("e3".into(), "mb-inbox".into())].into(),
            snoozed_at: at("2026-10-17T09:00:00Z"),
            return_at: at("2026-10-18T11:59:30Z"),
            returned_at: Some(at("2026-10-18T11:59:30Z")),
        }),
    );
    schedule
}

#[test]
fn a_returned_thread_beyond_the_queue_limit_still_comes_in_at_its_return_time() {
    let mut h = Harness::with_schedule(crowded(), returned_t3());

    let effects = h.start();
    let [Effect::Query { query, .. }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert_eq!(query.extra_threads, ["t3"]);
    h.settle(effects);

    let subjects = h.subjects();
    assert_eq!(subjects.len(), 501);
    assert_eq!(subjects[..32].iter().filter(|s| *s == "Invoice").count(), 1);
    assert_eq!(subjects[30], "Invoice", "after the 30 newer than 11:59:30");
    assert_eq!(h.app.total(), 503);
}

#[test]
fn a_returned_thread_in_the_queue_is_not_doubled() {
    let mut h = Harness::with_schedule(store(), returned_t3());

    let effects = h.start();
    h.settle(effects);

    assert_eq!(h.subjects(), ["Invoice", "Re: Trip", "Lunch"]);
}

#[test]
fn a_filter_with_more_than_a_mailbox_fetches_no_returned_thread() {
    let mut h = Harness::loaded_with(crowded(), returned_t3());

    let effects = h.send(Msg::SetFilter("is:unread".into()));

    let [Effect::Query { query, .. }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert!(query.extra_threads.is_empty());
}

#[test]
fn a_returned_thread_is_not_fetched_for_a_queue_it_did_not_return_to() {
    let mut h = Harness::loaded_with(store(), returned_t3());

    let effects = h.send(Msg::SetFilter("in:travel".into()));

    let [Effect::Query { query, .. }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert!(query.extra_threads.is_empty());
}

#[test]
fn a_returned_thread_that_leaves_its_queue_elsewhere_loses_its_entry() {
    let mut h = Harness::loaded_with(store(), returned_t3());
    let archive: EmailUpdate = update(&[(
        "e3",
        json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-archive": true}),
    )]);
    h.store_mut().set_emails(&archive);

    let effects = h.send(Msg::Refresh);
    let rest = h.settle(effects);

    assert_eq!(h.app.schedule().get("t3"), None);
    assert!(
        matches!(rest[..], [Effect::WriteSchedule { job: None, .. }]),
        "{rest:?}"
    );
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch"]);
}

#[test]
fn a_returned_thread_in_a_narrower_queue_keeps_its_entry() {
    let mut h = Harness::loaded_with(store(), returned_t3());

    let effects = h.send(Msg::SetFilter("subject:lunch".into()));
    let rest = h.settle(effects);

    assert_eq!(rest, vec![]);
    assert!(h.app.schedule().get("t3").is_some());
}

#[test]
fn a_due_thread_beyond_the_snooze_mailboxs_newest_500_still_returns() {
    let mut builder = builder().mailbox("mb-snooze", "Snoozed (blorp)").email(
        EmailSpec::new("e4", "t4", "2026-10-01T09:00:00Z")
            .in_mailbox("mb-snooze")
            .seen()
            .subject("Renewal"),
    );
    for i in 0..500 {
        builder = builder.email(
            EmailSpec::new(&format!("s{i}"), &format!("ts{i}"), "2026-10-17T09:00:00Z")
                .in_mailbox("mb-snooze")
                .subject("Snoozed later"),
        );
    }

    let (h, _) = Harness::launched(builder.build(), false, due_schedule());

    assert_eq!(h.subjects()[0], "Renewal");
    assert_eq!(
        h.app.schedule().get("t4").unwrap().returned_at,
        Some(at(NOW))
    );
}
