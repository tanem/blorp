//! `d` deletes, `!` marks spam, and `m` and `:move` move the selection.

use blorp::app::Msg;
use blorp::app::{Effect, Mode, MoveChoices};
use blorp::harness::Harness;
use blorp::jmap::EmailUpdate;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::snooze::{Entry, Schedule};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

/// Thread t1 spreads over the Inbox, Travel, and Sent, with one email in
/// both the Inbox and Sent. Threads t2 and t3 are single Inbox emails.
/// Newest first: t1, t2, t3. Trash holds t4, and t5 has one email in Junk
/// and one in the Inbox.
fn builder() -> blorp::jmap::fake::StoreBuilder {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-drafts", "Drafts", "drafts")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-junk", "Junk", "junk")
        .mailbox("mb-travel", "Travel")
        .mailbox("mb-work", "Work")
        .child_mailbox("mb-work-receipts", "Receipts", "mb-work")
        .mailbox("mb-home", "Home")
        .child_mailbox("mb-home-receipts", "Receipts", "mb-home")
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
        .email(
            EmailSpec::new("e1d", "t1", "2026-10-18T11:00:00Z")
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
            EmailSpec::new("e4", "t4", "2026-10-17T09:00:00Z")
                .in_mailbox("mb-trash")
                .subject("Old news"),
        )
        .email(
            EmailSpec::new("e5a", "t5", "2026-10-16T09:00:00Z")
                .in_mailbox("mb-junk")
                .subject("Prize"),
        )
        .email(
            EmailSpec::new("e5b", "t5", "2026-10-16T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Re: Prize"),
        )
}

/// The store [`builder`] describes.
fn store() -> Store {
    builder().build()
}

/// The patch moving each of `ids` out of `from` into `to`.
fn moved(ids: &[&str], from: &str, to: &str) -> EmailUpdate {
    ids.iter()
        .map(|id| {
            let patch = [
                (format!("mailboxIds/{from}"), Value::Null),
                (format!("mailboxIds/{to}"), Value::Bool(true)),
            ]
            .into();
            (id.to_string(), patch)
        })
        .collect()
}

/// The update of each `Email/set` among `effects`.
fn sets(effects: &[Effect]) -> Vec<&EmailUpdate> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::SetEmails { update, .. } => Some(update),
            _ => None,
        })
        .collect()
}

#[test]
fn d_moves_only_the_threads_queue_mailbox_messages_to_trash() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("d");

    assert_eq!(
        sets(&effects),
        [&moved(&["e1a", "e1d"], "mb-inbox", "mb-trash")]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice", "Re: Prize"]);
    assert_eq!(h.status(), "Deleted 1 thread");
}

#[test]
fn bang_moves_only_the_threads_queue_mailbox_messages_to_junk() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("j!");

    assert_eq!(sets(&effects), [&moved(&["e2"], "mb-inbox", "mb-junk")]);
    assert_eq!(h.subjects(), ["Re: Trip", "Invoice", "Re: Prize"]);
    assert_eq!(h.status(), "Marked 1 thread as spam");
}

/// Shows the queue `filter` selects.
fn open(h: &mut Harness, filter: &str) {
    let effects = h.send(Msg::SetFilter(filter.into()));
    h.settle(effects);
}

#[test]
fn d_and_bang_are_disabled_in_trash_and_junk() {
    let mut h = Harness::loaded(store());
    for (filter, rows) in [("in:trash", ["Old news"]), ("in:junk", ["Prize"])] {
        open(&mut h, filter);
        let name = &filter[3..];
        let name = format!("{}{}", name[..1].to_uppercase(), &name[1..]);

        assert_eq!(h.keys("d"), []);
        assert_eq!(h.status(), format!("Delete is disabled in {name}"));
        assert_eq!(h.keys("!"), []);
        assert_eq!(h.status(), format!("Spam is disabled in {name}"));
        assert_eq!(h.subjects(), rows);
        assert_eq!(h.app.undo_depth(), 0);
    }
}

#[test]
fn in_junk_e_and_move_take_only_the_messages_in_junk() {
    let mut h = Harness::loaded(store());
    open(&mut h, "in:junk");

    let effects = h.keys("e");

    assert_eq!(sets(&effects), [&moved(&["e5a"], "mb-junk", "mb-archive")]);
    assert_eq!(h.subjects(), Vec::<String>::new());

    let effects = h.keys("u");
    h.settle(effects);
    assert_eq!(h.subjects(), ["Prize"]);
    let effects = command(&mut h, "move travel");

    assert_eq!(sets(&effects), [&moved(&["e5a"], "mb-junk", "mb-travel")]);
}

/// Types `:`, then `text`, then Enter.
fn command(h: &mut Harness, text: &str) -> Vec<Effect> {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(h.send(Msg::Prompt(blorp::prompt::Key::Enter)));
    effects
}

#[test]
fn colon_move_moves_to_the_mailbox_the_rest_of_the_line_names() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "move work/receipts");

    assert_eq!(
        sets(&effects),
        [&moved(&["e1a", "e1d"], "mb-inbox", "mb-work-receipts")]
    );
    assert_eq!(h.subjects(), ["Lunch", "Invoice", "Re: Prize"]);
    assert_eq!(h.status(), "Moved 1 thread to Work/Receipts");

    let effects = command(&mut h, "m Travel");

    assert_eq!(sets(&effects), [&moved(&["e2"], "mb-inbox", "mb-travel")]);
    assert_eq!(h.status(), "Moved 1 thread to Travel");
}

#[test]
fn colon_move_reports_an_unknown_or_ambiguous_name_and_never_opens_the_picker() {
    let mut h = Harness::loaded(store());

    for (text, status) in [
        ("move Nowhere", "No mailbox matches \"Nowhere\""),
        (
            "move Receipts",
            "Mailbox name \"Receipts\" is ambiguous: Work/Receipts, Home/Receipts",
        ),
        ("move", "Usage: :move <mailbox>"),
        ("m", "Usage: :move <mailbox>"),
        ("move inbox", "Already in Inbox"),
        ("move Drafts", "Cannot move to Drafts"),
        ("move sent", "Cannot move to Sent"),
    ] {
        assert_eq!(command(&mut h, text), [], "{text}");
        assert_eq!(h.status(), status, "{text}");
        assert_eq!(h.app.move_picker(), None, "{text}");
    }
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice", "Re: Prize"]);
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn colon_move_on_an_empty_queue_says_nothing_is_selected_before_resolving_the_name() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
    );

    assert_eq!(command(&mut h, "move Nowhere"), []);
    assert_eq!(h.status(), "Nothing selected");
}

#[test]
fn colon_move_to_the_snooze_mailbox_says_to_use_z_whether_or_not_it_exists() {
    let without = store();
    let with = builder().mailbox("mb-snooze", "Snoozed (blorp)").build();
    for store in [without, with] {
        let mut h = Harness::loaded(store);

        assert_eq!(command(&mut h, "move Snoozed (blorp)"), []);
        assert_eq!(h.status(), "Use z to snooze");
        assert_eq!(command(&mut h, "move snoozed (BLORP)"), []);
        assert_eq!(h.status(), "Use z to snooze");
        assert_eq!(h.app.undo_depth(), 0);
    }
}

#[test]
fn colon_delete_and_colon_spam_act_on_the_selection_with_their_aliases() {
    let mut h = Harness::loaded(store());

    let effects = command(&mut h, "delete");
    assert_eq!(
        sets(&effects),
        [&moved(&["e1a", "e1d"], "mb-inbox", "mb-trash")]
    );
    let effects = command(&mut h, "d");
    assert_eq!(sets(&effects), [&moved(&["e2"], "mb-inbox", "mb-trash")]);
    let effects = h.keys("2");
    assert_eq!(effects, []);
    let effects = command(&mut h, "spam");
    assert_eq!(
        sets(&effects),
        [&moved(&["e3", "e5b"], "mb-inbox", "mb-junk")]
    );
    assert_eq!(h.status(), "Marked 2 threads as spam");
}

/// Presses `code` with no modifier.
fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    h.key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Types each character of `keys` as a key press, `\n` as Enter.
fn type_keys(h: &mut Harness, keys: &str) -> Vec<Effect> {
    let mut effects = Vec::new();
    for c in keys.chars() {
        effects.extend(match c {
            '\n' => press(h, KeyCode::Enter),
            c => h.keys(&c.to_string()),
        });
    }
    effects
}

#[test]
fn m_opens_the_picker_on_every_mailbox_but_drafts_sent_and_the_snooze_mailbox() {
    let mut h = Harness::loaded(builder().mailbox("mb-snooze", "Snoozed (blorp)").build());

    assert_eq!(h.keys("m"), []);

    assert_eq!(
        h.app.move_picker(),
        Some(MoveChoices {
            typed: String::new(),
            paths: [
                "Archive",
                "Home",
                "Home/Receipts",
                "Inbox",
                "Junk",
                "Trash",
                "Travel",
                "Work",
                "Work/Receipts",
            ]
            .map(String::from)
            .into(),
            index: 0,
            threads: 1,
        })
    );
}

#[test]
fn typing_at_the_picker_filters_the_mailboxes_and_the_keys_move_through_them() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    h.keys("rEc");
    let picker = h.app.move_picker().unwrap();
    assert_eq!(picker.typed, "rEc");
    assert_eq!(picker.paths, ["Home/Receipts", "Work/Receipts"]);

    h.ctrl('n');
    assert_eq!(h.app.move_picker().unwrap().index, 1);
    h.ctrl('n');
    assert_eq!(h.app.move_picker().unwrap().index, 0, "it wraps");
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.move_picker().unwrap().index, 1);
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.move_picker().unwrap().index, 0);
    h.ctrl('p');
    assert_eq!(h.app.move_picker().unwrap().index, 1);

    press(&mut h, KeyCode::Backspace);
    press(&mut h, KeyCode::Backspace);
    let picker = h.app.move_picker().unwrap();
    assert_eq!(picker.typed, "r");
    assert_eq!(picker.index, 0, "an edit goes back to the first match");
    assert_eq!(
        picker.paths,
        [
            "Archive",
            "Home/Receipts",
            "Trash",
            "Travel",
            "Work",
            "Work/Receipts"
        ]
    );

    h.keys("jk");
    assert_eq!(
        h.app.move_picker().unwrap().typed,
        "rjk",
        "j and k are typed"
    );
    assert_eq!(h.app.move_picker().unwrap().paths, Vec::<String>::new());
    assert_eq!(press(&mut h, KeyCode::Enter), [], "nothing to choose");
    assert!(h.app.move_picker().is_some());
}

#[test]
fn enter_at_the_picker_moves_the_whole_selection_and_reports_it() {
    let mut h = Harness::loaded(store());
    h.keys("Vjm");
    assert_eq!(h.app.move_picker().unwrap().threads, 2);
    h.keys("work");
    h.ctrl('n');

    let effects = press(&mut h, KeyCode::Enter);

    let mut update = moved(&["e1a", "e1d"], "mb-inbox", "mb-work-receipts");
    update.extend(moved(&["e2"], "mb-inbox", "mb-work-receipts"));
    assert_eq!(sets(&effects), [&update]);
    assert_eq!(h.app.move_picker(), None);
    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.subjects(), ["Invoice", "Re: Prize"]);
    assert_eq!(h.status(), "Moved 2 threads to Work/Receipts");
}

#[test]
fn esc_at_the_picker_closes_it_and_the_visual_range_and_moves_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("Vjmtra");

    assert_eq!(press(&mut h, KeyCode::Esc), []);

    assert_eq!(h.app.move_picker(), None);
    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice", "Re: Prize"]);
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn a_count_before_m_moves_that_many_threads() {
    let mut h = Harness::loaded(store());
    h.keys("2m");
    assert_eq!(h.app.move_picker().unwrap().threads, 2);
    h.keys("travel");

    let effects = press(&mut h, KeyCode::Enter);

    let mut update = moved(&["e1a", "e1d"], "mb-inbox", "mb-travel");
    update.extend(moved(&["e2"], "mb-inbox", "mb-travel"));
    assert_eq!(sets(&effects), [&update]);
}

#[test]
fn m_on_an_empty_queue_opens_nothing() {
    let mut h = Harness::loaded(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
    );

    assert_eq!(h.keys("m"), []);
    assert_eq!(h.app.move_picker(), None);
}

#[test]
fn every_move_is_undone_and_redone() {
    for (keys, to) in [
        ("d", "mb-trash"),
        ("!", "mb-junk"),
        ("mtravel\n", "mb-travel"),
    ] {
        let mut h = Harness::loaded(store());
        let effects = type_keys(&mut h, keys);
        h.settle(effects);
        assert_eq!(h.subjects(), ["Lunch", "Invoice", "Re: Prize"], "{keys}");

        let effects = h.keys("u");
        h.settle(effects);
        assert_eq!(
            h.subjects(),
            ["Re: Trip", "Lunch", "Invoice", "Re: Prize"],
            "{keys}"
        );
        assert_eq!(h.app.undo_depth(), 0, "{keys}");

        let effects = h.ctrl('r');
        assert_eq!(
            sets(&effects),
            [&moved(&["e1a", "e1d"], "mb-inbox", to)],
            "{keys}"
        );
        h.settle(effects);
        assert_eq!(h.subjects(), ["Lunch", "Invoice", "Re: Prize"], "{keys}");
    }
}

#[test]
fn dot_repeats_a_move_to_the_same_mailbox() {
    let mut h = Harness::loaded(store());
    h.keys("mtravel");
    let effects = press(&mut h, KeyCode::Enter);
    h.settle(effects);

    let effects = h.keys(".");

    assert_eq!(sets(&effects), [&moved(&["e2"], "mb-inbox", "mb-travel")]);
    assert_eq!(h.status(), "Moved 1 thread to Travel");
    h.settle(effects);

    let effects = h.keys("2.");

    let mut update = moved(&["e3"], "mb-inbox", "mb-travel");
    update.extend(moved(&["e5b"], "mb-inbox", "mb-travel"));
    assert_eq!(sets(&effects), [&update]);
    assert_eq!(h.app.undo_depth(), 3);
}

#[test]
fn dot_repeats_delete_and_spam() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("d");
    h.settle(effects);
    let effects = h.keys(".");
    assert_eq!(sets(&effects), [&moved(&["e2"], "mb-inbox", "mb-trash")]);
    h.settle(effects);
    let effects = h.keys("!");
    h.settle(effects);

    let effects = h.keys(".");

    assert_eq!(sets(&effects), [&moved(&["e5b"], "mb-inbox", "mb-junk")]);
}

#[test]
fn delete_spam_and_move_in_the_snooze_mailbox_cancel_the_entry_and_undo_restores_it() {
    let store = || {
        builder()
            .mailbox("mb-snooze", "Snoozed (blorp)")
            .email(
                EmailSpec::new("e6", "t6", "2026-10-15T09:00:00Z")
                    .in_mailbox("mb-snooze")
                    .subject("Renewal"),
            )
            .build()
    };
    let mut schedule = Schedule::default();
    schedule.set(
        "t6",
        Some(Entry {
            emails: [("e6".into(), "mb-inbox".into())].into(),
            snoozed_at: "2026-10-16T09:00:00Z".parse().unwrap(),
            return_at: "2026-10-20T08:00:00Z".parse().unwrap(),
            returned_at: None,
        }),
    );
    for (keys, to) in [
        ("d", "mb-trash"),
        ("!", "mb-junk"),
        ("mwork/\n", "mb-work-receipts"),
    ] {
        let (mut h, _) = Harness::launched(store(), false, schedule.clone());
        open(&mut h, "in:\"Snoozed (blorp)\"");
        let effects = type_keys(&mut h, keys);

        assert_eq!(sets(&effects), [&moved(&["e6"], "mb-snooze", to)], "{keys}");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::WriteSchedule { .. })),
            "{keys}"
        );
        h.settle(effects);
        assert_eq!(h.app.schedule().get("t6"), None, "{keys}");
        assert_eq!(h.subjects(), Vec::<String>::new(), "{keys}");

        let effects = h.keys("u");
        h.settle(effects);

        assert_eq!(h.app.schedule(), &schedule, "{keys}");
        assert_eq!(h.subjects(), ["Renewal"], "{keys}");
    }
}

#[test]
fn a_thread_restored_into_another_queue_stands_for_itself_by_an_email_there() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-sent", "Sent", "sent")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Plan"),
        )
        .email(
            EmailSpec::new("e1b", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .in_mailbox("mb-sent")
                .subject("Re: Plan"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Flight"),
        )
        .build();
    let mut h = Harness::loaded(store);
    assert_eq!(h.subjects(), ["Re: Plan"]);
    let effects = command(&mut h, "move travel");
    assert_eq!(sets(&effects), [&moved(&["e1a"], "mb-inbox", "mb-travel")]);
    h.settle(effects);
    open(&mut h, "in:travel");
    assert_eq!(h.subjects(), ["Flight", "Plan"]);
    let effects = h.keys("u");
    h.settle(effects);
    assert_eq!(h.subjects(), ["Flight"]);

    let effects = h.ctrl('r');
    h.settle(effects);

    assert_eq!(h.subjects(), ["Flight", "Plan"]);
}
