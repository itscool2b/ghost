#!/usr/bin/env bash
# Puts XcodeGen 2.45.4 on the PATH for the iPhone app's workflow, checked against its pinned size
# and SHA-256 before it is unpacked. GitHub's Macs have no XcodeGen of their own.
set -euo pipefail
version="2.45.4"
sha="090ec29491aad50aec10631bf6e62253fed733c50f3aab0f5ffc86bc170bdbef"
size=4319508
folder="${RUNNER_TEMP:-/tmp}/xcodegen"
mkdir -p "$folder"
curl --proto '=https' --tlsv1.2 -sSfL -o "$folder/xcodegen.zip" \
  "https://github.com/yonaskolb/XcodeGen/releases/download/$version/xcodegen.zip"
test "$(stat -f%z "$folder/xcodegen.zip" 2>/dev/null || stat -c%s "$folder/xcodegen.zip")" = "$size"
echo "$sha  $folder/xcodegen.zip" | shasum -a 256 -c -
unzip -q -o "$folder/xcodegen.zip" -d "$folder"
echo "$folder/xcodegen/bin" >> "${GITHUB_PATH:-/dev/null}"
"$folder/xcodegen/bin/xcodegen" --version
