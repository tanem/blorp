//! The reqwest adapter for [`Jmap`].

use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{CORE, Email, EmailQuery, Error, Invocation, Jmap, MAIL, Mailbox, QueryResult};
use super::{Request, Response, Session, Thread};

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

/// A JMAP client bound to one session's API URL and primary mail account.
#[derive(Clone)]
pub struct HttpJmap {
    http: reqwest::Client,
    token: Arc<SecretString>,
    api_url: String,
    account_id: String,
}

impl HttpJmap {
    /// Fetches the session at `session_url`, checks that it advertises the
    /// mail capability, and binds a client to its primary mail account.
    pub async fn connect(
        session_url: &str,
        token: SecretString,
    ) -> Result<(HttpJmap, Session), Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(transport)?;
        let token = Arc::new(token);
        let response = authorize(http.get(session_url), &token)
            .send()
            .await
            .map_err(transport)?;
        let session: Session = read_json(response).await?;
        let account_id = session.mail_account().ok_or(Error::NoMailCapability)?;
        let jmap = HttpJmap {
            http,
            token,
            api_url: session.api_url.clone(),
            account_id: account_id.to_string(),
        };
        Ok((jmap, session))
    }

    async fn send(&self, method_calls: Vec<Invocation>) -> Result<Vec<Invocation>, Error> {
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
        let got: GetResponse<Mailbox> = take(&responses, "Mailbox/get", "0")?;
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
                        "collapseThreads": true
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
                call(
                    "Email/get",
                    json!({
                        "accountId": account,
                        "#ids": {"resultOf": "2", "name": "Thread/get", "path": "/list/*/emailIds"},
                        "properties": EMAIL_PROPERTIES
                    }),
                    "3",
                ),
            ])
            .await?;
        let ids: QueryResponse = take(&responses, "Email/query", "0")?;
        let threads: GetResponse<Thread> = take(&responses, "Thread/get", "2")?;
        let emails: GetResponse<Email> = take(&responses, "Email/get", "3")?;
        Ok(QueryResult {
            ids: ids.ids,
            threads: threads.list,
            emails: emails.list,
        })
    }
}

#[derive(Deserialize)]
struct GetResponse<T> {
    list: Vec<T>,
}

#[derive(Deserialize)]
struct QueryResponse {
    ids: Vec<String>,
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

/// Finds the response to call `call_id` and parses its arguments, turning a
/// method error into [`Error::Method`].
fn take<T: DeserializeOwned>(
    responses: &[Invocation],
    method: &str,
    call_id: &str,
) -> Result<T, Error> {
    let response = responses
        .iter()
        .find(|r| r.call_id == call_id)
        .ok_or_else(|| Error::Parse(format!("no response to {method}")))?;
    if response.name == "error" {
        let kind = response.args["type"].as_str().unwrap_or("unknown");
        return Err(Error::Method {
            method: method.into(),
            kind: kind.into(),
        });
    }
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
