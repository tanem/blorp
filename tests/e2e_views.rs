//! Views in the blorp binary against a local JMAP server: the tabline, Tab,
//! `:view save` writing the config file, and a config file that does not
//! parse.

use std::fs;

use blorp::jmap::fake::{EmailSpec, Store};

mod e2e;

use e2e::server::LocalJmap;
use e2e::{Blorp, Sandbox, TOKEN};

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-17T09:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Read already"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-16T09:00:00Z")
                .in_mailbox("mb-travel")
                .seen()
                .subject("Flight"),
        )
        .build()
}

fn spawn(sandbox: &Sandbox) -> Blorp {
    sandbox.store_token(TOKEN);
    sandbox.spawn(&[], 80, 24)
}

#[tokio::test]
async fn tab_switches_views_and_view_save_writes_the_config_file_for_the_next_launch() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    let mut blorp = spawn(&sandbox);
    blorp.wait_statusline("1/2");
    assert_eq!(blorp.rows()[0], " Inbox  Unread  Flagged  Snoozed");

    blorp.keys("\t");
    blorp.wait_text(" Inbox  is:unread ");
    blorp.wait_statusline("1/1");
    blorp.keys("\x1b[Z");
    blorp.wait_statusline("1/2");
    assert!(!sandbox.config_path().exists());

    blorp.keys(":filter in:travel\r");
    blorp.wait_text(" in:travel ");
    blorp.keys(":view save Trips\r");
    blorp.wait_text("Saved view \"Trips\"");
    let written = fs::read_to_string(sandbox.config_path()).unwrap();
    assert!(written.contains("name = \"Inbox\""), "{written}");
    assert!(written.contains("name = \"Trips\""), "{written}");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);

    let mut blorp = spawn(&sandbox);
    blorp.wait_statusline("1/2");
    assert_eq!(blorp.rows()[0], " Inbox  Unread  Flagged  Snoozed  Trips");
    blorp.keys("\x1b[Z");
    blorp.wait_text(" Travel  in:travel ");
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
}

#[tokio::test]
async fn a_config_file_that_does_not_parse_stops_the_launch_naming_the_line() {
    let server = LocalJmap::start(inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    let path = sandbox.config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[[views]]\nname = \"Trips\"\nfilter = in:travel\n").unwrap();
    let mut blorp = spawn(&sandbox);

    let status = blorp.wait_exit();

    assert_eq!(status.exit_code(), 1);
    let output = blorp.output();
    assert!(output.contains("could not read"), "{output}");
    assert!(output.contains("line 3"), "{output}");
    assert!(!blorp.entered_alternate_screen());
}
