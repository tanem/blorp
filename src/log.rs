//! The per-session log file.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// The filter when `BLORP_LOG` is unset: warnings and errors.
pub const DEFAULT_FILTER: &str = "warn";

/// Truncates the log at `path` and installs a non-blocking subscriber that
/// writes to it, filtered by `filter` in `RUST_LOG` syntax or by
/// [`DEFAULT_FILTER`]. Lines are written until the guard is dropped.
pub fn init(path: &Path, filter: Option<&str>) -> io::Result<WorkerGuard> {
    let filter = EnvFilter::try_new(filter.unwrap_or(DEFAULT_FILTER))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("BLORP_LOG: {e}")))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = File::create(path)?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
        .map_err(io::Error::other)?;
    Ok(guard)
}
