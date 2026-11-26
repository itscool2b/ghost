# Developing Ghost

How the repository is laid out, how to run and check it, and the switches development builds read. The [README](../README.md) is the short version. Working on Ghost from a chat inside the installed app has its own guide, [developing-in-ghost.md](developing-in-ghost.md), and [CONTRIBUTING.md](../CONTRIBUTING.md) says how to send a change.

## Layout

```
crates/ghost-protocol/   shared types: events, frames, IPC and session types (serde, exported to TypeScript)
crates/ghost-process/    agent processes: spawning, shutdown, leftovers, the agents' env, protocol logs
crates/ghost-agents/     the agent catalog, detection, the one-time npm adapter install, launch specs
crates/ghost-core/       device identity, data folder, profiles, the per-profile SQLite store, chat sessions
crates/ghost-drivers/    the driver contract, frame batching, the scripted mock agent; the ACP driver (M1a)
crates/ghost-cli/        dev CLI: probes and smoke-tests agents headlessly, records and replays fixtures
apps/desktop/           Tauri v2 app: Rust shell in src-tauri/, React UI in src/
apps/design-lab/        M1a Design Lab: visual directions rendered from a recorded chat (see its README)
packages/ui/            shared design system: Aura tokens, components and marks (historical studies kept for the lab)
docs/                   plan and spike write-ups; spike code sits next to its write-up in docs/spikes/,
                        and docs/spikes/doc-check/ tests the write-ups against the code they cite
.github/workflows/      CI checks and unsigned desktop bundles
.github/scripts/        the scripts those workflows run (smoke run, spike staging, checks) and their tests
```

The rest (the Cloudflare resource inventory is in infra/README.md):

```
crates/ghost-config/     per-agent config adapters for the Plugin library and Instructions
crates/ghost-remote/     remote access: pairing, devices, bot links and the sealed channel
crates/ghost-release/    offline preparation and verification of release artifacts
apps/web/               onghost.dev (React Router 7, prerendered)
apps/web-worker/        the Rust Worker that serves the prerendered site
apps/relay/             the Worker behind remote access
infra/                  the Cloudflare resource inventory
```

## Development

### Prerequisites

- **Rust** stable with rustfmt and clippy. `rust-toolchain.toml` selects it, so rustup installs it on the
  first cargo command. The minimum supported version is `rust-version` in `Cargo.toml` (1.95, set by
  the rusqlite_migration dependency). rustup never updates an installed stable on its own, so if
  `rustc --version` is older, run `rustup update stable`.
- **Node.js** 22.12 or a later 22.x, 24.x, or 26 and newer: the range Vite 8 and Vitest 5 support, set
  by `engines` in `package.json`. `pnpm install` refuses any other version. CI uses 24.
- **pnpm** 12, the version pinned by `packageManager` in `package.json`: `brew install pnpm`, or see
  [pnpm.io/installation](https://pnpm.io/installation). pnpm reads its settings from
  `pnpm-workspace.yaml`.
- **Tauri's system dependencies** ([details](https://v2.tauri.app/start/prerequisites/)):
  - macOS: Xcode Command Line Tools (`xcode-select --install`).
  - Windows: Microsoft C++ Build Tools ("Desktop development with C++"), the MSVC Rust toolchain
    (rustup's default on Windows), and WebView2. The installer bootstraps a missing WebView2 runtime.

### Run

```sh
pnpm install
pnpm dev
```

`pnpm dev` runs `tauri dev`: Vite serves the UI on `localhost:1420` with hot reload, and the Rust shell
rebuilds and relaunches when its code changes. Release bundles (`pnpm bundle`) and the macOS signing
identity (`pnpm signing`) are described in [apps/desktop/README.md](../apps/desktop/README.md).

App data lives in a folder named `dev.onghost.desktop` inside the local (never roaming) app data folder:

| OS | Folder |
|---|---|
| macOS | `~/Library/Application Support/dev.onghost.desktop` |
| Windows | `%LOCALAPPDATA%\dev.onghost.desktop` |
| Linux | `$XDG_DATA_HOME/dev.onghost.desktop`, or `~/.local/share/dev.onghost.desktop` when `XDG_DATA_HOME` is unset |

A copy updated from auracode, the app's name until 19 November 2026, keeps using the folder it
had, `si.auracode.desktop`, because the worktrees inside it record their full paths
([docs/name.md](name.md)).

`GHOST_DATA_DIR` moves it anywhere else (see [Environment switches](#environment-switches)); the
old name, `AURACODE_DATA_DIR`, still works.
Uninstalling deletes it, with everything else Ghost keeps, and Settings can do the same
without uninstalling; [docs/uninstall.md](uninstall.md) lists what goes and what stays.

A development build (`pnpm dev`, `cargo run`, `tauri build --debug`) uses `dev.onghost.desktop.dev`
next to it instead, runs as its own instance beside the installed app, and cleans up only its own
folder, so working on Ghost from a chat inside Ghost never touches the app running that
chat. [docs/developing-in-ghost.md](developing-in-ghost.md) covers building, testing and
installing a change from inside the app, and recovering from a broken build.

### ghost-cli

`ghost-cli` runs agents without the app, through the same session manager, on a temporary profile
with a fresh temporary git repository as the chat folder (M1a spec 9.5 and 10):

```sh
cargo run -p ghost-cli -- agents                               # the env and each agent
cargo run -p ghost-cli -- probe hermes                         # free probe steps, no prompt
cargo run -p ghost-cli -- smoke hermes --scenario all --spend  # paid scenarios
cargo run -p ghost-cli -- fixture <log> --scenario basic --out <file>
cargo run -p ghost-cli -- replay <fixture>                     # against its golden; --update writes it
```

A probe or smoke run keeps its folder (protocol logs, database, chat folder) and ends with the
`ghost-cli fixture` command for each log it left. Its agent processes register in the data folder's
`run/` (for a development build, its own folder), so the next app start on that folder cleans up
after a crashed run, and it shares that folder's
downloaded adapters. `--data-dir <dir>` runs on that data folder's own profile instead, and is refused
while the app runs on it. `ghost-cli help` lists every option; `RUST_LOG` shows its logs on stderr
(default: warnings only).

### Checks

CI runs only when deliberately dispatched for an integration checkpoint or final verification. The
active matrix is Windows and macOS. Routine commits and pushes do not trigger CI. To run focused
checks locally, from the repository root:

```sh
pnpm check                      # pnpm lint, pnpm typecheck, pnpm test (Vitest)
pnpm build                      # the production frontend build
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --manifest-path docs/spikes/windows-spawn/Cargo.toml
cargo build --manifest-path docs/spikes/claude-control/Cargo.toml   # compile only, never run
cargo test --manifest-path docs/spikes/claude-control/Cargo.toml    # unit tests only, never the binary
cargo test --manifest-path docs/spikes/doc-check/Cargo.toml         # spike docs against the code
bash .github/scripts/lint-workflows.sh                              # action pins, runner labels, schedule names
shellcheck -x .github/scripts/*.sh
bash .github/scripts/test.sh                                        # the CI scripts' own tests

# The smoke runs: the app, with its frontend embedded, streams a demo turn, a cancelled turn, and a
# stress burst through the real UI; then it drives the chat screens against the demo agent in two
# launches on one data folder, quitting through the quit flow between them, and prints every
# check. A second argument, stream or chat, runs one of the two.
pnpm --filter @ghost/desktop tauri build --debug --no-bundle
bash .github/scripts/run-smoke.sh target/debug/ghost
```

`.github/workflows/ci.yml` includes Rust and native smoke checks on Windows/macOS, the Windows
PowerShell suites, frontend tests, compiled Rust Worker tests, and Chromium/WebKit browser gates.
`.github/workflows/build.yml` is an
explicit unsigned Windows/macOS installer qualification workflow. Neither a commit message nor a
tag publishes a release. Signing, hardware acceptance and distribution follow [docs/releases.md](releases.md).

- Biome is the only linter and formatter outside Rust (there is no ESLint or Prettier): `pnpm lint`
  checks, `pnpm format` rewrites. It covers TypeScript, CSS, JSON, and YAML. It does not format Markdown
  (hand-wrapped prose and tables) and never touches `pnpm-lock.yaml` (written by pnpm) or the replay
  fixtures and goldens in `crates/ghost-drivers/tests/fixtures` (written by the replay harness). Rust is
  formatted with `cargo fmt --all`.
- Clippy rejects `unwrap()` and `expect()` outside tests (`Cargo.toml` and `clippy.toml`).
- The claude-control binary is never run, in CI too, because running it drives a real Claude Code
  session, which spends quota. Only its unit tests run: they feed the code canned input, and a shell
  stands in for `claude` where they need a process.

### Generated TypeScript bindings

`apps/desktop/src/bindings.ts` types the UI's calls into Rust. tauri-specta generates it from the shell's
command list. After changing a command or a type it uses, run
`cargo test -p ghost-desktop export_bindings` and commit the result (debug builds of the app also
regenerate it at startup). CI fails when the committed file is out of date.

### Environment switches

For development and automation; the app reads them once at startup. On/off switches accept `1`, `true`,
`yes`, or `on` in any case; any other value is off.

| Variable | Effect |
|---|---|
| `GHOST_DATA_DIR` | Keeps all app data in this folder instead of the OS default (`dev.onghost.desktop`, or `dev.onghost.desktop.dev` for a development build), for example a throwaway folder to test a first launch. Such a launch runs as its own instance beside a running app. On Windows that includes the webview profile (`EBWebView`); such a launch does not remember the window size and position, so it never touches the installed app's. Blank values are ignored; a relative path is resolved against the current folder. |
| `GHOST_NO_GLASS` | Opaque window without the native glass (Liquid Glass, vibrancy, Mica): the baseline of the power spike. |
| `GHOST_STRESS` | Opens straight into the stress view. Comma-separated `orbs` (1 to 24), `rate` (1 to 5000 tokens/s), `aura` (1 or 0), and `seconds` (0 to 86400, 0 streams until the window closes); missing keys default to `orbs=6,rate=200,aura=1,seconds=0`, and `1` alone means all defaults. An invalid value is ignored and the app shows a notice. See [docs/spikes/render-power.md](spikes/render-power.md). |
| `GHOST_SMOKE` | Runs a scripted smoke check through the real UI and prints `GHOST_SMOKE_RESULT <json>` on stdout, or a failing result if the UI has not reported in time (120 seconds; 300 for `chat`, 900 with `GHOST_SMOKE_AGENT`). `1` (or `stream`): a demo turn, a cancelled turn, a short stress burst. `chat`: the chat screens against the demo agent, every block, card and queue path, ending with the quit flow while a turn runs; `chat-reopen`: the same data folder started again, checking the stored history, the interrupted turn and Resume. Each chat check is in the result. `0`, `false`, `no` and `off` are off; any other value that names no check prints a failing result. Takes precedence over `GHOST_STRESS`. Every smoke run needs `GHOST_DATA_DIR`, a data folder of its own; without it the run prints a failing result and opens nothing, so it can never change the installed app's chats. Smoke runs keep running while the window is hidden or the screen is locked (macOS 14 and later). |
| `GHOST_SMOKE_EXIT` | With `GHOST_SMOKE`, exits after the result, through the quit flow: code 0 if the check passed, 1 otherwise. |
| `GHOST_SMOKE_FOLDER` | With `GHOST_SMOKE=chat` or `chat-reopen`, the chat's folder (an absolute path), which the folder picker answers without a dialog. Required for those. |
| `GHOST_SMOKE_AGENT` | With `GHOST_SMOKE=chat`, one minimal chat with this agent (`hermes`, `claude`, `codex`) or connection (`claude-acp`, `codex-acp`: the run first sets that agent's Connection to it, in the run's own data folder) instead of the demo agent's run: under Ask it sends "Reply with exactly: ok" and passes when the reply is ok (`mock`, the demo agent, gets `/echo ok`). `run-smoke.sh` passes it on and then runs that one launch. A real agent spends your quota, so this is for local runs only: the app and `run-smoke.sh` refuse it while `CI` or `GITHUB_ACTIONS` is set. A first start may download the agent's adapter, hence the longer watchdog. |
| `RUST_LOG` | Log filter for the Rust side, which logs to stderr. Default `info`. |

For example `GHOST_STRESS=orbs=24,rate=200 pnpm dev`. In PowerShell, run
`$env:GHOST_STRESS = "orbs=24,rate=200"` first.

## History

The repository's README carried these notes until 20 November 2026. They describe the earliest milestones and the state of the project when they were written; [CHANGELOG.md](../CHANGELOG.md) is the record of everything since, and [progress.md](progress.md) the implementation evidence.

### Status as it was written

The integrated product is in development and final acceptance is pending. The full plan and decisions
live in [docs/plan.md](plan.md); [docs/progress.md](progress.md) records implementation and
verification evidence. Windows, macOS and Linux (deb, rpm and AppImage) are the current targets.

Visual work follows the [Aura visual system](design/visual-language.md): a website that follows the system appearance and a desktop with System, Light and Dark appearances.
The [styling review record](design/checkpoint-2026-10-30.md) tracks the full pass and its
review gallery. Owner visual approval and final verification remain pending. Historical Design Lab studies are not
the design authority for new product screens.

The auracode.si account system (sign-in, sync between devices, the account API and its website
pages) was removed on 16 November 2026 at the owner's request, because Claude Code and Codex sync
plugins natively. A profile folder an account left in the data folder is never opened, listed or
deleted.

Backend and cloud services are Rust-first: the website is a Rust Worker serving the prerendered
pages, published at [onghost.dev](https://onghost.dev) and its isolated preview, and remote access
goes through the relay Worker (`apps/relay`). The Cloudflare resources are listed in
[infra/README.md](../infra/README.md), with a record of the ones the account system used, which were
deleted with it.
React/TypeScript handles interfaces. Optional local AGENTS.md and CLAUDE.md files are ignored by
Git. When present, the lint gate requires both files to be identical; a clean checkout needs neither.
Installers are unsigned preview builds: publisher signing is deferred at the owner's request. They
are published on this repository's Releases page and listed with their checksums at
[onghost.dev/download](https://onghost.dev/download); [docs/releases.md](releases.md) says
how. Ghost is open source under the [MIT License](../LICENSE).

Current source includes native agent drivers, the desktop workspace, onboarding, project and Git
controls, a per-agent Plugin library, remote access, readable chat export and portable folder
resolution, with focused verification. Final integrated and native acceptance remain pending. The
milestone notes below describe the earlier merged foundation, not the current branch's full
implementation. Release requirements and the deliberate final verification batch are in
[docs/releases.md](releases.md).

#### M0: scaffold and spikes

Merged into main.

Done:

- **Domain.** The site is at onghost.dev since 2026-11-26. Its earlier addresses, o7.codes
  (from 2026-11-19) and auracode.si (a Cloudflare zone since 2026-10-24), send their pages there
  and still answer the relay and the remote page for copies of the app from before each move
  ([infra/README.md](../infra/README.md)).
- **Scaffold.** The Cargo and pnpm workspaces with rustfmt, clippy, and Biome; the event model
  (`ghost-protocol`); the data folder, device identity, profiles, and per-profile database (`ghost-core`);
  frame batching and the scripted mock agent (`ghost-drivers`); the design system and WebGL orb renderer
  (`packages/ui`); the desktop shell with its glass window, typed commands and channels, and a demo stream;
  the CI and bundle workflows, which run on Linux and Windows (see [Checks](#checks)).
- **Spikes.** [Claude Code's control protocol](spikes/claude-control.md) is decided.
  [Windows spawning](spikes/windows-spawn.md) passes its automated tests on Windows and Linux in
  CI; its manual checklist and [the Windows titlebar](spikes/windows-titlebar.md) wait for a
  Windows PC.
  [Render and power](spikes/render-power.md) has measured the hidden window and the page in WebKit;
  the visible window and Windows remain.

Pending, each waiting on the repository owner:

- **The local signing identity.** `pnpm signing create` adds a self-signed certificate and its private key
  to the login keychain, so it waits for the owner's OK (see
  [apps/desktop/README.md](../apps/desktop/README.md#signing-on-macos)). Until then builds are signed ad hoc,
  and macOS asks for `~/Desktop` access again after every rebuild.
- **The macOS visible-window render and power run.** It needs an unlocked Mac left idle for about 20
  minutes ([steps](spikes/render-power.md#finishing-the-macos-half)).
- **The Windows manual checklists.** CI runs on Linux and Windows (the spawning spike's tests, the
  smoke run on WebKitGTK and WebView2, the render-power suite's unit tests), but the manual checklists
  for [spawning](spikes/windows-spawn.md#manual-checklist-the-users-windows-pc),
  [the titlebar](spikes/windows-titlebar.md#verification), and
  [render and power](spikes/render-power.md#windows-webview2-half-still-to-run) need a Windows
  PC.
- **An app screenshot.** It needs an unlocked screen: a locked Mac hides every window.

#### M1a: sessions, processes, the ACP client and the bare chat UI

Merged into main from the `m1a-core-acp` branch, following [its spec](design/m1a.md): agent processes and their
environment, the session manager and its store, the ACP driver, the chat commands and channels, a bare
chat UI styled through tokens, and `ghost-cli`. Verified against real agents on this Mac (spec 10.6):
Hermes, Claude Code through claude-agent-acp and Codex through codex-acp each pass the free probes, the
six paid scenarios and a chat through the UI, with recorded fixtures replayed in CI. That M1a
milestone prototyped the Orbit language in the Design Lab; the proposed current direction is documented in
[the visual language](design/visual-language.md); the visual checkpoint awaits owner review.
