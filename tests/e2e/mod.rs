//! Support for the end-to-end tests: the blorp binary in a pseudo-terminal,
//! read back through a terminal emulator, against a local JMAP server.

#![allow(dead_code)]

pub mod server;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blorp::cli::{TokenFile, TokenStore};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use secrecy::{ExposeSecret, SecretString};
use tempfile::TempDir;

/// The token the local server accepts.
pub const TOKEN: &str = "fake-token-1";

/// What blorp prints when Fastmail answers 401.
pub const REJECTED: &str = "Fastmail rejected the API token. It may have been removed in \
Settings. Run `blorp setup` to store a new one.";

/// How long a wait for the screen or for blorp to exit may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The bytes that switch a terminal to the alternate screen.
const ALTERNATE_SCREEN: &str = "\x1b[?1049h";

/// A temp directory for one run of blorp: its home, its state and config
/// directories, and its token file. Every spawned binary gets an
/// environment holding only these and what the test adds, so it reaches no
/// keychain, nothing under the real home directory, and no
/// `BLORP_API_TOKEN` from the shell that runs the tests.
pub struct Sandbox {
    dir: TempDir,
    vars: Vec<(String, String)>,
    redact: bool,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        if !cfg!(debug_assertions) {
            panic!(
                "the end-to-end tests need a debug build: a release build ignores \
                 BLORP_SESSION_URL and BLORP_TOKEN_STORE"
            );
        }
        Sandbox {
            dir: tempfile::tempdir().expect("temp directory"),
            vars: Vec::new(),
            redact: false,
        }
    }

    /// Points blorp at a local server in place of Fastmail.
    pub fn session_url(self, url: &str) -> Sandbox {
        self.env("BLORP_SESSION_URL", url)
    }

    pub fn env(mut self, name: &str, value: &str) -> Sandbox {
        self.vars.push((name.into(), value.into()));
        self
    }

    /// Leaves the screen out of failure messages, for runs against real mail.
    pub fn redacted(mut self) -> Sandbox {
        self.redact = true;
        self
    }

    /// The file blorp keeps the token in, in a directory blorp creates.
    pub fn token_path(&self) -> PathBuf {
        self.dir.path().join("secret/token")
    }

    /// The log file blorp writes under its state directory.
    pub fn log_path(&self) -> PathBuf {
        self.dir.path().join("home/state/blorp/blorp.log")
    }

    pub fn store_token(&self, token: &str) {
        TokenFile::new(self.token_path())
            .set(&SecretString::from(token))
            .expect("store the token");
    }

    pub fn stored_token(&self) -> Option<String> {
        TokenFile::new(self.token_path())
            .get()
            .expect("read the token")
            .map(|token| token.expose_secret().to_string())
    }

    /// Starts `blorp ARGS` in a terminal of `cols` by `rows`.
    pub fn spawn(&self, args: &[&str], cols: u16, rows: u16) -> Blorp {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .expect("open a pseudo-terminal");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_blorp"));
        command.args(args);
        command.cwd(self.dir.path());
        command.env_clear();
        let home = self.dir.path().join("home");
        let token_path = self.token_path();
        let base = [
            ("HOME", home.to_str().unwrap()),
            ("XDG_STATE_HOME", &format!("{}/state", home.display())),
            ("XDG_CONFIG_HOME", &format!("{}/config", home.display())),
            ("BLORP_TOKEN_STORE", token_path.to_str().unwrap()),
            ("TERM", "xterm-256color"),
        ];
        for (name, value) in base {
            command.env(name, value);
        }
        for (name, value) in &self.vars {
            command.env(name, value);
        }
        let child = pair.slave.spawn_command(command).expect("spawn blorp");
        drop(pair.slave);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                parser: vt100::Parser::new(rows, cols, 0),
                output: Vec::new(),
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let mut reader = pair.master.try_clone_reader().expect("pty reader");
        let feed = shared.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                let n = reader.read(&mut buffer).unwrap_or(0);
                let mut state = feed.state.lock().unwrap();
                if n == 0 {
                    state.closed = true;
                } else {
                    state.parser.process(&buffer[..n]);
                    state.output.extend_from_slice(&buffer[..n]);
                }
                feed.changed.notify_all();
                if n == 0 {
                    break;
                }
            }
        });
        let writer = pair.master.take_writer().expect("pty writer");
        Blorp {
            master: pair.master,
            writer,
            child,
            shared,
            redact: self.redact,
        }
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    parser: vt100::Parser,
    output: Vec<u8>,
    closed: bool,
}

/// A running blorp and the terminal it draws on.
pub struct Blorp {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    shared: Arc<Shared>,
    redact: bool,
}

/// The terminal modes blorp leaves behind.
#[derive(Debug, PartialEq, Eq)]
pub struct Modes {
    /// Line editing, off in raw mode.
    pub canonical: bool,
    pub echo: bool,
}

impl Modes {
    /// The modes of a terminal out of raw mode.
    pub const COOKED: Modes = Modes {
        canonical: true,
        echo: true,
    };

    /// Raw mode, as the TUI runs in.
    pub const RAW: Modes = Modes {
        canonical: false,
        echo: false,
    };
}

impl Blorp {
    /// Waits until `ready` holds for the screen, or fails naming `what`.
    pub fn wait_for(&self, what: &str, ready: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        let mut state = self.state();
        while !ready(state.parser.screen()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let screen = self.describe(&state);
                drop(state);
                panic!("timed out waiting for {what}{screen}");
            }
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }

    /// Waits until `text` is on screen.
    pub fn wait_text(&self, text: &str) {
        self.wait_for(&format!("{text:?} on screen"), |screen| {
            screen.contents().contains(text)
        });
    }

    /// Waits until the statusline, trimmed, ends with `text`.
    pub fn wait_statusline(&self, text: &str) {
        self.wait_for(&format!("a statusline ending {text:?}"), |screen| {
            statusline(screen).ends_with(text)
        });
    }

    /// The screen's rows, without trailing spaces.
    pub fn rows(&self) -> Vec<String> {
        rows(self.state().parser.screen())
    }

    /// The second-last row, without trailing spaces.
    pub fn statusline(&self) -> String {
        statusline(self.state().parser.screen())
    }

    /// Types `keys` as the bytes a terminal sends.
    pub fn keys(&mut self, keys: &str) {
        self.writer.write_all(keys.as_bytes()).expect("type keys");
        self.writer.flush().expect("type keys");
    }

    /// Resizes the terminal, which sends blorp `SIGWINCH`.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.state().parser.screen_mut().set_size(rows, cols);
        self.master.resize(size(cols, rows)).expect("resize");
    }

    /// Waits until the terminal stops echoing, as it does once a hidden
    /// prompt is reading.
    pub fn wait_echo_off(&self) {
        let deadline = Instant::now() + TIMEOUT;
        while self.modes().echo {
            assert!(Instant::now() < deadline, "timed out waiting for echo off");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn modes(&self) -> Modes {
        let termios = self.master.get_termios().expect("terminal modes");
        let flags = termios.local_flags.bits();
        Modes {
            canonical: flags & libc::ICANON != 0,
            echo: flags & libc::ECHO != 0,
        }
    }

    /// Waits for blorp to exit and its output to be read.
    pub fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("blorp's status") {
                break status;
            }
            if Instant::now() >= deadline {
                let screen = self.describe(&self.state());
                panic!("timed out waiting for blorp to exit{screen}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut state = self.state();
        while !state.closed {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out reading blorp's output");
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
        status
    }

    /// Everything blorp wrote to the terminal, as text.
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.state().output).into_owned()
    }

    /// Whether blorp ever switched to the alternate screen, as the TUI does.
    pub fn entered_alternate_screen(&self) -> bool {
        self.output().contains(ALTERNATE_SCREEN)
    }

    /// Whether the terminal is on the alternate screen now.
    pub fn on_alternate_screen(&self) -> bool {
        self.state().parser.screen().alternate_screen()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap()
    }

    fn describe(&self, state: &State) -> String {
        if self.redact {
            return String::new();
        }
        format!(":\n{}", state.parser.screen().contents())
    }
}

impl Drop for Blorp {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

/// The screen's rows, without trailing spaces.
pub fn rows(screen: &vt100::Screen) -> Vec<String> {
    let (_, cols) = screen.size();
    screen
        .rows(0, cols)
        .map(|row| row.trim_end().to_string())
        .collect()
}

/// The screen's second-last row, without trailing spaces.
pub fn statusline(screen: &vt100::Screen) -> String {
    let mut rows = rows(screen);
    let index = rows.len().saturating_sub(2);
    rows.swap_remove(index)
}
