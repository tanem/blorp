# blorp

A terminal UI for triaging a Fastmail account's mailboxes over JMAP. Rust, ratatui.

Status: in development. With `BLORP_API_TOKEN` set to a Fastmail API token with the Email scope, `blorp` shows the Inbox read-only: `j`, `k`, `g`, `G`, and count prefixes move the cursor, and `q` quits.

The log is `~/.local/state/blorp/blorp.log` (or under `$XDG_STATE_HOME`), filtered by `BLORP_LOG` in `RUST_LOG` syntax.

`cargo test` needs no network or secrets. `cargo test --test live -- --ignored` runs a read-only smoke test against Fastmail with `BLORP_API_TOKEN` set.
