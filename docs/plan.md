# Ghost: implementation plan

## Current authority
**Accounts were removed on 16 November 2026, at the owner's request, because Claude Code and Codex
sync plugins natively.** The app is local-only: no sign-in, no sync between devices, no account API
and no account pages on the website. Everything below about accounts, sync, the cloud API, D1,
blob storage, sign-in email and Turnstile (the website's sign-up and account pages, M6 to M9, and
"Sync design") is history, kept as it was written; the code for it is gone. Remote access, the
website's download and legal pages, and the release bucket are current.

The owner revised the architecture on 2026-10-26: Rust owns backend logic, all application cloud
infrastructure lives on Cloudflare, and the root `AGENTS.md` and `CLAUDE.md` stay byte-identical.
The proposed full Aura styling pass supersedes the older Graphite, Orbit and Shallows directions. See [visual-language.md](design/visual-language.md) for current authority and review status.
`docs/progress.md` records implementation status and `infra/README.md` records provisioned resources. Historical machine facts below are not current discovery.
The owner narrowed the current release to Windows and macOS and deferred Linux/WSL, then added Linux
back on 2026-11-04 (deb, rpm and AppImage, built on Ubuntu 22.04); WSL agent hosts stay deferred. CI is now
reserved for a few deliberate integration checkpoints and the final verification, with failure
fixes rerun as needed. Visual work follows the proposed Aura system, with a website that follows the system appearance and light and dark desktop appearances, and the audited references in [visual-language.md](design/visual-language.md). The full styling pass is being tested; owner visual approval and native release verification remain pending.

## Context
The user runs many coding agents (Claude Code, Codex, Antigravity, Pi / oh-my-pi, Cursor, Grok Build,
OpenCode, Hermes...) and hates toggling between apps. Ghost is one desktop app (macOS, Windows and Linux)
with a ChatGPT-style interface: projects on the left, chats inside projects, any installed agent driving a
chat through its real harness (headless, rendered in our UI, never a terminal). The base app needs no login.
A website at onghost.dev lets people create an account and download the app; the same account signs into
the app and syncs projects, chats, and agent config (Claude Code, Codex, Pi) across machines. The bar: the
most beautiful app of its kind, a killer onboarding, dead simple, every edge case handled.

## Decisions (user-confirmed unless marked)
- **Stack:** Tauri v2. Everything that isn't UI is Rust (agent processes + protocols, detection, updates,
  git, storage, search, sync, crypto); UIs are React + TypeScript. The cloud API is Rust on
  Cloudflare Workers via workers-rs and WebAssembly. Non-Rust backend exceptions need a documented
  platform constraint; platform configuration and SQL stay in their native formats.
- **Agents:** one generic ACP client (most agents) plus native Rust drivers for Claude Code, Codex, Pi.
- **Platforms:** macOS, Windows and Linux; agents auto-detected on the native host. WSL agent hosts are deferred.
- **Default autonomy: Full auto**, with per-turn checkpoints, isolation, and a visible badge as safety net.
- **Data:** local SQLite per profile, offline with no login. Cloudflare D1 stores accounts, sessions,
  devices and encrypted sync envelopes. R2 stores releases and encrypted blobs. Cloudflare Workers
  host the Rust API and website; Turnstile protects signup; Cloudflare Email Service sends codes.
  No Neon, Neon Auth, Hyperdrive, or Resend dependency.
- **Location:** D1 uses US jurisdiction; no promise of a specific state or city.

- **Website:** landing page, downloads (auto-detected OS), sign up / sign in, account page (devices, export,
  delete), legal pages. Same accounts as the app.
- **Accounts:** passwordless email code, on the site and in the app; seamless account switching in the app.
- **Sync scope:** projects + chats + config; config only for Claude Code, Codex, Pi.
- **Domain:** stays registered at Namecheap; nameservers moved to Cloudflare on Day 1. The auracode.si
  zone uses `asa.ns.cloudflare.com` + `rohin.ns.cloudflare.com` (the pair all the user's Cloudflare zones
  share) and has been active since 2026-10-24 09:03 UTC. The site moved to onghost.dev, a zone on
  the same account, on 2026-11-19; auracode.si is kept only until copies of the app from before
  the move have updated (infra/README.md).
- **End-to-end encrypted sync.** Chats contain code and secrets. Cost:
  a new machine is approved from a signed-in one or uses a recovery key.
- **Trust:** never read, store, or proxy agent credentials; Ghost never injects hidden prompts.

## Historical machine facts (verified 2026-10-24 on the Mac)
- macOS 26.6.2 arm64, rustc/cargo 1.99, Node 26.0.0, npm 11.12, git 2.54, gh 2.92 logged in as itscool2b
  (ssh, repo scope), Homebrew 6, pnpm 12.15 (Homebrew, installed for M0). No bun. Global git user.name
  is NOT set (email is).
- Installed agents: Claude Code 2.1.310, Codex 0.164.0, Hermes 0.22.5 (`hermes-acp`), Antigravity CLI `agy`
  1.4.7. Not installed: pi, omp, opencode, gemini, cursor `agent`, grok (leftover config dirs prove nothing).
- Claude runs a `claude daemon` with helpers in separate process groups (one orphaned to launchd); Codex has
  a shared app-server daemon with its own updater; the ChatGPT app bundles its own codex.
- This dev environment exports Claude Code's parent-session markers (`CLAUDECODE`, `CLAUDE_PID`,
  `CLAUDE_EFFORT`, `CLAUDE_CODE_SESSION_ID`, ...): children must not inherit them (see the env scrub rule).
- The user's code lives under `~/Desktop` (macOS privacy-protected).
- Claude Code syncs account-level skills/plugins itself (`~/.claude/{skills,plugins}/synced/`): never touch.
  Shared cross-agent skills live in `~/.agents/skills`.
- Cloudflare: the owner's account. The auracode.si zone is active (since 2026-10-24 09:03 UTC,
  on `asa.ns.cloudflare.com` + `rohin.ns.cloudflare.com`, delegation served by public resolvers), next to
  three unrelated free zones on the same account. Resend and Neon MCPs connected.

## Product principles
1. Simple first: zero setup, no login needed, everything auto-detected, one composer box, smart defaults.
2. Real harnesses, not reimplementations: each agent keeps its own auth, config, tools, skills, plugins.
3. No hidden instructions. The only thing Ghost adds to a prompt is the visible handoff block when you
   switch agents or devices. (The "let the agent chill" philosophy.)
4. Local-first; cloud only for accounts and sync.
5. Every animation means something: agent marks and live labels show real state, not decoration.

## UX spec: app
The proposed [visual language](design/visual-language.md) governs the desktop and website. Use system appearance by default with persisted light and dark overrides on desktop; the website and its standalone notices page remain light without an appearance control or persisted theme. Preserve the logo and remove standalone animal and water decoration. The following functional requirements remain in force.

- Collapsible project/chat sidebar, Needs you inbox, agent dock, account menu, main transcript,
  composer, and a right panel for Changes, Files, Plan, and Session information.
- Agent marks (monochrome tiles with a pixel loader, needs-you square, done check) show state.
  Only live marks animate; pause offscreen and when inactive; reduced motion is static. No WebGL.
- Raised user blocks, plain agent prose, collapsible thinking/tool rows, diffs, terminal output,
  and receipts with duration, usage, cost when known, and changed files. Virtualize large histories.
- Permission/question cards occupy the composer when needed, with keyboard answers and visible
  request scope. Never offer an unsupported permission choice or auto-answer an always-ask request.
- Auto-growing composer with attachments, file mentions, slash commands, queue/steer, and unified
  agent/model/effort controls. Esc interrupts; the second Esc force-stops. Keep text being read stable.
- Full auto is the default, with a visible badge. Honor agent policy restrictions. Claude falls back
  to acceptEdits with a notice when bypass is unavailable; Codex applies policy per turn. Pi's
  inability to ask is explicit. Never turn a generic automatic approval into a persistent rule.
- Titles come from the agent or first user line, never a hidden title-generation prompt.

**Boot and onboarding.** Use the proposed visual language for setup and show the hidden window after first paint;
use cached detection and reconcile without blocking startup. Onboarding is keyboard-driven,
resumable, re-runnable, and entirely skippable: welcome, discovery, account, motion preference,
first project by folder/drop/gh clone. No agents means installation suggestions and a demo chat.
The account step stays disabled until M6; encrypted device approval/recovery and restore activate
with M7. No screen promises sync before it works.

## UX spec: website (onghost.dev)
Share semantic tokens and components with the app; use Geist on the website and Inter in desktop chrome. React/TypeScript renders interfaces;
prerender public pages and serve static assets through Cloudflare. Rust owns API behavior.

- /: proposed Vercel-inspired hero, real desktop imagery and staged capability proof, followed by FAQ and closing. Preserve existing account availability and actions.
- /download: every OS/architecture, version, notes, SHA-256 checksums, requirements and unsigned-build
  installation instructions. Works without JavaScript; always permits a different OS choice.
- /signup and /signin: email, six-digit code, resend cooldown, Turnstile, clear expiry/delivery errors.
- /account: devices and revocation, storage, sign out everywhere, export, verified email change,
  and account deletion after a fresh code.
- /privacy, /terms, /changelog: accurate processor, retention, export and deletion information.
  Legal copy needs review before public release; a domain suffix alone does not establish jurisdiction.
- The auracode://signin link prefills email only, never tokens. Display Download beside Open Ghost.
- Reduced motion, keyboard access, and a whole page without JavaScript (including essential
  information) are required.

## Projects, chats, agent switching
- **Project** = name + colour + 1..n folders + optional default agent/model. Folders have a portable
  identity (git remote URL + default branch, or "local-only on <device>") separate from the device-local
  path, which is what lets projects sync across macOS and Windows.
- **Chat** = one primary folder; extra folders become add-dirs only where the agent supports them, and the
  chat header shows exactly which folders it can reach.
- **Isolation (M4):** "Direct" vs "Isolated" (git worktree on a new branch `ghost/<slug>`). Until M4, a second
  chat in a busy folder gets a warning. From M4, Ghost asks before isolating.
- **One thread, many agents.** Switching agent resumes that agent's native session if it has one in this
  chat (sending only what happened since its last turn), else starts one in the same cwd; either way the
  first prompt carries a visible, token-budgeted handoff block (earlier user messages, other agents' final
  answers, how a turn that did not finish ended, the working folder, the newest shell commands with how
  each ended, the last command still failing with its first error, files changed + `git diff --stat`;
  12 KiB at most). The window lays it out in the review dialog, over the composer while it waits, and on
  the message that carried it, with the exact text one row away (`HandoffView.tsx`). Agents that can't resume (restart, cwd change) take the same
  handoff path. Claude Code resumes after a cwd change, so a Claude chat moving between Direct and
  Isolated folders keeps its native session without the handoff block: `--resume=<id>` from the new folder
  (the transcript keeps appending under the original folder's project dir), or, when the original chat
  goes on too, `--resume=<id> --fork-session --session-id=<new>`, whose transcript lives under the new
  folder (two live processes never share a session id). Claude still takes the handoff path when its
  transcript is gone (purged, cleaned up, another machine). Model/effort switches stay inside the native
  session where supported.
- **Checkpoints (M2):** before every turn, a snapshot into a shadow git repo per folder
  (`<profile>/checkpoints/<folder-id>.git`, temp `GIT_INDEX_FILE`, refs under `refs/aura/*`). Never touches
  the user's index, branches, or stash, and works in non-git folders. One-click revert of any turn.

## Architecture
```
auracode/                        Cargo workspace + pnpm workspace
  crates/
    ghost-protocol/               GhostEvent, commands, capabilities, sync/API types (serde + specta -> TS)
    ghost-process/                process lifecycle (spawn, framing, shutdown, leftovers), env resolution +
                                 scrub, protocol logs
    ghost-agents/                 data-driven catalog, detection, adapter install, channels, versions, auth
    ghost-drivers/                protocol drivers: acp, claude, codex, pi; stream fallbacks; mock
    ghost-core/                   sessions, store, git/checkpoints/worktrees, search, handoff
    ghost-config/                 (M8) per-agent config adapters (read into items, apply changes)
    aura-sync/                   (M6/M7/M9) API client, token manager, merge engines, crypto
    ghost-cli/                    dev CLI: detect, smoke-test agents, record fixtures, replay
  apps/
    desktop/src-tauri/           Tauri shell: thin commands/channels over ghost-core, window glass, plugins
    desktop/src/                 React UI
    web/                         (M6) onghost.dev: React Router 7 on Cloudflare Workers
  services/api/                  (M6) Rust Cloudflare Worker: accounts, devices, sync, email delivery
  packages/ui/                   TS Aura design system shared by app + site: tokens, marks, motion
  packages/api-types/            TS types generated from ghost-protocol (shared by app, site, API)
  infra/                         Wrangler configs, SQL migrations for D1, release scripts
```
- **Processes.** One Tauri process runs ghost-core on tokio (`panic = "unwind"` in release; panics contained
  per task). ghost-core doesn't depend on Tauri, so a headless daemon can reuse it later for remote access.
- **Shell.** Tauri 2.12.x: the window keeps its platform material plan (vibrancy on macOS, Mica on
  Windows 11, opaque elsewhere), but the interface uses solid surfaces, so native material
  shows through. Overlay titlebar + `trafficLightPosition`
  on macOS; Windows titlebar decided by
  an M0 spike (custom controls lose Snap Layouts, so native caption buttons are the default). Plugins:
  single-instance, deep-link (`auracode://`), notification, window-state, updater, dialog, opener.
- **Crates (exact versions pinned at M0):** tokio 1.54, serde, `serde_json` with `preserve_order` (agents'
  key order survives into logs and fixtures), tauri-specta 2.0.0-rc.26 (pinned `=`; ts-rs fallback), our own
  JSON-RPC peer for ACP with no runtime `agent-client-protocol` (`agent-client-protocol-schema` =1.12.0 is a
  dev-dependency, for conformance tests only), process-wrap 10, portable-pty 0.9 (login terminal),
  rusqlite 0.40 + rusqlite_migration (one writer thread owns the store in M1a, with no tokio-rusqlite; a read
  pool can come later behind the same API; FTS5; SQLCipher from M7), the `git` CLI for all git work
  (`GIT_OPTIONAL_LOCKS=0` when polling), ignore + nucleo-matcher, notify, reqwest 0.13, semver,
  similar/diffy, toml_edit + jsonc-parser, keepawake, keyring, tracing. Our own login-shell env resolver.
- **IPC.** Commands for request/response; `Channel` for streams; token deltas batched per frame into frames
  under 8 KiB; tool output capped (full output on disk).
- **App UI stack.** React 19.3, Vite, TypeScript, plain CSS on shared semantic tokens, Motion 14, shadcn on Base UI (fully
  restyled), Zustand, react-virtuoso + use-stick-to-bottom (no layout animations inside the virtual list),
  Streamdown 2.7 (sanitized, own reduced-motion handling), Shiki 4 in a worker (finished code blocks only,
  throttled while streaming), @pierre/diffs, xterm.js 6 + WebGL (login terminal), OGL, Lucide, Floating UI.
- **Web stack.** React + TypeScript, Tailwind, Motion, `packages/ui`, Turnstile. Public pages
  prerendered, account screens call the Rust API. Any framework server glue handles rendering only.
- **Cloud layout.**
  - onghost.dev and www (redirect): Cloudflare-hosted website assets. /api/v1/* reaches the Rust API
    Worker by service binding or a same-origin Worker route; no separate cross-origin login service.
  - Rust API: email-code auth, sessions, devices, sync, scoped blob access, quotas and rate limits.
    Use Workers bindings for D1, R2, Turnstile verification and Cloudflare Email Service delivery.
    Minimal JS binding glue is permitted if workers-rs lacks a platform API; business logic stays Rust.
  - D1 Ghost: US jurisdiction, read replication disabled for the initial authentication path.
    Check session and device revocation authoritatively, never through an eventually consistent cache.
  - R2 auracode-releases: published installers, manifests, checksums; auracode-blobs: private ciphertext.
    Development uses local/emulated or separately named resources, never production test data.
  - Structured diagnostics omit codes, tokens, credentials, account emails and plaintext sync data.

- **Engines.** WKWebView on macOS and WebView2 on Windows are the supported native targets. Repeat the M0 idle-power measurements after visual changes; Linux remains deferred.
- **Catalog vs drivers.** The catalog is data (per agent: binaries + known dirs per OS, version command,
  channel rules, latest-version source, vendor update command, auth-status command, launch spec, config dir,
  brand, last-verified version). Drivers are per protocol: ACP, Claude stream-json, Codex app-server, Pi RPC
  (+ stream fallbacks, mock). Agents launch through their stable symlink/shim; real paths only identify the
  install channel. Copies bundled inside other apps are read-only channels.
- **Driver contract.** A driver is an actor: `AgentDriver::run(ctx)` takes commands in (prompt, interrupt,
  permission and question answers, option and autonomy changes, shutdown) and hands events out over a
  channel bounded by count and bytes, every event tagged with the turn it belongs to, and spawns through
  the session's spawner, which records every process. Prompts are a list of parts (text in M1a; images and
  mentions in M2c). Questions have an answer path (`RespondUi` in, `UiResolved` out), like permissions.
  Turns, the prompt queue and persistence belong to the session manager, not the drivers. The UI reads
  `capabilities`, never agent names.
- **GhostEvent** (ACP vocabulary): turn.started/ended, user_message.delta, message.delta/completed,
  thought.delta/completed, tool.call, tool.update (text | diff | terminal), permission.requested/resolved,
  ui.request/resolved, plan.updated, usage.updated, limits.updated, session.updated (status, native id,
  title, model, effort, mode; no slash commands, which the UI fetches on request), prompt_lifecycle (the
  command lifecycle event: queued, started, completed, cancelled, failed), notice, error. `TokenUsage`
  splits cache reads from cache writes and says whether it counts a turn or the session; `StopReason`
  adds `interrupted` (Ghost ended the turn) and `other`. Stored enums read a value they don't know as a
  fallback, so rows from a newer build still load.
- **Profiles from day one.** All local data under `<app data>/profiles/<profile_id>/` (database,
  attachments, checkpoints, logs). M0 ships one "Local" profile; M6/M7 add one per account.
- **Storage.** UUIDv7 ids, `device_id`, tombstones (no hard deletes), portable folder identity from M0; the
  sync clock and sync-state tables arrive by migration in M7. Events append-only, compacted per turn; FTS
  updated at compaction. DB backed up before every migration.

## Agent integration (researched 2026-10-24)
| Agent | Path | Notes |
|---|---|---|
| Claude Code | Native: own Rust control loop over stream-json; user's `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio`, always with `--permission-mode`, with `--session-id=<uuid>` (new session) or `--resume=<id>`, and with `--allow-dangerously-skip-permissions` where the user's settings allow bypass | Contract and fixtures in the [M0 spike](spikes/claude-control.md) (2.1.310). Own lenient serde types; `claude-codes` crate and the official Python Agent SDK are references, diffed each release. Control: `initialize`, `interrupt`, `set_model`, `set_permission_mode`, `apply_flag_settings` (effort), `get_usage`, `get_context_usage`, `list_models`, `rewind_files`; we answer `can_use_tool`; fork via `--fork-session` (with our `--session-id`) or resume-at. `--permission-mode` is never left out: without it every spawn, resumes included, takes `permissions.defaultMode` from the user's settings, which can be `auto`. `--session-id` gives a new session's native id before its first turn. `bypassPermissions` can be switched on mid-chat only in a process launched with `--allow-dangerously-skip-permissions`. Resume works after a cwd change (the transcript keeps appending under the original folder's project dir), and `--fork-session` with `--session-id` writes the fork under the new folder's. The user's own hooks run inside these sessions (`SessionStart` context, `PermissionRequest` hooks that can answer or delay a prompt), so a tool call may never reach us as `can_use_tool`; principle 3 still holds, since Ghost itself adds nothing. `rate_limit_event` feeds the limits meter. Unknown messages logged, never fatal; capabilities from `system/init`; compatibility smoke per new version. Second connection (M1a; one agent with the native one since 2026-10-28, see "One entry per agent"): ACP via the `claude-agent-acp` npm adapter, installed once per version into `<data root>/adapters` with `npm ci --omit=optional --ignore-scripts` from a lockfile committed for that version, launched as `node <entry>` from there, with the user's `claude` forced through `CLAUDE_CODE_EXECUTABLE`; never `npx` in the chat folder. |
| Codex | Native: `codex app-server --listen stdio://`, own process | Own serde subset with `#[serde(other)]`, tested against the JSON Schema from `codex app-server generate-json-schema` per supported version. `thread/start/resume/fork/list`, `turn/start` (model, effort, cwd, approvalPolicy, sandboxPolicy, images, mentions), `turn/steer`, `turn/interrupt`, `turn/diff/updated`, approvals, `model/list`, `account/rateLimits/read`, plugin/skill/config APIs (reused by Library + Sync). Never the shared daemon (opt-in later). Second connection (M1a; one agent with the native one, see "One entry per agent"): the `codex-acp` npm adapter, installed and launched like Claude's, with the user's `codex` forced through `CODEX_PATH`. |
| Pi | Native RPC `pi --mode rpc` | `prompt`/`steer`/`follow_up`/`abort`, `set_model`, `get_available_models`, `set_thinking_level`, `fork`, `get_session_stats`, `extension_ui_request` (rendered as confirm/select/input). Turn ends on `agent_settled`; split on LF only. |
| Antigravity | ACP via Google's `agy_acp_server` (registry `antigravity-acp`) | Second connection, the text fallback: `agy --input-format stream-json --output-format stream-json`, with visible prompts on stdin (text only, no approvals, interrupt = kill). The persistent input mode drops `-p` prompts. |
| oh-my-pi | ACP `omp acp` | Never set `PI_CODING_AGENT_DIR` globally. |
| Cursor | ACP through the vendor-named `cursor-agent acp` | Dialect: deferred `cursor/ask_question` / `cursor/create_plan` and activity notifications. Use advertised model/config controls; model switching by restart still needs pinned vendor qualification. Never detect by the name `agent`. |
| Grok Build | ACP `grok agent stdio` | Second connection, the text fallback: `grok -p --output-format streaming-json`. |
| Hermes | ACP `hermes acp` | A viewer of its gateway daemon, where turns run: cancel before kill, with time for the cancel to land. Dialect: legacy `modes` (`default`, `accept_edits`, `dont_ask`) and `models` lists, the untyped `session/set_model`, mode changes only while idle; Full auto = `dont_ask` plus auto-answers for command approvals. Not in the registry; hard-coded. |
| OpenCode | ACP `opencode acp` | One agent; V1 (1.19.x) and V2 (2.1.x) are its two connections, qualified separately. |
| Gemini CLI + registry agents | Generic ACP (registry `cdn.agentclientprotocol.com/registry/v1/latest/registry.json`) | Shown with an "unverified" badge until tested. |

ACP client advertises `fs` and `terminal` as false (agents use their own file I/O and shells), and
`auth.terminal`, `session.notices` and `session.configOptions.boolean`. Model and effort are session config
options, told apart by category; `session/set_model` is gone from the schema and survives only in the
legacy dialect, for agents that still send a `models` list (Hermes). `session/load|resume`; `usage_update`;
sign-in through the agent's own terminal command. Unknown agent-to-client requests get JSON-RPC "method not
found" so agents never hang.

**One entry per agent (owner decision, 2026-10-28).** People see one Claude Code and one Codex. The
catalog lists *connections* (ways to start an agent) and files each under its agent: Claude Code is
`claude` (Ghost's native driver) and `claude-acp` (the npm ACP adapter); Codex is `codex` and
`codex-acp`; Antigravity and Grok Build are their ACP server and a restricted text fallback
(`agy-stream`, `grok-stream`); OpenCode is its 1.x and 2.x lines (`opencode`, `opencode-v2`).
New chat, the composer's picker, onboarding's count, the Agents page and the Plugin library all list
the agent once, from the same Rust list.
- **Automatic (the default):** a new session uses the first connection, in catalog order, that can
  run here: the native driver when the user's CLI meets its minimum version (Claude Code 2.1.310,
  Codex 0.162.1), else the adapter when it can run (Node.js 22 or newer, the CLI at the adapter's
  floor, npm until the adapter is installed). While the preferred connection is still being
  detected, the agent reads as checking.
- **Explicit:** each agent's details on the Agents page have a Connection row (Automatic, or one
  connection). It is device-local (`agent-connections.json` beside `agent-executables.json` in
  the data root), names catalog ids only, and applies to new sessions. An explicit connection that
  cannot run makes the agent unavailable with that connection's reason.
- **None can run:** the agent shows its preferred connection's reason, followed by each other
  connection's (named by its label) when either is installed but unusable, for example "Ghost
  needs Claude Code 2.1.310 or newer (found 2.1.309). ACP adapter: Node.js 22 or newer is required".
- **Existing data, no migration:** a chat, project default or New chat default saved as a
  connection id (`claude-acp`, also from a synced older device) reads as its agent. A native
  session row keeps the connection that made it, so a chat that ran on the adapter keeps resuming
  that saved session through the adapter, across handoffs to other agents and back; a fresh
  session (first start, Start fresh, a switch that cannot resume) takes the Connection setting. A
  native fork resumes through its source session's connection. A saved session whose connection
  can no longer run fails with its reason and says Start fresh continues on the other connection.
- **Actions** on an agent (update, sign-in check, executable choice, Plugin library listings) act
  on the connection it uses, so a CLI too old for the native driver can still be updated from the
  row the adapter runs. The adapter's one-time download is stated in the agent's details and in New
  chat before the first chat that needs it. The developer CLI (`ghost-cli probe`, `smoke`) still
  addresses connections by id.

**Detection + updates.**
- Env: resolve the login-shell env asynchronously (`$SHELL -ilc` with stdin `/dev/null`, markers, 10s
  timeout), cache it, never block boot on it; plus known dirs (`~/.local/bin`, `~/.opencode/bin`,
  `~/.grok/bin`, `~/.pi/agent/bin`, `~/.bun/bin`, `~/.volta/bin`, the asdf and mise shims,
  `/opt/homebrew/bin`, and the `prefix=` of `~/.npmrc`, read rather than by running npm). nvm stays with
  the login shell: its shell function can't be resolved reliably without one. The variables that describe
  how Ghost itself was launched (terminal session, npm, pnpm, Tauri dev) are stripped from the
  resolver shell's input.
- A binary counts only if `--version` runs. Channel from the real path; updates run the vendor's own update
  command for that channel (agents also self-update; we don't fight it). Each driver records a
  last-verified version; newer, untested versions get a small badge.
- Latest versions: npm dist-tags, GitHub releases (ETag), Homebrew JSON, vendor endpoints, ACP Registry;
  on launch + every 6h, cached.
- Auth status via each agent's own command; login runs the agent's own flow in a small terminal sheet.
- Windows: PATH = HKLM + HKCU merged with `REG_EXPAND_SZ` expansion; npm packages launched via their `bin`
  entry or bundled `.exe`; console handling chosen by an M0 spike (a tool call that runs git must not pop
  windows); Job Objects.
- WSL, automatic: `wsl.exe -l -q` (UTF-16LE), probe each distro, show "Codex · WSL Ubuntu"; an `AgentHost`
  (Native | Wsl{distro}) spawns via `wsl.exe -d <distro> --cd <path> -e ...` with path translation.

**Agent terms (matters now that other people can download Ghost).** Ghost only spawns the user's own
unmodified binaries. Anthropic allows signing in to the unmodified binary with your own subscription for
individual use and forbids third-party products offering claude.ai login or routing plan credentials;
OpenAI allows local/open-source app-server use, never commercial/hosted; Google's terms ban third-party
access, so their own ACP server is the sanctioned path. Personal use fits all three. Before a public launch:
get written confirmation from each vendor or default to API-key mode (every agent gets an optional API-key
setting).

**Prior art.** T3 Code (MIT; Codex, Claude, Cursor, Grok, OpenCode, Antigravity, polished), CC Switch (148k
stars; config push + WebDAV/S3 sync, no chat), AionUi (20+ agents via ACP), Zed, Conductor. Open ground:
full agent lifecycle, encrypted sync of projects + chats + config, one consistent chat for every agent,
consumer-grade polish. MIT/Apache code may be read for reference; never AGPL (Toad, opcode).

## Engineering rules
- **Processes:** drain stdout and stderr continuously; bounded large-line framing (base64 images); shutdown
  = protocol interrupt, close stdin, wait, SIGTERM, walk descendants by parent pid, SIGKILL last; record pid
  + start time and kill only our own leftovers at startup; treat agent state dirs as shared with vendor
  apps. The descendant walk kills every descendant, whatever its group or session, except the vendor
  daemons the agent's catalog entry declares (Hermes' gateway, Claude's daemon), which are spared with their
  subtrees, on every OS. On Windows every agent runs in a kill-on-close job; job breakaway for agents that
  declare daemons arrives in M1e, so M1a's Hermes on Windows needs its gateway already running. One
  Ghost process owns a profile's sessions (`<profile>/owner.lock`); a second one refuses to start them.
- **Env scrub:** children inherit the resolved login-shell env minus an explicit denylist. Everything
  else passes through, including documented user configuration (`CLAUDE_CODE_USE_BEDROCK`,
  `CLAUDE_CODE_USE_VERTEX`, `CLAUDE_CODE_MAX_OUTPUT_TOKENS`, `CLAUDE_CODE_GIT_BASH_PATH`, `CODEX_HOME`, and
  more), `SSH_AUTH_SOCK`, display and D-Bus session variables, proxies and CA bundles. Prefix families
  such as `CLAUDE_CODE_*` and `CODEX_*` would strip that configuration (Bedrock and Vertex users break,
  Codex reads the wrong home), while the M0 Claude control spike saw no observable effect from inherited
  markers, so over-stripping is the bigger risk. The denylist lives in one place in code with a test per
  entry (a marker found later joins it the same way), and values are never logged
  (`CLAUDE_CODE_MESSAGING_TOKEN` is a secret). It holds:
  - Claude Code's parent-session markers, as the spike saw them: `CLAUDECODE`, `CLAUDE_PID`,
    `CLAUDE_EFFORT`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_EXECPATH`, and the
    families `CLAUDE_CODE_SESSION_*`, `CLAUDE_CODE_MESSAGING_*` and `CLAUDE_CODE_BRIDGE_*` (seen:
    `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_MESSAGING_SOCKET`,
    `CLAUDE_CODE_MESSAGING_TOKEN`, `CLAUDE_CODE_BRIDGE_SESSION_ID`).
  - Codex's sandbox markers `CODEX_SANDBOX` and `CODEX_SANDBOX_NETWORK_DISABLED`.
  - `ELECTRON_RUN_AS_NODE`, `VSCODE_INSPECTOR_OPTIONS`, and `NODE_OPTIONS` only when it carries a debugger
    hook (`--inspect`, `--inspect-brk`, `--inspect-wait`, or a `--require` of a debugger bootloader such as
    the one VS Code's auto attach injects), which would attach to or pause every Node-based agent.
- **Long runs:** keep the machine awake while turns run; closing the window keeps Ghost in the menu bar /
  tray with a badge; quitting with live turns asks; app updates wait for idle.
- **Diagnostics:** rotating raw protocol log per session + "Copy debug bundle" (tokens redacted; also feeds
  fixtures).
- **Security:** sanitized markdown (no raw HTML from agents), strict CSP, minimal Tauri capabilities (the
  webview can't spawn processes or read arbitrary files; everything goes through vetted Rust commands),
  deep links validated against an allowlist of routes and params, path traversal checks on attachments and
  mentions, external links open in the browser (non-http schemes ask first).
- **macOS privacy:** builds signed with a stable local self-signed identity from M0 so `~/Desktop` grants
  stick (ad-hoc signatures change every build).
- **Worktrees (M4):** created outside the repo (short app-data path, avoids MAX_PATH and file watchers),
  always on a new branch, optional "bring uncommitted changes", per-folder setup (files to copy like
  `.env`, an install command), merge blocked while a Direct chat runs in the target.
- **Tooling:** rustfmt and clippy for Rust (`-D warnings` in CI; no `dbg!`, no `unwrap()` or `expect()`
  outside tests). No `unsafe` except SAFETY-commented calls into Windows APIs that std does not wrap, each
  item opting out of the denied `unsafe_code` lint on its own. The app uses these narrow helpers for
  private profile ACLs and invariant Windows path casing in native-session ownership checks.
  Biome alone lints and formats TypeScript, CSS, JSON, and YAML: one fast tool and one config instead of
  ESLint + Prettier. It does not format Markdown (its formatter re-pads hand-wrapped tables) and leaves
  `pnpm-lock.yaml` to pnpm. pnpm settings live in `pnpm-workspace.yaml` (pnpm 12 ignores them in
  `.npmrc`); `engineStrict` holds installs to the Node range in `engines`.

## Edge cases (each one has a defined behaviour and a test)
**Agents.** Not installed; installed but broken (`--version` fails: shown as broken with the error); several
installs of one agent (PATH order wins, others listed as shadowed, user can pick); installed, updated, or
removed while Ghost runs (watch bin dirs + re-detect on focus; running sessions keep their process);
signed out or token expired mid-turn (sign-in sheet, then resume); rate limit or quota hit (meter shows reset
time, one click hands the chat to another agent); version too old (update required) or untested-new (badge,
lenient parsing); crash mid-turn (error card with stderr tail + "Restart and resume"); silent for minutes
(soft "still working" with Interrupt, never auto-killed); folder-trust prompts that need a TTY (handled via
flags/config or surfaced in the UI); model removed upstream (qualified fallback or explicit recovery with a notice); proxies
(`HTTPS_PROXY` passed through).
**Sessions + streaming.** Huge transcripts (virtualized, paged from SQLite); huge or binary or invalid-UTF-8
output (capped, lossy decode, raw on disk); oversized or unsupported images (resized/converted, per-agent
limits); interrupt during a tool (second Esc force-stops); app quit or crash mid-turn (on relaunch the turn
is marked interrupted with Resume; partial message kept and labelled); sleep/wake (reconnect, WebGL restore);
disk full (persistence pauses with a banner, no crash); permission request while hidden (notification +
sidebar badge).
**Projects + git.** Folder moved, renamed, deleted, or on an unplugged drive ("Folder missing": Locate or
Remove); non-git folder (checkpoints still work; isolation offers `git init`); submodules, LFS, huge repos
(checkpoints respect .gitignore with size caps); detached HEAD or rebase/merge in progress (git actions
blocked with the reason); merge conflicts on merge-back (hand to an agent or open in editor); branch name
collisions (suffix); worktree deleted by hand (prune and recover); `gh` missing or signed out (PR action shows
setup); clone failures (auth, network, disk) with clear retry; Windows long paths, case-insensitive
collisions, CRLF; reverting a checkpoint after files changed outside the agent (diff + confirm, never a
silent clobber).
**Accounts.** Wrong or expired code, too many attempts (clear message, resend cooldown); email not arriving
(resend, spam hint, delivery status from Cloudflare Email Service); bot signups (Turnstile on the site; per-IP and per-email
rate limits in the proxy, also covering in-app requests); same flow for new and returning emails; session
expired or device revoked from the site (app drops to signed-out, local data intact and encrypted); offline
(sign-in waits, app fully usable locally); sign-out with unsynced changes (warns with a count); switching
accounts with chats running (asks); account deleted elsewhere (app signs out and offers to keep local data
as a Local profile); email change (re-verify); account created on the site but never used in the app (first
app sign-in becomes "first device"); approval request expires or codes don't match (abort, retry); lost
recovery key with no other device (cloud data can't be decrypted, said plainly at creation; "reset sync"
after email verification starts fresh).
**Sync.** Offline edits on several devices (clock-ordered last-writer-wins for metadata, append for
transcripts, 3-way for config); dropped connection mid-push (idempotent batches, resume from cursor); older
app meets newer data (schema version on records; never overwrites newer, prompts update); deleting a project
on one device while its chat runs on another (applied after the turn ends); repo at different paths (device
mapping; Locate or Clone); server down (local-first, backoff, status dot); storage quota reached ("sync
paused", never data loss); config for an agent not installed (waits); secrets (never plaintext).
**Website + downloads + updates.** Wrong OS/arch guess (universal macOS DMG avoids the Apple Silicon/Intel
problem; all platforms always listed); unsigned builds (clear Gatekeeper/SmartScreen steps + checksums until
signing is set up); "Open Ghost" when not installed (Download sits next to it); cookies or JS disabled
(download page still works); update signature mismatch (refused); interrupted download (retry); update while
chats run (waits for idle); WebView2 missing on Windows (installer bootstraps it); Windows 10 (no Mica:
fallback theme). Linux, Wayland, AppImage and Debian package qualification are deferred. Explicit
passphrase storage remains available for supported-platform secure-store recovery.
**Onboarding.** No agents found (install suggestions + demo chat); slow detection (cached/skeleton orbs that
reconcile); everything skipped (lands in a usable app); quit halfway (resumes where it left off); no WebGL2
or reduced motion (static, still beautiful); tiny windows and multiple monitors (responsive, minimum size).

## Milestones (each its own branch and PR, opened only when finished and tested)
**Day 1 (done 2026-10-24):** the `auracode.si` zone was created in Cloudflare and Namecheap now delegates
to its nameservers, `asa.ns.cloudflare.com` and `rohin.ns.cloudflare.com`. The zone has been active since
09:03 UTC, and public resolvers serve the delegation. No records are needed until M5
(`download.onghost.dev`) and M6 (the site and mail).

**Foundation (the app)**
- **M0 Scaffold + spikes.** Private repo, pnpm (Homebrew), Cargo + pnpm workspaces, fmt/clippy/Biome,
  Tauri app with the glass window, typed command + Channel round trip, profile layout + SQLite schema,
  stable self-signed signing identity (created locally with the user's OK), batched CI on macOS/Windows
  (fmt, clippy, tests, typecheck, lint, and a smoke run of the real app; installer builds only when
  explicitly dispatched for qualification). Spikes:
  worst-case render (animated aura + orbs + fast stream) on WebKit and WebView2 with idle power; Windows
  spawning (hidden console, git child); Claude control round trip incl. resume after a cwd change; Windows
  titlebar.
- **M1a Core + ACP + bare UI.** Session manager, process lifecycle + env scrub, mock driver + replay
  harness, protocol logs, ACP client, minimal unstyled chat UI. Verified with Hermes and the Claude/Codex
  ACP adapters. In parallel, Design Lab: 2-3 visual directions rendered from recorded streams; the user picks
  one. Protocol follow-ups from the Claude control spike, early in M1a so no driver is built on today's
  shapes: `SessionUpdated` cannot carry 150+ slash commands (about 60 KB) under `MAX_FRAME_BYTES`, and one
  event cannot span frames, so slash commands move to a request/response command (or a chunked event);
  `TokenUsage` splits cache reads from cache writes; a command lifecycle event (queued, started,
  completed, cancelled) feeds the composer queue.
- **M1b Claude native driver** to the contract in the
  [Claude control spike](spikes/claude-control.md#recommended-driver-contract-m1b), including its side of
  the M1a follow-ups: the session actor owns the lifecycle event, so the driver logs `command_lifecycle`
  instead of mapping it; `cache_read_input_tokens` and `cache_creation_input_tokens` fill `TokenUsage`
  separately; and the command list (from `initialize`, then debounced `commands_changed`) is reduced in
  Rust and reaches the UI by M1a's new route. Every spawn sets `CLAUDE_CODE_SDK_READS_SESSION_STATE=1`
  through `LaunchSpec.set_env`. **M1c Codex native driver.**
- **M1d Pi + the rest.** With the user's OK, install pi, oh-my-pi, OpenCode, Cursor CLI, Grok Build via their
  official installers (Cursor/Grok need the user's sign-in), then Pi native driver and the ACP dialects
  (Antigravity, Cursor, Grok, oh-my-pi, OpenCode) + agy/grok fallbacks.
- **M1e Catalog.** Detection on macOS/Windows, channels, versions, vendor updates,
  last-verified badges, auth status. Done when `ghost-cli smoke <agent>` passes for every installed agent.
- **M2a Design system + transcript.** Shared theme tokens, fonts, state-driven agent marks,
  transcript components and accessibility against live protocol fixtures. Retire legacy shader/serif
  dependencies only when no retained Design Lab direction uses them.
- **M2b App shell with real agents.** Sidebar, projects, chats, persistence, titles, edit/retry/fork,
  checkpoints + Changes panel + revert, agent switching with handoff, busy-folder warning, notifications,
  keep-awake, tray, quit guard. Open decision (Windows): wry never marks WebView2 invisible when the
  window is minimized, so Chromium may keep rendering orbs and streams nobody sees, and `document.hidden`
  may never turn true (section D of the [render and power spike](spikes/render-power.md)); once the
  Windows measurement confirms it, the shell hides the webview on minimize and shows it on restore.
  Carried over from M0 because they need the owner's time or hardware, and their results are used
  here: the macOS visible-window render and power run (an unlocked Mac left idle for about 20
  minutes), the manual Windows checklists (spawning, titlebar, render and power; needs a Windows PC),
  and creating the local signing identity (`pnpm signing create`, run by the owner).
- **M2c Composer + palette.** Attachments, @mentions, slash commands, queue/steer, agent/model/effort/
  autonomy pickers, command palette, shortcuts, settings.
- **M3 Boot, onboarding, agents.** Boot cinematic, full onboarding (account step behind a flag until M6),
  Agents page (install, update, sign-in sheet, verified badges).
- **M4 Parallel work.** Worktrees per the rules above, merge/PR/discard, concurrent chats polish,
  multi-folder handling, open in editor/terminal, general terminal panel.
- **M5 Packaging + releases.** DMG (universal) / NSIS (x64, arm64); `tauri-plugin-updater`
  (Tauri signing key generated and backed up; losing it ends updates). Deliberately dispatched CI builds
  acceptance artifacts. Qualified signed installers, `latest.json` and checksums are prepared and verified
  before publication to R2 (`download.onghost.dev`); a tag alone does not authorize publication.
  App updates wait for idle. Distributed binaries use `--remap-path-prefix` to remove build-machine
  paths. Signing, installation and old-build update qualification follow `docs/releases.md`.

**Then (order is the user's call)**
- **M6 Website + accounts.** Rust API on Cloudflare Workers, D1 account/session schema, Cloudflare
  Email Service domain setup, Turnstile, private R2 blobs, site and same-origin API routing. Build
  landing, download, email-code auth, account and legal pages; desktop sign-in and device registry.
  The initial D1/R2 resources already exist; provisioned IDs live in infra/README.md.

- **M7 Workspace sync.** E2E keys, device approval + recovery key, account profiles + seamless switching,
  projects + chats sync, "Continue here".
- **M8 Config Library.** Skills, MCP servers, plugins, instructions for Claude/Codex/Pi; copy across agents.
- **M9 Config sync** for Claude Code, Codex, Pi.
- **Suggested:** Arena (one prompt, N agents, separate worktrees, compare diffs); relay ("Codex reviews what
  Claude wrote"); import past CLI sessions; live remote view from phone/web; dictation; usage dashboard;
  browser-based app sign-in and GitHub sign-in (PKCE + loopback, if added later).

## Sync design (M6-M9)
**Accounts.** The Rust API owns passwordless six-digit email-code auth, through /api/v1/auth.
Use OS/runtime cryptographic randomness and reviewed crypto libraries, never custom cryptography.
Store HMAC-protected code verifiers, a challenge identifier, expiry and bounded attempt counters;
consume codes atomically. Initial defaults: ten-minute code expiry, five attempts per challenge,
60-second resend cooldown, and bounded per-email and per-IP send/verify rates. A resend invalidates
older challenges; generic responses avoid account enumeration. Turnstile protects initiation,
including the desktop flow through its verification UI. Never log codes or email addresses.

Deliver through Cloudflare Email Service, with explicit delivery failures and retry handling;
never create an authenticated session before verification succeeds. Use 32-byte random opaque
session tokens stored only as hashes in D1, with a 30-day absolute expiry and rotation on fresh
sign-in. Browser sessions use Secure, HttpOnly, SameSite cookies, CSRF protection and Origin checks.
Desktop sessions use a separate token stored in the OS keychain. Every authenticated request
checks expiry, account and session status; sync also checks its bound device. Revocation takes
effect on the next request. Device registration binds to the session rather than trusting a caller's
arbitrary device ID. Deep links carry no credentials. Fresh verification protects account deletion.

On sign-out or revoked credentials, retain encrypted local data and halt network sync. Account
deletions revoke all sessions before scheduled data cleanup. Keychain-unavailable systems use an
explicit passphrase-encrypted local store, not a plaintext fallback.

**Data (Cloudflare D1).** `devices` (user, name, platform, public key, last seen, revoked), `records`
(user, collection, id, hlc, device_id, deleted, ciphertext, nonce, key_version, server_seq), `chat_chunks`
(append-only, one per compacted turn), `user_keys` (wrapped keys, KDF params), `quotas`. Push = idempotent
upserts accepted only when the incoming clock is newer (records) or the expected ETag matches (config
blobs); pull = "changes since server_seq". Sync on focus, after each turn, and every 60s. The server never
merges anything. Every query is scoped by the server-validated session account. Use SQLite-compatible migrations,
prepared statements, bounded batches, conditional writes and monotonic server sequence numbers.
Port the merge contract, not PostgreSQL-specific SQL. Add users, auth_challenges and sessions for
the Rust-owned authentication service.

**Encryption (recommended):** one random account key; records sealed with XChaCha20-Poly1305 and AAD =
account|collection|id|version. New devices get the key sealed to their X25519 key after approval on a
signed-in device (matching short code), or from the recovery key shown once at the first device. Cached in
the OS keychain, zeroized in memory. Agent API keys never synced by default. (The site can't read synced
chats; it manages the account.)

**Account switching.** Each account is its own profile (SQLCipher-encrypted database, key in keychain);
switching closes one profile and opens another with an animated transition; running chats ask first. Agent
config follows the account: the account's synced items are applied to the real agent dirs after snapshotting
the outgoing account's managed items (switching back restores them exactly; unmanaged files untouched), or
optionally per account, isolated agent homes (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR`) for
fully separate configs and logins. Sign out keeps the encrypted profile; "remove from this device" wipes it.
First sign-in on a machine with Local data offers "Bring your local projects into this account".

**Workspace sync (projects + chats).** Metadata merges last-writer-wins per field by a hybrid logical clock;
deletes are tombstones; transcripts are append-only and union cleanly (offline continuation on two devices
shows a fork marker). Git folders resolve on a new device by origin URL across configured code roots, else
Clone; local-only folders show "Mac only" with one-click "Publish to GitHub". "Continue here" starts a fresh
native session seeded with the handoff block; optionally the chat's `ghost/<slug>` branch is pushed so code
state follows.

**Config sync (Claude Code, Codex, Pi).** Two snapshots can't tell "installed on the Mac" from "uninstalled
on Windows", so each device keeps its last-synced Base and runs a 3-way merge with tombstones (VS Code
Settings Sync's model).
- Items: a skill, a plugin (`name@marketplace` + enabled; version only if pinned), a marketplace, an MCP
  server (secrets split out), an instructions file, one setting key path, a subagent/command/prompt/hook,
  a Pi extension/theme/package.
- L=R in sync; L=B and R changed: apply remote (incl. deletes); R=B and L changed: push (deletes become
  tombstones); both changed: per-key merge for settings, `diffy::merge` for text files, conflict UI only for
  overlapping edits. First link = union only, with a "Windows gains B, Mac gains C" preview. Items that fail
  to apply go on a per-device "skipped" list so absence is never read as uninstall. Missing agents' items
  wait ("3 Codex items waiting").
- Synced: Claude `settings.json` per key, `CLAUDE.md`, `rules/`, `skills/`, `commands/`, `agents/`,
  `output-styles/`, `keybindings.json`, plugins + marketplaces, `mcpServers` from `~/.claude.json`; Codex
  `config.toml` per key (minus `projects`), `AGENTS.md`, `agents/*.toml`, `rules/`, `hooks.json`; Pi
  `settings.json` per key, `keybindings.json`, `mcp.json`, `models.json` (keys as secrets), `AGENTS.md`,
  `SYSTEM.md`, `APPEND_SYSTEM.md`, `skills/`, `prompts/`, `themes/`, `extensions/`, packages; `~/.agents/skills`
  once.
- Never: credentials, caches, sessions, trust tables, `~/.claude.json` as a whole, `~/.claude/*/synced`.
  Paths normalised to `~`; OS-specific items auto-scoped to their platform; anything can be "this device
  only". Applied through each agent's own CLI/API first, format-preserving edits otherwise, snapshot before
  every apply, dry-run preview, one-click rollback.

## Repo + git workflow
- **One monorepo** (private until it was opened in November 2026) holding the app, the website,
  the API, and the shared packages. Not one long-lived branch per product (they drift apart), and not two
  repos (the app, site, and API share the design system, the generated API types, and the API contract, so
  one PR must be able to change them together). If the app is ever open-sourced, the API can be split out
  then with history intact (`git filter-repo`).
- Repo-local `user.name` and `user.email` (global config untouched).
- `main` gets the initial scaffold commit. Every piece of work happens on a short-lived feature branch
  named after its milestone (`m0-scaffold`, `m1a-core-acp`, ..., `m6-website`), merged through one PR when
  finished and tested. No drafts, review fixes pushed to the same PR, no co-author or generated-by lines.
  PRs via the GitHub MCP, falling back to `gh` (logged in as itscool2b).
- Deploys are separate even though the repo is shared. Batch source validation before the final
  Windows/macOS CI checkpoint. Use isolated Cloudflare preview resources for web and API acceptance.
  A merge or version tag does not publish an unqualified desktop release; follow `docs/releases.md`.
- Cloud changes stay within the owner's authorized scope. The Ghost database/storage creation was
  authorized on 2026-10-26; do not request that permission again.
  New consequential actions outside that scope are confirmed before execution; secrets live in Wrangler/Cloudflare secrets and GitHub Actions secrets, never in git.

## Verification
- **Rust tests:** replay of recorded raw streams (Claude, Codex, Pi, ACP) into expected GhostEvents; Codex
  types validated against generated schemas; detection on fake PATHs (symlinks, `agent` collision, leftover
  dirs, WSL output parsing); channel rules; env scrub; process shutdown chain and leftover cleanup;
  checkpoint/revert in git and non-git folders; handoff budget and delta; merges (LWW, append, 3-way config
  table incl. first-sync union, tombstones, skipped, vendor zones); crypto round trips; each edge case above
  that lives in Rust.
- **Real-agent smoke (opt-in, uses quota):** `ghost-cli smoke <agent>` in a temp repo: stream, tool call,
  approval, interrupt, resume, model switch, fork.
- **App UI:** Vitest for logic; Playwright on the Vite build with mocked Tauri IPC in Chromium and WebKit for
  flows (onboarding, project, chat, stream, approve, diff, interrupt, restart, switch agent, sign in, switch
  account); DOM screenshot baselines with orb canvases masked; one full-app WebDriver smoke on Windows.
- **Web + API:** Rust unit tests plus Worker/Wasm integration tests against local D1/R2 bindings
  (single-use OTP, concurrent verification, session/device revocation, CSRF, rate limits, delivery
  failures, cross-account access, sync conflicts, quota); Playwright against a preview deploy (signup, signin, account, delete,
  download links per OS, no-JS download page); a release dry run that publishes to a staging R2 prefix and
  updates an old build through `latest.json`.
- **Manual:** I run the real app on this Mac with every installed agent and review screenshots for polish;
  the user tests Windows builds as they land. Budgets: 60fps while streaming and scrolling, near-zero idle
  CPU with orbs paused, warm launch interactive in under 300ms, landing page LCP under 2s.
- **CI:** macOS, Windows and Linux, at deliberate integration checkpoints and final verification only.
