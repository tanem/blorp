//! The help overlay's lines, built from the keymap and the command table,
//! and how many of them the overlay shows. Pure.

use crate::command::COMMANDS;
use crate::keymap::BINDINGS;

/// A line of the help overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// The title of the lines that follow.
    Heading(&'static str),
    /// Keys, or a command's usage and aliases, and what they do.
    Entry {
        name: String,
        aliases: String,
        help: &'static str,
    },
    /// An empty line between sections.
    Blank,
}

/// Every key, then every command, each under a heading.
pub fn lines() -> Vec<Line> {
    let mut lines = vec![Line::Heading("Keys")];
    lines.extend(BINDINGS.iter().map(|binding| Line::Entry {
        name: join(binding.keys.iter().map(ToString::to_string)),
        aliases: String::new(),
        help: binding.help,
    }));
    lines.push(Line::Blank);
    lines.push(Line::Heading("Commands"));
    lines.extend(COMMANDS.iter().map(|spec| Line::Entry {
        name: spec.usage.into(),
        aliases: join(spec.aliases.iter().map(|alias| format!(":{alias}"))),
        help: spec.help,
    }));
    lines
}

/// `words`, a space apart.
fn join(words: impl Iterator<Item = String>) -> String {
    words.collect::<Vec<_>>().join(" ")
}

/// How many lines the overlay shows on a screen `height` rows tall: the
/// rows between the tabline and the statusline, less the bottom line and
/// the overlay's top and bottom borders.
pub fn page(height: u16) -> usize {
    usize::from(height.saturating_sub(5))
}
