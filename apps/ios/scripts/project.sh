#!/usr/bin/env bash
# Generates apps/ios/Ghost.xcodeproj from project.yml with XcodeGen 2.45.4.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
want="2.45.4"
have="$(xcodegen --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$have" != "$want" ]; then
  echo "XcodeGen $want is needed (found ${have:-none}). On a Mac: brew install xcodegen" >&2
  exit 1
fi
cd "$here"
xcodegen generate --spec project.yml --quiet
echo "Generated $here/Ghost.xcodeproj"
