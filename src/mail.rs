//! The in-memory snapshot of fetched mailboxes, threads, and emails.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use jiff::Timestamp;

use crate::jmap::{self, QueryResult};

/// A system mailbox role, as Fastmail reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inbox,
    Archive,
    Trash,
    Junk,
    Snoozed,
    Drafts,
    Sent,
}

impl Role {
    fn parse(role: &str) -> Option<Role> {
        Some(match role {
            "inbox" => Role::Inbox,
            "archive" => Role::Archive,
            "trash" => Role::Trash,
            "junk" => Role::Junk,
            "snoozed" => Role::Snoozed,
            "drafts" => Role::Drafts,
            "sent" => Role::Sent,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Option<Role>,
}

/// The name of the snooze mailbox, which blorp creates without a role.
pub const SNOOZE_MAILBOX: &str = "Snoozed (blorp)";

/// The mailbox a queue shows: one mailbox, or for `in:any` every mailbox
/// but those excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueMailbox {
    One(String),
    /// Every mailbox except these ids, in the order the server listed them.
    Any(Vec<String>),
}

impl QueueMailbox {
    /// Whether the mailbox with `id` is part of the queue mailbox.
    pub fn contains(&self, id: &str) -> bool {
        match self {
            QueueMailbox::One(one) => one == id,
            QueueMailbox::Any(excluded) => !excluded.iter().any(|e| e == id),
        }
    }
}

/// Why a mailbox name resolved to no single mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Holds the name as written.
    NoMatch(String),
    /// Holds the name as written and the path of each match.
    Ambiguous(String, Vec<String>),
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::NoMatch(name) => write!(f, "No mailbox matches \"{name}\""),
            NameError::Ambiguous(name, paths) => write!(
                f,
                "Mailbox name \"{name}\" is ambiguous: {}",
                paths.join(", ")
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Every mailbox in the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailboxes(Vec<Mailbox>);

impl Mailboxes {
    /// The mailbox carrying `role`, if any.
    pub fn by_role(&self, role: Role) -> Option<&Mailbox> {
        self.0.iter().find(|m| m.role == Some(role))
    }

    /// The mailbox with `id`, if any.
    pub fn by_id(&self, id: &str) -> Option<&Mailbox> {
        self.0.iter().find(|m| m.id == id)
    }

    /// Every mailbox, in the order the server listed them.
    pub fn iter(&self) -> impl Iterator<Item = &Mailbox> {
        self.0.iter()
    }

    /// The mailbox's name after its parents' names, joined with `/`, as in
    /// `Work/Receipts`.
    pub fn path(&self, mailbox: &Mailbox) -> String {
        let mut names = vec![mailbox.name.as_str()];
        let mut parent = mailbox.parent_id.as_deref();
        while let Some(m) = parent.and_then(|id| self.by_id(id)) {
            if names.len() > self.0.len() {
                break;
            }
            names.push(&m.name);
            parent = m.parent_id.as_deref();
        }
        names.reverse();
        names.join("/")
    }

    /// The mailbox `name` refers to, matching case-insensitively and
    /// exactly: the role names `inbox`, `archive`, `trash`, and `junk` match
    /// the mailbox carrying that role, then a name matches a mailbox's path,
    /// as in `Work/Receipts`, and failing that its own name. A name matching
    /// none, or several at the same step, is an error.
    pub fn resolve(&self, name: &str) -> Result<&Mailbox, NameError> {
        let lower = name.to_lowercase();
        let role = match lower.as_str() {
            "inbox" => Some(Role::Inbox),
            "archive" => Some(Role::Archive),
            "trash" => Some(Role::Trash),
            "junk" => Some(Role::Junk),
            _ => None,
        };
        if let Some(mailbox) = role.and_then(|role| self.by_role(role)) {
            return Ok(mailbox);
        }
        let by_path: Vec<&Mailbox> = self
            .0
            .iter()
            .filter(|m| self.path(m).to_lowercase() == lower)
            .collect();
        let matches = if by_path.is_empty() {
            self.0
                .iter()
                .filter(|m| m.name.to_lowercase() == lower)
                .collect()
        } else {
            by_path
        };
        match matches.as_slice() {
            [] => Err(NameError::NoMatch(name.into())),
            [one] => Ok(one),
            several => Err(NameError::Ambiguous(
                name.into(),
                several.iter().map(|m| self.path(m)).collect(),
            )),
        }
    }

    /// Whether a move action out of the queue mailbox `from` moves `email`.
    /// It moves an email in `from` that is in no Drafts or Sent mailbox and
    /// in no Trash or Junk mailbox outside `from`, so a Trash or Junk queue
    /// can move its own emails out.
    pub fn moves(&self, email: &Email, from: &QueueMailbox) -> bool {
        email.mailbox_ids.iter().any(|id| from.contains(id))
            && !email
                .mailbox_ids
                .iter()
                .any(|id| match self.by_id(id).and_then(|m| m.role) {
                    Some(Role::Drafts | Role::Sent) => true,
                    Some(Role::Trash | Role::Junk) => !from.contains(id),
                    _ => false,
                })
    }
}

impl From<Vec<jmap::Mailbox>> for Mailboxes {
    fn from(mailboxes: Vec<jmap::Mailbox>) -> Self {
        Mailboxes(
            mailboxes
                .into_iter()
                .map(|m| Mailbox {
                    role: m.role.as_deref().and_then(Role::parse),
                    id: m.id,
                    name: m.name,
                    parent_id: m.parent_id,
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub id: String,
    pub mailbox_ids: BTreeSet<String>,
    pub seen: bool,
    pub flagged: bool,
    /// The first `From` address's name, or its address when it has none.
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub received_at: Timestamp,
}

impl From<jmap::Email> for Email {
    fn from(email: jmap::Email) -> Self {
        let sender =
            email
                .from
                .and_then(|from| from.into_iter().next())
                .map(|address| match address.name {
                    Some(name) if !name.is_empty() => name,
                    _ => address.email,
                });
        Email {
            seen: email.keywords.contains_key("$seen"),
            flagged: email.keywords.contains_key("$flagged"),
            mailbox_ids: email.mailbox_ids.into_keys().collect(),
            id: email.id,
            sender,
            subject: email.subject,
            received_at: email.received_at,
        }
    }
}

/// A thread in the queue: every one of its emails, and the one the query
/// matched, which stands for the thread in its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    /// Oldest first, as `Thread/get` orders them.
    pub emails: Vec<Email>,
    hit: usize,
}

impl Thread {
    /// The threads of a query result, in query order.
    pub fn from_query(result: QueryResult) -> Vec<Thread> {
        let mut emails: HashMap<String, jmap::Email> = result
            .emails
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let mut threads: HashMap<String, jmap::Thread> = result
            .threads
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        let mut queue = Vec::with_capacity(result.ids.len());
        for hit_id in result.ids {
            let Some(thread_id) = emails.get(&hit_id).map(|e| e.thread_id.clone()) else {
                continue;
            };
            let Some(thread) = threads.remove(&thread_id) else {
                continue;
            };
            let members: Vec<Email> = thread
                .email_ids
                .iter()
                .filter_map(|id| emails.remove(id))
                .map(Email::from)
                .collect();
            let Some(hit) = members.iter().position(|e| e.id == hit_id) else {
                continue;
            };
            queue.push(Thread {
                id: thread.id,
                emails: members,
                hit,
            });
        }
        queue
    }

    /// The email the query matched.
    pub fn hit(&self) -> &Email {
        &self.emails[self.hit]
    }

    /// True when any email lacks `$seen`.
    pub fn unread(&self) -> bool {
        self.emails.iter().any(|e| !e.seen)
    }

    /// True when any email has `$flagged`.
    pub fn flagged(&self) -> bool {
        self.emails.iter().any(|e| e.flagged)
    }

    /// The number of emails in the whole thread, across mailboxes.
    pub fn message_count(&self) -> usize {
        self.emails.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailboxes() -> Mailboxes {
        let mailbox = |id: &str, role: Option<Role>| Mailbox {
            id: id.into(),
            name: id.into(),
            parent_id: None,
            role,
        };
        Mailboxes(vec![
            mailbox("inbox", Some(Role::Inbox)),
            mailbox("archive", Some(Role::Archive)),
            mailbox("trash", Some(Role::Trash)),
            mailbox("junk", Some(Role::Junk)),
            mailbox("drafts", Some(Role::Drafts)),
            mailbox("sent", Some(Role::Sent)),
            mailbox("travel", None),
        ])
    }

    fn email(mailbox_ids: &[&str]) -> Email {
        Email {
            id: "e1".into(),
            mailbox_ids: mailbox_ids.iter().map(|id| id.to_string()).collect(),
            seen: false,
            flagged: false,
            sender: None,
            subject: None,
            received_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_move_takes_only_emails_in_the_queue_mailbox_and_never_drafts_or_sent() {
        let m = mailboxes();
        for (mailbox_ids, from, moved) in [
            (&["inbox"][..], "inbox", true),
            (&["travel"], "inbox", false),
            (&["inbox", "travel"], "inbox", true),
            (&["inbox", "sent"], "inbox", false),
            (&["inbox", "drafts"], "inbox", false),
            (&["inbox", "trash"], "inbox", false),
            (&["inbox", "junk"], "inbox", false),
            (&["trash"], "trash", true),
            (&["trash", "inbox"], "trash", true),
            (&["trash", "junk"], "trash", false),
            (&["archive", "inbox"], "archive", true),
            (&["junk"], "junk", true),
            (&["trash", "sent"], "trash", false),
            (&["sent"], "sent", false),
            (&["drafts"], "drafts", false),
        ] {
            assert_eq!(
                m.moves(&email(mailbox_ids), &QueueMailbox::One(from.into())),
                moved,
                "{mailbox_ids:?} from {from}"
            );
        }
    }
}
