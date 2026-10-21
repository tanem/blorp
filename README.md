# blorp

A terminal UI for a Fastmail account's Inbox over JMAP. Rust, ratatui.

Status: in development. blorp starts from a minimal set of keys, and a key is added only when using blorp shows it is needed ([ADR 0003](docs/adr/0003-every-key-earns-its-place.md)).

`blorp` shows the Inbox, newest first, one row per thread, up to the newest 500 threads. Each row shows `●` when any email of the thread is unread and `⚑` when any is flagged, then the sender and subject of the thread's newest Inbox email, the number of emails in the thread when there is more than one, and how long ago that email arrived. The statusline shows the Inbox's name and the cursor's row out of the rows shown, followed by the Inbox's total when it holds more than 500 threads.

| Key | Action |
| --- | --- |
| `j` | Down |
| `k` | Up |
| `q` | Quit |
| `Ctrl-c` | Quit |

The queue follows changes made elsewhere, such as in the web client: blorp listens on Fastmail's push stream and re-runs the Inbox query on each change and once a minute. The cursor stays on its thread while that thread is still in the Inbox.

A request that fails because Fastmail cannot be reached, is overloaded, or is limiting the rate of requests is retried three times, 1, 2, and 4 s apart, or after the wait its `Retry-After` header asks for, up to 60 s; the bottom line shows `Retrying Fastmail (2/3)` meanwhile. Any other failure is not retried: a failure of the first Inbox query shows as `Fastmail error: <type>`, and the response is written to the log. Once the retries run out blorp is disconnected: the statusline shows a `DISCONNECTED` badge, and blorp tries a reconcile 5 s later, doubling the wait each time up to a minute, or at once on a push event. The first that succeeds brings the queue up to date and says `Reconnected`. At launch, when the session or the mailbox list cannot be fetched despite the retries, blorp prints `Could not reach Fastmail: <error>` and exits 1. When Fastmail rejects the API token, blorp prints why and exits 1.

blorp needs a Fastmail API token with the Email scope only (not read-only, and not Email submission), created in Fastmail under Settings, Privacy & Security, Connected apps & API tokens. On the first launch blorp asks for one, checks it with Fastmail, and stores it in the platform keyring under service `blorp` and user `fastmail`: the login keychain on macOS, Secret Service (such as GNOME Keyring or KWallet) on Linux, and Credential Manager on Windows. `blorp setup` replaces the stored token and `blorp logout` deletes it; neither revokes the token, which is done in Fastmail Settings. `BLORP_API_TOKEN`, when set, is used in place of the keyring, for CI, scripted runs, and machines with no keyring, such as a Linux server without Secret Service; there `blorp setup` and launch say that no keyring can be reached. blorp never writes the token to a plain file. On macOS a rebuilt development binary asks once for keychain access; that is expected.

blorp runs on macOS, Linux, and Windows. The log is `~/.local/state/blorp/blorp.log` (or under `$XDG_STATE_HOME`) on macOS and Linux and `%LOCALAPPDATA%\blorp\blorp.log` on Windows, filtered by `BLORP_LOG` in `RUST_LOG` syntax. blorp has no config file.

`cargo test` needs no network beyond localhost and no secrets. The end-to-end tests run the binary in a pseudo-terminal against a local JMAP server, through two variables that only debug builds read: `BLORP_SESSION_URL` replaces Fastmail's session URL, and `BLORP_TOKEN_STORE=<path>` replaces the keyring with a file that only the user can read. `cargo test --test live -- --ignored` runs read-only smoke tests against Fastmail with `BLORP_API_TOKEN` set, one of them the end-to-end harness in a pseudo-terminal. `cargo test --test keyring -- --ignored` checks the keyring adapter against the platform keyring under a throwaway service name; it needs no token.
