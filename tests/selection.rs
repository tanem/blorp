//! Actions apply to the selection: the visual range if one is open, else
//! the marked rows, else the count of rows from the cursor. `.` repeats
//! the last action.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store, StoreBuilder};
use blorp::jmap::{EmailPatch, EmailUpdate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

/// Five single-email Inbox threads, newest first: "One" to "Five".
fn builder() -> StoreBuilder {
    let mut store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .mailbox("mb-travel", "Travel");
    for (i, subject) in ["One", "Two", "Three", "Four", "Five"].iter().enumerate() {
        let n = i + 1;
        let received = format!("2026-10-18T1{}:00:00Z", 6 - n);
        store = store.email(
            EmailSpec::new(&format!("e{n}"), &format!("t{n}"), &received)
                .in_mailbox("mb-inbox")
                .subject(subject),
        );
    }
    store
}

fn store() -> Store {
    builder().build()
}

/// [`store`] and thread t6, "Six", oldest: its one Inbox email is also in
/// Sent, so archive moves nothing of it.
fn store_with_sent() -> Store {
    builder()
        .email(
            EmailSpec::new("e6", "t6", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .in_mailbox("mb-sent")
                .subject("Six"),
        )
        .build()
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
fn a_count_archives_that_many_rows_from_the_cursor_in_one_email_set() {
    let mut h = Harness::loaded(store());
    h.keys("j");

    let effects = h.keys("3e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e2", "e3", "e4"]),
        }]
    );
    assert_eq!(h.subjects(), ["One", "Five"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), "Archived 3 threads");
    assert_eq!(h.app.undo_depth(), 1);
    assert_eq!(h.resolve(effects), []);
    let effects = h.send(Msg::Show("mb-archive".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Two", "Three", "Four"]);
}

#[test]
fn x_marks_a_row_and_moves_down_and_e_archives_the_marked_threads() {
    let mut h = Harness::loaded(store());

    h.keys("xjx");
    assert_eq!(h.cursor(), 3);
    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e1", "e3"]),
        }]
    );
    assert_eq!(h.subjects(), ["Two", "Four", "Five"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.status(), "Archived 2 threads");
    assert_eq!(h.app.undo_depth(), 1);
}

#[test]
fn x_on_a_marked_row_unmarks_it() {
    let mut h = Harness::loaded(store());

    h.keys("xkx");
    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e2"]),
        }]
    );
}

#[test]
fn a_count_before_x_marks_that_many_rows() {
    let mut h = Harness::loaded(store());

    h.keys("3x");
    assert_eq!(h.cursor(), 3);
    h.keys("e");

    assert_eq!(h.subjects(), ["Four", "Five"]);
}

#[test]
fn shift_x_clears_the_marks() {
    let mut h = Harness::loaded(store());

    h.keys("xxX");
    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e3"]),
        }]
    );
}

#[test]
fn marks_on_threads_the_action_left_in_the_queue_stay() {
    let mut h = Harness::loaded(store_with_sent());
    h.keys("G");
    h.keys("xkkx");

    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e4"]),
        }]
    );
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Five", "Six"]);
    assert_eq!(h.status(), "Archived 1 thread");
    let effects = h.keys("e");
    assert_eq!(effects, []);
    assert_eq!(h.status(), "Nothing to archive");
}

fn esc(h: &mut Harness) {
    h.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

#[test]
fn shift_v_opens_a_visual_range_that_j_extends_and_e_archives() {
    let mut h = Harness::loaded(store());
    h.keys("j");

    h.keys("Vjj");
    assert_eq!(h.app.queue().visual(), Some(1..=3));
    let effects = h.keys("e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e2", "e3", "e4"]),
        }]
    );
    assert_eq!(h.subjects(), ["One", "Five"]);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.app.queue().visual(), None);
    assert_eq!(h.status(), "Archived 3 threads");
}

#[test]
fn k_extends_a_visual_range_upward() {
    let mut h = Harness::loaded(store());
    h.keys("G");

    h.keys("Vkk");

    assert_eq!(h.app.queue().visual(), Some(2..=4));
}

#[test]
fn a_visual_range_wins_over_marks_and_a_count() {
    let mut h = Harness::loaded(store());
    h.keys("xjVj");

    let effects = h.keys("3e");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: archive(&["e3", "e4"]),
        }]
    );
    assert!(h.app.queue().marked(0));
}

#[test]
fn esc_or_shift_v_leaves_the_visual_range() {
    for leave in ["esc", "V"] {
        let mut h = Harness::loaded(store());
        h.keys("Vj");

        match leave {
            "esc" => esc(&mut h),
            _ => drop(h.keys("V")),
        }
        let effects = h.keys("e");

        assert_eq!(h.app.queue().visual(), None, "{leave}");
        assert_eq!(
            effects,
            [Effect::SetEmails {
                job: 1,
                update: archive(&["e2"]),
            }],
            "{leave}"
        );
    }
}

#[test]
fn showing_a_queue_clears_the_marks_and_the_visual_range() {
    let mut h = Harness::loaded(store());
    h.keys("xjV");

    let effects = h.send(Msg::Show("mb-inbox".into()));
    h.resolve(effects);

    assert!(!h.app.queue().marked(0));
    assert_eq!(h.app.queue().visual(), None);
}

#[test]
fn undo_puts_the_threads_back_without_their_marks() {
    let mut h = Harness::loaded(store());
    h.keys("xx");
    let effects = h.keys("e");
    h.resolve(effects);

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four", "Five"]);
    assert!(!h.app.queue().marked(0));
    assert!(!h.app.queue().marked(1));
    assert_eq!(h.status(), "Undone: archive 2 threads");
}

#[test]
fn dot_repeats_the_last_action_on_the_row_under_the_cursor_as_its_own_entry() {
    let mut h = Harness::loaded(store());
    h.keys("e");

    let effects = h.keys(".");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 2,
            update: archive(&["e2"]),
        }]
    );
    assert_eq!(h.subjects(), ["Three", "Four", "Five"]);
    assert_eq!(h.status(), "Archived 1 thread");
    assert_eq!(h.app.undo_depth(), 2);
}

#[test]
fn dot_reuses_the_original_count() {
    let mut h = Harness::loaded(store());
    h.keys("2e");

    h.keys(".");

    assert_eq!(h.subjects(), ["Five"]);
    assert_eq!(h.status(), "Archived 2 threads");
}

#[test]
fn a_count_before_dot_replaces_the_original_count_for_later_repeats_too() {
    let mut h = Harness::loaded(store());
    h.keys("2e");

    h.keys("1.");
    assert_eq!(h.subjects(), ["Four", "Five"]);
    h.keys(".");

    assert_eq!(h.subjects(), ["Five"]);
}

#[test]
fn dot_acts_on_the_selection_as_it_is_when_pressed() {
    let mut h = Harness::loaded(store());
    h.keys("e");
    h.keys("jxx");

    let effects = h.keys(".");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 2,
            update: archive(&["e3", "e4"]),
        }]
    );
}

#[test]
fn dot_after_undo_repeats_the_action_not_the_undo() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("e");
    h.resolve(effects);
    let effects = h.keys("u");
    h.resolve(effects);

    let effects = h.keys(".");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 3,
            update: archive(&["e1"]),
        }]
    );
    assert_eq!(h.status(), "Archived 1 thread");
}

#[test]
fn dot_before_any_action_does_nothing() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("3.");

    assert_eq!(effects, []);
    assert_eq!(h.pending_count(), None);
    assert_eq!(h.subjects().len(), 5);
}
