//! The state task's reconcile tick and the `sync` task's push stream, run
//! against the fake with paused time.

use std::io;
use std::time::Duration;

use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::jmap::{EmailQuery, Push};
use blorp::run::{Exit, TestClock, launch, run};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use tokio::task::JoinHandle;

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Invoice"),
        )
        .build()
}

fn inbox_query() -> EmailQuery {
    EmailQuery {
        filter: json!({"inMailbox": "mb-inbox"}),
        ascending: false,
        limit: 500,
    }
}

/// Types each key after `delay`, then types nothing more.
fn script(keys: &'static [(u64, char)]) -> impl Stream<Item = io::Result<Event>> + Unpin {
    let keys = stream::iter(keys).then(|&(delay, c)| async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        Ok(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    });
    Box::pin(keys.chain(stream::pending()))
}

/// Starts the state task over `jmap` with `keys`, the launch done.
async fn start(jmap: &FakeJmap, keys: &'static [(u64, char)]) -> JoinHandle<io::Result<Exit>> {
    let app = launch(jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let jmap = jmap.clone();
    tokio::spawn(async move {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        run(&mut terminal, script(keys), jmap, clock, app).await
    })
}

fn reconciles(jmap: &FakeJmap) -> usize {
    jmap.calls()
        .iter()
        .filter(|call| matches!(call, Call::Reconcile(_)))
        .count()
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[tokio::test(start_paused = true)]
async fn a_reconcile_runs_every_minute() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[]).await;

    sleep_ms(59_500).await;
    assert_eq!(reconciles(&jmap), 0);
    sleep_ms(1_000).await;
    assert_eq!(reconciles(&jmap), 1);
    sleep_ms(60_000).await;
    assert_eq!(reconciles(&jmap), 2);
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn keys_do_not_put_the_tick_off() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(20_000, 'j'), (20_000, 'k'), (15_000, 'j')]).await;

    sleep_ms(60_500).await;

    assert_eq!(reconciles(&jmap), 1);
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn each_push_event_reconciles_at_once() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[]).await;
    sleep_ms(500).await;
    assert_eq!(jmap.streams_opened(), 1);

    jmap.push(Push::State);
    sleep_ms(10).await;
    assert_eq!(reconciles(&jmap), 1);

    jmap.push(Push::Ping);
    sleep_ms(10).await;
    assert_eq!(
        jmap.calls()[2..],
        [
            Call::Reconcile(inbox_query()),
            Call::Reconcile(inbox_query())
        ]
    );
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn a_dropped_stream_is_opened_again_after_a_second() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[]).await;
    sleep_ms(500).await;

    jmap.close_streams();
    sleep_ms(900).await;
    assert_eq!(jmap.streams_opened(), 1);
    sleep_ms(700).await;
    assert_eq!(jmap.streams_opened(), 2);

    jmap.push(Push::State);
    sleep_ms(10).await;
    assert_eq!(reconciles(&jmap), 1, "the new stream wakes the state task");
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_will_not_open_is_tried_again_with_backoff() {
    let jmap = FakeJmap::new(inbox());
    jmap.refuse_streams(3);
    let state = start(&jmap, &[]).await;
    sleep_ms(10).await;
    assert_eq!(jmap.streams_opened(), 1);

    // Waits of 1, 2, and 4 s, each with up to 0.5 s of jitter.
    for (wait, opened) in [(1_600, 2), (2_500, 3), (4_500, 4)] {
        sleep_ms(wait).await;
        assert_eq!(jmap.streams_opened(), opened);
    }
    sleep_ms(30_000).await;
    assert_eq!(jmap.streams_opened(), 4, "the fourth stream stays open");
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn a_silent_stream_is_dead_after_330_seconds() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[]).await;
    sleep_ms(200_000).await;
    jmap.push(Push::Ping);

    sleep_ms(329_000).await;
    assert_eq!(jmap.streams_opened(), 1);
    sleep_ms(2_600).await;
    assert_eq!(jmap.streams_opened(), 2);
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn a_reconcile_that_spans_an_action_is_discarded_and_run_again() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(61_000, 'e')]).await;
    sleep_ms(59_500).await;
    jmap.delay_next(1, Duration::from_secs(5));

    sleep_ms(3_000).await;
    assert_eq!(reconciles(&jmap), 1);
    sleep_ms(5_000).await;

    let calls: Vec<Call> = jmap.calls()[2..].to_vec();
    assert!(
        matches!(
            calls[..],
            [Call::Reconcile(_), Call::SetEmails(_), Call::Reconcile(_)]
        ),
        "{calls:?}"
    );
    state.abort();
}
