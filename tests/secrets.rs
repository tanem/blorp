//! The token never reaches the log, an error, or the terminal.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blorp::cli::{FirstLine, StoreError, TokenStore, setup};
use blorp::jmap::{EmailQuery, HttpJmap, Jmap};
use blorp::run::{Exit, launch, run};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use secrecy::SecretString;
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{mail_capabilities, session};

const TOKEN: &str = "fmu1-secret-test-token-0123456789";

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

/// A store that never holds anything, so setup's write is the only use.
struct NoStore;

impl TokenStore for NoStore {
    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        Ok(None)
    }

    fn set(&self, _token: &SecretString) -> Result<(), StoreError> {
        Ok(())
    }

    fn delete(&self) -> Result<bool, StoreError> {
        Ok(false)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_token_appears_nowhere_after_an_action_a_failed_request_and_a_fatal_401() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session(&server, mail_capabilities())),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Mailbox/get", {"list": [
                {"id": "mb-inbox", "name": "Inbox", "parentId": null, "role": "inbox"},
                {"id": "mb-archive", "name": "Archive", "parentId": null, "role": "archive"}
            ]}, "0"]],
            "sessionState": "0"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_string_contains("Email/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/query", {"ids": ["Me1"], "total": 1}, "0"],
                ["Email/get", {"list": [{"id": "Me1", "threadId": "Tt1"}]}, "1"],
                ["Thread/get", {"list": [{"id": "Tt1", "emailIds": ["Me1"]}]}, "2"]
            ],
            "sessionState": "0"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_string_contains("Email/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/get", {"list": [{
                "id": "Me1",
                "threadId": "Tt1",
                "mailboxIds": {"mb-inbox": true},
                "keywords": {},
                "from": null,
                "subject": "Lunch",
                "receivedAt": "2026-10-18T09:00:00Z",
                "hasAttachment": false
            }]}, "0"]],
            "sessionState": "0"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_string_contains("Email/set"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let mut out = Vec::new();
    let mut errors = String::new();

    let session_url = format!("{}/jmap/session", server.uri());
    let input = format!("{TOKEN}\n");
    let (jmap, _) = setup(
        &NoStore,
        &mut FirstLine(input.as_bytes()),
        &mut out,
        &session_url,
    )
    .await
    .map_err(|e| e.to_string())
    .unwrap();
    let app = launch(&jmap, Timestamp::UNIX_EPOCH).await.unwrap();
    let failed = jmap
        .query(&EmailQuery {
            filter: json!({"inMailbox": "mb-inbox"}),
            ascending: false,
            limit: 500,
        })
        .await
        .unwrap_err();
    errors.push_str(&format!("{failed}\n{failed:?}\n"));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    // `e` once the queue has loaded, after five requests: the session fetch,
    // `Mailbox/get`, the failed query above, and the queue query's two
    // stages. The archive fails with a 500, and the query re-run after it is
    // refused with a 401.
    let keys = Box::pin(
        stream::once({
            let server = &server;
            async move {
                while server.received_requests().await.unwrap().len() < 5 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(Event::Key(KeyEvent::new(
                    KeyCode::Char('e'),
                    KeyModifiers::NONE,
                )))
            }
        })
        .chain(stream::pending::<io::Result<Event>>()),
    );
    let exit = tokio::time::timeout(
        Duration::from_secs(10),
        run(&mut terminal, keys, jmap, blorp::run::SystemClock, app),
    )
    .await
    .expect("the 401 stops the state task")
    .unwrap();
    let Exit::Fatal(fatal) = exit else {
        panic!("expected a fatal exit, got {exit:?}");
    };
    let bodies: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(
        bodies.iter().any(|body| body.contains("Email/set")),
        "the archive was sent: {bodies:#?}"
    );
    errors.push_str(&format!("{fatal}\n{fatal:?}\n"));
    let rejected = HttpJmap::connect(
        &format!("{}/missing", server.uri()),
        SecretString::from(TOKEN),
    )
    .await
    .err()
    .expect("no session there");
    errors.push_str(&format!("{rejected}\n{rejected:?}\n"));

    let log = capture.text();
    assert!(!log.is_empty(), "the subscriber captured nothing");
    for (name, text) in [
        ("log", log),
        ("output", String::from_utf8(out).unwrap()),
        ("errors", errors),
        ("screen", format!("{:?}", terminal.backend().buffer())),
    ] {
        assert!(!text.contains(TOKEN), "the token is in the {name}:\n{text}");
        assert!(
            !text.contains("secret-test-token"),
            "part of the token is in the {name}:\n{text}"
        );
    }
}
