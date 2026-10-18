//! An in-memory [`Jmap`] adapter over a small mail store, for tests.
//!
//! The store evaluates a subset of `Email/query`: `inMailbox`, sorting by
//! `receivedAt` in either direction, collapsing threads, `limit`, and
//! `calculateTotal`. Every get is held to Fastmail's `maxObjectsInGet`.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

pub mod api;

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
        self.call(Call::GetMailboxes, |store| {
            store.get_mailboxes(None).map(|(list, _)| list)
        })
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

    /// Evaluates `query` the way [`Jmap::query`] does: the same calls the
    /// reqwest adapter makes, each get held to [`MAX_OBJECTS_IN_GET`], with
    /// the threads' emails fetched in gets of at most that many ids.
    pub fn query(&self, query: &EmailQuery) -> Result<QueryResult, Error> {
        let (ids, total) = self.query_ids(query)?;
        let (hits, _) = self.get_emails(&ids)?;
        let thread_ids: Vec<String> = hits.into_iter().map(|e| e.thread_id).collect();
        let (threads, _) = self.get_threads(&thread_ids)?;
        let email_ids: Vec<String> = threads
            .iter()
            .flat_map(|t| t.email_ids.iter().cloned())
            .collect();
        let mut emails = Vec::new();
        for chunk in email_ids.chunks(MAX_OBJECTS_IN_GET) {
            emails.extend(self.get_emails(chunk)?.0);
        }
        Ok(QueryResult {
            ids,
            total,
            threads,
            emails,
        })
    }

    /// `Email/query` with `collapseThreads`: the ids of one email per
    /// matching thread, up to `query.limit`, and how many threads match.
    pub fn query_ids(&self, query: &EmailQuery) -> Result<(Vec<String>, usize), Error> {
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
        let mut ids: Vec<String> = matches
            .into_iter()
            .filter(|e| seen_threads.insert(e.thread_id.clone()))
            .map(|e| e.id.clone())
            .collect();
        let total = ids.len();
        ids.truncate(query.limit);
        Ok((ids, total))
    }

    /// `Mailbox/get`: the mailboxes with `ids`, or every mailbox for `None`,
    /// and the ids not found.
    pub fn get_mailboxes(
        &self,
        ids: Option<&[String]>,
    ) -> Result<(Vec<Mailbox>, Vec<String>), Error> {
        get_by_ids("Mailbox/get", &self.mailboxes, ids, |m| &m.id)
    }

    /// `Email/get`: the emails with `ids`, and the ids not found.
    pub fn get_emails(&self, ids: &[String]) -> Result<(Vec<Email>, Vec<String>), Error> {
        get_by_ids("Email/get", &self.emails, Some(ids), |e| &e.id)
    }

    /// `Thread/get`: the threads with `ids`, each listing its emails oldest
    /// first, and the ids not found.
    pub fn get_threads(&self, ids: &[String]) -> Result<(Vec<Thread>, Vec<String>), Error> {
        check_size("Thread/get", ids.len())?;
        let mut threads = Vec::new();
        let mut not_found = Vec::new();
        for id in ids {
            let mut members: Vec<&Email> =
                self.emails.iter().filter(|e| &e.thread_id == id).collect();
            if members.is_empty() {
                not_found.push(id.clone());
                continue;
            }
            members.sort_by_key(|e| e.received_at);
            threads.push(Thread {
                id: id.clone(),
                email_ids: members.iter().map(|e| e.id.clone()).collect(),
            });
        }
        Ok((threads, not_found))
    }
}

/// Fastmail's `maxObjectsInGet`: a get for more objects than this fails with
/// `requestTooLarge`.
pub const MAX_OBJECTS_IN_GET: usize = 4096;

/// Fastmail's `maxCallsInRequest`, which the local test server enforces on
/// each request it answers.
pub const MAX_CALLS_IN_REQUEST: usize = 50;

fn check_size(method: &str, requested: usize) -> Result<(), Error> {
    if requested > MAX_OBJECTS_IN_GET {
        return Err(Error::Method {
            method: method.into(),
            kind: "requestTooLarge".into(),
        });
    }
    Ok(())
}

/// A get over `objects` by id, or of all of them for `None`.
fn get_by_ids<T: Clone>(
    method: &str,
    objects: &[T],
    ids: Option<&[String]>,
    id_of: impl Fn(&T) -> &String,
) -> Result<(Vec<T>, Vec<String>), Error> {
    let Some(ids) = ids else {
        check_size(method, objects.len())?;
        return Ok((objects.to_vec(), Vec::new()));
    };
    check_size(method, ids.len())?;
    let mut found = Vec::new();
    let mut not_found = Vec::new();
    for id in ids {
        match objects.iter().find(|o| id_of(o) == id) {
            Some(object) => found.push(object.clone()),
            None => not_found.push(id.clone()),
        }
    }
    Ok((found, not_found))
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
    pub fn mailbox(self, id: &str, name: &str) -> Self {
        self.push_mailbox(id, name, None, None)
    }

    /// A top-level mailbox carrying a role such as `inbox`.
    pub fn role_mailbox(self, id: &str, name: &str, role: &str) -> Self {
        self.push_mailbox(id, name, None, Some(role))
    }

    /// A mailbox nested under `parent_id`.
    pub fn child_mailbox(self, id: &str, name: &str, parent_id: &str) -> Self {
        self.push_mailbox(id, name, Some(parent_id), None)
    }

    fn push_mailbox(
        mut self,
        id: &str,
        name: &str,
        parent_id: Option<&str>,
        role: Option<&str>,
    ) -> Self {
        self.store.mailboxes.push(Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: parent_id.map(Into::into),
            role: role.map(Into::into),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn inbox_query(limit: usize) -> EmailQuery {
        EmailQuery {
            filter: json!({"inMailbox": "mb-inbox"}),
            ascending: false,
            limit,
        }
    }

    /// `threads` threads in the Inbox, each of `per_thread` emails.
    fn inbox(threads: usize, per_thread: usize) -> Store {
        let mut builder = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
        for t in 0..threads {
            for e in 0..per_thread {
                let received = format!("2026-10-{:02}T{:02}:00:00Z", 1 + t % 28, e % 24);
                builder = builder.email(
                    EmailSpec::new(&format!("e{t}-{e}"), &format!("t{t}"), &received)
                        .in_mailbox("mb-inbox"),
                );
            }
        }
        builder.build()
    }

    #[tokio::test]
    async fn a_query_fetching_more_emails_than_max_objects_in_get_loads() {
        let jmap = FakeJmap::new(inbox(10, 410));

        let result = jmap.query(&inbox_query(500)).await.unwrap();

        assert_eq!(result.threads.len(), 10);
        assert_eq!(result.emails.len(), 4100);
    }

    #[test]
    fn a_query_fetching_exactly_max_objects_in_get_emails_succeeds() {
        let store = inbox(8, 512);

        let result = store.query(&inbox_query(500)).unwrap();

        assert_eq!(result.emails.len(), MAX_OBJECTS_IN_GET);
    }

    #[test]
    fn a_thread_of_more_emails_than_max_objects_in_get_loads_oldest_first() {
        let start: jiff::Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
        let mut builder = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
        for e in (0..5000).rev() {
            let received = start + jiff::SignedDuration::from_secs(e);
            builder = builder.email(
                EmailSpec::new(&format!("e{e}"), "t1", &received.to_string())
                    .in_mailbox("mb-inbox"),
            );
        }

        let result = builder.build().query(&inbox_query(500)).unwrap();

        let oldest_first: Vec<String> = (0..5000).map(|e| format!("e{e}")).collect();
        assert_eq!(result.threads.len(), 1);
        assert_eq!(result.threads[0].email_ids, oldest_first);
        let emails: Vec<&str> = result.emails.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(emails, oldest_first);
    }
}
