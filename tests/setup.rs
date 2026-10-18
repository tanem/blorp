use std::cell::RefCell;

use blorp::cli::{Lines, StoreError, TokenStore, find_token, logout, setup};
use blorp::jmap::Jmap;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf-8 output")
}

#[test]
fn logout_deletes_the_stored_token() {
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    logout(&store, false, &mut out).unwrap();

    assert_eq!(store.token(), None);
    assert_eq!(text(out), "Deleted the API token from the keychain.\n");
}

#[test]
fn logout_succeeds_when_no_token_is_stored() {
    let store = FakeStore::default();
    let mut out = Vec::new();

    logout(&store, false, &mut out).unwrap();

    assert_eq!(text(out), "No API token was stored in the keychain.\n");
}

#[test]
fn logout_warns_that_the_environment_token_is_still_used() {
    let store = FakeStore::holding("old-token");
    let mut out = Vec::new();

    logout(&store, true, &mut out).unwrap();

    assert_eq!(
        text(out),
        "Deleted the API token from the keychain.\n\
         BLORP_API_TOKEN is set, so blorp will still use the token it holds.\n"
    );
}

fn session(server: &MockServer, capabilities: Value) -> Value {
    json!({
        "capabilities": capabilities,
        "accounts": {"u1a2b3c4": {"name": "kim@example.invalid", "isPersonal": true}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "u1a2b3c4"},
        "username": "kim@example.invalid",
        "apiUrl": format!("{}/api/", server.uri()),
        "eventSourceUrl": format!("{}/event/", server.uri()),
        "state": "0"
    })
}

fn mail_capabilities() -> Value {
    json!({"urn:ietf:params:jmap:core": {}, "urn:ietf:params:jmap:mail": {}})
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
        &mut Lines(&b"  new-token \n"[..]),
        &mut out,
        &session_url(&server),
    )
    .await
    .unwrap();

    assert_eq!(store.token().as_deref(), Some("new-token"));
    assert_eq!(session.username, "kim@example.invalid");
    assert_eq!(
        text(out),
        format!("{INTRO}Stored the API token for kim@example.invalid in the keychain.\n")
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
        &mut Lines(&b"new-token\n"[..]),
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
        &mut Lines(&b"new-token\n"[..]),
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
        &mut Lines(&b"  \n"[..]),
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
