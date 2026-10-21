//! The keys normal mode and visual line mode take: each key, the message
//! it sends, and what help says it does. Pure.

use std::fmt;

use crate::app::Msg;

/// A key as the keymap names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// A character typed with Ctrl.
    Ctrl(char),
    Tab,
    /// Shift-Tab.
    BackTab,
    Esc,
    Enter,
    /// Any of `0` to `9`.
    Digit,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            Key::Ctrl(c) => write!(f, "Ctrl-{c}"),
            Key::Tab => f.write_str("Tab"),
            Key::BackTab => f.write_str("Shift-Tab"),
            Key::Esc => f.write_str("Esc"),
            Key::Enter => f.write_str("Enter"),
            Key::Digit => f.write_str("0-9"),
        }
    }
}

/// What a binding's keys send.
#[derive(Clone, Copy)]
enum Sends {
    Msg(fn() -> Msg),
    /// The digit typed, as the next digit of a count.
    Digit,
}

/// Keys and the message they send.
pub struct Binding {
    pub keys: &'static [Key],
    /// What help says the keys do.
    pub help: &'static str,
    sends: Sends,
}

const fn binding(keys: &'static [Key], help: &'static str, msg: fn() -> Msg) -> Binding {
    Binding {
        keys,
        help,
        sends: Sends::Msg(msg),
    }
}

/// Every binding, in the order help lists them.
pub const BINDINGS: &[Binding] = &[
    binding(&[Key::Char('j')], "Down", || Msg::Down),
    binding(&[Key::Char('k')], "Up", || Msg::Up),
    binding(&[Key::Char('g')], "First row", || Msg::Top),
    binding(
        &[Key::Char('G')],
        "Last row, or the row the count names",
        || Msg::Bottom,
    ),
    Binding {
        keys: &[Key::Digit],
        help: "Count for the next key",
        sends: Sends::Digit,
    },
    binding(&[Key::Char('e')], "Archive", || Msg::Archive),
    binding(&[Key::Char('z')], "Snooze", || Msg::OpenSnooze),
    binding(&[Key::Char('f')], "Flag or unflag", || Msg::ToggleFlag),
    binding(&[Key::Char('d')], "Delete", || Msg::Delete),
    binding(&[Key::Char('r')], "Mark read or unread", || Msg::ToggleRead),
    binding(&[Key::Char('m')], "Move", || Msg::OpenMove),
    binding(&[Key::Char('!')], "Mark spam", || Msg::Spam),
    binding(&[Key::Char('o')], "Open in the browser", || Msg::Open),
    binding(&[Key::Char('u')], "Undo", || Msg::Undo),
    binding(&[Key::Ctrl('r')], "Redo", || Msg::Redo),
    binding(&[Key::Char('x')], "Mark or unmark", || Msg::Mark),
    binding(&[Key::Char('X')], "Clear the marks", || Msg::ClearMarks),
    binding(&[Key::Char('V')], "Visual line", || Msg::Visual),
    binding(&[Key::Char('.')], "Repeat the last action", || Msg::Repeat),
    binding(&[Key::Char('/')], "Filter", || Msg::OpenFilter),
    binding(&[Key::Tab], "Next view", || Msg::NextView),
    binding(&[Key::BackTab], "Previous view", || Msg::PreviousView),
    binding(&[Key::Char('t')], "Newest or oldest first", || {
        Msg::ToggleSort
    }),
    binding(&[Key::Char('R')], "Refresh", || Msg::Refresh),
    binding(&[Key::Char(':')], "Command line", || Msg::OpenCommand),
    binding(&[Key::Char('D')], "Dry run on or off", || Msg::ToggleDryRun),
    binding(&[Key::Char('?')], "Help", || Msg::OpenHelp),
    binding(&[Key::Char('q')], "Quit", || Msg::Quit),
    binding(&[Key::Ctrl('c')], "Quit", || Msg::Quit),
    binding(
        &[Key::Esc, Key::Enter],
        "Drop the count and leave visual line",
        || Msg::Cancel,
    ),
];

/// The message `key` sends, if any.
pub fn msg(key: Key) -> Option<Msg> {
    let digit = match key {
        Key::Char(c) => c.to_digit(10),
        _ => None,
    };
    let key = if digit.is_some() { Key::Digit } else { key };
    let binding = BINDINGS.iter().find(|b| b.keys.contains(&key))?;
    match binding.sends {
        Sends::Msg(msg) => Some(msg()),
        Sends::Digit => digit.map(|d| Msg::Digit(d as u8)),
    }
}
