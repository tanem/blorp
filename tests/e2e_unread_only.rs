//! The blorp binary toggles unread only with `u` against a local JMAP
//! server.

use blorp::jmap::fake::{EmailSpec, Store};
use jiff::{SignedDuration, Timestamp};

mod e2e;

use e2e::server::LocalJmap;
use e2e::{Sandbox, TOKEN};

/// RFC 3339 for `minutes` before now.
fn ago(minutes: i64) -> String {
    (Timestamp::now() - SignedDuration::from_mins(minutes)).to_string()
}

/// Newest first: Lunch is unread, Trip read, Invoice unread, and Order
/// read in the Inbox with an unread reply in Archive.
fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(
            EmailSpec::new("e1", "t1", &ago(10))
                .in_mailbox("mb-inbox")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", &ago(20))
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip to Wellington"),
        )
        .email(
            EmailSpec::new("e3", "t3", &ago(30))
                .in_mailbox("mb-inbox")
                .subject("Invoice 1042"),
        )
        .email(
            EmailSpec::new("e4a", "t4", &ago(50))
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Order 7731"),
        )
        .email(
            EmailSpec::new("e4b", "t4", &ago(40))
                .in_mailbox("mb-archive")
                .subject("Re: Order 7731"),
        )
        .build()
}

#[tokio::test]
async fn u_shows_only_the_unread_threads_and_u_again_shows_every_thread() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/4");

    blorp.keys("u");
    blorp.wait_statusline(" 1/3");
    assert!(
        blorp.statusline().starts_with(" Inbox   Unread only "),
        "{}",
        blorp.statusline()
    );
    let rows = blorp.rows();
    assert!(rows[0].contains("Lunch on Friday?"), "{rows:#?}");
    assert!(rows[1].contains("Invoice 1042"), "{rows:#?}");
    assert!(rows[2].contains("Order 7731"), "{rows:#?}");

    blorp.keys("u");
    blorp.wait_statusline(" 1/4");
    assert!(
        !blorp.statusline().contains("Unread only"),
        "{}",
        blorp.statusline()
    );
    assert!(
        blorp.rows()[1].contains("Trip to Wellington"),
        "{:#?}",
        blorp.rows()
    );

    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
}
