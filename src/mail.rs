//! The in-memory snapshot of fetched mailboxes, threads, and emails.

use std::collections::{BTreeSet, HashMap};

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

/// Every mailbox in the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailboxes(Vec<Mailbox>);

impl Mailboxes {
    /// The mailbox carrying `role`, if any.
    pub fn by_role(&self, role: Role) -> Option<&Mailbox> {
        self.0.iter().find(|m| m.role == Some(role))
    }

    pub fn by_id(&self, id: &str) -> Option<&Mailbox> {
        self.0.iter().find(|m| m.id == id)
    }

    /// Whether a move action out of the queue mailbox `from` moves `email`:
    /// it is in `from`, and in no Trash, Junk, Drafts, or Sent mailbox
    /// other than `from`.
    pub fn moves(&self, email: &Email, from: &str) -> bool {
        email.mailbox_ids.contains(from)
            && !email.mailbox_ids.iter().any(|id| {
                id != from
                    && self.by_id(id).and_then(|m| m.role).is_some_and(|role| {
                        matches!(role, Role::Trash | Role::Junk | Role::Drafts | Role::Sent)
                    })
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
