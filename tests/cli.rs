use std::process::Output;

use blorp::cli::{Cli, Command};
use clap::Parser;

/// Runs blorp with no API token in the environment. Only arguments that
/// clap answers on its own are safe here: anything else reads the keychain.
fn blorp(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_blorp"))
        .args(args)
        .env_remove("BLORP_API_TOKEN")
        .output()
        .expect("run blorp")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8 output")
}

#[test]
fn version_prints_the_name_and_version() {
    let output = blorp(&["--version"]);

    assert!(output.status.success());
    assert_eq!(text(&output.stdout), "blorp 0.1.0\n");
}

#[test]
fn dry_run_is_rejected_alongside_a_subcommand() {
    for args in [
        ["--dry-run", "setup"],
        ["setup", "--dry-run"],
        ["--dry-run", "logout"],
    ] {
        let output = blorp(&args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn help_lists_the_commands_and_flags() {
    let output = blorp(&["--help"]);

    assert!(output.status.success());
    let help = text(&output.stdout);
    for item in ["setup", "logout", "--dry-run", "--version", "--help"] {
        assert!(help.contains(item), "help is missing {item}:\n{help}");
    }
}

#[test]
fn dry_run_is_accepted_on_the_bare_command() {
    let cli = Cli::try_parse_from(["blorp", "--dry-run"]).unwrap();

    assert!(cli.dry_run);
    assert!(cli.command.is_none());
}

#[test]
fn setup_and_logout_are_commands() {
    let setup = Cli::try_parse_from(["blorp", "setup"]).unwrap();
    let logout = Cli::try_parse_from(["blorp", "logout"]).unwrap();

    assert!(matches!(setup.command, Some(Command::Setup)));
    assert!(matches!(logout.command, Some(Command::Logout)));
}
