//! An in-memory [`Jmap`] adapter over a small mail store, for tests.
//!
//! The store evaluates a subset of `Email/query`: `inMailbox`, sorting by
//! `receivedAt` in either direction, collapsing threads, `limit`, and
//! `calculateTotal`. Every get is held to Fastmail's `maxObjectsInGet`.
//! `Email/set` applies `mailboxIds/<id>` and `keywords/<keyword>` patches.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

pub mod api;

use super::{Email, EmailAddress, EmailPatch, EmailQuery, EmailUpdate, Error, Jmap, Limits};
use super::{Mailbox, QueryResult, SetResult, Thread};

/// A call made to a [`FakeJmap`], in the order made.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    GetMailboxes,
    Query(EmailQuery),
    SetEmails(EmailUpdate),
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
        f: impl FnOnce(&mut Store) -> Result<T, Error>,
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
        f(&mut self.inner.lock().unwrap().store)
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

    async fn set_emails(&self, update: &EmailUpdate) -> Result<SetResult, Error> {
        self.call(Call::SetEmails(update.clone()), |store| {
            Ok(store.set_emails(update))
        })
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

    /// `Email/set` with `update`: applies each patch, and lists as not
    /// updated every unknown id and every patch with a path other than
    /// `mailboxIds/<id>` or `keywords/<keyword>` or a value other than
    /// `true` or `null`. Such a patch changes nothing.
    pub fn set_emails(&mut self, update: &EmailUpdate) -> SetResult {
        let mut not_updated = Vec::new();
        for (id, patch) in update {
            let Some(email) = self.emails.iter_mut().find(|e| &e.id == id) else {
                not_updated.push(id.clone());
                continue;
            };
            if !apply(email, patch) {
                not_updated.push(id.clone());
            }
        }
        SetResult { not_updated }
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
pub const MAX_OBJECTS_IN_GET: usize = Limits::FASTMAIL.max_objects_in_get;

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

/// Applies `patch` to `email` if every entry is one the store supports,
/// and says whether it did.
fn apply(email: &mut Email, patch: &EmailPatch) -> bool {
    let mut patched = email.clone();
    for (path, value) in patch {
        let (set, key) = match path.split_once('/') {
            Some(("mailboxIds", id)) => (&mut patched.mailbox_ids, id),
            Some(("keywords", keyword)) => (&mut patched.keywords, keyword),
            _ => return false,
        };
        if key.is_empty() {
            return false;
        }
        match value {
            Value::Bool(true) => {
                set.insert(key.to_string(), true);
            }
            Value::Null => {
                set.remove(key);
            }
            _ => return false,
        }
    }
    *email = patched;
    true
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

    fn patch(entries: &[(&str, Value)]) -> EmailPatch {
        entries
            .iter()
            .map(|(path, value)| (path.to_string(), value.clone()))
            .collect()
    }

    #[tokio::test]
    async fn set_emails_applies_mailbox_patches_and_reports_unknown_ids() {
        let store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .role_mailbox("mb-archive", "Archive", "archive")
            .email(EmailSpec::new("e1", "t1", "2026-10-01T00:00:00Z").in_mailbox("mb-inbox"))
            .build();
        let jmap = FakeJmap::new(store);
        let move_to_archive = patch(&[
            ("mailboxIds/mb-inbox", Value::Null),
            ("mailboxIds/mb-archive", Value::Bool(true)),
        ]);
        let update = EmailUpdate::from([
            ("e1".to_string(), move_to_archive.clone()),
            ("e9".to_string(), move_to_archive),
        ]);

        let result = jmap.set_emails(&update).await.unwrap();

        assert_eq!(result.not_updated, ["e9"]);
        assert_eq!(jmap.calls(), [Call::SetEmails(update)]);
        let inbox = jmap.query(&inbox_query(500)).await.unwrap();
        assert!(inbox.ids.is_empty());
        let archive = EmailQuery {
            filter: json!({"inMailbox": "mb-archive"}),
            ..inbox_query(500)
        };
        assert_eq!(jmap.query(&archive).await.unwrap().ids, ["e1"]);
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
