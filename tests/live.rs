//! The live smoke test against Fastmail. Ignored by default; run with
//! `BLORP_API_TOKEN` set and `cargo test --test live -- --ignored`. It reads
//! and writes nothing beyond the session, the mailbox list, and one Inbox
//! query, and asserts on shapes only.

use std::collections::HashSet;

use blorp::jmap::http::FASTMAIL_SESSION_URL;
use blorp::jmap::{EmailQuery, HttpJmap, Jmap};
use blorp::mail::{Mailboxes, Role};
use secrecy::SecretString;
use serde_json::json;

#[tokio::test]
#[ignore = "talks to Fastmail; needs BLORP_API_TOKEN"]
async fn session_mailboxes_and_an_inbox_query_have_the_expected_shapes() {
    let token = std::env::var("BLORP_API_TOKEN").expect("BLORP_API_TOKEN is set");

    let (jmap, session) = HttpJmap::connect(FASTMAIL_SESSION_URL, SecretString::from(token))
        .await
        .expect("session with the mail capability");
    assert!(session.mail_account().is_some());

    let mailboxes = Mailboxes::from(jmap.get_mailboxes().await.expect("Mailbox/get"));
    let inbox = mailboxes
        .by_role(Role::Inbox)
        .expect("a mailbox with the inbox role");

    let result = jmap
        .query(&EmailQuery {
            filter: json!({"inMailbox": inbox.id}),
            ascending: false,
        })
        .await
        .expect("Email/query chain");

    let emails: HashSet<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
    let threads: HashSet<&str> = result.emails.iter().map(|e| e.thread_id.as_str()).collect();
    assert_eq!(
        result.threads.len(),
        result.ids.len(),
        "one thread per collapsed hit"
    );
    assert_eq!(
        threads.len(),
        result.ids.len(),
        "collapsed hits are in distinct threads"
    );
    for id in &result.ids {
        assert!(emails.contains(id.as_str()), "hit {id} is among the emails");
    }
    for thread in &result.threads {
        for id in &thread.email_ids {
            assert!(
                emails.contains(id.as_str()),
                "thread email {id} was fetched"
            );
        }
    }
}
