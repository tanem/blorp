//! Setup and logout in a pseudo-terminal against a local JMAP server, with
//! a fake token and a fresh token file for each test.

use blorp::cli::KEYRING;
use blorp::jmap::fake::{EmailSpec, Store};

mod e2e;

#[cfg(unix)]
use e2e::Modes;
use e2e::server::LocalJmap;
use e2e::{REJECTED, Sandbox, TOKEN};

const INTRO: &str = "blorp needs a Fastmail API token. Create one in Fastmail under \
Settings, Privacy & Security, Connected apps & API tokens, with the Email scope \
only: not read-only, and not Email submission.";

fn stored() -> String {
    format!("Stored the API token for kim@example.invalid in {KEYRING}.")
}

const NO_EMAIL_SCOPE: &str = "The API token does not give access to mail. It needs the Email \
scope. Run `blorp setup` to store one that has it.";

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .build()
}

async fn server() -> LocalJmap {
    LocalJmap::start(inbox(), TOKEN).await
}

#[tokio::test]
async fn with_no_token_blorp_runs_setup_then_opens_the_inbox() {
    let server = server().await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    let mut blorp = sandbox.spawn(&[], 80, 24);

    blorp.wait_text("API token: ");
    blorp.wait_echo_off();
    blorp.keys(&format!("{TOKEN}\r"));
    blorp.wait_statusline("1/1");

    assert_eq!(sandbox.stored_token().as_deref(), Some(TOKEN));
    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
    let rows = blorp.rows();
    let prompt = rows
        .iter()
        .position(|row| row == "API token:")
        .expect("the prompt row");
    assert_eq!(rows[prompt + 1], stored(), "a line break follows Enter");
    assert!(rows.join("").contains(INTRO), "{rows:#?}");
    assert!(!blorp.output().contains(TOKEN), "the token was shown");
}

#[tokio::test]
async fn an_empty_token_exits_1_and_keeps_the_stored_token() {
    let server = server().await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token("old-token");
    let mut blorp = sandbox.spawn(&["setup"], 80, 24);

    blorp.wait_text("API token: ");
    blorp.wait_echo_off();
    blorp.keys("\r");

    assert_eq!(blorp.wait_exit().exit_code(), 1);
    assert!(blorp.output().contains("blorp: no API token was entered"));
    assert_eq!(sandbox.stored_token().as_deref(), Some("old-token"));
}

#[tokio::test]
async fn a_rejected_token_exits_1_and_keeps_the_stored_token() {
    let server = server().await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token("old-token");
    let mut blorp = sandbox.spawn(&["setup"], 80, 24);

    blorp.wait_text("API token: ");
    blorp.wait_echo_off();
    blorp.keys("fake-token-2\r");

    assert_eq!(blorp.wait_exit().exit_code(), 1);
    assert!(blorp.output().contains(REJECTED), "{}", blorp.output());
    assert_eq!(sandbox.stored_token().as_deref(), Some("old-token"));
}

#[tokio::test]
async fn ctrl_c_at_the_prompt_exits_with_echo_restored() {
    let server = server().await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    let mut blorp = sandbox.spawn(&[], 80, 24);

    blorp.wait_text("API token: ");
    blorp.wait_echo_off();
    blorp.keys("fake\x03");

    assert_eq!(blorp.wait_exit().exit_code(), 130);
    // ConPTY cannot report the console mode, so Windows skips this check.
    #[cfg(unix)]
    assert_eq!(blorp.modes(), Modes::COOKED);
    assert_eq!(sandbox.stored_token(), None);
    assert!(!blorp.entered_alternate_screen());
}

#[tokio::test]
async fn a_session_without_mail_prints_the_email_scope_message_and_exits_1() {
    let server = LocalJmap::start_without_mail(TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);

    assert_eq!(blorp.wait_exit().exit_code(), 1);
    assert!(
        blorp.output().contains(NO_EMAIL_SCOPE),
        "{}",
        blorp.output()
    );
    assert!(!blorp.entered_alternate_screen());
}

#[tokio::test]
async fn the_environment_token_is_used_in_place_of_the_stored_one() {
    let server = server().await;
    let sandbox = Sandbox::new()
        .session_url(&server.session_url())
        .env("BLORP_API_TOKEN", TOKEN);
    sandbox.store_token("fake-token-2");
    let mut blorp = sandbox.spawn(&[], 80, 24);

    blorp.wait_statusline("1/1");

    blorp.keys("q");
    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert_eq!(sandbox.stored_token().as_deref(), Some("fake-token-2"));
}

#[tokio::test]
async fn setup_prompts_even_with_the_environment_token_set() {
    let server = server().await;
    let sandbox = Sandbox::new()
        .session_url(&server.session_url())
        .env("BLORP_API_TOKEN", "fake-token-env");
    let mut blorp = sandbox.spawn(&["setup"], 80, 24);

    blorp.wait_text("API token: ");
    blorp.wait_echo_off();
    blorp.keys(&format!("{TOKEN}\r"));

    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert!(blorp.output().contains(&stored()), "{}", blorp.output());
    assert_eq!(sandbox.stored_token().as_deref(), Some(TOKEN));
}

#[test]
fn logout_deletes_the_stored_token() {
    let sandbox = Sandbox::new();
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&["logout"], 80, 24);

    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert!(
        blorp
            .output()
            .contains(&format!("Deleted the API token from {KEYRING}."))
    );
    assert_eq!(sandbox.stored_token(), None);
}

#[test]
fn logout_succeeds_when_no_token_is_stored() {
    let sandbox = Sandbox::new();
    let mut blorp = sandbox.spawn(&["logout"], 80, 24);

    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert!(
        blorp
            .output()
            .contains(&format!("No API token was stored in {KEYRING}."))
    );
}

#[test]
fn logout_warns_when_the_environment_token_is_set() {
    let sandbox = Sandbox::new().env("BLORP_API_TOKEN", TOKEN);
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&["logout"], 80, 24);

    assert_eq!(blorp.wait_exit().exit_code(), 0);
    let output = blorp.output();
    assert!(
        output.contains(&format!("Deleted the API token from {KEYRING}.")),
        "{output}"
    );
    assert!(
        output.contains("BLORP_API_TOKEN is set, so blorp will still use the token it holds."),
        "{output}"
    );
    assert_eq!(sandbox.stored_token(), None);
}
