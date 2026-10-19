//! The push stream: server-sent events from the session's `eventSourceUrl`
//! (RFC 8620 section 7.3), read as [`Push`] events.

use std::fmt::Display;

use eventsource_stream::{EventStreamError, Eventsource};
use futures::{Stream, StreamExt};

use super::Error;

/// The `types` blorp asks the push stream for.
pub const TYPES: &str = "Email,Mailbox";

/// The ping interval blorp asks for, in seconds. Fastmail clamps it to at
/// most 300.
pub const PING_SECS: u64 = 300;

/// An event on the push stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// A `state` event: something changed, or the stream has just
    /// connected, since the first event on a connection carries the
    /// current state strings.
    State,
    /// A `ping` event, sent when the stream has been quiet for the ping
    /// interval.
    Ping,
}

/// Reads the body of a push stream as its `state` and `ping` events. The
/// event data is not read, so Fastmail's extra `"type": "connect"` key does
/// not matter; comments and events of any other name are skipped. A
/// failure of the body ends in a [`Error::Transport`], and text that is not
/// UTF-8 or not an event stream in a [`Error::Parse`].
pub fn pushes<S, B, E>(body: S) -> impl Stream<Item = Result<Push, Error>>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Display,
{
    body.eventsource().filter_map(|event| async move {
        match event {
            Ok(event) => match event.event.as_str() {
                "state" => Some(Ok(Push::State)),
                "ping" => Some(Ok(Push::Ping)),
                _ => None,
            },
            Err(EventStreamError::Transport(error)) => {
                Some(Err(Error::Transport(error.to_string())))
            }
            Err(error) => Some(Err(Error::Parse(error.to_string()))),
        }
    })
}

/// The push stream URL for `event_source_url`: its `{types}`,
/// `{closeafter}`, and `{ping}` variables expanded when it is a template,
/// as RFC 8620 asks, or the three appended as query parameters when it is
/// a bare URL, as Fastmail's is.
pub fn event_source_url(event_source_url: &str) -> String {
    let closeafter = "no";
    let ping = PING_SECS.to_string();
    if event_source_url.contains("{types}") {
        return event_source_url
            .replace("{types}", TYPES)
            .replace("{closeafter}", closeafter)
            .replace("{ping}", &ping);
    }
    match url::Url::parse(event_source_url) {
        Ok(mut url) => {
            url.query_pairs_mut()
                .append_pair("types", TYPES)
                .append_pair("closeafter", closeafter)
                .append_pair("ping", &ping);
            url.to_string()
        }
        Err(_) => event_source_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use futures::stream;

    use super::*;

    /// The events of `chunks` read as one body.
    async fn read(chunks: &[&'static str]) -> Vec<Result<Push, Error>> {
        let body = stream::iter(chunks.iter().map(|c| Ok::<_, Infallible>(c.as_bytes())));
        pushes(body).collect().await
    }

    #[tokio::test]
    async fn a_fastmail_stream_reads_as_state_and_ping_events() {
        let events = read(&[
            ": new event source connection\r\n\r\n",
            "event: state\r\ndata: {\"@type\":\"StateChange\",\"changed\":{\"u1\":{\"Email\":\"s1\",\"Mailbox\":\"s2\"}},\"type\":\"connect\"}\r\n\r\n",
            "event: ping\r\ndata: {\"interval\":300}\r\n\r\n",
            "event: state\nid: abc\ndata: {\"@type\":\"StateChange\",\"changed\":{}}\n\n",
        ])
        .await;

        assert_eq!(events, [Ok(Push::State), Ok(Push::Ping), Ok(Push::State)]);
    }

    #[tokio::test]
    async fn an_event_split_across_chunks_reads_once_it_is_complete() {
        let events = read(&[
            "ev",
            "ent: sta",
            "te\ndata: {}\n",
            "\nevent: pi",
            "ng\ndata: {}\n\n",
        ])
        .await;

        assert_eq!(events, [Ok(Push::State), Ok(Push::Ping)]);
    }

    #[tokio::test]
    async fn events_of_other_names_and_an_unfinished_event_are_skipped() {
        let events = read(&[
            "data: {}\n\n",
            "event: other\ndata: {}\n\n",
            "event: state\ndata: {}\n",
        ])
        .await;

        assert_eq!(events, []);
    }

    #[tokio::test]
    async fn a_failed_body_ends_in_a_transport_error() {
        let body = stream::iter([
            Ok("event: state\ndata: {}\n\n".as_bytes()),
            Err("connection reset"),
        ]);

        let events: Vec<_> = pushes(body).collect().await;

        assert_eq!(
            events,
            [
                Ok(Push::State),
                Err(Error::Transport("connection reset".into()))
            ]
        );
    }

    #[test]
    fn a_bare_event_source_url_gets_the_parameters_as_a_query() {
        assert_eq!(
            event_source_url("https://api.fastmail.com/jmap/event/"),
            "https://api.fastmail.com/jmap/event/?types=Email%2CMailbox&closeafter=no&ping=300"
        );
    }

    #[test]
    fn a_templated_event_source_url_is_expanded() {
        assert_eq!(
            event_source_url(
                "https://jmap.example.invalid/events?types={types}&closeafter={closeafter}&ping={ping}"
            ),
            "https://jmap.example.invalid/events?types=Email,Mailbox&closeafter=no&ping=300"
        );
    }
}
