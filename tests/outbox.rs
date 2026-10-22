//! The outbox sends archives one at a time, in the order typed, and `q`
//! waits for it.

use std::io;
use std::time::Duration;

use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::jmap::{EmailUpdate, Error};
use blorp::run::{Exit, TestClock, launch, run, unconfirmed};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::Value;
use tokio::task::JoinHandle;

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

/// Types each key the given number of seconds after the last, then types
/// nothing more.
fn script(keys: &'static [(u64, char)]) -> impl Stream<Item = io::Result<Event>> + Unpin {
    let keys = stream::iter(keys).then(|&(secs, c)| async move {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        Ok(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    });
    Box::pin(keys.chain(stream::pending()))
}

/// Launches blorp on `jmap` and runs it with `keys` typed as [`script`]
/// types them.
async fn start(jmap: &FakeJmap, keys: &'static [(u64, char)]) -> JoinHandle<io::Result<Exit>> {
    let app = launch(jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let jmap = jmap.clone();
    tokio::spawn(async move {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        run(&mut terminal, script(keys), jmap, clock, app).await
    })
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
async fn the_outbox_sends_one_archive_at_a_time_in_the_order_typed() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(1, 'e'), (1, 'e')]).await;
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

#[tokio::test(start_paused = true)]
async fn an_archive_lost_despite_its_retries_drops_the_one_behind_it() {
    let jmap = FakeJmap::new(inbox());
    // The first archive is lost by about 9.5 s, and the probe 5 s later
    // reconnects, so the third goes out.
    let state = start(&jmap, &[(1, 'e'), (1, 'e'), (20, 'e')]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.fail_next(4, Error::Transport("connection reset".into()));

    tokio::time::sleep(Duration::from_secs(30)).await;

    assert_eq!(
        sets(&jmap),
        vec![archive("e1"); 5],
        "sent and retried 3 times, then sent again after the probe; e2's was dropped"
    );
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn q_quits_once_the_archive_in_flight_is_confirmed() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(1, 'e'), (1, 'q')]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.delay_next(1, Duration::from_millis(1500));
    let started = tokio::time::Instant::now();

    let exit = state.await.unwrap().unwrap();

    assert_eq!(exit, Exit::Quit);
    assert_eq!(sets(&jmap), [archive("e1")]);
    let waited = tokio::time::Instant::now() - started;
    assert_eq!(
        waited,
        Duration::from_secs(2),
        "e at 1 s, q at 2 s, the archive answered at 2.5 s"
    );
}

#[tokio::test(start_paused = true)]
async fn q_quits_after_3_s_with_the_archive_in_flight_unconfirmed() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(1, 'e'), (1, 'q')]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.delay_next(1, Duration::from_secs(60));
    let started = tokio::time::Instant::now();

    let exit = state.await.unwrap().unwrap();

    assert_eq!(exit, Exit::Unconfirmed(1));
    let waited = tokio::time::Instant::now() - started;
    assert!(
        waited >= Duration::from_millis(4500) && waited < Duration::from_secs(5),
        "two keys a second apart, then the 3 s wait: {waited:?}"
    );
}

#[test]
fn the_unconfirmed_line_counts_the_actions() {
    assert_eq!(
        unconfirmed(1),
        "1 action was not confirmed by Fastmail; check the Inbox."
    );
    assert_eq!(
        unconfirmed(2),
        "2 actions were not confirmed by Fastmail; check the Inbox."
    );
}
