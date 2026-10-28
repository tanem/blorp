# blorp

A terminal UI for a Fastmail account's mailboxes over JMAP.

![blorp showing a made-up Inbox with row numbers down the left, moving the cursor down and up, typing a count of 5 and then 4 to move five rows down and four up, marking a thread read, archiving a thread, showing only the unread threads and then every thread again, searching for "your" and clearing the search, opening the mailbox list with its unread counts, moving three rows down it with a count, showing only the mailboxes with unread mail, narrowing it to "rep" and switching to Work/Reports, then quitting](demo/blorp.gif)

[![ci](https://github.com/tanem/blorp/actions/workflows/ci.yml/badge.svg)](https://github.com/tanem/blorp/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/github/license/tanem/blorp)](LICENSE)

Status: early. blorp shows one mailbox at a time, starting on the Inbox, and has ten keys, `j`, `k`, `e`, `r`, `o`, `m`, `u`, `/`, `Esc`, and `q`, with `Ctrl-c` also quitting, and a count can go before `j` or `k`; more are added as using blorp shows they are needed ([ADR 0003](docs/adr/0003-every-key-earns-its-place.md)).

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

Create an API token in Fastmail under Settings, Privacy & Security, Connected apps & API tokens, with the Email scope only and without read-only access. blorp needs write access to mail to archive threads; with a read-only token it shows mail, and `e` says the token is read-only.

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
| `e` | Archive |
| `r` | Mark read |
| `o` | Open in Fastmail |
| `m` | Switch mailbox |
| `u` | Unread only |
| `/` | Search |
| `Esc` | Clear search |
| `q` | Quit |
| `Ctrl-c` | Quit |

On the mailbox list, which `m` opens in place of the queue with each mailbox's count of unread threads, `j` and `k` move, `u` shows only the mailboxes holding an unread thread, Enter switches to the highlighted mailbox, Esc clears the narrowing or else closes the list, and `q` and `Ctrl-c` quit. `/` narrows the list by path as it searches the queue: Enter at the prompt locks the text in without switching.

A count typed before `j` or `k`, such as `12j`, moves the cursor on the queue, or the highlight on the mailbox list, that many rows, stopping at the first or last row. It shows on the right of the statusline until `j` or `k` uses it; Esc and Enter drop it, and any other key drops it and acts once, so `3e` archives one thread. While a prompt is open, digits go into its text. Each row has a row number on its left: the cursor's or highlight's own row shows its position among the rows listed, and every other row shows its distance from it, the count to type to reach it. Search, unread only, and Unread mailboxes only narrow the rows that are numbered.

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
