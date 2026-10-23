//! The keys blorp takes: each key, the message it sends, and what it does.
//! Pure.

use std::fmt;

use crate::app::Msg;

/// A key as the keymap names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A character typed without Ctrl, shifted or not.
    Char(char),
    /// A character typed with Ctrl.
    Ctrl(char),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            Key::Ctrl(c) => write!(f, "Ctrl-{c}"),
        }
    }
}

/// Keys and the message they send.
pub struct Binding {
    /// The keys, any of which sends the message.
    pub keys: &'static [Key],
    /// What the keys do.
    pub help: &'static str,
    msg: fn() -> Msg,
}

/// Every binding.
pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: &[Key::Char('j')],
        help: "Down",
        msg: || Msg::Down,
    },
    Binding {
        keys: &[Key::Char('k')],
        help: "Up",
        msg: || Msg::Up,
    },
    Binding {
        keys: &[Key::Char('e')],
        help: "Archive",
        msg: || Msg::Archive,
    },
    Binding {
        keys: &[Key::Char('r')],
        help: "Mark read",
        msg: || Msg::MarkRead,
    },
    Binding {
        keys: &[Key::Char('o')],
        help: "Open in Fastmail",
        msg: || Msg::Open,
    },
    Binding {
        keys: &[Key::Char('m')],
        help: "Switch mailbox",
        msg: || Msg::OpenMailboxList,
    },
    Binding {
        keys: &[Key::Char('q')],
        help: "Quit",
        msg: || Msg::Quit,
    },
    Binding {
        keys: &[Key::Ctrl('c')],
        help: "Quit",
        msg: || Msg::Quit,
    },
];

/// The message `key` sends, if any.
pub fn msg(key: Key) -> Option<Msg> {
    let binding = BINDINGS.iter().find(|b| b.keys.contains(&key))?;
    Some((binding.msg)())
}
