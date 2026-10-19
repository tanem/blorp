//! Snooze presets and the `:snooze` time forms, in the local time zone.

use blorp::snooze::{Preset, When};
use jiff::Zoned;
use jiff::tz::TimeZone;

/// New Zealand time, which moves to daylight time on the last Sunday of
/// September and back on the first Sunday of April.
fn auckland() -> TimeZone {
    TimeZone::posix("NZST-12NZDT,M9.5.0,M4.1.0/3").unwrap()
}

/// `local` as a civil time in Auckland, such as `2026-10-21T09:15`.
fn at(local: &str) -> Zoned {
    local
        .parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned(auckland())
        .unwrap()
}

/// The civil time `when` resolves to from `now`, in Auckland.
fn resolve(when: &str, now: &str) -> String {
    let when = When::parse(when).unwrap_or_else(|| panic!("{when:?} parses"));
    when.resolve(&at(now))
        .unwrap_or_else(|| panic!("{when:?} resolves"))
        .datetime()
        .to_string()
}

#[test]
fn later_today_is_three_hours_on_rounded_up_to_the_hour() {
    // 2026-10-21 is a Wednesday.
    for (now, expected) in [
        ("2026-10-21T09:15", "2026-10-21T13:00:00"),
        ("2026-10-21T09:00", "2026-10-21T12:00:00"),
        ("2026-10-21T16:59", "2026-10-21T20:00:00"),
        ("2026-10-21T17:00", "2026-10-21T20:00:00"),
        ("2026-10-21T17:01", "2026-10-22T08:00:00"),
        ("2026-10-21T23:30", "2026-10-22T08:00:00"),
        ("2026-10-21T02:10", "2026-10-21T06:00:00"),
    ] {
        assert_eq!(
            Preset::Later.at(&at(now)).datetime().to_string(),
            expected,
            "from {now}"
        );
    }
}

#[test]
fn tomorrow_morning_is_eight_the_next_day() {
    assert_eq!(
        Preset::Tomorrow
            .at(&at("2026-10-21T23:59"))
            .datetime()
            .to_string(),
        "2026-10-22T08:00:00"
    );
}

#[test]
fn this_weekend_is_saturday_morning_or_next_saturday_at_the_weekend() {
    for (now, expected) in [
        ("2026-10-19T10:00", "2026-10-24T08:00:00"),
        ("2026-10-23T22:00", "2026-10-24T08:00:00"),
        ("2026-10-24T06:00", "2026-10-31T08:00:00"),
        ("2026-10-25T10:00", "2026-10-31T08:00:00"),
    ] {
        assert_eq!(
            Preset::Weekend.at(&at(now)).datetime().to_string(),
            expected,
            "from {now}"
        );
    }
}

#[test]
fn next_week_is_monday_morning() {
    for (now, expected) in [
        ("2026-10-19T06:00", "2026-10-26T08:00:00"),
        ("2026-10-21T10:00", "2026-10-26T08:00:00"),
        ("2026-10-25T23:00", "2026-10-26T08:00:00"),
    ] {
        assert_eq!(
            Preset::NextWeek.at(&at(now)).datetime().to_string(),
            expected,
            "from {now}"
        );
    }
}

#[test]
fn presets_keep_local_time_across_a_daylight_saving_change() {
    // Daylight time starts at 02:00 on Sunday 2026-09-27.
    let monday = Preset::NextWeek.at(&at("2026-09-25T12:00"));

    assert_eq!(monday.datetime().to_string(), "2026-09-28T08:00:00");
    assert_eq!(monday.offset().to_string(), "+13");
}

#[test]
fn the_command_takes_the_preset_names() {
    let now = "2026-10-21T09:15";
    for (name, expected) in [
        ("later", "2026-10-21T13:00:00"),
        ("tomorrow", "2026-10-22T08:00:00"),
        ("weekend", "2026-10-24T08:00:00"),
        ("next", "2026-10-26T08:00:00"),
    ] {
        assert_eq!(resolve(name, now), expected, "{name}");
    }
}

#[test]
fn durations_count_from_now() {
    let now = "2026-10-21T09:15";
    for (text, expected) in [
        ("30m", "2026-10-21T09:45:00"),
        ("2h", "2026-10-21T11:15:00"),
        ("3d", "2026-10-24T09:15:00"),
        ("1w", "2026-10-28T09:15:00"),
    ] {
        assert_eq!(resolve(text, now), expected, "{text}");
    }
}

#[test]
fn a_clock_time_is_today_or_tomorrow_once_it_has_passed() {
    assert_eq!(resolve("17:30", "2026-10-21T09:15"), "2026-10-21T17:30:00");
    assert_eq!(resolve("9:05", "2026-10-21T09:15"), "2026-10-22T09:05:00");
    assert_eq!(resolve("09:15", "2026-10-21T09:15"), "2026-10-22T09:15:00");
}

#[test]
fn a_weekday_is_its_next_morning_or_the_time_given() {
    // From Wednesday 2026-10-21.
    let now = "2026-10-21T09:15";
    for (text, expected) in [
        ("mon", "2026-10-26T08:00:00"),
        ("fri", "2026-10-23T08:00:00"),
        ("Friday", "2026-10-23T08:00:00"),
        ("mon 9", "2026-10-26T09:00:00"),
        ("thu 17:30", "2026-10-22T17:30:00"),
        ("wed 17:00", "2026-10-21T17:00:00"),
        ("wed", "2026-10-28T08:00:00"),
    ] {
        assert_eq!(resolve(text, now), expected, "{text}");
    }
}

#[test]
fn a_date_is_its_morning_or_the_time_given() {
    let now = "2026-10-21T09:15";
    assert_eq!(resolve("2026-11-02", now), "2026-11-02T08:00:00");
    assert_eq!(resolve("2026-11-02 14", now), "2026-11-02T14:00:00");
    assert_eq!(resolve("2026-11-02 14:45", now), "2026-11-02T14:45:00");
}

#[test]
fn anything_else_is_not_understood() {
    for text in [
        "",
        "next tuesday",
        "soon",
        "0m",
        "-2h",
        "5y",
        "25:00",
        "17:60",
        "9",
        "mon 24",
        "mon tue",
        "2026-02-30",
        "2026-11-02 9am",
        "later 9",
    ] {
        assert_eq!(When::parse(text), None, "{text:?}");
    }
}
