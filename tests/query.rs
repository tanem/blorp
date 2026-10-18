//! The executor aborts a queue query that a newer one supersedes.

use std::io;
use std::time::Duration;

use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::run::{TestClock, launch, run};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
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

fn text_filter(text: &str) -> Value {
    json!({"operator": "AND", "conditions": [{"inMailbox": "mb-inbox"}, {"text": text}]})
}

fn query_filters(calls: Vec<Call>) -> Vec<Value> {
    calls
        .into_iter()
        .filter_map(|call| match call {
            Call::Query(query) => Some(query.filter),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_newer_query_aborts_the_one_it_supersedes() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("ab"),
        )
        .build();
    let jmap = FakeJmap::new(store);
    let app = launch(&jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let state = tokio::spawn({
        let jmap = jmap.clone();
        async move {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            run(
                &mut terminal,
                script("/ab", Duration::from_secs(1)),
                jmap,
                clock,
                app,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.delay_next(1, Duration::from_secs(10));

    tokio::time::sleep(Duration::from_secs(30)).await;

    let inbox = json!({"inMailbox": "mb-inbox"});
    assert_eq!(
        query_filters(jmap.calls()),
        [inbox.clone(), text_filter("a"), text_filter("ab")]
    );
    assert_eq!(
        query_filters(jmap.answered()),
        [inbox, text_filter("ab")],
        "the query for \"a\" was aborted"
    );
    state.abort();
}
