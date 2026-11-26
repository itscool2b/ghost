#!/usr/bin/env bash
# Generate actual Rust bindings and optionally enforce committed parity. This never edits Git.
# The artifact is copied only after the exporter test positively reports a pass.
set -euo pipefail

mode=${1:-check}
destination=${2:-}
case "$mode" in check | refresh) ;; *) echo "usage: export-bindings.sh check|refresh <artifact folder>" >&2; exit 2 ;; esac
[ -n "$destination" ] || { echo "An artifact folder is required." >&2; exit 2; }

log=$(mktemp)
trap 'rm -f "$log"' EXIT
cargo test --locked -p ghost-desktop --lib export_bindings -- --color never | tee "$log"
if ! grep -Eq '^test [^ ]*export_bindings[^ ]* \.\.\. ok' "$log"; then
  echo "::error::No passing export_bindings test ran. No generated artifact will be published."
  exit 1
fi
file=apps/desktop/src/bindings.ts
if ! git ls-files --error-unmatch "$file" >/dev/null; then
  echo "::error file=$file::The generated binding path must already be tracked."
  exit 1
fi
mkdir -p "$destination"
cp "$file" "$destination/bindings.ts"
if [ -n "${GITHUB_OUTPUT:-}" ]; then printf 'generated=true\n' >>"$GITHUB_OUTPUT"; fi

if [ "$mode" = check ]; then
  if ! git diff --exit-code -- "$file"; then
    echo "::error file=$file::bindings.ts is out of date. Generate and commit the exact Rust output."
    exit 1
  fi
else
  echo "::notice::Binding refresh requested. Download the generated artifact and commit it before final default-mode verification."
fi
