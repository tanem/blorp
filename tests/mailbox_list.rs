//! `m` opens the mailbox list in place of the queue: every mailbox with its
//! count of unread threads, narrowed with `/`, and with `u` only those
//! holding an unread thread.

use blorp::app::{Effect, MailboxChoices, MailboxRow, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use crossterm::event::KeyCode;

/// Mailboxes listed out of path order, with Work/Receipts nested. Unread
/// threads: two in the Inbox, one in Travel, and one in Work/Receipts.
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
                .seen()
                .subject("Order 2"),
        )
        .email(
            EmailSpec::new("e5", "t5", "2026-10-16T10:00:00Z")
                .in_mailbox("mb-archive")
                .seen()
                .subject("Old news"),
        )
        .email(
            EmailSpec::new("e6", "t6", "2026-10-16T09:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Flight"),
        )
        .build()
}

const ALL: [(&str, usize); 6] = [
    ("Inbox", 2),
    ("Archive", 0),
    ("Sent", 0),
    ("Travel", 1),
    ("Work", 0),
    ("Work/Receipts", 1),
];

const UNREAD: [(&str, usize); 3] = [("Inbox", 2), ("Travel", 1), ("Work/Receipts", 1)];

fn list(h: &Harness) -> MailboxChoices {
    h.app.mailbox_list().expect("the mailbox list is open")
}

/// Each listed mailbox's path and unread thread count.
fn rows(h: &Harness) -> Vec<(String, usize)> {
    list(h)
        .rows
        .into_iter()
        .map(|MailboxRow { path, unread }| (path, unread))
        .collect()
}

fn owned(rows: &[(&str, usize)]) -> Vec<(String, usize)> {
    rows.iter().map(|(p, n)| (p.to_string(), *n)).collect()
}

/// The path of the highlighted mailbox.
fn highlighted(h: &Harness) -> String {
    let list = list(h);
    list.rows[list.highlight].path.clone()
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
fn m_lists_every_mailbox_with_its_unread_count_inbox_first_with_the_one_shown_highlighted() {
    let mut h = Harness::loaded(store());

    assert_eq!(h.keys("m"), []);

    assert_eq!(
        h.app.mailbox_list(),
        Some(MailboxChoices {
            rows: ALL
                .iter()
                .map(|(path, unread)| MailboxRow {
                    path: path.to_string(),
                    unread: *unread,
                })
                .collect(),
            highlight: 0,
            unread_mailboxes_only: false,
            text: String::new(),
            typing: false,
        })
    );
}

#[test]
fn the_list_highlights_the_mailbox_shown_when_it_opens() {
    let mut h = Harness::loaded(store());
    let effects = h.switch_to("receipts");
    h.resolve(effects);

    h.keys("m");

    assert_eq!(highlighted(&h), "Work/Receipts");
}

#[test]
fn j_and_k_move_the_highlight_and_stop_at_the_ends() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.keys("k"), []);
    assert_eq!(list(&h).highlight, 0);
    h.keys("jj");
    assert_eq!(list(&h).highlight, 2);
    h.keys("jjjjj");
    assert_eq!(list(&h).highlight, 5);
    h.keys("kk");
    assert_eq!(list(&h).highlight, 3);
    assert_eq!(h.cursor(), 0);
}

#[test]
fn esc_closes_the_list_and_returns_to_the_queue_as_it_was() {
    let mut h = Harness::loaded(store());
    h.keys("j");
    h.keys("mjj");

    assert_eq!(h.press(KeyCode::Esc), []);

    assert_eq!(h.app.mailbox_list(), None);
    assert_eq!(h.app.queue_path(), "Inbox");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert_eq!(h.cursor(), 1);
}

#[test]
fn q_quits_from_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.keys("q"), [Effect::Quit]);
}

#[test]
fn ctrl_c_quits_from_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

#[test]
fn e_r_o_and_m_do_nothing_on_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("mj");

    assert_eq!(h.keys("eorm"), []);

    assert_eq!(rows(&h), owned(&ALL));
    assert_eq!(highlighted(&h), "Archive");
    assert_eq!(h.status(), "");
    assert_eq!(h.subjects(), ["Lunch", "Invoice"]);
    assert!(h.app.queue().rows().iter().all(|t| t.unread()));
}

#[test]
fn slash_opens_the_prompt_and_the_list_narrows_on_each_key_ignoring_case() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.keys("/"), []);
    assert!(list(&h).typing);
    assert_eq!(h.app.prompt(), Some(""));
    assert_eq!(rows(&h), owned(&ALL));

    assert_eq!(h.keys("WoR"), []);
    assert_eq!(list(&h).text, "WoR");
    assert_eq!(h.app.prompt(), Some("WoR"));
    assert_eq!(rows(&h), owned(&[("Work", 0), ("Work/Receipts", 1)]));
    // The queue's search is apart from the list's text.
    assert_eq!(h.app.search(), "");
}

#[test]
fn keys_typed_at_the_prompt_go_into_the_text_and_do_not_act() {
    let mut h = Harness::loaded(store());
    h.keys("m/");

    assert_eq!(h.keys("jkueormq"), []);

    assert_eq!(list(&h).text, "jkueormq");
    assert!(!list(&h).unread_mailboxes_only);
    assert_eq!(list(&h).highlight, 0);
    assert_eq!(h.status(), "");
}

#[test]
fn backspace_deletes_at_the_prompt_and_widens_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m/workx");
    assert_eq!(rows(&h), []);

    h.press(KeyCode::Backspace);
    h.press(KeyCode::Backspace);

    assert_eq!(list(&h).text, "wor");
    assert_eq!(rows(&h), owned(&[("Work", 0), ("Work/Receipts", 1)]));
}

#[test]
fn enter_at_the_prompt_locks_the_text_in_without_switching() {
    let mut h = Harness::loaded(store());
    h.keys("m/rec");

    assert_eq!(h.press(KeyCode::Enter), []);

    assert!(!list(&h).typing);
    assert_eq!(list(&h).text, "rec");
    assert_eq!(h.app.prompt(), None);
    assert_eq!(rows(&h), owned(&[("Work/Receipts", 1)]));
    assert_eq!(h.app.queue_path(), "Inbox");
}

#[test]
fn enter_with_the_prompt_closed_switches_to_the_highlighted_mailbox() {
    let mut h = Harness::loaded(store());
    h.keys("m/wor");
    h.press(KeyCode::Enter);
    h.keys("j");

    let effects = h.press(KeyCode::Enter);

    assert!(matches!(effects[..], [Effect::Query { .. }]), "{effects:?}");
    assert_eq!(h.app.mailbox_list(), None);
    assert_eq!(h.app.loading_path(), "Work/Receipts");
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Order 1", "Order 2"]);
}

#[test]
fn enter_on_empty_text_clears_the_narrowing() {
    let mut h = Harness::loaded(store());
    h.keys("m/wor");
    h.press(KeyCode::Enter);
    h.keys("/");
    for _ in 0..3 {
        h.press(KeyCode::Backspace);
    }

    h.press(KeyCode::Enter);

    assert_eq!(list(&h).text, "");
    assert!(!list(&h).typing);
    assert_eq!(rows(&h), owned(&ALL));
}

#[test]
fn esc_at_the_prompt_puts_the_text_back_as_it_was_before_slash() {
    let mut h = Harness::loaded(store());
    h.keys("m/wor");
    h.press(KeyCode::Enter);
    h.keys("/k/");

    assert_eq!(h.press(KeyCode::Esc), []);

    assert_eq!(list(&h).text, "wor");
    assert!(!list(&h).typing);
    assert_eq!(rows(&h), owned(&[("Work", 0), ("Work/Receipts", 1)]));
}

#[test]
fn esc_with_the_prompt_closed_clears_the_text_and_then_closes_the_list() {
    let mut h = Harness::loaded(store());
    h.keys("m/wor");
    h.press(KeyCode::Enter);

    assert_eq!(h.press(KeyCode::Esc), []);
    assert_eq!(list(&h).text, "");
    assert_eq!(rows(&h), owned(&ALL));

    assert_eq!(h.press(KeyCode::Esc), []);
    assert_eq!(h.app.mailbox_list(), None);
}

#[test]
fn text_that_matches_no_mailbox_lists_none_and_enter_does_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("m/zz");
    h.press(KeyCode::Enter);

    assert_eq!(rows(&h), []);
    assert_eq!(h.press(KeyCode::Enter), []);
    assert_eq!(h.keys("jk"), []);

    assert!(h.app.mailbox_list().is_some());
    assert_eq!(h.app.queue_path(), "Inbox");
}

#[test]
fn ctrl_c_quits_at_the_prompt() {
    let mut h = Harness::loaded(store());
    h.keys("m/wor");

    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

#[test]
fn the_text_starts_empty_at_each_opening() {
    let mut h = Harness::loaded(store());
    let effects = h.switch_to("travel");
    h.resolve(effects);

    h.keys("m");

    assert_eq!(list(&h).text, "");
    assert!(!list(&h).typing);
}

#[test]
fn closing_the_list_keeps_the_queue_s_search_and_unread_only() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);
    h.keys("/lunch");
    h.press(KeyCode::Enter);
    h.keys("m/wor");
    h.press(KeyCode::Enter);

    h.press(KeyCode::Esc);
    h.press(KeyCode::Esc);

    assert_eq!(h.app.mailbox_list(), None);
    assert_eq!(h.app.search(), "lunch");
    assert!(h.app.unread_only());
    assert_eq!(h.subjects(), ["Lunch"]);
}

#[test]
fn a_switch_from_the_list_clears_the_queue_s_search_and_keeps_unread_only() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);
    h.keys("/order");
    h.press(KeyCode::Enter);

    let effects = h.switch_to("receipts");
    h.resolve(effects);

    assert_eq!(h.app.search(), "");
    assert!(h.app.unread_only());
    assert_eq!(h.subjects(), ["Order 1"]);
}

#[test]
fn u_shows_only_the_mailboxes_holding_an_unread_thread_and_again_shows_every_mailbox() {
    let mut h = Harness::loaded(store());
    h.keys("m");

    assert_eq!(h.keys("u"), []);
    assert!(list(&h).unread_mailboxes_only);
    assert_eq!(rows(&h), owned(&UNREAD));

    assert_eq!(h.keys("u"), []);
    assert!(!list(&h).unread_mailboxes_only);
    assert_eq!(rows(&h), owned(&ALL));
}

#[test]
fn unread_mailboxes_only_starts_off() {
    let mut h = Harness::loaded(store());

    h.keys("m");

    assert!(!list(&h).unread_mailboxes_only);
}

#[test]
fn unread_mailboxes_only_stays_on_across_openings_and_switches() {
    let mut h = Harness::loaded(store());
    h.keys("mu");
    h.press(KeyCode::Esc);

    h.keys("m");
    assert!(list(&h).unread_mailboxes_only);
    h.keys("j");
    let effects = h.press(KeyCode::Enter);
    h.resolve(effects);
    assert_eq!(h.app.queue_path(), "Travel");

    h.keys("m");
    assert!(list(&h).unread_mailboxes_only);
}

#[test]
fn unread_mailboxes_only_combines_with_narrowing() {
    let mut h = Harness::loaded(store());

    h.keys("mu/r");

    assert_eq!(rows(&h), owned(&[("Travel", 1), ("Work/Receipts", 1)]));
}

#[test]
fn unread_mailboxes_only_and_unread_only_leave_each_other_alone() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("u");
    h.resolve(effects);

    h.keys("m");
    assert!(!list(&h).unread_mailboxes_only);
    h.keys("u");
    assert!(h.app.unread_only());
    h.press(KeyCode::Esc);

    let effects = h.keys("u");
    h.resolve(effects);
    assert!(!h.app.unread_only());
    h.keys("m");
    assert!(list(&h).unread_mailboxes_only);
}

#[test]
fn the_highlight_starts_on_the_first_row_when_unread_mailboxes_only_hides_the_mailbox_shown() {
    let mut h = Harness::loaded(store());
    let effects = h.switch_to("archive");
    h.resolve(effects);
    h.keys("mu");
    h.press(KeyCode::Esc);

    h.keys("m");

    assert_eq!(list(&h).highlight, 0);
    assert_eq!(highlighted(&h), "Inbox");
}

#[test]
fn the_highlight_stays_on_its_mailbox_while_it_is_listed_and_otherwise_goes_to_the_first_row() {
    let mut h = Harness::loaded(store());
    h.keys("mjjj");
    assert_eq!(highlighted(&h), "Travel");

    h.keys("/r");
    assert_eq!(highlighted(&h), "Travel");
    assert_eq!(list(&h).highlight, 1);

    h.keys("e");
    assert_eq!(highlighted(&h), "Work/Receipts");
    h.press(KeyCode::Backspace);
    assert_eq!(highlighted(&h), "Work/Receipts");
    h.press(KeyCode::Enter);

    h.keys("k");
    assert_eq!(highlighted(&h), "Work");
    h.keys("u");
    assert_eq!(highlighted(&h), "Travel");
    h.keys("u");
    assert_eq!(highlighted(&h), "Travel");
}

#[test]
fn a_reconcile_refreshes_the_counts() {
    let mut h = Harness::loaded(store());
    h.keys("m");
    h.store_mut().change_email("e1", |email| {
        email.keywords.insert("$seen".into(), true);
    });
    h.store_mut().deliver(
        EmailSpec::new("e7", "t7", "2026-10-18T09:00:00Z")
            .in_mailbox("mb-work")
            .subject("Plan"),
    );

    reconcile(&mut h);

    assert_eq!(
        rows(&h),
        owned(&[
            ("Inbox", 1),
            ("Archive", 0),
            ("Sent", 0),
            ("Travel", 1),
            ("Work", 1),
            ("Work/Receipts", 1),
        ])
    );
}

#[test]
fn a_mailbox_whose_last_unread_thread_is_read_leaves_the_list_under_unread_mailboxes_only_on_a_reconcile()
 {
    let mut h = Harness::loaded(store());
    h.keys("muj");
    assert_eq!(highlighted(&h), "Travel");
    h.store_mut().change_email("e6", |email| {
        email.keywords.insert("$seen".into(), true);
    });

    reconcile(&mut h);

    assert_eq!(rows(&h), owned(&[("Inbox", 2), ("Work/Receipts", 1)]));
    assert_eq!(highlighted(&h), "Inbox");
}

#[test]
fn the_highlight_stays_on_its_mailbox_across_a_reconcile() {
    let mut h = Harness::loaded(store());
    h.keys("mjjj");
    h.store_mut().delete_mailbox("mb-archive");

    reconcile(&mut h);

    assert_eq!(highlighted(&h), "Travel");
    assert_eq!(list(&h).highlight, 2);
}

#[test]
fn the_highlight_goes_to_the_first_row_when_a_reconcile_drops_its_mailbox() {
    let mut h = Harness::loaded(store());
    h.keys("mjjjjj");
    h.store_mut().delete_mailbox("mb-receipts");

    reconcile(&mut h);

    assert_eq!(highlighted(&h), "Inbox");
}
