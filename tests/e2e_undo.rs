//! The blorp binary undoes an archive against a local JMAP server, live and
//! in dry run.

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

/// The method calls of every API request after launch, flattened, as
/// name and arguments.
async fn method_calls(server: &LocalJmap) -> Vec<(String, Value)> {
    server
        .api_requests()
        .await
        .into_iter()
        .flat_map(|request| request["methodCalls"].as_array().unwrap().clone())
        .map(|call| (call[0].as_str().unwrap().to_string(), call[1].clone()))
        .collect()
}

#[tokio::test]
async fn u_puts_the_archived_thread_back_on_the_server() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/2");

    blorp.keys("e");
    blorp.wait_text("Archived 1 thread");
    blorp.keys("u");
    blorp.wait_statusline(" 1/2");
    blorp.wait_text("Undone: archive 1 thread");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let calls = method_calls(&server).await;
    let sets: Vec<&Value> = calls
        .iter()
        .filter(|(name, _)| name == "Email/set")
        .map(|(_, args)| &args["update"])
        .collect();
    assert_eq!(
        sets,
        [
            &json!({"e1": {"mailboxIds/mb-inbox": null, "mailboxIds/mb-archive": true}}),
            &json!({"e1": {"mailboxIds/mb-archive": null, "mailboxIds/mb-inbox": true}}),
        ]
    );
    let last_get = calls
        .iter()
        .rev()
        .find(|(name, _)| name == "Email/get")
        .map(|(_, args)| &args["ids"]);
    assert_eq!(last_get, Some(&json!(["e1"])));
    let mut relaunched = sandbox.spawn(&[], 80, 24);
    relaunched.wait_statusline(" 1/2");
    relaunched.keys("q");
    relaunched.wait_exit();
}

#[tokio::test]
async fn a_dry_run_undo_is_logged_and_sends_nothing() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&["--dry-run"], 80, 24);
    blorp.wait_statusline(" 1/2");

    blorp.keys("e");
    blorp.wait_statusline(" 1/1");
    blorp.keys("u");
    blorp.wait_statusline(" 1/2");
    blorp.wait_text("[dry] Undone: archive 1 thread");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let calls = method_calls(&server).await;
    assert!(
        !calls.iter().any(|(name, _)| name == "Email/set"),
        "{calls:#?}"
    );
    let log = std::fs::read_to_string(sandbox.log_path()).unwrap();
    assert!(log.contains("blorp::dry_run: undo archive t1"), "{log}");
}
