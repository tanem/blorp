use blorp::jmap::{EmailQuery, Error, HttpJmap, Jmap, Mailbox};
use secrecy::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
                {"accountId": "u1a2b3c4", "ids": null, "properties": ["id", "name", "parentId", "role"]},
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
                        {"id": "mb1", "name": "Inbox", "parentId": null, "role": "inbox"},
                        {"id": "mb2", "name": "Travel", "parentId": null, "role": null}
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
                parent_id: None,
                role: Some("inbox".into()),
            },
            Mailbox {
                id: "mb2".into(),
                name: "Travel".into(),
                parent_id: None,
                role: None,
            },
        ]
    );
}

#[tokio::test]
async fn query_chains_thread_get_and_email_get_by_back_reference() {
    let server = MockServer::start().await;
    serve_session(&server, mail_capabilities()).await;
    Mock::given(method("POST"))
        .and(path("/api/"))
        .and(body_json(json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                ["Email/query", {
                    "accountId": "u1a2b3c4",
                    "filter": {"inMailbox": "mb1"},
                    "sort": [{"property": "receivedAt", "isAscending": false}],
                    "collapseThreads": true
                }, "0"],
                ["Email/get", {
                    "accountId": "u1a2b3c4",
                    "#ids": {"resultOf": "0", "name": "Email/query", "path": "/ids"},
                    "properties": ["threadId"]
                }, "1"],
                ["Thread/get", {
                    "accountId": "u1a2b3c4",
                    "#ids": {"resultOf": "1", "name": "Email/get", "path": "/list/*/threadId"}
                }, "2"],
                ["Email/get", {
                    "accountId": "u1a2b3c4",
                    "#ids": {"resultOf": "2", "name": "Thread/get", "path": "/list/*/emailIds"},
                    "properties": [
                        "id", "threadId", "mailboxIds", "keywords",
                        "from", "subject", "receivedAt", "hasAttachment"
                    ]
                }, "3"]
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [
                ["Email/query", {"accountId": "u1a2b3c4", "ids": ["Me2"], "position": 0}, "0"],
                ["Email/get", {"accountId": "u1a2b3c4", "list": [{"id": "Me2", "threadId": "Tt1"}], "notFound": []}, "1"],
                ["Thread/get", {"accountId": "u1a2b3c4", "list": [{"id": "Tt1", "emailIds": ["Me1", "Me2"]}], "notFound": []}, "2"],
                ["Email/get", {"accountId": "u1a2b3c4", "list": [
                    {
                        "id": "Me1", "threadId": "Tt1", "mailboxIds": {"mb3": true},
                        "keywords": {"$seen": true}, "from": [{"name": "Kim", "email": "kim@example.invalid"}],
                        "subject": "Trip", "receivedAt": "2026-10-16T08:00:00Z", "hasAttachment": false
                    },
                    {
                        "id": "Me2", "threadId": "Tt1", "mailboxIds": {"mb1": true},
                        "keywords": {}, "from": [{"name": "Ana Lopez", "email": "ana@example.invalid"}],
                        "subject": "Re: Trip", "receivedAt": "2026-10-17T09:30:00Z", "hasAttachment": true
                    }
                ], "notFound": []}, "3"]
            ],
            "sessionState": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let jmap = connect(&server).await.unwrap();

    let result = jmap
        .query(&EmailQuery {
            filter: json!({"inMailbox": "mb1"}),
            ascending: false,
        })
        .await
        .unwrap();

    assert_eq!(result.ids, ["Me2"]);
    assert_eq!(result.threads.len(), 1);
    assert_eq!(result.threads[0].email_ids, ["Me1", "Me2"]);
    let ids: Vec<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["Me1", "Me2"]);
    assert_eq!(result.emails[1].subject.as_deref(), Some("Re: Trip"));
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
