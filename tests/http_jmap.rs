use blorp::jmap::{EmailQuery, Error, HttpJmap, Jmap, Mailbox, Push, QueryResult};
use futures::StreamExt;
use secrecy::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

mod common;

use common::{mail_capabilities, session};

fn token() -> SecretString {
    SecretString::from("test-token-123")
}

async fn serve_session(server: &MockServer, capabilities: Value) {
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .and(header("authorization", "Bearer test-token-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session(server, capabilities)))
        .expect(1)
        .mount(server)
        .await;
}

async fn connect(server: &MockServer) -> Result<HttpJmap, Error> {
    HttpJmap::connect(&format!("{}/jmap/session", server.uri()), token())
        .await
        .map(|(jmap, _)| jmap)
}

#[tokio::test]
async fn connect_fetches_the_session_with_the_bearer_token() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;

    let (_, session) = HttpJmap::connect(&format!("{}/jmap/session", server.uri()), token())
        .await
        .unwrap();

    assert_eq!(session.username, "kim@example.invalid");
}

#[tokio::test]
async fn connect_refuses_a_session_without_the_mail_capability() {
    let server = MockServer::start().await;
    serve_session(&server, json!({"urn:ietf:params:jmap:core": {}})).await;

    let result = connect(&server).await;

    assert_eq!(result.err(), Some(Error::NoMailCapability));
}

#[tokio::test]
async fn connect_reports_a_rejected_token_as_its_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let result = connect(&server).await;

    assert_eq!(result.err(), Some(Error::Status(401)));
}

#[tokio::test]
async fn get_mailboxes_sends_mailbox_get_on_the_primary_mail_account() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(header("authorization", "Bearer test-token-123"))
        .and(body_json(json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Mailbox/get",
                {"accountId": "u1a2b3c4", "ids": null, "properties": ["id", "name", "role"]},
                "0"
            ]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [[
                "Mailbox/get",
                {
                    "accountId": "u1a2b3c4",
                    "state": "1",
                    "list": [
                        {"id": "mb1", "name": "Inbox", "role": "inbox"},
                        {"id": "mb2", "name": "Travel", "role": null}
                    ],
                    "notFound": []
                },
                "0"
            ]],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let mailboxes = jmap.get_mailboxes().await.unwrap();

    assert_eq!(
        mailboxes,
        [
            Mailbox {
                id: "mb1".into(),
                name: "Inbox".into(),
                role: Some("inbox".into()),
            },
            Mailbox {
                id: "mb2".into(),
                name: "Travel".into(),
                role: None,
            },
        ]
    );
}

/// The query's first request: `Email/query`, the hits' `threadId`, and
/// `Thread/get`, chained by back-reference.
fn first_stage(filter: Value) -> Value {
    json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [
            ["Email/query", {
                "accountId": "u1a2b3c4",
                "filter": filter,
                "sort": [{"property": "receivedAt", "isAscending": false}],
                "collapseThreads": true,
                "limit": 500,
                "calculateTotal": true
            }, "0"],
            ["Email/get", {
                "accountId": "u1a2b3c4",
                "#ids": {"resultOf": "0", "name": "Email/query", "path": "/ids"},
                "properties": ["threadId"]
            }, "1"],
            ["Thread/get", {
                "accountId": "u1a2b3c4",
                "#ids": {"resultOf": "1", "name": "Email/get", "path": "/list/*/threadId"}
            }, "2"]
        ]
    })
}

/// The answer to [`first_stage`]: one hit, `Me2`, in a thread `Tt1` of
/// `email_ids`.
fn first_stage_answer(email_ids: &[String]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "methodResponses": [
            ["Email/query", {"accountId": "u1a2b3c4", "ids": ["Me2"], "position": 0, "total": 6443}, "0"],
            ["Email/get", {"accountId": "u1a2b3c4", "list": [{"id": "Me2", "threadId": "Tt1"}], "notFound": []}, "1"],
            ["Thread/get", {"accountId": "u1a2b3c4", "list": [{"id": "Tt1", "emailIds": email_ids}], "notFound": []}, "2"]
        ],
        "sessionState": "0"
    }))
}

/// A member `Email/get` call for `ids`.
fn member_get(ids: &[String], call_id: &str) -> Value {
    json!(["Email/get", {
        "accountId": "u1a2b3c4",
        "ids": ids,
        "properties": [
            "id", "threadId", "mailboxIds", "keywords",
            "from", "subject", "receivedAt"
        ]
    }, call_id])
}

fn email(id: &str, received_at: &str) -> Value {
    json!({
        "id": id, "threadId": "Tt1", "mailboxIds": {"mb1": true},
        "keywords": {}, "from": [{"name": "Ana Lopez", "email": "ana@example.invalid"}],
        "subject": "Trip", "receivedAt": received_at
    })
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

/// Matches a request whose first method call is `name`.
struct FirstCall(&'static str);

impl Match for FirstCall {
    fn matches(&self, request: &Request) -> bool {
        serde_json::from_slice::<Value>(&request.body)
            .is_ok_and(|body| body["methodCalls"][0][0] == self.0)
    }
}

async fn inbox_query(jmap: &HttpJmap) -> Result<QueryResult, Error> {
    jmap.query(&EmailQuery {
        filter: json!({"inMailbox": "mb1"}),
        limit: 500,
    })
    .await
}

#[tokio::test]
async fn query_fetches_the_threads_then_their_emails_in_a_second_request() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(first_stage(json!({"inMailbox": "mb1"}))))
        .respond_with(first_stage_answer(&ids(&["Me1", "Me2"])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [member_get(&ids(&["Me1", "Me2"]), "0")]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/get", {"accountId": "u1a2b3c4", "list": [
                    {
                        "id": "Me1", "threadId": "Tt1", "mailboxIds": {"mb3": true},
                        "keywords": {"$seen": true}, "from": [{"name": "Kim", "email": "kim@example.invalid"}],
                        "subject": "Trip", "receivedAt": "2026-10-16T08:00:00Z"
                    },
                    {
                        "id": "Me2", "threadId": "Tt1", "mailboxIds": {"mb1": true},
                        "keywords": {}, "from": [{"name": "Ana Lopez", "email": "ana@example.invalid"}],
                        "subject": "Re: Trip", "receivedAt": "2026-10-17T09:30:00Z"
                    }
                ], "notFound": []}, "0"]
            ],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = inbox_query(&jmap).await.unwrap();

    assert_eq!(result.ids, ["Me2"]);
    assert_eq!(result.total, 6443);
    assert_eq!(result.threads.len(), 1);
    assert_eq!(result.threads[0].email_ids, ["Me1", "Me2"]);
    let ids: Vec<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["Me1", "Me2"]);
    assert_eq!(result.emails[1].subject.as_deref(), Some("Re: Trip"));
}

#[tokio::test]
async fn a_session_without_limits_gets_4096_emails_a_call() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    let members: Vec<String> = (0..4097).map(|e| format!("Me{e}")).collect();
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(FirstCall("Email/query"))
        .respond_with(first_stage_answer(&members))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                member_get(&members[..4096], "0"),
                member_get(&members[4096..], "1")
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/get", {"accountId": "u1a2b3c4", "list": [], "notFound": &members[..4096]}, "0"],
                ["Email/get", {"accountId": "u1a2b3c4", "list": [], "notFound": &members[4096..]}, "1"]
            ],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    inbox_query(&jmap).await.unwrap();
}

#[tokio::test]
async fn an_email_removed_before_the_second_request_is_left_out() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(FirstCall("Email/query"))
        .respond_with(first_stage_answer(&ids(&["Me1", "Me2"])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(FirstCall("Email/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/get", {
                    "accountId": "u1a2b3c4",
                    "list": [email("Me2", "2026-10-17T09:30:00Z")],
                    "notFound": ["Me1"]
                }, "0"]
            ],
            "sessionState": "0"
        })))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = inbox_query(&jmap).await.unwrap();

    assert_eq!(result.threads[0].email_ids, ["Me1", "Me2"]);
    let emails: Vec<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(emails, ["Me2"]);
}

#[tokio::test]
async fn a_method_error_in_the_second_request_fails_the_query() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(FirstCall("Email/query"))
        .respond_with(first_stage_answer(&ids(&["Me1", "Me2"])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(FirstCall("Email/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["error", {"type": "serverUnavailable"}, "0"]],
            "sessionState": "0"
        })))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = inbox_query(&jmap).await;

    assert_eq!(
        result.err(),
        Some(Error::Method {
            method: "Email/get".into(),
            kind: "serverUnavailable".into(),
        })
    );
}

#[tokio::test]
async fn a_method_error_is_reported_with_its_type() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["error", {"type": "accountNotFound"}, "0"]],
            "sessionState": "0"
        })))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = jmap.get_mailboxes().await;

    assert_eq!(
        result.err(),
        Some(Error::Method {
            method: "Mailbox/get".into(),
            kind: "accountNotFound".into(),
        })
    );
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_parse_error() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>oops</html>"))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = jmap.get_mailboxes().await;

    assert!(matches!(result, Err(Error::Parse(_))), "{result:?}");
}

#[tokio::test]
async fn errors_never_carry_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let error = connect(&server).await.err().unwrap();

    assert!(!format!("{error} {error:?}").contains("test-token-123"));
}

#[tokio::test]
async fn the_first_failing_call_in_a_chain_is_reported() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/query", {"accountId": "u1a2b3c4", "ids": ["Me1"], "position": 0, "total": 1}, "0"],
                ["error", {"type": "requestTooLarge", "description": "number of ids exceeds maxObjectsInGet limit"}, "1"],
                ["error", {"type": "invalidResultReference"}, "2"],
                ["error", {"type": "invalidResultReference"}, "3"]
            ],
            "sessionState": "0"
        })))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = jmap
        .query(&EmailQuery {
            filter: json!({"inMailbox": "mb1"}),
            limit: 500,
        })
        .await;

    assert_eq!(
        result.err(),
        Some(Error::Method {
            method: "Email/get".into(),
            kind: "requestTooLarge".into(),
        })
    );
}

#[tokio::test]
async fn connect_refuses_a_session_url_that_is_not_https() {
    let result = HttpJmap::connect("http://api.example.invalid/jmap/session", token()).await;

    assert_eq!(
        result.err(),
        Some(Error::InsecureUrl(
            "http://api.example.invalid/jmap/session".into()
        ))
    );
}

#[tokio::test]
async fn connect_refuses_an_api_url_that_is_not_https() {
    let server = MockServer::start().await;
    let mut session = session(&server, mail_capabilities());
    session["apiUrl"] = json!("http://api.example.invalid/jmap/api/");
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session))
        .mount(&server)
        .await;

    let result = connect(&server).await;

    assert_eq!(
        result.err(),
        Some(Error::InsecureUrl(
            "http://api.example.invalid/jmap/api/".into()
        ))
    );
}

#[tokio::test]
async fn reconcile_puts_mailbox_get_in_the_query_s_first_request() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    let mut first = first_stage(json!({"inMailbox": "mb1"}));
    first["methodCalls"].as_array_mut().unwrap().insert(
        0,
        json!(["Mailbox/get", {
            "accountId": "u1a2b3c4", "ids": null,
            "properties": ["id", "name", "role"]
        }, "m"]),
    );
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(first))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Mailbox/get", {"accountId": "u1a2b3c4", "state": "1", "list": [
                    {"id": "mb1", "name": "Inbox", "role": "inbox"}
                ], "notFound": []}, "m"],
                ["Email/query", {"accountId": "u1a2b3c4", "ids": ["Me2"], "position": 0, "total": 1}, "0"],
                ["Email/get", {"accountId": "u1a2b3c4", "list": [{"id": "Me2", "threadId": "Tt1"}], "notFound": []}, "1"],
                ["Thread/get", {"accountId": "u1a2b3c4", "list": [{"id": "Tt1", "emailIds": ["Me2"]}], "notFound": []}, "2"]
            ],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [member_get(&ids(&["Me2"]), "0")]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/get", {"accountId": "u1a2b3c4", "list": [email("Me2", "2026-10-17T09:30:00Z")], "notFound": []}, "0"]
            ],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let reconciled = jmap
        .reconcile(&EmailQuery {
            filter: json!({"inMailbox": "mb1"}),
            limit: 500,
        })
        .await
        .unwrap();

    assert_eq!(reconciled.mailboxes.len(), 1);
    assert_eq!(reconciled.mailboxes[0].role.as_deref(), Some("inbox"));
    assert_eq!(reconciled.result.ids, ["Me2"]);
    assert_eq!(reconciled.result.emails.len(), 1);
}

#[tokio::test]
async fn events_opens_the_push_stream_with_the_bearer_token_and_reads_it() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("GET"))
        .and(path("/event/"))
        .and(query_param("types", "Email,Mailbox"))
        .and(query_param("closeafter", "no"))
        .and(query_param("ping", "300"))
        .and(header("authorization", "Bearer test-token-123"))
        .and(header("accept", "text/event-stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    ": new event source connection\r\n\r\n",
                    "event: state\r\n",
                    "data: {\"@type\":\"StateChange\",\"changed\":{},\"type\":\"connect\"}\r\n\r\n",
                    "event: ping\r\ndata: {\"interval\":300}\r\n\r\n",
                )),
        )
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let stream = jmap.events().await.unwrap().expect("a push stream");
    let events: Vec<_> = stream.collect().await;

    assert_eq!(events, [Ok(Push::State), Ok(Push::Ping)]);
}

#[tokio::test]
async fn events_reports_a_refused_push_stream_as_its_status() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("GET"))
        .and(path("/event/"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = jmap.events().await;

    assert_eq!(result.err(), Some(Error::Status(503)));
}

#[tokio::test]
async fn events_gives_none_for_a_session_without_a_push_stream() {
    let server = MockServer::start().await;
    let mut session = session(&server, mail_capabilities());
    session.as_object_mut().unwrap().remove("eventSourceUrl");
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session))
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    assert!(jmap.events().await.unwrap().is_none());
}

#[tokio::test]
async fn connect_refuses_an_event_source_url_that_is_not_https() {
    let server = MockServer::start().await;
    let mut session = session(&server, mail_capabilities());
    session["eventSourceUrl"] = json!("http://api.example.invalid/jmap/event/");
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session))
        .mount(&server)
        .await;

    let result = connect(&server).await;

    assert_eq!(
        result.err(),
        Some(Error::InsecureUrl(
            "http://api.example.invalid/jmap/event/".into()
        ))
    );
}
