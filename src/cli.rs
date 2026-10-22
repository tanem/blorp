//! Command-line arguments, and the `setup` and `logout` commands.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use keyring_core::Entry;
use secrecy::zeroize::Zeroize;
use secrecy::{ExposeSecret, SecretString};

use crate::jmap::{self, Fatal, HttpJmap, Session};
use crate::private_file;

/// A terminal UI for a Fastmail account's Inbox over JMAP.
#[derive(Debug, Parser)]
#[command(name = "blorp", version)]
pub struct Cli {
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
    /// Fails with [`StoreError::Unavailable`] when the store cannot be
    /// reached, without reading the token.
    fn reachable(&self) -> Result<(), StoreError> {
        Ok(())
    }

    /// The stored token, or `None` when none is stored.
    fn get(&self) -> Result<Option<SecretString>, StoreError>;

    /// Stores `token`, replacing any stored one.
    fn set(&self, token: &SecretString) -> Result<(), StoreError>;

    /// Deletes the stored token, returning whether there was one.
    fn delete(&self) -> Result<bool, StoreError>;
}

/// A token store failed. Never carries the token.
#[derive(Debug)]
pub enum StoreError {
    /// No keyring can be reached, as on a Linux machine with no Secret
    /// Service, so the token can come only from `BLORP_API_TOKEN`.
    Unavailable(String),
    /// The keyring is locked, access was denied, and the like.
    Failed(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Unavailable(error) => write!(
                f,
                "cannot reach {KEYRING} to keep the API token in: {error}. \
                 Set BLORP_API_TOKEN to the token, or run `blorp setup` once it is available."
            ),
            StoreError::Failed(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for StoreError {}

/// The platform keyring blorp keeps the token in, as messages name it.
#[cfg(target_os = "macos")]
pub const KEYRING: &str = "the macOS keychain";
/// The platform keyring blorp keeps the token in, as messages name it.
#[cfg(target_os = "windows")]
pub const KEYRING: &str = "Windows Credential Manager";
/// The platform keyring blorp keeps the token in, as messages name it.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const KEYRING: &str = "the Secret Service keyring";

/// The credential with user `fastmail` in the platform keyring: service
/// `blorp` for blorp's token, or another for a test. On macOS it is a
/// generic-password item in the login keychain; on Windows a generic
/// credential that stays on this machine; elsewhere a Secret Service item
/// in the default collection.
pub struct Keyring {
    service: String,
}

impl Keyring {
    /// blorp's credential, which is not read or written until the store is
    /// used.
    pub fn blorp() -> Keyring {
        Keyring::for_service("blorp")
    }

    /// The credential with user `fastmail` under `service`, so a test can
    /// use one that is not blorp's.
    pub fn for_service(service: &str) -> Keyring {
        Keyring {
            service: service.into(),
        }
    }

    #[cfg(target_os = "macos")]
    fn entry(&self) -> Result<Entry, StoreError> {
        use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};

        Cred::build(MacKeychainDomain::User, &self.service, "fastmail").map_err(store_error)
    }

    #[cfg(target_os = "windows")]
    fn entry(&self) -> Result<Entry, StoreError> {
        use keyring_core::api::CredentialStoreApi;
        use windows_native_keyring_store::Store;

        let local = std::collections::HashMap::from([("persistence", "Local")]);
        Store::new()
            .and_then(|store| store.build(&self.service, "fastmail", Some(&local)))
            .map_err(store_error)
    }

    /// Connects to Secret Service, which fails where there is none.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn entry(&self) -> Result<Entry, StoreError> {
        use dbus_secret_service_keyring_store::Store;
        use keyring_core::api::CredentialStoreApi;

        let store = Store::new().map_err(|error| StoreError::Unavailable(error.to_string()))?;
        store
            .build(&self.service, "fastmail", None)
            .map_err(store_error)
    }
}

impl TokenStore for Keyring {
    fn reachable(&self) -> Result<(), StoreError> {
        self.entry().map(drop)
    }

    fn get(&self) -> Result<Option<SecretString>, StoreError> {
        match self.entry()?.get_password() {
            Ok(token) => Ok(Some(SecretString::from(token))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    /// Exposes the token to write it, since the keyring stores the token
    /// itself. The token stores' `set` and the `Authorization` header are
    /// the only places the token is exposed.
    fn set(&self, token: &SecretString) -> Result<(), StoreError> {
        self.entry()?
            .set_password(token.expose_secret())
            .map_err(store_error)
    }

    fn delete(&self) -> Result<bool, StoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(store_error(error)),
        }
    }
}

fn store_error(error: keyring_core::Error) -> StoreError {
    StoreError::Failed(format!("{KEYRING}: {error}"))
}

/// A file holding the token in place of the keyring, for tests. Written as
/// any secret-bearing file is, through [`private_file`].
pub struct TokenFile(PathBuf);

impl TokenFile {
    pub fn new(path: impl Into<PathBuf>) -> TokenFile {
        TokenFile(path.into())
    }

    fn error(&self, error: io::Error) -> StoreError {
        StoreError::Failed(format!("token store {}: {error}", self.0.display()))
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

    /// Exposes the token to write it, as [`Keyring::set`] does.
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
            catching_ctrl_c(|| rpassword::prompt_password(prompt))
        } else {
            FirstLine(io::stdin().lock()).read_token(prompt)
        }
    }
}

/// Runs `read` with SIGINT ignored. rpassword raises SIGINT for Ctrl-C
/// before it restores echo, so the default action would leave the terminal
/// without echo; ignored, the signal does nothing and rpassword fails the
/// read as [`io::ErrorKind::Interrupted`]. A SIGINT sent from outside is
/// ignored too while the prompt reads.
#[cfg(unix)]
fn catching_ctrl_c(read: impl FnOnce() -> io::Result<String>) -> io::Result<String> {
    let _ignored = IgnoreSigint::new()?;
    read()
}

/// Ignores SIGINT until dropped, then puts back the action it replaced.
#[cfg(unix)]
struct IgnoreSigint(libc::sigaction);

#[cfg(unix)]
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

#[cfg(unix)]
impl Drop for IgnoreSigint {
    fn drop(&mut self) {
        // SAFETY: restores the action saved in `new`.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
        }
    }
}

/// Runs `read` with a console control handler that takes Ctrl-C. rpassword
/// leaves Ctrl-C to the console, whose default handler ends the process
/// before rpassword restores echo. Handled here, Ctrl-C adds an Enter key
/// to the console input, which ends the read whether or not it has
/// started; rpassword restores the console mode as it returns, and the
/// flag the handler set turns the result into
/// [`io::ErrorKind::Interrupted`].
#[cfg(windows)]
fn catching_ctrl_c(read: impl FnOnce() -> io::Result<String>) -> io::Result<String> {
    use std::sync::atomic::Ordering;

    let handler = CtrlCHandler::new()?;
    let result = read();
    drop(handler);
    if CTRL_C.load(Ordering::SeqCst) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    result
}

/// Set by [`on_ctrl_c`] while a [`CtrlCHandler`] is installed.
#[cfg(windows)]
static CTRL_C: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs on a thread the console starts for the event. If the read has
/// already ended, the Enter stays in the console input, and the shell reads
/// it as an empty line once blorp exits.
#[cfg(windows)]
unsafe extern "system" fn on_ctrl_c(event: u32) -> windows_sys::core::BOOL {
    use std::sync::atomic::Ordering;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        CTRL_C_EVENT, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD,
        KEY_EVENT_RECORD_0, WriteConsoleInputW,
    };

    /// The virtual-key code of Enter.
    const VK_RETURN: u16 = 0x0D;

    if event != CTRL_C_EVENT {
        return 0;
    }
    CTRL_C.store(true, Ordering::SeqCst);
    let enter = |down| INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: down,
                wRepeatCount: 1,
                wVirtualKeyCode: VK_RETURN,
                wVirtualScanCode: 0,
                uChar: KEY_EVENT_RECORD_0 {
                    UnicodeChar: u16::from(b'\r'),
                },
                dwControlKeyState: 0,
            },
        },
    };
    let keys = [enter(1), enter(0)];
    let name: Vec<u16> = "CONIN$".encode_utf16().chain([0]).collect();
    // SAFETY: `name` is NUL-terminated, the handle is checked and closed,
    // and `keys` holds two initialised records.
    unsafe {
        let input = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if input != INVALID_HANDLE_VALUE {
            let mut written = 0;
            WriteConsoleInputW(input, keys.as_ptr(), keys.len() as u32, &mut written);
            CloseHandle(input);
        }
    }
    1
}

/// Handles Ctrl-C with [`on_ctrl_c`] until dropped.
#[cfg(windows)]
struct CtrlCHandler;

#[cfg(windows)]
impl CtrlCHandler {
    fn new() -> io::Result<CtrlCHandler> {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

        CTRL_C.store(false, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: `on_ctrl_c` touches only an atomic and a console handle it
        // opens and closes.
        if unsafe { SetConsoleCtrlHandler(Some(on_ctrl_c), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(CtrlCHandler)
    }
}

#[cfg(windows)]
impl Drop for CtrlCHandler {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

        // SAFETY: removes the handler `new` added.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl_c), 0);
        }
    }
}

/// `blorp setup`, and the first launch with no token: checks the store can
/// be reached, explains where to create a token, reads one through
/// `prompt`, checks it against the session at `session_url`, and only then
/// stores it, replacing any stored token.
/// Returns the client and session the check produced.
pub async fn setup(
    store: &dyn TokenStore,
    prompt: &mut impl Prompt,
    out: &mut impl Write,
    session_url: &str,
) -> Result<(HttpJmap, Session), CommandError> {
    store.reachable()?;
    writeln!(
        out,
        "blorp needs a Fastmail API token. Create one in Fastmail under \
         Settings, Privacy & Security, Connected apps & API tokens, with \
         read-only access and the Email scope only."
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
        "Stored the API token for {} in {KEYRING}.",
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
/// `BLORP_API_TOKEN` is set, since blorp uses that token regardless.
pub fn logout(
    store: &dyn TokenStore,
    env_token_set: bool,
    out: &mut impl Write,
) -> Result<(), CommandError> {
    if store.delete()? {
        writeln!(out, "Deleted the API token from {KEYRING}.")?;
    } else {
        writeln!(out, "No API token was stored in {KEYRING}.")?;
    }
    if env_token_set {
        writeln!(
            out,
            "BLORP_API_TOKEN is set, so blorp will still use the token it holds."
        )?;
    }
    Ok(())
}
