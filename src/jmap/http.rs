//! The reqwest adapter for [`Jmap`].

use std::collections::BTreeMap;
use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{CORE, Email, EmailQuery, EmailUpdate, Error, Invocation, Jmap, Limits, MAIL};
use super::{Mailbox, QueryResult, Request, Response, Session, SetResult, Thread};

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

/// A JMAP client bound to one session's API URL, primary mail account, and
/// request limits.
#[derive(Clone)]
pub struct HttpJmap {
    http: reqwest::Client,
    token: Arc<SecretString>,
    api_url: String,
    account_id: String,
    limits: Limits,
}

impl HttpJmap {
    /// Fetches the session at `session_url`, checks that it advertises the
    /// mail capability, and binds a client to its primary mail account. The
    /// session URL and the API URL it names must be HTTPS, or plain HTTP to
    /// a loopback address, since the token is sent to both.
    pub async fn connect(
        session_url: &str,
        token: impl Into<Arc<SecretString>>,
    ) -> Result<(HttpJmap, Session), Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
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
        let account_id = session.mail_account().ok_or(Error::NoMailCapability)?;
        let jmap = HttpJmap {
            http,
            token,
            api_url: session.api_url.clone(),
            account_id: account_id.to_string(),
            limits: session.limits(),
        };
        Ok((jmap, session))
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
        let account = &self.account_id;
        let responses = self
            .send(vec![
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
            ])
            .await?;
        let ids: QueryResponse = response_of(&responses, "Email/query", "0")?;
        let hits: GetResponse<Hit> = response_of(&responses, "Email/get", "1")?;
        let threads: GetResponse<Thread> = response_of(&responses, "Thread/get", "2")?;
        let email_ids = member_ids(&ids.ids, &hits.list, &threads.list);
        let emails = self.get_emails(&email_ids).await?;
        Ok(QueryResult {
            total: ids.total.unwrap_or(ids.ids.len()),
            ids: ids.ids,
            threads: threads.list,
            emails,
        })
    }

    async fn get_emails(&self, ids: &[String]) -> Result<Vec<Email>, Error> {
        let mut emails = Vec::with_capacity(ids.len());
        let chunks: Vec<&[String]> = ids.chunks(self.limits.max_objects_in_get).collect();
        for batch in chunks.chunks(self.limits.max_calls_in_request) {
            let calls = batch
                .iter()
                .enumerate()
                .map(|(i, chunk)| {
                    let args = json!({
                        "accountId": self.account_id,
                        "ids": chunk,
                        "properties": EMAIL_PROPERTIES
                    });
                    call("Email/get", args, &i.to_string())
                })
                .collect();
            let responses = self.send(calls).await?;
            for i in 0..batch.len() {
                let got: GetResponse<Email> = response_of(&responses, "Email/get", &i.to_string())?;
                emails.extend(got.list);
            }
        }
        Ok(emails)
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
        let mut not_updated = Vec::new();
        for batch in chunks.chunks(self.limits.max_calls_in_request) {
            let calls = batch
                .iter()
                .enumerate()
                .map(|(i, chunk)| {
                    let args = json!({"accountId": self.account_id, "update": chunk});
                    call("Email/set", args, &i.to_string())
                })
                .collect();
            let responses = self.send(calls).await?;
            for i in 0..batch.len() {
                let set: SetResponse = response_of(&responses, "Email/set", &i.to_string())?;
                not_updated.extend(set.not_updated.unwrap_or_default().into_keys());
            }
        }
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
