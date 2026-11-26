#!/usr/bin/env bash
# Collects the Windows spawning spike's test build into one self-contained folder: the integration
# test executable (renamed windows-spawn-tests) beside the spawn-probe and gui-host helpers it
# starts, plus a README. On Windows, CI uploads the folder as an artifact, so the manual checklist
# in docs/spikes/windows-spawn.md runs on a PC without Rust.
#
# The helpers are moved, not copied. Cargo bakes their build paths into the test executable, and
# with nothing left at those paths, CI's run of the staged tests proves the fallback that finds the
# helpers beside the test executable instead (helper_binary in tests/spawn.rs), which is the only
# way the artifact can work on another machine.
#
# Usage: stage-spawn-spike.sh <cargo test --no-run --message-format=json output> <empty folder>
# Output: test_exe=<the staged test executable>
# shellcheck source-path=SCRIPTDIR
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

[ "$#" -eq 2 ] || die "usage: stage-spawn-spike.sh <cargo JSON messages> <destination folder>"
# On Windows these arrive as $RUNNER_TEMP/..., a Windows path with a Unix tail.
messages="$(to_unix_path "$1")"
dest="$(to_unix_path "$2")"
[ -f "$messages" ] || die "no such file: $messages"

# name, kind, test|build, executable: one tab-separated line per artifact with an executable.
# join, not @tsv: @tsv escapes backslashes, which would double every separator in a Windows path.
# jq on Windows ends lines with CRLF, hence the tr.
artifacts="$(jq -r '
  select(.reason == "compiler-artifact" and .executable != null)
  | [.target.name, (.target.kind | join(",")), (if .profile.test then "test" else "build" end), .executable]
  | join("\t")' "$messages" | tr -d '\r')" ||
  die "could not read cargo's JSON messages from $messages"

# Sets <variable> to the one executable cargo built for <name> <kind> <test|build>, as a Git Bash
# path. Assigns rather than prints, so `die` runs in this shell and its message reaches the log
# instead of vanishing into a command substitution.
pick() {
  local matches count
  matches="$(printf '%s\n' "$artifacts" | awk -F'\t' -v name="$2" -v kind="$3" -v mode="$4" \
    '$1 == name && $2 == kind && $3 == mode { print $4 }')"
  count="$(printf '%s' "$matches" | grep -c '' || true)"
  if [ "$count" -ne 1 ]; then
    die "expected 1 $4 artifact for the $3 target \"$2\" in $messages, found $count (was it built with --no-run --message-format=json?)"
  fi
  printf -v "$1" '%s' "$(to_unix_path "$matches")"
}

tests="" probe="" gui=""
pick tests spawn test test
pick probe spawn-probe bin build
pick gui gui-host bin build
for file in "$tests" "$probe" "$gui"; do
  [ -f "$file" ] || die "cargo reported $file, but there is no such file"
done

mkdir -p "$dest"
if [ -n "$(ls -A "$dest")" ]; then
  die "$dest is not empty; stage into an empty folder so nothing stale ends up in the artifact"
fi

ext=""
case $tests in *.exe) ext=".exe" ;; esac
staged="$dest/windows-spawn-tests$ext"
mv "$tests" "$staged"
# The helpers keep their file names: the fallback looks for exactly these names.
mv "$probe" "$dest/$(basename "$probe")"
mv "$gui" "$dest/$(basename "$gui")"

cat >"$dest/README.txt" <<EOF
Ghost: the Windows spawning spike's test build, from CI (commit ${GITHUB_SHA:-unknown}).

These three files work from any folder, as long as they stay together:
  windows-spawn-tests$ext   the spike's integration tests (tests/spawn.rs)
  $(basename "$probe")           a helper process the tests start
  $(basename "$gui")              a GUI-subsystem host, like the Tauri app

Checklist step 1, without Rust: in a terminal in this folder, run
  .\\windows-spawn-tests$ext --skip control_
Every test passes and no console window appears. (The control_ tests flash windows on purpose.)

The other steps use gui-host$ext from this folder; the full checklist is in
docs/spikes/windows-spawn.md under "Manual checklist".
EOF

printf 'Staged into %s:\n' "$dest"
ls -l "$dest"
set_output test_exe "$staged"
