//! Command-line arguments, and the `setup` and `logout` commands.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Arc;

use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};
use clap::{Parser, Subcommand};
use keyring_core::Entry;
use secrecy::zeroize::Zeroize;
use secrecy::{ExposeSecret, SecretString};

use crate::jmap::{self, HttpJmap, Session};

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
        write!(f, "keychain: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// The generic-password item with service `blorp` and account `fastmail`
/// in the macOS login keychain.
pub struct Keychain(Entry);

impl Keychain {
    pub fn open() -> Result<Keychain, StoreError> {
        Cred::build(MacKeychainDomain::User, "blorp", "fastmail")
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

    /// The one place besides the `Authorization` header that the token is
    /// exposed, since the keychain stores the token itself.
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
    StoreError(error.to_string())
}

/// The token a bare `blorp` uses: `env`, from `BLORP_API_TOKEN`, when set,
/// else the stored one. `None` means setup must run.
pub fn find_token(
    env: Option<SecretString>,
    store: &impl TokenStore,
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
pub struct Lines<R>(pub R);

impl<R: BufRead> Prompt for Lines<R> {
    fn read_token(&mut self, _prompt: &str) -> io::Result<String> {
        let mut line = String::new();
        self.0.read_line(&mut line)?;
        Ok(line)
    }
}

/// The token typed at the terminal with echo off, or read from stdin when
/// stdin is not a terminal.
pub struct Stdin;

impl Prompt for Stdin {
    fn read_token(&mut self, prompt: &str) -> io::Result<String> {
        if io::stdin().is_terminal() {
            rpassword::prompt_password(prompt)
        } else {
            Lines(io::stdin().lock()).read_token(prompt)
        }
    }
}

/// `blorp setup`, and the first launch with no token: explains where to
/// create a token, reads one through `prompt`, checks it against the session
/// at `session_url`, and only then stores it, replacing any stored token.
/// Returns the client and session the check produced.
pub async fn setup(
    store: &impl TokenStore,
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
    let mut typed = prompt.read_token("API token: ")?;
    let token = Arc::new(SecretString::from(typed.trim()));
    typed.zeroize();
    if token.expose_secret().is_empty() {
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
    store: &impl TokenStore,
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
