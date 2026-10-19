//! The command line's language: command text to a [`Command`], and the
//! completions of the word under the cursor. Pure.

use std::fmt;
use std::ops::Range;

use crate::app::Sort;
use crate::filter::{HAS_VALUES, IS_VALUES, KEYS};
use crate::mail::{Mailboxes, Role};

/// Each command name followed by its aliases, in the order Tab offers
/// them.
const NAMES: [&str; 10] = [
    "archive", "e", "undo", "u", "redo", "filter", "sort", "dry", "quit", "q",
];

/// A command typed at the `:` prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `:archive`, `:e`.
    Archive,
    /// `:undo [count]`, `:u [count]`. The count given here is used in place
    /// of one typed before the colon.
    Undo(Option<usize>),
    /// `:redo`.
    Redo,
    /// `:filter [text]`: the filter text, empty for the Inbox.
    Filter(String),
    /// `:sort [newest|oldest]`: toggles with no argument.
    Sort(Option<Sort>),
    /// `:dry [off]`: whether dry run is to be on.
    Dry(bool),
    /// `:quit`, `:q`.
    Quit,
}

/// Why command text was refused. The message is shown in the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Holds the command name as written.
    Unknown(String),
    /// Missing or extra arguments; holds the command's usage.
    Usage(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unknown(name) => write!(f, "Unknown command: {name}"),
            Error::Usage(usage) => write!(f, "Usage: {usage}"),
        }
    }
}

impl std::error::Error for Error {}

/// Parses command text: a name, exact or an alias, then its arguments.
/// Blank text is no command.
pub fn parse(text: &str) -> Result<Option<Command>, Error> {
    let text = text.trim();
    let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let bare = |command: Command, usage| {
        if rest.is_empty() {
            Ok(command)
        } else {
            Err(Error::Usage(usage))
        }
    };
    let command = match name {
        "" => return Ok(None),
        "archive" | "e" => bare(Command::Archive, ":archive")?,
        "undo" | "u" => match rest {
            "" => Command::Undo(None),
            count => match count.parse() {
                Ok(n) if n > 0 => Command::Undo(Some(n)),
                _ => return Err(Error::Usage(":undo [count]")),
            },
        },
        "redo" => bare(Command::Redo, ":redo")?,
        "filter" => Command::Filter(rest.into()),
        "sort" => Command::Sort(match rest {
            "" => None,
            "newest" => Some(Sort::Newest),
            "oldest" => Some(Sort::Oldest),
            _ => return Err(Error::Usage(":sort [newest|oldest]")),
        }),
        "dry" => match rest {
            "" => Command::Dry(true),
            "off" => Command::Dry(false),
            _ => return Err(Error::Usage(":dry [off]")),
        },
        "quit" | "q" => bare(Command::Quit, ":quit")?,
        name => return Err(Error::Unknown(name.into())),
    };
    Ok(Some(command))
}

/// What Tab can put in place of the word under the cursor at character
/// `cursor` of `text`: the characters the word covers, and each candidate
/// for it, matching the part of the word before the cursor. A word runs
/// between spaces outside double quotes. The first word completes command
/// names and aliases; the words after `filter` complete token keys, `is:`
/// and `has:` values, and `in:` mailbox paths by segment; the word after
/// `sort` completes `newest` and `oldest`.
pub fn complete(text: &str, cursor: usize, mailboxes: &Mailboxes) -> (Range<usize>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut quoted = false;
    let mut start = 0;
    let mut words = 0;
    for (i, &c) in chars[..cursor].iter().enumerate() {
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            if start < i {
                words += 1;
            }
            start = i + 1;
        }
    }
    let mut end = cursor;
    while end < chars.len() && (quoted || !chars[end].is_whitespace()) {
        if chars[end] == '"' {
            quoted = !quoted;
        }
        end += 1;
    }
    let typed: String = chars[start..cursor].iter().collect();
    let name = text.split_whitespace().next().unwrap_or("");
    let candidates = match (words, name) {
        (0, _) => matching(NAMES, "", "", &typed),
        (1, "sort") => matching(["newest", "oldest"], "", "", &typed),
        (_, "filter") => filter_token(&typed, mailboxes),
        _ => Vec::new(),
    };
    (start..end, candidates)
}

/// Each of `words` that starts with `typed`, between `before` and `after`.
fn matching<'a>(
    words: impl IntoIterator<Item = &'a str>,
    before: &str,
    after: &str,
    typed: &str,
) -> Vec<String> {
    words
        .into_iter()
        .filter(|word| word.starts_with(typed))
        .map(|word| format!("{before}{word}{after}"))
        .collect()
}

/// The completions of a filter token, keeping a leading `-`.
fn filter_token(typed: &str, mailboxes: &Mailboxes) -> Vec<String> {
    let (minus, token) = match typed.strip_prefix('-') {
        Some(token) => ("-", token),
        None => ("", typed),
    };
    let Some((key, value)) = token.split_once(':') else {
        return matching(KEYS, minus, ":", token);
    };
    let names =
        |values: &[(&'static str, _)]| values.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    match key {
        "is" => matching(names(&IS_VALUES), &format!("{minus}is:"), "", value),
        "has" => matching(names(&HAS_VALUES), &format!("{minus}has:"), "", value),
        "in" => mailbox_paths(value, !minus.is_empty(), mailboxes)
            .into_iter()
            .map(|path| format!("{minus}in:{path}"))
            .collect(),
        _ => Vec::new(),
    }
}

/// The mailbox paths `typed` can complete to, sorted: those under the
/// parent path it names whose last segment starts with its last segment,
/// both matched case-insensitively. Drafts and Sent are left out unless
/// the token is `negated`, since they cannot be a queue. A path is quoted
/// when it holds a space or `typed` opens a quote.
fn mailbox_paths(typed: &str, negated: bool, mailboxes: &Mailboxes) -> Vec<String> {
    let (quote, typed) = match typed.strip_prefix('"') {
        Some(rest) => (true, rest.trim_end_matches('"')),
        None => (false, typed),
    };
    let typed = typed.to_lowercase();
    let (parent, segment) = split_path(&typed);
    let mut paths: Vec<String> = mailboxes
        .iter()
        .filter(|m| negated || !matches!(m.role, Some(Role::Drafts | Role::Sent)))
        .map(|m| mailboxes.path(m))
        .filter(|path| {
            let lower = path.to_lowercase();
            let (p, s) = split_path(&lower);
            p == parent && s.starts_with(segment)
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            if quote || path.contains(char::is_whitespace) {
                format!("\"{path}\"")
            } else {
                path
            }
        })
        .collect()
}

/// A mailbox path's parent path, if it has one, and its last segment.
fn split_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((parent, segment)) => (Some(parent), segment),
        None => (None, path),
    }
}
