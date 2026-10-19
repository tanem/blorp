//! The snooze schedule file: its shape, the atomic private write, and a
//! file that does not parse.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use blorp::snooze::{self, Entry, Schedule};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn auckland() -> TimeZone {
    TimeZone::posix("NZST-12NZDT,M9.5.0,M4.1.0/3").unwrap()
}

fn entry() -> Entry {
    Entry {
        emails: [
            ("e1".into(), "mb-inbox".into()),
            ("e2".into(), "mb-travel".into()),
        ]
        .into(),
        snoozed_at: at("2026-10-20T21:15:00Z"),
        return_at: at("2026-10-23T19:00:00Z"),
        returned_at: None,
    }
}

#[test]
fn the_schedule_is_written_in_its_documented_shape_with_the_local_offset() {
    let mut schedule = Schedule::default();
    schedule.set("t1", Some(entry()));
    schedule.set(
        "t2",
        Some(Entry {
            emails: [("e3".into(), "mb-inbox".into())].into(),
            returned_at: Some(at("2026-10-24T08:00:00Z")),
            ..entry()
        }),
    );

    let written: Value = serde_json::from_str(&schedule.to_json(&auckland())).unwrap();

    assert_eq!(
        written,
        json!({
            "version": 1,
            "threads": {
                "t1": {
                    "emails": {"e1": "mb-inbox", "e2": "mb-travel"},
                    "snoozedAt": "2026-10-21T10:15:00+13:00",
                    "returnAt": "2026-10-24T08:00:00+13:00",
                    "returnedAt": null
                },
                "t2": {
                    "emails": {"e3": "mb-inbox"},
                    "snoozedAt": "2026-10-21T10:15:00+13:00",
                    "returnAt": "2026-10-24T08:00:00+13:00",
                    "returnedAt": "2026-10-24T21:00:00+13:00"
                }
            }
        })
    );
}

#[test]
fn a_saved_schedule_loads_back_from_a_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/blorp/snooze.json");
    let mut schedule = Schedule::default();
    schedule.set("t1", Some(entry()));

    snooze::save(&path, &schedule.to_json(&auckland())).unwrap();
    let loaded = snooze::load(&path, at("2026-10-21T00:00:00Z")).unwrap();

    assert_eq!(loaded.schedule, schedule);
    assert_eq!(loaded.broken, None);
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[test]
fn a_save_replaces_the_whole_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("blorp");
    let path = dir.join("snooze.json");
    let mut schedule = Schedule::default();
    schedule.set("t1", Some(entry()));
    snooze::save(&path, &schedule.to_json(&auckland())).unwrap();

    schedule.set("t1", None);
    snooze::save(&path, &schedule.to_json(&auckland())).unwrap();

    let loaded = snooze::load(&path, at("2026-10-21T00:00:00Z")).unwrap();
    assert_eq!(loaded.schedule, Schedule::default());
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["snooze.json"]);
}

#[test]
fn no_file_is_an_empty_schedule() {
    let dir = tempfile::tempdir().unwrap();

    let loaded = snooze::load(&dir.path().join("snooze.json"), at("2026-10-21T00:00:00Z")).unwrap();

    assert_eq!(loaded.schedule, Schedule::default());
    assert_eq!(loaded.broken, None);
}

#[test]
fn a_file_that_does_not_parse_is_moved_aside() {
    for contents in [
        "{not json",
        r#"{"version": 2, "threads": {}}"#,
        r#"{"version": 1, "threads": {"t1": {"emails": {}}}}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snooze.json");
        fs::write(&path, contents).unwrap();

        let loaded = snooze::load(&path, at("2026-10-21T09:30:05Z")).unwrap();

        let broken = dir.path().join("snooze.json.broken-20261021T093005Z");
        assert_eq!(loaded.schedule, Schedule::default(), "{contents}");
        assert_eq!(
            loaded.broken.as_deref(),
            Some(broken.as_path()),
            "{contents}"
        );
        assert_eq!(fs::read_to_string(&broken).unwrap(), contents);
        assert!(!path.exists(), "{contents}");
    }
}

#[test]
fn an_unreadable_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snooze.json");
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), &path).unwrap();

    assert!(snooze::load(&path, at("2026-10-21T00:00:00Z")).is_err());
}

#[test]
fn a_save_makes_an_existing_state_directory_private() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("blorp");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join("snooze.json");

    snooze::save(&path, &Schedule::default().to_json(&auckland())).unwrap();

    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_save_refuses_a_symlinked_state_directory() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&real, temp.path().join("blorp")).unwrap();

    let saved = snooze::save(
        &temp.path().join("blorp/snooze.json"),
        &Schedule::default().to_json(&auckland()),
    );

    assert!(saved.is_err());
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
