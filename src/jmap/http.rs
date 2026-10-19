//! The reqwest adapter for [`Jmap`].

use std::collections::BTreeMap;
use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{CORE, Email, EmailQuery, EmailUpdate, Error, Invocation, Jmap, Limits, MAIL};
use super::{Mailbox, PushStream, QueryResult, Reconciled, Request, Response, Session};
use super::{SetResult, Thread, push};

/// Fastmail's session resource.
pub const FASTMAIL_SESSION_URL: &str = "https://api.fastmail.com/jmap/session";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MAILBOX_PROPERTIES: [&str; 4] = ["id", "name", "parentId", "role"];
const EMAIL_PROPERTIES: [&str; 8] = [
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "from",
    "subject",
    "receivedAt",
    "hasAttachment",
];

/// A JMAP client bound to one session's API URL, push stream URL, primary
/// mail account, and request limits.
#[derive(Clone)]
pub struct HttpJmap {
    http: reqwest::Client,
    /// The client for the push stream, which has no request timeout, since
    /// the stream stays open.
    events_http: reqwest::Client,
    token: Arc<SecretString>,
    api_url: String,
    event_source_url: Option<String>,
    account_id: String,
    limits: Limits,
}

impl HttpJmap {
    /// Fetches the session at `session_url`, checks that it advertises the
    /// mail capability, and binds a client to its primary mail account. The
    /// session URL and the API and push stream URLs it names must be HTTPS,
    /// or plain HTTP to a loopback address, since the token is sent to
    /// each.
    pub async fn connect(
        session_url: &str,
        token: impl Into<Arc<SecretString>>,
    ) -> Result<(HttpJmap, Session), Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(transport)?;
        let events_http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(transport)?;
        let token = token.into();
        require_secure(session_url)?;
        let response = authorize(http.get(session_url), &token)
            .send()
            .await
            .map_err(transport)?;
        let session: Session = read_json(response).await?;
        require_secure(&session.api_url)?;
        if let Some(url) = &session.event_source_url {
            require_secure(url)?;
        }
        let account_id = session.mail_account().ok_or(Error::NoMailCapability)?;
        let jmap = HttpJmap {
            http,
            events_http,
            token,
            api_url: session.api_url.clone(),
            event_source_url: session.event_source_url.clone(),
            account_id: account_id.to_string(),
            limits: session.limits(),
        };
        Ok((jmap, session))
    }

    /// Sends one `method` call per chunk, with the arguments `args` gives
    /// it, in requests of at most the session's `maxCallsInRequest` calls
    /// sent one after another, and returns the responses in chunk order.
    /// Nothing is sent for no chunks.
    async fn send_chunked<C: Sync, T: DeserializeOwned>(
        &self,
        method: &str,
        chunks: &[C],
        args: impl Fn(&C) -> Value + Sync,
    ) -> Result<Vec<T>, Error> {
        let mut answers = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(self.limits.max_calls_in_request) {
            let calls = batch
                .iter()
                .enumerate()
                .map(|(i, chunk)| call(method, args(chunk), &i.to_string()))
                .collect();
            let responses = self.send(calls).await?;
            for i in 0..batch.len() {
                answers.push(response_of(&responses, method, &i.to_string())?);
            }
        }
        Ok(answers)
    }

    /// Sends one request. A method error is reported for the first call
    /// that failed, since later calls that refer back to it fail only as a
    /// consequence.
    async fn send(&self, method_calls: Vec<Invocation>) -> Result<Vec<Invocation>, Error> {
        let methods: Vec<(String, String)> = method_calls
            .iter()
            .map(|call| (call.call_id.clone(), call.name.clone()))
            .collect();
        let request = Request {
            using: vec![CORE.into(), MAIL.into()],
            method_calls,
        };
        let response = authorize(self.http.post(&self.api_url), &self.token)
            .json(&request)
            .send()
            .await
            .map_err(transport)?;
        let response: Response = read_json(response).await?;
        if let Some(error) = response.method_responses.iter().find(|r| r.name == "error") {
            let method = methods
                .iter()
                .find(|(call_id, _)| *call_id == error.call_id)
                .map_or("unknown method", |(_, name)| name.as_str());
            return Err(Error::Method {
                method: method.into(),
                kind: error.args["type"].as_str().unwrap_or("unknown").into(),
            });
        }
        Ok(response.method_responses)
    }

    /// The staged fetch [`Jmap::query`] describes for each of `queries`,
    /// with `Mailbox/get` for every mailbox ahead of the queries' calls in
    /// the first request when `mailboxes` is true. Each query's first-stage
    /// chain goes into that one request, its call ids prefixed with the
    /// query's prefix, and the later stages follow for each query in turn.
    /// The mailbox list is empty when `mailboxes` is false.
    async fn fetch(
        &self,
        queries: &[(&str, &EmailQuery)],
        mailboxes: bool,
    ) -> Result<(Vec<Mailbox>, Vec<QueryResult>), Error> {
        let account = &self.account_id;
        let mut calls = Vec::new();
        if mailboxes {
            calls.push(call(
                "Mailbox/get",
                json!({"accountId": account, "ids": null, "properties": MAILBOX_PROPERTIES}),
                "m",
            ));
        }
        for (prefix, query) in queries {
            calls.extend([
                call(
                    "Email/query",
                    json!({
                        "accountId": account,
                        "filter": query.filter,
                        "sort": [{"property": "receivedAt", "isAscending": query.ascending}],
                        "collapseThreads": true,
                        "limit": query.limit,
                        "calculateTotal": true
                    }),
                    &format!("{prefix}0"),
                ),
                call(
                    "Email/get",
                    json!({
                        "accountId": account,
                        "#ids": {"resultOf": format!("{prefix}0"), "name": "Email/query", "path": "/ids"},
                        "properties": ["threadId"]
                    }),
                    &format!("{prefix}1"),
                ),
                call(
                    "Thread/get",
                    json!({
                        "accountId": account,
                        "#ids": {"resultOf": format!("{prefix}1"), "name": "Email/get", "path": "/list/*/threadId"}
                    }),
                    &format!("{prefix}2"),
                ),
            ]);
        }
        let responses = self.send(calls).await?;
        let mailboxes = if mailboxes {
            let got: GetResponse<Mailbox> = response_of(&responses, "Mailbox/get", "m")?;
            got.list
        } else {
            Vec::new()
        };
        let mut results = Vec::with_capacity(queries.len());
        for (prefix, query) in queries {
            let ids: QueryResponse = response_of(&responses, "Email/query", &format!("{prefix}0"))?;
            let hits: GetResponse<Hit> =
                response_of(&responses, "Email/get", &format!("{prefix}1"))?;
            let threads: GetResponse<Thread> =
                response_of(&responses, "Thread/get", &format!("{prefix}2"))?;
            results.push(
                self.fetch_members(query, ids, hits.list, threads.list)
                    .await?,
            );
        }
        Ok((mailboxes, results))
    }

    /// The later stages of a query's fetch: its extra threads, then every
    /// email of its threads.
    async fn fetch_members(
        &self,
        query: &EmailQuery,
        ids: QueryResponse,
        hits: Vec<Hit>,
        threads: Vec<Thread>,
    ) -> Result<QueryResult, Error> {
        let account = &self.account_id;
        let chunks: Vec<&[String]> = query
            .extra_threads
            .chunks(self.limits.max_objects_in_get)
            .collect();
        let extra: Vec<GetResponse<Thread>> = self
            .send_chunked(
                "Thread/get",
                &chunks,
                |chunk| json!({"accountId": account, "ids": chunk}),
            )
            .await?;
        let extra_threads: Vec<Thread> = extra.into_iter().flat_map(|got| got.list).collect();
        let mut email_ids = member_ids(&ids.ids, &hits, &threads);
        for thread in &extra_threads {
            if !threads.iter().any(|t| t.id == thread.id) {
                email_ids.extend(thread.email_ids.iter().cloned());
            }
        }
        let emails = self.get_emails(&email_ids).await?;
        Ok(QueryResult {
            total: ids.total.unwrap_or(ids.ids.len()),
            ids: ids.ids,
            threads,
            extra_threads,
            emails,
        })
    }
}

impl Jmap for HttpJmap {
    async fn get_mailboxes(&self) -> Result<Vec<Mailbox>, Error> {
        let responses = self
            .send(vec![call(
                "Mailbox/get",
                json!({"accountId": self.account_id, "ids": null, "properties": MAILBOX_PROPERTIES}),
                "0",
            )])
            .await?;
        let got: GetResponse<Mailbox> = response_of(&responses, "Mailbox/get", "0")?;
        Ok(got.list)
    }

    async fn query(&self, query: &EmailQuery) -> Result<QueryResult, Error> {
        let (_, results) = self.fetch(&[("", query)], false).await?;
        Ok(results.into_iter().next().expect("the query's result"))
    }

    async fn reconcile(
        &self,
        query: &EmailQuery,
        snoozed: Option<&EmailQuery>,
    ) -> Result<Reconciled, Error> {
        let mut queries = vec![("", query)];
        queries.extend(snoozed.map(|snoozed| ("s", snoozed)));
        let (mailboxes, results) = self.fetch(&queries, true).await?;
        let mut results = results.into_iter();
        Ok(Reconciled {
            mailboxes,
            result: results.next().expect("the queue's result"),
            snoozed: results.next(),
        })
    }

    async fn events(&self) -> Result<Option<PushStream>, Error> {
        let Some(url) = &self.event_source_url else {
            return Ok(None);
        };
        let request = self
            .events_http
            .get(push::event_source_url(url))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache");
        let response = authorize(request, &self.token)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        Ok(Some(push::pushes(response.bytes_stream()).boxed()))
    }

    async fn get_emails(&self, ids: &[String]) -> Result<Vec<Email>, Error> {
        let chunks: Vec<&[String]> = ids.chunks(self.limits.max_objects_in_get).collect();
        let responses: Vec<GetResponse<Email>> = self
            .send_chunked("Email/get", &chunks, |chunk| {
                json!({
                    "accountId": self.account_id,
                    "ids": chunk,
                    "properties": EMAIL_PROPERTIES
                })
            })
            .await?;
        Ok(responses.into_iter().flat_map(|got| got.list).collect())
    }

    async fn create_mailbox(&self, name: &str) -> Result<Mailbox, Error> {
        let responses = self
            .send(vec![call(
                "Mailbox/set",
                json!({
                    "accountId": self.account_id,
                    "create": {"new": {"name": name, "parentId": null}}
                }),
                "0",
            )])
            .await?;
        let set: MailboxSetResponse = response_of(&responses, "Mailbox/set", "0")?;
        if let Some(created) = set.created.as_ref().and_then(|c| c.get("new")) {
            return Ok(Mailbox {
                id: created.id.clone(),
                name: name.into(),
                parent_id: None,
                role: None,
            });
        }
        let kind = set
            .not_created
            .as_ref()
            .and_then(|n| n.get("new"))
            .and_then(|e| e["type"].as_str())
            .ok_or_else(|| Error::Parse("Mailbox/set neither created nor refused it".into()))?;
        Err(Error::Method {
            method: "Mailbox/set".into(),
            kind: kind.into(),
        })
    }

    async fn set_emails(&self, update: &EmailUpdate) -> Result<SetResult, Error> {
        let entries: Vec<_> = update.iter().collect();
        let chunks: Vec<EmailUpdate> = entries
            .chunks(self.limits.max_objects_in_set)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|(id, patch)| ((*id).clone(), (*patch).clone()))
                    .collect()
            })
            .collect();
        let responses: Vec<SetResponse> = self
            .send_chunked(
                "Email/set",
                &chunks,
                |chunk| json!({"accountId": self.account_id, "update": chunk}),
            )
            .await?;
        let not_updated = responses
            .into_iter()
            .flat_map(|set| set.not_updated.unwrap_or_default().into_keys())
            .collect();
        Ok(SetResult { not_updated })
    }
}

/// The ids of every email of `threads`, thread by thread in the order of
/// the query's `ids`, since a `/get` may list its objects in any order. A
/// thread no hit names comes last.
fn member_ids(ids: &[String], hits: &[Hit], threads: &[Thread]) -> Vec<String> {
    let position = |thread: &Thread| {
        hits.iter()
            .find(|hit| hit.thread_id == thread.id)
            .and_then(|hit| ids.iter().position(|id| *id == hit.id))
            .unwrap_or(usize::MAX)
    };
    let mut ordered: Vec<&Thread> = threads.iter().collect();
    ordered.sort_by_cached_key(|thread| position(thread));
    ordered
        .into_iter()
        .flat_map(|thread| thread.email_ids.iter().cloned())
        .collect()
}

/// A query hit, fetched for its thread.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    id: String,
    thread_id: String,
}

#[derive(Deserialize)]
struct GetResponse<T> {
    list: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetResponse {
    not_updated: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailboxSetResponse {
    created: Option<BTreeMap<String, Created>>,
    not_created: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

#[derive(Deserialize)]
struct QueryResponse {
    ids: Vec<String>,
    total: Option<usize>,
}

/// Accepts HTTPS, and plain HTTP only to a loopback address such as a test
/// server on localhost.
fn require_secure(url: &str) -> Result<(), Error> {
    let insecure = || Error::InsecureUrl(url.to_string());
    let parsed = reqwest::Url::parse(url).map_err(|_| insecure())?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(insecure()),
    }
}

/// The only place the token is exposed.
fn authorize(request: reqwest::RequestBuilder, token: &SecretString) -> reqwest::RequestBuilder {
    request.bearer_auth(token.expose_secret())
}

fn call(name: &str, args: Value, call_id: &str) -> Invocation {
    Invocation {
        name: name.into(),
        args,
        call_id: call_id.into(),
    }
}

/// Finds the response to call `call_id` and parses its arguments.
fn response_of<T: DeserializeOwned>(
    responses: &[Invocation],
    method: &str,
    call_id: &str,
) -> Result<T, Error> {
    let response = responses
        .iter()
        .find(|r| r.call_id == call_id)
        .ok_or_else(|| Error::Parse(format!("no response to {method}")))?;
    serde_json::from_value(response.args.clone()).map_err(|e| Error::Parse(e.to_string()))
}

async fn read_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16()));
    }
    let body = response.bytes().await.map_err(transport)?;
    serde_json::from_slice(&body).map_err(|e| Error::Parse(e.to_string()))
}

fn transport(error: reqwest::Error) -> Error {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    Error::Transport(message)
}
