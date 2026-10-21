//! The blorp binary archives against a local JMAP server, live and in dry
//! run.

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
                .subject("Trip to Wellington"),
        )
        .build()
}

/// The method calls of every API request, flattened.
async fn method_calls(server: &LocalJmap) -> Vec<Value> {
    server
        .api_requests()
        .await
        .into_iter()
        .flat_map(|request| request["methodCalls"].as_array().unwrap().clone())
        .collect()
}

#[tokio::test]
async fn e_archives_the_thread_on_the_server() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/2");

    // q straight after the action waits for its Email/set.
    blorp.keys("eq");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let sets: Vec<Value> = method_calls(&server)
        .await
        .into_iter()
        .filter(|call| call[0] == "Email/set")
        .collect();
    assert_eq!(
        sets,
        [json!(["Email/set", {"accountId": "u1a2b3c4", "update": {
            "e1": {"mailboxIds/mb-inbox": null, "mailboxIds/mb-archive": true}
        }}, "0"])]
    );
    let mut relaunched = sandbox.spawn(&[], 80, 24);
    relaunched.wait_statusline(" 1/1");
    assert!(
        relaunched.rows()[0].contains("Trip to Wellington"),
        "{:#?}",
        relaunched.rows()
    );
    relaunched.keys("q");
    relaunched.wait_exit();
}

#[tokio::test]
async fn dry_run_says_so_shows_the_badge_and_sends_nothing() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&["--dry-run"], 80, 24);
    blorp.wait_statusline(" 1/2");
    assert!(
        blorp.statusline().contains(" DRY RUN "),
        "{}",
        blorp.statusline()
    );

    blorp.keys("e");
    blorp.wait_statusline(" 1/1");
    blorp.wait_text("[dry] Archived 1 thread");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    assert!(
        blorp
            .output()
            .contains("Dry run: actions are logged, not sent to Fastmail."),
        "{}",
        blorp.output()
    );
    let calls = method_calls(&server).await;
    assert!(
        !calls.iter().any(|call| call[0] == "Email/set"),
        "{calls:#?}"
    );
    let log = std::fs::read_to_string(sandbox.log_path()).unwrap();
    assert!(log.contains("blorp::dry_run: archive t1"), "{log}");
}
