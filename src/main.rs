use std::process::ExitCode;

use clap::Parser;

use blorp::cli::Cli;

fn main() -> ExitCode {
    blorp::run(Cli::parse())
}
