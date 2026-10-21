use std::cell::RefCell;

use blorp::cli::{FirstLine, KEYRING, Prompt, StoreError, TokenStore, find_token, logout, setup};
use blorp::jmap::Jmap;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{mail_capabilities, session};

/// A token store held in memory.
#[derive(Default)]
struct FakeStore {
    token: RefCell<Option<String>>,
}

impl FakeStore {
    fn holding(token: &str) -> Self {
        FakeStore {
            token: RefCell::new(Some(token.into())),
        }
    }

    fn token(&self) -> Option<String> {
        self.token.borrow().clone()
    }
}

impl TokenStore for FakeStore {
    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        Ok(self.token.borrow().as_deref().map(SecretString::from))
    }

    fn set(&self, token: &SecretString) -> Result<(), StoreError> {
        *self.token.borrow_mut() = Some(token.expose_secret().into());
        Ok(())
    }

    fn delete(&self) -> Result<bool, StoreError> {
        Ok(self.token.borrow_mut().take().is_some())
    }
}

/// A store whose keyring cannot be reached, as on a Linux machine with no
/// Secret Service.
struct NoKeyring;

impl TokenStore for NoKeyring {
    fn reachable(&self) -> Result<(), StoreError> {
        Err(no_keyring())
    }

    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        Err(no_keyring())
    }

    fn set(&self, _token: &SecretString) -> Result<(), StoreError> {
        Err(no_keyring())
    }

    fn delete(&self) -> Result<bool, StoreError> {
        Err(no_keyring())
    }
}

fn no_keyring() -> StoreError {
    StoreError::Unavailable("no session bus".into())
}

/// What blorp says when no keyring can be reached and no token is set.
fn no_keyring_message() -> String {
    format!(
        "cannot reach {KEYRING} to keep the API token in: no session bus. \
         Set BLORP_API_TOKEN to the token, or run `blorp setup` once it is available."
    )
}

/// A prompt that fails the test if it is read.
struct NoPrompt;

impl Prompt for NoPrompt {
    fn read_token(&mut self, _prompt: &str) -> std::io::Result<String> {
        panic!("the prompt was read");
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf-8 output")
}

#[test]
fn logout_deletes_the_stored_token() {
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    logout(&store, false, &mut out).unwrap();

    assert_eq!(store.token(), None);
    assert_eq!(
        text(out),
        format!("Deleted the API token from {KEYRING}.\n")
    );
}

#[test]
fn logout_succeeds_when_no_token_is_stored() {
    let store = FakeStore::default();
    let mut out = Vec::new();

    logout(&store, false, &mut out).unwrap();

    assert_eq!(
        text(out),
        format!("No API token was stored in {KEYRING}.\n")
    );
}

#[test]
fn logout_warns_that_the_environment_token_is_still_used() {
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    logout(&store, true, &mut out).unwrap();

    assert_eq!(
        text(out),
        format!(
            "Deleted the API token from {KEYRING}.\n\
             BLORP_API_TOKEN is set, so blorp will still use the token it holds.\n"
        )
    );
}

/// Serves a session with `capabilities` to requests bearing `token`.
async fn serve_session(server: &MockServer, token: &str, capabilities: Value) {
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .and(header("authorization", format!("Bearer {token}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(session(server, capabilities)))
        .expect(1)
        .mount(server)
        .await;
}

fn session_url(server: &MockServer) -> String {
    format!("{}/jmap/session", server.uri())
}

const INTRO: &str = "blorp needs a Fastmail API token. Create one in Fastmail under \
Settings, Privacy & Security, Connected apps & API tokens, with the Email scope \
only: not read-only, and not Email submission.\n";

#[tokio::test]
async fn setup_checks_the_token_then_stores_it_and_names_the_account() {
    let server = MockServer::start().await;
    serve_session(&server, "new-token", mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(header("authorization", "Bearer new-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Mailbox/get", {"list": []}, "0"]],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    let (jmap, session) = setup(
        &store,
        &mut FirstLine(&b"  new-token \n"[..]),
        &mut out,
        &session_url(&server),
    )
    .await
    .unwrap();

    assert_eq!(store.token().as_deref(), Some("new-token"));
    assert_eq!(session.username, "kim@example.invalid");
    assert_eq!(
        text(out),
        format!("{INTRO}Stored the API token for kim@example.invalid in {KEYRING}.\n")
    );
    jmap.get_mailboxes()
        .await
        .expect("the client setup returns uses the new token");
}

const REJECTED: &str = "Fastmail rejected the API token. It may have been removed in Settings. \
Run `blorp setup` to store a new one.";

#[tokio::test]
async fn setup_keeps_the_stored_token_when_fastmail_rejects_the_new_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    let error = setup(
        &store,
        &mut FirstLine(&b"new-token\n"[..]),
        &mut out,
        &session_url(&server),
    )
    .await
    .err()
    .expect("setup fails");

    assert_eq!(error.to_string(), REJECTED);
    assert_eq!(store.token().as_deref(), Some("old-token"));
}

#[tokio::test]
async fn setup_keeps_the_stored_token_when_the_new_one_lacks_the_email_scope() {
    let server = MockServer::start().await;
    serve_session(
        &server,
        "new-token",
        json!({"urn:ietf:params:jmap:core": {}}),
    )
    .await;
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    let error = setup(
        &store,
        &mut FirstLine(&b"new-token\n"[..]),
        &mut out,
        &session_url(&server),
    )
    .await
    .err()
    .expect("setup fails");

    assert_eq!(
        error.to_string(),
        "The API token does not give access to mail. It needs the Email scope. \
         Run `blorp setup` to store one that has it."
    );
    assert_eq!(store.token().as_deref(), Some("old-token"));
}

#[tokio::test]
async fn setup_stops_without_a_request_when_no_token_is_entered() {
    let server = MockServer::start().await;
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    let error = setup(
        &store,
        &mut FirstLine(&b"  \n"[..]),
        &mut out,
        &session_url(&server),
    )
    .await
    .err()
    .expect("setup fails");

    assert_eq!(error.to_string(), "no API token was entered");
    assert_eq!(store.token().as_deref(), Some("old-token"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn the_environment_token_is_used_in_place_of_the_stored_one() {
    let store = FakeStore::holding("stored-token");

    let token = find_token(Some(SecretString::from("env-token")), &store).unwrap();

    assert_eq!(token.unwrap().expose_secret(), "env-token");
}

#[test]
fn the_stored_token_is_used_without_the_environment_one() {
    let store = FakeStore::holding("stored-token");

    let token = find_token(None, &store).unwrap();

    assert_eq!(token.unwrap().expose_secret(), "stored-token");
}

#[test]
fn no_token_anywhere_means_setup_runs() {
    let token = find_token(None, &FakeStore::default()).unwrap();

    assert!(token.is_none());
}

#[test]
fn with_no_keyring_the_environment_token_is_used() {
    let token = find_token(Some(SecretString::from("env-token")), &NoKeyring).unwrap();

    assert_eq!(token.unwrap().expose_secret(), "env-token");
}

#[test]
fn with_no_keyring_and_no_environment_token_launch_names_both_options() {
    let error = find_token(None, &NoKeyring).expect_err("no token");

    assert_eq!(error.to_string(), no_keyring_message());
}

#[tokio::test]
async fn with_no_keyring_setup_names_both_options_before_the_prompt() {
    let server = MockServer::start().await;
    let mut out = Vec::new();

    let error = setup(&NoKeyring, &mut NoPrompt, &mut out, &session_url(&server))
        .await
        .err()
        .expect("setup fails");

    assert_eq!(error.to_string(), no_keyring_message());
    assert_eq!(text(out), "");
    assert!(server.received_requests().await.unwrap().is_empty());
}
