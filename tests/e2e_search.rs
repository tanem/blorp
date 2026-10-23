//! The blorp binary searches the queue with `/` against a local JMAP
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

/// Newest first: Lunch from Ana, Invoice from Acme Billing, and the ACME
/// offsite from Kim.
fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .email(
            EmailSpec::new("e1", "t1", &ago(10))
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", &ago(20))
                .in_mailbox("mb-inbox")
                .from("Acme Billing", "billing@example.invalid")
                .subject("Invoice 1042"),
        )
        .email(
            EmailSpec::new("e3", "t3", &ago(30))
                .in_mailbox("mb-inbox")
                .seen()
                .from("Kim Park", "kim@example.invalid")
                .subject("Re: ACME offsite"),
        )
        .build()
}

#[tokio::test]
async fn slash_narrows_the_queue_and_esc_clears_the_search() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/3");

    blorp.keys("/acme");
    blorp.wait_for("the search typed", |screen| {
        e2e::statusline(screen).starts_with(" Inbox   /acme ")
    });
    assert!(
        blorp.statusline().ends_with(" 1/2"),
        "{}",
        blorp.statusline()
    );
    assert_eq!(blorp.rows()[23], "/acme");
    blorp.keys("\r");
    blorp.wait_for("the prompt closed", |screen| {
        e2e::rows(screen)[23].is_empty()
    });
    assert!(
        blorp.statusline().starts_with(" Inbox   /acme "),
        "{}",
        blorp.statusline()
    );
    let rows = blorp.rows();
    assert!(rows[0].contains("Invoice 1042"), "{rows:#?}");
    assert!(rows[1].contains("Re: ACME offsite"), "{rows:#?}");
    assert!(!rows[2].contains("Lunch"), "{rows:#?}");

    blorp.keys("\x1b");
    // The cursor stays on Invoice.
    blorp.wait_statusline(" 2/3");
    assert!(
        !blorp.statusline().contains("/acme"),
        "{}",
        blorp.statusline()
    );
    assert!(
        blorp.rows()[0].contains("Lunch on Friday?"),
        "{:#?}",
        blorp.rows()
    );

    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
}
