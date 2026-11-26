# Ghost desktop

The Tauri v2 app: the Rust shell in `src-tauri/` and the React UI in `src/`. Run these from the
repository root:

| Command | What it does |
|---|---|
| `pnpm dev` | The app with hot reload (Vite for the UI, Cargo for the shell). |
| `pnpm bundle` | Release bundles (`tauri build`), signed on macOS as described below. Extra arguments go to `tauri build`. |
| `pnpm signing create` / `pnpm signing status` | The local macOS signing identity (below). |
| `pnpm --filter @ghost/desktop host-upgrade <command>` | Windows: installs a local build over the installed app with a snapshot to roll back to, also from a chat inside the app (`install --after-exit`). See [docs/developing-in-ghost.md](../../docs/developing-in-ghost.md). |
| `pnpm --filter @ghost/desktop test` | UI tests (Vitest). |
| `cargo test -p ghost-desktop` | Shell tests; also regenerates `src/bindings.ts`. |

## Linux

On Debian and Ubuntu, install the system libraries Tauri and dictation build against, then use
the commands above:

```sh
sudo apt-get install -y build-essential clang libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev libxdo-dev patchelf rpm \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-gl gstreamer1.0-alsa \
  gstreamer1.0-pulseaudio
```

`pnpm bundle --bundles deb,rpm,appimage` makes all three packages. A package runs on the
distribution it was built on and newer ones, since it links that system's glibc: the release
packages come from Ubuntu 22.04 (`.github/workflows/build.yml`). The deb and rpm require the
AppIndicator, ALSA and OpenSSL libraries the binary uses, and GStreamer's base and good plugins,
which the browser's pages play sound and video with. The AppImage carries those plugins itself,
staged from the build machine; the bundle stops and names the packages when they are missing, and
`--without-media` builds an image without them that uses the system's
(`docs/developing-in-ghost.md`, Linux).

- The window is opaque: Linux has no glass material behind windows.
- Sound and video: where GStreamer's base and good plugins are missing, pages start with them off
  rather than crash, and Settings > General > Sounds names the packages to install.
- The tray is an AppIndicator. Without its library Ghost runs without a tray, and closing the
  window quits. A desktop that shows no AppIndicators (Fedora's GNOME without the AppIndicator
  extension) still hides the window on close; opening Ghost again brings it back, and turning
  off Settings > General > Keep running in the tray makes closing quit.
- Notifications need a desktop notification service, and Keep awake asks systemd's login manager.
- Added agent accounts keep their tokens in the desktop keyring: any Secret Service, such as GNOME
  Keyring or KWallet. Without one, accounts cannot be added.
- Opening files finds Visual Studio Code, its Insiders build and Cursor from their packages and Zed
  from its install script; JetBrains IDEs stay manual there.
- The updater works as elsewhere once a release build carries the update key: an AppImage replaces
  its own file, and a deb or rpm installs through `pkexec`, which asks for an administrator's
  password.

## Signing on macOS

### Why

macOS stores privacy grants, such as access to `~/Desktop` where your projects live, against an
app's *designated requirement*. A build with the default ad-hoc signature has the requirement
`cdhash H"…"`, the hash of that exact build, which no other build satisfies: after every rebuild
macOS treats Ghost as a new app, asks for access again, and leaves the old entries behind in
System Settings. Signed with one stable identity, every build has the requirement
`identifier "dev.onghost.desktop" and certificate root = H"<certificate SHA-1>"`, so grants carry
over from build to build.

The identity is a self-signed certificate named **auracode Local Signing**, kept in your login
keychain. It is for builds on your own Mac; distributing Ghost to other people needs an Apple
Developer ID and notarization, which come with releases (M5).

### Create it (once per Mac)

```sh
pnpm signing create
```

This generates an RSA 2048 key and a code-signing certificate valid for ten years, and imports
both into the login keychain, with `codesign` allowed to use the key. It prints the certificate's
SHA-1 and expiry date. It never replaces an identity that already exists. Nothing leaves your Mac:
the private key exists outside the keychain only in a private temporary folder that is deleted
right after the import.

The trade-off: `codesign` uses the key without asking, and any program running under your account
can run `codesign`. So code on your account could sign itself as this identity and inherit the folder
access you granted to local Ghost builds, which a per-build ad-hoc signature never allows. That is
the usual price of a local signing identity. Keep it in mind before granting a local build broad
access such as Full Disk Access, and rotate the identity (below) if you suspect misuse. Released
builds never use it (CI signs them ad hoc until Ghost has an Apple Developer ID), so it cannot
impersonate them.

`security find-identity -v -p codesigning` keeps reporting `0 valid identities`. "Valid" there
means vouched for by a certificate authority, which a self-signed certificate never is, and
`codesign` does not need it. `pnpm signing status`, or the same command without `-v`, shows the
identity (marked `CSSMERR_TP_NOT_TRUSTED`, which is expected).

### How builds use it

- **Bundles:** `pnpm bundle` passes the identity's SHA-1 to `tauri build` as
  `APPLE_SIGNING_IDENTITY`. On a Mac without the identity it signs ad hoc (`-`, as CI does) and
  warns, so the build still works. If you set `APPLE_SIGNING_IDENTITY` yourself, it wins. Check a
  bundle with `codesign -d -r- target/release/bundle/macos/Ghost.app`.
- **Dev builds:** `src-tauri/.cargo/config.toml` makes `scripts/macos-signing.sh` Cargo's runner on
  macOS, so `pnpm dev` signs `target/debug/auracode` with the identity (and the bundle's
  identifier) just before it starts. Without the identity the build runs as before, with a hint.
  Note that macOS charges a program started from a terminal to that terminal, so while Ghost
  runs under `pnpm dev`, folder-access prompts name your terminal app.
- **CI** builds sign ad hoc (`APPLE_SIGNING_IDENTITY=-` in `.github/workflows/build.yml`) and never
  read the runner config, which Cargo only loads for commands run inside `src-tauri/`.
- **Windows and Linux** need nothing: the scripts only sign on macOS.

To keep the identity in another keychain, set `GHOST_SIGNING_KEYCHAIN` to its path for
`create`, `status` and builds. `codesign` only finds identities in keychains on your search list
(`security list-keychains`), which the login keychain always is.

### Back it up

Losing the identity means a new one, and a new one makes macOS ask for Ghost's permissions
again. To back it up:

1. Open Keychain Access, choose the **login** keychain and the **My Certificates** tab.
2. Select **auracode Local Signing** (its private key, named "Imported Private Key", is listed
   under it) and choose **File > Export Items…** as a `.p12` file with a strong password.
3. Keep the file and its password in your password manager.

To restore it on a new or reinstalled Mac:

```sh
security import auracode-signing.p12 -f pkcs12 -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
pnpm signing status
```

A restored identity has the same SHA-1, so existing grants stay valid.

### Rotate it

Rotate when the certificate nears its expiry date (`pnpm signing status`) or if its key may have
leaked:

```sh
pnpm signing status                                   # note the SHA-1
security delete-identity -Z <SHA-1> ~/Library/Keychains/login.keychain-db
pnpm signing create
```

The next build has a new designated requirement, so macOS asks for folder access once more. Clear
the stale grant with `tccutil reset SystemPolicyDesktopFolder dev.onghost.desktop`, or in System
Settings > Privacy & Security > Files and Folders.

### Troubleshooting

- **"N signing identities are named auracode Local Signing":** builds refuse to guess between them
  (dev builds run unsigned, `pnpm bundle` stops). Keep one and delete the others with
  `security delete-identity -Z <SHA-1>`.
- **"codesign wants to sign using key Imported Private Key":** the key's access list was changed
  (for example by restoring it without `-T /usr/bin/codesign`). Choose **Always Allow**.
- **"could not sign this dev build":** the run continues unsigned; the message carries `codesign`'s
  reason, usually a locked keychain.
