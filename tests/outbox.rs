//! The outbox sends actions one at a time, in the order the user acted.

use std::io;
use std::time::Duration;

use blorp::jmap::EmailUpdate;
use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::run::{TestClock, launch, run};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::Value;

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z").in_mailbox("mb-inbox"))
        .email(EmailSpec::new("e2", "t2", "2026-10-18T08:00:00Z").in_mailbox("mb-inbox"))
        .build()
}

/// Types each key `gap` after the last, then types nothing more.
fn script(keys: &'static str, gap: Duration) -> impl Stream<Item = io::Result<Event>> + Unpin {
    let keys = stream::iter(keys.chars()).then(move |c| async move {
        tokio::time::sleep(gap).await;
        Ok(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    });
    Box::pin(keys.chain(stream::pending()))
}

fn archive(id: &str) -> EmailUpdate {
    EmailUpdate::from([(
        id.to_string(),
        [
            ("mailboxIds/mb-inbox".to_string(), Value::Null),
            ("mailboxIds/mb-archive".to_string(), Value::Bool(true)),
        ]
        .into(),
    )])
}

fn sets(jmap: &FakeJmap) -> Vec<EmailUpdate> {
    jmap.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::SetEmails(update) => Some(update),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn the_outbox_sends_one_job_at_a_time_in_the_order_acted() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let state = tokio::spawn({
        let jmap = jmap.clone();
        async move {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            run(
                &mut terminal,
                script("ee", Duration::from_secs(1)),
                jmap,
                clock,
                app,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.delay_next(1, Duration::from_secs(10));

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        sets(&jmap),
        [archive("e1")],
        "the second archive waits for the first"
    );

    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(sets(&jmap), [archive("e1"), archive("e2")]);
    state.abort();
}

/// The calls after launch's query: the outbox's gets and sets.
fn jobs(jmap: &FakeJmap) -> Vec<Call> {
    jmap.calls()
        .into_iter()
        .filter(|call| matches!(call, Call::GetEmails(_) | Call::SetEmails(_)))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_undo_waits_for_its_action_then_gets_before_it_sets() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let state = tokio::spawn({
        let jmap = jmap.clone();
        async move {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            run(
                &mut terminal,
                script("eu", Duration::from_secs(1)),
                jmap,
                clock,
                app,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.delay_next(1, Duration::from_secs(10));

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        jobs(&jmap),
        [Call::SetEmails(archive("e1"))],
        "the undo waits for the archive"
    );

    tokio::time::sleep(Duration::from_secs(10)).await;
    let undo = EmailUpdate::from([(
        "e1".to_string(),
        [
            ("mailboxIds/mb-archive".to_string(), Value::Null),
            ("mailboxIds/mb-inbox".to_string(), Value::Bool(true)),
        ]
        .into(),
    )]);
    assert_eq!(
        jobs(&jmap),
        [
            Call::SetEmails(archive("e1")),
            Call::GetEmails(vec!["e1".into()]),
            Call::SetEmails(undo),
        ]
    );
    state.abort();
}
