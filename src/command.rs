//! The command line's language: command text to a [`Command`], and the
//! completions of the word under the cursor. Pure.

use std::fmt;
use std::ops::Range;

use crate::app::Sort;
use crate::config::View;
use crate::filter::{HAS_VALUES, IS_VALUES, KEYS};
use crate::mail::{Mailbox, Mailboxes, Role};
use crate::snooze::{self, When};

/// Each command name followed by its aliases, in the order Tab offers
/// them.
const NAMES: [&str; 27] = [
    "archive", "e", "delete", "d", "spam", "move", "m", "snooze", "z", "unsnooze", "uz", "flag",
    "f", "read", "r", "unread", "undo", "u", "redo", "view", "v", "filter", "sort", "refresh",
    "dry", "quit", "q",
];

/// The words after `:view` that are not view names.
const VIEW_WORDS: [&str; 2] = ["save", "delete"];

/// A command typed at the `:` prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `:archive`, `:e`.
    Archive,
    /// `:delete`, `:d`.
    Delete,
    /// `:spam`.
    Spam,
    /// `:move <mailbox>`, `:m <mailbox>`: the mailbox name as written, the
    /// rest of the line.
    Move(String),
    /// `:snooze <time>`, `:z <time>`.
    Snooze(When),
    /// `:unsnooze`, `:uz`: return the selection from the snooze mailbox now.
    Unsnooze,
    /// `:flag`, `:f`: toggle the flag, as `f` does.
    Flag,
    /// `:read`, `:r`: mark read.
    Read,
    /// `:unread`: mark unread.
    Unread,
    /// `:undo [count]`, `:u [count]`. The count given here is used in place
    /// of one typed before the colon.
    Undo(Option<usize>),
    /// `:redo`.
    Redo,
    /// `:view <name>`, `:v <name>`: switch to the view with this name.
    View(String),
    /// `:view save <name>`: save the current filter and sort as this view.
    SaveView(String),
    /// `:view delete <name>`: delete the view with this name.
    DeleteView(String),
    /// `:filter [text]`: the filter text, empty for the Inbox.
    Filter(String),
    /// `:sort [newest|oldest]`: toggles with no argument.
    Sort(Option<Sort>),
    /// `:refresh`: reconcile at once, as `R` does.
    Refresh,
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
    /// Holds the snooze time as written.
    SnoozeTime(String),
    /// `save` or `delete` given as a view name to save.
    ReservedView(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unknown(name) => write!(f, "Unknown command: {name}"),
            Error::Usage(usage) => write!(f, "Usage: {usage}"),
            Error::SnoozeTime(text) => write!(f, "Snooze time not understood: \"{text}\""),
            Error::ReservedView(name) => write!(f, "View name \"{name}\" is reserved"),
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
        "delete" | "d" => bare(Command::Delete, ":delete")?,
        "spam" => bare(Command::Spam, ":spam")?,
        "move" | "m" => match rest {
            "" => return Err(Error::Usage(":move <mailbox>")),
            name => Command::Move(name.into()),
        },
        "snooze" | "z" => match rest {
            "" => return Err(Error::Usage(":snooze <time>")),
            time => {
                Command::Snooze(When::parse(time).ok_or_else(|| Error::SnoozeTime(time.into()))?)
            }
        },
        "unsnooze" | "uz" => bare(Command::Unsnooze, ":unsnooze")?,
        "flag" | "f" => bare(Command::Flag, ":flag")?,
        "read" | "r" => bare(Command::Read, ":read")?,
        "unread" => bare(Command::Unread, ":unread")?,
        "undo" | "u" => match rest {
            "" => Command::Undo(None),
            count => match count.parse() {
                Ok(n) if n > 0 => Command::Undo(Some(n)),
                _ => return Err(Error::Usage(":undo [count]")),
            },
        },
        "redo" => bare(Command::Redo, ":redo")?,
        "view" | "v" => view(rest)?,
        "filter" => Command::Filter(rest.into()),
        "sort" => Command::Sort(match rest {
            "" => None,
            "newest" => Some(Sort::Newest),
            "oldest" => Some(Sort::Oldest),
            _ => return Err(Error::Usage(":sort [newest|oldest]")),
        }),
        "refresh" => bare(Command::Refresh, ":refresh")?,
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

/// The arguments of `:view`, the rest of the line: a view name, or `save`
/// or `delete` and a view name, which cannot itself be `save` or `delete`.
fn view(rest: &str) -> Result<Command, Error> {
    const USAGE: Error = Error::Usage(":view [save|delete] <name>");
    let (word, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = name.trim();
    match word {
        "" => Err(USAGE),
        "save" | "delete" if name.is_empty() => Err(USAGE),
        "save" if VIEW_WORDS.contains(&name) => Err(Error::ReservedView(name.into())),
        "save" => Ok(Command::SaveView(name.into())),
        "delete" => Ok(Command::DeleteView(name.into())),
        _ => Ok(Command::View(rest.into())),
    }
}

/// What Tab can put in place of the word under the cursor at character
/// `cursor` of `text`: the characters the word covers, and each candidate
/// for it, matching the part of the word before the cursor. A word runs
/// between spaces outside double quotes. The first word completes command
/// names and aliases; the words after `filter` complete token keys, `is:`
/// and `has:` values, and `in:` mailbox paths by segment; the word after
/// `sort` completes `newest` and `oldest`; the word after `snooze` or `z`
/// completes the preset names and weekdays. The rest of the line after
/// `move` or `m` completes a mailbox path, and after `view` or `v`,
/// `save`, `delete`, or the name of one of `views`, which after `save` or
/// `delete` completes a view name.
pub fn complete(
    text: &str,
    cursor: usize,
    mailboxes: &Mailboxes,
    views: &[View],
) -> (Range<usize>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    if let Some(start) = argument_start(&chars, cursor)
        && matches!(text.split_whitespace().next(), Some("view" | "v"))
    {
        return view_argument(&chars[start..cursor], start, chars.len(), views);
    }
    if let Some(start) = argument_start(&chars, cursor)
        && matches!(text.split_whitespace().next(), Some("move" | "m"))
    {
        let typed: String = chars[start..cursor].iter().collect();
        let candidates = paths(&typed.to_lowercase(), mailboxes, |m| {
            mailboxes.is_move_target(m)
        });
        return (start..chars.len(), candidates);
    }
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
        (1, "snooze" | "z") => matching(snooze::completions(), "", "", &typed.to_lowercase()),
        (_, "filter") => filter_token(&typed, mailboxes),
        _ => Vec::new(),
    };
    (start..end, candidates)
}

/// The completions of `typed`, the argument of `:view` up to the cursor,
/// starting at character `start` of a line `len` characters long: `save`,
/// `delete`, and the view names, or after `save` or `delete` the view
/// names, each covering the rest of the line. View names match
/// case-insensitively.
fn view_argument(
    typed: &[char],
    start: usize,
    len: usize,
    views: &[View],
) -> (Range<usize>, Vec<String>) {
    let names = |typed: &[char]| -> Vec<String> {
        let typed: String = typed.iter().collect::<String>().to_lowercase();
        views
            .iter()
            .filter(|v| v.name.to_lowercase().starts_with(&typed))
            .map(|v| v.name.clone())
            .collect()
    };
    let word = typed.iter().position(|c| c.is_whitespace());
    if let Some(end) = word {
        let first: String = typed[..end].iter().collect();
        if VIEW_WORDS.contains(&first.as_str()) {
            let name = typed[end..]
                .iter()
                .position(|c| !c.is_whitespace())
                .map_or(typed.len(), |i| end + i);
            return (start + name..len, names(&typed[name..]));
        }
    }
    let first: String = typed.iter().collect();
    let mut candidates = matching(VIEW_WORDS, "", "", &first);
    candidates.extend(names(typed));
    (start..len, candidates)
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
    match key {
        "is" => matching(
            IS_VALUES.iter().map(|(name, _)| *name),
            &format!("{minus}is:"),
            "",
            value,
        ),
        "has" => matching(
            HAS_VALUES.iter().map(|(name, _)| *name),
            &format!("{minus}has:"),
            "",
            value,
        ),
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
    paths(&typed.to_lowercase(), mailboxes, |m| {
        negated || !matches!(m.role, Some(Role::Drafts | Role::Sent))
    })
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

/// The paths of the mailboxes that `keep` keeps under the parent path
/// `typed` names whose last segment starts with its last segment, sorted.
/// `typed` is lowercase, and paths match it case-insensitively.
fn paths(typed: &str, mailboxes: &Mailboxes, keep: impl Fn(&Mailbox) -> bool) -> Vec<String> {
    let (parent, segment) = split_path(typed);
    let mut paths: Vec<String> = mailboxes
        .iter()
        .filter(|m| keep(m))
        .map(|m| mailboxes.path(m))
        .filter(|path| {
            let lower = path.to_lowercase();
            let (p, s) = split_path(&lower);
            p == parent && s.starts_with(segment)
        })
        .collect();
    paths.sort();
    paths
}

/// Where the argument starts in `chars`, a command line, when `cursor` is
/// past the command name: at its first character after the name, or at
/// `cursor` when only spaces come between them.
fn argument_start(chars: &[char], cursor: usize) -> Option<usize> {
    let name_start = chars.iter().position(|c| !c.is_whitespace())?;
    let name_end = chars[name_start..]
        .iter()
        .position(|c| c.is_whitespace())
        .map(|i| name_start + i)?;
    if cursor <= name_end {
        return None;
    }
    let start = chars[name_end..]
        .iter()
        .position(|c| !c.is_whitespace())
        .map_or(chars.len(), |i| name_end + i);
    Some(start.min(cursor))
}

/// A mailbox path's parent path, if it has one, and its last segment.
fn split_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((parent, segment)) => (Some(parent), segment),
        None => (None, path),
    }
}
