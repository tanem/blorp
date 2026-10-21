//! JMAP transport and wire types.
//!
//! [`Jmap`] is the seam: one method per kind of call blorp makes, with the
//! reqwest adapter [`HttpJmap`] for production and the in-memory
//! [`fake::FakeJmap`] for tests.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use futures::stream::BoxStream;
use serde_json::Value;

pub mod fake;
pub mod http;
pub mod push;
pub mod retry;
pub mod wire;

pub use http::{HttpJmap, Timeouts};
pub use push::Push;
pub use wire::*;

/// The JMAP calls blorp makes, at the method level.
pub trait Jmap: Clone + Send + Sync + 'static {
    /// `Mailbox/get` for every mailbox in the account.
    fn get_mailboxes(&self) -> impl Future<Output = Result<Vec<Mailbox>, Error>> + Send;

    /// The threads matching `query` and every email of them, in two stages
    /// that are not atomic. First, one request chains `Email/query` with
    /// `collapseThreads`, `Email/get` for the hits' threads, and `Thread/get`
    /// by back-reference. Then the threads' emails are fetched in
    /// `Email/get` calls of at most the session's `maxObjectsInGet` ids, in
    /// requests of at most its `maxCallsInRequest` calls, sent one after
    /// another; none is sent for an empty queue. An email gone by then is
    /// left out. A method error in any request fails the whole query.
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
}

/// An `Email/query` sorted by `receivedAt`, newest first, with threads
/// collapsed.
#[derive(Debug, Clone, PartialEq)]
pub struct EmailQuery {
    /// The `filter` argument, as `Email/query` JSON.
    pub filter: Value,
    /// The most threads to return.
    pub limit: usize,
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
    /// Fastmail answered HTTP 429, 5xx, or the request-level `limit`
    /// error with a `Retry-After` header asking for this wait.
    RetryAfter { status: u16, wait: Duration },
    /// The session does not advertise the mail capability, or names no
    /// primary mail account.
    NoMailCapability,
    /// Fastmail refused the request as a whole with this JMAP
    /// request-level error type, such as `limit` or `notJSON`.
    Request(String),
    /// A method call returned a JMAP method error.
    Method { method: String, kind: String },
    /// The response did not parse.
    Parse(String),
    /// A URL the token would be sent to is not HTTPS, and not loopback.
    InsecureUrl(String),
}

/// How a failed request is handled: retried, ending blorp, or reported as
/// a fault on one side or the other that a retry would not fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A network failure, an overloaded or rate-limiting server: retried.
    Transient,
    /// Fastmail refuses the token: blorp prints the message and exits 1.
    Fatal(Fatal),
    /// Anything else, never retried.
    Bug,
}

/// The JMAP method error types that are transient.
const TRANSIENT_KINDS: [&str; 2] = ["serverUnavailable", "serverFail"];

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(message) => write!(f, "{message}"),
            Error::Status(status) | Error::RetryAfter { status, .. }
                if self.class() == Class::Transient =>
            {
                write!(f, "Fastmail returned HTTP {status}")
            }
            Error::Status(status) | Error::RetryAfter { status, .. } => {
                write!(f, "Fastmail error: HTTP {status}")
            }
            Error::NoMailCapability => write!(
                f,
                "the Fastmail session has no mail access; the API token needs the Email scope"
            ),
            Error::Request(kind) | Error::Method { kind, .. } => {
                write!(f, "Fastmail error: {kind}")
            }
            Error::Parse(message) => write!(f, "Fastmail error: unreadable response: {message}"),
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

    /// The class the failure falls into: transient for a request that did
    /// not complete, HTTP 429 and 5xx, the request-level `limit` error, and
    /// the `serverUnavailable` and `serverFail` method errors; fatal
    /// as [`Error::fatal`] says; otherwise a bug.
    pub fn class(&self) -> Class {
        if let Some(fatal) = self.fatal() {
            return Class::Fatal(fatal);
        }
        let transient = match self {
            Error::Transport(_) | Error::RetryAfter { .. } => true,
            Error::Status(status) => *status == 429 || (500..600).contains(status),
            Error::Request(kind) => kind == "limit",
            Error::Method { kind, .. } => TRANSIENT_KINDS.contains(&kind.as_str()),
            _ => false,
        };
        if transient {
            Class::Transient
        } else {
            Class::Bug
        }
    }

    /// The wait Fastmail asked for before trying again, if it named one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RetryAfter { wait, .. } => Some(*wait),
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
