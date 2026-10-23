//! The blorp binary marks a thread read against a local JMAP server.

use blorp::jmap::fake::{EmailSpec, Store};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

mod e2e;

use e2e::server::LocalJmap;
use e2e::{Sandbox, TOKEN};

/// RFC 3339 for `minutes` before now.
fn ago(minutes: i64) -> String {
    (Timestamp::now() - SignedDuration::from_mins(minutes)).to_string()
}

/// Thread t1 is unread in the Inbox and in Archive; t2 is read.
fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(
            EmailSpec::new("e1a", "t1", &ago(30))
                .in_mailbox("mb-archive")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e1b", "t1", &ago(10))
                .in_mailbox("mb-inbox")
                .subject("Re: Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", &ago(20))
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip to Wellington"),
        )
        .build()
}

/// The `Email/set` calls of every API request.
async fn email_sets(server: &LocalJmap) -> Vec<Value> {
    server
        .api_requests()
        .await
        .into_iter()
        .flat_map(|request| request["methodCalls"].as_array().unwrap().clone())
        .filter(|call| call[0] == "Email/set")
        .collect()
}

#[tokio::test]
async fn r_marks_the_thread_read_on_the_server() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/2");
    assert!(blorp.rows()[0].starts_with('●'), "{:#?}", blorp.rows());

    blorp.keys("r");
    blorp.wait_text("Marked read");
    let rows = blorp.rows();
    assert!(
        rows[0].contains("Re: Lunch on Friday?") && !rows[0].starts_with('●'),
        "{rows:#?}"
    );
    assert!(
        blorp.statusline().ends_with(" 1/2"),
        "{}",
        blorp.statusline()
    );

    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert_eq!(
        email_sets(&server).await,
        [json!(["Email/set", {"accountId": "u1a2b3c4", "update": {
            "e1a": {"keywords/$seen": true},
            "e1b": {"keywords/$seen": true}
        }}, "0"])]
    );
    let mut relaunched = sandbox.spawn(&[], 80, 24);
    relaunched.wait_statusline(" 1/2");
    let rows = relaunched.rows();
    assert!(
        rows[0].contains("Re: Lunch on Friday?") && !rows[0].starts_with('●'),
        "{rows:#?}"
    );
    relaunched.keys("q");
    relaunched.wait_exit();
}
