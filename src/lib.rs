//! blorp triages a Fastmail account's mailboxes over JMAP.

use std::env;
use std::process::ExitCode;

use crossterm::event::EventStream;
use jiff::Timestamp;
use secrecy::SecretString;
use tracing_appender::non_blocking::WorkerGuard;

pub mod app;
pub mod cli;
pub mod config;
pub mod harness;
pub mod jmap;
pub mod log;
pub mod mail;
pub mod queue;
pub mod run;
pub mod ui;

use cli::{Cli, Command};
use jmap::HttpJmap;
use jmap::http::FASTMAIL_SESSION_URL;

/// Runs blorp for parsed arguments and returns the process exit code.
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        None => tui(),
        Some(Command::Setup) => not_built("setup"),
        Some(Command::Logout) => not_built("logout"),
    }
}

/// Starts the TUI: everything up to `Mailbox/get` runs on the plain
/// terminal, and a failure there prints the error and exits 1.
fn tui() -> ExitCode {
    let Some(token) = env_token() else {
        eprintln!(
            "blorp: a Fastmail API token is needed; set BLORP_API_TOKEN to one with the Email scope"
        );
        return ExitCode::FAILURE;
    };
    let _log = match start_log() {
        Ok(guard) => guard,
        Err(message) => {
            eprintln!("blorp: {message}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("blorp: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let (jmap, _session) = HttpJmap::connect(FASTMAIL_SESSION_URL, token)
            .await
            .map_err(|e| e.to_string())?;
        let app = run::launch(&jmap, Timestamp::now())
            .await
            .map_err(|e| e.to_string())?;
        let mut terminal = ratatui::try_init().map_err(|e| {
            ratatui::restore();
            e.to_string()
        })?;
        let result = run::run(
            &mut terminal,
            EventStream::new(),
            jmap,
            run::SystemClock,
            app,
        )
        .await;
        ratatui::restore();
        result.map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("blorp: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `BLORP_API_TOKEN`, when set and not empty.
fn env_token() -> Option<SecretString> {
    env::var("BLORP_API_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(SecretString::from)
}

/// Starts the log, with no log rather than no blorp when its file cannot be
/// opened. An invalid `BLORP_LOG` is an error.
fn start_log() -> Result<Option<WorkerGuard>, String> {
    let filter = env::var("BLORP_LOG").ok();
    let Some(dir) = config::state_dir(|name| env::var_os(name)) else {
        return Ok(None);
    };
    match log::init(&dir.join("blorp.log"), filter.as_deref()) {
        Ok(guard) => Ok(Some(guard)),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Err(error.to_string()),
        Err(error) => {
            eprintln!("blorp: no log this session: {error}");
            Ok(None)
        }
    }
}

fn not_built(what: &str) -> ExitCode {
    eprintln!("blorp: {what} is not built yet");
    ExitCode::FAILURE
}
