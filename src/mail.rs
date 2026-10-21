//! The in-memory snapshot of fetched mailboxes, threads, and emails.

use std::collections::HashMap;

use jiff::Timestamp;

use crate::jmap::{self, QueryResult};

/// Every mailbox in the account, in the order the server listed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailboxes(Vec<jmap::Mailbox>);

impl Mailboxes {
    /// The mailbox carrying the `inbox` role, if any.
    pub fn inbox(&self) -> Option<&jmap::Mailbox> {
        self.0.iter().find(|m| m.role.as_deref() == Some("inbox"))
    }
}

impl From<Vec<jmap::Mailbox>> for Mailboxes {
    fn from(mailboxes: Vec<jmap::Mailbox>) -> Self {
        Mailboxes(mailboxes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub id: String,
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
