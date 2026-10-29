//! The blorp binary lists the mailboxes with their unread counts and
//! switches mailboxes with `m` against a local JMAP server.

use blorp::jmap::fake::{EmailSpec, Store};
use jiff::{SignedDuration, Timestamp};

mod e2e;

use e2e::server::LocalJmap;
use e2e::{Sandbox, TOKEN};

/// RFC 3339 for `minutes` before now.
fn ago(minutes: i64) -> String {
    (Timestamp::now() - SignedDuration::from_mins(minutes)).to_string()
}

/// Unread threads: one in the Inbox and two in Work/Receipts. Work and
/// Travel hold only read mail.
fn mailboxes() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-travel", "Travel")
        .mailbox("mb-work", "Work")
        .child_mailbox("mb-receipts", "Receipts", "mb-work")
        .email(
            EmailSpec::new("e1", "t1", &ago(10))
                .in_mailbox("mb-inbox")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", &ago(20))
                .in_mailbox("mb-receipts")
                .subject("Order 48213"),
        )
        .email(
            EmailSpec::new("e3", "t3", &ago(30))
                .in_mailbox("mb-receipts")
                .subject("Order 48214"),
        )
        .email(
            EmailSpec::new("e4", "t4", &ago(40))
                .in_mailbox("mb-work")
                .seen()
                .subject("Plan"),
        )
        .email(
            EmailSpec::new("e5", "t5", &ago(50))
                .in_mailbox("mb-travel")
                .seen()
                .subject("Flight"),
        )
        .build()
}

#[tokio::test]
async fn m_lists_the_mailboxes_with_their_unread_counts_and_switches_to_the_one_chosen() {
    let server = LocalJmap::start(mailboxes(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/1");

    blorp.keys("m");
    blorp.wait_statusline(" 1/4");
    let rows = blorp.rows();
    assert_eq!(
        rows[..4],
        [
            format!("1 Inbox{}1", " ".repeat(72)),
            "1 Travel".to_string(),
            "2 Work".to_string(),
            format!("3 Work/Receipts{}2", " ".repeat(64)),
        ],
        "{rows:#?}"
    );

    blorp.keys("u");
    blorp.wait_statusline(" 1/2");
    assert!(
        blorp.statusline().contains(" Unread mailboxes only "),
        "{}",
        blorp.statusline()
    );
    assert!(
        blorp.rows()[1].starts_with("1 Work/Receipts "),
        "{:#?}",
        blorp.rows()
    );

    blorp.keys("/rec");
    blorp.wait_for("the prompt", |screen| e2e::rows(screen)[23] == "/rec");
    blorp.keys("\r");
    blorp.wait_for("the prompt to close", |screen| {
        e2e::rows(screen)[23].is_empty()
    });
    assert!(
        blorp.statusline().contains(" /rec "),
        "{}",
        blorp.statusline()
    );
    blorp.keys("\r");

    blorp.wait_statusline(" 1/2");
    assert!(
        blorp.statusline().starts_with(" Work/Receipts "),
        "{}",
        blorp.statusline()
    );
    assert!(
        blorp.rows()[0].contains("Order 48213"),
        "{:#?}",
        blorp.rows()
    );
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
}
