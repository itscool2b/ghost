#!/bin/sh
# Local code signing for Ghost's macOS builds.
#
# macOS stores privacy grants (access to ~/Desktop, where projects live) against an app's
# designated requirement. With the default ad-hoc signature that requirement is the build's own
# hash, which no other build matches, so every rebuild is a new app to macOS: access is asked for
# again and stale entries pile up in System Settings. Signed with one stable self-signed identity,
# every build satisfies the same requirement (its identifier and the certificate's hash). This
# script creates that identity and signs with it: dev builds through Cargo's runner
# (../src-tauri/.cargo/config.toml) and bundles through `pnpm bundle` (bundle.mjs). Machines
# without the identity still build; they get the ad-hoc signature and a warning. See ../README.md.
#
# Usage:
#   macos-signing.sh create      create the identity in the login keychain (once per Mac)
#   macos-signing.sh status      show the identity builds will use
#   macos-signing.sh identity    print it (exit 1: none, 2: ambiguous); used by bundle.mjs
#   macos-signing.sh run PROGRAM [ARGS...]
#                                Cargo's runner: sign PROGRAM if it is the app, then run it
#
# Environment:
#   APPLE_SIGNING_IDENTITY     sign with this identity instead (Tauri's own variable, so one
#                              setting covers dev runs and bundles; `-` means ad hoc)
#   GHOST_SIGNING_KEYCHAIN  create and look for the identity in this keychain instead of the
#                              default one; codesign only finds it if the keychain is on the
#                              user's search list (`security list-keychains`)

set -eu

NAME="Ghost Local Signing"
# Ten years: the certificate is only ever checked by its hash (a code signature's designated
# requirement names it), so a long life just postpones rotation.
VALID_DAYS=3650

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
conf="$here/../src-tauri/tauri.conf.json"

say() {
  printf 'Ghost: %s\n' "$*" >&2
}

die() {
  say "$*"
  exit 1
}

# Keychain arguments for `security` and `codesign`: none unless GHOST_SIGNING_KEYCHAIN is set.
keychain_args() {
  if [ -n "${GHOST_SIGNING_KEYCHAIN:-}" ]; then
    printf '%s\n' "$GHOST_SIGNING_KEYCHAIN"
  fi
}

# Prints the SHA-1 of the one identity named $NAME. Exit status: 0 found, 1 none (or no
# `security` tool), 2 more than one. A hash, not the name, so that codesign never has to choose
# between two certificates with the same name.
lookup_keychain() {
  command -v security >/dev/null 2>&1 || return 1
  kc=$(keychain_args)
  if [ -n "$kc" ]; then
    listing=$(security find-identity -p codesigning "$kc" 2>/dev/null) || return 1
  else
    listing=$(security find-identity -p codesigning 2>/dev/null) || return 1
  fi
  # `find-identity` lists every matching identity, then repeats the valid ones under "Valid
  # identities only". A self-signed certificate is never valid there (nothing vouches for it),
  # which does not matter to codesign, so only the first list counts.
  hashes=$(printf '%s\n' "$listing" | awk -v name="\"$NAME\"" '
    /Valid identities only/ { exit }
    index($0, name) { print $2 }
  ')
  count=$(printf '%s\n' "$hashes" | grep -c . || true)
  case $count in
  0) return 1 ;;
  1)
    printf '%s\n' "$hashes"
    return 0
    ;;
  *)
    say "$count signing identities are named \"$NAME\" ($(printf '%s' "$hashes" | tr '\n' ' '))."
    say "Keep one: delete the others with 'security delete-identity -Z <SHA-1>' (see apps/desktop/README.md)."
    return 2
    ;;
  esac
}

# The identity to sign with: APPLE_SIGNING_IDENTITY when set, else the keychain's. Same exit
# status as lookup_keychain.
find_identity() {
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    printf '%s\n' "$APPLE_SIGNING_IDENTITY"
    return 0
  fi
  lookup_keychain
}

# The bundle identifier from tauri.conf.json, so a dev build's designated requirement names the
# same app as the bundle's (a bare binary is otherwise identified by its file name).
bundle_identifier() {
  if command -v plutil >/dev/null 2>&1 &&
    plutil -extract identifier raw -o - "$conf" 2>/dev/null; then
    return 0
  fi
  sed -n 's/^[[:space:]]*"identifier":[[:space:]]*"\([^"]*\)".*/\1/p' "$conf" | head -n 1
}

# Signs a dev build before it runs. Never stops the run: a build that cannot be signed still
# starts, with a warning (macOS then treats it as a new app, as it did before this script).
sign_dev_build() {
  program=$1
  found=0
  identity=$(find_identity) || found=$?
  case $found in
  0) ;;
  1)
    say "no \"$NAME\" signing identity on this Mac, so this dev build is signed ad hoc and macOS"
    say "treats every rebuild as a new app. Create it once with: pnpm signing create"
    return 0
    ;;
  *)
    say "running this dev build without the local signature."
    return 0
    ;;
  esac
  id=$(bundle_identifier)
  kc=$(keychain_args)
  if [ -n "$kc" ]; then
    set -- --force --sign "$identity" --identifier "$id" --keychain "$kc"
  else
    set -- --force --sign "$identity" --identifier "$id"
  fi
  if ! output=$(codesign "$@" --entitlements "$here/../src-tauri/Entitlements.plist" "$program" 2>&1); then
    say "could not sign this dev build ($output); running it anyway."
  fi
  return 0
}

run() {
  [ $# -ge 1 ] || {
    say "usage: macos-signing.sh run PROGRAM [ARGS...]"
    exit 64
  }
  # Only the app itself: Cargo uses the same runner for test binaries (deps/<name>-<hash>), which
  # gain nothing from a signature.
  if [ "$(basename -- "$1")" = ghost ]; then
    sign_dev_build "$1"
  fi
  exec "$@"
}

require_macos() {
  [ "$(uname -s)" = Darwin ] || die "local signing identities are only needed on macOS."
  for tool in security openssl codesign; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is missing."
  done
}

print_identity() {
  hash=$1
  kc=$(keychain_args)
  if [ -n "$kc" ]; then
    listing=$(security find-certificate -Z -a -c "$NAME" -p "$kc" 2>/dev/null || true)
  else
    listing=$(security find-certificate -Z -a -c "$NAME" -p 2>/dev/null || true)
  fi
  # Each certificate comes as "SHA-256 hash: ...", "SHA-1 hash: ...", then its PEM: take the PEM
  # of the one whose hash matches, not merely the first with the name.
  pem=$(printf '%s\n' "$listing" | awk -v want="SHA-1 hash: $hash" '
    /^SHA-256 hash:/ { take = 0 }
    take { print }
    $0 == want { take = 1 }
  ')
  expires=$(printf '%s\n' "$pem" | openssl x509 -noout -enddate 2>/dev/null | sed 's/^notAfter=//' || true)
  printf 'Signing identity: "%s"\n' "$NAME"
  printf '  SHA-1:   %s\n' "$hash"
  printf '  Expires: %s\n' "${expires:-unknown}"
}

create() {
  require_macos
  found=0
  existing=$(lookup_keychain) || found=$?
  case $found in
  0)
    print_identity "$existing"
    die "the identity already exists; builds use it. To replace it, see 'Rotate it' in apps/desktop/README.md."
    ;;
  1) ;;
  *) exit 2 ;;
  esac

  work=$(mktemp -d "${TMPDIR:-/tmp}/ghost-signing.XXXXXX")
  trap 'rm -rf "$work"' EXIT
  trap 'exit 130' HUP INT TERM
  # The private key exists outside the keychain only here, readable by this user alone, until
  # the trap removes it.
  umask 077
  cat >"$work/openssl.cnf" <<EOF
[ req ]
distinguished_name = dn
prompt = no
[ dn ]
CN = $NAME
O = Ghost local builds
[ codesign ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF
  if ! openssl req -x509 -newkey rsa:2048 -sha256 -days "$VALID_DAYS" -nodes \
    -config "$work/openssl.cnf" -extensions codesign \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>"$work/openssl.log"; then
    die "openssl could not create the certificate: $(cat "$work/openssl.log")"
  fi
  cat "$work/key.pem" "$work/cert.pem" >"$work/identity.pem"

  kc=$(keychain_args)
  # -T lets codesign use the key without asking each time.
  if [ -n "$kc" ]; then
    set -- -k "$kc"
  else
    set --
  fi
  if ! security import "$work/identity.pem" "$@" -f pemseq -t agg -T /usr/bin/codesign >/dev/null; then
    die "the keychain refused the new identity; nothing was changed."
  fi
  rm -rf "$work"

  hash=$(lookup_keychain) || die "the identity was imported but cannot be found again; run 'pnpm signing status'."
  print_identity "$hash"
  cat <<EOF

Dev builds (pnpm dev) and bundles (pnpm bundle) are now signed with it. Back it up (see
apps/desktop/README.md): losing it changes the app's identity, so macOS asks for its
permissions again.
EOF
}

show_status() {
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    printf 'APPLE_SIGNING_IDENTITY is set: builds are signed with "%s".\n' "$APPLE_SIGNING_IDENTITY"
  fi
  result=0
  hash=$(lookup_keychain) || result=$?
  case $result in
  0) print_identity "$hash" ;;
  1)
    printf 'No "%s" identity in the keychain: builds are signed ad hoc.\n' "$NAME"
    printf 'Create it once with: pnpm signing create\n'
    ;;
  *) exit 2 ;;
  esac
}

command=${1:-}
[ $# -gt 0 ] && shift
case $command in
create) create ;;
status) show_status ;;
identity) find_identity ;;
run) run "$@" ;;
*)
  say "usage: macos-signing.sh create | status | identity | run PROGRAM [ARGS...]"
  exit 64
  ;;
esac
