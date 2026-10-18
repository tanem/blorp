//! JMAP transport and wire types.
//!
//! [`Jmap`] is the seam: one method per kind of call blorp makes, with the
//! reqwest adapter [`HttpJmap`] for production and the in-memory
//! [`fake::FakeJmap`] for tests.

use std::fmt;
use std::future::Future;

use serde_json::Value;

pub mod fake;
pub mod http;
pub mod wire;

pub use http::HttpJmap;
pub use wire::*;

/// The JMAP calls blorp makes, at the method level.
pub trait Jmap: Clone + Send + Sync + 'static {
    /// `Mailbox/get` for every mailbox in the account.
    fn get_mailboxes(&self) -> impl Future<Output = Result<Vec<Mailbox>, Error>> + Send;

    /// The threads matching `query` and every email of them, in two stages
    /// that are not atomic. First, one request chains `Email/query` with
    /// `collapseThreads`, `Email/get` for the hits' threads, and `Thread/get`
    /// by back-reference. Then the threads' emails are fetched with
    /// `Email/get` calls of at most the session's `maxObjectsInGet` ids, in
    /// requests of at most its `maxCallsInRequest` calls, sent one after
    /// another; none is sent for an empty queue. An email gone
    /// by the second stage is left out. A method error in any request fails
    /// the whole query.
    fn query(&self, query: &EmailQuery) -> impl Future<Output = Result<QueryResult, Error>> + Send;
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
