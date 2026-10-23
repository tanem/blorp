//! An in-memory [`Jmap`] adapter over a small mail store, for tests.
//!
//! The store evaluates the `Email/query` blorp sends: the `inMailbox`
//! condition, sorting by `receivedAt` newest first, collapsing threads,
//! `limit`, and `calculateTotal`. Every get is held to Fastmail's
//! `maxObjectsInGet`, and the local test server holds every `Email/set` to
//! its `maxObjectsInSet`. `Email/set` applies `mailboxIds/<id>` patches.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub mod api;

use super::{Email, EmailAddress, EmailPatch, EmailQuery, EmailUpdate, Error, Jmap, Limits};
use super::{Mailbox, Push, PushStream, QueryResult, Reconciled, SetResult, Thread};

/// A call made to a [`FakeJmap`], in the order made.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// The session fetch.
    Session,
    GetMailboxes,
    Query(EmailQuery),
    Reconcile(EmailQuery),
    SetEmails(EmailUpdate),
}

/// A fake JMAP server: a store, a call log, failure injection, and push
/// streams the test sends events on. Opening a push stream is counted
/// apart from the call log and takes no injected failure or delay.
#[derive(Clone)]
pub struct FakeJmap {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    store: Store,
    calls: Vec<Call>,
    /// The calls that ran to the end, rather than being dropped while
    /// they waited.
    answered: Vec<Call>,
    injected: VecDeque<Injected>,
    /// The senders of the push streams opened and not yet closed.
    streams: Vec<UnboundedSender<Result<Push, Error>>>,
    /// How many push streams have been asked for.
    streams_opened: usize,
    /// The errors the next push stream openings fail with, in turn.
    refused_streams: VecDeque<Error>,
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
                answered: Vec::new(),
                injected: VecDeque::new(),
                streams: Vec::new(),
                streams_opened: 0,
                refused_streams: VecDeque::new(),
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

    /// Changes the store behind blorp's back, as the web client would.
    pub fn change(&self, f: impl FnOnce(&mut Store)) {
        f(&mut self.inner.lock().unwrap().store);
    }

    /// Sends `event` on every open push stream.
    pub fn push(&self, event: Push) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .streams
            .retain(|stream| stream.send(Ok(event)).is_ok());
    }

    /// Ends every open push stream, as a dropped connection does.
    pub fn close_streams(&self) {
        self.inner.lock().unwrap().streams.clear();
    }

    /// Makes the next `n` push stream openings fail with HTTP 503.
    pub fn refuse_streams(&self, n: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .refused_streams
            .extend(std::iter::repeat_n(Error::Status(503), n));
    }

    /// Makes the next push stream opening fail with `error`.
    pub fn refuse_stream_with(&self, error: Error) {
        self.inner.lock().unwrap().refused_streams.push_back(error);
    }

    /// How many push streams have been asked for, including those refused.
    pub fn streams_opened(&self) -> usize {
        self.inner.lock().unwrap().streams_opened
    }

    /// Every call made so far.
    pub fn calls(&self) -> Vec<Call> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// Every call that ran to the end, in the order they ended. A call
    /// dropped while it waited out a delay is not here.
    pub fn answered(&self) -> Vec<Call> {
        self.inner.lock().unwrap().answered.clone()
    }

    /// Answers a session fetch with this fake, as connecting does with a
    /// client bound to the account.
    pub async fn connect(&self) -> Result<FakeJmap, Error> {
        let fake = self.clone();
        self.call(Call::Session, |_| Ok(fake)).await
    }

    async fn call<T>(
        &self,
        call: Call,
        f: impl FnOnce(&mut Store) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let injected = {
            let mut inner = self.inner.lock().unwrap();
            inner.calls.push(call.clone());
            inner.injected.pop_front()
        };
        let delayed = match injected {
            Some(Injected::Fail(error)) => Err(error),
            Some(Injected::Delay(delay)) => {
                tokio::time::sleep(delay).await;
                Ok(())
            }
            None => Ok(()),
        };
        let mut inner = self.inner.lock().unwrap();
        inner.answered.push(call);
        delayed?;
        f(&mut inner.store)
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

    async fn reconcile(&self, query: &EmailQuery) -> Result<Reconciled, Error> {
        self.call(Call::Reconcile(query.clone()), |store| {
            store.reconcile(query)
        })
        .await
    }

    async fn events(&self) -> Result<Option<PushStream>, Error> {
        let mut inner = self.inner.lock().unwrap();
        inner.streams_opened += 1;
        if let Some(error) = inner.refused_streams.pop_front() {
            return Err(error);
        }
        let (tx, rx) = unbounded_channel();
        inner.streams.push(tx);
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        });
        Ok(Some(stream.boxed()))
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

    /// Adds `email`, as a delivery or a filing rule would.
    pub fn deliver(&mut self, email: EmailSpec) {
        self.emails.push(email.email);
    }

    /// Changes the email with `id` through `change`, as the web client
    /// would. Panics when there is no such email.
    pub fn change_email(&mut self, id: &str, change: impl FnOnce(&mut Email)) {
        let email = self
            .emails
            .iter_mut()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("no email {id} in the store"));
        change(email);
    }

    /// Renames the mailbox with `id`, as the web client would.
    pub fn rename_mailbox(&mut self, id: &str, name: &str) {
        if let Some(mailbox) = self.mailboxes.iter_mut().find(|m| m.id == id) {
            mailbox.name = name.into();
        }
    }

    /// Deletes the mailbox with `id` and takes its emails out of it, as the
    /// web client would.
    pub fn delete_mailbox(&mut self, id: &str) {
        self.mailboxes.retain(|m| m.id != id);
        for email in &mut self.emails {
            email.mailbox_ids.remove(id);
        }
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

    /// Evaluates `query` the way [`Jmap::reconcile`] does: every mailbox,
    /// then [`Store::query`].
    pub fn reconcile(&self, query: &EmailQuery) -> Result<Reconciled, Error> {
        let (mailboxes, _) = self.get_mailboxes(None)?;
        let result = self.query(query)?;
        Ok(Reconciled { mailboxes, result })
    }

    /// `Email/query` with `collapseThreads`, newest first: the ids of one
    /// email per matching thread, up to `query.limit`, and how many threads
    /// match.
    pub fn query_ids(&self, query: &EmailQuery) -> Result<(Vec<String>, usize), Error> {
        let mut matches: Vec<&Email> = Vec::new();
        for email in &self.emails {
            if matches_filter(email, &query.filter)? {
                matches.push(email);
            }
        }
        matches.sort_by_key(|e| std::cmp::Reverse(e.received_at));
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

/// Fastmail's `maxObjectsInSet`: an `Email/set` of more updates than this
/// fails with `requestTooLarge`.
pub const MAX_OBJECTS_IN_SET: usize = Limits::FASTMAIL.max_objects_in_set;

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
        let Some((set, key)) = path
            .split_once('/')
            .filter(|(_, key)| !key.is_empty())
            .and_then(|(set, key)| match set {
                "mailboxIds" => Some((&mut patched.mailbox_ids, key)),
                "keywords" => Some((&mut patched.keywords, key)),
                _ => None,
            })
        else {
            return false;
        };
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

/// Whether `email` matches `filter`, an `inMailbox` condition.
fn matches_filter(email: &Email, filter: &Value) -> Result<bool, Error> {
    let Value::Object(filter) = filter else {
        return Err(unsupported_filter());
    };
    let mut matched = true;
    for (key, value) in filter {
        matched &= match (key.as_str(), value) {
            ("inMailbox", Value::String(id)) => email.mailbox_ids.contains_key(id),
            _ => return Err(unsupported_filter()),
        };
    }
    Ok(matched)
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
    /// A mailbox with no role.
    pub fn mailbox(self, id: &str, name: &str) -> Self {
        self.push_mailbox(id, name, None)
    }

    /// A mailbox carrying a role such as `inbox`.
    pub fn role_mailbox(self, id: &str, name: &str, role: &str) -> Self {
        self.push_mailbox(id, name, Some(role))
    }

    /// A mailbox with no role inside the mailbox with id `parent`.
    pub fn child_mailbox(mut self, id: &str, name: &str, parent: &str) -> Self {
        self = self.push_mailbox(id, name, None);
        if let Some(mailbox) = self.store.mailboxes.last_mut() {
            mailbox.parent_id = Some(parent.into());
        }
        self
    }

    fn push_mailbox(mut self, id: &str, name: &str, role: Option<&str>) -> Self {
        self.store.mailboxes.push(Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: None,
            role: role.map(Into::into),
        });
        self
    }

    pub fn email(mut self, email: EmailSpec) -> Self {
        self.store.emails.push(email.email);
        self
    }

    pub fn build(self) -> Store {
        self.store
    }
}

/// One synthetic email: unread, unflagged, in no mailbox, and with no
/// sender or subject until set.
#[derive(Debug, Clone)]
pub struct EmailSpec {
    email: Email,
}

impl EmailSpec {
    /// `received_at` is RFC 3339, such as `2026-10-17T09:30:00Z`.
    pub fn new(id: &str, thread_id: &str, received_at: &str) -> Self {
        let email = Email {
            id: id.into(),
            thread_id: thread_id.into(),
            mailbox_ids: Default::default(),
            keywords: Default::default(),
            from: None,
            subject: None,
            received_at: received_at.parse().expect("RFC 3339 timestamp"),
        };
        EmailSpec { email }
    }

    pub fn from(mut self, name: &str, email: &str) -> Self {
        self.email.from = Some(vec![EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        }]);
        self
    }

    pub fn subject(mut self, subject: &str) -> Self {
        self.email.subject = Some(subject.into());
        self
    }

    pub fn in_mailbox(mut self, mailbox_id: &str) -> Self {
        self.email.mailbox_ids.insert(mailbox_id.into(), true);
        self
    }

    pub fn seen(mut self) -> Self {
        self.email.keywords.insert("$seen".into(), true);
        self
    }

    pub fn flagged(mut self) -> Self {
        self.email.keywords.insert("$flagged".into(), true);
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

    #[test]
    fn a_query_matches_the_mailbox_newest_first_and_collapses_threads() {
        let store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .mailbox("mb-travel", "Travel")
            .email(EmailSpec::new("e1", "t1", "2026-10-03T00:00:00Z").in_mailbox("mb-inbox"))
            .email(EmailSpec::new("e2", "t2", "2026-10-05T00:00:00Z").in_mailbox("mb-inbox"))
            .email(EmailSpec::new("e3", "t3", "2026-10-04T00:00:00Z").in_mailbox("mb-travel"))
            .email(EmailSpec::new("e4a", "t4", "2026-10-01T00:00:00Z").in_mailbox("mb-inbox"))
            .email(EmailSpec::new("e4b", "t4", "2026-10-02T00:00:00Z").in_mailbox("mb-inbox"))
            .build();

        let (ids, total) = store.query_ids(&inbox_query(500)).unwrap();
        assert_eq!(ids, ["e2", "e1", "e4b"]);
        assert_eq!(total, 3);

        let (ids, total) = store.query_ids(&inbox_query(2)).unwrap();
        assert_eq!(ids, ["e2", "e1"]);
        assert_eq!(total, 3);
    }

    #[test]
    fn a_query_outside_the_subset_is_unsupported() {
        let store = inbox(1, 1);
        for filter in [
            json!({"hasKeyword": "$seen"}),
            json!({"operator": "AND", "conditions": []}),
            json!({"inMailbox": 1}),
        ] {
            let query = EmailQuery {
                filter: filter.clone(),
                limit: 500,
            };
            assert_eq!(
                store.query_ids(&query),
                Err(unsupported_filter()),
                "{filter}"
            );
        }
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

    #[test]
    fn a_change_to_an_email_shows_in_the_next_query() {
        let mut store = inbox(2, 1);

        store.change_email("e0-0", |email| email.mailbox_ids.clear());

        let (ids, _) = store.query_ids(&inbox_query(500)).unwrap();
        assert_eq!(ids, ["e1-0"]);
    }
}
