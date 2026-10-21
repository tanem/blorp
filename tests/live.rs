//! The live smoke tests against Fastmail. Ignored by default; run with
//! `BLORP_API_TOKEN` set and `cargo test --test live -- --ignored`. They
//! read nothing beyond the session, the mailbox list, and the Inbox query,
//! write nothing, and assert on shapes only. Failure messages leave out
//! mail content.

use std::collections::HashSet;

use blorp::jmap::http::FASTMAIL_SESSION_URL;
use blorp::jmap::{CORE, EmailQuery, HttpJmap, Jmap};
use blorp::mail::Mailboxes;
use secrecy::SecretString;
use serde_json::json;

mod e2e;

use e2e::{Modes, Sandbox, statusline};

fn token() -> String {
    std::env::var("BLORP_API_TOKEN").expect("BLORP_API_TOKEN is set")
}

#[tokio::test]
#[ignore = "talks to Fastmail; needs BLORP_API_TOKEN"]
async fn session_mailboxes_and_an_inbox_query_have_the_expected_shapes() {
    let (jmap, session) = HttpJmap::connect(FASTMAIL_SESSION_URL, SecretString::from(token()))
        .await
        .expect("session with the mail capability");
    assert!(session.mail_account().is_some());
    for limit in ["maxObjectsInGet", "maxCallsInRequest"] {
        // The message leaves out the value, which comes from the live session.
        assert!(
            session.capabilities[CORE][limit]
                .as_u64()
                .is_some_and(|n| n > 0),
            "the core capability advertises {limit} as a positive integer"
        );
    }

    let mailboxes = Mailboxes::from(jmap.get_mailboxes().await.expect("Mailbox/get"));
    let inbox = mailboxes.inbox().expect("a mailbox with the inbox role");

    let result = jmap
        .query(&EmailQuery {
            filter: json!({"inMailbox": inbox.id}),
            limit: 500,
        })
        .await
        .expect("the two-stage Inbox query");

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

/// The cursor row and the queue length from a statusline such as
/// ` Inbox    1/500 of 6443`.
fn position(statusline: &str) -> Option<(usize, usize)> {
    let words: Vec<&str> = statusline.split_whitespace().collect();
    let position = match words.as_slice() {
        [.., position, "of", _total] => position,
        [.., position] => position,
        [] => return None,
    };
    let (row, len) = position.split_once('/')?;
    Some((row.parse().ok()?, len.parse().ok()?))
}

#[test]
fn position_reads_the_row_and_length_with_or_without_a_total() {
    assert_eq!(position(" Inbox      1/500 of 6443"), Some((1, 500)));
    assert_eq!(position(" Inbox      2/37"), Some((2, 37)));
    assert_eq!(position(" Inbox"), None);
}

#[test]
#[ignore = "talks to Fastmail; needs BLORP_API_TOKEN"]
fn the_inbox_opens_in_a_terminal_moves_and_quits_cleanly() {
    let sandbox = Sandbox::new().env("BLORP_API_TOKEN", &token()).redacted();
    let mut blorp = sandbox.spawn(&[], 80, 24);

    blorp.wait_for("the statusline to show the loaded queue", |screen| {
        position(&statusline(screen)).is_some_and(|(row, _)| row == 1)
    });
    let (_, len) = position(&blorp.statusline()).unwrap();
    if len > 1 {
        blorp.keys("j");
        blorp.wait_for("the cursor on row 2", |screen| {
            position(&statusline(screen)).is_some_and(|(row, _)| row == 2)
        });
    }
    blorp.keys("q");

    assert_eq!(blorp.wait_exit().exit_code(), 0);
    assert!(!blorp.on_alternate_screen());
    assert_eq!(blorp.modes(), Modes::COOKED);
    // Not assert_eq!, which would print a stored token.
    assert!(sandbox.stored_token().is_none(), "nothing was stored");
}
