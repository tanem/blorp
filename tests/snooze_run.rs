//! Snooze and return through the state task, the executor, and the schedule
//! file, against the fake.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use blorp::app::App;
use blorp::jmap::fake::{Call, EmailSpec, FakeJmap, Store};
use blorp::jmap::{EmailQuery, EmailUpdate};
use blorp::run::{TestClock, launch, run};
use blorp::snooze::{self, Entry, Schedule};
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

const NOW: &str = "2026-10-18T12:00:00Z";

fn store(snooze_mailbox: bool) -> Store {
    let mut builder = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Trip to Wellington"),
        );
    if snooze_mailbox {
        builder = builder.mailbox("mb-snooze", "Snoozed (blorp)").email(
            EmailSpec::new("e3", "t3", "2026-10-01T07:00:00Z")
                .in_mailbox("mb-snooze")
                .seen()
                .subject("Renewal"),
        );
    }
    builder.build()
}

/// A schedule in which t3, in the snooze mailbox, returns next Sunday.
fn snoozed_t3() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set(
        "t3",
        Some(Entry {
            emails: [("e3".into(), "mb-inbox".into())].into(),
            snoozed_at: at("2026-10-10T09:00:00Z"),
            return_at: at("2026-10-25T08:00:00Z"),
            returned_at: None,
        }),
    );
    schedule
}

/// Types each key one virtual second after the last, then waits a second.
fn script(keys: &'static str) -> impl Stream<Item = io::Result<Event>> + Unpin {
    let keys = keys.chars().map(Some).chain([None]);
    Box::pin(stream::iter(keys).then(|c| async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let code = c.map_or(KeyCode::Null, KeyCode::Char);
        Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
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

async fn start(jmap: &FakeJmap, file: &Path, schedule: Schedule) -> App {
    launch(jmap, at(NOW))
        .await
        .unwrap()
        .with_time_zone(TimeZone::UTC)
        .with_schedule(file.to_path_buf(), schedule)
}

fn written(file: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
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

fn update(entries: &[(&str, Value)]) -> EmailUpdate {
    entries
        .iter()
        .map(|(id, patch)| {
            (
                id.to_string(),
                serde_json::from_value(patch.clone()).unwrap(),
            )
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_snooze_writes_the_schedule_file_and_sends_the_move() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blorp/snooze.json");
    let jmap = FakeJmap::new(store(true));
    let app = start(&jmap, &file, snoozed_t3()).await;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    run(
        &mut terminal,
        script("zt"),
        jmap.clone(),
        TestClock::new(at(NOW)),
        app,
    )
    .await
    .unwrap();

    assert_eq!(
        written(&file)["threads"]["t1"],
        json!({
            "emails": {"e1": "mb-inbox"},
            "snoozedAt": "2026-10-18T12:00:00+00:00",
            "returnAt": "2026-10-19T08:00:00+00:00",
            "returnedAt": null
        })
    );
    assert_eq!(
        sets(&jmap),
        [update(&[(
            "e1",
            json!({"mailboxIds/mb-inbox": null, "mailboxIds/mb-snooze": true})
        )])]
    );
    let screen = screen(&terminal);
    assert!(screen[0].contains("Trip to Wellington"), "{screen:#?}");
    assert_eq!(screen[23].trim_end(), "Snoozed 1 thread until Mon 08:00");
}

#[tokio::test(start_paused = true)]
async fn a_schedule_write_that_fails_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("blorp");
    fs::write(&state, "a file where the state directory should be").unwrap();
    let file = state.join("snooze.json");
    let jmap = FakeJmap::new(store(true));
    let app = start(&jmap, &file, snoozed_t3()).await;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    run(
        &mut terminal,
        script("zt"),
        jmap.clone(),
        TestClock::new(at(NOW)),
        app,
    )
    .await
    .unwrap();

    assert_eq!(sets(&jmap), Vec::<EmailUpdate>::new());
    assert!(!file.exists());
    let screen = screen(&terminal);
    assert!(screen[0].contains("Lunch on Friday?"), "{screen:#?}");
    assert!(
        screen[23].starts_with("Could not snooze 1 thread: could not write the snooze schedule: "),
        "{screen:#?}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_first_snooze_creates_the_snooze_mailbox_before_it_moves_anything() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blorp/snooze.json");
    let jmap = FakeJmap::new(store(false));
    let app = start(&jmap, &file, Schedule::default()).await;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    run(
        &mut terminal,
        script("zt"),
        jmap.clone(),
        TestClock::new(at(NOW)),
        app,
    )
    .await
    .unwrap();

    let calls: Vec<Call> = jmap
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::CreateMailbox(_) | Call::SetEmails(_)))
        .collect();
    assert_eq!(calls[0], Call::CreateMailbox("Snoozed (blorp)".into()));
    let Call::SetEmails(sent) = &calls[1] else {
        panic!("{calls:?}");
    };
    assert_eq!(sent["e1"]["mailboxIds/mb-created-1"], json!(true));
    assert_eq!(
        written(&file)["threads"]["t1"]["emails"],
        json!({"e1": "mb-inbox"})
    );
}

#[tokio::test(start_paused = true)]
async fn a_due_thread_returns_at_launch_after_the_inbox_loads() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blorp/snooze.json");
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
    snooze::save(&file, &schedule.to_json(&TimeZone::UTC)).unwrap();
    let jmap = FakeJmap::new(store(true));
    let loaded = snooze::load(&file, at(NOW)).unwrap();
    let app = start(&jmap, &file, loaded.schedule).await;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    run(
        &mut terminal,
        script(""),
        jmap.clone(),
        TestClock::new(at(NOW)),
        app,
    )
    .await
    .unwrap();

    let query = |mailbox: &str, extra_threads: &[&str]| {
        Call::Query(EmailQuery {
            filter: json!({"inMailbox": mailbox}),
            ascending: false,
            limit: 500,
            extra_threads: extra_threads.iter().map(|t| t.to_string()).collect(),
        })
    };
    assert_eq!(
        jmap.calls()[..4],
        [
            Call::GetMailboxes,
            query("mb-inbox", &[]),
            query("mb-snooze", &["t3"]),
            Call::SetEmails(update(&[(
                "e3",
                json!({
                    "mailboxIds/mb-snooze": null,
                    "mailboxIds/mb-inbox": true,
                    "keywords/$seen": null
                })
            )])),
        ]
    );
    assert_eq!(
        written(&file)["threads"]["t3"]["returnedAt"],
        "2026-10-18T12:00:00+00:00"
    );
    let screen = screen(&terminal);
    assert!(screen[0].contains("Renewal"), "{screen:#?}");
    assert!(screen[0].contains('●'), "returned unread: {screen:#?}");
    assert_eq!(screen[23].trim_end(), "Returned 1 thread from snooze");
}

#[tokio::test(start_paused = true)]
async fn a_thread_that_falls_due_while_blorp_runs_returns_on_the_reconcile_tick() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blorp/snooze.json");
    let mut schedule = snoozed_t3();
    let mut entry = schedule.get("t3").unwrap().clone();
    entry.return_at = at("2026-10-18T12:00:30Z");
    schedule.set("t3", Some(entry));
    let jmap = FakeJmap::new(store(true));
    let app = start(&jmap, &file, schedule).await;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let clock = TestClock::new(at(NOW));
    // The clock moves past the return time 45 s in; the tick comes at 60 s,
    // and the keys end at 75 s.
    let keys = {
        let clock = clock.clone();
        Box::pin(stream::once(async move {
            tokio::time::sleep(Duration::from_secs(45)).await;
            clock.set(at("2026-10-18T12:00:45Z"));
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(Event::Key(KeyEvent::new(KeyCode::Null, KeyModifiers::NONE)))
        }))
    };

    run(&mut terminal, keys, jmap.clone(), clock, app)
        .await
        .unwrap();

    let calls: Vec<Call> = jmap
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Reconcile(..) | Call::SetEmails(_)))
        .collect();
    let query = |mailbox: &str, extra_threads: &[&str]| EmailQuery {
        filter: json!({"inMailbox": mailbox}),
        ascending: false,
        limit: 500,
        extra_threads: extra_threads.iter().map(|t| t.to_string()).collect(),
    };
    assert_eq!(
        calls,
        [
            Call::Reconcile(query("mb-inbox", &[]), Some(query("mb-snooze", &["t3"]))),
            Call::SetEmails(update(&[(
                "e3",
                json!({
                    "mailboxIds/mb-snooze": null,
                    "mailboxIds/mb-inbox": true,
                    "keywords/$seen": null
                })
            )])),
        ]
    );
    assert_eq!(
        written(&file)["threads"]["t3"]["returnedAt"],
        "2026-10-18T12:00:45+00:00"
    );
    let screen = screen(&terminal);
    assert!(screen[0].contains("Renewal"), "{screen:#?}");
    assert_eq!(screen[23].trim_end(), "Returned 1 thread from snooze");
}
