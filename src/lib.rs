//! blorp triages a Fastmail account's mailboxes over JMAP.

use std::env;
use std::io;
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

use cli::{Cli, Command, CommandError, Keychain, Stdin};
use jmap::HttpJmap;
use jmap::http::FASTMAIL_SESSION_URL;
use run::Exit;

/// Runs blorp for parsed arguments and returns the process exit code.
pub fn run(cli: Cli) -> ExitCode {
    let result = match cli.command {
        None => tui(),
        Some(Command::Setup) => setup(),
        Some(Command::Logout) => logout(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("blorp: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `blorp setup`: always prompts, whether or not `BLORP_API_TOKEN` is set.
fn setup() -> Result<(), String> {
    let store = Keychain::open().map_err(|e| e.to_string())?;
    let _log = start_log()?;
    runtime()?.block_on(async {
        cli::setup(&store, &mut Stdin, &mut io::stdout(), FASTMAIL_SESSION_URL)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

fn logout() -> Result<(), String> {
    let store = Keychain::open().map_err(|e| e.to_string())?;
    cli::logout(&store, env_token().is_some(), &mut io::stdout()).map_err(|e| e.to_string())
}

/// Everything up to `Mailbox/get` runs on the plain terminal, including
/// setup when no token is found; only then does the TUI take over.
fn tui() -> Result<(), String> {
    let store = Keychain::open().map_err(|e| e.to_string())?;
    let token = cli::find_token(env_token(), &store).map_err(|e| e.to_string())?;
    let _log = start_log()?;
    runtime()?.block_on(async {
        let (jmap, _session) = match token {
            Some(token) => HttpJmap::connect(FASTMAIL_SESSION_URL, token)
                .await
                .map_err(CommandError::from),
            None => cli::setup(&store, &mut Stdin, &mut io::stdout(), FASTMAIL_SESSION_URL).await,
        }
        .map_err(|e| e.to_string())?;
        let app = run::launch(&jmap, Timestamp::now())
            .await
            .map_err(|e| CommandError::from(e).to_string())?;
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
        match result.map_err(|e| e.to_string())? {
            Exit::Quit => Ok(()),
            Exit::Fatal(fatal) => Err(fatal.to_string()),
        }
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|e| e.to_string())
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
