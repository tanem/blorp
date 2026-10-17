# TUI verification

Drive the real `blorp` binary in tmux against the real Fastmail account, read the screen, and report what it shows. This catches what `cargo test` cannot: Fastmail's own limits and quirks (the 500-thread queue cap came from this), raw mode and terminal restore, and real key decoding. Automated end-to-end tests against a local fake are #27; this is the live check that complements them.

## When

Run it before pushing any change to what blorp draws, what a key or command does, startup and exit, or a Fastmail request. Run it again when behaviour changes after review.

## Prerequisites

- tmux (`brew install tmux`).
- The keychain item, service `blorp` and account `fastmail`, holding a Fastmail API token. Check it exists with `security find-generic-password -s blorp -a fastmail >/dev/null`, which reads metadata only. If it is missing, ask Tane to run `security add-generic-password -s blorp -a fastmail -w` in a separate terminal window (it prompts with no echo; `!` in Claude Code cannot take the hidden input). Once setup is built (#8), `blorp setup` writes the same item.

## The token

The token reaches only blorp's environment, through `scripts/tui.sh`, which reads it from the keychain inside the tmux command. Commands, screen captures, and reports carry the variable or the keychain lookup, never the value. When checking for leaks, use `scripts/tui.sh log-check`, which prints a count.

## Procedure

`scripts/tui.sh` with no arguments prints its commands. A run:

1. `scripts/tui.sh start` (or `start 120x40`; `BLORP_LOG=trace scripts/tui.sh start` for a full log). It builds the debug binary and prints the log path, which is under `$TMPDIR`, never the home directory.
2. `scripts/tui.sh wait '<text>'` until the screen settles, for example `wait 'of 6443'` or a statusline position. Wait on text, not on fixed sleeps.
3. For each behaviour the change touches: `scripts/tui.sh keys …` with tmux key names (`j`, `1 2 G`, `Escape`, `Enter`, `C-c`), then `scripts/tui.sh screen`, and compare against the spec in #3 and the ticket. Use `resize` for layout changes.
4. Quit with `q` or `C-c`, then `scripts/tui.sh status`: expect `alternate screen: 0`, `blorp exited 0`, and `icanon echo`.
5. `scripts/tui.sh log-check`: expect `0`.
6. For startup and error paths: `start --no-token` and `start --bad-token`, then `wait 'blorp exited'` and `screen`.
7. `scripts/tui.sh stop`.

Done when every behaviour the change touches has a screen capture that matches the spec, the terminal is restored, and the log check prints 0.

## Mail content

Screens show Tane's real mail. In the conversation, quote only the rows and lines needed to show a behaviour, and ask Tane to confirm that rows match the web UI; only Tane can make that comparison. In PR comments and issues, record behaviours, counts, and statusline text, with mail content left out.
