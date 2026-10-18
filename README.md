# blorp

A terminal UI for triaging a Fastmail account's mailboxes over JMAP. Rust, ratatui.

Status: in development. `blorp` shows the Inbox read-only: `j`, `k`, `g`, `G`, and count prefixes move the cursor, and `q` quits.

blorp needs a Fastmail API token with the Email scope only (not read-only, and not Email submission), created in Fastmail under Settings, Privacy & Security, Connected apps & API tokens. On the first launch blorp asks for one, checks it with Fastmail, and stores it in the macOS login keychain (service `blorp`, account `fastmail`). `blorp setup` replaces the stored token and `blorp logout` deletes it; neither revokes the token, which is done in Fastmail Settings. `BLORP_API_TOKEN`, when set, is used in place of the keychain, for CI and scripted runs. A rebuilt development binary asks once for keychain access; that is expected.

The log is `~/.local/state/blorp/blorp.log` (or under `$XDG_STATE_HOME`), filtered by `BLORP_LOG` in `RUST_LOG` syntax.

`cargo test` needs no network or secrets. `cargo test --test live -- --ignored` runs a read-only smoke test against Fastmail with `BLORP_API_TOKEN` set.
