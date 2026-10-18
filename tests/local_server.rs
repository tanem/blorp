//! The reqwest adapter against the local JMAP server the end-to-end tests
//! use, which serves a fake store.

use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{CORE, EmailQuery, Error, HttpJmap, Jmap, Mailbox, Thread};
use secrecy::SecretString;
use serde_json::json;

mod e2e;

use e2e::TOKEN;
use e2e::server::LocalJmap;

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

#[tokio::test]
async fn a_query_fetching_more_than_max_objects_in_get_is_too_large() {
    let mut builder = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
    for e in 0..4097 {
        let received = format!("2026-10-18T{:02}:{:02}:00Z", e / 60 % 24, e % 60);
        builder =
            builder.email(EmailSpec::new(&format!("e{e}"), "t1", &received).in_mailbox("mb-inbox"));
    }
    let server = LocalJmap::start(builder.build(), TOKEN).await;
    let jmap = connect(&server, TOKEN).await.unwrap();

    let result = jmap.query(&inbox(500)).await;

    assert_eq!(
        result.err(),
        Some(Error::Method {
            method: "Email/get".into(),
            kind: "requestTooLarge".into(),
        })
    );
}
