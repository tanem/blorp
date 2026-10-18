//! Command-line arguments, and the `setup` and `logout` commands.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;

use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};
use clap::{Parser, Subcommand};
use keyring_core::Entry;
use secrecy::zeroize::Zeroize;
use secrecy::{ExposeSecret, SecretString};

use crate::jmap::{self, Fatal, HttpJmap, Session};
use crate::private_file;

/// A terminal UI for triaging a Fastmail account's mailboxes over JMAP.
#[derive(Debug, Parser)]
#[command(name = "blorp", version, args_conflicts_with_subcommands = true)]
pub struct Cli {
    /// Log actions instead of sending them to Fastmail.
    #[arg(long)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Store a new Fastmail API token, replacing any stored one.
    Setup,
    /// Delete the stored Fastmail API token.
    Logout,
}

/// Where the API token is kept between launches.
pub trait TokenStore {
    /// The stored token, or `None` when none is stored.
    fn get(&self) -> Result<Option<SecretString>, StoreError>;

    /// Stores `token`, replacing any stored one.
    fn set(&self, token: &SecretString) -> Result<(), StoreError>;

    /// Deletes the stored token, returning whether there was one.
    fn delete(&self) -> Result<bool, StoreError>;
}

/// A token store failed: the keychain is locked, access was denied, and the
/// like. Never carries the token.
#[derive(Debug)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// A generic-password item with account `fastmail` in the macOS login
/// keychain: service `blorp` for blorp's token, or another for a test.
pub struct Keychain(Entry);

impl Keychain {
    /// The item, which is not read or written until the store is used.
    pub fn open() -> Result<Keychain, StoreError> {
        Keychain::for_service("blorp")
    }

    /// The item with account `fastmail` under `service`, so a test can use
    /// one that is not blorp's.
    pub fn for_service(service: &str) -> Result<Keychain, StoreError> {
        Cred::build(MacKeychainDomain::User, service, "fastmail")
            .map(Keychain)
            .map_err(store_error)
    }
}

impl TokenStore for Keychain {
    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        match self.0.get_password() {
            Ok(token) => Ok(Some(SecretString::from(token))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    /// Exposes the token to write it, since the keychain stores the token
    /// itself. The token stores' `set` and the `Authorization` header are
    /// the only places the token is exposed.
    fn set(&self, token: &SecretString) -> Result<(), StoreError> {
        self.0
            .set_password(token.expose_secret())
            .map_err(store_error)
    }

    fn delete(&self) -> Result<bool, StoreError> {
        match self.0.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(store_error(error)),
        }
    }
}

fn store_error(error: keyring_core::Error) -> StoreError {
    StoreError(format!("keychain: {error}"))
}

/// A file holding the token in place of the keychain, for tests. Written as
/// any secret-bearing file is: mode 0600 in a 0700 directory, through a temp
/// file and rename, with symlinks refused.
pub struct TokenFile(PathBuf);

impl TokenFile {
    pub fn new(path: impl Into<PathBuf>) -> TokenFile {
        TokenFile(path.into())
    }

    fn error(&self, error: io::Error) -> StoreError {
        StoreError(format!("token store {}: {error}", self.0.display()))
    }
}

impl TokenStore for TokenFile {
    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        let Some(contents) = private_file::read(&self.0).map_err(|e| self.error(e))? else {
            return Ok(None);
        };
        String::from_utf8(contents)
            .map(|token| Some(SecretString::from(token)))
            .map_err(|_| self.error(io::Error::other("not UTF-8")))
    }

    /// Exposes the token to write it, as [`Keychain::set`] does.
    fn set(&self, token: &SecretString) -> Result<(), StoreError> {
        private_file::write(&self.0, token.expose_secret().as_bytes()).map_err(|e| self.error(e))
    }

    fn delete(&self) -> Result<bool, StoreError> {
        private_file::remove(&self.0).map_err(|e| self.error(e))
    }
}

/// The token a bare `blorp` uses: `env`, from `BLORP_API_TOKEN`, when set,
/// else the stored one. `None` means setup must run.
pub fn find_token(
    env: Option<SecretString>,
    store: &dyn TokenStore,
) -> Result<Option<SecretString>, StoreError> {
    match env {
        Some(token) => Ok(Some(token)),
        None => store.get(),
    }
}

/// Where setup reads the token from.
pub trait Prompt {
    /// Reads the token, showing `prompt` first if a person is typing it.
    fn read_token(&mut self, prompt: &str) -> io::Result<String>;
}

/// The token as the first line of a reader, with no prompt: stdin when it
/// is not a terminal, or scripted input in tests.
pub struct FirstLine<R>(pub R);

impl<R: BufRead> Prompt for FirstLine<R> {
    fn read_token(&mut self, _prompt: &str) -> io::Result<String> {
        let mut line = String::new();
        self.0.read_line(&mut line)?;
        Ok(line)
    }
}

/// The token typed at the terminal with echo off, or read from stdin when
/// stdin is not a terminal. Ctrl-C fails the read as
/// [`io::ErrorKind::Interrupted`], with the terminal restored.
pub struct Stdin;

impl Prompt for Stdin {
    fn read_token(&mut self, prompt: &str) -> io::Result<String> {
        if io::stdin().is_terminal() {
            // rpassword raises SIGINT for Ctrl-C before it restores echo, so
            // the default action would leave the terminal without echo. A
            // SIGINT sent from outside is ignored too while the prompt reads.
            let _ignored = IgnoreSigint::new()?;
            rpassword::prompt_password(prompt)
        } else {
            FirstLine(io::stdin().lock()).read_token(prompt)
        }
    }
}

/// Ignores SIGINT until dropped, then puts back the action it replaced.
struct IgnoreSigint(libc::sigaction);

impl IgnoreSigint {
    fn new() -> io::Result<IgnoreSigint> {
        // SAFETY: sigaction is called with valid pointers to initialised
        // structs, and SIG_IGN needs no handler code.
        unsafe {
            let mut ignore: libc::sigaction = std::mem::zeroed();
            ignore.sa_sigaction = libc::SIG_IGN;
            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGINT, &ignore, &mut previous) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(IgnoreSigint(previous))
        }
    }
}

impl Drop for IgnoreSigint {
    fn drop(&mut self) {
        // SAFETY: restores the action saved in `new`.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
        }
    }
}

/// `blorp setup`, and the first launch with no token: explains where to
/// create a token, reads one through `prompt`, checks it against the session
/// at `session_url`, and only then stores it, replacing any stored token.
/// Returns the client and session the check produced.
pub async fn setup(
    store: &dyn TokenStore,
    prompt: &mut impl Prompt,
    out: &mut impl Write,
    session_url: &str,
) -> Result<(HttpJmap, Session), CommandError> {
    writeln!(
        out,
        "blorp needs a Fastmail API token. Create one in Fastmail under \
         Settings, Privacy & Security, Connected apps & API tokens, with the \
         Email scope only: not read-only, and not Email submission."
    )?;
    out.flush()?;
    let mut typed = prompt
        .read_token("API token: ")
        .map_err(|error| match error.kind() {
            io::ErrorKind::Interrupted => CommandError::Cancelled,
            _ => CommandError::Io(error),
        })?;
    let entered = !typed.trim().is_empty();
    let token = Arc::new(SecretString::from(typed.trim()));
    typed.zeroize();
    if !entered {
        return Err(CommandError::NoToken);
    }
    let (jmap, session) = HttpJmap::connect(session_url, token.clone()).await?;
    store.set(&token)?;
    writeln!(
        out,
        "Stored the API token for {} in the keychain.",
        session.username
    )?;
    Ok((jmap, session))
}

/// A command failed.
#[derive(Debug)]
pub enum CommandError {
    Store(StoreError),
    Io(io::Error),
    Jmap(jmap::Error),
    /// Setup read an empty token.
    NoToken,
    /// Ctrl-C at the setup prompt.
    Cancelled,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Store(error) => error.fmt(f),
            CommandError::Io(error) => error.fmt(f),
            CommandError::Jmap(error) => match error.fatal() {
                Some(fatal) => fatal.fmt(f),
                None => error.fmt(f),
            },
            CommandError::NoToken => write!(f, "no API token was entered"),
            CommandError::Cancelled => write!(f, "setup was cancelled"),
        }
    }
}

impl CommandError {
    /// Why blorp must stop, if Fastmail refused the token.
    pub fn fatal(&self) -> Option<Fatal> {
        match self {
            CommandError::Jmap(error) => error.fatal(),
            _ => None,
        }
    }
}

impl From<jmap::Error> for CommandError {
    fn from(error: jmap::Error) -> Self {
        CommandError::Jmap(error)
    }
}

impl std::error::Error for CommandError {}

impl From<StoreError> for CommandError {
    fn from(error: StoreError) -> Self {
        CommandError::Store(error)
    }
}

impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        CommandError::Io(error)
    }
}

/// `blorp logout`: deletes the stored token, and warns when
/// `BLORP_API_TOKEN` is set, since blorp uses that token regardless. The
/// config file and the snooze schedule are left alone.
pub fn logout(
    store: &dyn TokenStore,
    env_token_set: bool,
    out: &mut impl Write,
) -> Result<(), CommandError> {
    if store.delete()? {
        writeln!(out, "Deleted the API token from the keychain.")?;
    } else {
        writeln!(out, "No API token was stored in the keychain.")?;
    }
    if env_token_set {
        writeln!(
            out,
            "BLORP_API_TOKEN is set, so blorp will still use the token it holds."
        )?;
    }
    Ok(())
}
