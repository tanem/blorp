# TUI verification

Drive the real `blorp` binary in tmux against the real Fastmail account with `scripts/tui.sh`, read the screen, and compare it against the spec in #3 and the ticket. `scripts/tui.sh` with no arguments prints its commands and options.

## Prerequisites

- tmux (`brew install tmux`).
- The keychain item, service `blorp` and account `fastmail`, holding a Fastmail API token. `security find-generic-password -s blorp -a fastmail >/dev/null` checks it exists and reads metadata only. If it is missing, ask Tane to run `security add-generic-password -s blorp -a fastmail -w` in a separate terminal window, where it prompts with no echo.

## The token

The token reaches only blorp's environment: `scripts/tui.sh` reads it from the keychain inside the tmux command, and `log-check` passes it to grep as a pattern file. Commands, screen captures, and reports carry the keychain lookup, never the value. If `wait` times out with a keychain prompt on the screen, ask Tane to allow it.

## Dry run

Run with `-- --dry-run` whenever the check presses a key or runs a command that changes mail (archive, delete, move, flag, read, spam, snooze, undo). A live send needs Tane's go-ahead for that run.

## Procedure

1. `start`, with a size for layout changes and `BLORP_LOG=trace` set for a full log.
2. `wait` on text that appears once the screen settles, such as the first row's sender or ` 1/` in the statusline.
3. For each behaviour the change touches: `keys`, then `screen`, and compare. `resize` for layout.
4. Quit with `q` or `C-c`, then `status`: expect `alternate screen: 0`, `blorp exited 0`, and `icanon echo`.
5. `log-check`: expect `0`.
6. For startup and error paths: `start --no-token` (BLORP_API_TOKEN empty) or `start --bad-token`, `wait 'blorp exited'`, then `screen`.
7. `stop`.

Done when every behaviour the change touches has a screen capture that matches the spec, the terminal is restored, and the log check prints 0.

## Mail content

Screens show Tane's real mail. In the conversation, quote only the rows and lines that show a behaviour, and ask Tane to confirm that rows match the web UI, since only Tane can make that comparison. In PR comments and issues, record behaviours, counts, and statusline text, and keep mail content to the conversation.
