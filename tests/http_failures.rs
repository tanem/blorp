//! How the reqwest adapter reports failed requests, and the class each
//! failure falls into.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blorp::jmap::{Class, EmailUpdate, Error, Fatal, HttpJmap, Jmap, Timeouts};
use secrecy::SecretString;
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{mail_capabilities, session};

async fn serve_session(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session(server, mail_capabilities())),
        )
        .mount(server)
        .await;
}

async fn connect_with(server: &MockServer, timeouts: Timeouts) -> HttpJmap {
    serve_session(server).await;
    let url = format!("{}/jmap/session", server.uri());
    HttpJmap::connect_with(&url, SecretString::from("test-token-123"), timeouts)
        .await
        .unwrap()
        .0
}

async fn connect(server: &MockServer) -> HttpJmap {
    connect_with(server, Timeouts::default()).await
}

/// Answers every API request with `response`.
async fn answer(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(response)
        .mount(server)
        .await;
}

fn method_error(kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "methodResponses": [["error", {"type": kind}, "0"]],
        "sessionState": "0"
    }))
}

fn problem(kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "type": format!("urn:ietf:params:jmap:error:{kind}"),
        "status": 400
    }))
}

#[test]
fn requests_time_out_after_10_s_to_connect_and_30_s_in_all() {
    assert_eq!(
        Timeouts::default(),
        Timeouts {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(30),
        }
    );
}

#[tokio::test]
async fn a_429_carries_the_wait_its_retry_after_asks_for() {
    let server = MockServer::start().await;
    let jmap = connect(&server).await;
    answer(
        &server,
        ResponseTemplate::new(429).insert_header("Retry-After", "7"),
    )
    .await;

    let error = jmap.get_mailboxes().await.unwrap_err();

    assert_eq!(
        error,
        Error::RetryAfter {
            status: 429,
            wait: Duration::from_secs(7),
        }
    );
    assert_eq!(error.class(), Class::Transient);
    assert_eq!(error.retry_after(), Some(Duration::from_secs(7)));
}

#[tokio::test]
async fn a_429_without_retry_after_is_transient() {
    let server = MockServer::start().await;
    let jmap = connect(&server).await;
    answer(&server, ResponseTemplate::new(429)).await;

    let error = jmap.get_mailboxes().await.unwrap_err();

    assert_eq!(error, Error::Status(429));
    assert_eq!(error.class(), Class::Transient);
    assert_eq!(error.retry_after(), None);
}

#[tokio::test]
async fn a_5xx_is_transient() {
    for status in [500, 502, 503] {
        let server = MockServer::start().await;
        let jmap = connect(&server).await;
        answer(&server, ResponseTemplate::new(status)).await;

        let error = jmap.get_mailboxes().await.unwrap_err();

        assert_eq!(error, Error::Status(status));
        assert_eq!(error.class(), Class::Transient, "HTTP {status}");
    }
}

#[tokio::test]
async fn a_request_that_takes_too_long_times_out_as_a_transient_failure() {
    let server = MockServer::start().await;
    let jmap = connect_with(
        &server,
        Timeouts {
            connect: Duration::from_secs(10),
            request: Duration::from_millis(200),
        },
    )
    .await;
    answer(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"methodResponses": [], "sessionState": "0"}))
            .set_delay(Duration::from_secs(5)),
    )
    .await;

    let error = jmap.get_mailboxes().await.unwrap_err();

    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(error.class(), Class::Transient);
}

#[tokio::test]
async fn a_server_that_is_not_listening_is_a_transient_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/jmap/session", listener.local_addr().unwrap());
    drop(listener);

    let error = HttpJmap::connect(&url, SecretString::from("test-token-123"))
        .await
        .err()
        .unwrap();

    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert_eq!(error.class(), Class::Transient);
}

#[tokio::test]
async fn the_request_level_limit_error_is_transient_and_the_others_are_bugs() {
    for (kind, class) in [
        ("limit", Class::Transient),
        ("notJSON", Class::Bug),
        ("notRequest", Class::Bug),
        ("unknownCapability", Class::Bug),
    ] {
        let server = MockServer::start().await;
        let jmap = connect(&server).await;
        answer(&server, problem(kind)).await;

        let error = jmap.get_mailboxes().await.unwrap_err();

        assert_eq!(error, Error::Request(kind.into()));
        assert_eq!(error.class(), class, "{kind}");
    }
}

#[tokio::test]
async fn server_unavailable_and_server_fail_are_transient_and_other_method_errors_bugs() {
    for (kind, class) in [
        ("serverUnavailable", Class::Transient),
        ("serverFail", Class::Transient),
        ("invalidArguments", Class::Bug),
        ("requestTooLarge", Class::Bug),
        ("serverPartialFail", Class::Bug),
    ] {
        let server = MockServer::start().await;
        let jmap = connect(&server).await;
        answer(&server, method_error(kind)).await;

        let error = jmap.get_mailboxes().await.unwrap_err();

        assert_eq!(error.class(), class, "{kind}");
    }
}

#[tokio::test]
async fn a_rate_limit_set_error_fails_the_update_as_transient() {
    let server = MockServer::start().await;
    let jmap = connect(&server).await;
    answer(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [[
                "Email/set",
                {
                    "accountId": "u1a2b3c4",
                    "newState": "2",
                    "updated": {"e1": null},
                    "notUpdated": {"e2": {"type": "rateLimit"}}
                },
                "0"
            ]],
            "sessionState": "0"
        })),
    )
    .await;
    let patch = || [("keywords/$seen".to_string(), Value::Bool(true))].into();
    let update = EmailUpdate::from([("e1".into(), patch()), ("e2".into(), patch())]);

    let error = jmap.set_emails(&update).await.unwrap_err();

    assert_eq!(
        error,
        Error::Method {
            method: "Email/set".into(),
            kind: "rateLimit".into(),
        }
    );
    assert_eq!(error.class(), Class::Transient);
}

#[tokio::test]
async fn other_4xx_statuses_are_bugs_and_401_and_403_fatal() {
    for (status, class) in [
        (400, Class::Bug),
        (404, Class::Bug),
        (401, Class::Fatal(Fatal::TokenRejected)),
        (403, Class::Fatal(Fatal::NoEmailScope)),
    ] {
        let server = MockServer::start().await;
        let jmap = connect(&server).await;
        answer(&server, ResponseTemplate::new(status)).await;

        let error = jmap.get_mailboxes().await.unwrap_err();

        assert_eq!(error.class(), class, "HTTP {status}");
    }
}

#[test]
fn a_bug_reads_as_fastmail_error_and_its_type() {
    let method = Error::Method {
        method: "Email/set".into(),
        kind: "invalidArguments".into(),
    };
    assert_eq!(method.to_string(), "Fastmail error: invalidArguments");
    assert_eq!(
        Error::Request("notJSON".into()).to_string(),
        "Fastmail error: notJSON"
    );
    assert_eq!(Error::Status(404).to_string(), "Fastmail error: HTTP 404");
}

/// Everything a subscriber writes, kept for inspection.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_body_that_is_not_json_is_a_bug_logged_without_its_headers() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let server = MockServer::start().await;
    let jmap = connect(&server).await;
    answer(
        &server,
        ResponseTemplate::new(200)
            .insert_header("x-trace", "header-value-77")
            .set_body_string("<html>oops</html>"),
    )
    .await;

    let error = jmap.get_mailboxes().await.unwrap_err();

    assert!(matches!(error, Error::Parse(_)), "{error:?}");
    assert_eq!(error.class(), Class::Bug);
    let log = capture.text();
    assert!(log.contains("ERROR"), "{log}");
    assert!(log.contains("<html>oops</html>"), "{log}");
    assert!(!log.contains("header-value-77"), "{log}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_bug_method_error_logs_the_response() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let server = MockServer::start().await;
    let jmap = connect(&server).await;
    answer(&server, method_error("invalidArguments")).await;

    jmap.get_mailboxes().await.unwrap_err();

    let log = capture.text();
    assert!(log.contains("ERROR"), "{log}");
    assert!(log.contains(r#""type":"invalidArguments""#), "{log}");
}
