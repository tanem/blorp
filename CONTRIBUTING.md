# Contributing

## Building

blorp is a Rust crate. `cargo build` builds it and `cargo run` runs it against Fastmail with the token in the platform keyring or `BLORP_API_TOKEN`. On macOS a rebuilt development binary asks once for keychain access; that is expected.

## Checks

CI runs these on macOS, Linux, and Windows, and a pull request needs all three to pass on every platform:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
```

`cargo test` needs no network beyond localhost and no secrets. The end-to-end tests run the binary in a pseudo-terminal against a local JMAP server, through two variables that only debug builds read: `BLORP_SESSION_URL` replaces Fastmail's session URL, and `BLORP_TOKEN_STORE=<path>` replaces the keyring with a file that only the user can read. A test fails when the key table in `README.md` differs from the key bindings in `src/keymap.rs`.

Two sets of tests are ignored by default:

- `cargo test --test live -- --ignored` runs read-only smoke tests against Fastmail with `BLORP_API_TOKEN` set, one of them the end-to-end harness in a pseudo-terminal.
- `cargo test --test keyring -- --ignored` checks the keyring adapter against the platform keyring under a throwaway service name. It needs no token.

## Keys

blorp starts from a minimal set of keys, and a key is added only when using blorp shows it is needed, with one issue per key that names the need ([ADR 0003](docs/adr/0003-every-key-earns-its-place.md)). To ask for a key, open an issue with the "Request a key" template. A pull request that adds a key adds it to the key table in `README.md` and re-records the demo.

## Demo

`demo/blorp.tape` renders `demo/blorp.gif` with [VHS](https://github.com/charmbracelet/vhs). Run `vhs demo/blorp.tape` from the repo root. The tape runs a debug build against the made-up account in `examples/demo_server.rs`, served on `127.0.0.1:4848`, never a real account.

## Commits

- The subject is an imperative sentence in sentence case, with no prefix and no trailing period, such as `Keep the required check named check`.
- The body, wrapped at 72 columns, says what changed and why.
- The body ends with `Refs #<issue>.`, or `Closes #<issue>.` when the commit finishes the issue.

## Writing

Commits, comments, and docs are plain and factual. Markdown prose is not hard-wrapped; commit bodies are.
