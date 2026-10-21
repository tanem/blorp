//! The `sync` task: keeps the push stream open and sends [`Msg::Wake`] on
//! each of its events. The reconcile tick is not here but in
//! `update` (ADR 0002).

use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;

use crate::app::Msg;
use crate::jmap::retry::jitter;
use crate::jmap::{Jmap, Push, push};

/// How long the stream may stay silent before it counts as dead: the ping
/// interval plus 30 s.
pub const WATCHDOG: Duration = Duration::from_secs(push::PING_SECS + 30);

/// The first wait before reconnecting.
pub const BACKOFF_START: Duration = Duration::from_secs(1);

/// The longest wait before reconnecting, apart from jitter.
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// How long a connection must stay open for the next wait to start again
/// from [`BACKOFF_START`].
pub const STABLE: Duration = BACKOFF_CAP;

/// The reconnect backoff: 1 s doubling to a 60 s cap, plus jitter, with no
/// limit on attempts.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            base: BACKOFF_START,
        }
    }
}

impl Backoff {
    /// The wait before reconnecting after a connection that stayed open
    /// for `lasted`, counted from when it opened, zero when it did not
    /// open: the current base plus `jitter`. A connection that stayed open [`STABLE`] or longer starts
    /// the base again from [`BACKOFF_START`]. The base then doubles, up to
    /// [`BACKOFF_CAP`].
    pub fn next(&mut self, lasted: Duration, jitter: Duration) -> Duration {
        if lasted >= STABLE {
            self.base = BACKOFF_START;
        }
        let wait = self.base + jitter;
        self.base = (self.base * 2).min(BACKOFF_CAP);
        wait
    }
}

/// Runs the push stream until `tx` closes or the session turns out to
/// have no push stream. Each event, `state` or `ping`, including the first
/// on every connection, is sent as [`Msg::Wake`]. A stream that fails to
/// open, ends, fails, or stays silent for [`WATCHDOG`] is opened again
/// after the [`Backoff`] wait.
pub async fn run<J: Jmap>(jmap: J, tx: UnboundedSender<Msg>) {
    let mut backoff = Backoff::default();
    loop {
        let mut stream = match jmap.events().await {
            Ok(Some(stream)) => stream,
            Ok(None) => return,
            Err(error) => {
                tracing::info!("push stream did not open: {error}");
                wait(&mut backoff, Duration::ZERO).await;
                continue;
            }
        };
        tracing::debug!("push stream open");
        let opened = Instant::now();
        loop {
            match tokio::time::timeout(WATCHDOG, stream.next()).await {
                Ok(Some(Ok(Push::State | Push::Ping))) => {
                    if tx.send(Msg::Wake).is_err() {
                        return;
                    }
                }
                Ok(Some(Err(error))) => {
                    tracing::info!("push stream failed: {error}");
                    break;
                }
                Ok(None) => {
                    tracing::info!("push stream ended");
                    break;
                }
                Err(_) => {
                    tracing::info!("push stream silent for {} s", WATCHDOG.as_secs());
                    break;
                }
            }
        }
        if tx.is_closed() {
            return;
        }
        wait(&mut backoff, opened.elapsed()).await;
    }
}

async fn wait(backoff: &mut Backoff, lasted: Duration) {
    tokio::time::sleep(backoff.next(lasted, jitter())).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn the_wait_doubles_from_one_second_to_a_one_minute_cap() {
        let mut backoff = Backoff::default();

        let waits: Vec<u64> = (0..9)
            .map(|_| backoff.next(Duration::ZERO, Duration::ZERO).as_secs())
            .collect();

        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn jitter_is_added_to_the_wait() {
        let mut backoff = Backoff::default();

        assert_eq!(
            backoff.next(Duration::ZERO, Duration::from_millis(300)),
            Duration::from_millis(1300)
        );
        assert_eq!(
            backoff.next(Duration::ZERO, Duration::from_millis(20)),
            Duration::from_millis(2020)
        );
    }

    #[test]
    fn a_connection_open_a_minute_starts_the_wait_again() {
        let mut backoff = Backoff::default();
        for _ in 0..5 {
            backoff.next(secs(5), Duration::ZERO);
        }

        assert_eq!(backoff.next(secs(59), Duration::ZERO), secs(32));
        assert_eq!(backoff.next(secs(60), Duration::ZERO), secs(1));
        assert_eq!(backoff.next(secs(0), Duration::ZERO), secs(2));
    }
}
