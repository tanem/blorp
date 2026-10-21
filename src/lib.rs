//! blorp shows a Fastmail account's Inbox over JMAP.

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
pub mod keymap;
pub mod log;
pub mod mail;
pub mod private_file;
pub mod queue;
pub mod run;
pub mod sync;
pub mod ui;

use cli::{Cli, Command, CommandError, Keyring, Stdin, StoreError, TokenFile, TokenStore};
use config::DebugOverrides;
use jmap::http::FASTMAIL_SESSION_URL;
use jmap::{Fatal, HttpJmap};
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
        Err(Failure::Cancelled) => ExitCode::from(130),
        Err(Failure::Fatal(fatal)) => {
            eprintln!("{fatal}");
            ExitCode::FAILURE
        }
        Err(Failure::Unreachable(error)) => {
            eprintln!("Could not reach Fastmail: {error}");
            ExitCode::FAILURE
        }
        Err(Failure::Other(message)) => {
            eprintln!("blorp: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Why blorp exits 1: Fastmail refused the token, printed as the fixed
/// message; Fastmail could not be reached at launch despite retries; or
/// anything else, printed after `blorp: `. Ctrl-C at the setup prompt
/// exits 130, as a shell reports SIGINT, with nothing printed.
enum Failure {
    Cancelled,
    Fatal(Fatal),
    Unreachable(jmap::Error),
    Other(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Other(message)
    }
}

impl From<CommandError> for Failure {
    fn from(error: CommandError) -> Self {
        if let CommandError::Cancelled = error {
            return Failure::Cancelled;
        }
        match error.fatal() {
            Some(fatal) => Failure::Fatal(fatal),
            None => Failure::Other(error.to_string()),
        }
    }
}

impl From<jmap::Error> for Failure {
    /// A transient failure, which has been retried, is Fastmail being out
    /// of reach.
    fn from(error: jmap::Error) -> Self {
        if error.class() == jmap::Class::Transient {
            return Failure::Unreachable(error);
        }
        CommandError::from(error).into()
    }
}

impl From<StoreError> for Failure {
    fn from(error: StoreError) -> Self {
        CommandError::from(error).into()
    }
}

/// `blorp setup`: always prompts, whether or not `BLORP_API_TOKEN` is set.
fn setup() -> Result<(), Failure> {
    let overrides = debug_overrides();
    let store = token_store(&overrides);
    let _log = start_log()?;
    runtime()?.block_on(async {
        cli::setup(
            &*store,
            &mut Stdin,
            &mut io::stdout(),
            &session_url(&overrides),
        )
        .await?;
        Ok(())
    })
}

/// `blorp logout`.
fn logout() -> Result<(), Failure> {
    let store = token_store(&debug_overrides());
    cli::logout(&*store, env_token().is_some(), &mut io::stdout())?;
    Ok(())
}

/// Everything up to `Mailbox/get` runs on the plain terminal, with setup
/// first when no token is found; only then does the TUI take over. The
/// session fetch and `Mailbox/get` are retried on the usual schedule.
fn tui() -> Result<(), Failure> {
    let overrides = debug_overrides();
    let store = token_store(&overrides);
    let token = cli::find_token(env_token(), &*store)?;
    let session_url = session_url(&overrides);
    let _log = start_log()?;
    runtime()?.block_on(async {
        let (jmap, _session) = match token {
            Some(token) => {
                let token = std::sync::Arc::new(token);
                run::connect(|| HttpJmap::connect(&session_url, token.clone())).await?
            }
            None => cli::setup(&*store, &mut Stdin, &mut io::stdout(), &session_url).await?,
        };
        let app = run::launch(&jmap, Timestamp::now()).await?;
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
            Exit::Fatal(fatal) => Err(Failure::Fatal(fatal)),
        }
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|e| e.to_string())
}

fn debug_overrides() -> DebugOverrides {
    config::debug_overrides(cfg!(debug_assertions), |name| env::var_os(name))
}

/// Fastmail's session URL, or `BLORP_SESSION_URL` in a debug build.
fn session_url(overrides: &DebugOverrides) -> String {
    overrides
        .session_url
        .clone()
        .unwrap_or_else(|| FASTMAIL_SESSION_URL.to_string())
}

/// The platform keyring, or the file `BLORP_TOKEN_STORE` names in a debug
/// build.
fn token_store(overrides: &DebugOverrides) -> Box<dyn TokenStore> {
    match &overrides.token_store {
        Some(path) => Box::new(TokenFile::new(path)),
        None => Box::new(Keyring::blorp()),
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
