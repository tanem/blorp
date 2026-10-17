#!/usr/bin/env bash
# Drives the debug blorp binary in a detached tmux session, for checking a
# change in the real terminal against the real account.
#
#   scripts/tui.sh start [--no-token | --bad-token] [COLSxROWS]
#       Build, then run blorp in tmux (default 80x24). The token comes from
#       the keychain item (service blorp, account fastmail) and only ever
#       reaches blorp's environment. The log goes to a temporary state
#       directory, printed on start.
#   scripts/tui.sh wait TEXT [SECONDS]   Wait until the screen shows TEXT (default 15 s).
#   scripts/tui.sh keys KEY...           Send tmux key names: j 1 2 G Escape Enter C-c.
#   scripts/tui.sh screen                Print the screen.
#   scripts/tui.sh resize COLSxROWS      Resize the window.
#   scripts/tui.sh status                After blorp exits: its exit code and tty modes.
#   scripts/tui.sh log-check             Count token occurrences in the log; prints 0 when clean.
#   scripts/tui.sh stop                  Kill the session.

set -euo pipefail

session=blorp-verify
state="${TMPDIR:-/tmp}"
state="${state%/}/blorp-verify/state"
root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${CARGO_TARGET_DIR:-$root/target}/debug/blorp"

keychain_token() {
  security find-generic-password -s blorp -a fastmail -w
}

size() {
  local cols="${1%x*}" rows="${1#*x}"
  printf '%s %s' "$cols" "$rows"
}

screen() {
  tmux capture-pane -t "$session" -p
}

case "${1:-}" in
  start)
    shift
    token='"$(security find-generic-password -s blorp -a fastmail -w)"'
    case "${1:-}" in
      --no-token) token=''; shift ;;
      --bad-token) token='not-a-real-token'; shift ;;
    esac
    read -r cols rows <<<"$(size "${1:-80x24}")"
    cargo build --quiet --manifest-path "$root/Cargo.toml"
    tmux kill-session -t "$session" 2>/dev/null || true
    rm -rf "$state" && mkdir -p "$state"
    tmux new-session -d -s "$session" -x "$cols" -y "$rows" \
      "BLORP_API_TOKEN=$token XDG_STATE_HOME='$state' ${BLORP_LOG:+BLORP_LOG='$BLORP_LOG'} '$binary'; \
       echo \"blorp exited \$?\"; stty -a | grep -oE '(-)?(icanon|echo) ' | tr -d '\n'; echo; sleep 3600"
    tmux set-option -t "$session" window-size manual >/dev/null
    echo "started ${cols}x${rows}; log in $state/blorp/blorp.log"
    ;;
  wait)
    text="$2" seconds="${3:-15}"
    for _ in $(seq 1 $((seconds * 4))); do
      screen | grep -qF -- "$text" && exit 0
      sleep 0.25
    done
    echo "timed out after ${seconds}s waiting for: $text" >&2
    screen >&2
    exit 1
    ;;
  keys)
    shift
    for key in "$@"; do
      tmux send-keys -t "$session" "$key"
      sleep 0.15
    done
    sleep 0.3
    ;;
  screen)
    screen
    ;;
  resize)
    read -r cols rows <<<"$(size "$2")"
    tmux resize-window -t "$session" -x "$cols" -y "$rows"
    sleep 0.5
    ;;
  status)
    echo "alternate screen: $(tmux display -p -t "$session" '#{alternate_on}')"
    screen | grep -E '^(blorp exited|-?icanon)' || echo "blorp is still running"
    ;;
  log-check)
    grep -cF -- "$(keychain_token)" "$state/blorp/blorp.log" || true
    ;;
  stop)
    tmux kill-session -t "$session" 2>/dev/null || true
    ;;
  *)
    sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
