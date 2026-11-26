# Windows, macOS and Linux packaging

Two workflows build installers. `Publish` runs when the owner approves a build and puts a
version in front of people: see [Published builds and in-app updates](#published-builds-and-in-app-updates).
`Build` is the manual one below, for trying a branch: its installers are never published and do
not update themselves. No publisher signs either kind for the operating systems (the owner chose
to stay unsigned on 2026-11-06).

The manual `Build` workflow produces unsigned acceptance artifacts for four explicit targets:

| Target | Installer | Native acceptance still required |
|---|---|---|
| `universal-apple-darwin` | Universal DMG | Apple Silicon and Intel installation, launch and upgrade |
| `x86_64-pc-windows-msvc` | x64 NSIS | Windows x64 install, upgrade and uninstall |
| `aarch64-pc-windows-msvc` | arm64 NSIS | Windows arm64 install, upgrade and uninstall |
| `x86_64-unknown-linux-gnu` | deb, rpm and AppImage, built on Ubuntu 22.04 | Debian or Ubuntu, Fedora and AppImage install, launch and upgrade |

The Windows arm64 installer is cross-compiled on the Windows runner. This verifies compilation,
not execution on an arm64 device. The runner needs the MSVC arm64 build tools. Both macOS Rust
targets are installed before the universal build. The Linux packages link Ubuntu 22.04's glibc, so
they run there and on newer distributions; the runner installs WebKitGTK 4.1, the AppIndicator,
librsvg, ALSA, libxdo, patchelf and rpm first. WSL agent hosts are separate from these packages.

These artifacts are labeled `ghost-unsigned-<target>-<SHA>`, expire after seven days, and are
never published automatically. macOS uses an explicit ad-hoc signature. Neither an ad-hoc signature
nor an unsigned Windows installer establishes a trusted public release. No platform security
settings are changed to make a development installer run.

`pnpm bundle` forwards the requested Tauri target and formats and remaps source paths from the
build home, Cargo registry, Rust toolchain and checkout. It preserves existing encoded Rust flags
and uses encoded arguments so Windows paths containing spaces remain intact. This is build tooling;
runtime update and release validation logic remain Rust. Existing macOS local-signing behavior is
preserved, while CI explicitly selects its ad-hoc identity.

## Published builds and in-app updates

A build is published only when the owner approves one (owner, 23 November 2026: "dont make a
build unless i approve a build ... money gets wasted"). Merges collect on main in between; the
approved build puts everything merged since the last one in front of people, and every
installed copy offers it as an update. (From 19 November 2026 until then every merge that changed
the app was built and published by itself.)

### How main reaches people

1. **`Publish` is started by hand** (`.github/workflows/publish.yml`, `gh workflow run publish.yml
   --ref main`), only after the owner approves a build. A merge starts nothing. One run at a time.
   It builds main as it is, so `VERSION` is whatever the merged pull requests raised it to: the
   versions in between are never published, which is fine.
2. **The build is numbered by `VERSION`**, three numbers the pull request that changed the
   app chose (owner, 23 November 2026): the minor number up for a big feature, the patch for a
   small change (docs/developing-in-ghost.md, "Version"). It must be newer than every published
   version, or installed copies would never be offered it; the workflow refuses one that is not.
   A version already published is skipped when nothing in the app changed since it (a rerun, or
   release tooling alone) and refused when the app did change: raise `VERSION` in a pull request
   and its merge publishes. `pnpm bundle --app-version` gives the number to Tauri; the workspace
   version in `Cargo.toml` stays as it is. People see the number as it is, "Ghost 0.2.1".
   Until 23 November 2026 the number was `0.1.N`, N the count of commits on main (the last was
   0.1.1787); the first chosen version, 0.2.0, is newer than all of those.
3. **Four targets build with their updater signatures** (`pnpm bundle --signed-updater`): the
   universal macOS app and DMG, Windows x64 and ARM64 NSIS installers, and the Linux AppImage, deb
   and rpm. macOS is signed ad hoc. No publisher signs the installers for the operating systems.
4. **`ghost-release prepare` and `verify` check every byte.** They refuse a missing or doubled
   package, a package without its signature, a signature that does not verify against the trusted
   public key or is bound to another version, and a package built as another version. They write
   `latest.json` (what installed copies read), `downloads.json` (what the download page reads)
   and `SHA256SUMS`.
5. **The files become the GitHub release `v<VERSION>`**, marked the latest. The workflow then fetches
   `releases/latest/download/latest.json`, the address compiled into the app, and requires it to
   be this version's manifest byte for byte.
6. **The download page follows**: the workflow commits `downloads.json` as
   `apps/web/app/release-catalog.json` to main, and the site redeploys. A push made with the
   workflow's own token starts no workflow, so it cannot publish again.
7. **The newest fifteen versions are kept.** Older `v` releases and their tags are deleted; a copy
   that old updates straight to the newest. Hand-made releases (`build-…`) are left alone.

What is new in a version is taken from `CHANGELOG.md`: the headlines of the lines added since the
last published version (`.github/scripts/release-notes.mjs`). The date people see is the day it
was built in US Eastern time.

### Who is trusted with what

- **The updater signature decides what gets installed.** It is made in the workflow's build step
  with the private key in the repository's secrets, and checked by each installed copy against
  the public key compiled into it, over the exact bytes downloaded and bound to the version the
  manifest named. GitHub, a network in between, or anyone who could edit a release cannot make an
  installed copy accept other bytes, an older version, or another product's package.
- **Whoever holds the private key can sign an update**, and whoever can also publish a release in
  this repository can deliver it. So the key lives only in GitHub's secret store and in the
  owner's own backup; it is never in the repository, a log or a command line. The workflow runs
  only for pushes to main and manual runs by people with write access. A pull request, from a
  fork or not, never runs it.
- **The build runs third-party code** (crates, npm packages, pinned actions) in the step that
  holds the key. A malicious dependency could read it there. Dependencies are locked, actions are
  pinned to commits, and the cache is written only by main.
- **No publisher vouches for the installers to the operating system.** Windows and macOS warn
  before a downloaded installer runs, and the download page says so and how to open it. An update
  the app downloads itself is not stopped by those warnings; Smart App Control, where it is on,
  can still block an unsigned installer.
- **Losing the private key ends updates** for every installed copy: a new key means a new public
  key, which old copies do not trust. They would have to be reinstalled by hand.

### One-time setup (done 19 November 2026)

| Where | Name | What |
|---|---|---|
| Repository variable | `GHOST_UPDATER_PUBLIC_KEY` | The updater's public key, as `tauri signer generate` wrote it |
| Repository secret | `TAURI_SIGNING_PRIVATE_KEY` | The updater's private key |
| Repository secret | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Its password |

A publisher's signature can be added later in the same workflow (Developer ID and notarization on
macOS, Authenticode on Windows) and the plan's `publisher_signed` set, which drops the download
page's warnings. The workflow that did both for a manual release was removed with this change; it
is in the history (`.github/workflows/release.yml` before #148).

### What the app does

`apps/desktop/src-tauri/src/updater.rs`. Builds without a compiled-in public key (a development
build, an old preview build) report `disabled` and say to install from the download page.

- **Checking is automatic.** 45 seconds after start and every four hours, the app reads
  `https://github.com/itscool2b/ghost/releases/latest/download/latest.json`: at most 64 KiB,
  HTTPS only. Settings, General, "Check for updates" turns this off; "Check for updates" in
  Settings, About still works then. A check the app made by itself that fails leaves no trace, and
  none runs over a download or an installation.
- **Redirects are followed only to GitHub's own asset hosts** (`github.com`,
  `objects.githubusercontent.com`, `release-assets.githubusercontent.com`), over HTTPS, five at
  most. A package address in the manifest must be
  `https://github.com/itscool2b/ghost/releases/download/v<version>/<file>` with plain names and
  nothing else: no other repository, no query, no encoded path.
- **Update shows at the foot of the sidebar** when a newer version is found. Its panel has the
  version, what is new, what the restart does to work under way, and one button, Update and
  restart: it downloads at most 512 MiB, verifies the signature and the signed version, and
  installs.
- **Installing** stops every chat's agent and closes terminals, saves drafts, confirms the store
  is closed, then verifies again and installs. It never offers to discard unsaved work. It waits,
  and says so, while a Git change or an agent's own update runs. Windows exits into the installer;
  macOS and Linux restart. On Linux an AppImage replaces its own file and a deb or rpm installs
  through `pkexec`.
- **No updater permission is granted to the webview.** Every decision, request, bound and check is
  in Rust.

### Trying it without installing anything

A development build never installs an update. Started with both
`GHOST_UPDATE_TEST_MANIFEST=http://127.0.0.1:<port>/latest.json` and
`GHOST_UPDATE_TEST_PUBLIC_KEY=<a test key>`, it reads that loopback server instead of GitHub,
downloads the package it names from the same port, and verifies it against that key; Update and
restart then says the package was verified and not installed. Any other address, or either
variable alone, leaves updates off. A released build ignores both variables.

`apps/desktop/scripts/check-app-update.mjs` checks the button and its panel in Chromium and WebKit
against a synthetic updater.

### Staging

A build made with `GHOST_UPDATER_CHANNEL=staging` reads
`releases/download/staging/latest.json` and accepts packages only from `staging-v…` releases; a
stable build accepts only `v…` ones. Nothing publishes to staging today.

## The release tool

`ghost-release` (`crates/ghost-release`) is offline and fail-closed: no network, no private key, no
platform signing. It accepts an explicit plan and a directory containing exactly the seven named
package roles: a universal DMG, a universal `.app.tar.gz` updater, Windows x64 NSIS, Windows ARM64
NSIS, and the Linux x64 AppImage, deb and rpm. The six updater packages (all but the DMG) need
their Tauri `.sig` files. The tool cannot tell whether a file's declared architecture matches the
executable inside it; installing each package remains a separate check.

```sh
cargo run --locked -p ghost-release -- prepare path/to/release-plan.json path/to/artifacts path/to/new-stage
cargo run --locked -p ghost-release -- verify path/to/new-stage
```

`.github/scripts/release-plan.mjs` writes the plan from a directory of packages;
[the example plan](../crates/ghost-release/release-plan.example.json) shows its shape. Never
include a private key in a plan.

Preparation refuses existing output directories. It bounds each artifact at 512 MiB and metadata at
64 KiB, rejects leaf links and filenames outside the explicit directory, verifies prehashed Minisign
signatures using the supplied public key, and requires the signed version to equal the advertised
version. It streams the exact verified bytes into the new stage. Missing or invalid artifacts leave
`PREPARATION_INCOMPLETE` and no public manifests. A valid stage contains:

- `latest.json`, with both macOS architectures pointing to the universal updater, distinct Windows
  architecture entries including the NSIS-specific target keys, and Linux entries for the AppImage
  (`linux-x86_64-appimage` and the plain `linux-x86_64`, which the updater reads for an AppImage),
  the deb (`linux-x86_64-deb`) and the rpm (`linux-x86_64-rpm`).
- `downloads.json`, the website's catalog: the version, notes, each installer's link, size and
  hash, `signed` (the packages carry updater signatures) and `publisher_signed`.
- `SHA256SUMS` and `releases/<version>/SHA256SUMS`, covering package and detached-signature bytes.
- `releases/<version>/`, with copied packages and signatures: the files of the GitHub release.
- `release-plan.json` and `COMPLETE`, local preparation records that are not public release objects.

`verify` needs `GHOST_UPDATER_PUBLIC_KEY` from the independently trusted release configuration,
the same public key embedded in the application, never one read from the stage being checked. It
reads the staged bytes again and compares every generated manifest and checksum. Artifact opens
refuse leaf symlinks atomically.

The public catalog types are generated by `cargo run -p ghost-release -- export-types`; a Rust
parity test checks the committed TypeScript contract. The website reads the committed
`app/release-catalog.json`; with `GHOST_RELEASE_STAGE` set to a stage directory its build
verifies that stage and reads its catalog instead (`apps/web/tests/release.test.mjs`).

## Final verification commands and evidence

These commands are prepared for the final Windows/macOS integration checkpoint. They were not
dispatched to hosted CI by the release-readiness work. Run with the pinned pnpm and a qualified
native toolchain. Do not change platform policy if an executable is denied.

```sh
pnpm install --frozen-lockfile
node .github/scripts/check-instructions.mjs
node --test .github/scripts/acceptance-gates.test.mjs
cargo fmt --all --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
bash .github/scripts/export-bindings.sh check target/final-bindings
pnpm lint
pnpm typecheck
pnpm test
pnpm --filter @ghost/desktop check:chat
pnpm --filter @ghost/desktop check:settings
pnpm --filter @ghost/desktop check:setup
pnpm --filter @ghost/desktop check:fixtures
pnpm --filter @ghost/desktop exec node scripts/check-setup-actions.mjs
pnpm --filter @ghost/desktop exec node scripts/check-profile-status.mjs
pnpm --filter @ghost/desktop exec node scripts/check-chat-auth.mjs
pnpm --filter @ghost/desktop exec node scripts/check-library.mjs
pnpm --filter @ghost/desktop exec node scripts/check-folder-resolution.mjs
pnpm --filter @ghost/desktop exec node scripts/check-repository-publication.mjs
pnpm --filter @ghost/desktop exec node scripts/check-workspace-removal.mjs
pnpm --filter @ghost/desktop exec node scripts/check-agent-installations.mjs
pnpm --filter @ghost/web test:release
pnpm --filter @ghost/web test:browser
pnpm --filter @ghost/desktop tauri build --debug --no-bundle
```

With the frontend embedded, run the synthetic native app smoke on each target. `GHOST_SMOKE_AGENT`
must be unset. The script uses owned temporary profiles and the built-in demo driver, not a paid
vendor account:

```sh
# macOS
bash .github/scripts/run-smoke.sh target/debug/auracode all
# Windows, from the existing Git Bash runner
bash .github/scripts/run-smoke.sh target/debug/ghost.exe all
# Linux, on a virtual display when there is no desktop session
xvfb-run -a -s "-screen 0 1440x900x24" bash .github/scripts/run-smoke.sh target/debug/auracode all
```

The native harness covers real webview, IPC, actor, transcript, restart and quit paths. It does not
prove vendor compatibility, packaged installation or independent external WebDriver control. Keep
its result JSON and logs. The settings browser gate also runs Git reviews, setup, profile
controls, explicit steering and missing-transcript recovery in Chromium and WebKit. Those use synthetic commands and do not replace native acceptance.

The final CI keeps each browser gate bounded and checks that every desktop acceptance script is
reachable directly or through an aggregator. Leave `refresh_bindings` off for the final gate after
committing exact generated output. The unsigned Build workflow separately refuses stale bindings
before compiling the frontend and does not request updater signing material.

The same Windows Rust job then runs an independent external WebDriver check against its embedded
debug app, using the built-in mock agent and an isolated disposable profile. It pins `tauri-driver`
2.1.0 and downloads Microsoft Edge Driver for the installed WebView2 runtime, validates its Microsoft
signature and version, and records the version/hash evidence. Ambiguous or missing runtimes fail
qualification instead of installing or updating browser software. The test performs real external
clicks and typing, observes output, interrupts a turn and reopens persisted history; it retains bounded
logs, a result report and screenshots without uploading the profile. This follows the official
[Tauri WebDriver setup](https://v2.tauri.app/develop/tests/webdriver/manual-setup/) and Microsoft's
[WebView2 driver requirement](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/webdriver).
The workflow is prepared, but only a successful hosted run qualifies that native gate. macOS remains
covered by the existing native app smoke and WebKit browser tests, with actual device acceptance
tracked separately.

Unsigned package compilation uses the existing manual `Build` matrix. Individual qualified hosts can
use the same explicit targets locally:

```sh
pnpm bundle --target universal-apple-darwin --bundles dmg
pnpm bundle --target x86_64-pc-windows-msvc --bundles nsis
pnpm bundle --target aarch64-pc-windows-msvc --bundles nsis
pnpm bundle --target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage
```

For signed updater qualification add `--signed-updater` with the secret-store environment already
supplied. Do not put signing values in commands or logs. Verify the final packages after code signing,
notarization and stapling, then prepare the stage from those final bytes.

| Gate | Required evidence before calling it complete |
|---|---|
| macOS universal | Inspect both architecture slices, install and launch on Apple Silicon and Intel; verify configured minimum OS, signature, notarization and staple; retain existing profile data across upgrade |
| Windows x64 and ARM64 | Install, launch, upgrade and uninstall on each architecture; verify Authenticode and WebView2 behavior, including missing-runtime behavior; an upgrade retains user data, an uninstall deletes it as [uninstall.md](uninstall.md) describes, and both leave unrelated vendor processes running |
| Actual updater | An older published build finds, downloads and installs a newer one on each system; tampering and wrong-version signatures refuse installation; interrupted download retries; agents, PTYs, setup/Git work and unsaved drafts prevent unsafe install |
| Release route | `releases/latest/download/latest.json` serves the newest version's manifest; stable and staging tags stay separate; each package's bytes match its published checksum |
| Final visuals and power | Real Windows/macOS screenshots, accessibility and keyboard review against the chosen references; unlocked native visible/minimized/background power measurements and warm-launch timing |

What still needs things outside this repository: the owner's backup of the updater key, a
publisher's signing identities if the warnings are ever to go, Windows ARM64 and both Mac
architectures to install on, and an unlocked visible desktop for manual performance and privacy
checks. An update installed by a published build on a real machine, on each system, is still to be
seen: the checks so far cover the app's side against a loopback server and the published files
against their checksums.

The M2-M5 audit's reviewed non-Git initialization, project terminal/editor controls and
capability-qualified extra folder forwarding are implemented and have focused local verification.
Readable chat export and portable folder resolution are also implemented with focused
verification; their final integrated Windows/macOS qualification remains pending. The account
lifecycle that stood here (recovery, passphrase storage, reset sync, device removal, Keep as Local
and Local import) was removed with Ghost's accounts on 16 November 2026.
Exact older-message native truncation/fork and Pi steering remain unsupported where the recorded
protocol cannot safely identify the requested boundary; the visible reviewed handoff keeps the source
conversation intact.

Source references checked for this implementation:
[Tauri Windows installers](https://v2.tauri.app/distribute/windows-installer/),
[universal macOS builds](https://v2.tauri.app/distribute/app-store/),
[updater artifacts and signatures](https://v2.tauri.app/plugin/updater/), and
[Windows code signing](https://tauri.app/distribute/sign/windows/).
