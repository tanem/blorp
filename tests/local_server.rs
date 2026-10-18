//! The reqwest adapter against the local JMAP server the end-to-end tests
//! use, which serves a fake store.

use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{
    CORE, EmailPatch, EmailQuery, EmailUpdate, Error, HttpJmap, Jmap, MAIL, Mailbox, Thread,
};
use secrecy::SecretString;
use serde_json::{Value, json};

mod e2e;

use e2e::TOKEN;
use e2e::server::{ACCOUNT_ID, LocalJmap};

fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .child_mailbox("mb-travel", "Travel", "mb-inbox")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .email(EmailSpec::new("e2a", "t2", "2026-10-17T08:00:00Z").in_mailbox("mb-travel"))
        .email(EmailSpec::new("e2b", "t2", "2026-10-18T08:00:00Z").in_mailbox("mb-inbox"))
        .email(EmailSpec::new("e3", "t3", "2026-10-16T08:00:00Z").in_mailbox("mb-inbox"))
        .build()
}

async fn connect(server: &LocalJmap, token: &str) -> Result<HttpJmap, Error> {
    HttpJmap::connect(&server.session_url(), SecretString::from(token))
        .await
        .map(|(jmap, _)| jmap)
}

fn inbox(limit: usize) -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        ascending: false,
        limit,
    }
}

#[tokio::test]
async fn the_session_advertises_fastmails_max_objects_in_get() {
    let server = LocalJmap::start(store(), TOKEN).await;

    let (_, session) = HttpJmap::connect(&server.session_url(), SecretString::from(TOKEN))
        .await
        .unwrap();

    assert_eq!(session.username, "kim@example.invalid");
    assert_eq!(session.capabilities[CORE]["maxObjectsInGet"], 4096);
}

#[tokio::test]
async fn another_token_is_rejected() {
    let server = LocalJmap::start(store(), TOKEN).await;

    let result = connect(&server, "fake-token-2").await;

    assert_eq!(result.err(), Some(Error::Status(401)));
}

#[tokio::test]
async fn a_session_without_mail_is_refused() {
    let server = LocalJmap::start_without_mail(TOKEN).await;

    let result = connect(&server, TOKEN).await;

    assert_eq!(result.err(), Some(Error::NoMailCapability));
}

#[tokio::test]
async fn mailbox_get_returns_the_stores_mailboxes() {
    let server = LocalJmap::start(store(), TOKEN).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let mailboxes = jmap.get_mailboxes().await.unwrap();

    assert_eq!(
        mailboxes,
        [
            Mailbox {
                id: "mb-inbox".into(),
                name: "Inbox".into(),
                parent_id: None,
                role: Some("inbox".into()),
            },
            Mailbox {
                id: "mb-travel".into(),
                name: "Travel".into(),
                parent_id: Some("mb-inbox".into()),
                role: None,
            },
        ]
    );
}

#[tokio::test]
async fn a_query_follows_the_back_references_to_every_email_of_each_thread() {
    let server = LocalJmap::start(store(), TOKEN).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap.query(&inbox(2)).await.unwrap();

    assert_eq!(result.ids, ["e1", "e2b"]);
    assert_eq!(result.total, 3);
    assert_eq!(
        result.threads,
        [
            Thread {
                id: "t1".into(),
                email_ids: vec!["e1".into()],
            },
            Thread {
                id: "t2".into(),
                email_ids: vec!["e2a".into(), "e2b".into()],
            },
        ]
    );
    let emails: Vec<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(emails, ["e1", "e2a", "e2b"]);
    let e1 = &result.emails[0];
    assert_eq!(e1.subject.as_deref(), Some("Lunch on Friday?"));
    assert_eq!(e1.from.as_ref().unwrap()[0].email, "ana@example.invalid");
    assert!(e1.mailbox_ids.contains_key("mb-inbox"));
}

/// `threads` threads in the Inbox, each of `per_thread` emails received a
/// second apart, and an empty Archive.
fn long_threads(threads: usize, per_thread: usize) -> Store {
    let start: jiff::Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive");
    for t in 0..threads {
        for e in 0..per_thread {
            let received = start + jiff::SignedDuration::from_secs((t * per_thread + e) as i64);
            builder = builder.email(
                EmailSpec::new(
                    &format!("e{t}-{e}"),
                    &format!("t{t}"),
                    &received.to_string(),
                )
                .in_mailbox("mb-inbox"),
            );
        }
    }
    builder.build()
}

/// The method calls of each request, as (name, arguments) pairs.
fn calls(requests: &[Value]) -> Vec<Vec<(&str, &Value)>> {
    requests
        .iter()
        .map(|request| {
            request["methodCalls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|call| (call[0].as_str().unwrap(), &call[1]))
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn the_session_advertises_fastmails_max_calls_in_request() {
    let server = LocalJmap::start(store(), TOKEN).await;

    let (_, session) = HttpJmap::connect(&server.session_url(), SecretString::from(TOKEN))
        .await
        .unwrap();

    assert_eq!(session.capabilities[CORE]["maxCallsInRequest"], 50);
}

#[tokio::test]
async fn a_request_with_more_calls_than_max_calls_in_request_is_refused() {
    let server = LocalJmap::start_with_max_calls(store(), TOKEN, 3).await;
    let call = json!(["Mailbox/get", {"accountId": ACCOUNT_ID, "ids": null}, "0"]);

    let response = reqwest::Client::new()
        .post(server.api_url())
        .bearer_auth(TOKEN)
        .json(&json!({
            "using": [CORE, MAIL],
            "methodCalls": [call, call, call, call]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({
            "type": "urn:ietf:params:jmap:error:limit",
            "limit": "maxCallsInRequest",
            "status": 400
        })
    );
}

#[tokio::test]
async fn a_query_of_more_emails_than_max_objects_in_get_fetches_them_in_chunks() {
    let server = LocalJmap::start(long_threads(10, 410), TOKEN).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap.query(&inbox(500)).await.unwrap();

    assert_eq!(result.threads.len(), 10);
    assert_eq!(result.emails.len(), 4100);
    let requests = server.api_requests().await;
    let gets: Vec<usize> = calls(&requests)
        .into_iter()
        .flatten()
        .filter(|(name, args)| *name == "Email/get" && args["ids"].is_array())
        .map(|(_, args)| args["ids"].as_array().unwrap().len())
        .collect();
    assert_eq!(gets, [4096, 4]);
}

#[tokio::test]
async fn chunks_beyond_max_calls_in_request_go_in_further_requests() {
    let server = LocalJmap::start_with_max_calls(long_threads(13, 1000), TOKEN, 3).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap.query(&inbox(500)).await.unwrap();

    assert_eq!(result.threads.len(), 13);
    assert_eq!(result.emails.len(), 13_000);
    let requests = server.api_requests().await;
    let shapes: Vec<Vec<&str>> = calls(&requests)
        .into_iter()
        .map(|calls| calls.into_iter().map(|(name, _)| name).collect())
        .collect();
    assert_eq!(
        shapes,
        [
            vec!["Email/query", "Email/get", "Thread/get"],
            vec!["Email/get", "Email/get", "Email/get"],
            vec!["Email/get"],
        ]
    );
}

#[tokio::test]
async fn an_empty_queue_sends_only_the_first_request() {
    let server = LocalJmap::start(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .build(),
        TOKEN,
    )
    .await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap.query(&inbox(500)).await.unwrap();

    assert_eq!(result, Default::default());
    assert_eq!(server.api_requests().await.len(), 1);
}

#[tokio::test]
async fn the_session_advertises_fastmails_max_objects_in_set() {
    let server = LocalJmap::start(store(), TOKEN).await;

    let (_, session) = HttpJmap::connect(&server.session_url(), SecretString::from(TOKEN))
        .await
        .unwrap();

    assert_eq!(session.capabilities[CORE]["maxObjectsInSet"], 4096);
}

/// An `Email/set` update moving each of `ids` from the Inbox to Archive.
fn archive_all(ids: impl IntoIterator<Item = String>) -> EmailUpdate {
    let patch: EmailPatch = [
        ("mailboxIds/mb-inbox".to_string(), Value::Null),
        ("mailboxIds/mb-archive".to_string(), Value::Bool(true)),
    ]
    .into();
    ids.into_iter().map(|id| (id, patch.clone())).collect()
}

fn email_ids(threads: usize, per_thread: usize) -> Vec<String> {
    (0..threads)
        .flat_map(|t| (0..per_thread).map(move |e| format!("e{t}-{e}")))
        .collect()
}

#[tokio::test]
async fn an_email_set_of_more_than_max_objects_in_set_is_refused() {
    let server = LocalJmap::start(long_threads(1, 4097), TOKEN).await;
    let update = archive_all(email_ids(1, 4097));

    let response = reqwest::Client::new()
        .post(server.api_url())
        .bearer_auth(TOKEN)
        .json(&json!({
            "using": [CORE, MAIL],
            "methodCalls": [["Email/set", {"accountId": ACCOUNT_ID, "update": update}, "0"]]
        }))
        .send()
        .await
        .unwrap();

    let body = response.json::<Value>().await.unwrap();
    assert_eq!(
        body["methodResponses"],
        json!([["error", {"type": "requestTooLarge"}, "0"]])
    );
}

#[tokio::test]
async fn a_set_of_more_emails_than_max_objects_in_set_goes_in_chunks() {
    let server = LocalJmap::start(long_threads(1, 4100), TOKEN).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap
        .set_emails(&archive_all(email_ids(1, 4100)))
        .await
        .unwrap();

    assert_eq!(result.not_updated, Vec::<String>::new());
    let requests = server.api_requests().await;
    let sets: Vec<usize> = calls(&requests)
        .into_iter()
        .flatten()
        .filter(|(name, _)| *name == "Email/set")
        .map(|(_, args)| args["update"].as_object().unwrap().len())
        .collect();
    assert_eq!(sets, [4096, 4]);
    let archived = EmailQuery {
        filter: json!({"inMailbox": "mb-archive"}),
        ..inbox(500)
    };
    let result = jmap.query(&archived).await.unwrap();
    assert_eq!(result.emails.len(), 4100);
}

#[tokio::test]
async fn set_chunks_beyond_max_calls_in_request_go_in_further_requests() {
    let server = LocalJmap::start_with_max_calls(long_threads(1, 13_000), TOKEN, 3).await;
    let jmap = connect(&server, TOKEN).await.unwrap();
    let mut ids = email_ids(1, 13_000);
    ids.push("zz-gone".into());

    let result = jmap.set_emails(&archive_all(ids)).await.unwrap();

    assert_eq!(result.not_updated, ["zz-gone"]);
    let requests = server.api_requests().await;
    let shapes: Vec<Vec<&str>> = calls(&requests)
        .into_iter()
        .map(|calls| calls.into_iter().map(|(name, _)| name).collect())
        .collect();
    assert_eq!(
        shapes,
        [
            vec!["Email/set", "Email/set", "Email/set"],
            vec!["Email/set"],
        ]
    );
}
