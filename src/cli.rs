//! Command-line arguments.

use clap::{Parser, Subcommand};

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
