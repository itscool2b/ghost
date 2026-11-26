#!/usr/bin/env bash
# Build both revisions from source and compare identical fixtures on one native runner.
set -euo pipefail

die() {
  printf 'checkpoint benchmark: %s\n' "$*" >&2
  exit 1
}

native_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s\n' "$1"
  fi
}

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd "$here/../.." && pwd -P)"
cd "$repo"

counts=${GHOST_CHECKPOINT_BENCH_COUNTS:-1000,10000,20000}
warm=${GHOST_CHECKPOINT_BENCH_WARM:-1}
baseline_ref=${CHECKPOINT_BASELINE_REF:-af499cd05684df8d2f08e48af21d268b522862f7}
[[ $counts =~ ^[1-9][0-9]*(,[1-9][0-9]*){0,2}$ ]] ||
  die 'fixture sizes must be one to three comma-separated positive integers'
IFS=, read -r -a sizes <<< "$counts"
for size in "${sizes[@]}"; do
  if [[ ${#size} -gt 5 ]] || ((size > 20000)); then
    die 'fixture sizes must not exceed 20000'
  fi
done
[[ $warm =~ ^([1-9]|10)$ ]] || die 'unchanged sample count must be from 1 to 10'
[[ -n ${RUNNER_TEMP:-} ]] || die 'RUNNER_TEMP is required for isolated baseline files and evidence'

temp_root=$RUNNER_TEMP
if command -v cygpath >/dev/null 2>&1; then
  temp_root=$(cygpath -u "$temp_root")
fi
temp_root="$(cd "$temp_root" && pwd -P)"
case "$temp_root/" in
  "$repo/"*) die 'RUNNER_TEMP must be outside the measured checkout' ;;
esac
[[ -z $(git status --porcelain=v1 --untracked-files=normal) ]] ||
  die 'the measured checkout must be clean before preparing the comparison'
git check-ignore --quiet -- target/ || die 'the current build directory must be ignored by the measured checkout'
baseline_commit=$(git rev-parse --verify --end-of-options "${baseline_ref}^{commit}") ||
  die 'the baseline must resolve to a commit in the fetched history'
current_commit=$(git rev-parse HEAD)

run_dir=$(mktemp -d "$temp_root/checkpoint-performance.XXXXXX")
baseline_dir="$run_dir/baseline"
evidence="$temp_root/checkpoint-performance-evidence"
mkdir "$evidence"
git worktree add --detach "$baseline_dir" "$baseline_commit"

core=crates/ghost-core/src/checkpoints
session=crates/ghost-core/src/session/tests
[[ -f "$baseline_dir/$core/mod.rs" && -f "$baseline_dir/$session/checkpoints.rs" ]] ||
  die 'the baseline does not support these checkpoint benchmark fixtures'
cp "$repo/$core/benchmarks.rs" "$baseline_dir/$core/benchmarks.rs"
if ! grep -Eq '^[[:space:]]*mod benchmarks;' "$baseline_dir/$core/mod.rs"; then
  printf '\n#[cfg(test)]\nmod benchmarks;\n' >> "$baseline_dir/$core/mod.rs"
fi
cp "$repo/$session/checkpoint_benchmarks.rs" "$baseline_dir/$session/checkpoint_benchmarks.rs"
if grep -Eq '^[[:space:]]*mod benchmarks;' "$baseline_dir/$session/checkpoints.rs"; then
  grep -Fq '#[path = "checkpoint_benchmarks.rs"]' "$baseline_dir/$session/checkpoints.rs" ||
    die 'the baseline has a different native checkpoint benchmark module'
else
  printf '\n#[path = "checkpoint_benchmarks.rs"]\nmod benchmarks;\n' >> "$baseline_dir/$session/checkpoints.rs"
fi

# Cargo can reuse a same-named path package's test binary across worktrees when they share a
# target directory. Keep the baseline's artifacts separate from the already verified current
# build. Neither copied binaries nor shared path-package fingerprints establish provenance.
baseline_target=$(native_path "$run_dir/baseline-target")
current_target=$(native_path "$repo/target")
[[ $baseline_target != "$current_target" ]] || die 'comparison revisions must not share build artifacts'
export CARGO_TARGET_DIR
export GHOST_CHECKPOINT_BENCH_WORKSPACE
GHOST_CHECKPOINT_BENCH_WORKSPACE=$(native_path "$repo")
export GHOST_CHECKPOINT_BENCH_COUNTS=$counts
export GHOST_CHECKPOINT_BENCH_WARM=$warm

{
  printf 'baseline_commit=%s\ncurrent_commit=%s\n' "$baseline_commit" "$current_commit"
  printf 'baseline_target=%s\ncurrent_target=%s\n' "$baseline_target" "$current_target"
  printf 'counts=%s\nwarm_samples=%s\nbuild_profile=test\nfeatures=default\n' "$counts" "$warm"
  printf 'started_at_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'core_fixture_blob=%s\n' "$(git hash-object "$core/benchmarks.rs")"
  printf 'native_fixture_blob=%s\n' "$(git hash-object "$session/checkpoint_benchmarks.rs")"
  git --version
  cargo --version
  rustc -Vv
  uname -srv
} > "$evidence/environment.txt"

verify_binary() {
  local revision=$1 directory=$2
  local log="$evidence/$revision-test-list.log"
  local list="$evidence/$revision-test-list.txt"
  local sentinel_name=binary_batch_framing_rejects_corruption_and_accepts_missing
  local sentinel="checkpoints::cache::tests::$sentinel_name: test"
  local expected=absent observed=absent
  if [[ -f "$directory/$core/cache.rs" ]] &&
    grep -Fq "fn $sentinel_name(" "$directory/$core/cache.rs"; then
    expected=present
  fi
  (
    cd "$directory"
    cargo test -p ghost-core --lib --locked -- --list
  ) 2>&1 | tee "$log"
  tr -d '\r' < "$log" > "$list"
  for fixture in \
    checkpoints::benchmarks::checkpoint_workspace_performance \
    checkpoints::benchmarks::checkpoint_performance \
    session::tests::checkpoints::benchmarks::checkpoint_prompt_performance; do
    grep -Fxq "$fixture: test" "$list" || die "$revision binary is missing $fixture"
  done
  if grep -Fxq "$sentinel" "$list"; then observed=present; fi
  [[ $observed == "$expected" ]] ||
    die "$revision binary's checkpoint tests do not match that revision's source"
  printf '%s cache_sentinel_expected=%s cache_sentinel_observed=%s target=%s\n' \
    "$revision" "$expected" "$observed" "$CARGO_TARGET_DIR" >> "$evidence/provenance.txt"
}

measure() {
  local revision=$1 directory=$2 fixture=$3 marker=$4
  local log="$evidence/$revision-$fixture.log"
  (
    cd "$directory"
    cargo test -p ghost-core --lib --locked "$fixture" -- \
      --ignored --nocapture --test-threads=1
  ) 2>&1 | tee "$log"
  grep -Eq 'test result: ok[.] 1 passed; 0 failed;' "$log" ||
    die "$revision did not run exactly one successful $fixture test"
  grep -Fq "$marker" "$log" || die "$revision produced no $fixture measurements"
  awk -v revision="$revision" '/CHECKPOINT_/ { sub(/^.*CHECKPOINT_/, "CHECKPOINT_"); print revision " " $0 }' \
    "$log" >> "$evidence/measurements.txt"
}

# Builds finish before each test begins. Never run another build or measurement concurrently on
# this runner, and do not write evidence or baseline sources inside the measured checkout.
for revision in baseline current; do
  directory=$repo
  CARGO_TARGET_DIR=$current_target
  if [[ $revision == baseline ]]; then
    directory=$baseline_dir
    CARGO_TARGET_DIR=$baseline_target
  fi
  verify_binary "$revision" "$directory"
  measure "$revision" "$directory" checkpoint_workspace_performance 'CHECKPOINT_WORKSPACE storage=local'
  measure "$revision" "$directory" checkpoint_prompt_performance 'CHECKPOINT_PROMPT files=1000'
  measure "$revision" "$directory" checkpoint_performance 'CHECKPOINT_BENCH storage=local'
done

[[ -z $(git status --porcelain=v1 --untracked-files=normal) ]] ||
  die 'the measured checkout changed during the comparison'
printf 'completed_at_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$evidence/environment.txt"
