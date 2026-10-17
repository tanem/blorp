//! blorp triages a Fastmail account's mailboxes over JMAP.

use std::process::ExitCode;

pub mod cli;

use cli::{Cli, Command};

/// Runs blorp for parsed arguments and returns the process exit code.
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        None => not_built("the terminal UI"),
        Some(Command::Setup) => not_built("setup"),
        Some(Command::Logout) => not_built("logout"),
    }
}

fn not_built(what: &str) -> ExitCode {
    eprintln!("blorp: {what} is not built yet");
    ExitCode::FAILURE
}
