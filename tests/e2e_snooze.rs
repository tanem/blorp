//! The blorp binary snoozes and returns threads against a local JMAP
//! server, with the schedule in its state directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;

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

#[tokio::test]
async fn z_creates_the_snooze_mailbox_writes_the_schedule_and_moves_the_thread() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);
    blorp.wait_statusline(" 1/2");

    blorp.keys("z");
    blorp.wait_text("Later today");
    blorp.keys("n");
    blorp.wait_text("Snoozed 1 thread until");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let file = sandbox.state_dir().join("snooze.json");
    let schedule: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(schedule["version"], 1);
    assert_eq!(
        schedule["threads"]["t1"]["emails"],
        json!({"e1": "mb-inbox"})
    );
    assert_eq!(schedule["threads"]["t1"]["returnedAt"], Value::Null);
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
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
async fn a_due_thread_returns_to_the_top_of_the_inbox_at_launch() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-snooze", "Snoozed (blorp)")
        .email(
            EmailSpec::new("e1", "t1", &ago(10))
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e3", "t3", &ago(3 * 24 * 60))
                .in_mailbox("mb-snooze")
                .seen()
                .subject("Renewal"),
        )
        .build();
    let server = LocalJmap::start(store, TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let schedule = json!({
        "version": 1,
        "threads": {"t3": {
            "emails": {"e3": "mb-inbox"},
            "snoozedAt": ago(2 * 24 * 60),
            "returnAt": ago(5),
            "returnedAt": null
        }}
    });
    fs::create_dir_all(sandbox.state_dir()).unwrap();
    fs::set_permissions(sandbox.state_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        sandbox.state_dir().join("snooze.json"),
        schedule.to_string(),
    )
    .unwrap();
    let mut blorp = sandbox.spawn(&[], 80, 24);

    blorp.wait_text("Returned 1 thread from snooze");
    blorp.wait_statusline(" 1/2");
    assert!(blorp.rows()[0].contains("Renewal"), "{:#?}", blorp.rows());
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let written: Value =
        serde_json::from_str(&fs::read_to_string(sandbox.state_dir().join("snooze.json")).unwrap())
            .unwrap();
    assert!(
        written["threads"]["t3"]["returnedAt"].is_string(),
        "{written}"
    );
}

#[tokio::test]
async fn a_schedule_that_does_not_parse_is_moved_aside_and_reported() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    fs::create_dir_all(sandbox.state_dir()).unwrap();
    fs::set_permissions(sandbox.state_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(sandbox.state_dir().join("snooze.json"), "{not json").unwrap();
    let mut blorp = sandbox.spawn(&[], 120, 24);

    blorp.wait_text("The snooze schedule did not parse; it was moved to");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let names: Vec<String> = fs::read_dir(sandbox.state_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("snooze.json.broken-")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n == "snooze.json"), "{names:?}");
}
