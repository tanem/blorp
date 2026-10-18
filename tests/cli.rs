use std::process::{Command, Output};

/// Runs blorp with no API token in the environment, so nothing reaches the
/// network.
fn blorp(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blorp"))
        .args(args)
        .env_remove("BLORP_API_TOKEN")
        .output()
        .expect("run blorp")
}

const NO_TOKEN: &str =
    "blorp: a Fastmail API token is needed; set BLORP_API_TOKEN to one with the Email scope\n";

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
fn bare_command_without_a_token_says_one_is_needed() {
    let output = blorp(&[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    assert_eq!(text(&output.stderr), NO_TOKEN);
}

#[test]
fn an_empty_token_counts_as_none() {
    let output = Command::new(env!("CARGO_BIN_EXE_blorp"))
        .env("BLORP_API_TOKEN", "")
        .output()
        .expect("run blorp");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), NO_TOKEN);
}

#[test]
fn dry_run_is_accepted_and_still_needs_a_token() {
    let output = blorp(&["--dry-run"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), NO_TOKEN);
}

#[test]
fn setup_says_it_is_not_built_yet() {
    let output = blorp(&["setup"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), "blorp: setup is not built yet\n");
}

#[test]
fn logout_says_it_is_not_built_yet() {
    let output = blorp(&["logout"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), "blorp: logout is not built yet\n");
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
