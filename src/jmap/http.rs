//! The reqwest adapter for [`Jmap`].

use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{CORE, Class, Email, EmailQuery, Error, Invocation, Jmap, Limits, MAIL};
use super::{Mailbox, PushStream, QueryResult, Reconciled, Request, Response, Session};
use super::{Thread, push};

/// Fastmail's session resource.
pub const FASTMAIL_SESSION_URL: &str = "https://api.fastmail.com/jmap/session";

/// The prefix of a JMAP request-level error's problem type.
const PROBLEM_PREFIX: &str = "urn:ietf:params:jmap:error:";

/// How long a request may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// To open the connection.
    pub connect: Duration,
    /// For the whole request, from sending it to reading the last byte of
    /// the response. The push stream has none.
    pub request: Duration,
}

impl Default for Timeouts {
    /// 10 s to connect and 30 s for each request.
    fn default() -> Self {
        Timeouts {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(30),
        }
    }
}

const MAILBOX_PROPERTIES: [&str; 3] = ["id", "name", "role"];
const EMAIL_PROPERTIES: [&str; 7] = [
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "from",
    "subject",
    "receivedAt",
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
    /// each. Requests have the default [`Timeouts`].
    pub async fn connect(
        session_url: &str,
        token: impl Into<Arc<SecretString>>,
    ) -> Result<(HttpJmap, Session), Error> {
        HttpJmap::connect_with(session_url, token, Timeouts::default()).await
    }

    /// [`HttpJmap::connect`] with `timeouts` for every request.
    pub async fn connect_with(
        session_url: &str,
        token: impl Into<Arc<SecretString>>,
        timeouts: Timeouts,
    ) -> Result<(HttpJmap, Session), Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(timeouts.connect)
            .timeout(timeouts.request)
            .build()
            .map_err(transport)?;
        let events_http = reqwest::Client::builder()
            .connect_timeout(timeouts.connect)
            .build()
            .map_err(transport)?;
        let token = token.into();
        require_secure(session_url)?;
        let response = authorize(http.get(session_url), &token)
            .send()
            .await
            .map_err(transport)?;
        let session: Session = parse(&read_body(response).await?)?;
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
    /// consequence, and logged with the response when it is a bug.
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
        let body = read_body(response).await?;
        let response: Response = parse(&body)?;
        if let Some(error) = response.method_responses.iter().find(|r| r.name == "error") {
            let method = methods
                .iter()
                .find(|(call_id, _)| *call_id == error.call_id)
                .map_or("unknown method", |(_, name)| name.as_str());
            let error = Error::Method {
                method: method.into(),
                kind: error.args["type"].as_str().unwrap_or("unknown").into(),
            };
            if error.class() == Class::Bug {
                log_bug(&format!("{method} failed"), &body);
            }
            return Err(error);
        }
        Ok(response.method_responses)
    }

    /// The staged fetch [`Jmap::query`] describes, with `Mailbox/get` for
    /// every mailbox ahead of the query's calls in the first request when
    /// `mailboxes` is true. The mailbox list is empty when `mailboxes` is
    /// false.
    async fn fetch(
        &self,
        query: &EmailQuery,
        mailboxes: bool,
    ) -> Result<(Vec<Mailbox>, QueryResult), Error> {
        let account = &self.account_id;
        let mut calls = Vec::new();
        if mailboxes {
            calls.push(call(
                "Mailbox/get",
                json!({"accountId": account, "ids": null, "properties": MAILBOX_PROPERTIES}),
                "m",
            ));
        }
        calls.extend([
            call(
                "Email/query",
                json!({
                    "accountId": account,
                    "filter": query.filter,
                    "sort": [{"property": "receivedAt", "isAscending": false}],
                    "collapseThreads": true,
                    "limit": query.limit,
                    "calculateTotal": true
                }),
                "0",
            ),
            call(
                "Email/get",
                json!({
                    "accountId": account,
                    "#ids": {"resultOf": "0", "name": "Email/query", "path": "/ids"},
                    "properties": ["threadId"]
                }),
                "1",
            ),
            call(
                "Thread/get",
                json!({
                    "accountId": account,
                    "#ids": {"resultOf": "1", "name": "Email/get", "path": "/list/*/threadId"}
                }),
                "2",
            ),
        ]);
        let responses = self.send(calls).await?;
        let mailboxes = if mailboxes {
            let got: GetResponse<Mailbox> = response_of(&responses, "Mailbox/get", "m")?;
            got.list
        } else {
            Vec::new()
        };
        let ids: QueryResponse = response_of(&responses, "Email/query", "0")?;
        let hits: GetResponse<Hit> = response_of(&responses, "Email/get", "1")?;
        let threads: GetResponse<Thread> = response_of(&responses, "Thread/get", "2")?;
        let emails = self
            .get_emails(&member_ids(&ids.ids, &hits.list, &threads.list))
            .await?;
        let result = QueryResult {
            total: ids.total.unwrap_or(ids.ids.len()),
            ids: ids.ids,
            threads: threads.list,
            emails,
        };
        Ok((mailboxes, result))
    }

    /// The emails with `ids`, in `Email/get` calls of at most the session's
    /// `maxObjectsInGet` ids, in requests of at most its
    /// `maxCallsInRequest` calls, sent one after another. Ids not found are
    /// left out.
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
        let (_, result) = self.fetch(query, false).await?;
        Ok(result)
    }

    async fn reconcile(&self, query: &EmailQuery) -> Result<Reconciled, Error> {
        let (mailboxes, result) = self.fetch(query, true).await?;
        Ok(Reconciled { mailboxes, result })
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

/// Where the token is exposed to build the `Authorization` header. The
/// token stores also expose it, to write it.
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

/// A response's body, or the failure a non-success status reports: the
/// wait a `Retry-After` header asks for on HTTP 429, 5xx, or the
/// request-level `limit` error, else a request-level error when the body
/// is JMAP problem details. A bug is logged with the body.
async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_wait(value, jiff::Timestamp::now()));
    let body = response.bytes().await.map_err(transport)?.to_vec();
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let problem = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|problem| {
            let kind = problem["type"].as_str()?.strip_prefix(PROBLEM_PREFIX)?;
            Some(kind.to_string())
        });
    let busy = status == 429 || (500..600).contains(&status) || problem.as_deref() == Some("limit");
    let error = match (problem, retry_after) {
        _ if matches!(status, 401 | 403) => Error::Status(status),
        (_, Some(wait)) if busy => Error::RetryAfter { status, wait },
        (Some(kind), _) => Error::Request(kind),
        (None, _) => Error::Status(status),
    };
    if error.class() == Class::Bug {
        log_bug(&format!("HTTP {status}"), &body);
    }
    Err(error)
}

/// The wait a `Retry-After` value asks for at `now`: a number of seconds,
/// or an HTTP date, zero when that has passed.
fn retry_wait(value: &str, now: jiff::Timestamp) -> Option<Duration> {
    if let Ok(secs) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = jiff::fmt::rfc2822::parse(value.trim()).ok()?.timestamp();
    Some(Duration::try_from(at.duration_since(now)).unwrap_or(Duration::ZERO))
}

/// Parses a successful response's body, logging it when it does not
/// parse.
fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(body).map_err(|e| {
        log_bug("unreadable response", body);
        Error::Parse(e.to_string())
    })
}

/// Logs a response that is a bug at error level: its body, with no
/// headers.
fn log_bug(what: &str, body: &[u8]) {
    tracing::error!("Fastmail error, {what}: {}", String::from_utf8_lossy(body));
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
