//! The in-memory snapshot of fetched mailboxes, threads, and emails.

use std::collections::{BTreeSet, HashMap};

use jiff::Timestamp;

use crate::jmap::{self, QueryResult};

/// Every mailbox in the account, in the order the server listed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailboxes(Vec<jmap::Mailbox>);

impl Mailboxes {
    /// The mailbox carrying the `inbox` role, if any.
    pub fn inbox(&self) -> Option<&jmap::Mailbox> {
        self.by_role("inbox")
    }

    /// The mailbox carrying the `archive` role, if any.
    pub fn archive(&self) -> Option<&jmap::Mailbox> {
        self.by_role("archive")
    }

    fn by_role(&self, role: &str) -> Option<&jmap::Mailbox> {
        self.0.iter().find(|m| m.role.as_deref() == Some(role))
    }

    /// The mailbox with `id`, if any.
    pub fn by_id(&self, id: &str) -> Option<&jmap::Mailbox> {
        self.0.iter().find(|m| m.id == id)
    }

    /// The mailbox's name after its parents' names, joined with `/`, as in
    /// `Work/Receipts`.
    pub fn path(&self, mailbox: &jmap::Mailbox) -> String {
        let mut names = vec![mailbox.name.as_str()];
        let mut parent = mailbox.parent_id.as_deref();
        while let Some(m) = parent.and_then(|id| self.by_id(id)) {
            // A parentId cycle would otherwise never reach the top.
            if names.len() > self.0.len() {
                break;
            }
            names.push(&m.name);
            parent = m.parent_id.as_deref();
        }
        names.reverse();
        names.join("/")
    }

    /// Each mailbox whose path holds `typed`, ignoring case, the Inbox
    /// first and the rest in path order.
    pub fn matching(&self, typed: &str) -> Vec<Listed> {
        let typed = typed.to_lowercase();
        let mut listed: Vec<(bool, Listed)> = self
            .0
            .iter()
            .map(|m| {
                let listed = Listed {
                    path: self.path(m),
                    id: m.id.clone(),
                };
                (m.role.as_deref() != Some("inbox"), listed)
            })
            .filter(|(_, m)| m.path.to_lowercase().contains(&typed))
            .collect();
        listed.sort();
        listed.into_iter().map(|(_, m)| m).collect()
    }

    /// Whether a move out of mailbox `from` moves `email`: it is in `from`,
    /// in no Drafts or Sent mailbox, and in no Trash or Junk mailbox other
    /// than `from`.
    pub fn moves(&self, email: &Email, from: &str) -> bool {
        let kept = |id: &String| {
            let role = self
                .0
                .iter()
                .find(|m| m.id == *id)
                .and_then(|m| m.role.as_deref());
            match role {
                Some("drafts" | "sent") => true,
                Some("trash" | "junk") => id != from,
                _ => false,
            }
        };
        email.mailbox_ids.contains(from) && !email.mailbox_ids.iter().any(kept)
    }
}

/// A mailbox as the mailbox list shows it: its path, and the id a switch
/// loads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Listed {
    pub path: String,
    pub id: String,
}

impl From<Vec<jmap::Mailbox>> for Mailboxes {
    fn from(mailboxes: Vec<jmap::Mailbox>) -> Self {
        Mailboxes(mailboxes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub id: String,
    /// The ids of the mailboxes the email is in.
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
            id: email.id,
            mailbox_ids: email.mailbox_ids.into_keys().collect(),
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

    /// True when any email in the mailbox with id `mailbox` lacks `$seen`.
    pub fn unread_in(&self, mailbox: &str) -> bool {
        self.emails
            .iter()
            .any(|e| !e.seen && e.mailbox_ids.contains(mailbox))
    }

    /// Sets whether each email with an id in `ids` is seen.
    pub fn set_seen(&mut self, ids: &[String], seen: bool) {
        for email in &mut self.emails {
            if ids.contains(&email.id) {
                email.seen = seen;
            }
        }
    }

    /// Whether the sender or the subject of the email that stands for the
    /// thread holds `search`, ignoring case. Every thread matches an empty
    /// search.
    pub fn matches(&self, search: &str) -> bool {
        if search.is_empty() {
            return true;
        }
        let search = search.to_lowercase();
        let hit = self.hit();
        [&hit.sender, &hit.subject]
            .into_iter()
            .flatten()
            .any(|text| text.to_lowercase().contains(&search))
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

/// The address of `thread` in Fastmail's web UI, under the mailbox named
/// `mailbox`: the thread's id and the id of the email the query matched,
/// its newest in the queue mailbox, joined by `.`. The web UI shows the
/// whole thread there with that email expanded. It adds its own `?u=`
/// parameter, so none is given here.
pub fn web_url(mailbox: &str, thread: &Thread) -> String {
    let mut url = url::Url::parse("https://app.fastmail.com/mail").expect("valid URL");
    url.path_segments_mut()
        .expect("URL with a path")
        .push(mailbox)
        .push(&format!("{}.{}", thread.id, thread.hit().id));
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread of emails with `ids`, standing for itself by the one at
    /// `hit`.
    fn thread(ids: &[&str], hit: usize) -> Thread {
        let emails = ids
            .iter()
            .map(|id| Email {
                id: id.to_string(),
                mailbox_ids: BTreeSet::new(),
                seen: true,
                flagged: false,
                sender: None,
                subject: None,
                received_at: Timestamp::UNIX_EPOCH,
            })
            .collect();
        Thread {
            id: "Tthread".into(),
            emails,
            hit,
        }
    }

    #[test]
    fn the_web_url_names_the_mailbox_the_thread_and_the_email_the_query_matched() {
        let thread = thread(&["e-old", "e-new", "e-sent"], 1);
        assert_eq!(
            web_url("Inbox", &thread),
            "https://app.fastmail.com/mail/Inbox/Tthread.e-new"
        );
    }

    #[test]
    fn the_web_url_escapes_the_mailbox_name() {
        let thread = thread(&["e1"], 0);
        for (mailbox, segment) in [
            ("Snoozed (blorp)", "Snoozed%20(blorp)"),
            ("A/B", "A%2FB"),
            ("Café", "Caf%C3%A9"),
            ("50%", "50%25"),
            ("Q?#", "Q%3F%23"),
        ] {
            assert_eq!(
                web_url(mailbox, &thread),
                format!("https://app.fastmail.com/mail/{segment}/Tthread.e1")
            );
        }
    }
}
