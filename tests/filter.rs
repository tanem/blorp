//! `/` selects the queue with the filter language, and `t` flips the sort.

use blorp::app::{Effect, Sort};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailQuery, EmailUpdate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

/// Newest first: Lunch (Inbox, unread), Flight (Travel), Invoice (Inbox),
/// Old news (Trash), Receipt (Archive), Draft (Drafts).
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-drafts", "Drafts", "drafts")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-travel")
                .seen()
                .from("Air", "bookings@air.example.invalid")
                .subject("Flight"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .from("Ravi Shah", "ravi@example.invalid")
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-trash")
                .subject("Old news"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-18T07:00:00Z")
                .in_mailbox("mb-archive")
                .seen()
                .subject("Receipt"),
        )
        .email(
            EmailSpec::new("e6", "t6", "2026-10-18T06:00:00Z")
                .in_mailbox("mb-drafts")
                .subject("Draft"),
        )
        .build()
}

fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    h.key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Types `text` at the prompt and presses Enter, answering every query.
fn filter(h: &mut Harness, text: &str) {
    let mut effects = h.keys("/");
    effects.extend(h.ctrl('u'));
    effects.extend(h.keys(text));
    effects.extend(press(h, KeyCode::Enter));
    let rest = h.resolve(effects);
    assert_eq!(rest, [], "{text:?}");
}

fn queries(effects: &[Effect]) -> Vec<&EmailQuery> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Query { query, .. } => Some(query),
            _ => None,
        })
        .collect()
}

#[test]
fn a_filter_typed_at_the_prompt_selects_the_queue() {
    let mut h = Harness::loaded(store());

    filter(&mut h, "is:unread");

    assert_eq!(h.subjects(), ["Lunch"]);
    assert_eq!(h.app.filter(), "is:unread");
    assert_eq!(h.app.prompt(), None);
}

#[test]
fn the_queue_follows_the_filter_as_it_is_typed() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("/in:Travel");
    let last = queries(&effects).last().cloned().cloned();
    h.resolve(effects);

    assert_eq!(h.app.prompt(), Some("in:Travel"));
    assert_eq!(h.subjects(), ["Flight"]);
    assert_eq!(
        last,
        Some(EmailQuery {
            filter: json!({"inMailbox": "mb-travel"}),
            ascending: false,
            limit: 500,
            extra_threads: Vec::new(),
        })
    );
}

#[test]
fn slash_opens_the_prompt_with_the_current_filter() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "in:travel");

    let effects = h.keys("/");

    assert_eq!(h.app.prompt(), Some("in:travel"));
    assert_eq!(effects, []);
}

#[test]
fn enter_on_the_current_filter_sends_no_query() {
    let mut h = Harness::loaded(store());

    let mut effects = h.keys("/");
    effects.extend(press(&mut h, KeyCode::Enter));

    assert_eq!(effects, []);
    assert_eq!(h.app.prompt(), None);
}

#[test]
fn esc_restores_the_previous_queue_with_its_cursor_and_marks() {
    let mut h = Harness::loaded(store());
    h.keys("jx");
    let effects = h.keys("/in:Travel");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Flight"]);

    let effects = press(&mut h, KeyCode::Esc);

    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.app.filter(), "");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert!(h.app.queue().marked(1));
    assert_eq!(
        queries(&effects),
        [&EmailQuery {
            filter: json!({"inMailbox": "mb-inbox"}),
            ascending: false,
            limit: 500,
            extra_threads: Vec::new(),
        }],
        "the previous queue is fetched again"
    );
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert!(h.app.queue().marked(1));
}

#[test]
fn esc_after_typing_a_change_and_taking_it_back_restores_the_cursor_and_marks() {
    let mut h = Harness::loaded(store());
    h.keys("jx");
    let mut effects = h.keys("/x");
    effects.extend(press(&mut h, KeyCode::Backspace));
    h.resolve(effects);

    let effects = press(&mut h, KeyCode::Esc);
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
    assert!(h.app.queue().marked(1));
}

#[test]
fn esc_before_the_queue_changed_sends_no_query() {
    let mut h = Harness::loaded(store());

    let mut effects = h.keys("/\"Lun");
    effects.extend(press(&mut h, KeyCode::Esc));

    assert_eq!(effects, []);
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn an_empty_filter_means_the_inbox() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "in:travel");

    filter(&mut h, "");

    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.app.filter(), "");
    assert_eq!(h.app.queue_name(), "Inbox");
}

#[test]
fn an_invalid_filter_typed_from_the_start_sends_no_query() {
    let mut h = Harness::loaded(store());

    let mut effects = h.keys("/\"Ana");
    effects.extend(press(&mut h, KeyCode::Enter));

    assert_eq!(effects, []);
    assert_eq!(h.status(), "Unbalanced quote");
}

#[test]
fn an_invalid_filter_leaves_the_queue_unchanged_and_shows_the_error() {
    for (text, error) in [
        ("in:Nowhere", "No mailbox matches \"Nowhere\""),
        ("is:spam", "Unknown filter token \"is:spam\""),
        ("from:\"Ana", "Unbalanced quote"),
        ("in:drafts", "Drafts cannot be a queue"),
    ] {
        let mut h = Harness::loaded(store());
        h.keys("j");

        let mut effects = h.keys("/");
        effects.extend(h.keys(text));
        effects.extend(press(&mut h, KeyCode::Enter));
        h.resolve(effects);

        assert_eq!(h.status(), error);
        assert_eq!(h.app.prompt(), None);
        assert_eq!(h.app.filter(), "");
        assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
        assert_eq!(h.cursor(), 1);
    }
}

#[test]
fn enter_on_an_invalid_filter_after_a_valid_one_restores_the_previous_queue() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("/in:travel");
    h.resolve(effects);

    let mut effects = h.keys(" \"");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);

    assert_eq!(h.status(), "Unbalanced quote");
    assert_eq!(h.app.filter(), "");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn changing_the_filter_clears_the_marks_and_puts_the_cursor_on_the_first_row() {
    let mut h = Harness::loaded(store());
    h.keys("xx");
    filter(&mut h, "in:any");
    h.keys("2jx");

    filter(&mut h, "-is:unread");

    assert_eq!(h.subjects(), ["Invoice"]);
    assert_eq!(h.cursor(), 0);
    assert!(!h.app.queue().marked(0));
}

#[test]
fn in_any_covers_every_mailbox_but_trash_junk_drafts_and_sent() {
    let mut h = Harness::loaded(store());

    filter(&mut h, "in:any");

    assert_eq!(h.subjects(), ["Lunch", "Flight", "Invoice", "Receipt"]);
    assert_eq!(h.app.queue_name(), "All mail");
}

fn archive_patch(from: &str) -> EmailPatch {
    [
        (format!("mailboxIds/{from}"), Value::Null),
        ("mailboxIds/mb-archive".to_string(), Value::Bool(true)),
    ]
    .into()
}

#[test]
fn an_action_uses_the_queue_mailbox_of_the_current_filter() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "in:travel");

    let effects = h.keys("e");

    assert!(
        effects.contains(&Effect::SetEmails {
            job: 1,
            update: EmailUpdate::from([("e2".to_string(), archive_patch("mb-travel"))]),
        }),
        "{effects:?}"
    );
    assert_eq!(h.subjects(), Vec::<String>::new());
}

#[test]
fn an_archive_in_any_moves_each_message_out_of_the_mailboxes_it_is_in() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "in:any");

    let effects = h.keys("3e");

    assert!(
        effects.contains(&Effect::SetEmails {
            job: 1,
            update: EmailUpdate::from([
                ("e1".to_string(), archive_patch("mb-inbox")),
                ("e2".to_string(), archive_patch("mb-travel")),
                ("e3".to_string(), archive_patch("mb-inbox")),
            ]),
        }),
        "{effects:?}"
    );
    assert_eq!(h.status(), "Archived 3 threads");
    assert_eq!(
        h.subjects(),
        ["Lunch", "Flight", "Invoice", "Receipt"],
        "Archive is part of in:any, so the threads stay"
    );
}

#[test]
fn up_and_down_walk_the_filter_history_filtered_by_the_typed_text() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "is:unread");
    filter(&mut h, "in:travel");
    filter(&mut h, "is:read");
    h.keys("/");
    h.ctrl('u');

    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:read"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("in:travel"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"), "no older entry");
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.prompt(), Some("in:travel"));
    press(&mut h, KeyCode::Down);
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.prompt(), Some(""), "back to the typed text");

    h.keys("is:");
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:read"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
    press(&mut h, KeyCode::Down);
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.prompt(), Some("is:"));
}

#[test]
fn a_failed_filter_stays_in_history_and_a_repeated_one_moves_to_the_end() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "in:travel");
    filter(&mut h, "is:unread");
    filter(&mut h, "in:travel");
    let mut effects = h.keys("/");
    effects.extend(h.ctrl('u'));
    effects.extend(h.keys("in:Nowhere"));
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);
    h.keys("/");
    h.ctrl('u');

    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("in:Nowhere"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("in:travel"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
}

#[test]
fn history_keeps_a_filter_once_whatever_spaces_surround_it() {
    let mut h = Harness::loaded(store());
    filter(&mut h, "is:unread");
    filter(&mut h, "  in:travel ");
    filter(&mut h, "in:travel");
    h.keys("/");
    h.ctrl('u');

    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("in:travel"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("is:unread"));
}

#[test]
fn slash_clears_an_earlier_error_from_the_status_line() {
    let mut h = Harness::loaded(store());
    let mut effects = h.keys("/\"");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);
    assert_eq!(h.status(), "Unbalanced quote");

    h.keys("/");
    press(&mut h, KeyCode::Esc);

    assert_eq!(h.status(), "");
}

#[test]
fn keys_at_the_prompt_are_text_and_ctrl_c_still_quits() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("/qejx");

    assert!(
        effects.iter().all(|e| matches!(e, Effect::Query { .. })),
        "{effects:?}"
    );
    assert_eq!(h.app.prompt(), Some("qejx"));
    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

#[test]
fn slash_drops_a_pending_count() {
    let mut h = Harness::loaded(store());

    h.keys("3/");

    assert_eq!(h.pending_count(), None);
}

#[test]
fn t_toggles_newest_and_oldest_first() {
    let mut h = Harness::loaded(store());
    assert_eq!(h.app.sort(), Sort::Newest);

    let effects = h.keys("t");

    assert_eq!(h.app.sort(), Sort::Oldest);
    assert_eq!(
        queries(&effects),
        [&EmailQuery {
            filter: json!({"inMailbox": "mb-inbox"}),
            ascending: true,
            limit: 500,
            extra_threads: Vec::new(),
        }]
    );
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Invoice", "Lunch"]);

    let effects = h.keys("t");
    h.resolve(effects);
    assert_eq!(h.app.sort(), Sort::Newest);
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn t_clears_the_marks_and_puts_the_cursor_on_the_first_row() {
    let mut h = Harness::loaded(store());
    h.keys("jx");

    let effects = h.keys("t");
    h.resolve(effects);

    assert_eq!(h.cursor(), 0);
    assert!(!h.app.queue().marked(0) && !h.app.queue().marked(1));
}

#[test]
fn the_sort_carries_over_to_a_new_filter() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("t");
    h.resolve(effects);

    filter(&mut h, "in:any");

    assert_eq!(h.subjects(), ["Receipt", "Invoice", "Flight", "Lunch"]);
}

#[test]
fn an_archive_undone_in_an_oldest_first_queue_goes_back_in_order() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("t");
    h.resolve(effects);
    let effects = h.keys("e");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Lunch"]);

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Invoice", "Lunch"]);
}
