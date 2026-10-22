# blorp

A terminal UI for a Fastmail account's Inbox over JMAP.

![blorp showing a made-up Inbox, moving the cursor down and up, then quitting](demo/blorp.gif)

[![ci](https://github.com/tanem/blorp/actions/workflows/ci.yml/badge.svg)](https://github.com/tanem/blorp/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/github/license/tanem/blorp)](LICENSE)

Status: early. blorp shows the Inbox and has three keys; more are added as using blorp shows they are needed ([ADR 0003](docs/adr/0003-every-key-earns-its-place.md)).

## Requirements

- macOS, Linux, or Windows.
- A Fastmail account.
- A Fastmail API token.

## Install

With a Rust toolchain installed:

```sh
cargo install --git https://github.com/tanem/blorp
```

## Setup

Create an API token in Fastmail under Settings, Privacy & Security, Connected apps & API tokens, with read-only access and the Email scope only. blorp only reads mail for now; when it gains its first action that changes mail, the token will need full access to the Email scope.

On the first launch, `blorp` asks for the token, checks it with Fastmail, and stores it in the platform keyring. `blorp setup` replaces the stored token, and `blorp logout` deletes it. Neither revokes the token; revoke it in Fastmail under the same settings.

Where no keyring is available, such as on a Linux server without Secret Service, set `BLORP_API_TOKEN` instead, for example from a password manager's command-line tool:

```sh
BLORP_API_TOKEN="$(pass show fastmail/blorp)" blorp
```

## Keys

| Key | Action |
| --- | --- |
| `j` | Down |
| `k` | Up |
| `q` | Quit |
| `Ctrl-c` | Quit |

## Files and environment variables

| | macOS | Linux and other Unix systems | Windows |
| --- | --- | --- | --- |
| API token | Login keychain | Secret Service, such as GNOME Keyring or KWallet | Credential Manager |
| Log | `~/.local/state/blorp/blorp.log` | `~/.local/state/blorp/blorp.log` | `%LOCALAPPDATA%\blorp\blorp.log` |

The token is kept under service `blorp` and user `fastmail`. On macOS and Linux the log is under `$XDG_STATE_HOME/blorp/` when `XDG_STATE_HOME` is set. The log is emptied at each launch. blorp has no config file.

| Variable | Effect |
| --- | --- |
| `BLORP_API_TOKEN` | The API token to use in place of the stored one. |
| `BLORP_LOG` | What the log records, in `RUST_LOG` syntax, such as `blorp=debug`. Unset, it records warnings and errors. |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). To report a security problem, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
