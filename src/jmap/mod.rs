//! JMAP transport and wire types.
//!
//! [`Jmap`] is the seam: one method per kind of call blorp makes, with the
//! reqwest adapter [`HttpJmap`] for production and the in-memory
//! [`fake::FakeJmap`] for tests.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;

use futures::stream::BoxStream;
use serde_json::Value;

pub mod fake;
pub mod http;
pub mod push;
pub mod wire;

pub use http::HttpJmap;
pub use push::Push;
pub use wire::*;

/// The JMAP calls blorp makes, at the method level.
pub trait Jmap: Clone + Send + Sync + 'static {
    /// `Mailbox/get` for every mailbox in the account.
    fn get_mailboxes(&self) -> impl Future<Output = Result<Vec<Mailbox>, Error>> + Send;

    /// The threads matching `query` and every email of them, in two stages
    /// that are not atomic. First, one request chains `Email/query` with
    /// `collapseThreads`, `Email/get` for the hits' threads, and `Thread/get`
    /// by back-reference, then a `Thread/get` of the query's extra threads
    /// when it names any. Then the threads' emails are fetched with
    /// `Email/get` calls of at most the session's `maxObjectsInGet` ids, in
    /// requests of at most its `maxCallsInRequest` calls, sent one after
    /// another; none is sent for an empty queue. An email gone
    /// by the second stage is left out. A method error in any request fails
    /// the whole query.
    fn query(&self, query: &EmailQuery) -> impl Future<Output = Result<QueryResult, Error>> + Send;

    /// A reconcile's fetch: [`Jmap::query`] with `Mailbox/get` for every
    /// mailbox added to its first request, ahead of the query's calls.
    fn reconcile(
        &self,
        query: &EmailQuery,
    ) -> impl Future<Output = Result<Reconciled, Error>> + Send;

    /// Opens the push stream for `Email` and `Mailbox` changes, with no
    /// `closeafter` and a ping every [`push::PING_SECS`] seconds, or gives
    /// `None` when the session advertises no push stream. The stream ends,
    /// or ends in an error, when the connection drops.
    fn events(&self) -> impl Future<Output = Result<Option<PushStream>, Error>> + Send;

    /// The emails with `ids`, in `Email/get` calls of at most the session's
    /// `maxObjectsInGet` ids, in requests of at most its
    /// `maxCallsInRequest` calls, sent one after another. Ids not found are
    /// left out.
    fn get_emails(&self, ids: &[String]) -> impl Future<Output = Result<Vec<Email>, Error>> + Send;

    /// A top-level mailbox named `name` with no role, created with
    /// `Mailbox/set`. A refusal is a [`Error::Method`] with the `SetError`
    /// type as its kind.
    fn create_mailbox(&self, name: &str) -> impl Future<Output = Result<Mailbox, Error>> + Send;

    /// `update` applied in `Email/set` calls of at most the session's
    /// `maxObjectsInSet` emails, in requests of at most its
    /// `maxCallsInRequest` calls, sent one after another; none is sent for
    /// an empty update. An email the server did not update is listed in the
    /// result, not reported as an error. A failed request fails the whole
    /// update, and the calls answered before it stay applied.
    fn set_emails(
        &self,
        update: &EmailUpdate,
    ) -> impl Future<Output = Result<SetResult, Error>> + Send;
}

/// An `Email/set` patch: paths such as `mailboxIds/<id>` or
/// `keywords/$seen`, each set to `true` or `null`. Applying one twice
/// leaves the email as applying it once does.
pub type EmailPatch = BTreeMap<String, Value>;

/// The `update` argument of an `Email/set`: a patch per email id.
pub type EmailUpdate = BTreeMap<String, EmailPatch>;

/// The result of a [`Jmap::set_emails`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetResult {
    /// The ids in `notUpdated`, in id order.
    pub not_updated: Vec<String>,
}

/// An `Email/query` sorted by `receivedAt`, with threads collapsed.
#[derive(Debug, Clone, PartialEq)]
pub struct EmailQuery {
    /// The `filter` argument, as `Email/query` JSON.
    pub filter: Value,
    /// Oldest first when true, newest first when false.
    pub ascending: bool,
    /// The most threads to return.
    pub limit: usize,
    /// Threads fetched by id beside the query's, whatever they match, with
    /// every email of them.
    pub extra_threads: Vec<String>,
}

/// The result of a [`Jmap::query`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryResult {
    /// The ids `Email/query` returned, one email per thread, in query order.
    pub ids: Vec<String>,
    /// How many threads match, including any beyond the limit.
    pub total: usize,
    /// The threads of those emails.
    pub threads: Vec<Thread>,
    /// The threads of [`EmailQuery::extra_threads`] that exist.
    pub extra_threads: Vec<Thread>,
    /// Every email of those threads.
    pub emails: Vec<Email>,
}

/// The result of a [`Jmap::reconcile`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reconciled {
    /// Every mailbox in the account.
    pub mailboxes: Vec<Mailbox>,
    /// The query's result.
    pub result: QueryResult,
}

/// An open push stream's events.
pub type PushStream = BoxStream<'static, Result<Push, Error>>;

/// A failed JMAP request. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The request did not complete: DNS, connect, reset, or timeout.
    Transport(String),
    /// Fastmail answered with a non-success HTTP status.
    Status(u16),
    /// The session does not advertise the mail capability, or names no
    /// primary mail account.
    NoMailCapability,
    /// A method call returned a JMAP method error.
    Method { method: String, kind: String },
    /// The response did not parse.
    Parse(String),
    /// A URL the token would be sent to is not HTTPS, and not loopback.
    InsecureUrl(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(message) => write!(f, "{message}"),
            Error::Status(status) => write!(f, "Fastmail returned HTTP {status}"),
            Error::NoMailCapability => write!(
                f,
                "the Fastmail session has no mail access; the API token needs the Email scope"
            ),
            Error::Method { method, kind } => write!(f, "Fastmail error: {kind} from {method}"),
            Error::Parse(message) => write!(f, "could not read Fastmail's response: {message}"),
            Error::InsecureUrl(url) => {
                write!(
                    f,
                    "refusing to send the API token to {url}, which is not HTTPS"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Why blorp must stop, if this failure means Fastmail refuses the
    /// token: no retry with the same token can succeed.
    pub fn fatal(&self) -> Option<Fatal> {
        match self {
            Error::Status(401) => Some(Fatal::TokenRejected),
            Error::Status(403) | Error::NoMailCapability => Some(Fatal::NoEmailScope),
            _ => None,
        }
    }
}

/// A failure that ends blorp with a fixed message and exit status 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fatal {
    /// HTTP 401: the token is wrong or was removed.
    TokenRejected,
    /// HTTP 403, or a session without the mail capability.
    NoEmailScope,
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fatal::TokenRejected => write!(
                f,
                "Fastmail rejected the API token. It may have been removed in Settings. \
                 Run `blorp setup` to store a new one."
            ),
            Fatal::NoEmailScope => write!(
                f,
                "The API token does not give access to mail. It needs the Email scope. \
                 Run `blorp setup` to store one that has it."
            ),
        }
    }
}
