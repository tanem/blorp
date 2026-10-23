//! `/` searches the queue, narrowing it to the threads whose row's sender
//! or subject holds the typed text.

use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{Error, SetResult};
use crossterm::event::KeyCode;

/// Newest first, the Inbox holds Lunch from Ana (unread, with an older
/// email from Acme Support in Archive), Invoice from Acme Billing
/// (unread), News from news@acme.example.invalid (read), and the ACME
/// offsite from Kim (read). Travel holds Flight from Acme Air.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-17T11:00:00Z")
                .in_mailbox("mb-archive")
                .seen()
                .from("Acme Support", "support@acme.example.invalid")
                .subject("Acme lunch"),
        )
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Acme Billing", "billing@example.invalid")
                .subject("Invoice"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .from("", "news@acme.example.invalid")
                .subject("News"),
        )
        .email(
            EmailSpec::new("e4", "t4", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .from("Kim Park", "kim@example.invalid")
                .subject("Re: ACME offsite"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-17T10:00:00Z")
                .in_mailbox("mb-travel")
                .from("Acme Air", "air@example.invalid")
                .subject("Flight"),
        )
        .build()
}

/// Types `search` at the prompt and locks it in with Enter.
fn search(h: &mut Harness, search: &str) {
    h.keys("/");
    h.keys(search);
    h.press(KeyCode::Enter);
}

/// Toggles unread only and lets the reload land.
fn toggle_unread_only(h: &mut Harness) {
    let effects = h.keys("u");
    assert_eq!(h.resolve(effects), []);
}

/// Runs a reconcile on a push event and answers it from the store.
fn reconcile(h: &mut Harness) {
    let effects = h.send(Msg::Wake);
    assert!(
        matches!(effects[..], [Effect::Reconcile { .. }]),
        "{effects:?}"
    );
    assert_eq!(h.resolve(effects), []);
}

#[test]
fn slash_opens_the_prompt_and_the_queue_narrows_on_each_key() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("/"), []);
    assert_eq!(h.app.prompt(), Some(""));
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );

    assert_eq!(h.keys("a"), []);
    assert_eq!(h.app.prompt(), Some("a"));
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );

    assert_eq!(h.keys("cMe"), []);
    assert_eq!(h.app.prompt(), Some("acMe"));
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
}

#[test]
fn keys_typed_at_the_prompt_go_into_the_text_and_do_not_act() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("/jkermuoq"), []);
    assert_eq!(h.app.prompt(), Some("jkermuoq"));
    assert_eq!(h.status(), "");
    assert!(!h.app.unread_only());
    assert!(h.app.mailbox_list().is_none());
}

#[test]
fn down_and_up_do_not_move_the_cursor_at_the_prompt() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");

    assert_eq!(h.press(KeyCode::Down), []);
    assert_eq!(h.press(KeyCode::Up), []);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.app.prompt(), Some("acme"));
}

#[test]
fn backspace_deletes_and_widens_the_queue() {
    let mut h = Harness::loaded(store());
    h.keys("/news");
    assert_eq!(h.subjects(), ["News"]);

    h.press(KeyCode::Backspace);
    h.press(KeyCode::Backspace);

    assert_eq!(h.app.prompt(), Some("ne"));
    assert_eq!(h.subjects(), ["News"]);
    h.press(KeyCode::Backspace);
    assert_eq!(h.subjects(), ["Lunch", "Invoice", "News"]);
}

#[test]
fn the_sender_is_matched_as_the_row_shows_it_and_only_for_the_email_that_stands_for_the_thread() {
    let mut h = Harness::loaded(store());

    h.keys("/billing");
    assert_eq!(h.subjects(), ["Invoice"], "a name hides the address");

    h.press(KeyCode::Esc);
    h.keys("/news@");
    assert_eq!(h.subjects(), ["News"], "the address stands in for no name");

    h.press(KeyCode::Esc);
    h.keys("/support");
    assert_eq!(h.subjects(), Vec::<String>::new(), "only the row's email");
}

#[test]
fn the_whole_text_is_matched_as_one_string() {
    let mut h = Harness::loaded(store());

    h.keys("/acme off");
    assert_eq!(h.subjects(), ["Re: ACME offsite"]);

    h.keys("x");
    assert_eq!(h.subjects(), Vec::<String>::new());
}

#[test]
fn enter_locks_the_search_in_and_closes_the_prompt() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");

    assert_eq!(h.press(KeyCode::Enter), []);

    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.app.search(), "acme");
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
    h.keys("j");
    assert_eq!(h.cursor(), 1, "keys act again");
}

#[test]
fn enter_on_empty_text_clears_the_search() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");
    h.press(KeyCode::Enter);

    h.keys("/");
    for _ in 0..4 {
        h.press(KeyCode::Backspace);
    }
    h.press(KeyCode::Enter);

    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.app.search(), "");
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );
}

#[test]
fn slash_pre_fills_the_current_search() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");
    h.press(KeyCode::Enter);

    h.keys("/");

    assert_eq!(h.app.prompt(), Some("acme"));
    h.keys(" off");
    assert_eq!(h.subjects(), ["Re: ACME offsite"]);
}

#[test]
fn esc_at_the_prompt_puts_the_search_back_as_it_was_before_slash() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");
    h.press(KeyCode::Enter);
    h.keys("/ off");

    assert_eq!(h.press(KeyCode::Esc), []);

    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.app.search(), "acme");
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
}

#[test]
fn esc_at_the_prompt_with_no_search_before_it_clears_the_text() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");

    h.press(KeyCode::Esc);

    assert_eq!(h.app.search(), "");
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );
}

#[test]
fn esc_with_the_prompt_closed_clears_the_search() {
    let mut h = Harness::loaded(store());
    h.keys("/acme");
    h.press(KeyCode::Enter);

    assert_eq!(h.press(KeyCode::Esc), []);

    assert_eq!(h.app.search(), "");
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );
}

#[test]
fn the_cursor_keeps_its_thread_while_it_matches_and_otherwise_goes_to_the_first_row() {
    let mut h = Harness::loaded(store());
    h.keys("jj");
    assert_eq!(h.subjects()[h.cursor()], "News");

    h.keys("/acme");
    assert_eq!(h.cursor(), 1, "News still matches");

    h.keys(" ");
    assert_eq!(h.subjects(), ["Invoice", "Re: ACME offsite"]);
    assert_eq!(h.cursor(), 0, "News no longer matches");

    h.press(KeyCode::Enter);
    h.keys("j");
    h.press(KeyCode::Esc);
    assert_eq!(
        h.subjects()[h.cursor()],
        "Re: ACME offsite",
        "kept on clearing"
    );
}

#[test]
fn clearing_the_search_puts_the_cursor_on_the_first_row_when_the_queue_was_empty() {
    let mut h = Harness::loaded(store());
    h.keys("jjj/zzz");
    assert_eq!(h.subjects(), Vec::<String>::new());

    h.press(KeyCode::Esc);

    assert_eq!(h.cursor(), 0);
}

#[test]
fn a_search_under_unread_only_narrows_the_unread_threads() {
    let mut h = Harness::loaded(store());
    toggle_unread_only(&mut h);

    search(&mut h, "acme");

    assert_eq!(h.subjects(), ["Invoice"]);
}

#[test]
fn u_keeps_the_search_applied_to_the_reloaded_queue() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");

    toggle_unread_only(&mut h);
    assert_eq!(h.app.search(), "acme");
    assert_eq!(h.subjects(), ["Invoice"]);

    toggle_unread_only(&mut h);
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
}

#[test]
fn u_while_the_prompt_is_open_is_typed() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("/u"), []);
    assert!(!h.app.loading());
}

#[test]
fn a_switch_with_m_clears_the_search() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");

    let effects = h.switch_to("travel");
    assert_eq!(h.app.search(), "acme", "until the switch lands");
    h.resolve(effects);

    assert_eq!(h.app.search(), "");
    assert_eq!(h.subjects(), ["Flight"]);

    let effects = h.switch_to("inbox");
    h.resolve(effects);
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );
}

#[test]
fn a_switch_landing_while_the_prompt_is_open_closes_it() {
    let mut h = Harness::loaded(store());
    let effects = h.switch_to("travel");
    h.keys("/acme");

    h.resolve(effects);

    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.app.search(), "");
    assert_eq!(h.subjects(), ["Flight"]);
}

#[test]
fn a_reconcile_keeps_the_search() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");
    h.store_mut().change_email("e4", |email| {
        email.subject = Some("Re: Offsite".into());
    });
    h.store_mut().deliver(
        EmailSpec::new("e6", "t6", "2026-10-18T09:30:00Z")
            .in_mailbox("mb-inbox")
            .from("Acme Sales", "sales@example.invalid")
            .subject("Renewal"),
    );
    h.store_mut().deliver(
        EmailSpec::new("e7", "t7", "2026-10-18T09:40:00Z")
            .in_mailbox("mb-inbox")
            .from("Sam", "sam@example.invalid")
            .subject("Drinks"),
    );

    reconcile(&mut h);

    assert_eq!(h.app.search(), "acme");
    assert_eq!(h.subjects(), ["Invoice", "Renewal", "News"]);
}

#[test]
fn e_archives_the_row_under_the_cursor_of_the_narrowed_queue() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");

    let effects = h.keys("e");

    assert!(
        matches!(&effects[..], [Effect::SetEmails { update, .. }] if update.contains_key("e2")),
        "{effects:?}"
    );
    assert_eq!(h.subjects(), ["News", "Re: ACME offsite"]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.resolve(effects), []);
    h.press(KeyCode::Esc);
    assert_eq!(h.subjects(), ["Lunch", "News", "Re: ACME offsite"]);
}

#[test]
fn a_failed_archive_under_a_search_puts_the_row_back() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");
    h.keys("e");

    h.send(Msg::Sent {
        job: 1,
        result: Ok(SetResult {
            not_updated: vec!["e2".into()],
        }),
    });

    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
}

#[test]
fn r_leaves_the_row_in_place() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");

    let effects = h.keys("r");

    assert!(
        matches!(&effects[..], [Effect::SetEmails { update, .. }] if update.contains_key("e2")),
        "{effects:?}"
    );
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
    assert!(!h.app.queue().rows()[0].unread());
}

#[test]
fn r_under_unread_only_takes_the_row_out() {
    let mut h = Harness::loaded(store());
    toggle_unread_only(&mut h);
    search(&mut h, "acme");

    h.keys("r");

    assert_eq!(h.subjects(), Vec::<String>::new());
    h.press(KeyCode::Esc);
    assert_eq!(h.subjects(), ["Lunch"]);
}

#[test]
fn a_switch_with_m_to_the_mailbox_shown_clears_the_search() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");

    let effects = h.switch_to("inbox");
    h.resolve(effects);

    assert_eq!(h.app.search(), "");
    assert_eq!(
        h.subjects(),
        ["Lunch", "Invoice", "News", "Re: ACME offsite"]
    );
}

#[test]
fn u_after_a_switch_before_it_lands_still_clears_the_search() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");
    h.switch_to("inbox");

    let effects = h.keys("u");
    h.resolve(effects);

    assert!(h.app.unread_only());
    assert_eq!(h.app.search(), "");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
}

#[test]
fn a_failed_switch_leaves_the_search_as_it_was() {
    let mut h = Harness::loaded(store());
    search(&mut h, "acme");
    let generation = match h.switch_to("travel")[..] {
        [Effect::Query { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };

    h.send(Msg::Loaded {
        generation,
        result: Err(Error::Status(400)),
    });

    assert_eq!(h.app.search(), "acme");
    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.subjects(), ["Invoice", "News", "Re: ACME offsite"]);
}
