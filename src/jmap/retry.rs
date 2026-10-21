//! The retry schedule for transient failures: three retries, 1, 2, and
//! 4 s apart plus jitter, or as long as `Retry-After` asks, up to 60 s.

use std::future::Future;
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use tokio::time::Instant;

use super::{Class, Error};

/// How many times a transient failure is retried.
pub const RETRIES: u32 = 3;

/// The wait before each retry, apart from jitter.
pub const DELAYS: [Duration; RETRIES as usize] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// The most jitter added to a wait.
pub const MAX_JITTER: Duration = Duration::from_millis(500);

/// The longest `Retry-After` honoured; a longer one waits this long.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// A random wait of up to [`MAX_JITTER`].
pub fn jitter() -> Duration {
    let random = RandomState::new().hash_one(Instant::now());
    MAX_JITTER.mul_f64(random as f64 / u64::MAX as f64)
}

/// The wait before retry `retry`, counted from 1, after `error`: what its
/// `Retry-After` asks, up to [`MAX_RETRY_AFTER`], or else that retry's
/// delay plus `jitter`.
pub fn wait(retry: u32, error: &Error, jitter: Duration) -> Duration {
    match error.retry_after() {
        Some(wait) => wait.min(MAX_RETRY_AFTER),
        None => DELAYS[retry as usize - 1] + jitter,
    }
}

/// Runs `call`, and again after each transient failure up to [`RETRIES`]
/// times, calling `retried` with the retry's number before waiting for
/// it. Gives the first result that is not a transient failure, or the
/// last failure.
pub async fn retrying<T, F, Fut>(mut call: F, mut retried: impl FnMut(u32)) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let mut retry = 0;
    loop {
        match call().await {
            Err(error) if error.class() == Class::Transient && retry < RETRIES => {
                retry += 1;
                retried(retry);
                tokio::time::sleep(wait(retry, &error, jitter())).await;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_at_most_half_a_second() {
        for _ in 0..100 {
            assert!(jitter() <= MAX_JITTER);
        }
    }
}
