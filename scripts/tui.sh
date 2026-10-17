#!/usr/bin/env bash
# Drives the debug blorp binary in a detached tmux session, for checking a
# change in the real terminal against the real account.
#
#   scripts/tui.sh start [--no-token | --bad-token] [COLSxROWS] [-- BLORP_ARGS...]
#       Build, then run blorp in tmux (default 80x24). The token comes from
#       the keychain item (service blorp, account fastmail) and only ever
#       reaches blorp's environment. --no-token runs with BLORP_API_TOKEN
#       empty. Arguments after -- go to blorp, such as --dry-run. BLORP_LOG
#       is passed through. The log path is printed on start.
#   scripts/tui.sh wait TEXT [SECONDS]   Wait until the screen shows TEXT (default 15 s).
#   scripts/tui.sh keys KEY...           Send tmux key names: j 1 2 G Escape Enter C-c.
#   scripts/tui.sh screen                Print the screen.
#   scripts/tui.sh resize COLSxROWS      Resize the window.
#   scripts/tui.sh status                Exit code and tty modes once blorp has exited.
#   scripts/tui.sh log-check             Count log lines containing the token; 0 when clean.
#   scripts/tui.sh stop                  Kill the session.

set -euo pipefail

session=blorp-verify
state="${TMPDIR:-/tmp}"
state="${state%/}/blorp-verify/state"
log="$state/blorp/blorp.log"
root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${CARGO_TARGET_DIR:-$root/target}/debug/blorp"

die() {
  echo "tui.sh: $*" >&2
  exit 1
}

require_session() {
  tmux has-session -t "$session" 2>/dev/null || die "no $session session; run start first"
}

require_keychain_item() {
  security find-generic-password -s blorp -a fastmail >/dev/null 2>&1 \
    || die "no keychain item (service blorp, account fastmail)"
}

# Sets cols and rows from COLSxROWS.
parse_size() {
  [[ "$1" =~ ^[0-9]+x[0-9]+$ ]] || die "size must be COLSxROWS, such as 80x24"
  cols="${1%x*}" rows="${1#*x}"
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
      *) require_keychain_item ;;
    esac
    dims=80x24
    if [[ $# -gt 0 && "$1" != -- ]]; then dims="$1"; shift; fi
    parse_size "$dims"
    [[ "${1:-}" == -- ]] && shift
    args=""
    for arg in "$@"; do args+=" $(printf '%q' "$arg")"; done
    cargo build --quiet --manifest-path "$root/Cargo.toml"
    tmux kill-session -t "$session" 2>/dev/null || true
    rm -rf "$state" && mkdir -p "$state"
    tmux new-session -d -s "$session" -x "$cols" -y "$rows" \
      "BLORP_API_TOKEN=$token XDG_STATE_HOME=$(printf '%q' "$state") ${BLORP_LOG:+BLORP_LOG=$(printf '%q' "$BLORP_LOG")} $(printf '%q' "$binary")$args; \
       echo \"blorp exited \$?\"; stty -a | grep -oE '(-)?(icanon|echo) ' | tr -d '\n'; echo; sleep 3600"
    tmux set-option -t "$session" window-size manual >/dev/null
    echo "started ${cols}x${rows}; log in $log"
    ;;
  wait)
    text="${2:?usage: wait TEXT [SECONDS]}" seconds="${3:-15}"
    require_session
    for _ in $(seq 1 $((seconds * 4))); do
      screen | grep -qF -- "$text" && exit 0
      sleep 0.25
    done
    screen >&2
    die "timed out after ${seconds}s waiting for: $text"
    ;;
  keys)
    shift
    require_session
    for key in "$@"; do
      tmux send-keys -t "$session" "$key"
      sleep 0.15
    done
    sleep 0.3
    ;;
  screen)
    require_session
    screen
    ;;
  resize)
    parse_size "${2:?usage: resize COLSxROWS}"
    require_session
    tmux resize-window -t "$session" -x "$cols" -y "$rows"
    sleep 0.5
    ;;
  status)
    require_session
    echo "alternate screen: $(tmux display -p -t "$session" '#{alternate_on}')"
    screen | grep -E '^(blorp exited|-?icanon)' || die "blorp has not exited"
    ;;
  log-check)
    [[ -f "$log" ]] || die "no log at $log"
    require_keychain_item
    # The token reaches grep as a pattern file, not an argument.
    grep -cFf <(security find-generic-password -s blorp -a fastmail -w) "$log" || [[ $? -eq 1 ]]
    ;;
  stop)
    tmux kill-session -t "$session" 2>/dev/null || true
    ;;
  *)
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
    exit 2
    ;;
esac
