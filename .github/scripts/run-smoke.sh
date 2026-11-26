#!/usr/bin/env bash
# Runs the desktop app in smoke mode and fails unless every launch passed. The app (built with its
# frontend embedded, so no dev server is involved) drives the real UI through the real typed
# commands and channels, then prints one `GHOST_SMOKE_RESULT <json>` line and exits 0 on a pass
# or 1 on a failure, through its quit flow (apps/desktop/src-tauri/src/smoke.rs). Its own watchdog
# reports a failure if the UI never reports, so a hang shorter than the CI step timeout still ends
# with a verdict.
#
# Scenarios:
# - stream: a demo turn, a cancelled turn and a stress burst (GHOST_SMOKE=1), in a new data
#   folder.
# - chat: the chat screens against the demo agent (apps/desktop/src/lib/smokeChat.ts), in a new data
#   folder and a new chat folder (GHOST_SMOKE_FOLDER). The first launch (GHOST_SMOKE=chat)
#   ends with a turn running and quits through the quit flow; this script then checks that the quit
#   removed the leftover registry file and folded the database's write-ahead log back, and starts a
#   second launch on the same folders (GHOST_SMOKE=chat-reopen) that checks the stored history,
#   the interrupted turn and Resume. With GHOST_SMOKE_AGENT=<agent id> set, chat is instead one
#   minimal chat with that real agent: for local runs only, since a real agent spends its user's
#   quota, so this script refuses it under CI (and so does the app).
# - all (the default): stream, then chat. A failure in one does not skip the other.
#
# Every launch must agree on both signals: exactly one result line saying "ok": true for the
# scenario it was asked for, and exit code 0. A crash after a passing report, a report lost on the
# way out, a doubled report, or a report for another scenario all fail. Each chat check is printed.
#
# Usage: run-smoke.sh <app executable> [stream|chat|all]   (Linux needs a display: use xvfb-run)
# shellcheck source-path=SCRIPTDIR
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

usage="usage: run-smoke.sh <app executable> [stream|chat|all]"
{ [ "$#" -ge 1 ] && [ "$#" -le 2 ]; } || die "$usage"
app=$1
scenario=${2:-all}
case $scenario in
stream | chat | all) ;;
*) die "$usage" ;;
esac
if ! { [ -f "$app" ] && [ -x "$app" ]; }; then
  die "$app is not an executable file; build it first (pnpm --filter @ghost/desktop tauri build --debug --no-bundle)"
fi

# On values as the app reads them (launch.rs): 1, true, yes or on, in any case.
is_on() {
  case "$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')" in
  1 | true | yes | on) return 0 ;;
  *) return 1 ;;
  esac
}

# The real agent a local chat run may ask for. Each launch gets the smoke switches it needs and no
# others: the stream check and the reopened launch refuse an agent.
agent="$(printf '%s' "${GHOST_SMOKE_AGENT:-}" | tr -d '[:space:]')"
unset GHOST_SMOKE GHOST_SMOKE_EXIT GHOST_SMOKE_FOLDER GHOST_SMOKE_AGENT
if [ -n "$agent" ] && { is_on "${CI:-}" || is_on "${GITHUB_ACTIONS:-}"; }; then
  die "GHOST_SMOKE_AGENT=$agent runs a real agent, which spends its user's quota: it is for local runs only, and CI must never set it."
fi

scratch="$(to_unix_path "${RUNNER_TEMP:-${TMPDIR:-/tmp}}")"
# macOS's TMPDIR ends with a slash.
scratch=${scratch%/}

# launch <label> <scenario the report must name> <data folder> [VAR=value...]: runs the app once in
# smoke mode with the data folder and the variables given, and checks its one result line and its
# exit code. Prints the verdict (and each check of a chat scenario); returns 1 on a failure.
launch() {
  local label=$1 want=$2 data=$3
  shift 3
  local log="$data.$label.stdout" status results count report ok got summary
  printf 'Running %s: the %s launch (data folder %s)\n' "$app" "$label" "$data"
  set +e
  env "$@" GHOST_SMOKE_EXIT=1 GHOST_DATA_DIR="$(to_native_path "$data")" \
    "$app" </dev/null | tee "$log"
  status=${PIPESTATUS[0]}
  set -e

  results="$(grep -a '^GHOST_SMOKE_RESULT ' "$log" | tr -d '\r' || true)"
  count="$(printf '%s' "$results" | grep -c '' || true)"
  if [ "$count" -eq 0 ]; then
    error "The $label launch exited with code $status without printing a GHOST_SMOKE_RESULT line (its log is above)."
    return 1
  fi
  if [ "$count" -gt 1 ]; then
    error "The $label launch printed $count GHOST_SMOKE_RESULT lines; it must report exactly once."
    return 1
  fi

  report=${results#GHOST_SMOKE_RESULT }
  # tostring: with -e, a bare false would read as jq failing.
  if ! ok="$(printf '%s\n' "$report" | jq -er 'if type == "object" and (.ok | type) == "boolean" then (.ok | tostring) else error("no boolean ok") end' 2>/dev/null)"; then
    error "The $label smoke result is not a JSON object with a boolean \"ok\": $report"
    return 1
  fi
  ok=${ok%$'\r'}
  # Reports from before scenarios existed name none; they were all the stream check.
  got="$(printf '%s\n' "$report" | jq -r 'if has("scenario") then (.scenario // "no scenario") else "stream" end' | tr -d '\r')"

  # "?" for a field the report lacks: smoke.rs falls back to a bare {"ok":false,"errors":[...]} if
  # the full report cannot be serialized.
  if [ "$want" = stream ]; then
    summary="$(printf '%s\n' "$report" | jq -r 'def n(f): f // "?"; "\(n(.stream.events_received)) events in \(n(.stream.frames_received)) frames, \(n(.stream.seq_gaps)) sequence gaps, \(n(.stream.rendered_chars)) characters rendered, \(n(.stream.fps_min))/\(n(.stream.fps_avg)) fps (min/avg)"' | tr -d '\r')"
  else
    printf '%s\n' "$report" | jq -r '(.checks // [])[] | "\(if .ok then "ok   " else "FAIL " end) \(.name): \(.detail)"' | tr -d '\r'
    summary="$(printf '%s\n' "$report" | jq -r '"\([(.checks // [])[] | select(.ok)] | length) of \((.checks // []) | length) checks passed"' | tr -d '\r')"
  fi

  if [ "$ok" != true ]; then
    error "The $label smoke check failed ($summary):"
    printf '%s\n' "$report" | jq -r '(.errors // [])[] | "  - \(.)"' | tr -d '\r'
    return 1
  fi
  if [ "$got" != "$want" ]; then
    error "The $label launch reported the $got scenario, not $want: is $app older than this script?"
    return 1
  fi
  if [ "$status" -ne 0 ]; then
    error "The $label smoke check reported a pass ($summary), but the app then exited with code $status."
    return 1
  fi
  notice "Smoke check passed ($label): $summary."
}

# quit_left_nothing <label> <data folder>: what the quit flow removes once the app has exited: this
# instance's leftover registry file (Unix; Windows keeps none), and the database's write-ahead log,
# which only a closed connection folds back into the file (spec 10.4: no -wal file left).
quit_left_nothing() {
  local label=$1 data=$2 found
  found="$(find "$data/run" -type f -name '*.json' 2>/dev/null | tr '\n' ' ' || true)"
  if [ -n "$found" ]; then
    error "The $label launch's quit left its leftover registry file behind: $found"
    return 1
  fi
  found="$(find "$data/profiles" -type f -name '*-wal' -size +0c 2>/dev/null | tr '\n' ' ' || true)"
  if [ -n "$found" ]; then
    error "The $label launch's quit left the database's write-ahead log behind, so it never closed the database: $found"
    return 1
  fi
  notice "The $label launch's quit removed its registry file and closed the database."
}

# A fresh, empty data folder for each scenario: the first-launch path (profile and database created
# from nothing), and nothing from an earlier run can leak in.
run_stream() {
  local data
  data="$(mktemp -d "$scratch/auracode-smoke.XXXXXX")"
  launch stream stream "$data" GHOST_SMOKE=1
}

run_chat() {
  local data folder native
  data="$(mktemp -d "$scratch/auracode-smoke-chat.XXXXXX")"
  # A space in the chat folder's name, as in many real ones.
  folder="$(mktemp -d "$scratch/auracode smoke chat.XXXXXX")"
  native="$(to_native_path "$folder")"
  if [ -n "$agent" ]; then
    launch agent agent "$data" GHOST_SMOKE=chat GHOST_SMOKE_FOLDER="$native" \
      GHOST_SMOKE_AGENT="$agent" || return 1
    quit_left_nothing agent "$data"
    return
  fi
  if ! launch chat chat "$data" GHOST_SMOKE=chat GHOST_SMOKE_FOLDER="$native"; then
    warning "Skipped the chat-reopen launch: it starts from what the chat launch leaves."
    return 1
  fi
  quit_left_nothing chat "$data" || return 1
  launch chat-reopen chat_reopen "$data" GHOST_SMOKE=chat-reopen \
    GHOST_SMOKE_FOLDER="$native" || return 1
  quit_left_nothing chat-reopen "$data"
}

failed=0
case $scenario in
stream) run_stream || failed=1 ;;
chat) run_chat || failed=1 ;;
all)
  run_stream || failed=1
  run_chat || failed=1
  ;;
esac
exit "$failed"
