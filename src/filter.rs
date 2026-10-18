//! The filter language: filter text to terms, and the terms to the
//! `Email/query` filter and queue mailbox they select, resolving mailbox
//! names through [`crate::mail`]. Pure.

use std::fmt;

use serde_json::{Value, json};

use crate::mail::{Mailboxes, NameError, QueueMailbox, Role, SNOOZE_MAILBOX};

/// A parsed filter: terms that must all match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub terms: Vec<Term>,
}

/// One token of a filter, negated by a leading `-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub negated: bool,
    pub condition: Condition,
}

/// What a token matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `in:<mailbox>`, the name as written.
    In(String),
    /// `in:any`.
    InAny,
    From(String),
    To(String),
    Subject(String),
    /// `is:unread`.
    Unread,
    /// `is:read`.
    Read,
    /// `is:flagged`.
    Flagged,
    /// `is:list`: a `List-Id` header exists.
    List,
    /// `has:attachment`.
    Attachment,
    /// Bare text, matched as Fastmail's `text` condition.
    Text(String),
}

/// Why filter text was refused. The message is shown in the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An unknown key, or an unknown `is:` or `has:` value; holds the token
    /// as written.
    UnknownToken(String),
    UnbalancedQuote,
    /// A token with nothing after its key or `-`; holds the token as
    /// written.
    MissingValue(String),
    /// More than one `in:` token that is not negated.
    SecondIn,
    NegatedAny,
    /// An `in:` name matching no single mailbox.
    Mailbox(NameError),
    /// `in:` naming Drafts or Sent; holds the mailbox's path.
    NotAQueue(String),
    /// No `in:`, and no mailbox has the inbox role.
    NoInbox,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownToken(token) => write!(f, "Unknown filter token \"{token}\""),
            Error::UnbalancedQuote => write!(f, "Unbalanced quote"),
            Error::MissingValue(token) => write!(f, "Missing value in \"{token}\""),
            Error::SecondIn => write!(f, "More than one in: token"),
            Error::NegatedAny => write!(f, "in:any cannot be negated"),
            Error::Mailbox(error) => write!(f, "{error}"),
            Error::NotAQueue(path) => write!(f, "{path} cannot be a queue"),
            Error::NoInbox => write!(f, "No Inbox mailbox"),
        }
    }
}

impl std::error::Error for Error {}

/// What a filter selects, with its mailbox names resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub mailbox: QueueMailbox,
    /// The queue mailbox's path, or `All mail` for `in:any`.
    pub name: String,
    /// The `filter` argument of the `Email/query`.
    pub condition: Value,
}

impl Filter {
    /// Resolves the filter's mailbox names against `mailboxes`. With no
    /// `in:`, the queue mailbox is the Inbox; `in:any` is every mailbox
    /// except Trash, Junk, Drafts, Sent, and the snooze mailbox. Drafts and
    /// Sent cannot be a queue. The `Email/query` filter is the queue
    /// mailbox's condition, then one per term in order, joined with `AND`
    /// when there are several.
    pub fn resolve(&self, mailboxes: &Mailboxes) -> Result<Resolved, Error> {
        let mut queue = None;
        let mut conditions = Vec::new();
        for term in &self.terms {
            let condition = match &term.condition {
                Condition::In(name) => {
                    let mailbox = mailboxes.resolve(name).map_err(Error::Mailbox)?;
                    let path = mailboxes.path(mailbox);
                    if !term.negated {
                        if matches!(mailbox.role, Some(Role::Drafts | Role::Sent)) {
                            return Err(Error::NotAQueue(path));
                        }
                        queue = Some((QueueMailbox::One(mailbox.id.clone()), path));
                        continue;
                    }
                    json!({"inMailbox": mailbox.id})
                }
                Condition::InAny => {
                    let excluded = mailboxes
                        .iter()
                        .filter(|m| {
                            matches!(
                                m.role,
                                Some(Role::Trash | Role::Junk | Role::Drafts | Role::Sent)
                            ) || (m.parent_id.is_none() && m.name == SNOOZE_MAILBOX)
                        })
                        .map(|m| m.id.clone())
                        .collect();
                    let mailbox = QueueMailbox::Any(excluded);
                    queue = Some((mailbox, "All mail".into()));
                    continue;
                }
                Condition::From(value) => json!({"from": value}),
                Condition::To(value) => json!({"to": value}),
                Condition::Subject(value) => json!({"subject": value}),
                Condition::Unread => json!({"notKeyword": "$seen"}),
                Condition::Read => json!({"hasKeyword": "$seen"}),
                Condition::Flagged => json!({"hasKeyword": "$flagged"}),
                Condition::List => json!({"header": ["List-Id"]}),
                Condition::Attachment => json!({"hasAttachment": true}),
                Condition::Text(value) => json!({"text": value}),
            };
            conditions.push(if term.negated {
                json!({"operator": "NOT", "conditions": [condition]})
            } else {
                condition
            });
        }
        let (mailbox, name) = match queue {
            Some(queue) => queue,
            None => {
                let inbox = mailboxes.by_role(Role::Inbox).ok_or(Error::NoInbox)?;
                (QueueMailbox::One(inbox.id.clone()), mailboxes.path(inbox))
            }
        };
        let first = match &mailbox {
            QueueMailbox::One(id) => json!({"inMailbox": id}),
            QueueMailbox::Any(excluded) => json!({"inMailboxOtherThan": excluded}),
        };
        conditions.insert(0, first);
        let condition = if conditions.len() == 1 {
            conditions.remove(0)
        } else {
            json!({"operator": "AND", "conditions": conditions})
        };
        Ok(Resolved {
            mailbox,
            name,
            condition,
        })
    }
}

/// Parses filter text: space-separated tokens, each `key:value` or bare
/// text, negated by a leading `-`, with double quotes around a value that
/// holds spaces. Keys and `is:`, `has:`, and `in:any` values are
/// case-insensitive.
pub fn parse(text: &str) -> Result<Filter, Error> {
    let mut terms = Vec::new();
    let mut chars = text.chars().peekable();
    let mut positive_in = false;
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let token = Token::read(&mut chars)?;
        let term = token.term()?;
        if let Term {
            negated,
            condition: Condition::In(_) | Condition::InAny,
        } = term
        {
            if negated && term.condition == Condition::InAny {
                return Err(Error::NegatedAny);
            }
            if !negated {
                if positive_in {
                    return Err(Error::SecondIn);
                }
                positive_in = true;
            }
        }
        terms.push(term);
    }
    Ok(Filter { terms })
}

/// One token as read: as written, and split into its key and value.
struct Token {
    raw: String,
    negated: bool,
    /// The text before the first `:` outside quotes, when no quote comes
    /// before it.
    key: Option<String>,
    value: String,
}

impl Token {
    /// Reads up to the next whitespace outside quotes.
    fn read(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<Token, Error> {
        let mut token = Token {
            raw: String::new(),
            negated: false,
            key: None,
            value: String::new(),
        };
        let mut quoted = false;
        let mut seen_quote = false;
        while let Some(c) = chars.next_if(|c| quoted || !c.is_whitespace()) {
            token.raw.push(c);
            match c {
                '"' => {
                    quoted = !quoted;
                    seen_quote = true;
                }
                '-' if token.raw.len() == 1 => token.negated = true,
                ':' if !quoted && !seen_quote && token.key.is_none() => {
                    token.key = Some(std::mem::take(&mut token.value));
                }
                c => token.value.push(c),
            }
        }
        if quoted {
            return Err(Error::UnbalancedQuote);
        }
        Ok(token)
    }

    fn term(self) -> Result<Term, Error> {
        if self.value.is_empty() {
            return Err(Error::MissingValue(self.raw));
        }
        let unknown = || Error::UnknownToken(self.raw.clone());
        let value = self.value.clone();
        let condition = match self.key.as_deref().map(str::to_lowercase).as_deref() {
            None => Condition::Text(value),
            Some("in") if value.eq_ignore_ascii_case("any") => Condition::InAny,
            Some("in") => Condition::In(value),
            Some("from") => Condition::From(value),
            Some("to") => Condition::To(value),
            Some("subject") => Condition::Subject(value),
            Some("is") => match value.to_lowercase().as_str() {
                "unread" => Condition::Unread,
                "read" => Condition::Read,
                "flagged" => Condition::Flagged,
                "list" => Condition::List,
                _ => return Err(unknown()),
            },
            Some("has") if value.eq_ignore_ascii_case("attachment") => Condition::Attachment,
            Some(_) => return Err(unknown()),
        };
        Ok(Term {
            negated: self.negated,
            condition,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn text(value: &str) -> Condition {
        Condition::Text(value.into())
    }

    fn term(condition: Condition) -> Term {
        Term {
            negated: false,
            condition,
        }
    }

    fn not(condition: Condition) -> Term {
        Term {
            negated: true,
            condition,
        }
    }

    /// Nested, mixed-case, and role mailboxes, with role mailboxes not named
    /// after their roles.
    fn mailboxes() -> Mailboxes {
        let mailbox =
            |id: &str, name: &str, parent: Option<&str>, role: Option<&str>| crate::jmap::Mailbox {
                id: id.into(),
                name: name.into(),
                parent_id: parent.map(Into::into),
                role: role.map(Into::into),
            };
        Mailboxes::from(vec![
            mailbox("mb-inbox", "Inbox", None, Some("inbox")),
            mailbox("mb-archive", "Archive", None, Some("archive")),
            mailbox("mb-trash", "Deleted Items", None, Some("trash")),
            mailbox("mb-junk", "Spam", None, Some("junk")),
            mailbox("mb-drafts", "Drafts", None, Some("drafts")),
            mailbox("mb-sent", "Sent Items", None, Some("sent")),
            mailbox("mb-fm-snoozed", "Snoozed", None, Some("snoozed")),
            mailbox("mb-snooze", "Snoozed (blorp)", None, None),
            mailbox("mb-travel", "TraVel", None, None),
            mailbox("mb-work", "Work", None, None),
            mailbox("mb-work-receipts", "Receipts", Some("mb-work"), None),
            mailbox("mb-work-done", "Done", Some("mb-work"), None),
            mailbox("mb-home", "Home", None, None),
            mailbox("mb-home-receipts", "Receipts", Some("mb-home"), None),
            mailbox("mb-done", "Done", None, None),
            mailbox("mb-old", "Old", None, None),
            mailbox("mb-old-inbox", "Inbox", Some("mb-old"), None),
        ])
    }

    fn one(id: &str) -> QueueMailbox {
        QueueMailbox::One(id.into())
    }

    #[test]
    fn a_filter_resolves_to_its_queue_mailbox_and_email_query_filter() {
        let any = QueueMailbox::Any(
            ["mb-trash", "mb-junk", "mb-drafts", "mb-sent", "mb-snooze"]
                .map(String::from)
                .into(),
        );
        let any_filter = json!({"inMailboxOtherThan": ["mb-trash", "mb-junk", "mb-drafts", "mb-sent", "mb-snooze"]});
        for (filter, mailbox, name, condition) in [
            (
                "",
                one("mb-inbox"),
                "Inbox",
                json!({"inMailbox": "mb-inbox"}),
            ),
            (
                "in:travel",
                one("mb-travel"),
                "TraVel",
                json!({"inMailbox": "mb-travel"}),
            ),
            (
                "in:TRAVEL",
                one("mb-travel"),
                "TraVel",
                json!({"inMailbox": "mb-travel"}),
            ),
            (
                "in:Work/Receipts",
                one("mb-work-receipts"),
                "Work/Receipts",
                json!({"inMailbox": "mb-work-receipts"}),
            ),
            (
                "in:home/receipts",
                one("mb-home-receipts"),
                "Home/Receipts",
                json!({"inMailbox": "mb-home-receipts"}),
            ),
            (
                "in:done",
                one("mb-done"),
                "Done",
                json!({"inMailbox": "mb-done"}),
            ),
            (
                "in:work/done",
                one("mb-work-done"),
                "Work/Done",
                json!({"inMailbox": "mb-work-done"}),
            ),
            (
                "in:inbox",
                one("mb-inbox"),
                "Inbox",
                json!({"inMailbox": "mb-inbox"}),
            ),
            (
                "in:old/inbox",
                one("mb-old-inbox"),
                "Old/Inbox",
                json!({"inMailbox": "mb-old-inbox"}),
            ),
            (
                "in:trash",
                one("mb-trash"),
                "Deleted Items",
                json!({"inMailbox": "mb-trash"}),
            ),
            (
                "in:junk",
                one("mb-junk"),
                "Spam",
                json!({"inMailbox": "mb-junk"}),
            ),
            (
                "in:Archive",
                one("mb-archive"),
                "Archive",
                json!({"inMailbox": "mb-archive"}),
            ),
            (
                "in:\"deleted items\"",
                one("mb-trash"),
                "Deleted Items",
                json!({"inMailbox": "mb-trash"}),
            ),
            (
                "in:snoozed",
                one("mb-fm-snoozed"),
                "Snoozed",
                json!({"inMailbox": "mb-fm-snoozed"}),
            ),
            (
                "in:\"Snoozed (blorp)\"",
                one("mb-snooze"),
                "Snoozed (blorp)",
                json!({"inMailbox": "mb-snooze"}),
            ),
            ("in:any", any.clone(), "All mail", any_filter.clone()),
            (
                "in:any is:unread",
                any.clone(),
                "All mail",
                json!({"operator": "AND", "conditions": [any_filter, {"notKeyword": "$seen"}]}),
            ),
            (
                "is:unread",
                one("mb-inbox"),
                "Inbox",
                json!({"operator": "AND", "conditions": [{"inMailbox": "mb-inbox"}, {"notKeyword": "$seen"}]}),
            ),
            (
                "is:flagged from:ana in:travel",
                one("mb-travel"),
                "TraVel",
                json!({"operator": "AND", "conditions": [
                    {"inMailbox": "mb-travel"},
                    {"hasKeyword": "$flagged"},
                    {"from": "ana"},
                ]}),
            ),
            (
                "is:read to:kim subject:\"Re: trip\" is:list has:attachment lunch",
                one("mb-inbox"),
                "Inbox",
                json!({"operator": "AND", "conditions": [
                    {"inMailbox": "mb-inbox"},
                    {"hasKeyword": "$seen"},
                    {"to": "kim"},
                    {"subject": "Re: trip"},
                    {"header": ["List-Id"]},
                    {"hasAttachment": true},
                    {"text": "lunch"},
                ]}),
            ),
            (
                "-is:unread -in:travel -\"out of office\"",
                one("mb-inbox"),
                "Inbox",
                json!({"operator": "AND", "conditions": [
                    {"inMailbox": "mb-inbox"},
                    {"operator": "NOT", "conditions": [{"notKeyword": "$seen"}]},
                    {"operator": "NOT", "conditions": [{"inMailbox": "mb-travel"}]},
                    {"operator": "NOT", "conditions": [{"text": "out of office"}]},
                ]}),
            ),
            (
                "-in:drafts",
                one("mb-inbox"),
                "Inbox",
                json!({"operator": "AND", "conditions": [
                    {"inMailbox": "mb-inbox"},
                    {"operator": "NOT", "conditions": [{"inMailbox": "mb-drafts"}]},
                ]}),
            ),
        ] {
            let resolved = parse(filter).unwrap().resolve(&mailboxes());
            assert_eq!(
                resolved,
                Ok(Resolved {
                    mailbox,
                    name: name.into(),
                    condition,
                }),
                "{filter:?}"
            );
        }
    }

    #[test]
    fn a_mailbox_name_matching_none_or_several_or_drafts_or_sent_is_an_error() {
        for (filter, error) in [
            ("in:Trav", "No mailbox matches \"Trav\""),
            (
                "in:Receipts",
                "Mailbox name \"Receipts\" is ambiguous: Work/Receipts, Home/Receipts",
            ),
            (
                "-in:Receipts",
                "Mailbox name \"Receipts\" is ambiguous: Work/Receipts, Home/Receipts",
            ),
            ("in:drafts", "Drafts cannot be a queue"),
            ("in:\"sent items\"", "Sent Items cannot be a queue"),
            ("in:Work/", "No mailbox matches \"Work/\""),
        ] {
            let resolved = parse(filter).unwrap().resolve(&mailboxes());
            assert_eq!(
                resolved.map_err(|e| e.to_string()),
                Err(error.to_string()),
                "{filter:?}"
            );
        }
    }

    #[test]
    fn a_filter_without_in_needs_an_inbox() {
        let filter = parse("is:unread").unwrap();

        assert_eq!(filter.resolve(&Mailboxes::default()), Err(Error::NoInbox));
    }

    #[test]
    fn filter_text_parses_to_its_terms() {
        for (filter, terms) in [
            ("", vec![]),
            ("   ", vec![]),
            ("in:Travel", vec![term(Condition::In("Travel".into()))]),
            ("in:any", vec![term(Condition::InAny)]),
            ("IN:ANY", vec![term(Condition::InAny)]),
            ("from:ana", vec![term(Condition::From("ana".into()))]),
            (
                "to:kim@example.invalid",
                vec![term(Condition::To("kim@example.invalid".into()))],
            ),
            (
                "subject:lunch",
                vec![term(Condition::Subject("lunch".into()))],
            ),
            ("is:unread", vec![term(Condition::Unread)]),
            ("is:read", vec![term(Condition::Read)]),
            ("is:flagged", vec![term(Condition::Flagged)]),
            ("is:list", vec![term(Condition::List)]),
            ("Is:Unread", vec![term(Condition::Unread)]),
            ("has:attachment", vec![term(Condition::Attachment)]),
            ("invoice", vec![term(text("invoice"))]),
            ("-invoice", vec![not(text("invoice"))]),
            ("-is:read", vec![not(Condition::Read)]),
            ("-in:Travel", vec![not(Condition::In("Travel".into()))]),
            (
                "from:\"Jane Doe\"",
                vec![term(Condition::From("Jane Doe".into()))],
            ),
            (
                "in:\"Snoozed (blorp)\"",
                vec![term(Condition::In("Snoozed (blorp)".into()))],
            ),
            ("\"lunch on friday\"", vec![term(text("lunch on friday"))]),
            ("\"is:unread\"", vec![term(text("is:unread"))]),
            ("-\"out of office\"", vec![not(text("out of office"))]),
            ("foo-bar", vec![term(text("foo-bar"))]),
            (
                "  is:unread   from:ana  trip ",
                vec![
                    term(Condition::Unread),
                    term(Condition::From("ana".into())),
                    term(text("trip")),
                ],
            ),
            (
                "from:ana from:kim",
                vec![
                    term(Condition::From("ana".into())),
                    term(Condition::From("kim".into())),
                ],
            ),
            ("subject:a:b", vec![term(Condition::Subject("a:b".into()))]),
            ("OR", vec![term(text("OR"))]),
        ] {
            assert_eq!(parse(filter), Ok(Filter { terms }), "{filter:?}");
        }
    }

    #[test]
    fn invalid_filter_text_is_an_error() {
        for (filter, error) in [
            ("foo:bar", "Unknown filter token \"foo:bar\""),
            ("is:spam", "Unknown filter token \"is:spam\""),
            ("is:invite", "Unknown filter token \"is:invite\""),
            ("has:star", "Unknown filter token \"has:star\""),
            ("-has:star", "Unknown filter token \"-has:star\""),
            ("from:\"Jane", "Unbalanced quote"),
            ("\"", "Unbalanced quote"),
            ("from:", "Missing value in \"from:\""),
            ("from:\"\"", "Missing value in \"from:\"\"\""),
            ("in:", "Missing value in \"in:\""),
            ("-", "Missing value in \"-\""),
            ("\"\"", "Missing value in \"\"\"\""),
            ("in:Travel in:Work", "More than one in: token"),
            ("in:any in:Travel", "More than one in: token"),
            ("-in:any", "in:any cannot be negated"),
        ] {
            assert_eq!(
                parse(filter).map_err(|e| e.to_string()),
                Err(error.to_string()),
                "{filter:?}"
            );
        }
    }
}
