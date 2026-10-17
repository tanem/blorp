//! An in-memory [`Jmap`] adapter over a small mail store, for tests.
//!
//! The store evaluates a subset of `Email/query`: `inMailbox`, sorting by
//! `receivedAt` in either direction, and collapsing threads.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::{Email, EmailAddress, EmailQuery, Error, Jmap, Mailbox, QueryResult, Thread};

/// A call made to a [`FakeJmap`], in the order made.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    GetMailboxes,
    Query(EmailQuery),
}

/// A fake JMAP server: a store, a call log, and failure injection.
#[derive(Clone)]
pub struct FakeJmap {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    store: Store,
    calls: Vec<Call>,
    injected: VecDeque<Injected>,
}

enum Injected {
    Fail(Error),
    Delay(Duration),
}

impl FakeJmap {
    pub fn new(store: Store) -> Self {
        FakeJmap {
            inner: Arc::new(Mutex::new(Inner {
                store,
                calls: Vec::new(),
                injected: VecDeque::new(),
            })),
        }
    }

    /// Makes the next `n` calls fail with `error`.
    pub fn fail_next(&self, n: usize, error: Error) {
        let mut inner = self.inner.lock().unwrap();
        let failures = std::iter::repeat_n(error, n).map(Injected::Fail);
        inner.injected.extend(failures);
    }

    /// Makes the next `n` calls wait `delay` before they answer.
    pub fn delay_next(&self, n: usize, delay: Duration) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .injected
            .extend((0..n).map(|_| Injected::Delay(delay)));
    }

    /// Every call made so far.
    pub fn calls(&self) -> Vec<Call> {
        self.inner.lock().unwrap().calls.clone()
    }

    async fn call<T>(
        &self,
        call: Call,
        f: impl FnOnce(&Store) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let injected = {
            let mut inner = self.inner.lock().unwrap();
            inner.calls.push(call);
            inner.injected.pop_front()
        };
        match injected {
            Some(Injected::Fail(error)) => return Err(error),
            Some(Injected::Delay(delay)) => tokio::time::sleep(delay).await,
            None => {}
        }
        f(&self.inner.lock().unwrap().store)
    }
}

impl Jmap for FakeJmap {
    async fn get_mailboxes(&self) -> Result<Vec<Mailbox>, Error> {
        self.call(Call::GetMailboxes, |store| Ok(store.mailboxes.clone()))
            .await
    }

    async fn query(&self, query: &EmailQuery) -> Result<QueryResult, Error> {
        self.call(Call::Query(query.clone()), |store| store.query(query))
            .await
    }
}

/// Synthetic mailboxes and emails.
#[derive(Debug, Clone, Default)]
pub struct Store {
    mailboxes: Vec<Mailbox>,
    emails: Vec<Email>,
}

impl Store {
    pub fn builder() -> StoreBuilder {
        StoreBuilder::default()
    }

    pub fn mailboxes(&self) -> Vec<Mailbox> {
        self.mailboxes.clone()
    }

    /// Evaluates `query` the way [`Jmap::query`] does.
    pub fn query(&self, query: &EmailQuery) -> Result<QueryResult, Error> {
        let mut matches: Vec<&Email> = Vec::new();
        for email in &self.emails {
            if matches_filter(email, &query.filter)? {
                matches.push(email);
            }
        }
        matches.sort_by(|a, b| {
            let order = a.received_at.cmp(&b.received_at);
            if query.ascending {
                order
            } else {
                order.reverse()
            }
        });
        let mut seen_threads = HashSet::new();
        let ids: Vec<String> = matches
            .into_iter()
            .filter(|e| seen_threads.insert(e.thread_id.clone()))
            .map(|e| e.id.clone())
            .collect();
        let mut threads = Vec::new();
        let mut emails = Vec::new();
        for id in &ids {
            let thread_id = &self.email(id).thread_id;
            let mut members: Vec<&Email> = self
                .emails
                .iter()
                .filter(|e| &e.thread_id == thread_id)
                .collect();
            members.sort_by_key(|e| e.received_at);
            threads.push(Thread {
                id: thread_id.clone(),
                email_ids: members.iter().map(|e| e.id.clone()).collect(),
            });
            emails.extend(members.into_iter().cloned());
        }
        Ok(QueryResult {
            ids,
            threads,
            emails,
        })
    }

    fn email(&self, id: &str) -> &Email {
        self.emails
            .iter()
            .find(|e| e.id == id)
            .expect("email in store")
    }
}

fn matches_filter(email: &Email, filter: &Value) -> Result<bool, Error> {
    let Value::Object(conditions) = filter else {
        return Err(unsupported_filter());
    };
    for (key, value) in conditions {
        let matched = match (key.as_str(), value) {
            ("inMailbox", Value::String(id)) => email.mailbox_ids.contains_key(id),
            _ => return Err(unsupported_filter()),
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn unsupported_filter() -> Error {
    Error::Method {
        method: "Email/query".into(),
        kind: "unsupportedFilter".into(),
    }
}

/// Builds a [`Store`].
#[derive(Debug, Default)]
pub struct StoreBuilder {
    store: Store,
}

impl StoreBuilder {
    /// A top-level mailbox with no role.
    pub fn mailbox(mut self, id: &str, name: &str) -> Self {
        self.store.mailboxes.push(Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: None,
            role: None,
        });
        self
    }

    /// A top-level mailbox carrying a role such as `inbox`.
    pub fn role_mailbox(mut self, id: &str, name: &str, role: &str) -> Self {
        self.store.mailboxes.push(Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: None,
            role: Some(role.into()),
        });
        self
    }

    /// A mailbox nested under `parent_id`.
    pub fn child_mailbox(mut self, id: &str, name: &str, parent_id: &str) -> Self {
        self.store.mailboxes.push(Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: Some(parent_id.into()),
            role: None,
        });
        self
    }

    pub fn email(mut self, email: EmailSpec) -> Self {
        self.store.emails.push(email.0);
        self
    }

    pub fn build(self) -> Store {
        self.store
    }
}

/// One synthetic email: unread, unflagged, in no mailbox, and with no
/// sender or subject until set.
#[derive(Debug, Clone)]
pub struct EmailSpec(Email);

impl EmailSpec {
    /// `received_at` is RFC 3339, such as `2026-10-17T09:30:00Z`.
    pub fn new(id: &str, thread_id: &str, received_at: &str) -> Self {
        EmailSpec(Email {
            id: id.into(),
            thread_id: thread_id.into(),
            mailbox_ids: Default::default(),
            keywords: Default::default(),
            from: None,
            subject: None,
            received_at: received_at.parse().expect("RFC 3339 timestamp"),
            has_attachment: false,
        })
    }

    pub fn from(mut self, name: &str, email: &str) -> Self {
        self.0.from = Some(vec![EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        }]);
        self
    }

    pub fn subject(mut self, subject: &str) -> Self {
        self.0.subject = Some(subject.into());
        self
    }

    pub fn in_mailbox(mut self, mailbox_id: &str) -> Self {
        self.0.mailbox_ids.insert(mailbox_id.into(), true);
        self
    }

    pub fn seen(mut self) -> Self {
        self.0.keywords.insert("$seen".into(), true);
        self
    }

    pub fn flagged(mut self) -> Self {
        self.0.keywords.insert("$flagged".into(), true);
        self
    }

    pub fn attachment(mut self) -> Self {
        self.0.has_attachment = true;
        self
    }
}
