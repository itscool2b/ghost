#!/bin/sh
# Builds tests/ios-app for the iPhone simulator, with Xcode's compiler and no project files:
#   build.sh <out folder>
# and prints the .app's path. Used by the "iPhone simulator" workflow before tests/ios.rs.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$1
app="$out/HelloGhost.app"
rm -rf "$app"
mkdir -p "$app"
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
# The simulator runs the Mac's own architecture.
arch=$(uname -m)
xcrun --sdk iphonesimulator clang \
  -target "$arch-apple-ios17.0-simulator" \
  -isysroot "$sdk" \
  -fobjc-arc \
  -framework UIKit -framework Foundation \
  "$here/main.m" -o "$app/HelloGhost"
cp "$here/Info.plist" "$app/Info.plist"
codesign --force --sign - "$app" >/dev/null
echo "$app"
