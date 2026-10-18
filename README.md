# blorp

A terminal UI for triaging a Fastmail account's mailboxes over JMAP. Rust, ratatui.

Status: in development. `blorp` shows the Inbox: `j`, `k`, `g`, `G`, and count prefixes move the cursor, `e` archives the thread under the cursor, and `q` quits. `blorp --dry-run`, or `D` while running, turns on dry run: actions change the queue on screen and are written to the log at `~/.local/state/blorp/blorp.log` instead of being sent to Fastmail.

blorp needs a Fastmail API token with the Email scope only (not read-only, and not Email submission), created in Fastmail under Settings, Privacy & Security, Connected apps & API tokens. On the first launch blorp asks for one, checks it with Fastmail, and stores it in the macOS login keychain (service `blorp`, account `fastmail`). `blorp setup` replaces the stored token and `blorp logout` deletes it; neither revokes the token, which is done in Fastmail Settings. `BLORP_API_TOKEN`, when set, is used in place of the keychain, for CI and scripted runs. A rebuilt development binary asks once for keychain access; that is expected.

The log is `~/.local/state/blorp/blorp.log` (or under `$XDG_STATE_HOME`), filtered by `BLORP_LOG` in `RUST_LOG` syntax.

`cargo test` needs no network beyond localhost and no secrets. The end-to-end tests run the binary in a pseudo-terminal against a local JMAP server, through two variables that only debug builds read: `BLORP_SESSION_URL` replaces Fastmail's session URL, and `BLORP_TOKEN_STORE=<path>` replaces the keychain with a file. `cargo test --test live -- --ignored` runs read-only smoke tests against Fastmail with `BLORP_API_TOKEN` set, one of them the end-to-end harness in a pseudo-terminal. `cargo test --test keychain -- --ignored` checks the keychain adapter against the real login keychain under a throwaway service name; it needs no token.
