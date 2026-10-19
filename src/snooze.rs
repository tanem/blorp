//! Snooze: the schedule, the presets, and the `:snooze` time forms, in the
//! local time zone. Pure apart from [`load`] and [`save`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jiff::civil::{Date, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};
use serde::{Deserialize, Serialize};

use crate::private_file;

/// The schedule file's name in blorp's state directory.
pub const FILE_NAME: &str = "snooze.json";

/// The schedule file's format version.
const VERSION: u32 = 1;

/// One entry per snoozed thread, by thread id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schedule {
    threads: BTreeMap<String, Entry>,
}

/// A snoozed thread: the emails the snooze moved, each with the id of the
/// mailbox it came from, when it was snoozed, when it returns, and when it
/// returned, once it has.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub emails: BTreeMap<String, String>,
    pub snoozed_at: Timestamp,
    pub return_at: Timestamp,
    pub returned_at: Option<Timestamp>,
}

impl Entry {
    /// Whether the thread is due to return at `now` and has not.
    pub fn is_due(&self, now: Timestamp) -> bool {
        self.returned_at.is_none() && self.return_at <= now
    }
}

impl Schedule {
    /// The entry for the thread with `id`.
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.threads.get(id)
    }

    /// Sets or, for `None`, removes the entry for the thread with `id`.
    pub fn set(&mut self, id: &str, entry: Option<Entry>) {
        match entry {
            Some(entry) => self.threads.insert(id.to_string(), entry),
            None => self.threads.remove(id),
        };
    }

    /// Every entry, by thread id.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.threads.iter()
    }

    /// The file's contents: each time in RFC 3339 with the offset `tz` had
    /// when its thread was snoozed.
    pub fn to_json(&self, tz: &TimeZone) -> String {
        let threads = self
            .threads
            .iter()
            .map(|(id, entry)| {
                let offset = tz.to_offset(entry.snoozed_at);
                let time = |t: Timestamp| t.display_with_offset(offset).to_string();
                let written = WrittenEntry {
                    emails: &entry.emails,
                    snoozed_at: time(entry.snoozed_at),
                    return_at: time(entry.return_at),
                    returned_at: entry.returned_at.map(time),
                };
                (id, written)
            })
            .collect();
        let file = WrittenFile {
            version: VERSION,
            threads,
        };
        serde_json::to_string_pretty(&file).expect("the schedule serialises")
    }
}

#[derive(Serialize)]
struct WrittenFile<'a> {
    version: u32,
    threads: BTreeMap<&'a String, WrittenEntry<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WrittenEntry<'a> {
    emails: &'a BTreeMap<String, String>,
    snoozed_at: String,
    return_at: String,
    returned_at: Option<String>,
}

#[derive(Deserialize)]
struct ReadFile {
    version: u32,
    threads: BTreeMap<String, Entry>,
}

/// A schedule as [`load`] read it.
#[derive(Debug)]
pub struct Loaded {
    pub schedule: Schedule,
    /// Where a file that did not parse was moved to.
    pub broken: Option<PathBuf>,
}

/// Reads the schedule at `path`. No file is an empty schedule. A file that
/// does not parse is renamed to `<name>.broken-<now>` beside it, and the
/// schedule is empty.
pub fn load(path: &Path, now: Timestamp) -> io::Result<Loaded> {
    let Some(contents) = private_file::read(path)? else {
        return Ok(Loaded {
            schedule: Schedule::default(),
            broken: None,
        });
    };
    match serde_json::from_slice::<ReadFile>(&contents) {
        Ok(file) if file.version == VERSION => Ok(Loaded {
            schedule: Schedule {
                threads: file.threads,
            },
            broken: None,
        }),
        _ => {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".broken-{}", now.strftime("%Y%m%dT%H%M%SZ")));
            let broken = path.with_file_name(name);
            fs::rename(path, &broken)?;
            Ok(Loaded {
                schedule: Schedule::default(),
                broken: Some(broken),
            })
        }
    }
}

/// Replaces the schedule at `path` with `contents`, through a temp file and
/// rename, mode 0600 in a 0700 directory. The directory is blorp's state
/// directory, which the log may have created first: an existing one that
/// others can reach is set to mode 0700.
pub fn save(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && let Ok(meta) = fs::symlink_metadata(dir)
        && meta.is_dir()
        && meta.permissions().mode() & 0o077 != 0
    {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    private_file::write(path, contents.as_bytes())
}

/// The hour the presets, weekdays, and dates without a time return at.
const MORNING: i8 = 8;

/// Later today returns no later than this hour; past it, it is tomorrow
/// morning.
const EVENING: i8 = 20;

/// A fixed snooze time offered by the `z` picker and by name to `:snooze`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Later,
    Tomorrow,
    Weekend,
    NextWeek,
}

impl Preset {
    /// Every preset, in the order the picker lists them.
    pub const ALL: [Preset; 4] = [
        Preset::Later,
        Preset::Tomorrow,
        Preset::Weekend,
        Preset::NextWeek,
    ];

    /// The picker's hotkey.
    pub fn key(self) -> char {
        match self {
            Preset::Later => 'l',
            Preset::Tomorrow => 't',
            Preset::Weekend => 'w',
            Preset::NextWeek => 'n',
        }
    }

    /// The picker's label.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Later => "Later today",
            Preset::Tomorrow => "Tomorrow morning",
            Preset::Weekend => "This weekend",
            Preset::NextWeek => "Next week",
        }
    }

    /// The name `:snooze` takes.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Later => "later",
            Preset::Tomorrow => "tomorrow",
            Preset::Weekend => "weekend",
            Preset::NextWeek => "next",
        }
    }

    /// When the preset returns, from `now` in its time zone. Later today is
    /// three hours on, rounded up to the hour, or tomorrow morning when that
    /// is past 20:00. Tomorrow morning is 08:00 the next day. This weekend
    /// is the next Saturday 08:00, so a week on from a Saturday. Next week is
    /// the next Monday 08:00.
    pub fn at(self, now: &Zoned) -> Zoned {
        let today = now.date();
        let date = match self {
            Preset::Later => {
                let later = now.checked_add(3.hours()).expect("in range").datetime();
                let hour = later.date().at(later.hour(), 0, 0, 0);
                let hour = if hour == later {
                    hour
                } else {
                    hour.checked_add(1.hour()).expect("in range")
                };
                if hour <= today.at(EVENING, 0, 0, 0) {
                    return local(hour.date(), hour.time(), now).expect("in range");
                }
                today.tomorrow().expect("in range")
            }
            Preset::Tomorrow => today.tomorrow().expect("in range"),
            Preset::Weekend => match today.weekday() {
                Weekday::Saturday => today.checked_add(7.days()).expect("in range"),
                _ => today.nth_weekday(1, Weekday::Saturday).expect("in range"),
            },
            Preset::NextWeek => today.nth_weekday(1, Weekday::Monday).expect("in range"),
        };
        local(date, morning(), now).expect("in range")
    }
}

/// A snooze time as `:snooze` takes it, not yet resolved against the
/// current time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Preset(Preset),
    /// A duration from now, such as `30m` or `3d`.
    In(i64, Unit),
    /// A clock time: today, or tomorrow once it has passed.
    At(Time),
    /// The next such weekday, at the time given or 08:00, as soon as that
    /// is still to come.
    Weekday(Weekday, Option<Time>),
    /// A date, at the time given or 08:00.
    Date(Date, Option<Time>),
}

/// The unit of a duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Minutes,
    Hours,
    Days,
    Weeks,
}

/// The weekday names `:snooze` takes, short form first, in the order Tab
/// offers them.
const WEEKDAYS: [(&str, &str, Weekday); 7] = [
    ("mon", "monday", Weekday::Monday),
    ("tue", "tuesday", Weekday::Tuesday),
    ("wed", "wednesday", Weekday::Wednesday),
    ("thu", "thursday", Weekday::Thursday),
    ("fri", "friday", Weekday::Friday),
    ("sat", "saturday", Weekday::Saturday),
    ("sun", "sunday", Weekday::Sunday),
];

/// The words Tab offers for `:snooze`: the preset names, then the short
/// weekday names.
pub fn completions() -> impl Iterator<Item = &'static str> {
    Preset::ALL
        .iter()
        .map(|p| p.name())
        .chain(WEEKDAYS.iter().map(|(short, _, _)| *short))
}

impl When {
    /// Parses `:snooze`'s argument, ignoring case: a preset name; a
    /// duration such as `30m`, `2h`, `3d`, or `1w`; a clock time such as
    /// `17:30`; a weekday such as `mon` or `monday`, optionally followed by
    /// an hour or a clock time, as in `mon 9`; or an ISO date such as
    /// `2026-11-02`, optionally followed the same way. Anything else is
    /// `None`.
    pub fn parse(text: &str) -> Option<When> {
        let text = text.trim().to_lowercase();
        let words: Vec<&str> = text.split_whitespace().collect();
        match words.as_slice() {
            [word] => {
                if let Some(preset) = Preset::ALL.into_iter().find(|p| p.name() == *word) {
                    return Some(When::Preset(preset));
                }
                if let Some(when) = duration(word) {
                    return Some(when);
                }
                if word.contains(':') {
                    return clock(word).map(When::At);
                }
                if let Some(day) = weekday(word) {
                    return Some(When::Weekday(day, None));
                }
                date(word).map(|d| When::Date(d, None))
            }
            [day, time] => {
                let time = clock(time)?;
                if let Some(day) = weekday(day) {
                    return Some(When::Weekday(day, Some(time)));
                }
                date(day).map(|d| When::Date(d, Some(time)))
            }
            _ => None,
        }
    }

    /// When the snooze returns, from `now` in its time zone, or `None` when
    /// that is out of range. A date resolves even when it has passed.
    pub fn resolve(&self, now: &Zoned) -> Option<Zoned> {
        let today = now.date();
        match *self {
            When::Preset(preset) => Some(preset.at(now)),
            When::In(n, unit) => {
                let span = match unit {
                    Unit::Minutes => n.minutes(),
                    Unit::Hours => n.hours(),
                    Unit::Days => n.days(),
                    Unit::Weeks => n.weeks(),
                };
                now.checked_add(span).ok()
            }
            When::At(time) => {
                let at = local(today, time, now)?;
                if at > *now {
                    return Some(at);
                }
                local(today.tomorrow().ok()?, time, now)
            }
            When::Weekday(day, time) => {
                let time = time.unwrap_or_else(morning);
                let mut date = if today.weekday() == day {
                    today
                } else {
                    today.nth_weekday(1, day).ok()?
                };
                loop {
                    let at = local(date, time, now)?;
                    if at > *now {
                        return Some(at);
                    }
                    date = date.checked_add(7.days()).ok()?;
                }
            }
            When::Date(date, time) => local(date, time.unwrap_or_else(morning), now),
        }
    }
}

/// `30m`, `2h`, `3d`, `1w`: a whole number above zero and a unit.
fn duration(word: &str) -> Option<When> {
    let unit = match word.chars().last()? {
        'm' => Unit::Minutes,
        'h' => Unit::Hours,
        'd' => Unit::Days,
        'w' => Unit::Weeks,
        _ => return None,
    };
    let digits = &word[..word.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    (n > 0).then_some(When::In(n, unit))
}

/// `9`, `17`, `9:05`, `17:30`: an hour, and minutes after a colon.
fn clock(word: &str) -> Option<Time> {
    let (hour, minute) = match word.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => (hour, minute),
        Some(_) => return None,
        None => (word, "00"),
    };
    let number = |text: &str| -> Option<i8> {
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    Time::new(number(hour)?, number(minute)?, 0, 0).ok()
}

fn weekday(word: &str) -> Option<Weekday> {
    WEEKDAYS
        .iter()
        .find(|(short, long, _)| word == *short || word == *long)
        .map(|(_, _, day)| *day)
}

/// `2026-11-02`.
fn date(word: &str) -> Option<Date> {
    let parts: Vec<&str> = word.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return None;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    Date::new(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?).ok()
}

fn morning() -> Time {
    Time::constant(MORNING, 0, 0, 0)
}

/// `date` at `time` in `now`'s time zone, or none when out of range.
fn local(date: Date, time: Time, now: &Zoned) -> Option<Zoned> {
    date.to_datetime(time)
        .to_zoned(now.time_zone().clone())
        .ok()
}

/// How a snooze time `at` reads from `now`: `today 15:00` on the same day,
/// `Mon 08:00` within the next six days, and `Mon 26 Oct 08:00` otherwise,
/// with the year when it is not this year's.
pub fn describe(at: &Zoned, now: &Zoned) -> String {
    let days = (at.date() - now.date()).get_days();
    if at.date() == now.date() {
        at.strftime("today %H:%M").to_string()
    } else if (1..7).contains(&days) {
        at.strftime("%a %H:%M").to_string()
    } else if at.year() == now.year() {
        at.strftime("%a %-d %b %H:%M").to_string()
    } else {
        at.strftime("%a %-d %b %Y %H:%M").to_string()
    }
}
