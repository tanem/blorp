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

    /// `Email/query` with `collapseThreads`, then `Thread/get` and `Email/get`
    /// chained by back-reference, as one request.
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
        }
    }
}

impl std::error::Error for Error {}
