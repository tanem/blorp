use std::process::Output;

use blorp::cli::{Cli, Command};
use clap::Parser;

/// Runs blorp with no API token in the environment. Only arguments that
/// clap answers on its own are safe here: anything else reads the keyring.
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
fn help_lists_the_commands_and_flags() {
    let output = blorp(&["--help"]);

    assert!(output.status.success());
    let help = text(&output.stdout);
    for item in ["setup", "logout", "--version", "--help"] {
        assert!(help.contains(item), "help is missing {item}:\n{help}");
    }
}

#[test]
fn an_unknown_flag_is_rejected() {
    let output = blorp(&["--no-such-flag"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn setup_and_logout_are_commands() {
    let setup = Cli::try_parse_from(["blorp", "setup"]).unwrap();
    let logout = Cli::try_parse_from(["blorp", "logout"]).unwrap();

    assert!(matches!(setup.command, Some(Command::Setup)));
    assert!(matches!(logout.command, Some(Command::Logout)));
}
