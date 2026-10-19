//! An in-memory [`Jmap`] adapter over a small mail store, for tests.
//!
//! The store evaluates a subset of `Email/query`: the conditions
//! `inMailbox`, `inMailboxOtherThan`, `hasKeyword`, `notKeyword`,
//! `hasAttachment`, and `from`, `to`, `subject`, and `text` as
//! case-insensitive substring matches, where `text` covers from, to, and
//! subject; the operators `AND` and `NOT`; sorting by `receivedAt` in either
//! direction; collapsing threads; `limit`; and `calculateTotal`. Every get
//! is held to Fastmail's `maxObjectsInGet`, and the local test server holds
//! every `Email/set` to its `maxObjectsInSet`.
//! `Email/set` applies `mailboxIds/<id>` and `keywords/<keyword>` patches.

use std::collections::{HashMap, HashSet, VecDeque};
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
    GetEmails(Vec<String>),
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
    /// The calls that ran to the end, rather than being dropped while
    /// they waited.
    answered: Vec<Call>,
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
                answered: Vec::new(),
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

    /// Every call that ran to the end, in the order they ended. A call
    /// dropped while it waited out a delay is not here.
    pub fn answered(&self) -> Vec<Call> {
        self.inner.lock().unwrap().answered.clone()
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

    async fn get_emails(&self, ids: &[String]) -> Result<Vec<Email>, Error> {
        self.call(Call::GetEmails(ids.to_vec()), |store| {
            let mut emails = Vec::new();
            for chunk in ids.chunks(MAX_OBJECTS_IN_GET) {
                emails.extend(store.get_emails(chunk)?.0);
            }
            Ok(emails)
        })
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
    /// Each email's `To` addresses by email id, which blorp does not fetch
    /// but `to` and `text` conditions match.
    to: HashMap<String, Vec<EmailAddress>>,
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
            let to = self
                .to
                .get(&email.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if matches_filter(email, to, &query.filter)? {
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

/// Whether `email`, sent to `to`, matches `filter`: a `FilterOperator` or a
/// `FilterCondition` whose properties must all match.
fn matches_filter(email: &Email, to: &[EmailAddress], filter: &Value) -> Result<bool, Error> {
    let Value::Object(filter) = filter else {
        return Err(unsupported_filter());
    };
    if let Some(operator) = filter.get("operator") {
        let Some(Value::Array(conditions)) = filter.get("conditions") else {
            return Err(unsupported_filter());
        };
        let mut any = false;
        let mut all = true;
        for condition in conditions {
            let matched = matches_filter(email, to, condition)?;
            any |= matched;
            all &= matched;
        }
        return match operator.as_str() {
            Some("AND") => Ok(all),
            Some("NOT") => Ok(!any),
            _ => Err(unsupported_filter()),
        };
    }
    let from = email.from.as_deref().unwrap_or_default();
    let subject = email.subject.as_deref().unwrap_or_default();
    for (key, value) in filter {
        let matched = match (key.as_str(), value) {
            ("inMailbox", Value::String(id)) => email.mailbox_ids.contains_key(id),
            ("inMailboxOtherThan", Value::Array(ids)) => {
                let ids: Vec<&str> = ids.iter().filter_map(Value::as_str).collect();
                email
                    .mailbox_ids
                    .keys()
                    .any(|id| !ids.contains(&id.as_str()))
            }
            ("hasKeyword", Value::String(k)) => email.keywords.contains_key(k),
            ("notKeyword", Value::String(k)) => !email.keywords.contains_key(k),
            ("hasAttachment", Value::Bool(has)) => email.has_attachment == *has,
            ("from", Value::String(text)) => addresses_contain(from, text),
            ("to", Value::String(text)) => addresses_contain(to, text),
            ("subject", Value::String(text)) => contains(subject, text),
            ("text", Value::String(text)) => {
                addresses_contain(from, text)
                    || addresses_contain(to, text)
                    || contains(subject, text)
            }
            _ => return Err(unsupported_filter()),
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `haystack` contains `needle`, ignoring case.
fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Whether any address's name or email contains `text`, ignoring case.
fn addresses_contain(addresses: &[EmailAddress], text: &str) -> bool {
    addresses.iter().any(|a| {
        a.name.as_deref().is_some_and(|name| contains(name, text)) || contains(&a.email, text)
    })
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
        if !email.to.is_empty() {
            self.store.to.insert(email.email.id.clone(), email.to);
        }
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
    to: Vec<EmailAddress>,
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
            has_attachment: false,
        };
        EmailSpec {
            email,
            to: Vec::new(),
        }
    }

    pub fn from(mut self, name: &str, email: &str) -> Self {
        self.email.from = Some(vec![EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        }]);
        self
    }

    /// Adds a `To` address.
    pub fn to(mut self, name: &str, email: &str) -> Self {
        self.to.push(EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        });
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

    pub fn attachment(mut self) -> Self {
        self.email.has_attachment = true;
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

    #[test]
    fn a_query_evaluates_the_filter_subset_per_email_and_collapses_threads() {
        let store = Store::builder()
            .role_mailbox("mb-inbox", "Inbox", "inbox")
            .role_mailbox("mb-trash", "Trash", "trash")
            .mailbox("mb-travel", "Travel")
            .email(
                EmailSpec::new("e1", "t1", "2026-10-05T00:00:00Z")
                    .in_mailbox("mb-inbox")
                    .from("Ana Lopez", "ana@example.invalid")
                    .to("Kim Park", "kim@example.invalid")
                    .subject("Lunch on Friday"),
            )
            .email(
                EmailSpec::new("e2", "t2", "2026-10-04T00:00:00Z")
                    .in_mailbox("mb-inbox")
                    .seen()
                    .flagged()
                    .from("Ravi Shah", "ravi@example.invalid")
                    .to("Ana Lopez", "ana@example.invalid")
                    .subject("Invoice")
                    .attachment(),
            )
            .email(
                EmailSpec::new("e3", "t3", "2026-10-03T00:00:00Z")
                    .in_mailbox("mb-travel")
                    .seen()
                    .from("", "bookings@air.example.invalid")
                    .subject("Your flight"),
            )
            .email(
                EmailSpec::new("e4a", "t4", "2026-10-01T00:00:00Z")
                    .in_mailbox("mb-trash")
                    .seen()
                    .subject("Old news"),
            )
            .email(
                EmailSpec::new("e4b", "t4", "2026-10-02T00:00:00Z")
                    .in_mailbox("mb-inbox")
                    .subject("Re: Old news"),
            )
            .build();
        for (filter, ids) in [
            (json!({}), vec!["e1", "e2", "e3", "e4b"]),
            (json!({"inMailbox": "mb-inbox"}), vec!["e1", "e2", "e4b"]),
            (
                json!({"inMailboxOtherThan": ["mb-trash", "mb-travel"]}),
                vec!["e1", "e2", "e4b"],
            ),
            (
                json!({"inMailboxOtherThan": ["mb-inbox"]}),
                vec!["e3", "e4a"],
            ),
            (json!({"hasKeyword": "$seen"}), vec!["e2", "e3", "e4a"]),
            (json!({"notKeyword": "$seen"}), vec!["e1", "e4b"]),
            (json!({"hasKeyword": "$flagged"}), vec!["e2"]),
            (json!({"from": "ANA"}), vec!["e1"]),
            (json!({"from": "air.example"}), vec!["e3"]),
            (json!({"to": "lopez"}), vec!["e2"]),
            (json!({"subject": "old NEWS"}), vec!["e4b"]),
            (json!({"text": "ana"}), vec!["e1", "e2"]),
            (json!({"text": "flight"}), vec!["e3"]),
            (json!({"hasAttachment": true}), vec!["e2"]),
            (json!({"hasAttachment": false}), vec!["e1", "e3", "e4b"]),
            (
                json!({"operator": "AND", "conditions": [{"inMailbox": "mb-inbox"}, {"hasKeyword": "$seen"}]}),
                vec!["e2"],
            ),
            (
                json!({"operator": "NOT", "conditions": [{"inMailbox": "mb-inbox"}, {"inMailbox": "mb-trash"}]}),
                vec!["e3"],
            ),
            (
                json!({"inMailbox": "mb-inbox", "notKeyword": "$seen"}),
                vec!["e1", "e4b"],
            ),
        ] {
            let query = EmailQuery {
                filter: filter.clone(),
                ascending: false,
                limit: 500,
            };
            let (got, total) = store.query_ids(&query).unwrap();
            assert_eq!(got, ids, "{filter}");
            assert_eq!(total, ids.len(), "{filter}");
        }
    }

    #[test]
    fn a_query_outside_the_subset_is_unsupported() {
        let store = inbox(1, 1);
        for filter in [
            json!({"header": ["List-Id"]}),
            json!({"operator": "OR", "conditions": []}),
            json!({"inMailbox": 1}),
        ] {
            let query = EmailQuery {
                filter: filter.clone(),
                ascending: false,
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
