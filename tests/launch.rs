use std::io;
use std::time::Duration;

use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::jmap::{EmailQuery, EmailUpdate, Error, Fatal};
use blorp::run::{Exit, TestClock, connect, launch, run};
use blorp::snooze::{Entry, Schedule};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .from("Kim Park", "kim@example.invalid")
                .subject("Trip to Wellington"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T07:00:00Z")
                .in_mailbox("mb-travel")
                .from("Ravi Shah", "ravi@example.invalid")
                .subject("Flights"),
        )
        .build()
}

/// Types each key one virtual second after the last.
fn script(keys: &'static str, gap: Duration) -> impl Stream<Item = io::Result<Event>> + Unpin {
    Box::pin(stream::iter(keys.chars()).then(move |c| async move {
        tokio::time::sleep(gap).await;
        Ok(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    }))
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

#[tokio::test(start_paused = true)]
async fn launch_loads_the_inbox_and_the_keys_move_through_it() {
    let jmap = FakeJmap::new(inbox());
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));
    let app = launch(&jmap, at("2026-10-18T00:00:00Z")).await.unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    run(
        &mut terminal,
        script("jq", Duration::from_secs(1)),
        jmap.clone(),
        clock,
        app,
    )
    .await
    .unwrap();

    assert_eq!(
        jmap.calls(),
        [
            Call::GetMailboxes,
            Call::Query(EmailQuery {
                filter: json!({"inMailbox": "mb-inbox"}),
                ascending: false,
                limit: 500,
                extra_threads: Vec::new(),
            }),
        ]
    );
    let screen = screen(&terminal);
    assert!(screen[0].starts_with(" Inbox  Unread "), "{screen:#?}");
    assert!(screen[1].contains("Lunch on Friday?"), "{screen:#?}");
    assert!(
        screen[1].trim_end().ends_with("1h"),
        "ages come from the clock: {screen:#?}"
    );
    assert!(screen[2].contains("Trip to Wellington"), "{screen:#?}");
    assert!(
        !screen.iter().any(|line| line.contains("Flights")),
        "{screen:#?}"
    );
    assert!(screen[22].contains(" NORMAL "), "{screen:#?}");
    assert!(screen[22].trim_end().ends_with("2/2"), "{screen:#?}");
}

#[tokio::test(start_paused = true)]
async fn a_failed_initial_query_is_reported_on_the_bottom_line() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T00:00:00Z")).await.unwrap();
    jmap.fail_next(1, Error::Status(400));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));

    run(
        &mut terminal,
        script("q", Duration::from_secs(1)),
        jmap,
        clock,
        app,
    )
    .await
    .unwrap();

    let screen = screen(&terminal);
    assert_eq!(screen[23].trim_end(), "Fastmail error: HTTP 400");
}

#[tokio::test(start_paused = true)]
async fn the_spinner_ticks_every_250_ms_while_the_query_is_in_flight() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T00:00:00Z")).await.unwrap();
    jmap.delay_next(1, Duration::from_secs(5));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));

    run(
        &mut terminal,
        script("q", Duration::from_millis(900)),
        jmap,
        clock,
        app,
    )
    .await
    .unwrap();

    let screen = screen(&terminal);
    assert_eq!(screen[23].trim_end(), "⠸ Loading Inbox");
}

#[tokio::test(start_paused = true)]
async fn launch_retries_mailbox_get_after_1_2_and_4_s() {
    let jmap = FakeJmap::new(inbox());
    jmap.fail_next(3, Error::Status(503));
    let started = tokio::time::Instant::now();

    let result = launch(&jmap, at("2026-10-18T00:00:00Z")).await;

    assert!(result.is_ok());
    assert_eq!(jmap.calls(), vec![Call::GetMailboxes; 4]);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(7) && waited <= Duration::from_millis(8500),
        "{waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn launch_fails_when_mailbox_get_fails_three_retries_later() {
    let jmap = FakeJmap::new(inbox());
    jmap.fail_next(4, Error::Status(500));

    let result = launch(&jmap, at("2026-10-18T00:00:00Z")).await;

    assert_eq!(result.err(), Some(Error::Status(500)));
    assert_eq!(jmap.calls().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn launch_fails_at_once_on_a_bug() {
    let jmap = FakeJmap::new(inbox());
    jmap.fail_next(1, Error::Status(400));

    let result = launch(&jmap, at("2026-10-18T00:00:00Z")).await;

    assert_eq!(result.err(), Some(Error::Status(400)));
    assert_eq!(jmap.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_rejected_token_stops_the_state_task_with_the_fatal_exit() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T00:00:00Z")).await.unwrap();
    jmap.fail_next(1, Error::Status(401));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));

    let exit = run(
        &mut terminal,
        script("jjjq", Duration::from_secs(1)),
        jmap,
        clock,
        app,
    )
    .await
    .unwrap();

    assert_eq!(exit, Exit::Fatal(Fatal::TokenRejected));
}

#[tokio::test(start_paused = true)]
async fn q_stops_the_state_task_with_the_quit_exit() {
    let jmap = FakeJmap::new(inbox());
    let app = launch(&jmap, at("2026-10-18T00:00:00Z")).await.unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let clock = TestClock::new(at("2026-10-18T10:00:00Z"));

    let exit = run(
        &mut terminal,
        script("q", Duration::from_secs(1)),
        jmap,
        clock,
        app,
    )
    .await
    .unwrap();

    assert_eq!(exit, Exit::Quit);
}

#[tokio::test(start_paused = true)]
async fn launch_runs_from_the_session_fetch_to_the_initial_query_and_the_due_snooze_return() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blorp/snooze.json");
    let fake = FakeJmap::new(
        Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .mailbox("mb-snooze", "Snoozed (blorp)")
            .email(
                EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                    .in_mailbox("mb-inbox")
                    .seen()
                    .subject("Lunch on Friday?"),
            )
            .email(
                EmailSpec::new("e3", "t3", "2026-10-01T07:00:00Z")
                    .in_mailbox("mb-snooze")
                    .seen()
                    .subject("Renewal"),
            )
            .build(),
    );
    let mut schedule = Schedule::default();
    schedule.set(
        "t3",
        Some(Entry {
            emails: [("e3".into(), "mb-inbox".into())].into(),
            snoozed_at: at("2026-10-10T09:00:00Z"),
            return_at: at("2026-10-18T07:00:00Z"),
            returned_at: None,
        }),
    );
    fake.fail_next(1, Error::Status(503));
    let started = tokio::time::Instant::now();

    let jmap = connect(|| fake.connect()).await.unwrap();
    let app = launch(&jmap, at("2026-10-18T12:00:00Z"))
        .await
        .unwrap()
        .with_time_zone(TimeZone::UTC)
        .with_schedule(file, schedule);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    run(
        &mut terminal,
        script("q", Duration::from_secs(1)),
        jmap,
        TestClock::new(at("2026-10-18T12:00:00Z")),
        app,
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the session fetch is retried after 1 s: {:?}",
        started.elapsed()
    );
    let query = |mailbox: &str, extra_threads: &[&str]| {
        Call::Query(EmailQuery {
            filter: json!({"inMailbox": mailbox}),
            ascending: false,
            limit: 500,
            extra_threads: extra_threads.iter().map(|t| t.to_string()).collect(),
        })
    };
    let returned = EmailUpdate::from([(
        "e3".to_string(),
        [
            ("mailboxIds/mb-snooze".to_string(), Value::Null),
            ("mailboxIds/mb-inbox".to_string(), Value::Bool(true)),
            ("keywords/$seen".to_string(), Value::Null),
        ]
        .into(),
    )]);
    assert_eq!(
        fake.calls(),
        [
            Call::Session,
            Call::Session,
            Call::GetMailboxes,
            query("mb-inbox", &[]),
            query("mb-snooze", &["t3"]),
            Call::SetEmails(returned),
        ]
    );
    let screen = screen(&terminal);
    assert!(screen[1].contains("Renewal"), "{screen:#?}");
    assert!(screen[2].contains("Lunch on Friday?"), "{screen:#?}");
}
