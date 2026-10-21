//! The executor retries transient failures, goes disconnected when the
//! retries run out, probes on a backoff until a request succeeds, and
//! waits for the outbox on quit; run against the fake with paused time.

use std::io;
use std::time::Duration;

use blorp::jmap::Error;
use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::run::{Exit, TestClock, launch, run, unconfirmed};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
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

fn unreachable() -> Error {
    Error::Transport("connection refused".into())
}

/// Types each key `delay` ms after the last, then types nothing more.
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

type Ended = (io::Result<Exit>, Vec<String>);

/// Starts the state task over `jmap` with `keys`, the launch done. It
/// gives how it ended and the screen it left.
async fn start(jmap: &FakeJmap, keys: &'static [(u64, char)]) -> JoinHandle<Ended> {
    let app = launch(jmap, at("2026-10-18T10:00:00Z")).await.unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let jmap = jmap.clone();
    tokio::spawn(async move {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let exit = run(&mut terminal, script(keys), jmap, clock, app).await;
        (exit, screen(&terminal))
    })
}

fn screen(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn reconciles(jmap: &FakeJmap) -> usize {
    jmap.calls()
        .iter()
        .filter(|call| matches!(call, Call::Reconcile(..)))
        .count()
}

fn sets(jmap: &FakeJmap) -> usize {
    jmap.calls()
        .iter()
        .filter(|call| matches!(call, Call::SetEmails(_)))
        .count()
}

/// Sleeps until `ms` after the state task started.
async fn until(started: tokio::time::Instant, ms: u64) {
    tokio::time::sleep_until(started + Duration::from_millis(ms)).await;
}

#[tokio::test(start_paused = true)]
async fn a_retry_shows_on_the_bottom_line() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(60_500, 'q')]).await;
    tokio::time::sleep(Duration::from_millis(59_500)).await;
    jmap.fail_next(1, unreachable());

    let (exit, screen) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Quit);
    assert!(
        screen[23].trim_end().ends_with("Retrying Fastmail (1/3)"),
        "{screen:#?}"
    );
}

#[tokio::test(start_paused = true)]
async fn retries_that_run_out_disconnect_and_the_probe_backs_off_from_5_s() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[(75_000, 'q')]).await;
    until(started, 59_500).await;
    jmap.fail_next(10, unreachable());

    // The tick at 60 s and its retries 1, 2, and 4 s apart, each with up
    // to 500 ms of jitter, run out by 68.5 s.
    until(started, 60_100).await;
    assert_eq!(reconciles(&jmap), 1);
    until(started, 71_900).await;
    assert_eq!(reconciles(&jmap), 4);
    // The first probe, 5 s later, is sent once.
    until(started, 73_600).await;
    assert_eq!(reconciles(&jmap), 5);

    let (exit, screen) = state.await.unwrap();
    assert_eq!(
        exit.unwrap(),
        Exit::Quit,
        "quit is at once while disconnected"
    );
    assert!(screen[22].contains(" DISCONNECTED "), "{screen:#?}");
    assert_eq!(screen[23].trim_end(), "Disconnected from Fastmail");
}

#[tokio::test(start_paused = true)]
async fn the_probe_wait_doubles_to_a_minute() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[]).await;
    until(started, 59_500).await;
    jmap.fail_next(100, unreachable());

    // Out of retries by 68.5 s at the latest, then probes 5, 10, 20, 40,
    // and 60 s apart.
    let mut seen = Vec::new();
    for ms in [
        71_900, 73_600, 81_900, 83_600, 101_900, 103_600, 141_900, 143_600, 201_900, 203_600,
    ] {
        until(started, ms).await;
        seen.push(reconciles(&jmap));
    }

    assert_eq!(seen, [4, 5, 5, 6, 6, 7, 7, 8, 8, 9]);
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn the_first_probe_that_succeeds_reconciles_the_queue_and_reconnects() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[(90_000, 'q')]).await;
    until(started, 59_500).await;
    jmap.fail_next(5, unreachable());
    jmap.change(|store| {
        store.deliver(
            EmailSpec::new("e3", "t3", "2026-10-18T09:30:00Z")
                .in_mailbox("mb-inbox")
                .subject("Flights"),
        )
    });

    let (exit, screen) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Quit);
    assert_eq!(
        reconciles(&jmap),
        6,
        "four tries, a failed probe, and one that worked"
    );
    assert!(screen[1].contains("Flights"), "{screen:#?}");
    assert!(!screen[22].contains("DISCONNECTED"), "{screen:#?}");
    assert_eq!(screen[23].trim_end(), "Reconnected");
}

#[tokio::test(start_paused = true)]
async fn an_action_is_retried_inside_its_outbox_job() {
    let jmap = FakeJmap::new(inbox());
    let state = start(&jmap, &[(1_000, 'e'), (9_000, 'q')]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    jmap.fail_next(2, Error::Status(503));

    let (exit, screen) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Quit);
    assert_eq!(sets(&jmap), 3);
    assert!(screen[1].contains("Invoice"), "{screen:#?}");
    assert!(!screen[22].contains("DISCONNECTED"), "{screen:#?}");
}

#[tokio::test(start_paused = true)]
async fn retry_after_sets_the_wait() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[(1_000, 'e')]).await;
    until(started, 500).await;
    jmap.fail_next(
        1,
        Error::RetryAfter {
            status: 429,
            wait: Duration::from_secs(10),
        },
    );

    until(started, 10_900).await;
    assert_eq!(sets(&jmap), 1);
    until(started, 11_100).await;
    assert_eq!(sets(&jmap), 2);
    state.abort();
}

#[tokio::test(start_paused = true)]
async fn an_action_out_of_retries_drops_the_jobs_behind_it_until_reconnected() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    // Two archives, then once reconnected a third, which archives the
    // first thread again, since the first two were rolled back.
    let state = start(
        &jmap,
        &[(1_000, 'e'), (100, 'e'), (28_900, 'e'), (2_000, 'q')],
    )
    .await;
    until(started, 500).await;
    jmap.fail_next(4, unreachable());

    let (exit, screen) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Quit);
    let calls = jmap.calls();
    let sets: Vec<&Call> = calls
        .iter()
        .filter(|call| matches!(call, Call::SetEmails(_)))
        .collect();
    assert_eq!(
        sets.len(),
        5,
        "four tries of the first, none of the second, the third: {calls:?}"
    );
    assert!(
        sets.iter().all(|set| *set == sets[0]),
        "each archives e1: {sets:?}"
    );
    assert!(screen[1].contains("Invoice"), "{screen:#?}");
}

#[tokio::test(start_paused = true)]
async fn quit_waits_for_the_outbox() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[(1_000, 'e'), (10, 'q')]).await;
    until(started, 500).await;
    jmap.delay_next(1, Duration::from_secs(2));

    let (exit, _) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Quit);
    assert_eq!(
        jmap.answered()
            .iter()
            .filter(|c| matches!(c, Call::SetEmails(_)))
            .count(),
        1
    );
    let ended = started.elapsed();
    assert!(
        ended >= Duration::from_secs(3) && ended < Duration::from_millis(3100),
        "{ended:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn quit_gives_up_on_the_outbox_after_3_s() {
    let jmap = FakeJmap::new(inbox());
    let started = tokio::time::Instant::now();
    let state = start(&jmap, &[(1_000, 'e'), (10, 'q')]).await;
    until(started, 500).await;
    jmap.delay_next(1, Duration::from_secs(10));

    let (exit, _) = state.await.unwrap();

    assert_eq!(exit.unwrap(), Exit::Unconfirmed(1));
    let ended = started.elapsed();
    assert!(
        ended >= Duration::from_millis(4010) && ended < Duration::from_millis(4100),
        "{ended:?}"
    );
}

#[test]
fn the_unconfirmed_line_counts_one_action_or_several() {
    assert_eq!(
        unconfirmed(1),
        "1 action was not confirmed by Fastmail; check the Inbox."
    );
    assert_eq!(
        unconfirmed(3),
        "3 actions were not confirmed by Fastmail; check the Inbox."
    );
}
