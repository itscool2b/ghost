# M1a implementation spec: sessions, processes, the ACP client and the bare chat UI

Branch `m1a-core-acp`, written 2026-10-24 on top of M0 (commit `85c05b7`), revised the same day after two reviews
(a gaps review and a build review). This is the single spec the four M1a implementers build from. It merges three
independent designs (robustness, extensibility, simplicity) into one answer per question; section 0 states each
decision and its reason. "Review decisions" at the end lists the review findings this revision rejected or solved
differently than proposed.

Facts were checked against the sources, not memory: ACP schema `agent-client-protocol-schema` 1.12.0 and the SDK
`agent-client-protocol` 3.5.0 (the scratchpad copies), `@agentclientprotocol/claude-agent-acp` 0.95.0 and
`@agentclientprotocol/codex-acp` 2.2.2 sources plus their npm registry metadata (`npm view`, read-only), the installed
Hermes 0.22.5 (`~/.hermes/hermes-agent`), npm 11.12.1's `libnpmexec` and `@npmcli/config`, Tauri 2.12.5,
process-wrap 10.0.1, tauri-plugin-dialog 2.8.3, this repo's `Cargo.lock`, and the M0 spikes. Installed on this Mac:
Claude Code 2.1.310, codex-cli 0.164.0, Hermes Agent v0.22.5, Node v26.0.0, npm and npx 11.12.1. A scratch crate
compiled the external APIs this spec names on aarch64-apple-darwin and x86_64-pc-windows-gnu (process-wrap with the
features of 1.1, `windows` 0.62.2 with only `Win32_System_Threading`, the direct-child wait of 3.2, tokio signal
handlers, the frozen trait shapes, both select loops); a second probe confirmed that a hand-written `Deserialize`
keeps specta's TypeScript unions closed while `#[serde(other)]` widens them to `string`, and a third listed the 19
`sessionUpdate` tags through `schemars::schema_for!`. No agent received a prompt.

M1a scope: session manager; process lifecycle and env scrub; login-shell env resolution; just enough agent discovery
for Hermes and the Claude and Codex ACP adapters (including their one-time install); mock driver and replay harness;
per-session protocol logs; the ACP client driver; the three protocol follow-ups from the Claude spike; the UI-request
round trip and prompt parts, frozen now and exercised by the mock only; typed Tauri commands and channels; a minimal,
unstyled, fully working chat UI. Out of scope: steering, fork, rewind, attachments and images, @mentions, add-dirs,
checkpoints, worktrees, handoff, agent switching inside a chat, running sign-in flows (`authenticate`), search UI (FTS
is filled), notifications, tray, keep-awake and the App Nap assertion (M2b), WSL, the M1e catalog breadth, Windows
job breakaway for Hermes' gateway (M1e), ACP elicitation, compaction updates,
`session/close|delete|list|fork`, ACP v2.

---

## 0. Decisions

| # | Topic | Decision | Reason (and the proposal it overrules, where they disagreed) |
|---|---|---|---|
| 1 | Crates | New leaf crate `aura-process` (spawn, framing, shutdown, leftover registry, env resolution, scrub, protocol logs). New minimal `aura-agents` (catalog, detection, adapter install, launch). Sessions and persistence stay in `aura-core`. | Drivers, probes, the env resolver and sessions all spawn, and `aura-core` depends on `aura-drivers`, so process code must sit below both. Extensible's inversion (drivers depend on aura-core) drags SQLite into every driver; simple's "drivers never spawn" breaks M1b (flags per session) and M1d (restart with `--model`, one process per turn). |
| 2 | ACP library | Our own JSON-RPC peer and lenient wire types in `aura-drivers`; no runtime dependency on `agent-client-protocol`. `agent-client-protocol-schema =1.12.0` is a dev-dependency for conformance tests. | Every message M1a depends on needs the SDK's untyped path anyway: `SessionUpdate`, `StopReason` and `PermissionOptionKind` have no catch-all, `NewSessionResponse` drops the legacy `models` field, `session/set_model` is gone. One reader loop gives wire ordering, bounded memory and backpressure by construction, without the SDK's handler deadlock class, unbounded queues or uncapped `ByteStreams` lines, and the same peer serves Codex app-server in M1c (JSON-RPC without the `jsonrpc` field). Robust and extensible kept the SDK and added a byte budget, drop guards and `on_receiving_result` plumbing to undo exactly those hazards. |
| 3 | Handlers vs `ActiveSession`; `ByteStreams` vs `AcpAgent` | Neither (follows from 2). The transport is `aura-process`'s pipes and framer. | `AcpAgent` has no cwd option, adds env on top of the inherited env (no scrub), has no job object and kills with SIGKILL after 1 s; `ByteStreams` reads lines with no cap. |
| 4 | Driver interface | An actor: `AgentDriver::run(ctx)`, unbounded command channel in, event channel out bounded by count and bytes. Drivers spawn through an injected `AgentSpawner` that records every process for the session. Exit and shutdown are phases: a driver reads stdout to its end and hands over every event before `run` returns. | Only one bounded direction, so two actors can never wait on each other; the recorded `ProcessControl` lets force stop bypass a wedged driver. All three designs chose an actor. Breaking the loop at the exit dropped buffered output under backpressure (the build review reproduced 320 of 2,000 lines handled). |
| 5 | Turn ownership | The session actor mints turn ids and emits `TurnStarted`, `UserMessageDelta`, `TurnEnded` and `PromptLifecycle`. Drivers report exactly one `TurnEnded` per prompt and tag every event, permission and question with the turn it belongs to; the actor stores by tag. | One place for queueing, crash cleanup and recovery for every driver (all agree). Attribution by position put late updates (and background permission requests) into the next turn. |
| 6 | Permission policy | Drivers forward every request; the session actor decides with one generic function over the dialect's `AutonomyState` and records auto-answers as `PermissionRequested` + `PermissionResolved { by: Auto }`. Questions (`UiRequest`) are never auto-answered. | The same policy then serves ACP, Claude, Codex and Pi; extensible and simple applied it inside each driver. |
| 7 | Full auto limits | Never auto-select `allow_always`; never answer `switch_mode`, kind `other` or a request with no kind; never answer what the agent still asks in its most autonomous mode (Claude's bypass-immune prompts, Hermes' sensitive-path edits). | `allow_always` writes persistent rules into the user's agent settings; agents ask questions through permission requests of kind `other` (Hermes' clarify and guarded-model prompts), so answering them would pick the user's answer for them; the rest are the agent's own safety decisions. Simple auto-answered everything. |
| 8 | Claude without bypass | Full auto falls back to `acceptEdits` with a note and no auto-answers. | The user's own settings removed bypass (`disableBypassPermissionsMode`); answering for them would defeat their policy (simple auto-answered). |
| 9 | Options | One `ConfigOption` list (select or toggle, with a category). ACP config options win; legacy `modes` and `models` become synthesized options; the legacy model is set with an untyped `session/set_model`. A mode picked by hand overrides autonomy's auto-answers. | All agree; the hand-picked rule closes the review's "Ask-mode Hermes still auto-approves" gap. |
| 10 | When an agent starts | On `chat_create`, on `chat_send` to a stopped chat, and on `chat_start` (with "Start fresh" as an option); never just because a chat was opened. Idle agents stop after 10 minutes without a viewer; an agent with work of its own running in the background is not idle. | Opening an old chat to read it must not spawn an adapter and run the user's `SessionStart` hooks (robust started on every subscribe). |
| 11 | Prompt queue | Persisted in `queued_prompts` with a per-row hold. Rows queued before a stop, interrupt, error, crash or restart are held; a prompt typed after that moment is not held but waits behind them, in order, until the person sends or discards the held one (2026-10-28: it used to go first, which skipped the held one after a restart). | Typed text survives a crash, and nothing the user queued before saying stop is sent without them (simple kept the queue in memory and continued after a soft interrupt). One sticky pause flag stranded prompts typed after a stop. |
| 12 | Storage | One writer thread owns the `Store`: typed ops in one FIFO bounded by count and bytes, group commit, atomic ops, poison isolation. Reads go through the same FIFO, so they see every earlier write. While a disk-full pause holds the FIFO, the store serves reads from saved data through a second channel and marks what is unsaved. No read pool in M1a. | Ordering is the correctness property (join protocol, compaction); tokio-rusqlite (extensible) runs closures without typed batching; a read pool can be added behind the same API later. |
| 13 | What is stored | Every event except `SessionUpdated`, `PromptLifecycle` and `LimitsUpdated`; `UsageUpdated` only inside a turn. Events outside turns (late agent output, an idle crash) are stored with no turn. | Live state belongs in snapshots; agent output must never be dropped (simple dropped events outside turns). |
| 14 | Sequence numbers | One space per chat, assigned by the `FrameBatcher` at flush and stored with each row; compaction keeps original seqs. | Frames and rows share one numbering, and `MAX(seq)` never goes backwards. |
| 15 | Join protocol | `chat_attach` flushes, fixes `next_seq = S`, queues the latest-page read in the writer FIFO behind every write below `S`, registers the subscriber, and returns snapshot and page together. | A separate history call races compaction: a turn that ends and is compacted between subscribe and read comes back with its full text at a seq below `S` while its later deltas also arrive live (robust and extensible both had this race). |
| 16 | UI channels | One `Channel<EventBatch>` per open chat view plus one status channel; every subscription of a webview is dropped when its page loads. | Deltas of chats nobody looks at never cross into the webview (simple's single global channel sent them all). |
| 17 | Disk full | Saving pauses with a banner. The writer keeps only the failed batch and stops taking ops, so backpressure reaches the agents as soon as the queues fill; frames keep flowing to open views until then. Nothing is lost unless auracode quits while paused, and quitting then asks first. | An agent's own writes fail on a full disk too, so running ahead buys little. The first draft's 64 MiB in-memory backlog blocked every read once full; robust stalled after 4 MiB; extensible and simple dropped data past a cap. |
| 18 | Large output | One size budget per event (7 KiB of JSON), filled in a fixed order; whenever anything is cut, the complete original goes to a content-addressed blob read back with `chat_blob`. | Frames stay on Tauri's inline path and "full output on disk" (the plan) holds. Independent per-field caps could not keep an event under one frame. |
| 19 | Spawning | The Windows spike's stack: `env_clear` plus the complete env, absolute program path, Unix group leader, Windows Job Object with `CREATE_NO_WINDOW`, for every agent. Windows job breakaway moves to M1e. | process-wrap keeps its job helpers `pub(crate)`, so breakaway means about 150 lines of new `unsafe` Windows code that nothing can exercise in M1a: CI runs Windows but has no Hermes, and there is no Windows PC yet. On Windows, M1a's Hermes works when its gateway is already running and says how to start it otherwise. |
| 20 | Kill rule | The agent's process group plus every descendant the parent-pid walk finds, whatever its session, except daemons the catalog declares for that agent (Hermes' gateway, Claude's daemon), matched by command line, with their subtrees. | A new session is not a daemon marker: PTY hosts (portable-pty) and detached Node children call `setsid` too, so sparing other sessions left dev servers and shells running after Stop. |
| 21 | Leftover registry | One file per running auracode instance (app or CLI), `<data root>/run/<pid>-<start>.json`, with a heartbeat; each entry records the agent's group, its declared daemons and, after every turn, the descendants that left the group. The startup sweep touches only dead instances' records. None on Windows (jobs die with their owner). | A shared file (simple) would let the CLI kill a running app's agents; the heartbeat bounds start times so a recycled group id in a long-lived terminal session is never hit. |
| 22 | Oversized lines | 64 MiB cap; a streaming sniffer keeps the top-level `id` and `method`; an oversized request gets -32603, an oversized response fails its request, an oversized notification becomes a notice. | Nothing ever waits forever on a line we dropped (simple let the request hang until Stop). |
| 23 | Env resolution | `$SHELL -i -l -c` in a new session with nonce markers and `env -0`, 10 s; then `-l -c`, 5 s; then a fallback. The shell starts from auracode's env minus launch-context variables (terminal session, npm, pnpm, Tauri dev). Memory only. Windows: HKLM and HKCU through `winreg`. | `setsid` keeps an interactive shell off the terminal auracode was started from; the retry rescues rc files that hang interactively; launch-context variables made a dev launch leak `npm_config_*` into every agent and replay Apple Terminal's session restore. |
| 24 | Usage scope | `TokenUsage.scope`. claude-acp and codex-acp report per turn; Hermes and unknown agents report session totals, which the driver turns into per-turn deltas within one process. | claude-acp resets `accumulatedUsage` at each user turn (acp-agent.ts lines 4814 and 7023), so robust's "session totals" reading was wrong, while the ACP schema documents session totals. |
| 25 | Slash commands | Out of the event stream: `SessionInfo.revision` changes, the UI fetches `chat_lists`. | Command responses have no frame budget (the spike measured 60 KB lists). |
| 26 | Protocol logs | `<profile>/logs/protocol/<chat_id>/<spawn_id>.jsonl`, written at the process boundary, plus `cmd` records from the driver; redaction in the writer task. | One format for diagnostics and fixtures; `cmd` records make a log replayable. |
| 27 | Replay | The real ACP driver against a fake agent playing a curated protocol log, on paused time, compared with golden `DriverEvent` files. | Explicit `cmd` records cover commands with no wire effect (autonomy changes) that robust's inference from wire traffic would miss. |
| 28 | Auth | Advertise `auth.terminal: true`; never call `authenticate` in M1a; show each sign-in method as a command for the user's own binary. A prompt answered -32000 stops the process; Retry restarts it with native resume and resends the held prompt. | Agents then list their terminal sign-in methods; M3 runs the same commands in its sign-in sheet; auracode never touches credentials. A live process in `AuthRequired` let Retry start a second process on the same native session. |
| 29 | `serde_json` | Enable `preserve_order` workspace-wide. | The schema dev-dependency enables it in test builds; enabling it everywhere keeps tests and releases identical and keeps agents' key order in logs and fixtures. |
| 30 | Profile owner | One process owns a profile's sessions: `<profile>/owner.lock`, taken before the store opens. A second process (a second launch with the same `AURACODE_DATA_DIR`, `aura-cli --data-dir`) fails with a clear message. | Recovery and seq allocation assume one writer per chat; a second process recovering the first one's live turns broke its seq allocation and sent its transcript to the dead-letter file. A read-only second mode would touch every write path for a case nobody needs. |
| 31 | npm adapters | Installed once per version into `<data root>/adapters/<package>-<version>/` with `npm ci --omit=optional --ignore-scripts` from the lockfile committed for that version, then launched as `node <entry>` from there with that folder as the working directory. The chat folder reaches the adapter only as the ACP `cwd`. The user's binary is always forced through `CLAUDE_CODE_EXECUTABLE` or `CODEX_PATH`. | npm resolves packages and `.npmrc` from the working folder (`libnpmexec` runs a local `node_modules` copy whose version matches), so `npx` in a cloned repo would run that repo's code with the user's login. Both adapters fall back to the CLI they bundle when the variable is empty, and the bundled CLIs are optional dependencies, so `--omit=optional` makes that fallback impossible and the download much smaller. A marked install survives an interrupted download. The committed lockfile pins every transitive version and its integrity hash, so the code that runs with the user's login is exactly the reviewed tree. |
| 32 | Frozen extension points | The UI-request round trip (`UiRequest`, `UiResolved`, `RespondUi`) and prompt parts (`Vec<PromptPart>`) are frozen now and exercised by the mock only. Spawns can always set variables (`LaunchSpec.set_env`). | Claude's AskUserQuestion, Codex user input, Pi's `extension_ui_request` and Cursor's `cursor/ask_question` need a question and an answer; images and mentions arrive in M2c; M1b must set `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` on every spawn. Adding them later would reopen the protocol, the stored vocabulary, the replay format and every driver. |

---

## 1. Crates, modules and dependency direction

```text
crate              depends on (inside the workspace)
aura-protocol      nothing
aura-process       nothing                      (knows no AuraEvent; tested against plain children)
aura-agents        aura-protocol, aura-process
aura-drivers       aura-protocol, aura-process
aura-core          aura-protocol, aura-process, aura-agents, aura-drivers
aura-cli           aura-core, aura-agents, aura-drivers (feature replay), aura-process    (phase 2, headless)
auracode-desktop   aura-core, aura-protocol, aura-drivers, aura-process, aura-agents (startup wiring)
```

`aura-core` still has no Tauri dependency. `aura-core` gains `tokio`; `Store` stays synchronous and moves into the
writer thread. Every constructor that spawns tasks takes a `tokio::runtime::Handle` instead of assuming a runtime
context, because Tauri's `setup` hook and run-event callbacks run on the main thread outside any Tokio context.

### 1.1 New workspace dependencies (exact; added only in P0, so `Cargo.lock` changes nowhere else)

| Crate | Version | Features | Used by |
|---|---|---|---|
| `process-wrap` | `=10.0.1` | default off; `tokio1`, `creation-flags`, `job-object`, `kill-on-drop`, `process-group`, `process-session`, `reset-sigmask` | aura-process |
| `nix` (Unix) | `0.31.3` | `signal`, `process`, `user` | aura-process (`killpg`, `getsid`, the passwd shell) |
| `windows` (Windows) | `0.62.2` | `Win32_System_Threading` | aura-process (only the `CREATE_NO_WINDOW` constant `CreationFlags` takes) |
| `winreg` (Windows) | `0.56.0` | default | aura-process (registry env, safe API) |
| `regex` | `1.13.1` | default | aura-process (redaction), aura-drivers (fixture scan) |
| `sha2` | `0.10.9` | default | aura-core (blob addressing, reply hashes), aura-drivers (long `toolCallId`s, replay summary) |
| `futures` | `0.3.34` | `std` | aura-process, aura-drivers, aura-core (`BoxFuture`, `FuturesUnordered`) |
| `tokio` (existing) | `1.54.1` | add `signal` | workspace (the desktop's quit signals) |
| `unicode-segmentation` | `1.13.3` | default | already in aura-drivers |
| `tauri-plugin-dialog` | `=2.8.3` | default | desktop (folder picker and the quit question, called from Rust only) |
| `agent-client-protocol-schema` | `=1.12.0` | default (`schemars`) plus `unstable` | aura-drivers, dev only |
| `serde_json` (existing) | `1.0.154` | add `preserve_order` | workspace |

Workspace-internal and existing dependencies each crate gains, all in P0: `aura-process` uses `tokio`, `futures`,
`serde`, `serde_json`, `thiserror`, `uuid`, `tracing`, `regex`, `nix`, `windows`, `winreg`, `process-wrap`, and has a
feature `testing`; `aura-agents` uses `aura-protocol`, `aura-process`, `tokio`, `serde`, `serde_json`, `thiserror`,
`tracing`; `aura-drivers` gains `aura-process`, `serde`, `thiserror`, `uuid`, `sha2`, `regex`, `futures`, and a feature
`replay = ["testing", "aura-process/testing", "tokio/test-util"]` that only `aura-cli` and dev-dependencies enable, plus
`schemars` 1.2.3 as a dev-dependency (for `schema_for!`); `aura-core`
gains `tokio`, `futures`, `sha2`, `aura-process`, `aura-agents`, `aura-drivers`, with `aura-process/testing` and
`aura-drivers` with `testing` and `replay` as dev-dependencies (aura-core uses no replay: through feature unification
the dev-dependency makes every `--workspace` test and clippy run, CI's included, compile and run the replay harness
and its tests); the desktop gains `aura-process`, `aura-agents`, `tauri-plugin-dialog`, with `aura-process/testing`
and `aura-drivers/testing` as dev-dependencies (D's Rust tests of the quit flow, page loads and signals). `sha2`,
`regex`, `winreg`, `windows` 0.62.2, `schemars` 1.2.3 and `futures-util` 0.3.34 are already in `Cargo.lock`. P0 runs
the whole suite right after enabling `preserve_order`.

### 1.2 Module trees (new files unless marked)

```text
crates/aura-protocol/src/
  lib.rs              events, frames, tool, permission, question, usage types (changed, section 2)
  session.rs          IPC and session-state types (2.3)
  lenient.rs          the lenient_enum! macro (2.4)

crates/aura-process/src/
  lib.rs              re-exports, constants (appendix A)
  spawn.rs            LaunchSpec, ProcessCwd, DaemonMatch, SpawnSpec, ProcessSpawner, Spawner, SpawnedProcess,
                      SpawnError, HelperSpec, HelperEnv, HelperOutput, run_helper
  io.rs               AgentIo, LineSink, LineSource, StdoutItem, TopKeys
  framing.rs          bounded line framer (ported from docs/spikes/claude-control/src/framing.rs), noise filter
  sniff.rs            streaming top-level JSON key sniffer
  stderr.rs           stderr drain and tail
  control.rs          ProcessControl, ControlImpl, ExitInfo, the direct-child waiter
  shutdown.rs         ShutdownPlan, ShutdownReport, ShutdownStep, the chain, the kill rule
  ptable/mod.rs       cfg(unix): ProcInfo, StartTime, snapshot, start_time, descendants, boot_marker, daemon matching
  ptable/macos.rs     `ps` parsing      ptable/linux.rs  /proc parsing
  registry.rs         cfg(unix): LeftoverRegistry, InstanceRecord, LeftoverEntry, EscapedProc, sweep_stale,
                      SweepReport (Windows: a disabled registry)
  env/mod.rs          EnvKey, EnvMap, ResolvedEnv, EnvSource, EnvResolver, ResolverStatus, child_env, process_env,
                      strip_launch_context
  env/shell.rs        login shell resolution, parse_env0
  env/windows.rs      registry merge, expand_env_strings, merge_windows_path
  env/scrub.rs        the denylist (one place), scrub, deny_reason
  env/which.rs        find_program, find_exe
  log/mod.rs          LogFactory, ProtocolLog, LogNote, Direction, LogLimits, RetentionPolicy
  log/redact.rs       Redactor          log/writer.rs  writer task, rotation        log/retention.rs  prune
  testing.rs          feature "testing": FakeSpawner, FakeAgentEnd, FakeLine, FakeControl
  bin/ghost-testbin.rs test child (modes in 9.3)

crates/aura-agents/src/
  lib.rs, catalog.rs, detect.rs, launch.rs, install.rs (npm adapters)

crates/aura-drivers/src/
  batch.rs            (exists) `session_id` -> `chat_id`, DeltaKind::User, tagged push and flush
  mock.rs             (exists) rename fallout, event-level `play`, new MockSteps
  driver.rs           the frozen driver contract (5.1), EventSender, EventReceiver
  jsonrpc.rs          the JSON-RPC peer (5.2)
  acp/mod.rs          AcpDriver: setup, main loop, phases
  acp/wire.rs         lenient inbound types
  acp/map.rs          session/update -> events (pure, with the id minter), MAPPED_TAGS
  acp/options.rs      option model and routes
  acp/dialect.rs      AcpDialect, generic, claude-acp, codex-acp, hermes
  acp/permissions.rs  pending requests, withdrawal
  mock_driver.rs      MockDriver
  factory.rs          StandardDriverFactory
  testing.rs          feature "testing": ScriptedDriver, ScriptHandle, ScriptedDriverFactory, DirectSpawner
  replay/mod.rs, replay/fixture.rs, replay/fake_agent.rs, replay/golden.rs, replay/curate.rs   (feature replay)
tests/conformance.rs, tests/replay.rs, tests/fixtures/acp/**

crates/aura-core/
  migrations/0002_sessions.sql
  src/owner.rs        ProfileOwner (the profile's owner.lock)
  src/store.rs        (exists) SCHEMA_VERSION = 2, MIGRATIONS gains 0002
  src/chatstore/mod.rs     ChatStore, Op, records, StoreError
  src/chatstore/writer.rs  the writer thread, the read channel
  src/chatstore/rows.rs    SQL for chats, folders, agent_sessions, turns, events, queue
  src/chatstore/history.rs page assembly
  src/chatstore/compact.rs pure compaction and FTS rows
  src/chatstore/recover.rs startup and per-chat recovery
  src/chatstore/blobs.rs   blob files
  src/session/mod.rs       SessionManager, ManagerDeps, limits, sinks, SessionError
  src/session/actor.rs     ChatActor
  src/session/policy.rs    permission decision
  src/session/normalize.rs event budget, blob spill, open-block bookkeeping
  src/session/spawner.rs   SessionSpawner
  src/session/live.rs      live-agent cap, native-session uniqueness

apps/desktop/src-tauri/src/
  sessions.rs         channel sinks, startup wiring, page-load hook, quit flow, signals, folder picker
  commands.rs, state.rs, lib.rs, error.rs, bindings.rs, boot.rs   (changed)
apps/desktop/src/
  lib/ipc.ts (additions), lib/chat/{transport,transcript,chatStore,appStore}.ts (+ tests)
  components/chat/{ChatList,NewChat,ChatView,Header,Pickers,Transcript,TurnView,ToolBlock,PermissionCard,
                   QuestionCard,ErrorCard,AuthCard,Composer,QueueList,SlashMenu,Banners}.tsx
  App.tsx (chats as the default view; M0 info card and stress view under Diagnostics)
```

---

## 2. aura-protocol changes, exactly

All types derive `Debug, Clone, PartialEq, Serialize, specta::Type` (plus `Eq`, `Copy`, `Default` where possible) and
`Deserialize` (derived, except the lenient enums of 2.4). Enums use `#[serde(rename_all = "snake_case")]`;
data-carrying enums are internally tagged with `"type"` unless noted. Nothing has been persisted before M1a, so
renames break no stored data. From M1a on, any field added to a stored type must be `Option` with
`#[serde(default)]`; history loading reads unknown enum values leniently (2.4) and skips (and counts) rows that still
do not parse, except `turn_ended` rows (6.3.2).

### 2.1 Events (`lib.rs`)

```rust
pub type Millis = i64;
pub const MAX_FRAME_BYTES: usize = 8 * 1024;                       // unchanged
pub const EVENT_BUDGET: usize = MAX_FRAME_BYTES - 1024;            // NEW: every non-delta event after normalizing

#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuraEvent {
    TurnStarted { turn_id: String, prompt_id: Option<String> },                              // + prompt_id
    TurnEnded { turn_id: String, stop_reason: StopReason, usage: Option<TokenUsage>,
                duration_ms: Option<u64>,                                                    // + duration_ms
                recovered: bool },                                                           // + recovered (6.3.3)
    UserMessageDelta { message_id: String, text: String },                                   // NEW
    MessageDelta { message_id: String, text: String },
    MessageCompleted { message_id: String },
    ThoughtDelta { thought_id: String, text: String },
    ThoughtCompleted { thought_id: String, duration_ms: Option<u32> },
    ToolCall(ToolCall),                                                                      // changed struct
    ToolUpdate(ToolUpdate),                                                                  // changed struct
    PermissionRequested(PermissionRequest),                                                  // changed struct
    PermissionResolved { request_id: String, decision: PermissionDecision, by: ResolvedBy }, // + by
    UiRequest(UiRequest),                                                                    // M1a: the mock's /ask
    UiResolved { request_id: String, answer: Option<UiAnswer>, by: ResolvedBy },             // NEW (None = dismissed)
    PlanUpdated { entries: Vec<PlanEntry> },
    UsageUpdated(UsageInfo),                                                                 // u64 fields
    LimitsUpdated { windows: Vec<LimitWindow> },
    SessionUpdated(SessionInfo),                                                             // new payload
    PromptLifecycle { prompt_id: String, state: PromptState, preview: Option<String> },      // NEW
    Notice { level: NoticeLevel, message: String },
    Error { kind: ErrorKind, message: String, detail: Option<String>, recoverable: bool },   // + kind, detail
}

impl AuraEvent {
    /// The serde tag ("message_delta", ...), stored in `events.kind`.
    pub fn kind(&self) -> &'static str;
    /// Stored as a history row? `in_turn`: the event is tagged with a turn (5.1).
    /// false for SessionUpdated, PromptLifecycle, LimitsUpdated; UsageUpdated only when in_turn; else true.
    pub fn is_stored(&self, in_turn: bool) -> bool;
}

pub enum StopReason { EndTurn, MaxTokens, MaxTurnRequests, Refusal,
                      Cancelled,      // the agent confirmed a cancel
                      Interrupted,    // NEW: auracode ended the turn without the agent (force stop, crash, quit)
                      Error,
                      Other }         // NEW: a reason auracode does not know (an agent's or a newer build's)

pub struct TokenUsage {
    pub input_tokens: u64,                 // u32 -> u64
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,    // renamed from cached_input_tokens (ACP cachedReadTokens,
                                           // Claude cache_read_input_tokens)
    pub cache_write_tokens: Option<u64>,   // NEW (ACP cachedWriteTokens, Claude cache_creation_input_tokens)
    pub reasoning_tokens: Option<u64>,     // ACP thoughtTokens
    pub scope: UsageScope,                 // NEW
}
pub enum UsageScope { Turn, Session }

pub struct ToolCall {
    pub tool_call_id: String,
    pub kind: ToolKind,
    pub title: String,
    pub status: ToolStatus,
    pub locations: Vec<ToolLocation>,
    pub raw_input: Option<serde_json::Value>,
    pub tool_name: Option<String>,         // NEW (ACP ToolCall.name)
    pub content: Vec<ToolContent>,         // NEW (ACP tool calls carry content from the start)
    pub truncated: Option<Truncation>,     // NEW: more than entry text was cut (6.7); `blob` = the original event JSON
}

/// Mirrors ACP's update: `None` keeps a field; `content: Some(v)` replaces the content, `Some([])` clears it.
pub struct ToolUpdate {
    pub tool_call_id: String,
    pub status: Option<ToolStatus>,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,                       // NEW
    pub locations: Option<Vec<ToolLocation>>,         // NEW
    pub raw_input: Option<serde_json::Value>,         // NEW
    pub tool_name: Option<String>,                    // NEW (ACP ToolCallUpdateFields.name)
    pub content: Option<Vec<ToolContent>>,            // was Vec (where empty meant unchanged)
    pub truncated: Option<Truncation>,                // NEW, as on ToolCall
}

pub enum ToolKind { Read, Edit, Delete, Move, Search, Execute, Think, Fetch, SwitchMode, Other }   // lenient

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text { text: String, truncated: Option<Truncation> },                              // + truncated
    Diff { path: String, old_text: Option<String>, new_text: String,
           truncated: Option<Truncation> },                                             // + truncated
    Terminal { output: String, exit_code: Option<i32>, truncated: Option<Truncation> },  // bool -> Option
}
/// Inline content was cut; `blob` is the sha256 of the full content in the profile's attachments.
pub struct Truncation { pub total_bytes: u64, pub blob: Option<String> }

pub struct PermissionRequest {
    pub request_id: String,
    pub tool_call_id: Option<String>,             // set only when it names a tool block already shown
    pub title: String,
    pub kind: Option<ToolKind>,                   // NEW (the policy reads it)
    pub raw_input: Option<serde_json::Value>,     // NEW (Hermes' permission tool calls never become blocks)
    pub content: Vec<ToolContent>,                // NEW
    pub locations: Vec<ToolLocation>,             // NEW
    pub options: Vec<PermissionOption>,
    pub truncated: Option<Truncation>,            // NEW, as on ToolCall
}
pub enum PermissionOptionKind { AllowOnce, AllowAlways, RejectOnce, RejectAlways, Other }          // lenient, + Other
pub enum ResolvedBy { User, Auto, Agent, Turn, System, Other }   // NEW: who answered (Agent = withdrawn,
                                                                 // Turn = turn ended or cancelled, System = recovery
                                                                 // or the agent's process ended); lenient

/// The answer to a `UiRequest`, matching its `UiPrompt` (`Notify` takes none).
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiAnswer { Confirm { yes: bool }, Select { index: u32 }, Input { text: String } }          // NEW

/// One piece of a prompt. M1a sends `Text` only; M2c adds images and mentions.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptPart { Text { text: String }, Image { blob: String, mime: String }, Mention { path: String } } // NEW

pub struct UsageInfo { pub context_used: Option<u64>, pub context_size: Option<u64>, pub cost: Option<Cost> }

/// Live header state of a chat. Never stored; coalesced to at most one per frame.
pub struct SessionInfo {
    pub status: SessionStatus,
    pub detail: Option<String>,               // "Starting Hermes", "Stopping the turn; press Esc again to force stop"
    pub native_session_id: Option<String>,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,                // display labels of the current Model, Effort, Mode choices
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub autonomy: Autonomy,
    pub queue_held: bool,                     // some queued prompts are held ("Send queued" shows)
    /// Increments when capabilities, options, slash commands or the autonomy state change:
    /// the UI refetches `chat_lists`.
    pub revision: u32,
}                                                        // `commands` removed
#[derive(Default)]
pub enum SessionStatus { #[default] Stopped, Starting, Ready, Running, Cancelling, Stopping, Failed, AuthRequired }
pub enum PromptState { Queued, Started, Completed, Cancelled, Failed }
pub enum NoticeLevel { Info, Warning, Error }                                        // + Error; lenient
pub enum ErrorKind { AgentUnavailable, FolderMissing, FolderNotAllowed, SpawnFailed, StartTimeout,
                     ProtocolMismatch, AuthRequired, AgentError, AgentExited, ProtocolError, Storage,
                     Internal, Other }                                               // lenient

/// What the dialect made of the requested autonomy. The session's permission policy reads `auto_answer`.
pub struct AutonomyState {
    pub requested: Autonomy,
    pub native_mode: Option<String>,          // the agent mode in effect, if auracode knows it
    pub auto_answer: AutoAnswer,
    pub note: Option<String>,                 // shown next to the autonomy picker
}
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutoAnswer { Never, All, Kinds { kinds: Vec<ToolKind> } }    // All = the eight tool kinds only (6.6)

pub enum DriverKind { Acp, Mock }                       // M1b to M1d add ClaudeStream, CodexAppServer, PiRpc

pub struct EventBatch { pub chat_id: String, pub seq_start: u32, pub events: Vec<AuraEvent> } // session_id -> chat_id
```

Unchanged in shape: `Autonomy` (default `FullAuto`), `Capabilities`, `ToolStatus` (lenient), `ToolLocation`,
`PermissionOption`, `PermissionDecision`, `UiRequest`, `UiPrompt`, `PlanEntry`, `PlanStatus` and `PlanPriority`
(both lenient), `Cost`, `LimitWindow`, `SlashCommand`. `SessionInfo.commands` is gone; slash commands travel in
`SessionLists` (2.3).

### 2.2 Config options (`session.rs`)

```rust
pub enum ConfigCategory { Model, Effort, Mode, ModelConfig, Other }                   // lenient
pub struct ConfigOption {
    pub id: String,                          // opaque to the UI; the driver maps it back to the wire
    pub name: String,
    pub description: Option<String>,
    pub category: ConfigCategory,
    pub kind: ConfigKind,
    pub pending: Option<ConfigValue>,        // a change waiting for the turn to end (Hermes mode)
}
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigKind { Select { current: String, choices: Vec<ConfigChoice> }, Toggle { current: bool } }
pub struct ConfigChoice { pub value: String, pub name: String, pub description: Option<String>, pub group: Option<String> }
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigValue { Choice { value: String }, Toggle { value: bool } }
```

### 2.3 IPC and session types (`session.rs`)

```rust
pub struct SessionLists { pub revision: u32, pub capabilities: Option<Capabilities>, pub options: Vec<ConfigOption>,
                          pub commands: Vec<SlashCommand>, pub autonomy: AutonomyState }
pub enum FolderState { Ok, Missing, NotAllowed }          // NotAllowed: the OS refused access (macOS privacy)
pub struct ChatSummary { pub id: String, pub title: Option<String>, pub agent_id: String, pub agent_name: String,
                         pub folder: String, pub folder_state: FolderState, pub status: SessionStatus,
                         pub needs_attention: bool, pub created_at: Millis, pub updated_at: Millis }
pub struct ChatStatusUpdate { pub chat_id: String, pub status: SessionStatus, pub needs_attention: bool,
                              pub title: Option<String>, pub updated_at: Millis }
pub struct ChatAttach { pub subscription_id: String, pub next_seq: u32, pub snapshot: ChatSnapshot,
                        pub history: HistoryPage }
pub struct ChatSnapshot { pub chat: ChatSummary, pub session: SessionInfo, pub queue: Vec<QueuedPrompt>,
                          pub open_turn_id: Option<String>, pub usage: Option<UsageInfo>,
                          pub limits: Option<Vec<LimitWindow>>,     // NEW: the last LimitsUpdated (never stored)
                          pub last_error: Option<ErrorInfo>, pub auth: Option<AuthPrompt>,
                          pub storage: StorageStatus }
pub struct HistoryPage { pub chat_id: String, pub items: Vec<HistoryItem>, pub has_more: bool,
                         pub before_seq: Option<u32>,      // pass to chat_history for the next older page
                         pub cut_turn_id: Option<String>,  // the oldest items belong to this turn, whose earlier
                                                           // part is in the next older page
                         pub unsaved_from: Option<u32>,    // saving is paused: rows from this seq up to the page's
                                                           // cursor are not saved yet and not in the page
                         pub skipped: u32 }                // rows that did not parse (written by a newer build)
pub struct HistoryItem { pub seq: u32, pub turn_id: Option<String>, pub at: Millis, pub event: AuraEvent }
pub struct PromptAck { pub prompt_id: String, pub queued: bool }
pub struct QueuedPrompt { pub prompt_id: String, pub preview: String, pub held: bool, pub created_at: Millis }
pub struct ErrorInfo { pub kind: ErrorKind, pub message: String, pub detail: Option<String> }
pub struct AuthPrompt { pub message: String, pub commands: Vec<AuthCommand> }
pub struct AuthCommand { pub label: String, pub argv: Vec<String>,
                         pub env: Vec<(String, String)> }  // NEW: variables the method sets (ACP AuthMethodTerminal.env)
pub struct AgentEntry { pub id: String, pub name: String, pub description: String, pub availability: Availability,
                        pub launch: String, pub needs_download: bool,   // NEW: the first start downloads the adapter
                        pub last_verified: String }
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability { Checking, Ready { version: Option<String>, path: String },
                        Missing { reason: String }, Broken { reason: String } }
pub struct EnvStatus { pub state: EnvState, pub removed: Vec<String> }        // names only, never values
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnvState { Resolving, Ready { source: String, took_ms: u32, vars: u32 }, Fallback { reason: String } }
pub struct StorageStatus { pub state: StorageState, pub unsaved_bytes: u64 }
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StorageState { Ok, Paused { reason: PauseReason, since: Millis }, Failed { message: String } }
pub enum PauseReason { DiskFull, Busy }
pub struct BlobText { pub sha256: String, pub offset: u64,
                      pub next_offset: u64,                // NEW: where the next page starts (a character boundary)
                      pub total: u64, pub text: String, pub eof: bool }
```

### 2.4 Lenient enums and other consequences

- **Lenient enums.** `StopReason`, `ToolKind`, `ToolStatus`, `PermissionOptionKind`, `ResolvedBy`, `PlanStatus`,
  `PlanPriority`, `NoticeLevel`, `ErrorKind`, `UsageScope` and `ConfigCategory` derive `Serialize` and get a
  hand-written `Deserialize` from one macro, `lenient_enum!` in `lenient.rs`, that maps an unknown string to the
  enum's fallback: `Other` where the enum has it, else `ToolStatus` and `PlanStatus` to `Pending`, `PlanPriority` to
  `Medium`, `NoticeLevel` to `Info`, `UsageScope` to `Session`. Never `#[serde(other)]`: through tauri-specta's export
  path it widens the TypeScript union to `string` and drops `"other"` (checked with specta rc.26 and specta-serde
  0.0.13), which defeats the UI's exhaustive switches. Data-carrying enums (`AuraEvent`, `ToolContent`, `UiPrompt`,
  `UiAnswer`, `PermissionDecision`) stay derived; a row with an unknown variant is skipped and counted.
- `aura-drivers/batch.rs`: `chat_id`; `DeltaKind::User` so `UserMessageDelta` coalesces and splits like the other
  deltas; `push_tagged(event, turn: Option<Arc<str>>)` and `flush_tagged() -> Vec<(EventBatch, Vec<Option<Arc<str>>>)>`
  carry each event's turn tag through merging (only adjacent deltas with equal tags merge) and splitting (pieces keep
  the tag); `push` and `flush` stay as they are for the M0 demo stream. `mock.rs`: `TokenUsage` and `ToolUpdate`
  fallout; `SessionUpdated` carries the new `SessionInfo`. `apps/desktop/src-tauri/src/demo.rs`: field rename.
- `bindings.ts` regenerates (tauri-specta, `disable_serde_phases`, bigints cast to number so `u64` is `number`).
  `lib/stream.ts` and `lib/session.ts` (M0 demo) take the rename and `content: ToolContent[] | null`.
- New reducers end their `switch` on `event.type` with `const unreachable: never = event`.
- Tests: round trip of every variant; `is_stored` for every variant; `kind()` equals the serde tag for every
  variant; every lenient enum maps an unknown string to its fallback and still rejects a non-string. The UI-facing
  types are checked through the desktop's bindings test (the real export path, which casts bigints); the aura-core
  export test refuses `i64` and is not used for them.

---

## 3. aura-process: spawning, framing, shutdown, leftovers, env, logs

### 3.1 Public API (frozen)

```rust
// spawn.rs
/// Where the agent process runs. An ACP agent always gets the chat folder as the protocol's `cwd` (5.3.1).
pub enum ProcessCwd { ChatFolder, Fixed(PathBuf) }

/// A descendant that is a vendor daemon and must survive shutdown (3.4): argv[0]'s file name equals `program`
/// when set, and the words of `args_in_order` appear in argv in that order (not necessarily adjacent).
#[derive(Serialize, Deserialize)]
pub struct DaemonMatch { pub program: Option<String>, pub args_in_order: Vec<String> }

pub struct LaunchSpec {
    pub label: String,                          // "hermes acp", for logs and status text
    pub program: PathBuf,                       // absolute; symlinks are not resolved
    pub args: Vec<OsString>,
    pub cwd: ProcessCwd,
    pub set_env: Vec<(OsString, OsString)>,     // always set, after the scrub; names logged, values never
    pub daemons: Vec<DaemonMatch>,              // spared by the kill rule
    pub startup_timeout: Duration,
}

pub struct SpawnSpec { pub launch: LaunchSpec, pub folder: PathBuf, pub env: Arc<ResolvedEnv>, pub max_line_bytes: usize }

pub trait ProcessSpawner: Send + Sync + 'static {
    /// Spawns an agent with all three pipes; `log` receives every stdin, stdout and stderr line.
    fn spawn(&self, spec: SpawnSpec, log: ProtocolLog) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>>;
}

pub struct Spawner { /* Arc<LeftoverRegistry>, runtime handle */ }
impl Spawner { pub fn new(registry: Arc<LeftoverRegistry>, rt: Handle) -> Arc<Spawner>;
               pub fn registry(&self) -> &Arc<LeftoverRegistry>; }
impl ProcessSpawner for Spawner {}

pub struct SpawnedProcess { pub spawn_id: String, pub io: AgentIo, pub control: ProcessControl, pub log: ProtocolLog }

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("{program} was not found")]                       ProgramNotFound { program: String },
    #[error("{program} can't be run: permission denied")]     PermissionDenied { program: String },
    #[error("the folder {} can't be used: {reason}", path.display())] InvalidCwd { path: PathBuf, reason: String },
    #[error("auracode isn't allowed to open {}", path.display())]      CwdNotAllowed { path: PathBuf },
    #[error("spawning needs a Tokio runtime")]                NoRuntime,
    #[error("could not start {program}")]                     Spawn { program: String, #[source] source: io::Error },
}

/// `ProcessEnv`: auracode's own env minus `strip_launch_context` (the env resolver's shell). Both are scrubbed.
pub enum HelperEnv { ProcessEnv, Resolved(Arc<ResolvedEnv>) }
pub struct HelperSpec { pub program: PathBuf, pub args: Vec<OsString>, pub cwd: PathBuf, pub env: HelperEnv,
                        pub set: Vec<(OsString, OsString)>,   // always set, after the scrub
                        pub new_session: bool, pub timeout: Duration, pub max_stdout: usize }
pub struct HelperOutput { pub exit: Option<ExitInfo>, pub stdout: Vec<u8>, pub stdout_truncated: bool,
                          pub stderr_tail: String, pub timed_out: bool }
/// Short-lived helpers (the env resolver's shell, `--version` probes, the adapter install): stdin /dev/null,
/// registered for the sweep, the whole tree killed on timeout and when the returned future is dropped.
pub async fn run_helper(registry: &LeftoverRegistry, spec: HelperSpec) -> Result<HelperOutput, SpawnError>;

// io.rs
pub struct AgentIo { pub stdin: LineSink, pub stdout: LineSource }
#[derive(Clone)] pub struct LineSink { /* unbounded sender to the stdin writer task */ }
impl LineSink {
    /// Queues one line (no trailing newline). Never blocks: a writer task writes `line\n` and flushes.
    pub fn send(&self, line: String) -> Result<(), StdinClosed>;
    /// End of file after the queued lines.
    pub fn close(&self);
}
pub struct StdinClosed;
pub struct LineSource { /* receiver bounded at 16 lines and 8 MiB (a longer line passes alone) */ }
impl LineSource { pub async fn next(&mut self) -> Option<StdoutItem>; }   // cancel-safe; None at EOF
pub enum StdoutItem { Line(String), Oversized { bytes: u64, keys: TopKeys } }
pub struct TopKeys { pub id: Option<serde_json::Value>, pub method: Option<String>, pub kind: Option<String> }

// control.rs
pub trait ControlImpl: Send + Sync + 'static {
    fn pid(&self) -> u32;
    fn close_stdin(&self);
    /// Immediate and callable from any thread, with or without a runtime (3.4).
    fn kill_tree(&self);
    fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport>;  // idempotent, shared
    /// The direct child's exit on every OS: never a job-wide or group-wide wait (3.2).
    fn exited(&self) -> BoxFuture<'static, ExitInfo>;
    fn try_exited(&self) -> Option<ExitInfo>;
    /// Unix: one process-table snapshot (on a blocking thread) recording, in the leftover registry, the descendants
    /// that left the agent's group, minus declared daemons. Windows: nothing.
    fn record_escaped(&self) -> BoxFuture<'static, ()>;
    fn stderr_tail(&self, max_bytes: usize) -> String;           // lossy, ANSI stripped, NOT redacted
}
#[derive(Clone)] pub struct ProcessControl(Arc<dyn ControlImpl>);   // methods forward; the last drop kills the tree
impl ProcessControl { pub fn new(imp: Arc<dyn ControlImpl>) -> ProcessControl; }
pub struct ExitInfo { pub code: Option<i32>, pub signal: Option<i32>, pub at_ms: i64 }

// shutdown.rs
pub struct ShutdownPlan { pub stdin_eof_grace: Duration, pub sigterm_grace: Duration, pub kill_confirm: Duration }
impl ShutdownPlan {
    pub const STANDARD: ShutdownPlan;   // 5 s, 3 s, 2 s
    pub const QUIT: ShutdownPlan;       // 3 s, 1 s, 2 s
    pub const FORCE: ShutdownPlan;      // 0 (skip EOF), 2 s, 2 s
}
pub struct ShutdownReport { pub exit: Option<ExitInfo>, pub steps: Vec<ShutdownStep>, pub leftovers: Vec<u32>,
                            pub spared: Vec<u32>, pub took: Duration }
pub enum ShutdownStep { AlreadyExited, ClosedStdin, ExitedAfterEof, SentSigterm { processes: u32 },
                        ExitedAfterSigterm, Killed { processes: u32 }, TerminatedJob }

// ptable/mod.rs (cfg(unix))
pub struct ProcInfo { pub pid: u32, pub ppid: u32, pub pgid: u32, pub sid: Option<u32>, pub start: StartTime,
                      pub argv: Vec<String> }
pub enum StartTime { LinuxTicks { ticks: u64, boot_id: String }, MacLstart(String), Unknown }
pub fn snapshot() -> io::Result<Vec<ProcInfo>>;
pub fn start_time(pid: u32) -> io::Result<StartTime>;
pub fn descendants(table: &[ProcInfo], root: u32) -> Vec<ProcInfo>;
pub fn matches_daemon(info: &ProcInfo, daemons: &[DaemonMatch]) -> bool;    // pure
pub fn boot_marker() -> Option<String>;          // Linux boot_id, macOS kern.boottime
pub fn parse_ps(text: &str) -> Vec<ProcInfo>;    // pure, tested with captured output
pub fn parse_proc_stat(pid: u32, stat: &str, cmdline: &[u8], boot_id: &str) -> Option<ProcInfo>;

// registry.rs (cfg(unix); on Windows `open` returns `disabled()` and `sweep_stale` does nothing)
pub struct InstanceRecord { pub pid: u32, pub start: StartTime, pub sid: Option<u32>, pub boot: Option<String> }
pub struct EscapedProc { pub pid: u32, pub start: StartTime }
pub struct LeftoverEntry { pub pid: u32, pub pgid: u32, pub sid: Option<u32>, pub start: StartTime,
                           pub label: String, pub spawned_at_ms: i64, pub daemons: Vec<DaemonMatch>,
                           pub escaped: Vec<EscapedProc> }
pub struct LeftoverRegistry { /* our file, entries, heartbeat task */ }
impl LeftoverRegistry {
    pub fn open(run_dir: &Path, rt: Handle) -> io::Result<Arc<LeftoverRegistry>>;
    pub fn disabled() -> Arc<LeftoverRegistry>;
    pub fn own(&self) -> &InstanceRecord;
    pub fn register(&self, entry: LeftoverEntry) -> io::Result<()>;
    pub fn set_escaped(&self, pid: u32, escaped: Vec<EscapedProc>);
    pub fn unregister(&self, pid: u32);
    pub fn close(&self) -> io::Result<()>;          // deletes our file at a clean exit
}
pub async fn sweep_stale(run_dir: &Path, own: &InstanceRecord) -> SweepReport;
pub struct SweepReport { pub files_removed: u32, pub killed: Vec<(u32, String)>, pub skipped_live_owner: u32,
                         pub skipped_reused: u32, pub errors: Vec<String> }

// env
pub struct EnvKey(OsString);                     // Eq and Ord ignore ASCII case on Windows
pub type EnvMap = BTreeMap<EnvKey, OsString>;
pub struct ResolvedEnv { pub vars: EnvMap, pub source: EnvSource, pub warnings: Vec<String>, pub resolved_at_ms: i64 }
pub enum EnvSource { LoginShell { shell: PathBuf, interactive: bool, took_ms: u64 }, WindowsRegistry,
                     Fallback { reason: String } }
pub struct EnvResolverConfig { pub timeout: Duration, pub retry_timeout: Duration, pub retry_after: Duration,
                               pub shell: Option<PathBuf>, pub base: Option<EnvMap>, pub home: Option<PathBuf>,
                               pub known_dirs: Option<Vec<PathBuf>>,
                               pub redactor: Option<Arc<Redactor>> }   // gets every env's secret literals (3.6)
impl EnvResolverConfig { pub fn standard() -> EnvResolverConfig; }   // 10 s, 5 s, 60 s, rest None
pub struct EnvResolver { /* watch of the current attempt */ }
impl EnvResolver {
    pub fn start(config: EnvResolverConfig, registry: Arc<LeftoverRegistry>, rt: Handle) -> Arc<EnvResolver>;
    pub fn fixed(env: ResolvedEnv) -> Arc<EnvResolver>;   // tests: no shell, `get()` returns `env` at once
    pub async fn get(&self) -> Arc<ResolvedEnv>;     // shares the in-flight attempt
    pub fn current(&self) -> Option<Arc<ResolvedEnv>>;
    pub fn refresh(&self);
    pub fn status(&self) -> ResolverStatus;
}
pub enum ResolverStatus { Resolving, Ready { source: EnvSource, took_ms: u64, vars: usize }, Fallback { reason: String } }
pub enum DenyReason { ParentSessionMarker, SandboxMarker, DebuggerHook }
pub fn deny_reason(name: &OsStr, value: &OsStr) -> Option<DenyReason>;
pub fn node_options_has_debugger_hook(value: &str) -> bool;
pub fn scrub(vars: &mut EnvMap) -> ScrubReport;
pub struct ScrubReport { pub removed: Vec<String> }           // sorted names
/// Removes the variables that describe how auracode itself was launched (3.6); returns their names.
pub fn strip_launch_context(vars: &mut EnvMap) -> Vec<String>;
/// The complete env of one child: resolved, scrubbed, then every `set` entry.
pub fn child_env(resolved: &ResolvedEnv, set: &[(OsString, OsString)]) -> (EnvMap, ScrubReport);
/// The values of variables whose names contain KEY, TOKEN, SECRET, PASSWORD or CREDENTIAL, ignoring ASCII case (3.6).
pub fn secret_values(vars: &EnvMap) -> Vec<String>;
pub fn process_env() -> EnvMap;
pub fn find_program(name: &str, env: &EnvMap) -> Option<PathBuf>;
pub fn find_exe(name: &str, env: &EnvMap) -> Option<PathBuf>;   // Windows: `<name>.exe` only; Unix: find_program
pub fn parse_env0(output: &[u8], start: &str, end: &str) -> Option<EnvMap>;               // pure
pub fn expand_env_strings(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String; // pure
pub fn merge_windows_path(machine: Option<&str>, user: Option<&str>, process: Option<&str>) -> String; // pure

// log
pub struct LogLimits { pub max_record_bytes: usize, pub max_file_bytes: u64, pub max_files: u32, pub queue: usize }
impl LogLimits { pub const STANDARD: LogLimits; }             // 256 KiB, 8 MiB, 3, 4096
pub struct RetentionPolicy { pub max_total_bytes: u64, pub max_age: Duration }
impl RetentionPolicy { pub const STANDARD: RetentionPolicy; } // 256 MiB, 14 days
#[derive(Clone)] pub struct LogFactory { /* root, Arc<Redactor>, limits, runtime handle */ }
impl LogFactory {
    pub fn new(root: PathBuf, redactor: Arc<Redactor>, rt: Handle) -> LogFactory;   // root = <profile>/logs/protocol
    pub fn open(&self, chat_id: &str, spawn_id: &str) -> ProtocolLog;   // never fails; disabled on I/O error
    pub fn chat_dir(&self, chat_id: &str) -> PathBuf;
    pub async fn prune(&self, policy: RetentionPolicy) -> RetentionReport;
}
#[derive(Clone)] pub struct ProtocolLog { /* Option<bounded sender>, dropped counter, spawn id */ }
impl ProtocolLog {
    pub fn disabled() -> ProtocolLog;                         // carries a fresh UUIDv7 spawn id
    pub fn disabled_with_id(spawn_id: impl Into<String>) -> ProtocolLog;   // carries this one (replay, 5.8)
    pub fn spawn_id(&self) -> &str;                           // copied into SpawnedProcess::spawn_id
    pub fn line(&self, direction: Direction, line: &str);     // non-blocking; drops and counts when full
    pub fn command(&self, command: serde_json::Value);
    pub fn note(&self, note: LogNote);
    pub fn path(&self) -> Option<PathBuf>;
}
pub enum Direction { Out, In, Err }
pub enum LogNote { Spawn { label: String, program: String, args: Vec<String>, cwd: String, removed: Vec<String>,
                           set: Vec<String>, pid: u32 },
                   Start(serde_json::Value), Exit { code: Option<i32>, signal: Option<i32> },
                   Oversized { bytes: u64, method: Option<String> }, Noise { bytes: usize },
                   Shutdown { steps: Vec<String>, took_ms: u64 }, Text(String) }
pub struct Redactor { /* compiled patterns, secret literals */ }
impl Redactor {
    pub fn standard() -> Redactor;
    /// Adds secret literals (`secret_values`); keeps those of at least 12 characters and never drops earlier ones.
    pub fn set_secrets(&self, values: Vec<String>);
    pub fn redact_value(&self, value: &mut serde_json::Value);
    pub fn redact_text<'a>(&self, text: &'a str) -> Cow<'a, str>;
}

// testing.rs (feature "testing")
pub struct FakeSpawner { /* queue */ }
impl FakeSpawner { pub fn new() -> (Arc<FakeSpawner>, mpsc::UnboundedReceiver<FakeAgentEnd>); pub fn fail_next(&self, e: SpawnError); }
impl ProcessSpawner for FakeSpawner {}
pub struct FakeAgentEnd { pub spec: SpawnSpec, pub from_client: mpsc::UnboundedReceiver<String>,
                          pub to_client: mpsc::UnboundedSender<FakeLine>, pub stderr: mpsc::UnboundedSender<String>,
                          pub exit: oneshot::Sender<ExitInfo>, pub killed: watch::Receiver<bool> }
pub enum FakeLine { Line(String), Oversized { bytes: u64, keys: TopKeys }, Eof }
pub struct FakeControl { /* records close_stdin, kill_tree, shutdown calls */ }   // implements ControlImpl
```

### 3.2 Spawning

| Step | Unix | Windows |
|---|---|---|
| checks | the process cwd (`ChatFolder` resolves to `spec.folder`) by `metadata`: not found or not a directory -> `InvalidCwd`, permission denied (macOS privacy answers `EPERM`) -> `CwdNotAllowed`; checked first, since Unix reports a missing cwd as "program not found". `program` absolute, a file, executable (`ProgramNotFound`, `PermissionDenied`) | same; a cwd longer than 259 UTF-16 units -> `InvalidCwd` ("longer than Windows allows for a program's working folder"); program has a `PATHEXT` extension |
| command | `tokio::process::Command::new(program)`, args, `current_dir(cwd)`, `env_clear()` then `child_env(env, set_env)`, three pipes | same |
| wrappers | `KillOnDrop`, `ResetSigmask`, `ProcessGroup::leader()` (helpers with `new_session`: `ProcessSession`) | `KillOnDrop`, `CreationFlags(CREATE_NO_WINDOW)`, then process-wrap's `JobObject`, for every agent and helper |
| after | read the start time, `register` in the leftover registry (failure: warning, never a failed spawn), start the stderr drain, the stdout reader, the stdin writer and the waiter, write `LogNote::Spawn` (names only) | same, without the registry |

- The checks run on the blocking pool: a macOS privacy prompt can hold the call until the user answers. Folders are
  never passed through `std::fs::canonicalize`, whose Windows result carries a `\\?\` prefix Node agents mishandle.
- `DETACHED_PROCESS` and `CREATE_NEW_CONSOLE` are never used (spike). process-wrap's Tokio `KillOnDrop` kills only
  the direct child on Unix, so the last `ProcessControl` drop also sends `killpg(SIGKILL)` (the spike's `HiddenChild`
  rule).
- **Exit detection.** The waiter task owns the wrapped child and awaits the native `tokio::process::Child` beneath
  the wrappers, found by walking `inner_mut()` and downcasting through `Any` (safe code, compiled for both targets),
  never a wrapper's `wait()`: process-wrap's `JobObjectChild::wait` returns only once every process in the job has
  exited, so a crashed adapter with a lingering descendant would look alive forever. Kill requests reach the waiter
  over a channel (process-wrap's `start_kill` needs the child); the job layer is used only to terminate and to close.
- **No job breakaway in M1a** (decision 19). A process that asks for `CREATE_BREAKAWAY_FROM_JOB` cannot start inside
  our job, and Hermes' client-side gateway start does (`spawn_unmanaged_gateway` raises
  `windows_breakaway_unavailable`), so on Windows Hermes works when its gateway already runs; the hermes dialect turns
  that string in the stderr tail into the hint "On Windows, auracode can't let Hermes start its gateway yet. Run
  `hermes gateway ensure` in a terminal, then press Retry." M1e adds breakaway for agents that declare daemons,
  through an upstreamed or vendored process-wrap `JobObject` variant that takes extra limit flags
  (`JOB_OBJECT_LIMIT_BREAKAWAY_OK`, never `SILENT_BREAKAWAY_OK`), checked with the Windows manual checklist.
- **Windows programs.** Binaries an adapter must spawn are `.exe` only (4); adapters start as `node.exe <entry>`; only
  the one-time adapter install runs `npm.cmd`, which std routes through cmd.exe with its batch escaping, which accepts
  our constant arguments.
- aura-process has no `unsafe` code: Windows needs no process table in M1a (the job tracks the tree and no sweep runs
  there), and `CREATE_NO_WINDOW` is a constant.

### 3.3 stdin, stdout, stderr

- **stdin:** a writer task drains an unbounded queue, writes `line + "\n"`, flushes per line, logs `out`. A write
  error stops it (the agent is gone). The driver never waits on stdin, so a full stdin pipe can never stall the
  stdout reader.
- **stdout:** a reader task with the framer: 64 KiB reads; split on `\n` only; one trailing `\r` dropped; lossy
  UTF-8; empty lines skipped. A line whose first non-space byte is neither `{` nor `[` is noise: logged as
  `LogNote::Noise`, never passed on. Each line is logged `in` and sent over a channel bounded at 16 lines and 8 MiB
  (one longer line passes alone), so a slow driver stops the reader, the pipe fills and the agent blocks on its own
  write. A read error ends the stream like EOF.
- **Oversized lines** (over `max_line_bytes`, 64 MiB; Hermes' own limit is 50 MiB, the Claude spike saw 163 KB): the
  framer stops buffering at the cap, keeps feeding bytes to the sniffer (depth, in-string and escape tracking;
  captures the raw top-level `"id"` value and the `"method"` and `"type"` strings, each at most 256 bytes, in
  constant memory, wherever they appear), and at the line's end yields `StdoutItem::Oversized { bytes, keys }` and
  logs `LogNote::Oversized`.
- **stderr:** drained continuously into a 16 KiB ring (`stderr_tail`) and split into lines (4 KiB per line) for the
  log, at most 256 KiB per second logged (the rest counted). A 2 MB stderr flood never blocks stdout.
- Memory per agent is bounded by bytes end to end: reader 8 MiB, driver outbox 8 MiB, event channel 8 MiB, the actor's
  outbox 4 MiB, the store FIFO's share, plus at most one oversized item in flight at each stage.

### 3.4 The shutdown chain and the kill rule

| Step | Unix | Windows |
|---|---|---|
| 0 | snapshot the agent's descendants by parent pid with their groups and command lines (orphans reparent to init, so this happens first) and sort them: group members, declared daemons with their subtrees (spared), escaped descendants (every other) | (the job tracks the tree) |
| 1 | close stdin (skipped by `FORCE`) | same |
| 2 | wait `stdin_eof_grace` for the agent to exit | same |
| 3 | re-snapshot; `killpg(pgid, SIGTERM)` plus SIGTERM to every escaped descendant (identity re-checked by pid and start time) | skipped: no SIGTERM across consoles |
| 4 | wait `sigterm_grace` | |
| 5 | `killpg(pgid, SIGKILL)` plus SIGKILL to the escaped descendants still alive | `TerminateJobObject` |
| 6 | wait `kill_confirm`; macOS answers `EPERM` for a group of zombies, counted as gone once the agent exited (spike) | wait |
| 7 | unregister from the leftover registry; report leftovers and spared pids | same |

**Kill rule.** Every member of the agent's process group dies, and so does every descendant the parent walk finds in
another group or session, unless it matches one of the agent's declared daemons (`LaunchSpec.daemons`): a match and
its whole subtree are spared and listed in `spared`. Matching reads the command line (macOS `ps -o command=` split on
spaces, Linux `/proc/<pid>/cmdline` split on NUL). Declared in M1a:

- Hermes' gateway: `args_in_order = ["hermes_cli.main", "gateway", "run"]`. `spawn_unmanaged_gateway` starts it as
  `<python> -m hermes_cli.main [--profile default] gateway run --quiet [--idle-exit]` in a new session.
- Claude's daemon, as a precaution for claude-acp: `program = "claude"`, `["daemon", "run"]` (seen on this Mac as
  `claude daemon run --origin transient --spawned-by {...}` with `bg-pty-host` helpers below it; the Claude spike saw
  no `-p` child start it).

Everything else an agent starts (shells, dev servers, watchers, PTY hosts that called `setsid`, Node children spawned
`detached`) dies at Stop, force stop, crash cleanup and quit, which matches Windows, where the job kills them. A
vendor daemon that only changed its process group while still parented under the agent is protected by its
declaration, not by its group. A match still in the agent's group is a member like any other: it dies with the
group, and its subtree is not spared (the sweep applies the same rule). Agents are spawned with `process_group(0)`,
not `setsid`.

`kill_tree()` is step 0 and step 5 at once, and needs no runtime (a run-event callback has none):
`killpg(pgid, SIGSTOP)` runs inline, so nothing in the group can exit, fork or orphan its children; a walk on a thread
of its own then sends SIGKILL to the escaped descendants it finds, to the group, and to the recorded escaped
descendants that still run as themselves. It returns once that is done, or after 1 s with `killpg(pgid, SIGKILL)`
sent itself. Once the waiter has reaped the agent, the group's id is free for a new group, so nothing signals the group
after it was seen empty, or once the agent's pid names a new process. On Windows it is a non-blocking request to the
waiter, and the job dies with auracode in any case. After every turn the session calls `record_escaped()`, so a crash
leaves the next sweep a list of the agent's escaped descendants.

Timings: codex-acp closes its app-server's stdin on its own stdin EOF and sends SIGTERM 2 s later, so EOF needs more
than 2 s; claude-agent-acp exits on stdin EOF; Claude's SDKs wait 5 s.

### 3.5 Leftover registry and the startup sweep (Unix)

```text
<data root>/run/<instance pid>-<instance start key>.json     one per running auracode process (app or CLI)
{"version":1,"instance":{"pid":4242,"start":{...},"sid":4242,"boot":"<marker>"},
 "entries":[{"pid":5001,"pgid":5001,"sid":4242,"start":{...},"label":"hermes acp","spawned_at_ms":1792840000000,
             "daemons":[{"program":null,"args_in_order":["hermes_cli.main","gateway","run"]}],
             "escaped":[{"pid":5120,"start":{...}}]}]}
```

- Written atomically (temp file, fsync, rename) on every change, and rewritten every 60 s while it has entries: its
  modification time is the instance's heartbeat. Deleted at a clean exit. Windows keeps no registry: the job dies with
  its owner (spike).
- `aura-cli` registers in the real data root's `run/` (machine-wide) even when its profile is a temporary one, so the
  app's next sweep cleans up after a crashed CLI run.
- `sweep_stale` runs once at boot on a background task, for every file that is not ours:
  1. instance alive (pid running with the recorded start time): skip; another auracode owns those processes.
  2. boot marker differs: the machine rebooted; delete the file, kill nothing.
  3. instance dead: candidates are (a) processes whose `pgid` and `sid` equal the entry's and whose start time lies
     between the entry's start and the file's modification time plus 60 s (a process started after that window
     cannot belong to the dead instance, so a terminal job that later reused the group id is never touched); (b) the
     entry's escaped processes whose pid and start time still match; (c) when the entry's leader is alive with its
     recorded start time, its descendants under the kill rule, with the entry's daemons spared. SIGTERM, 2 s,
     SIGKILL, start times re-checked before each signal. Every skipped candidate is logged with its reason.
  4. delete the file; log counts; when anything was killed, a startup notice: "auracode stopped 2 agent processes
     left running by an earlier session that ended unexpectedly."
- Accepted gap: a group member started more than 60 s after the dead instance's last heartbeat, whose leader has also
  exited, is not found (an agent that kept spawning after auracode crashed, then quit); signalling a recycled group
  would be worse.
- Start times: macOS `ps -axo pid=,ppid=,pgid=,lstart=,command=` with `LC_ALL=C TZ=UTC0` (one-second resolution,
  checked on this Mac; `lstart` is five fixed tokens), Linux `/proc/<pid>/stat` field 22 with the boot id. Session
  ids on macOS come from `nix::unistd::getsid` (macOS `ps -o sess` prints 0).

### 3.6 Env resolution

macOS and Linux:

1. Shell: `$SHELL` when absolute and executable, else the passwd entry (`nix::unistd::User::from_uid`), else
   `/bin/zsh` (macOS) or `/bin/sh` (Linux).
2. Command, with `S` and `E` = `__AURA_ENV_<16 random hex>_START__` / `..._END__`, fresh per attempt:

   | Shell basename | argv |
   |---|---|
   | `bash`, `zsh`, `sh`, `dash`, `ksh`, `mksh`, `yash`, `fish` | `<shell> -i -l -c "printf '%s' 'S'; /usr/bin/env -0; printf '%s' 'E'"` |
   | `tcsh`, `csh` | `<shell> -i -c "..."` (csh takes `-l` only as its sole argument) |
   | anything else (`nu`, `xonsh`) | the platform default shell with the first form, plus a warning naming the shell |

3. Run through `run_helper`: the shell's input env is `process_env()` minus `strip_launch_context`, plus
   `set = [AURACODE_RESOLVING_ENV=1]` (users can skip slow rc parts), cwd `$HOME`, `new_session: true` (an
   interactive shell must never take the foreground of the terminal auracode was started from), stdin `/dev/null`,
   stdout at most 4 MiB, timeout 10 s.
4. Parse with `parse_env0`: the text between the last `S` before the last `E` and that `E`, split on NUL, each entry
   at its first `=`; entries without `=` or with an empty name skipped; a later duplicate wins. Output before `S` is
   ignored. A non-zero exit with both markers is accepted with a warning; no `PATH` counts as failure.
5. On failure or timeout: the same command with `-l -c` (no `-i`), 5 s. Then the fallback: `process_env()` minus
   `strip_launch_context`, plus the known dirs, `EnvSource::Fallback { reason }`.
6. Remove `PWD`, `OLDPWD`, `SHLVL`, `_`, `AURACODE_RESOLVING_ENV`. Append each existing known dir missing from
   `PATH`: `~/.local/bin`, `~/.opencode/bin`, `~/.grok/bin`, `~/.pi/agent/bin`, `~/.bun/bin`, `~/.volta/bin`,
   `~/.asdf/shims`, `~/.local/share/mise/shims`, `<prefix>/bin` for a `prefix=` line in `~/.npmrc` (read, never by
   running npm), `/opt/homebrew/bin`, `/usr/local/bin`. nvm is left to the login shell: it is a shell function whose
   aliases the fallback cannot resolve reliably.

`strip_launch_context` removes what describes how auracode itself was launched: `TERM_PROGRAM`,
`TERM_PROGRAM_VERSION`, `TERM_SESSION_ID`, names starting with `SHELL_SESSION_` (a dev launch from Apple Terminal
otherwise runs `/etc/zshrc_Apple_Terminal`'s session save and restore against the user's tab history, the "Restored
session" output seen on this Mac), names starting with `npm_` (`npm_config_*`, `npm_package_*`, `npm_lifecycle_*`,
`npm_execpath`, ...), `INIT_CWD`, names starting with `PNPM_` or `pnpm_`, and names starting with `TAURI_` (a
`pnpm dev` launch). Names and prefixes compare case-sensitively on every OS, unlike the scrub's (3.7): npm, pnpm and
Tauri set them in a fixed case, and a user's own `NPM_TOKEN` or `NPM_CONFIG_REGISTRY` must survive on Windows too. It
applies only to the shell's input: whatever the user's own rc files export comes back through the shell's output.
AppImage's additions (`APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`, AppRun's `LD_LIBRARY_PATH` and GTK variables) join this
list in M5, when the AppImage exists.

Windows: base = `process_env()` minus `strip_launch_context`; every value of HKLM
`SYSTEM\CurrentControlSet\Control\Session Manager\Environment`, then of HKCU `Environment` (user wins), read with
`winreg`, `REG_EXPAND_SZ` expanded by `expand_env_strings` (`%NAME%` case-insensitive, unknown names kept literally,
cycle guard); `PATH` = `merge_windows_path(system, user, process)`: system, then user, then process entries not yet
present, de-duplicated case-insensitively, first occurrence kept. Runs on a blocking thread. `USERNAME` and
`PROCESSOR_ARCHITECTURE` keep the base's values, in the expansion too: HKLM's are `SYSTEM` and the machine's own
architecture (Windows' `CreateEnvironmentBlock` and Chocolatey's `refreshenv` keep the logon's as well).

Caching: started at boot in the background; boot never waits. `get()` shares the in-flight attempt; a failed attempt
older than 60 s is retried on the next `get()`; `refresh()` starts a new one; running sessions keep the env they
started with. **Memory only**: the env holds API keys, so it never reaches disk, not even `PATH`. After each
resolution, and before any caller gets that env, the resolver (package A) hands the values of variables whose names
contain `KEY`, `TOKEN`, `SECRET`, `PASSWORD` or `CREDENTIAL`, ignoring ASCII case (`secret_values`; npm's
`npm_config_//registry.npmjs.org/:_authToken` counts), to `Redactor::set_secrets` on `EnvResolverConfig::redactor`.
`set_secrets` keeps the values of at least 12 characters and never drops earlier ones, because running sessions keep
the env they started with. The desktop passes the shared redactor at boot, the one `LogFactory` and `ManagerDeps` hold
(7.2), so protocol logs and the session actor redact the same literals. `EnvResolver::fixed` hands its env to no
redactor: tests call `set_secrets` themselves.

### 3.7 The scrub (the plan's exact denylist, in `env/scrub.rs`)

| Removed | Rule |
|---|---|
| `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_EXECPATH` | exact name |
| `CLAUDE_CODE_SESSION_*`, `CLAUDE_CODE_MESSAGING_*`, `CLAUDE_CODE_BRIDGE_*` | prefix families (seen: `..._SESSION_ID`, `..._SESSION_ATTENDED`, `..._MESSAGING_SOCKET`, `..._MESSAGING_TOKEN`, `..._BRIDGE_SESSION_ID`) |
| `CODEX_SANDBOX`, `CODEX_SANDBOX_NETWORK_DISABLED` | exact name |
| `ELECTRON_RUN_AS_NODE`, `VSCODE_INSPECTOR_OPTIONS` | exact name |
| `NODE_OPTIONS` | the whole variable, only when a whitespace-separated token (quotes honoured) is `--inspect`, `--inspect-brk` or `--inspect-wait` (bare or `=value`), or `--require`/`-r` (separate or `=`-joined) of a path containing `bootloader` or `js-debug` |

Names compare case-sensitively on Unix and ASCII case-insensitively on Windows; a non-Unicode name is kept. Applied by
every spawn (`child_env`), helpers included. Values are never logged anywhere; `ScrubReport` and `LogNote::Spawn`
carry names only. Kept, each with its own test: `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`,
`CLAUDE_CODE_MAX_OUTPUT_TOKENS`, `CLAUDE_CODE_GIT_BASH_PATH`, `CLAUDE_CODE_OAUTH_TOKEN`,
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
`SSH_AUTH_SOCK`, `DISPLAY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`,
`NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, `PATH`, `HOME`, `NODE_OPTIONS=--max-old-space-size=4096`. The spike's
`env_scrub.rs` still has the old prefix rule; do not port it.

`set_env` entries are always set after the scrub. An adapter's user-binary variable (`CLAUDE_CODE_EXECUTABLE`,
`CODEX_PATH`) is set to the binary detection chose (4): the user's own value when it names an executable file,
otherwise the binary found on `PATH`. An empty or broken value is overridden, because both adapters treat an empty
value as unset and run the CLI they bundle (claude-acp's `claudeCliPath()` tests `process.env.CLAUDE_CODE_EXECUTABLE`
for truthiness, and its session path uses `?? claudeCliPath()`; codex-acp's `spawnCodexCli` tests `codexPath`).

`find_program(name, env)`: the env's `PATH` in order (Windows: each `PATHEXT` extension), first match that is a file
and executable; symlinks are never resolved (agents launch through their stable symlink or shim). `find_exe` is the
same on Unix and accepts only `<name>.exe` on Windows.

### 3.8 Protocol logs

One JSON Lines file per agent process: `<profile>/logs/protocol/<chat_id>/<spawn_id>.jsonl` (`spawn_id` is a UUIDv7,
so names sort by time), rotated to `.1.jsonl`, `.2.jsonl`. The env resolver's output is never logged (it is all env
values); probes and installs are not logged here (installs log to the app log, names and exit codes only).

```text
{"t":1792840001230,"d":"note","note":{"spawn":{"label":"hermes acp","program":"<abs path>","args":["acp"],"cwd":"...","removed":["CLAUDECODE"],"set":[],"pid":5001}}}
{"t":1792840001231,"d":"note","note":{"start":{"agent_id":"hermes","dialect":"hermes","resume":null,"autonomy":"full_auto","desired":{},"spawn_id":"019a..."}}}
{"t":1792840001240,"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}}
{"t":1792840001900,"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{...}}}
{"t":1792840002001,"d":"err","line":"INFO gateway ready"}
{"t":1792840003000,"d":"cmd","cmd":{"type":"prompt","turn_id":"019a...","parts":[{"type":"text","text":"Reply with exactly: ok"}]}}
{"t":1792840003500,"d":"in","raw":"Debugger listening on ..."}
{"t":1792840009000,"d":"note","note":{"oversized":{"bytes":71303168,"method":"session/update"}}}
{"t":1792840010000,"d":"note","note":{"exit":{"code":0,"signal":null}}}
```

`d` is `out` (to the agent), `in` (from it; `msg` for JSON, `raw` for other text), `err`, `cmd` (a driver command in
processing order, from the driver), `note`. `t` is for humans; replay uses line order.

| Limit | Value | Behaviour |
|---|---|---|
| record | 256 KiB | longer: `{"t","d","raw","truncated"}`, with the record's own `d`, the first 128 KiB of its payload as text in `raw` (a `msg`, `cmd` or `note` serialized first) and the bytes cut in `truncated`; nothing in it can be replayed |
| file | 8 MiB | rotate; 3 files per spawn, the oldest deleted |
| total | 256 MiB per profile | oldest files first |
| age | 14 days | deleted |
| queue | 4,096 records | full: drop and count; a `{"note":{"dropped":n}}` follows, at the latest when the log closes; the protocol path never waits |
| I/O error, disk full | | logging for that spawn stops with one app-log warning |

Retention runs at boot (background) and when a session ends.

**Redaction**, in the writer task, before any byte reaches disk:

1. JSON keys, compared ASCII-lowercase with `_` and `-` removed. Credential keys: equal to or ending with one of
   `token`, `secret`, `password`, `passwd`, `apikey`, `authorization`, `cookie`, `credential`, `credentials`,
   `privatekey`, `sessionkey` (so `accessToken` and `client_secret` match; `inputTokens` and `apiKeySource` do not).
   Identity keys: containing `email`, or equal to one of `account`, `authstatus`, `oauthaccount`, `organization`,
   `organizationname`, `organizationuuid`, `orgid`, `orgname`, `accountuuid`, `accountid`, `userid`, `username`,
   `fullname`. Every string at or under such a key becomes `"<REDACTED>"`; numbers and booleans are kept. This covers
   Claude's `initialize` `account` object and the adapters' `_auth/status_update` (`authStatus.account.{email,
   organization}`, and `authStatus.label`, which names the user's plan, as the real-agent probes showed), and leaves
   model names (`displayName`, `name`) readable.
2. Every value of an `env` object, and `value` of `{ "name", "value" }` entries under `env` or `headers` arrays.
3. Every remaining string, every `raw` and stderr line: email addresses (the spike's `mask_emails` rules, so
   `react@18.3.1` and `@scope/pkg` stay) to `<EMAIL>`; `sk-ant-[A-Za-z0-9_-]{10,}`, `sk-[A-Za-z0-9_-]{20,}`,
   `gh[pousr]_[A-Za-z0-9]{20,}`, `github_pat_[A-Za-z0-9_]{20,}`, `xox[abpr]-[A-Za-z0-9-]{10,}`, `AKIA[0-9A-Z]{16}`,
   `AIza[0-9A-Za-z_-]{35}`, `glpat-[A-Za-z0-9_-]{20,}`, three-part JWTs (`eyJ...\.eyJ...\....`),
   `Bearer [A-Za-z0-9._~+/=-]{16,}` to `<TOKEN>`; secret literals from `set_secrets` to `<SECRET>`.
4. Env values never appear in a record at all.

Account data never reaches SQLite either: `initialize` replies, auth notifications and catalogs are not events. The
stderr tail shown in an error card, or stored in `Error.detail`, passes `redact_text` first, in the session actor
(6.5.5): drivers hand it over raw.

---

## 4. aura-agents: catalog, detection, adapter install, launch

```rust
pub struct AgentSpec {
    pub id: &'static str, pub name: &'static str, pub description: &'static str,
    pub kind: aura_protocol::DriverKind,
    pub dialect: &'static str,
    pub recipe: Recipe,
    pub startup_timeout: Duration,
    /// Turns run in a daemon that outlives the agent process (Hermes' gateway): cancels wait longer (6.5.4) and
    /// resumes compare the replay with stored history (5.3.2).
    pub turns_survive_process: bool,
    pub daemons: Vec<DaemonMatch>,                 // copied into LaunchSpec.daemons
    pub last_verified: &'static str,
}
pub enum Recipe {
    Binary { program: &'static str, args: &'static [&'static str], version_args: &'static [&'static str],
             min_version: Option<&'static str> },
    NpmAdapter { package: &'static str, version: &'static str, bin: &'static str, min_node_major: u32,
                 user_binary: UserBinary },
    Builtin,
}
/// The user's own CLI that an adapter must drive, passed through `env` (always set, 3.7).
pub struct UserBinary { pub label: &'static str, pub program: &'static str, pub env: &'static str,
                        pub version_args: &'static [&'static str], pub min_version: &'static str }
pub fn catalog() -> &'static [AgentSpec];                  // built once (LazyLock)
pub fn get(id: &str) -> Option<&'static AgentSpec>;
pub fn parse_version(text: &str) -> Option<String>;        // first `\d+\.\d+\.\d+` (with optional `v`)
pub fn version_at_least(found: &str, floor: &str) -> bool; // numeric x.y.z comparison

pub struct Detector { /* env resolver, registry, specs, adapters dir, cache, in-flight installs, forgotten */ }
impl Detector {
    pub fn new(env: Arc<EnvResolver>, registry: Arc<LeftoverRegistry>, adapters_dir: PathBuf, rt: Handle) -> Arc<Detector>;
    /// Tests: these agents with fixed answers (`FixedAgent` below); no probes, no env, no installs.
    pub fn fixed(agents: Vec<FixedAgent>) -> Arc<Detector>;
    /// The session manager looks agents up here, never in `catalog()`, so tests can inject their own.
    pub fn spec(&self, id: &str) -> Option<AgentSpec>;
    /// Cached; waits for the env resolution and the probe in flight, shared with every other caller, so it answers
    /// `Ready`, `Missing` or `Broken`, never `Checking`.
    pub async fn detect(&self, spec: &AgentSpec) -> aura_protocol::session::Availability;
    /// `refresh: false` returns `cached()` at once and never waits (`agents_list`); detection runs in the background
    /// from `new` on, once the env resolves. `refresh: true` resolves the env again, probes every agent and waits for
    /// the results (`agents_refresh`).
    pub async fn detect_all(&self, refresh: bool) -> Vec<aura_protocol::session::AgentEntry>;
    pub fn cached(&self) -> Vec<aura_protocol::session::AgentEntry>;    // Checking until its first detection ends
    /// Installs an npm adapter when it is not installed (one shared attempt per package; `progress` receives status
    /// details), then builds the launch spec. Dropping the future kills a running install.
    pub async fn prepare_launch(&self, spec: &AgentSpec, progress: mpsc::UnboundedSender<String>)
        -> Result<LaunchSpec, LaunchError>;
    /// Deletes an adapter's install marker, so the next start installs it again (a start failed on a missing module).
    pub fn forget_install(&self, spec: &AgentSpec);
    /// Tests: the agent id of every `forget_install` so far, in order.
    pub fn forgotten(&self) -> Vec<String>;
}
#[derive(Debug, thiserror::Error)]
pub enum LaunchError { #[error("{reason}")] Missing { reason: String }, #[error("{reason}")] Broken { reason: String },
                       #[error("{reason}")] InstallFailed { reason: String } }

/// A test agent. `non_exhaustive`: built with `new` and the `with_*` methods, so a later field never breaks a test.
#[non_exhaustive]
pub struct FixedAgent {
    pub spec: AgentSpec,
    pub availability: Availability,          // cached() until a detect() ends; what detect() answers without `detected`
    pub detected: Option<Availability>,      // what detect() answers, and cached() from then on (Checking, then Ready)
    pub launch: Option<LaunchSpec>,          // None: a built-in agent, or prepare_launch fails with the reason
    pub launch_error: Option<LaunchError>,   // prepare_launch fails with this (an InstallFailed download)
    pub needs_download: bool,                // not installed: AgentEntry.needs_download, and the next prepare_launch
                                             // sends `progress` first; a successful one installs, and forget_install
                                             // makes an npm adapter's download needed again
    pub progress: Vec<String>,               // the download's status details
    pub detect_gate: Option<Gate>,           // holds every detect() until the gate opens
    pub launch_gate: Option<Gate>,           // holds every prepare_launch(), after its progress, until it opens
}
impl FixedAgent {
    pub fn new(spec: AgentSpec, availability: Availability, launch: Option<LaunchSpec>) -> FixedAgent;
    pub fn with_detected(self, availability: Availability) -> FixedAgent;
    pub fn with_download(self, progress: Vec<String>) -> FixedAgent;   // needs_download with its details
    pub fn with_launch_error(self, error: LaunchError) -> FixedAgent;
    pub fn with_detect_gate(self, gate: Gate) -> FixedAgent;
    pub fn with_launch_gate(self, gate: Gate) -> FixedAgent;
}
/// Holds a fixed agent's detection or launch preparation until the test opens it. Clones share it; it stays open.
#[derive(Clone, Default)] pub struct Gate { /* open flag, waiting count */ }
impl Gate {
    pub fn new() -> Gate;
    pub fn open(&self);
    pub fn waiting(&self) -> usize;                    // a call whose future was dropped no longer counts
    pub async fn until_waiting(&self, count: usize);   // 1: a preparation reached it; 0: it passed or was dropped
}
```

`AgentSpec` and `Recipe` derive `Debug, Clone`. P0 implements `Detector::fixed` with `FixedAgent` and `Gate`,
`Detector::spec` and a functional `Detector::new` stub (9.1), which C and D need on day one. The fixed detector is how
C's tests drive a start's preparation: P6's download detail (`with_download`, held at a launch gate so the detail
shows), S32's Esc during an install or a probe (a gate's `waiting()` drops to 0 when the preparation is dropped), P42's
forgotten install (`forgotten()`, and the next start downloads again), an `InstallFailed` start (`with_launch_error`),
and a chat created while its agent reads `Checking` whose start waits for detection (`with_detected`, a detect gate).

| id | Name | Recipe | Dialect | Start timeout | Survive | Daemons | Last verified |
|---|---|---|---|---|---|---|---|
| `hermes` | Hermes | `Binary { "hermes", ["acp"], ["--version"], min None }` | `hermes` | 60 s (the gateway may start first) | yes | Hermes' gateway (3.4) | Hermes 0.22.5 |
| `claude-acp` | Claude Code (ACP) | `NpmAdapter { "@agentclientprotocol/claude-agent-acp", "0.95.0", bin "claude-agent-acp", node 22, Claude Code: claude -> CLAUDE_CODE_EXECUTABLE, min 2.1.309 }` | `claude-acp` | 60 s | no | Claude's daemon (3.4) | adapter 0.95.0 with Claude Code 2.1.310 |
| `codex-acp` | Codex (ACP) | `NpmAdapter { "@agentclientprotocol/codex-acp", "2.2.2", bin "codex-acp", node 22, Codex: codex -> CODEX_PATH, min 0.160.1 }` | `codex-acp` | 60 s | no | none | adapter 2.2.2 with Codex 0.164.0 |
| `mock` | Demo agent (scripted, no AI) | `Builtin` | `mock` | 5 s | no | none | auracode |

The floors are the CLI versions the adapters bundle (claude-agent-acp 0.95.0 bundles the Agent SDK 0.3.309 with Claude
Code 2.1.309; codex-acp 2.2.2 depends on `@openai/codex` ^0.160.1); probes 1 and 2 confirm the installed CLIs work.

**Detection** (probes are `run_helper` with the scrubbed env, 10 s, 64 KiB of output; binaries run in `$HOME`,
adapter probes in the adapter folder):

- `Binary`: `find_program(program)`; `<program> <version_args>` must run; below `min_version` (when set) ->
  `Broken { "auracode needs <name> <floor> or newer (found <version>)." }`.
- `NpmAdapter`: `node` found and `node --version` at least `min_node_major`, else `Missing { "Node.js 22 or newer is
  required (found X)." }`. The user binary is the resolved env's `<user_binary.env>` value when it is non-empty,
  absolute and an executable file (on Windows a `.exe`), else `find_exe(program)`. On Windows, when only a shim
  (`.cmd`, `.bat`, `.ps1`) exists: `Missing { "Claude Code is installed through npm here, and the adapter can't run
  its claude.cmd shim on Windows. Install the standalone Claude Code, or set CLAUDE_CODE_EXECUTABLE to a claude.exe."
  }` (Node refuses to spawn `.cmd` files without a shell, and the adapters spawn the path they are given). None:
  `Missing { "The Claude adapter drives your own Claude Code, and claude was not found on your PATH." }`. The binary's
  `--version` must run and reach `min_version`, else `Broken`. `npm` (`find_program("npm")`) is needed only while
  the adapter is not installed: `Missing { "npm was not found; auracode needs it once to download the adapter." }`.
  Ready reads `{ version: "Claude Code 2.1.310 via claude-agent-acp 0.95.0", path: <the user binary> }`, so the
  binary that will actually run is the one shown; `AgentEntry.needs_download` is true without a valid install marker.
- Detection never installs and never runs an adapter. `Broken { reason }` quotes the first stderr line when a probe
  exits non-zero or times out (the tree is killed on timeout). Several installs: `PATH` order wins (shadowed installs
  are M1e).

**Install** (`install.rs`, inside `prepare_launch` for `NpmAdapter`):

```text
folder   <data root>/adapters/<package without "@", "/" as "-">-<version>, created private
         (agentclientprotocol-claude-agent-acp-0.95.0)
valid    folder/installed.json names this package and version, and its entry file exists inside the folder
install  only when not valid; one attempt per package at a time in this process (later callers share it), holding
         folder/install.lock exclusively (std File::lock, so the CLI and the app never install at once):
         1. remove folder/node_modules; write folder/package.json and folder/package-lock.json from the copies
            committed for this version (crates/aura-agents/adapters/<folder name>/, embedded with include_str!);
            the package.json, {"name":"auracode-adapter","private":true,"dependencies":{"<package>":"<version>"}},
            ends npm's walk up for a project
         2. run_helper <npm> ci --omit=optional --ignore-scripts --no-audit --no-fund
            cwd folder, the resolved and scrubbed env plus npm_config_update_notifier=false, timeout 10 min;
            npm ci installs exactly the lockfile's tree and fails on any integrity mismatch
         3. folder/node_modules/<package>/package.json: version equal; `bin` (a string, or the map entry named `bin`)
            resolves to a file inside the folder (no `..` escape)
         4. run_helper <node> <entry> --version, 30 s: the output contains <version> (both adapters print it and
            load every module first, so a partial tree fails here)
         5. write folder/installed.json atomically: {package, version, entry (relative), installed_at_ms}
launch   LaunchSpec { label: <bin>, program: <node>, args: [<absolute entry>], cwd: Fixed(folder),
                      set_env: [(user_binary.env, <user binary>)], daemons, startup_timeout }
```

- Progress detail: "Downloading the Claude adapter (@agentclientprotocol/claude-agent-acp 0.95.0). This happens once
  and can take a few minutes." A failure gives `InstallFailed { "Could not download the Claude adapter: <first npm
  error line, redacted>" }` and marks nothing (the detector holds no shared redactor: it redacts with a
  `Redactor::standard()` given the install env's `secret_values`). Dropping the future (Esc, force stop, Stop, quit
  during Starting) kills npm; the unmarked folder is cleaned at the next attempt, so an interrupted download never
  leaves a broken install in use. A start that later fails on `ERR_MODULE_NOT_FOUND`, `MODULE_NOT_FOUND` or
  `Cannot find module` calls `forget_install` (6.5.5), so Retry downloads it again.
- `--omit=optional` drops the CLIs the adapters bundle, which are optional dependencies (the Agent SDK's eight
  `@anthropic-ai/claude-agent-sdk-<platform>` packages; `@openai/codex`'s six platform packages): the fallback to a
  bundled CLI becomes impossible and the download shrinks by the size of one native CLI for this platform. `--ignore-scripts`: neither adapter's
  top-level packages declare install scripts (registry metadata), so nothing from the tree runs at install. Transitive
  versions are pinned: each adapter version has a `package.json` and a `package-lock.json` under
  `crates/aura-agents/adapters/<folder name>/`, generated once with `npm install --package-lock-only --omit=optional
  --ignore-scripts` (registry reads only; nothing runs) and reviewed in the diff. Without them, codex-acp's ranges
  (`@openai/codex` ^0.160.1, `zod` ^4.0.0, ...) would install whatever was newest that day. Bumping an adapter adds a
  new folder with its own lockfile. A test checks that every catalog adapter has both files, that the root dependency
  matches the catalog version exactly, and that every entry marked `hasInstallScript` is on a reviewed allowlist
  (empty unless the lockfile shows one; the script never runs either way).
- Why not `npx`: npx (npm 11's `libnpmexec`) loads the project tree from npm's local prefix, found by walking up from
  the working folder to the first `package.json` or `node_modules`, runs a copy there whose `pkgid` equals the
  requested spec, and npm reads that project's `.npmrc`. Started in the chat folder, it would let a cloned repo choose
  the code auracode runs with the user's login. The adapters do not need the chat folder as their working directory:
  claude-acp takes the session folder from `params.cwd` and uses `process.cwd()` only for its identity probe before a
  session exists; codex-acp passes the request's `cwd` to `codex app-server`.
- Consent: when `needs_download` is set, the New chat screen says "Creating this chat downloads
  @agentclientprotocol/claude-agent-acp 0.95.0 from npm (one time)."; creating the chat is the consent.
- Antigravity is not in M1a: `agy` 1.4.7 has no ACP mode and `antigravity-acp` is a separate download that needs the
  user's OK.

---

## 5. aura-drivers

### 5.1 The driver contract (`driver.rs`, frozen)

```rust
pub trait AgentDriver: Send + 'static {
    /// Runs until the session ends; never panics on agent input. Returns only after stdout reached its end (or its
    /// grace after the process exited) and every event was handed to `ctx.events`. If the future is dropped or
    /// panics, the session actor force-stops every process this driver spawned (6.5.5).
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit>;
}
pub trait DriverFactory: Send + Sync + 'static {
    fn create(&self, kind: DriverKind) -> Box<dyn AgentDriver>;
}
/// What drivers spawn through; the session's implementation opens the protocol log and records the control.
pub trait AgentSpawner: Send + Sync + 'static {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>>;
}

pub struct DriverContext {
    pub commands: mpsc::UnboundedReceiver<DriverCommand>,
    pub events: EventSender,
    pub spawner: Arc<dyn AgentSpawner>,
    pub setup: SessionSetup,
}
pub struct SessionSetup {
    pub chat_id: String,
    pub agent_id: String,
    pub dialect: String,
    pub folder: PathBuf,                              // the chat folder on this machine
    pub agent_folder: String,                         // the folder as the agent sees it (ACP cwd); M1a: `folder`
    pub add_dirs: Vec<PathBuf>,                       // M4; empty in M1a
    pub launch: LaunchSpec,
    pub env: Arc<ResolvedEnv>,
    pub resume: Option<String>,                       // the native session to continue
    pub autonomy: Autonomy,
    pub desired: BTreeMap<String, ConfigValue>,       // the user's explicit choices, by option id
    pub timings: DriverTimings,
    pub client_version: String,
}
pub struct DriverTimings { pub open_idle: Duration, pub open_ceiling: Duration, pub request: Duration,
                           pub commands_debounce: Duration }
impl DriverTimings { pub const STANDARD: DriverTimings; }   // 60 s, 10 min, 30 s, 250 ms

pub struct TurnRef { pub turn_id: String }
pub enum PermissionOutcome { Selected(String), Cancelled }
pub struct StopPlan { pub cancel_grace: Duration, pub process: ShutdownPlan }

pub enum DriverCommand {
    /// Sent only in Ready with no turn running; answered by exactly one DriverEvent::TurnEnded. A part the driver
    /// does not support ends the turn at once with `Error` and `ErrorKind::Internal` (the actor checks first).
    Prompt { turn: TurnRef, parts: Vec<PromptPart> },
    /// Soft interrupt; idempotent; the turn still ends with TurnEnded.
    Interrupt,
    RespondPermission { request_id: String, outcome: PermissionOutcome },
    RespondUi { request_id: String, answer: Option<UiAnswer> },          // None = dismissed
    SetOption { option_id: String, value: ConfigValue, reply: oneshot::Sender<Result<(), DriverError>> },
    SetAutonomy { autonomy: Autonomy, reply: oneshot::Sender<Result<AutonomyState, DriverError>> },
    /// Graceful end: interrupt a running turn, wait cancel_grace for its reply, close stdin, run the chain.
    Shutdown { plan: StopPlan },
}

pub enum DriverEvent {
    Status(DriverStatus),
    Ready(ReadyInfo),
    /// Transcript events, tagged with their turn (rule 8). Never TurnStarted, TurnEnded, UserMessageDelta,
    /// SessionUpdated, PromptLifecycle, PermissionRequested, PermissionResolved, UiRequest or UiResolved.
    Event { turn: Option<String>, event: AuraEvent },
    TurnEnded { turn_id: String, stop_reason: StopReason, usage: Option<TokenUsage>, error: Option<DriverError> },
    Permission { turn: Option<String>, request: PermissionRequest },
    PermissionWithdrawn { request_id: String },       // the driver already answered the agent `cancelled`
    UiRequest { turn: Option<String>, request: UiRequest },
    UiWithdrawn { request_id: String },
    Commands(Vec<SlashCommand>),                      // the full reduced list
    Options(Vec<ConfigOption>),                       // the full list with current values
    Autonomy(AutonomyState),
    Title(Option<String>),
    AuthRequired(AuthPrompt),
}
pub enum DriverStatus { Spawning, Initializing, Opening, Configuring, Ready, Prompting, Cancelling, Closing }
pub struct ReadyInfo { pub native_session_id: String, pub agent_name: Option<String>, pub agent_version: Option<String>,
                       pub capabilities: Capabilities, pub resumed: ResumeOutcome, pub spawn_id: String,
                       pub replay: Option<ReplaySummary> }
pub enum ResumeOutcome { New, Resumed, Loaded, Fresh { reason: String } }
/// What a load or resume replayed, for dialects whose turns outlive the process (Hermes, 5.3.2).
pub struct ReplaySummary { pub user_turns: u32,
                           pub last_reply: Option<String>,        // the last agent reply, at most 64 KiB
                           pub last_reply_sha256: Option<String> } // of that whole reply with all whitespace removed

#[derive(Debug, Clone, thiserror::Error)] #[error("{message}")]
pub struct DriverError { pub kind: ErrorKind, pub message: String, pub detail: Option<String> }

pub enum DriverExit {
    Closed,                                                          // asked to stop and did
    Crashed { exit: Option<ExitInfo>, stderr_tail: String },         // ended without being asked (the raw tail)
    StartFailed(DriverError),                                        // never reached Ready
    Panicked,                                                        // set by the session actor only
}
pub const DRIVER_EVENT_CAPACITY: usize = 256;
pub const DRIVER_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// The event channel, bounded by count and by approximate bytes; one event larger than the byte bound passes alone.
pub fn event_channel() -> (EventSender, EventReceiver);
#[derive(Clone)] pub struct EventSender { /* mpsc sender plus a byte semaphore */ }
impl EventSender {
    /// Waits until an event of `bytes` fits. Cancel-safe: use it in select! with the outbox head's size.
    pub async fn reserve(&self, bytes: usize) -> Result<EventPermit, ChannelClosed>;
}
pub struct EventPermit { /* channel permit plus byte permit */ }
impl EventPermit { pub fn send(self, event: DriverEvent); }
pub struct EventReceiver { /* releases an event's bytes when it is received */ }
impl EventReceiver { pub async fn recv(&mut self) -> Option<DriverEvent>; }  // None: the driver dropped its sender
impl DriverEvent { pub fn approx_bytes(&self) -> usize; }                    // lengths of its strings and JSON values

/// Serializable mirror of DriverCommand (no reply channels), written as `cmd` log records and read by replay.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoggedCommand { Prompt { turn_id: String, parts: Vec<PromptPart> }, Interrupt,
                         RespondPermission { request_id: String, option_id: Option<String> },
                         RespondUi { request_id: String, answer: Option<UiAnswer> },
                         SetOption { option_id: String, value: ConfigValue }, SetAutonomy { autonomy: Autonomy },
                         Shutdown }
```

Contract every driver keeps (tested for `AcpDriver` and `MockDriver`):

1. Before `Ready`, only `Status`, `Options`, `Commands`, `Title`, `Autonomy`, `AuthRequired`, `Event` with a
   `Notice` or `UsageUpdated` (untagged), and untagged `Permission` and `UiRequest` (an agent may ask during setup, as
   Hermes' guarded-model confirmation does while `session/set_model` waits). A fatal start problem ends `run` with
   `StartFailed` (after `AuthRequired` when that is the cause).
2. Every `Prompt` gets exactly one `TurnEnded` with its turn id, after every event tagged with that turn, unless `run`
   ends first (then the actor ends the turn itself).
3. Every forwarded `Permission` or `UiRequest` is answered to the agent: a tagged one before its turn's `TurnEnded`
   (the user's answer, `cancelled` at interrupt or turn end, or withdrawal); an untagged one when the user answers,
   when the agent withdraws it, or at shutdown.
4. Closed command channel = `Shutdown` with zero cancel grace.
5. Minted ids are deterministic (5.3.5), so replays compare exactly.
6. Right after the spawner's spawn note, the driver writes `LogNote::Start(setup)` (secrets excluded: no env);
   every command it receives is logged as a `cmd` record, in processing order.
7. Nothing is dropped while the receiver lives: the driver waits for permits instead, and `run` returns only after
   stdout ended (or got its grace after the exit) and every event was handed over. A failed send (the actor dropped
   the receiver at force stop) ends the driver's interest in events.
8. An event read after the driver processed `Prompt(t)` and before that prompt's reply is tagged `t`; every other
   event is untagged, including updates the agent sends after its reply and requests from background work.

Error text crosses raw: a driver holds no session redactor, so it puts stderr tails and the agent's error text into
`DriverError` and `Crashed` unredacted, and never writes them to the app log. The session actor passes every driver
error's message and detail and every `Crashed` tail through `redact_text`, with the session's secret literals, before
it shows or stores them (6.5.3, 6.5.5).

How M1b to M1d fit without rework: Claude stream-json maps `--permission-mode`, `--session-id`/`--resume` into its own
`SpawnSpec` (with `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` in `set_env`), `control_request interrupt` to `Interrupt`,
`can_use_tool` to `Permission` (AskUserQuestion to `UiRequest`), `control_cancel_request` to `PermissionWithdrawn` or
`UiWithdrawn`, `initialize.models` plus `supportedEffortLevels` to `Options`, `commands_changed` to `Commands`,
`cache_read_input_tokens`/`cache_creation_input_tokens` to `TokenUsage`, `result` to `TurnEnded`; the actor owns
the lifecycle event, so `command_lifecycle` is only logged. Codex app-server reuses `jsonrpc.rs` with
`jsonrpc_field: false`, `turn/interrupt`, approvals, user-input requests as `UiRequest` and `model/list`. Pi RPC maps
`abort`, `agent_settled` and `extension_ui_request`; Cursor's `cursor/ask_question` is a `UiRequest`. `UiPrompt` and
`UiAnswer` grow variants additively (a form of several questions). Images and mentions arrive in M2c as
`PromptPart::Image` and `PromptPart::Mention`; steering as an additive `DriverCommand::Steer` gated by
`Capabilities::steer`.

### 5.2 The JSON-RPC peer (`jsonrpc.rs`)

```rust
pub enum Incoming {
    Request { id: serde_json::Value, method: String, params: serde_json::Value },
    Notification { method: String, params: serde_json::Value },
    Response { id: serde_json::Value, result: Result<serde_json::Value, RpcError> },
    Invalid { reason: String },               // logged, never answered
}
/// One stdout line: a message, or a batch array (each entry in order). `jsonrpc` is not required.
pub fn parse_line(line: &str) -> Vec<Incoming>;
#[derive(Debug, Clone, thiserror::Error)] #[error("{message} ({code})")]
pub struct RpcError { pub code: i64, pub message: String, pub data: Option<serde_json::Value> }
pub struct RpcOut { /* LineSink, next_id: i64 from 1, jsonrpc_field */ }
impl RpcOut {
    pub fn new(sink: LineSink, jsonrpc_field: bool) -> RpcOut;
    pub fn request(&mut self, method: &str, params: serde_json::Value) -> Result<i64, StdinClosed>;
    pub fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), StdinClosed>;
    pub fn respond(&mut self, id: &serde_json::Value, result: serde_json::Value) -> Result<(), StdinClosed>;
    pub fn respond_error(&mut self, id: &serde_json::Value, code: i64, message: &str) -> Result<(), StdinClosed>;
}
pub mod codes { pub const PARSE_ERROR: i64 = -32700; pub const INVALID_REQUEST: i64 = -32600;
                pub const METHOD_NOT_FOUND: i64 = -32601; pub const INVALID_PARAMS: i64 = -32602;
                pub const INTERNAL_ERROR: i64 = -32603; pub const REQUEST_CANCELLED: i64 = -32800;
                pub const AUTH_REQUIRED: i64 = -32000; pub const RESOURCE_NOT_FOUND: i64 = -32002; }
```

Request ids we send are integers counting from 1 per process; a response whose id matches no pending request is
logged and dropped. Ids from the agent are echoed exactly as received. We never answer invalid lines.

### 5.3 The ACP driver (`acp/`)

#### 5.3.1 What auracode sends

```json
initialize        {"protocolVersion":1,
                   "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,
                                         "session":{"notices":{},"configOptions":{"boolean":{}}},
                                         "auth":{"terminal":true},"_meta":{"terminal_output":true}},
                   "clientInfo":{"name":"auracode","title":"auracode","version":"<workspace version>"}}
session/new       {"cwd":"<agent_folder>","mcpServers":[]}
session/resume    {"sessionId":"<id>","cwd":"<agent_folder>","mcpServers":[]}
session/load      {"sessionId":"<id>","cwd":"<agent_folder>","mcpServers":[]}
session/prompt    {"sessionId":"<id>","prompt":[{"type":"text","text":"..."}]}       (one block per Text part)
session/cancel    {"sessionId":"<id>"}                                  (notification)
session/set_config_option {"sessionId":"<id>","configId":"<id>","value":"<value id>"}
                          {"sessionId":"<id>","configId":"<id>","type":"boolean","value":true}
session/set_mode  {"sessionId":"<id>","modeId":"<id>"}
session/set_model {"sessionId":"<id>","modelId":"<id>"}                 (legacy, untyped)
permission reply  {"outcome":{"outcome":"selected","optionId":"<id>"}} | {"outcome":{"outcome":"cancelled"}}
```

No `_meta`, no `systemPrompt`, no `additionalDirectories`, `mcpServers` always empty: agents load their own setup and
auracode adds nothing (principle 3). `fs` and `terminal` are false (agents use their own file I/O and shells);
`elicitation` is not advertised (claude-acp then disables AskUserQuestion; M1d adds it). `Image` and `Mention` parts
are not sent in M1a (rule in 5.1).

#### 5.3.2 Startup

```text
Spawning      ctx.spawner.spawn(SpawnSpec { launch, folder, env, max_line_bytes: 64 MiB })
              ProgramNotFound -> AgentUnavailable; InvalidCwd -> FolderMissing; CwdNotAllowed -> FolderNotAllowed;
              other -> SpawnFailed
Initializing  initialize, deadline = launch.startup_timeout (60 s)
              reply protocolVersion != 1 -> ProtocolMismatch "This agent speaks ACP protocol version N; auracode
              supports version 1."; error reply -> AgentError; EOF or exit -> AgentExited (the raw stderr tail, plus
              the dialect's start hint when one matches); deadline -> StartTimeout. Recorded leniently: loadSession,
              sessionCapabilities.{resume, additionalDirectories}, promptCapabilities.image, authMethods,
              agentInfo.{name, version}.
Opening       resume = Some(id): agent advertises resume -> session/resume; else loadSession -> session/load;
                else session/new with Fresh { "this agent can't continue earlier sessions" }
              resume or load reply:
                -32002, -32601, -32602, or an error the dialect reads as not found (codex-acp: -32603 whose
                  `data.details` says "no rollout found", its answer for a thread never prompted) -> session/new and
                  Fresh { agent's message, or the dialect's reason }, with the notice "Could not continue the earlier
                  session (<message>); started a new one. The conversation above stays here, but the agent does not
                  remember it."
                -32000 -> AuthRequired
                any other error, or the open deadline -> StartFailed (AgentError or StartTimeout); the native id is
                  kept, so Retry tries to continue it again, and the card also offers Start fresh (chat_start fresh)
              resume = None (or Start fresh): session/new; -32000 -> AuthRequired; other -> AgentError
              deadline: no reply and no update for 60 s, or 10 minutes in all (a long replay keeps the open alive)
                -> StartTimeout; the process is then stopped, so a late reply can never race a second open
              while load or resume is pending: transcript updates are replay and dropped, state kinds are applied; on
                dialects whose turns outlive the process (Hermes) the driver keeps a ReplaySummary of them
              while session/new is pending: updates for an unknown sessionId are buffered (at most 256, then dropped
                with a log line) and processed once the reply names the session
Configuring   build the option model (5.5.1) and emit Options; apply autonomy (5.5.2) and emit Autonomy; re-apply
              each `desired` value that differs and is still offered, one request at a time (30 s each); a value
              no longer offered -> notice "The model you chose earlier (X) isn't offered anymore; using Y."
Ready         emit Ready { native_session_id, agent info, capabilities, resumed, spawn_id, replay }
```

On `AuthRequired` the driver emits `DriverEvent::AuthRequired(AuthPrompt)` (5.6.4), stops the process and returns
`StartFailed { kind: AuthRequired }`. `ReplaySummary` counts runs of `user_message_chunk` (user turns) and keeps the
agent text after the last of them: the session actor compares it with what it stored (6.5.2), because Hermes' gateway
can finish work after a force stop, a Stop whose cancel never landed, or a quit, and that work exists only in the
replay.

#### 5.3.3 The main loop and its phases

```rust
loop {
    tokio::select! {
        biased;
        cmd = commands.recv(), if phase == Running => match cmd { Some(c) => on_command(c), None => begin_shutdown(ZERO_GRACE) },
        permit = events.reserve(outbox_head_bytes()), if !outbox.is_empty() => match permit {
            Ok(p) => p.send(outbox.pop_front()),
            Err(ChannelClosed) => discard_events_from_now_on(),       // the actor dropped the receiver (force stop)
        },
        item = stdout.next(), if outbox.len() < 64 && outbox_bytes < 8 MiB && !stdout_done => on_item(item),
        exit = control.exited(), if !exited => on_exit(exit),          // records the exit; never ends the loop
        report = shutdown_chain, if closing => on_chain_done(report),  // control.shutdown(plan), polled in place
        () = sleep_until(deadline), if has_deadline => on_deadline(),  // request deadlines, the EOF grace
    }
    if draining_done() { break end() }   // stdout ended (EOF, or its grace ran out), the process exited, outbox empty
}
```

| Phase | What it does |
|---|---|
| Running | Commands, stdout and permits as above, inline and in wire order on one task: a reply is processed after every line the agent wrote before it. |
| Closing | Entered on `Shutdown`: steps of 5.6.2 if a turn runs, wait at most `cancel_grace` for the prompt reply, close stdin, then poll `control.shutdown(plan)` inside the loop while stdout keeps being read and events keep being forwarded, so the agent can finish writing and exit on EOF. |
| Draining | Entered at stdout EOF or process exit while Running or Closing. No more commands are read (the actor ends whatever is still open, 6.5.5; a dropped `SetOption` or `SetAutonomy` reply reads as "the agent stopped"). The outbox keeps draining, waiting for permits. stdout is read to EOF; after the exit it gets at most `PIPE_EOF_GRACE` (2 s, a descendant may hold the pipe). EOF with the process still alive: 2 s for it to exit, then `control.kill_tree()`. |
| End | `Closed` when a `Shutdown` (or the closed command channel) started it, else `Crashed { exit, stderr_tail }`. |

Outgoing messages go through `RpcOut` (never blocks). The loop waits only on its own inputs and on event-channel
permits, so it can never deadlock with the actor; while the actor is slow the outbox fills, stdout reading stops, and
the agent blocks on its own write, and nothing it wrote before exiting is lost. Requests have deadlines kept by the
loop, except `session/prompt`, which has none (an agent may work for hours). A turn still open at the end gets no
`TurnEnded` from the driver; the actor ends it (6.5.5). Deadlines use tokio's clock, which does not advance while
macOS or Linux sleeps.

#### 5.3.4 Oversized items

| `TopKeys` | Action |
|---|---|
| `id` and `method` (agent request) | `respond_error(id, -32603, "auracode can't accept a request of N MB; the limit is 64 MiB.")` |
| `id` only (response to us) | the pending request fails with `ProtocolError` "auracode dropped a response of N MB; the limit is 64 MiB." |
| no `id` | `Notice { warning, "auracode skipped an update from the agent that was larger than 64 MiB." }` |

#### 5.3.5 Ids, message boundaries and turn tags

- The actor mints `turn_id` (UUIDv7) and the user message id `<turn_id>.u`.
- The driver mints agent message ids `<turn_id>.m<n>`, thought ids `<turn_id>.t<n>`, permission and question ids
  `<turn_id>.p<n>` (counters from 1 per turn), and tool ids `<turn_id>.<toolCallId>` fixed at first sight in a
  session-wide map, so a late update to an earlier turn's tool lands on its block. A `toolCallId` over 96 chars is
  replaced by the first 16 hex chars of its sha256. Outside a turn the prefix is the `spawn_id`.
- An agent message stays open while consecutive `agent_message_chunk`s share one `messageId` or all lack one. It
  closes (`MessageCompleted`) on a chunk with another `messageId`, on any transcript update of another kind, and before
  `TurnEnded`; state updates (`usage_update`, `notice`, `session_info_update`, `available_commands_update`,
  `current_mode_update`, `config_option_update`) do not close it. Thoughts follow the same rules;
  `ThoughtCompleted.duration_ms` is the time from the first chunk to the close.
- Tags follow rule 8: the driver's current turn is set when it processes `Prompt` and cleared when the prompt's
  reply arrives.

### 5.4 The ACP to AuraEvent mapping table

`session/update` by `update.sessionUpdate` (schema 1.12.0 `SessionUpdate`; `params.sessionId` must be ours). Every row
below emits `DriverEvent::Event` tagged per rule 8 unless it names another `DriverEvent`.

| `sessionUpdate` | Output | Rules |
|---|---|---|
| `user_message_chunk` | none (logged) | the actor emits the user's text; during an open these are replay |
| `agent_message_chunk` `{content, messageId?}` | `MessageDelta { message_id, text }` | content to text (table below); ids and boundaries 5.3.5 |
| `agent_thought_chunk` | `ThoughtDelta { thought_id, text }`; `ThoughtCompleted { duration_ms }` at close | same |
| `tool_call` `{toolCallId, title, name?, kind, status, content, locations, rawInput?}` | `ToolCall { tool_call_id, kind, title, status, locations, raw_input, tool_name, content, truncated: None }` | closes an open message or thought; unknown kind -> `other`, unknown status -> `pending`, empty title -> `name` or "Tool"; a `toolCallId` already seen -> `ToolUpdate` with every field |
| `tool_call_update` | `ToolUpdate` with only the present fields (`name` -> `tool_name`); `content: Some` exactly when present (replace) | unknown id -> first a synthesized `ToolCall` (title from the update or "Tool", kind `other`, status `pending`); `rawOutput` dropped |
| tool content `{"type":"content","content":block}` | `ToolContent::Text { text }` | content to text |
| tool content `{"type":"diff",path,oldText?,newText}` | `ToolContent::Diff { path, old_text, new_text }` | `oldText` absent = new file |
| tool content `{"type":"terminal",terminalId}` | `ToolContent::Terminal { output, exit_code }` of that display terminal | since 2026-11-05: a display-only terminal (Zed's extension, which codex-acp speaks once `clientCapabilities._meta.terminal_output` is true). The agent runs the command and streams `_meta.terminal_output {terminal_id, data}` on the call's updates, then `_meta.terminal_exit {terminal_id, exit_code, signal}`; `_meta.terminal_info` names it. A running command shows its last 3,000 bytes at most every 200 ms, so live updates never spill a blob; its exit shows the last 256 KiB at once, saying how much came before, which the session stores whole. Escape sequences are dropped and a CR rewrites its line (`acp/terminals.rs`). Client-run terminals stay off (`terminal: false`), as do client file reads and writes: the agents run commands and use files themselves, and aura has no unsaved buffers to offer, the reason ACP has them |
| tool content, other `type` | dropped, logged | |
| `plan` `{entries:[{content,priority,status}]}` | `PlanUpdated { entries }` | full replacement; unknown priority -> `None`, unknown status -> `pending` |
| `available_commands_update` | `DriverEvent::Commands` | name (128 chars), description (300), `input.hint` (200) as `input_hint`; de-duplicated by name; at most 1,000; debounced 250 ms |
| `current_mode_update` `{currentModeId}` | `DriverEvent::Options` (and `Autonomy` when it leaves autonomy's mode, 5.5.2) | sets the current value of the Mode option |
| `config_option_update` `{configOptions}` | `DriverEvent::Options` | replaces every real option; synthesized ones stay |
| `session_info_update` `{title?, updatedAt?}` | `DriverEvent::Title` | string sets, `null` clears, absent keeps |
| `usage_update` `{used, size, cost?}` | `UsageUpdated { context_used: used, context_size: size, cost }` | `cost` is the session's cumulative cost |
| `notice` `{severity, title, description?}` | `Notice { level, message }` | `info`/`warning`/`error` map by name, other -> Info; message `title` + ": " + `description` |
| `compaction_update`, `compaction_summary_chunk` | none (logged) | not advertised |
| `plan_update`, `plan_removed`, `subagent_update`, `session_message`, `session_message_chunk` | none (logged) | unstable, not advertised |
| any other tag | none; logged once per tag per spawn | never fatal |
| any update while load or resume is pending | transcript kinds dropped as replay (summarized for Hermes); state kinds applied | Hermes replays on both load and resume |
| an update for another `sessionId` | none (logged); while `session/new` is pending, buffered (5.3.2) | one session per process |
| a malformed update | that update dropped (logged), the stream continues | the tag is read as a string first, then the fields leniently |

`acp/map.rs` exports `MAPPED_TAGS`, the tags of the rows above; a conformance test checks it against the schema (8.3,
A35).

Content block to text: `text` -> its text; `image` -> `[image: <mimeType>, <n> KB]`; `audio` -> `[audio: <mimeType>,
<n> KB]`; `resource_link` -> `[<title or name>](<uri>)`; `resource` with `text` -> the text; `resource` with `blob` ->
`[resource <uri>]`; other -> `[<type>]`. Images arrive in M2c.

Other agent-to-client traffic:

| Message | Handling |
|---|---|
| request `session/request_permission` | 5.6.1 |
| request `fs/read_text_file`, `fs/write_text_file`, `terminal/*`, `elicitation/create` | -32601 (not advertised) |
| any other request | the dialect's `on_request`, else -32601, so the agent never hangs |
| notification `$/cancel_request {requestId}` | withdraws a pending permission request (5.6.1); other ids ignored |
| notification `_auth/status_update` (claude-acp, codex-acp) | ignored (the log redacts its account fields); never stored |
| any other notification | the dialect's `on_notification`, else ignored (logged) |

Replies to our requests:

| Request | Success | Error |
|---|---|---|
| `initialize` | capabilities, auth methods, agent info | `ProtocolMismatch` (version), else `AgentError` |
| `session/new` | `sessionId`, `configOptions`, `modes`, legacy `models` (read raw) | -32000 `AuthRequired`; else `AgentError` |
| `session/load`, `session/resume` | the same fields | per 5.3.2 (new session, `AuthRequired`, or `StartFailed` keeping the id) |
| `session/prompt` | `stopReason`: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled` map by name; other -> `Other` plus a log line; `usage` (read raw, unstable field) -> `TokenUsage` (5.5.4) | after an interrupt any error -> `Cancelled`; -32800 -> `Cancelled`; -32000 -> `Error` with `AuthRequired` (plus `DriverEvent::AuthRequired`; the actor then stops the process, 6.5.3); other -> `Error` with the agent's message; the session stays `Ready` |
| `session/set_config_option` | the reply's complete `configOptions` replace the real options; an empty list from an agent whose open reply had no `configOptions` (Hermes) is ignored | -32602 -> `OptionInvalid` with the agent's message |
| `session/set_mode`, `session/set_model` | the synthesized option's current value changes | as above |

Capabilities reported for an ACP session: `images` = `promptCapabilities.image`; `interrupt`, `permissions`, `plans`,
`usage`, `slash_commands` true; `resume` = `loadSession` or `sessionCapabilities.resume`; `set_model` / `set_effort` =
an option of category Model / Effort exists; `add_dirs` = `sessionCapabilities.additionalDirectories`; `steer`,
`fork`, `rewind`, `limits` false; `autonomy_levels` = all three.

### 5.5 Options, autonomy, usage, dialects

#### 5.5.1 The option model

- `configOptions` entries: `select` -> `ConfigKind::Select` (grouped choices flattened, the group name kept in
  `ConfigChoice::group`), `boolean` -> `Toggle`, other types skipped. Category `mode` -> Mode, `model` -> Model,
  `thought_level` -> Effort, `model_config` -> ModelConfig, missing or other -> Other. Route: `set_config_option`.
- Legacy `modes {currentModeId, availableModes:[{id,name,description?}]}` -> a synthesized option `mode` (category
  Mode) only when no real option has category Mode. Route: `session/set_mode`.
- Legacy `models {currentModelId, availableModels:[{modelId,name,description?}]}`, read from the raw reply -> a
  synthesized option `model` (category Model) only when no real option has category Model. Route:
  `session/set_model` (untyped). So codex-acp uses its config options and Hermes its legacy lists.
- `SetOption`: the value must be one of the offered choices (else `OptionInvalid`, without contacting the agent). On a
  `mode_change_requires_idle` dialect, a Mode change during a turn sets `pending`, replies `Ok`, emits a notice
  "Hermes changes mode when the current turn ends." and is sent after `TurnEnded`. Every change ends in
  `DriverEvent::Options` with the full list. Limits: 100 options, 500 choices each, names 200 chars, descriptions 300.

Agent facts: claude-acp ids `mode`, `model`, `effort` (`thought_level`), `fast` (boolean), `agent`; codex-acp ids
`mode`, `model`, `reasoning_effort`, `fast` plus a legacy `models` list that is ignored; Hermes legacy `modes`
(`default`, `accept_edits`, `dont_ask`) and `models` (`provider:model`), `set_config_option` honours only
`edit_approval_policy` and returns `configOptions: []`.

#### 5.5.2 Autonomy

The dialect turns the requested level into ordered candidates; the driver applies the first one whose mode is among
the Mode option's choices (or that has no mode) and whose mode change succeeds, then appends a last resort itself: no
mode change, `AutoAnswer::Never`, note "auracode could not switch <agent> to a matching mode (<reason>), so it asks
before every action."

| Dialect | Full auto (default) | Auto-edit | Ask |
|---|---|---|---|
| `claude-acp` | 1. mode `bypassPermissions`, `Never`; 2. mode `acceptEdits`, `Never`, note "Claude Code's settings turn off bypass mode, so Full auto runs as Accept edits and auracode asks before other actions." | `acceptEdits`, `Never` | `default`, `Never` |
| `codex-acp` | mode `agent-full-access` (approval never, full access), `Never` | `workspace-write`, `Never` | `read-only`, `Never` |
| `hermes` | mode `dont_ask`, `Kinds [execute]` (Hermes' dangerous-command approvals, the set its own `--yolo` bypasses; sensitive-path edit asks reach the user) | `accept_edits`, `Never` | `default`, `Never` |
| `generic` | no mode change, `All` (the plan: auto-answer when no bypass is advertised; tool kinds only, 6.6) | no mode change, `Kinds [read, search, think, edit, delete, move]` | no mode change, `Never` |
| `mock` | mode `full_auto`, `All` | `auto_edit`, `Kinds [read, search, think, edit, delete, move]` | `ask`, `Never` |

The mode is always set, because each agent's default is not ours: claude-acp starts in the user's
`permissions.defaultMode` (this user: `auto`), codex-acp in `agent` (auto review), Hermes in `default`. `SetAutonomy`
takes effect at once for the policy; the mode change is sent at once, or after the turn on
`mode_change_requires_idle` dialects.

A Mode picked by hand (`SetOption` on the Mode option) or reported by the agent (`current_mode_update`) that differs
from the applied candidate's mode turns auto-answers off: `AutonomyState { requested, native_mode: Some(<that
mode>), auto_answer: Never, note: "You picked the <mode name> mode by hand, so auracode asks before every action.
Choose an autonomy level to hand control back." }`, emitted as `Autonomy`. Picking the candidate's mode again, or
any `SetAutonomy`, restores the candidate.

#### 5.5.3 Dialect hooks (`acp/dialect.rs`)

```rust
pub trait AcpDialect: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate>;
    fn prompt_usage_scope(&self) -> UsageScope;
    fn mode_change_requires_idle(&self) -> bool { false }
    /// Keep a ReplaySummary while load or resume replays (5.3.2).
    fn replay_may_hold_unseen_work(&self) -> bool { false }
    /// A sign-in command the user runs with their own binary, for one advertised auth method.
    fn auth_command(&self, method: &wire::AuthMethod, launch: &LaunchSpec, env: &EnvMap) -> Option<AuthCommand>;
    /// Fallback commands when the agent advertises none (codex-acp: `codex login`).
    fn fallback_auth_commands(&self, _launch: &LaunchSpec, _env: &EnvMap) -> Vec<AuthCommand> { Vec::new() }
    /// A sentence added to a start failure whose stderr tail matches (Hermes on Windows, 3.2).
    fn start_failure_hint(&self, _stderr_tail: &str) -> Option<String> { None }
    /// An error answer to a resume or load that says the session does not exist, beyond -32002, -32601 and
    /// -32602: the reason the fallback's notice gives (5.3.2).
    fn open_not_found(&self, _error: &RpcError) -> Option<String> { None }
    fn on_request(&self, _method: &str, _params: &serde_json::Value) -> DialectReply { DialectReply::NotMine }
    fn on_notification(&self, _method: &str, _params: &serde_json::Value) -> Vec<DriverEvent> { Vec::new() }
}
pub struct AutonomyCandidate { pub mode: Option<String>, pub auto_answer: AutoAnswer, pub note: Option<String> }
pub enum DialectReply { NotMine, Respond(serde_json::Value), Error { code: i64, message: String } }
pub fn dialect_for(id: &str) -> Box<dyn AcpDialect>;      // unknown ids get the generic dialect
```

Default bodies name their unused parameters with a leading underscore (clippy's `-D warnings` rejects them otherwise,
as the build review's compile showed).

| | `hermes` | `claude-acp` | `codex-acp` | `generic` |
|---|---|---|---|---|
| usage scope | Session | Turn | Turn | Session (the schema documents session totals) |
| mode change only while idle | yes (Hermes answers -32603 and posts an agent message while busy) | no | no | no |
| replay may hold unseen work | yes (its gateway outlives the viewer) | no | no | no |
| auth commands | terminal method `hermes-setup` `["--setup"]` -> `hermes acp --setup` (the launch command plus the method's args) | terminal methods whose args start with `--cli` -> `[<user's claude>, rest]`, so `claude auth login --claudeai` and `--console` | none advertised as terminal -> fallback `[<user's codex>, "login"]` | launch command plus the method's args |
| start failure hint | `windows_breakaway_unavailable` -> the 3.2 hint | none | none | none |
| open not found | none | none | -32603 whose `data.details` holds "no rollout found" (a thread never prompted has no rollout), reason: the details | none |

A terminal method's `env` (schema `AuthMethodTerminal.env`) is copied into `AuthCommand.env` and shown with the
command.

#### 5.5.4 Usage

`PromptResponse.usage {inputTokens, outputTokens, cachedReadTokens?, cachedWriteTokens?, thoughtTokens?}` maps to
`TokenUsage` field by field. Turn-scope dialects report `scope: Turn`. Session-scope dialects: with a previous
cumulative value from this process, the driver reports the saturating difference as `scope: Turn`; without one (the
first turn after a spawn) it reports `scope: Session`, labelled "this session" in the UI.

### 5.6 Permissions, questions, cancellation, auth

#### 5.6.1 Permission requests

1. Parse leniently: `sessionId`, `toolCall` (tool call update fields), `options [{optionId, name, kind}]`, unknown kind
   -> `Other`. Params that do not parse: reply -32602.
2. While the turn is being cancelled: reply `cancelled` at once, forward nothing (logged).
3. Otherwise mint `request_id`, keep `{ rpc id, option ids, turn tag }`, and emit `DriverEvent::Permission { turn,
   request: PermissionRequest { request_id, tool_call_id: the minted id when the toolCallId names a block already
   shown, else None, title, kind, raw_input, content, locations, options } }`. No tool block is synthesized from a
   permission request (Hermes uses fresh `perm-check-N` ids that never become tool calls). Requests outside a turn,
   and before `Ready`, are forwarded the same way, untagged (a background task, or Hermes confirming a guarded model
   while `session/set_model` waits).
4. `RespondPermission { Selected(id) }` -> `{"outcome":{"outcome":"selected","optionId":id}}`; `Cancelled` ->
   `{"outcome":{"outcome":"cancelled"}}`. An unknown request id (already withdrawn) is ignored.
5. `$/cancel_request` naming a pending request's rpc id: reply `cancelled` anyway (the receiver must still answer),
   forget it, emit `PermissionWithdrawn`.
6. At turn end every request tagged with that turn and still pending is answered `cancelled` (Hermes auto-denies after
   its own 300 s timeout without withdrawing; this clears it). Untagged requests stay pending until answered, withdrawn
   or shut down.

ACP questions arrive as permission requests (Hermes' clarify prompts use kind `other` with one `allow_once` option per
choice), so in M1a they are permission cards that are never auto-answered (6.6). `UiRequest` is produced by the mock
only; ACP elicitation (M1d) will map to it.

#### 5.6.2 Cancellation, exactly

On `Interrupt` while a prompt is pending and not already cancelling: status `Cancelling`; send `session/cancel` (a
notification); answer every pending request tagged with the turn `cancelled`; answer any later request `cancelled` at
once; keep waiting for the `session/prompt` reply, mapping updates as usual. The reply ends the turn with its own
`stopReason` (an agent that finished first may say `end_turn`, which is shown as the truth); an error reply after a
cancel counts as `Cancelled`. The pending prompt request is never dropped or withdrawn (`$/cancel_request` is a
different mechanism agents may ignore). There is no timer: an agent that never answers stays `Cancelling` until the
user force-stops it (claude-acp settles a wedged cancel itself after 30 s). `Interrupt` with no prompt pending does
nothing.

#### 5.6.3 Shutdown

`Shutdown { plan }`: the Closing phase of 5.3.3. Steps of 5.6.2 if a turn runs; wait at most `plan.cancel_grace` for
the prompt reply (`TurnEnded` with its reason; on timeout the actor ends the turn `Interrupted`); answer every untagged
pending request `cancelled`; close stdin; poll `control.shutdown(plan.process)` while stdout keeps being read; drain;
log the report; return `Closed`.

#### 5.6.4 Auth required

JSON-RPC -32000 on `session/new`, `session/load`, `session/resume` or `session/prompt`. `AuthPrompt.message`: "<agent>
needs you to sign in. Run the command below in a terminal, then press Retry." `commands`: one per terminal method
through `auth_command`, else the dialect's fallback. Agent-type methods are never called in M1a (codex-acp's
`api-key` would make auracode carry a credential; browser flows wait for M3's sign-in sheet).

### 5.7 The mock driver (`mock_driver.rs`)

```rust
pub struct MockBehavior { pub tokens_per_sec: u32, pub startup_delay: Duration, pub fail_start: Option<DriverError> }
impl Default for MockBehavior { /* 120 tokens/s, 300 ms, None */ }
pub struct MockDriver { behavior: MockBehavior }
impl MockDriver { pub fn new(behavior: MockBehavior) -> MockDriver; }
impl AgentDriver for MockDriver {}
```

No process. It reuses `mock.rs`'s timeline through a new event-level `play(script, cancel, emit)` (`run_script` keeps
its signature for the M0 demo stream and stress view), drops a script's `TurnStarted` and `SessionUpdated`, turns its
`TurnEnded` into `DriverEvent::TurnEnded`, and offers synthetic options (Model `mock-fast`/`mock-smart`, Effort
`low`/`medium`/`high`, Mode `full_auto`/`auto_edit`/`ask`, a `fast` toggle) and 40 slash commands. Native ids are
`mock-<uuidv7>`; a resume quietly starts new. Magic prompts:

| Prompt | Behaviour |
|---|---|
| `/echo <text>` | one `MessageDelta` with `<text>`, `TurnEnded { EndTurn }` with tiny usage |
| `/permission` | an `execute` tool call and a permission request (Allow once, Reject once); allow: tool completed, "Done."; reject: tool failed, "Skipped." |
| `/withdraw` | a permission request withdrawn after 1 s |
| `/ask` | a `UiRequest` `Select { "Which color?", ["Red", "Green", "Blue"] }`; an answer: "You picked Green."; dismissed: "No answer." |
| `/background` | ends the turn, then 500 ms later sends an untagged tool update for the turn's tool and an untagged permission request (late output, rule 8) |
| `/slow` | a long answer at 20 tokens/s |
| `/hang` | streams a little, never ends |
| `/ignore-cancel` | ignores `Interrupt` (force-stop tests) |
| `/fail` | `TurnEnded { Error, AgentError "The demo agent failed this turn on purpose." }` |
| `/crash` | `run` returns `Crashed` mid-turn with a scripted stderr tail |
| `/auth` | `AuthRequired` then `TurnEnded { Error }` with `AuthRequired` |
| `/big` | a 200 KB tool output (blob spill) |
| `/stress <n>` | `MockScript::stress` with n tokens |
| anything else | `MockScript::demo_turn` |

`testing.rs` (feature `testing`, frozen with P0, used by C's tests):

```rust
pub enum Step {
    Expect(CommandMatch),          // waits for the next command; a mismatch panics the driver task (Panicked)
    Emit(DriverEvent),
    EmitInTurn(DriverEvent),       // tagged with the last Prompt's turn: the actor mints turn ids (5.3.5)
    EndTurn { stop_reason: StopReason, usage: Option<TokenUsage>, error: Option<DriverError> },   // for that turn
    Wait(Duration),                // virtual time under paused clocks
    Spawn(LaunchSpec),             // spawns through ctx.spawner and keeps the process (recorded controls)
    Exit(DriverExit),              // run returns this
    Hang,                          // never emits or exits again; keeps reading commands (a wedged driver)
}
pub enum CommandMatch { Prompt, Interrupt, RespondPermission { request_id: Option<String> },
                        RespondUi { request_id: Option<String> }, SetOption, SetAutonomy, Shutdown, Any }
pub struct ScriptedDriver { /* steps, handle state */ }
impl ScriptedDriver { pub fn new(steps: Vec<Step>) -> (ScriptedDriver, ScriptHandle); }
#[derive(Clone)] pub struct ScriptHandle { /* shared */ }
impl ScriptHandle {
    pub fn emit(&self, event: DriverEvent);                    // inject at any time, in order with the steps
    pub fn exit(&self, exit: DriverExit);
    pub fn commands(&self) -> Vec<LoggedCommand>;              // everything received so far
    /// Waits for, then returns, the first received command `matcher` matches that no earlier `wait_for` returned
    /// (a Prompt, to read the turn id the actor minted).
    pub async fn wait_for(&self, matcher: CommandMatch) -> LoggedCommand;
    pub fn set_option_reply(&self, reply: Result<(), DriverError>);    // default: Ok(())
    pub fn set_autonomy_reply(&self, state: AutonomyState);   // default: requested level, no mode, Never
    pub fn set_autonomy_result(&self, reply: Result<AutonomyState, DriverError>);   // or an error
}
pub struct ScriptedDriverFactory { /* queue */ }
impl ScriptedDriverFactory { pub fn new(drivers: Vec<ScriptedDriver>) -> Arc<ScriptedDriverFactory>; }
impl DriverFactory for ScriptedDriverFactory {}            // hands out the queued drivers in order
pub struct DirectSpawner { /* base spawner, log factory */ }
impl DirectSpawner { pub fn new(base: Arc<dyn ProcessSpawner>, logs: LogFactory) -> Arc<DirectSpawner>; }
impl AgentSpawner for DirectSpawner {}
```

`SetOption` and `SetAutonomy` get the handle's replies: `Ok` and the default state unless the handle says otherwise
(an `Err` stands for an agent that refused, as the ACP driver's -32602, which the manager reports as `OptionInvalid`).
`EmitInTurn` and `EndTurn` before any `Prompt` panic the driver task. `ScriptedDriver` keeps contract rules 2, 3 and 7
only as far as its steps say (C's tests use it to break them on purpose). `factory.rs`: `StandardDriverFactory::new() ->
Arc<StandardDriverFactory>` creates `AcpDriver` for `DriverKind::Acp` and `MockDriver::new(MockBehavior::default())`
for `DriverKind::Mock`.

### 5.8 Replay harness (`replay/`, feature `replay`)

```rust
pub struct Fixture { pub name: String, pub records: Vec<FixtureRecord> }
pub enum FixtureRecord { Start(serde_json::Value), Out(serde_json::Value), In(serde_json::Value), InRaw(String),
                         Err(String), Cmd(LoggedCommand), Agent(AgentAction), Note(serde_json::Value) }
pub enum AgentAction { Eof, Exit { code: i32 }, Oversized { bytes: u64, keys: serde_json::Value }, Wait { ms: u64 } }
impl Fixture { pub fn load(path: &Path) -> io::Result<Fixture>; pub fn parse(name: &str, text: &str) -> Result<Fixture, FixtureError>; }
pub struct ReplayReport { pub events: Vec<serde_json::Value>, pub replies: Vec<serde_json::Value>,
                          pub exit: serde_json::Value, pub mismatches: Vec<String> }
/// Runs the real AcpDriver over FakeSpawner pipes against a fake agent playing `fixture`.
pub async fn run(fixture: &Fixture) -> ReplayReport;
pub fn assert_golden(report: &ReplayReport, golden: &Path);       // AURA_UPDATE_GOLDEN=1 rewrites
/// A raw protocol log into a fixture: export redaction, then `scan_clean`.
pub fn curate(log_text: &str, home: &str, cwd: &str, user: &str, host: &str) -> Result<String, CurateError>;
pub fn scan_clean(text: &str) -> Vec<String>;                     // emails, token shapes, home paths
```

A fixture is a curated protocol log (3.8 format) plus fixture-only `{"d":"agent", ...}` records. On a current-thread
runtime with paused time, `run` takes the `SessionSetup` from the `start` note (cwd `<CWD>`, test timings) and spawns
through `FakeSpawner` with `ProtocolLog::disabled_with_id(<the start note's spawn_id>)`, so the ids the driver mints
outside a turn match the recording; then it walks the records: `cmd` sends the matching `DriverCommand` (replies
collected); `in` writes the line to the driver, a response's `id` rewritten to the live id of the request it answers;
`out` reads the next line the driver wrote (5 s virtual timeout) and compares kind, `method`, id correspondence, the
permission `outcome`, and for `session/prompt`, `session/set_config_option`, `session/set_mode` and `session/set_model`
the text, ids and value (paths and session ids match placeholders); `err` goes to the fake stderr; `agent` records act.
After each record, `tokio::time::sleep(1 ms)` lets every task go idle, so interleaving is deterministic. The collected
`DriverEvent`s (durations blanked, turn tags kept) and replies are compared with `<fixture>.events.json`.

The harness and its tests exist only with the `replay` feature: every `--workspace` test and clippy run has it
(aura-core's dev-dependency, 1.1), and `cargo test -p aura-drivers --features replay` runs them alone. So
`tests/replay.rs` starts with `#![cfg(feature = "replay")]` (`cargo test -p aura-drivers` then builds it empty), and no
other `tests/**` file uses `aura_drivers::testing`, which only workspace runs and `replay` enable.

Curation (`aura-cli fixture`): the log's redaction re-run, then literal `<HOME>`, `<USER>`, `<HOST>`, `<CWD>` (and
Claude's project-dir spelling, as the spike's `Redactor::add_path`), an npm adapter's install folder (the spawn note's
`cwd` when it is not the chat folder, under whatever data root the run used) to `<ADAPTER>`, native session ids to
`<SESSION_n>`, ISO timestamps to `<TIME>`, `t` made relative; then `scan_clean` must return nothing. A log holding a
capped record (3.8) cannot become a fixture: `curate` refuses it with the line and the bytes cut, and `Fixture::parse`
refuses one found in a fixture. Fixtures live in `crates/aura-drivers/tests/fixtures/acp/<agent>/<scenario>.jsonl` with
`<scenario>.events.json` beside them, and a test scans every committed fixture for `@` email shapes, token prefixes,
`/Users/`, `/home/`, `C:\Users\`.

Synthetic fixtures written before any real run (B owns them): `new-session`, `new-session-early-update`,
`legacy-models-modes`, `load-replay`, `open-replay-long` (the open deadline resets on each replayed update),
`resume-fallback` (-32002, -32601), `resume-error-keeps-id` (-32603 ends the start, the id stays),
`hermes-continued-after-quit` (the replay's last reply differs from what was stored), `hermes-replay-matches`,
`auth-required`, `protocol-mismatch`, `turn-basic` (chunks with and without `messageId`, a thought, tool call, update
and diff, plan, usage), `unknown-update`, `prompt-auth-error`, `prompt-error`, `permission-ask`,
`permission-full-auto`, `permission-withdrawn`, `permission-outside-turn`, `permission-during-setup` (asked while
`session/set_model` waits), `question-kind-other` (a Hermes clarify request under the generic dialect's Full auto),
`updates-after-reply` (untagged), `hand-picked-mode`, `cancel` (interrupt with an open permission, updates after the
cancel, `cancelled` reply), `agent-requests`, `garbage` (non-JSON, non-RPC JSON, a batch), `oversized`,
`eof-mid-turn`, `exit-with-buffered-output`, `mode-idle-only`, `claude-no-bypass`, `hermes-usage-session`.

---

## 6. aura-core: storage and the session manager

### 6.1 The profile owner and the chat store (`owner.rs`, `chatstore/`)

```rust
// owner.rs
pub struct ProfileOwner { /* the locked <profile>/owner.lock */ }
impl ProfileOwner {
    /// Takes `<profile>/owner.lock` exclusively (std `File::try_lock`). Held by another process ->
    /// `CoreError::ProfileInUse`; a filesystem that cannot lock -> Ok with a warning, as the store's open lock does.
    pub fn acquire(profile: &Profile) -> Result<ProfileOwner, CoreError>;
}
```

`CoreError::ProfileInUse { path }` reads "auracode is already running with the profile in <path>. Quit it first, or
start this copy with its own AURACODE_DATA_DIR." The desktop's boot takes the owner lock right after `open_active()`
and before `Store::open` (a failure becomes the startup error screen through `Booted.fatal`); `aura-cli` does the same
and refuses with that message. Only the owner can call `ChatStore::start`, so only the owner runs recovery, actors
and agent spawns. M0's integrity check reads through its own read-only connection in the same process and is
unaffected. Smoke runs use their own temporary data folder (`.github/scripts/run-smoke.sh`) and never meet the lock.

```rust
pub struct ChatStore { /* writer thread, FIFO sender, read-channel sender, status watch, unsaved-op counters */ }
impl ChatStore {
    /// Moves the opened Store and the owner lock into the writer thread; global recovery (6.3.3) is its first op.
    pub fn start(store: Store, owner: ProfileOwner, profile: &Profile, device_id: String, rt: Handle)
        -> Result<Arc<ChatStore>, CoreError>;
    pub fn status(&self) -> watch::Receiver<StorageStatus>;
    pub fn is_paused(&self) -> bool;
    pub async fn recovered(&self) -> RecoveryReport;
    /// The backpressure point for actors: room for one op of about `bytes` in the FIFO.
    pub async fn reserve(&self, bytes: usize) -> Result<OpPermit, StoreClosed>;
    pub async fn create_chat(&self, new: NewChat) -> Result<ChatRecord, StoreError>;
    pub async fn chats(&self) -> Result<Vec<ChatRecord>, StoreError>;
    /// Per-chat recovery of turns left `running` (an actor that died), then everything an actor needs.
    pub async fn load_chat(&self, chat_id: &str) -> Result<ChatLoad, StoreError>;
    pub async fn read_blob(&self, sha256: &str, offset: u64, len: u32) -> Result<BlobText, StoreError>;
    /// While paused: the page from saved rows only, with `unsaved_from` set (6.8).
    pub async fn read_saved_page(&self, chat_id: &str, before_seq: u32, max_bytes: usize)
        -> Result<HistoryPage, StoreError>;
    pub async fn acknowledge_recovery(&self, quarantined: String) -> Result<bool, CoreError>;
    pub async fn shutdown(&self, deadline: Duration) -> bool;            // commits what is queued, closes
    /// The same from any thread, with or without a runtime (RunEvent::Exit); never panics.
    pub fn shutdown_blocking(&self, deadline: Duration) -> bool;
}
pub struct OpPermit { /* FIFO permit plus byte permit */ }
impl OpPermit { pub fn send(self, op: Op); }

pub enum Op {
    InsertAgentSession(AgentSessionRow),
    UpdateAgentSession { id: String, patch: AgentSessionPatch },
    /// Inserts the turn, appends `rows` (TurnStarted, the user's text, ...) and deletes the queued prompt
    /// `turn.prompt_id`, in one transaction: a crash keeps either the queued prompt or the started turn.
    BeginTurn { turn: TurnRow, rows: Vec<EventRow> },
    AppendEvents { chat_id: String, rows: Vec<EventRow> },
    EndTurn(TurnEnd),
    CompactTurn { chat_id: String, turn_id: String },
    PutBlob { sha256: String, mime: String, bytes: Arc<[u8]> },
    SetTitle { chat_id: String, title: String, source: TitleSource },
    TouchChat { chat_id: String, at: Millis },
    QueuePut(QueuedRow),
    QueueRemove { id: String },
    QueueHoldAll { chat_id: String },
    QueueReleaseAll { chat_id: String },
    /// History read through the actor's outbox, so a page follows every write the actor queued before it (6.8).
    /// The latest page is `before_seq = next_seq`.
    ReadPage { chat_id: String, before_seq: u32, max_bytes: usize,
               reply: oneshot::Sender<Result<HistoryPage, StoreError>> },
    Barrier(oneshot::Sender<()>),
}
pub struct NewChat { pub agent_id: String, pub folder: PathBuf, pub device_id: String, pub now: Millis }
pub struct ChatRecord { pub id: String, pub title: Option<String>, pub title_source: Option<TitleSource>,
                        pub agent_id: String, pub folder: PathBuf, pub created_at: Millis, pub updated_at: Millis }
pub struct ChatLoad { pub chat: ChatRecord, pub next_seq: u32, pub next_turn_ordinal: u32,
                      pub agent_session: Option<AgentSessionRow>, pub queue: Vec<QueuedRow>,
                      pub recovered_turns: u32, pub replay_check: Option<ReplayCheck> }
/// For the Hermes replay comparison (6.5.2): the latest agent session's turn count and its newest reply's hash.
pub struct ReplayCheck { pub native_session_id: String, pub turns: u32, pub last_reply_sha256: Option<String> }
pub struct AgentSessionRow { pub id: String, pub chat_id: String, pub agent_id: String, pub native_session_id: Option<String>,
                             pub model: Option<String>, pub effort: Option<String>, pub autonomy: Autonomy,
                             pub config_json: Option<String>, pub cwd: PathBuf, pub created_at: Millis,
                             pub updated_at: Millis }
pub struct AgentSessionPatch { pub native_session_id: Option<String>, pub model: Option<String>, pub effort: Option<String>,
                               pub autonomy: Option<Autonomy>, pub config_json: Option<String>, pub updated_at: Millis }
pub struct TurnRow { pub id: String, pub chat_id: String, pub agent_session_id: Option<String>, pub ordinal: u32,
                     pub prompt_id: String, pub first_event_seq: u32, pub started_at: Millis }
pub struct TurnEnd { pub turn_id: String, pub status: TurnStatus, pub stop_reason: StopReason,
                     pub ended_at: Millis, pub last_event_seq: u32 }
pub enum TurnStatus { Running, Completed, Interrupted, Failed }
pub struct EventRow { pub seq: u32, pub turn_id: Option<String>, pub kind: &'static str, pub payload: String,
                      pub created_at: Millis }
pub struct QueuedRow { pub id: String, pub chat_id: String, pub text: String, pub held: bool, pub created_at: Millis }
pub enum TitleSource { FirstLine, Agent }
pub struct RecoveryReport { pub interrupted_turns: u32, pub resolved_requests: u32, pub compacted_turns: u32 }
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    #[error("The disk is full, so auracode paused saving. Free some space, then try again.")] DiskFull,
    #[error("The database is busy.")] Busy,
    #[error("auracode's database is closed.")] Closed,
    #[error("auracode's database is damaged: {0}")] Damaged(String),
    /// "The full output is no longer available." (the file is gone), or "The full output isn't saved yet because the
    /// disk is full." (it waits in the failed batch or the FIFO); passed on as SessionError::BlobMissing.
    #[error("{0}")] BlobMissing(String),
    #[error("{0}")] Other(String),
}
pub struct StoreClosed;
```

Writer thread rules:

- **FIFO, group commit.** Blocks on the first op, drains up to 256 ops or 4 MiB, runs them in one `BEGIN IMMEDIATE`
  transaction, commits, answers replies. An op is atomic: it never spans transactions. A read op ends the batch: the
  batch commits first, then the read runs on the same connection, so it sees every op queued before it. Per-chat
  order holds because each chat's ops come from one actor over one FIFO. The FIFO is bounded at 1,024 ops and 32 MiB
  (one larger op passes alone). The store counts each chat's unsaved ops (sent and not yet committed).
- **Disk full** (`SQLITE_FULL`, or `ENOSPC` writing a blob): roll back, keep the failed batch, status
  `Paused { DiskFull }`, retry the batch every 5 s, and stop taking ops from the FIFO. The FIFO and then the actors'
  outboxes fill, the actors stop reading driver events, the drivers stop reading stdout and the agents block. Between
  retries the writer serves a second channel from saved data: `chats`, `read_blob` (saved files; a blob still in the
  failed batch or the FIFO answers `BlobMissing` with "The full output isn't saved yet because the disk is full."),
  `read_saved_page`, and `load_chat` for a chat with no unsaved ops and no running turn. While paused the
  `ChatStore` routes those calls to the second channel; FIFO reads queued before the pause time out after 5 s with
  `StoreError::DiskFull`. Writes (`create_chat`, `acknowledge_recovery`) fail after their bound. The banner reads "The
  disk is full. auracode paused saving and the agents are waiting. Free some space; nothing is lost unless you quit
  now." Live frames keep flowing until the backpressure reaches the agents.
- **Busy** (`SQLITE_BUSY` past the 5 s busy timeout; another auracode never writes this profile, so only outside
  tools can cause it): retry with backoff from 100 ms to 2 s; after 10 s, `Paused { Busy }` with the same routing.
- **Poison isolation.** Any other error: roll back, re-run the batch one op per transaction; an op that fails again
  goes to `<profile>/backups/deadletter-<millis>.jsonl` (the op as JSON) with an error log. A panic inside a batch is
  caught (`catch_unwind`), rolled back, isolated the same way; the thread continues.
- **Damage** (`SQLITE_CORRUPT`, `SQLITE_NOTADB`): `Store::note_query_error` records it for the next open's repair;
  status `Failed { "auracode's database is damaged. Restart auracode to repair it; recent messages are kept in a
  recovery file." }`; every later data op goes to the dead-letter file so sessions keep running.
- **Blobs:** `<profile>/attachments/<sha256>`, written `.partial`, synced, renamed, plus `INSERT OR IGNORE` into
  `attachments`; at most 64 MiB stored per blob (a longer original keeps its first 64 MiB and `total_bytes` tells
  how much there was). `PutBlob` is queued before the op that holds the row referencing it, so no row ever names a
  missing file.
- **Shutdown** commits everything queued within the deadline, then closes the connection, which folds the WAL back so
  the next launch sees a clean shutdown and skips the integrity check. A paused writer gives up at the deadline and
  logs what it drops (op counts per chat, never content). `shutdown_blocking` does the same over a std channel.

No read pool in M1a: chat lists, pages and blob reads are small, and consistency with writes matters more. In normal
operation every public method sends its op through the FIFO; a call that gets no answer within 5 s fails with
`StoreError::Busy`, which the manager reports as `StorageUnavailable`.

### 6.2 Migration 0002 (`migrations/0002_sessions.sql`, `SCHEMA_VERSION = 2`)

```sql
-- Schema v2: what M1a's session manager needs on top of v1.
-- Only ADD COLUMN and new objects: nothing is rebuilt, existing rows keep their values (new columns are NULL).

-- The user's explicit option choices, re-applied when the agent starts: JSON {option_id: ConfigValue}.
ALTER TABLE agent_sessions ADD COLUMN config_json TEXT;

-- The prompt a turn came from, the seq range its events occupy, and when it was compacted.
ALTER TABLE turns ADD COLUMN prompt_id TEXT;
ALTER TABLE turns ADD COLUMN first_event_seq INTEGER;
ALTER TABLE turns ADD COLUMN last_event_seq INTEGER;
ALTER TABLE turns ADD COLUMN compacted_at INTEGER;
-- History pages walk turns by their first seq; recovery reads only unfinished or uncompacted turns.
CREATE INDEX turns_chat_first_event_seq ON turns (chat_id, first_event_seq);
CREATE INDEX turns_running ON turns (chat_id) WHERE status = 'running';
CREATE INDEX turns_uncompacted ON turns (chat_id) WHERE status <> 'running' AND compacted_at IS NULL;

-- Permission requests, questions and their resolutions: recovery finds unanswered ones without scanning events.
CREATE INDEX events_requests ON events (chat_id, seq)
    WHERE kind IN ('permission_requested', 'permission_resolved', 'ui_request', 'ui_resolved');

-- Device-local and never synced: prompts waiting behind a running turn, so a crash can't lose them.
-- A row is removed when its prompt starts (in the BeginTurn transaction) or is cancelled.
-- held = 1: queued before a stop, error, crash or restart; sent only after the user says so.
CREATE TABLE queued_prompts (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats (id),
    text TEXT NOT NULL,
    held INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    device_id TEXT NOT NULL
);
CREATE INDEX queued_prompts_chat_id ON queued_prompts (chat_id, created_at);
```

Conventions kept: the new foreign key leads an index (the FK-index test's count goes from 9 to 10);
`queued_prompts` is transient device-local state like `folder_locations`, so it is hard-deleted. `MIGRATIONS` gains
`M::up(include_str!("../migrations/0002_sessions.sql")).foreign_key_check()`; P0 updates the `TABLES` lists in the
`store.rs` and `salvage.rs` tests and adds a test that upgrades a populated v1 database (backup taken, foreign key
check clean, new columns `NULL`, `held` 0). `TurnRow.ordinal` is v1's `turns.seq` (1-based per chat,
`UNIQUE (chat_id, seq)`). `turns.status` keeps v1's CHECK: `EndTurn`, `MaxTokens`, `MaxTurnRequests`, `Refusal`,
`Cancelled`, `Other` -> `completed`; `Interrupted` -> `interrupted`; `Error` -> `failed`; `stop_reason` holds the
detail. `chats.title_source` takes `first_line` or `agent`.

`create_chat` reuses the project, folder and location already recorded for this path on this device, else writes
`projects` (named after the folder), `project_folders` (`kind = 'local'`, `origin_device_id` = this device),
`folder_locations` (the path) and `chats` (`active_agent` = the agent id).

### 6.3 Compaction, recovery, paging

#### 6.3.1 Per-turn compaction (`compact.rs`, pure; `Op::CompactTurn` applies it in one transaction)

| Rows of one turn | Kept as |
|---|---|
| `UserMessageDelta`, `MessageDelta`, `ThoughtDelta` per id | concatenated in seq order into pieces of at most 1 MiB, each piece one event, at the seqs of the first deltas of each piece |
| `ToolCall` and every `ToolUpdate` for an id minted in this turn (`<turn_id>.` prefix) | one `ToolCall` with the final kind, title, status, locations, raw input, name, content and truncation, at the `ToolCall`'s seq (no `ToolCall` stored: a synthesized call at the first update's seq) |
| `ToolUpdate`s for an id minted elsewhere (another turn's or a `spawn_id` prefix) | one merged `ToolUpdate` (the fields last set) at the first such update's seq, never a synthesized call |
| `PlanUpdated`, `UsageUpdated` | the last of each |
| `TurnStarted`, `MessageCompleted`, `ThoughtCompleted`, `PermissionRequested`, `PermissionResolved`, `UiRequest`, `UiResolved`, `Notice`, `Error`, `TurnEnded`, and rows that do not parse | unchanged, the payload copied verbatim |

Rows stored outside turns are never compacted. Kept events keep original seqs (a subset of the inputs', so
`UNIQUE (chat_id, seq)` holds and order is preserved; `TurnEnded`, the turn's highest tagged seq, is always kept, so
`MAX(seq)` never drops). The transaction deletes the turn's rows, inserts the kept ones (new UUIDv7 row ids), inserts
one `message_fts` row per user message and per agent message (text at most 1 MiB), and sets `compacted_at`. Guarded by
`compacted_at IS NULL`, so it runs once. A turn whose payload exceeds 32 MiB is not merged (memory stays bounded);
only its FTS rows are written, streamed row by row. Property test: a test-only Rust reducer in `compact.rs` (messages
and thoughts by id, each tool's final state, the last plan, permissions and questions with their resolutions) gives
the same result for generated live event sequences and their compacted rows, including sequences that span turns,
updates to earlier turns' tools, and updates stored outside turns.

#### 6.3.2 History pages (`history.rs`)

```text
inputs   chat_id, cursor (exclusive upper bound), max_bytes (default 512 KiB, clamped to 64 KiB..4 MiB)
1  turn ranges below the cursor, newest first; a range is [a turn's first_event_seq, the next newer turn's
   first_event_seq or the cursor), and events outside turns belong to the range they fall in:
     SELECT id, first_event_seq FROM turns WHERE chat_id = ?1 AND first_event_seq < ?2 ORDER BY first_event_seq DESC LIMIT 64;
     SELECT COALESCE(SUM(length(payload)), 0) FROM events WHERE chat_id = ?1 AND seq >= ?2 AND seq < ?3;
2  whole ranges are added while the total stays within max_bytes
3  when not even the newest range fits, the page is its newest part: its rows newest first while the total stays within
   max_bytes, at least one row (SELECT seq, length(payload) ... ORDER BY seq DESC); lo = the oldest added row's seq;
   cut_turn_id = that turn
4  otherwise lo = the oldest added range's start, or 0 when that range is the chat's oldest turn or no turn lies below
   the cursor (events before the first turn come along)
5  the page: SELECT seq, turn_id, payload, created_at FROM events WHERE chat_id = ?1 AND seq >= lo AND seq < cursor ORDER BY seq;
   has_more = an event below lo exists; before_seq = lo
```

A page holds at most `max_bytes` plus one row, and a row is at most 1 MiB (a compacted text piece; uncompacted rows are
single frame events under 8 KiB), so a 50 MB turn arrives in pages; the next page continues inside a cut turn the
same way. Rows that do not parse are skipped and counted, except `turn_ended` rows, which are read with their
`turn_id` and `stop_reason: Other` so the turn still renders as ended. `read_saved_page` runs the same algorithm over
saved rows and sets `unsaved_from` to the first seq below the cursor that is not saved, when there is one.

#### 6.3.3 Recovery (`recover.rs`)

Global, as the writer's first op, and per chat in `load_chat`:

1. Every `PermissionRequested` and `UiRequest` without a later resolution (found through `events_requests`), inside
   or outside a turn, gets `PermissionResolved { Cancelled, by: System }` or `UiResolved { answer: None, by: System }`:
   inside a running turn, before its `TurnEnded`; otherwise at `MAX(seq) + 1`, outside any turn.
2. Every turn with `status = 'running'` gets `TurnEnded { stop_reason: Interrupted, usage: None, duration_ms: None,
   recovered: true }` at `MAX(seq) + 1`; `status = 'interrupted'`, `stop_reason = 'interrupted'`, `ended_at` (the
   last row's `created_at`, else `started_at`), `last_event_seq`.
3. Every ended turn with `compacted_at IS NULL` is compacted.
4. Every queued row is held (globally: every chat; per chat: that chat).

The partial answer survives; the UI labels a `recovered` turn "Interrupted when auracode closed" with Resume session
(`chat_start`, native resume, nothing sent). Tools left `pending` or `in_progress` render per 7.4; no update is
invented.

### 6.4 The session manager API (`session/mod.rs`, frozen)

```rust
pub struct ManagerDeps {
    pub store: Arc<ChatStore>,
    pub env: Arc<EnvResolver>,
    pub spawner: Arc<dyn ProcessSpawner>,           // aura_process::Spawner, or FakeSpawner in tests
    pub detector: Arc<Detector>,
    pub drivers: Arc<dyn DriverFactory>,
    pub logs: LogFactory,
    pub redactor: Arc<Redactor>,
    pub device_id: String,
    pub app_version: String,
    pub limits: ManagerLimits,
    pub timings: SessionTimings,
    pub rt: Handle,
}
pub struct ManagerLimits { pub max_live_agents: usize, pub max_queue: usize, pub max_prompt_bytes: usize,
                           pub idle_eviction: Duration, pub subscribers_per_chat: usize, pub subscribers_per_owner: usize }
impl ManagerLimits { pub const STANDARD: ManagerLimits; }      // 16, 20, 1 MiB, 10 min, 4, 16
pub struct SessionTimings { pub flush_live: Duration, pub flush_idle: Duration, pub force_cancel_wait: Duration,
                            pub daemon_cancel_wait: Duration, pub stop_cancel_grace: Duration,
                            pub quit_cancel_grace: Duration, pub stop_kill_after: Duration, pub attach_timeout: Duration }
impl SessionTimings { pub const STANDARD: SessionTimings; }    // 16 ms, 100 ms, 200 ms, 5 s, 3 s, 2 s, 15 s, 5 s

pub trait FrameSink: Send + Sync + 'static {
    /// Called on the actor task; must not block. An error removes the subscriber.
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone>;
    fn owner(&self) -> &str;                        // webview label
}
pub trait StatusSink: Send + Sync + 'static {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone>;
    fn owner(&self) -> &str;
}
pub struct SinkGone;

#[derive(Clone)] pub struct SessionManager { /* Arc<Inner> */ }
impl SessionManager {
    pub fn new(deps: ManagerDeps) -> SessionManager;
    /// `refresh: false`: the detector's cache at once, never waiting (`agents_list`); `true`: detects again and waits.
    pub async fn agents(&self, refresh: bool) -> Vec<AgentEntry>;
    pub fn env_status(&self) -> EnvStatus;
    pub fn env_refresh(&self) -> EnvStatus;
    pub async fn chats(&self) -> Result<Vec<ChatSummary>, SessionError>;
    pub async fn create_chat(&self, agent_id: &str, folder: &Path) -> Result<ChatSummary, SessionError>;
    pub async fn attach(&self, chat_id: &str, sink: Arc<dyn FrameSink>) -> Result<ChatAttach, SessionError>;
    pub fn detach(&self, subscription_id: &str);
    pub fn drop_owner(&self, owner: &str);
    pub async fn history(&self, chat_id: &str, before_seq: u32, max_bytes: Option<u32>) -> Result<HistoryPage, SessionError>;
    pub async fn lists(&self, chat_id: &str) -> Result<SessionLists, SessionError>;
    pub async fn send(&self, chat_id: &str, text: String) -> Result<PromptAck, SessionError>;
    pub async fn cancel_queued(&self, chat_id: &str, prompt_id: &str) -> Result<(), SessionError>;
    pub async fn resume_queue(&self, chat_id: &str) -> Result<(), SessionError>;
    pub async fn interrupt(&self, chat_id: &str) -> Result<(), SessionError>;
    pub async fn force_stop(&self, chat_id: &str) -> Result<(), SessionError>;
    pub async fn start(&self, chat_id: &str, fresh: bool) -> Result<(), SessionError>;
    pub async fn stop(&self, chat_id: &str) -> Result<(), SessionError>;
    pub async fn respond_permission(&self, chat_id: &str, request_id: &str, option_id: Option<String>) -> Result<(), SessionError>;
    pub async fn respond_ui(&self, chat_id: &str, request_id: &str, answer: Option<UiAnswer>) -> Result<(), SessionError>;
    pub async fn set_option(&self, chat_id: &str, option_id: &str, value: ConfigValue) -> Result<SessionLists, SessionError>;
    pub async fn set_autonomy(&self, chat_id: &str, autonomy: Autonomy) -> Result<SessionLists, SessionError>;
    pub async fn blob(&self, chat_id: &str, sha256: &str, offset: u64, len: u32) -> Result<BlobText, SessionError>;
    pub fn protocol_log_dir(&self, chat_id: &str) -> PathBuf;
    pub fn status_subscribe(&self, sink: Arc<dyn StatusSink>) -> String;
    pub fn storage_status(&self) -> StorageStatus;
    pub fn has_live_sessions(&self) -> bool;
    pub async fn shutdown_all(&self, deadline: Duration) -> ShutdownSummary;
    /// `kill_tree()` on every recorded process of every chat; synchronous, callable from any thread, never panics.
    pub fn kill_all_now(&self);
}
pub struct ShutdownSummary { pub sessions: usize, pub graceful: usize, pub forced: usize, pub took: Duration }

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("There is no chat with id {0}.")]                                             ChatNotFound(String),
    #[error("auracode doesn't know an agent called \"{0}\".")]                            UnknownAgent(String),
    #[error("{0}")]                                                                       AgentUnavailable(String),
    #[error("{path} can't be used as the chat's folder: {reason}")]                       InvalidFolder { path: String, reason: String },
    #[error("The message is empty.")]                                                     PromptEmpty,
    #[error("The message is too long ({bytes} bytes; the limit is {limit}).")]            PromptTooLarge { bytes: usize, limit: usize },
    #[error("{0} messages are already waiting in this chat. Remove one or wait for one to start.")] QueueFull(usize),
    #[error("This request is no longer waiting for an answer.")]                          RequestNotPending,
    #[error("{0}")]                                                                       OptionInvalid(String),
    #[error("The agent isn't running in this chat. Start it, then try again.")]           SessionNotReady,
    #[error("{0} agents are already running. Stop one, then try again.")]                 TooManyAgents(usize),
    #[error("Too many views are open for this chat.")]                                    TooManySubscriptions,
    #[error("auracode's database is not available: {0}")]                                 StorageUnavailable(String),
    #[error("{0}")]                                                                       BlobMissing(String),
    #[error("auracode is shutting down.")]                                                ShuttingDown,
    #[error("Something went wrong inside auracode: {0}")]                                 Internal(String),
}
```

Bounds: `create_chat` waits at most 10 s for the store; `attach` 5 s; `set_option` and `set_autonomy` 35 s (the
driver's 30 s plus margin); every other call returns once the actor has taken the command. A caller whose actor died
gets `Internal("the session stopped")` (its oneshot drops).

`create_chat`: unknown agent -> `UnknownAgent`; cached availability `Missing` or `Broken` -> `AgentUnavailable(reason)`
(`Checking` and `Ready` proceed; the start waits for detection); the folder must be absolute, valid Unicode (the ACP
`cwd` is a JSON string), existing, a folder and readable (`metadata`, then `read_dir` for one entry, on the blocking
pool), else `InvalidFolder { reason }`, where permission denied reads "auracode isn't allowed to open this folder.
Allow it in System Settings > Privacy & Security > Files and Folders, then try again."; then the store (6.2), then the
actor starts the agent.

### 6.5 The chat actor (`session/actor.rs`)

One actor per chat, created on the first call for that chat and kept for the app run (a stopped chat's actor is a
parked task); it loads `ChatLoad` first. It owns: chat state, the `FrameBatcher` (continuing at `next_seq` via
`starting_at`), the normalizer, subscribers, the queue, the open turn and its `interrupt_requested` flag, the outbox
to the store, the live driver (commands sender, event receiver, `JoinHandle`), the recorded `ProcessControl`s (shared
with a supervisor), and `in_flight`, a `FuturesUnordered` of everything it waits for. Its loop:

```rust
loop {
    tokio::select! {
        biased;
        msg = mailbox.recv() => on_msg(msg),                                     // IPC commands first
        done = in_flight.next(), if !in_flight.is_empty() => on_done(done),
        permit = store.reserve(outbox_head_bytes), if !outbox.is_empty() => permit.send(outbox.pop_front()),
        ev = driver_events.recv(), if driver_attached && outbox_bytes < 4 MiB => match ev {
            Some(ev) => on_driver_event(ev),
            None => on_driver_events_closed(),        // the driver returned or panicked: its JoinHandle joins in_flight
        },
        () = sleep_until(next_flush), if batcher_pending => flush(),
        () = sleep_until(timer), if timer_armed => on_timer(),                   // force-stop wait, idle eviction
    }
}
```

The actor never awaits anything inline. Every wait is a future in `in_flight`, tagged with what it completes: start
preparation (folder check, live slot, detection, adapter install, launch spec; dropped as a whole by interrupt, force
stop, stop and quit, which kills a running helper), each pending attach's page read (any number of attaches can be
pending), each `SetOption` or `SetAutonomy` reply routed back to its IPC caller (a reply dropped because the driver
ended answers `SessionNotReady`), the driver's `JoinHandle` after its
event channel closed (so every event is applied before the exit is handled), and a stop's kill deadline.

The mailbox is unbounded (commands are human-rate), so an IPC call never waits to enqueue. Its messages
(crate-private; every reply is a oneshot):

```rust
enum ChatMsg {
    Attach { sink: Arc<dyn FrameSink>, reply: oneshot::Sender<Result<ChatAttach, SessionError>> },
    Detach { subscription_id: String },
    DropOwner { owner: String },
    History { before_seq: u32, max_bytes: usize, reply: oneshot::Sender<Result<HistoryPage, SessionError>> },
    Lists { reply: oneshot::Sender<SessionLists> },
    Send { text: String, reply: oneshot::Sender<Result<PromptAck, SessionError>> },
    CancelQueued { prompt_id: String, reply: oneshot::Sender<Result<(), SessionError>> },
    ResumeQueue { reply: oneshot::Sender<Result<(), SessionError>> },
    Interrupt { reply: oneshot::Sender<Result<(), SessionError>> },
    ForceStop { reply: oneshot::Sender<Result<(), SessionError>> },
    Start { fresh: bool, reply: oneshot::Sender<Result<(), SessionError>> },
    Stop { reply: oneshot::Sender<Result<(), SessionError>> },
    RespondPermission { request_id: String, option_id: Option<String>, reply: oneshot::Sender<Result<(), SessionError>> },
    RespondUi { request_id: String, answer: Option<UiAnswer>, reply: oneshot::Sender<Result<(), SessionError>> },
    SetOption { option_id: String, value: ConfigValue, reply: oneshot::Sender<Result<SessionLists, SessionError>> },
    SetAutonomy { autonomy: Autonomy, reply: oneshot::Sender<Result<SessionLists, SessionError>> },
    Shutdown { plan: StopPlan, reply: oneshot::Sender<()> },
}

// session/spawner.rs: what drivers spawn through
pub(crate) struct SessionSpawner { base: Arc<dyn ProcessSpawner>, logs: LogFactory, chat_id: String,
                                   controls: Arc<Mutex<Vec<ProcessControl>>> }
impl AgentSpawner for SessionSpawner {}   // prune controls whose process exited, mint spawn_id (UUIDv7),
                                          // logs.open(chat_id, spawn_id), base.spawn(spec, log), record the control
```

A supervisor task awaits each actor's `JoinHandle`. On a panic it kills every recorded process (it holds the same
`controls` list), removes the actor (the next call recreates it, and `load_chat` recovers its running turn; ops still
in its outbox are lost), and publishes `Stopped`; subscribers' channels end and the UI re-attaches.

#### 6.5.1 States (`SessionStatus`)

| State | Meaning | Entered from | Leaves to |
|---|---|---|---|
| `Stopped` | no process (initial; after a stop, idle eviction, crash, force stop or quit) | `Stopping`, a crash | `Starting` |
| `Starting` | folder check, detection, adapter install, env, spawn, initialize, open, configure | `Stopped`, `Failed`, `AuthRequired` | `Ready`, `Failed`, `AuthRequired`, `Stopping` |
| `Ready` | process up, no turn | `Starting`, end of a turn | `Running`, `Stopping` |
| `Running` | exactly one turn in flight | `Ready` | `Ready`, `Cancelling`, `Stopping`, `Stopped` (crash) |
| `Cancelling` | `session/cancel` sent, waiting for the reply | `Running` | `Ready`, `Stopping`, `Stopped` (crash) |
| `Stopping` | graceful or forced stop in progress, including the stop after a prompt answered -32000 | `Ready`, `Running`, `Cancelling`, `Starting` | `Stopped`, `AuthRequired` |
| `Failed` | could not start; `last_error` set; no process | `Starting` | `Starting` (`chat_start`, `chat_send`, `chat_resume_queue`) |
| `AuthRequired` | the agent needs its own sign-in; `auth` set; never a live process | `Starting` (auth at open), `Stopping` (after a prompt answered -32000) | `Starting` |

Every transition marks `SessionInfo` dirty (one `SessionUpdated` with the next flush) and publishes a
`ChatStatusUpdate`. Status details: "Reading your shell environment", "Checking <agent>", "Downloading the <agent>
adapter (<package> <version>). This happens once and can take a few minutes.", "Starting <agent>", "Resuming the
earlier session", "Applying your settings", "Stopping the turn; press Esc again to force stop", "Stopping Hermes'
turn in its gateway", "Stopping".

#### 6.5.2 Start

```text
start(fresh)   from create_chat, send while Stopped/Failed/AuthRequired, chat_start, chat_resume_queue
  invariant: no live driver (Failed and AuthRequired never keep one); a live one is stopped first (STANDARD)
  preparation, one in_flight future, cancellable as a whole:
    folder (blocking pool): missing or not a folder -> Failed { FolderMissing }; permission denied -> Failed {
      FolderNotAllowed, "auracode isn't allowed to open <folder>. Allow it in System Settings > Privacy & Security >
      Files and Folders, then press Retry." }
    live-agent slot (6.5.6), else Failed { TooManyAgents message }
    detector.detect(spec): shares the in-flight env resolution and probe; Missing or Broken -> Failed { AgentUnavailable }
    detector.prepare_launch(spec, progress): progress becomes the status detail; InstallFailed -> Failed { SpawnFailed }
    resume = if fresh { None } else { the latest agent session's native id }; uniqueness for it (6.5.6)
  spawn the driver task with a SessionSpawner and SessionSetup { folder, agent_folder (the folder as given), add_dirs: [],
    resume, autonomy and desired from the latest agent session row (or the actor's values), ... }
at Ready
  uniqueness for the returned native id (6.5.6); a conflict stops the driver (STANDARD) and fails the start
  Resumed or Loaded with the same native id -> UpdateAgentSession on that row; otherwise InsertAgentSession (new UUIDv7,
    carrying config_json and autonomy from the previous row); New after Start fresh -> Notice { info, "Started a new
    session. The agent doesn't remember the conversation above." }
  ReplaySummary (Hermes): when its last_reply_sha256 differs from the stored hash of the newest reply in this native
    session (ChatLoad.replay_check, kept current as turns end), or it counts more user turns than auracode recorded
    there, push outside any turn: Notice { info, "Hermes continued this session while auracode was away (in its
    gateway or another Hermes app). Its latest reply is below; earlier steps aren't shown here." }, then MessageDelta
    and MessageCompleted with id "<spawn_id>.r1" carrying last_reply
  dispatch() the head row unless it is held
```

Reply hashes are sha256 over the concatenated agent message text of a turn with all whitespace removed, computed
incrementally by the actor as deltas arrive, so chunking and spacing differences in Hermes' replay do not count as
changes.

#### 6.5.3 Turns and the queue

```text
send(text)
  text.trim() empty -> PromptEmpty (the text itself is sent exactly as typed); > 1 MiB -> PromptTooLarge; seq near the
  limit -> Internal("This chat is too long to continue; start a new chat."); prompt_id = new_id()
  Ready with nothing queued -> dispatch(this prompt) now; PromptAck { queued: false }
  otherwise: 20 rows queued -> QueueFull; QueuePut { held: false }; PromptLifecycle { Queued, preview: first 256 chars };
    PromptAck { queued: true }; behind a held row nothing more happens (it waits for the person); otherwise Ready ->
    dispatch(); Stopped, Failed or AuthRequired -> start(false) (AuthRequired never keeps a process,
    so this cannot start a second one); Stopping -> started when Stopped is reached (6.5.5), unless auracode is quitting
dispatch(prompt)                      Ready only; the given prompt, or the head row unless it is held
  ordinal += 1; turn_id = new_id(); interrupt_requested = false
  push, tagged turn_id: TurnStarted { turn_id, prompt_id }, UserMessageDelta { "<turn_id>.u", text },
    PromptLifecycle { prompt_id, Started }
  untitled chat -> SetTitle(first line, at most 80 chars, FirstLine); flush now (BeginTurn carries these rows and
    deletes the queued row)
  DriverCommand::Prompt { turn: TurnRef { turn_id }, parts: [Text { text }] }; state Running; start time recorded
interrupt (a turn is open), force_stop or stop
  QueueHoldAll: every row queued until now is held; rows typed afterwards are not, and wait behind them;
  interrupt_requested = true when a
  turn is open
DriverEvent::TurnEnded { stop_reason, usage, error }
  normalizer closes open messages and thoughts; open cards and questions tagged with this turn -> resolved { Cancelled,
    by: Turn }
  push Error { kind, message, detail } if error (message and detail redacted, 5.1); TurnEnded { turn_id, stop_reason,
    usage, duration_ms, recovered: false };
    PromptLifecycle { Completed | Cancelled (Cancelled, Interrupted) | Failed (Error) }; flush now
  error kind AuthRequired -> the failed prompt goes back into the queue, held (same prompt_id; its original created_at
    keeps its place); QueueHoldAll; Shutdown { cancel_grace: 0, process: STANDARD }; state Stopping, then AuthRequired
    with `auth` set
  else: stop_reason Error with no user stop -> QueueHoldAll; state Ready; dispatch() the head row unless it is held
resume_queue()   QueueReleaseAll; Ready -> dispatch(); Stopped, Failed or AuthRequired -> start(false)
cancel_queued()  QueueRemove; PromptLifecycle { Cancelled }; unknown id -> RequestNotPending; removing a held head
                 lets the next row go as a send would (dispatch, or start a stopped agent)
```

So an `end_turn` reply racing the user's Stop still holds the rows queued before the Stop (decision 11), and a prompt
typed after a stop, a crash or a cancel waits behind the held rows until the person sends them (Send it now, Send all)
or discards them. After a restart every queued row comes back held: auracode never sends a message on its own after a
restart. The auth card's Retry calls `chat_resume_queue`, which releases the held prompts, restarts the agent with
native resume, and sends them in order, the one that met the sign-in error included.

#### 6.5.4 Interrupt and force stop

| Call | State | Action |
|---|---|---|
| `interrupt` | `Running` | `interrupt_requested`; `QueueHoldAll`; state `Cancelling`; `DriverCommand::Interrupt`; open cards and questions of the turn -> resolved `{ Cancelled, by: Turn }` |
| `interrupt` | `Cancelling` | same as `force_stop` (the second Esc) |
| `interrupt` | `Starting` | as `force_stop`: the preparation is dropped (a running install or probe is killed) or the driver force-stopped; the prompt that triggered the start stays queued, held |
| `interrupt` | other | nothing |
| `force_stop` | `Running`, `Cancelling`, `Ready`, `Starting`, `Stopping` | 1. `QueueHoldAll` (rows queued until now, the one that triggered a start included); if a turn is open: `interrupt_requested` and `DriverCommand::Interrupt` while the driver lives; 2. wait for that turn's `TurnEnded` at most `force_cancel_wait` (200 ms) or, for agents whose turns survive the process, `daemon_cancel_wait` (5 s: Hermes' cancel makes up to three gateway calls, and killing its viewer stops nothing), applying events meanwhile; 3. drop the event receiver (no late event is ever applied) and run `control.shutdown(FORCE)` on every recorded process; drop any start preparation; the driver's `JoinHandle` joins `in_flight` (aborted if it has not returned 5 s after the kill), and its exit, whatever it is, counts as this force stop (no crash card); 4. if the turn is still open: open cards and questions resolved `Turn`, `TurnEnded { Interrupted }`, `PromptLifecycle { Cancelled }`, `Notice { warning, "Force stopped: auracode ended the agent's processes." }` plus, when turns survive the process, "Hermes may still finish this turn in its gateway; auracode asked it to cancel. Whatever it finishes shows when this chat resumes."; 5. state `Stopped` |

`stop()` (Stop button, idle eviction): `QueueHoldAll`, and `interrupt_requested` when a turn is open, then
`DriverCommand::Shutdown { StopPlan { cancel_grace: 3 s (5 s when turns survive the process), process: STANDARD } }`;
if the driver has not exited 15 s later, `force_stop`. `shutdown_all(deadline)`: no new starts, every preparation
dropped, and every live actor gets `Shutdown { cancel_grace: 2 s (5 s when turns survive the process), process: QUIT }`
at once; each actor then ends its open turn `Interrupted`, flushes its batcher, hands its whole outbox to the store
(waiting for permits until the deadline), and only then replies. At the deadline (8 s), `kill_tree()` on whatever is
left; anything not handed over is lost and logged (counts), and recovery ends the turns at the next start.

#### 6.5.5 Driver exit

- `Closed`: state `Stopped` (or `AuthRequired` when the stop was for auth); a turn still open (the cancel grace ran
  out) ends `TurnEnded { Interrupted }` with its cards and questions resolved `Turn`; untagged open cards and questions
  are resolved `System` (the process is gone). Non-held rows (typed while `Stopping`) start the agent again, unless
  auracode is quitting.
- `Crashed`, `Panicked`, or the task was aborted: first `control.shutdown(FORCE)` on every recorded process, then the
  list is cleared (the driver may have died with its agent still running); push `Error { AgentExited, "The agent
  stopped unexpectedly (exit code N).", detail: the stderr tail through redact_text, at most 2 KiB }`; an open turn ends
  `TurnEnded { Error }` with cards and questions resolved `System`; untagged ones are resolved `System`;
  `QueueHoldAll`; state `Stopped`; `last_error` set.
- `StartFailed(e)`: the recorded processes are force-stopped as above; state `Failed` (or `AuthRequired` with `auth`
  from the `AuthRequired` event); `last_error`; an `Error` event stored outside any turn, from `e` with its message
  and detail through `redact_text` (the driver already appended the dialect's start hint when one matches). For an
  adapter whose stderr tail shows `ERR_MODULE_NOT_FOUND`,
  `MODULE_NOT_FOUND` or `Cannot find module`: `detector.forget_install(spec)` and the message "The <agent> adapter's
  files are damaged. Press Retry to download it again."

#### 6.5.6 Concurrency and limits

Several chats run turns at once, one actor, one process and one native session each. At most 16 live agents. A 17th
start makes room: the agent idle longest by the idle-eviction test below (each actor reports to `LiveAgents` whenever
that changes, with its last activity) is asked to `stop()`, and the start waits (`Starting`, "Waiting for an idle chat
to stop and make room", 60 s at most) for that agent's slot, which passes to it once the agent's processes are gone. An
agent asked after it stopped idling declines and the next idle one is asked; when none is, the stop is owed and the
next agent to turn idle pays it. Only when no agent idles does the start fail with `TooManyAgents`, saying what the
running agents do ("16 agents are running and none is idle: 13 working, 2 waiting for your answer and 1 open in a
terminal. Stop one, then try again."). A stopped agent keeps its held queue and resumes its native session on its next
message. A start registers its chat's recorded folder with its slot at once, so a start still preparing blocks folder
mutations of that folder only; the folders the preparation resolves replace it, checked again before anything spawns.
`SessionManager::stop_idle(scope)` and `stop_all(scope, deadline)` stop several chats (idle ones only, or all) and
answer once their slots are really free, without closing admission. Idle eviction: `Ready`, no subscribers (remote
views included), no open card or question, no non-held queued row, no background work, not open in a terminal, and 10
minutes since the last driver event or command -> `stop()`; the next use restarts it with native resume. Background
work is what the driver reports: `RunState { idle: false }`, or `BackgroundWork` above zero. The second exists because
Claude Code reports itself idle, ready for a prompt, while a command it left in the background still runs, and lists
such tasks in a frame of their own (`background_tasks_changed`). Until 22 November 2026 the session read only the
first, so ten minutes after the person left such a chat its agent was stopped, and the command and whatever Claude
meant to do when it finished ended with it (reproduced with a real chat; fixed in #174). An agent kept from idling only
by `BackgroundWork` is not stopped to make room for another start or for a change to its folder either; it stops after
a day with no driver event and no viewer (`ManagerLimits::background_eviction`), so a server someone forgot does not
hold an agent for good. Two live processes
never share one `(agent_id, native_session_id)` across chats (the Claude spike: two processes on one session id
interleave one transcript): checked for the resume id before spawning and again for the id the agent returns at
`Ready`, for every driver, the mock included; the second fails with `Internal("this session is already open in
another chat")`. Two chats in one folder are allowed (the busy-folder warning is M2b).

Terminals count in pools of their own, so one kind never refuses another: project shells (the docked panel, Run)
several per window and 8 in the app; terminal chats several per window and 8 in the app; worktree setup commands
several per window and 4 in the app, so two projects' new worktrees set up at once; sign-in and installation one per
window and 4 in the app. A project shell opens in one step (`project_terminal_open`: the chat's folder and the shell
are checked and the shell prepared together), so a panel tab carries nothing that expires and one first shown long
after it was made still opens; the panel counts a tab toward its limit only once its shell was asked to start. A
terminal chat in a new worktree waits for the worktree's setup to succeed, then opens; a failed setup answers it with
the setup's reason.

#### 6.5.7 Flush and persistence

Flush every 16 ms while the chat has a subscriber, every 100 ms otherwise, and at once after `TurnStarted`,
`TurnEnded`, `PermissionRequested`, `UiRequest`, `Error` and status changes. Every event enters the batcher with its
tag (the driver's tag, or the turn the actor pushed it for). A flush: push `SessionUpdated` if dirty;
`frames = batcher.flush_tagged()`; send each frame to every subscriber (a failed send removes it); build `EventRow`s
for events with `is_stored(tag.is_some())`, each row's `turn_id` being its tag; queue in seq order: `AppendEvents`,
`EndTurn` then `CompactTurn` after a turn's `TurnEnded`, and `BeginTurn` carrying the rows from a `TurnStarted` to the
end of the flush. Blob ops from the normalizer are queued when the event enters the batcher, so they precede its row.
After each `TurnEnded` the actor runs `record_escaped()` on its recorded controls (in `in_flight`).

### 6.6 The permission policy (`session/policy.rs`)

```rust
pub enum Decision { AutoAllow { option_id: String }, Ask }
pub fn decide(state: &AutonomyState, req: &PermissionRequest) -> Decision;
```

1. `req.kind` is `None`, `SwitchMode` or `Other` -> `Ask` (leaving plan mode is the user's call; agents ask questions
   with kind `other`, Hermes' clarify and guarded-model prompts among them).
2. `AutoAnswer::Never` -> `Ask`; `All` -> step 3 for `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`,
   `fetch`; `Kinds { kinds }` -> step 3 when `req.kind` is in `kinds`, else `Ask`.
3. The first option of kind `AllowOnce` -> `AutoAllow`; none -> `Ask` (`AllowAlways` and `Other` are never
   auto-selected).

`AutoAllow`: push `PermissionRequested` then `PermissionResolved { Selected(option), by: Auto }`, send
`RespondPermission`. `Ask`: push `PermissionRequested` (the card), flush now, mark `needs_attention`.
`respond_permission(id, option_id)`: unknown or resolved id -> `RequestNotPending`; an option the request does not
offer -> `OptionInvalid`; else `RespondPermission`, `PermissionResolved { by: User }` (`None` = cancelled).
`PermissionWithdrawn` -> `PermissionResolved { Cancelled, by: Agent }`; a withdrawal for a request the user already
answered (both crossed in flight) adds no second resolution but an info notice: `The agent withdrew "<title>" before
your answer reached it.`

`UiRequest` is never auto-answered: push it (the question card), flush now, mark `needs_attention`.
`respond_ui(id, answer)`: unknown or resolved id -> `RequestNotPending`; an answer that does not fit the prompt (wrong
variant, index out of range, any answer to `Notify`) -> `OptionInvalid`; else `RespondUi` and
`UiResolved { answer, by: User }` (`None` = dismissed). `UiWithdrawn` -> `UiResolved { None, by: Agent }`, with the
same crossed-in-flight notice. `Notify` prompts are never open (nothing answers them).

When autonomy changes (or a hand-picked mode changes `auto_answer`, 5.5.2), the new state applies to the next decision
at once, and every open card the new state would auto-answer is answered now.

### 6.7 The normalizer (`session/normalize.rs`)

Applies to every driver's events before they enter the batcher. After normalizing, every non-delta event serializes
within `EVENT_BUDGET` (7 KiB of JSON), except a `PermissionRequested` or `UiRequest` whose option ids alone exceed it
(ids are echoed exactly, so such an event goes out alone in an oversized frame, logged). Text deltas have no cap; the
`FrameBatcher` splits them on grapheme boundaries.

Fixed caps first, without blobs: titles 256 chars, tool names 128, option names and question choices 64, each
location path 512 bytes, notice and error messages and error detail 2 KiB each, `SessionInfo` strings (detail 256
chars, title 200, labels and names 128, version 64).

Then, for a `ToolCall`, `ToolUpdate` or `PermissionRequested` still over the budget, in this order until it fits:

1. `raw_input` becomes `{"_auracode_truncated": true, "bytes": n, "preview": "<first 512 bytes of its JSON>"}`.
2. `locations` keep the longest prefix within 1 KiB of JSON.
3. `content` shares what is left, in order: each `Text` keeps its head and each `Terminal` its tail (the end explains
   failures), cut on a character boundary and never below 256 bytes, with a blob of the full entry text (`text/plain`)
   in its `Truncation`; a `Diff` that does not fit inline keeps empty texts and a blob holding
   `{"old_text":..,"new_text":..}`; entries that do not fit even at 256 bytes are dropped.
4. Option names shrink to 24 chars.

When anything besides an entry's text was cut (raw input, locations, a dropped entry, option names), the complete
original event (before any cut) goes to one blob (`application/json`) named by the event's `truncated`; when only
entry texts were cut, their own blobs hold everything, so a large output is stored once. `PlanUpdated`: entries cut
to 500 chars, then trailing entries become one "+ n more" entry. `UiRequest`: message 2 KiB, choices shrink to 24
chars.

It also tracks open messages, thoughts, cards and questions with their tags, so `TurnEnded` and driver exits close them
even when a driver did not:

```rust
pub(crate) struct Normalizer { /* caps, open messages and thoughts with start times, open cards and questions */ }
impl Normalizer {
    /// Normalizes one driver event; returns the events to push and the `PutBlob` ops to queue before them.
    pub(crate) fn apply(&mut self, event: AuraEvent, turn: Option<&str>, now: Millis) -> (Vec<AuraEvent>, Vec<Op>);
    /// `MessageCompleted`/`ThoughtCompleted` for what is still open in `turn`, and resolutions `by` for its open cards
    /// and questions; called before every `TurnEnded` the actor pushes.
    pub(crate) fn close_turn(&mut self, turn: &str, by: ResolvedBy, now: Millis) -> Vec<AuraEvent>;
    /// The same for untagged cards and questions, when the driver ends.
    pub(crate) fn close_untagged(&mut self, by: ResolvedBy, now: Millis) -> Vec<AuraEvent>;
}
```

### 6.8 Attach: the join protocol (`session/attach` part of the actor)

1. The UI creates a `Channel`, buffers its frames, calls `chat_attach`.
2. The actor checks the limits (4 per chat, 16 per webview; an existing subscription of the same webview for this
   chat is replaced, one view per chat per webview in M1a), flushes, fixes `S = batcher.next_seq()`, builds the
   snapshot, and starts the page read: normally `Op::ReadPage { before_seq: S, max_bytes: 512 KiB }` in its outbox,
   behind every op of every event below `S`; while the store is paused, `store.read_saved_page(chat_id, S, 512 KiB)`.
   It registers the sink: every frame it will carry has `seq_start >= S`. The read waits in `in_flight`; the actor
   keeps processing driver events, and anything they queue (a turn ending and its `CompactTurn` included) runs after
   the read.
3. The page arrives: `ChatAttach { subscription_id, next_seq: S, snapshot, history }`. No reply within 5 s ->
   `StorageUnavailable`, the subscriber is removed.
4. The UI applies the page, then buffered frames with seq at or above `S`, then live frames. A frame whose
   `seq_start` is past the expected seq, or a channel end, makes it detach, drop its state and attach again. A page
   with `unsaved_from` shows "Recent output isn't saved yet; it appears here when saving resumes." at the gap, and
   the view re-attaches when `storage_status` returns to `Ok`.

`chat_history(before_seq)` serves older pages the same way: `Op::ReadPage` in the actor's outbox, or
`read_saved_page` while paused (6.3.2). Pages are command responses, not channel frames: the 8 KiB figure is Tauri's
threshold for evaluating channel payloads inline (`MAX_JSON_DIRECT_EXECUTE_THRESHOLD = 8192`, tauri 2.12.5
`ipc/channel.rs`), and responses do not take that path.

---

## 7. The Tauri IPC surface and the bare UI

### 7.1 Commands

Thin wrappers over `SessionManager` (logic stays testable without a running app). The M0 commands stay: `app_info`
(the schema version now comes from boot), `start_demo_stream`, `cancel_demo_stream`, `window_ready`,
`reveal_data_dir`, `acknowledge_recovery` (now async, through `ChatStore`), `smoke_report`. Webview-scoped commands
take the injected `webview: tauri::Webview`, whose label is the subscription owner.

| Command | Arguments | Returns | Bound | Error codes |
|---|---|---|---|---|
| `agents_list` | | `Vec<AgentEntry>` | immediate (cached; detection runs in the background) | |
| `agents_refresh` | | `Vec<AgentEntry>` | 15 s (env refresh, probes in parallel) | |
| `env_status` | | `EnvStatus` | immediate | |
| `env_refresh` | | `EnvStatus` | immediate (starts an attempt) | |
| `pick_folder` | | `Option<String>` | until the dialog closes | `dialog_failed` |
| `chats_list` | | `Vec<ChatSummary>` | 5 s | `storage_unavailable` |
| `chat_create` | `agent_id`, `folder` | `ChatSummary` | 10 s | `unknown_agent`, `agent_unavailable`, `invalid_folder`, `storage_unavailable`, `shutting_down` |
| `chat_attach` | `chat_id`, `channel: Channel<EventBatch>` | `ChatAttach` | 5 s | `chat_not_found`, `too_many_subscriptions`, `storage_unavailable` |
| `chat_detach` | `subscription_id` | | immediate | |
| `chat_history` | `chat_id`, `before_seq: u32`, `max_bytes: Option<u32>` | `HistoryPage` | 5 s | `chat_not_found`, `storage_unavailable` |
| `chat_lists` | `chat_id` | `SessionLists` | immediate | `chat_not_found` |
| `chat_send` | `chat_id`, `text` | `PromptAck` | until the actor takes it | `chat_not_found`, `prompt_empty`, `prompt_too_large`, `queue_full`, `storage_unavailable`, `shutting_down`, `internal` |
| `chat_cancel_queued` | `chat_id`, `prompt_id` | | same | `chat_not_found`, `request_not_pending` |
| `chat_resume_queue` | `chat_id` | | same | `chat_not_found` |
| `chat_interrupt` | `chat_id` | | same | `chat_not_found` |
| `chat_force_stop` | `chat_id` | | same | `chat_not_found` |
| `chat_start` | `chat_id`, `fresh: bool` | | same (failures arrive as cards) | `chat_not_found`, `too_many_agents`, `internal` |
| `chat_stop` | `chat_id` | | same | `chat_not_found` |
| `chat_respond_permission` | `chat_id`, `request_id`, `option_id: Option<String>` (none = cancel) | | same | `chat_not_found`, `request_not_pending`, `option_invalid` |
| `chat_respond_ui` | `chat_id`, `request_id`, `answer: Option<UiAnswer>` (none = dismiss) | | same | `chat_not_found`, `request_not_pending`, `option_invalid` |
| `chat_set_option` | `chat_id`, `option_id`, `value: ConfigValue` | `SessionLists` | 35 s | `chat_not_found`, `session_not_ready`, `option_invalid` |
| `chat_set_autonomy` | `chat_id`, `autonomy` | `SessionLists` | 35 s | `chat_not_found` |
| `chat_blob` | `chat_id`, `sha256` (64 lowercase hex only), `offset: u64`, `len: u32` (at most 256 KiB) | `BlobText` | 5 s | `chat_not_found`, `blob_missing`, `invalid_argument`, `storage_unavailable` |
| `chat_reveal_log` | `chat_id` | | the file-manager call | `chat_not_found`, `reveal_failed` |
| `status_subscribe` | `channel: Channel<ChatStatusUpdate>` | `String` | immediate | `too_many_subscriptions` |
| `storage_status` | | `StorageStatus` | immediate | |

`chat_set_autonomy` works while stopped (stored, applied at the next start; the note reads "Applies when the agent
starts."). `chat_blob` ends each page on a character boundary at or before `offset + len` and returns `next_offset`
for the next call, so "Load full output" never splits a character. `chat_reveal_log` takes no path: the shell reveals
the chat's newest protocol log, chosen in Rust, like `reveal_data_dir`. New `AppErrorCode` values, each
`SessionError` variant mapping to exactly one (an exhaustive match plus a test), the message being the variant's
sentence: `chat_not_found`, `unknown_agent`, `agent_unavailable`, `invalid_folder`, `prompt_empty`,
`prompt_too_large`, `queue_full`, `request_not_pending`, `option_invalid`, `session_not_ready`, `too_many_agents`,
`too_many_subscriptions`, `storage_unavailable`, `blob_missing`, `shutting_down`, `internal`, plus `dialog_failed`
(shell). Existing codes stay.

### 7.2 Channels and lifecycle glue (`sessions.rs`)

- **Chat channel** (`Channel<EventBatch>`, one per open chat view): `ChannelSink` implements `FrameSink`; a failed
  `send` removes it; detaching drops the last Rust clone, so Tauri ends the JS channel.
- **Status channel** (`Channel<ChatStatusUpdate>`, one per webview; a second `status_subscribe` from the same webview
  replaces the first).
- **Startup.** `boot()` takes the profile owner lock between `open_active()` and `Store::open` (6.1); `Booted` gains
  `owner: Option<ProfileOwner>` and the profile directory. Then, with `rt = tauri::async_runtime::handle().inner().clone()`
  passed to every constructor that spawns (Tauri's `setup` hook runs outside any Tokio context): `ChatStore::start`
  (the `Store` and the owner move into the writer thread; `AppState` keeps `Option<Arc<ChatStore>>` and
  `Option<SessionManager>` instead of `Mutex<Option<Store>>`), `LeftoverRegistry::open(<data root>/run, rt)`,
  `sweep_stale` and log retention on background tasks, one `Arc<Redactor>` (`Redactor::standard()`) shared by the
  next three, `EnvResolver::start` (with the redactor in `EnvResolverConfig::redactor`, 3.6), `Detector::new(...,
  <data root>/adapters, rt)` (detection starts once the env resolves), `LogFactory::new(<profile>/logs/protocol,
  redactor, rt)`, `SessionManager::new` (`ManagerDeps.redactor`).
- **Page load:** `Builder::on_page_load`, on `PageLoadEvent::Started` -> `manager.drop_owner(webview.label())`: a
  reloaded page has new callback ids, so frames for the old ones would be lost.
- **Quit**, on every first `RunEvent::ExitRequested` (any code, any cause):
  1. `api.prevent_exit()`; hide the main window.
  2. When storage is paused with unsaved data: ask through tauri-plugin-dialog (Rust API, callback into a oneshot):
     "auracode can't save its latest messages because the disk is full. If you quit now, they are lost." with Quit
     anyway and Keep auracode open. Keep shows the window again and ends the quit.
  3. On a Tauri async task: `shutdown_all(8 s)` (a no-op when nothing runs), `ChatStore::shutdown(5 s)`,
     `LeftoverRegistry::close()`, set the quitting flag, `app.exit(code)`; the next `ExitRequested` sees the flag
     and passes.
  4. `RunEvent::Exit` is the synchronous last resort, since Tauri runs run-event callbacks on the main thread outside
     any Tokio context: `manager.kill_all_now()` (`kill_tree()` on every recorded control; Unix `killpg` inline, and
     the Windows jobs close with the process), `ChatStore::shutdown_blocking(1 s)` when step 3 did not finish, then
     M0's integrity close and demo cancel.
  5. Unix signals: a task on `rt` listens for SIGTERM, SIGINT and SIGHUP and calls `app.exit(0)` (the same flow; a
     Ctrl+C under `pnpm dev` or a logout ends cleanly); a second signal during the flow runs `kill_all_now()`,
     `shutdown_blocking(1 s)` and exits with 128 plus the signal number.
  M1a has no tray, so closing the window quits; the "quit with live turns asks" dialog is M2b.
- **Folder picker:** `lib.rs` registers `tauri_plugin_dialog::init()`; the command uses its Rust `pick_folder` with a
  callback into a oneshot (never the blocking variant on the main thread). The webview gets no dialog permission;
  `capabilities/default.json` is unchanged.

### 7.3 Screens (plain HTML controls, unstyled, complete)

| Screen or part | Contents | Behaviour |
|---|---|---|
| Chat list (left) | New chat; each chat: title (or "New chat"), agent, folder, status word, "folder missing" or "folder not allowed", attention dot | `chats_list` plus the status channel |
| New chat | agent select (unavailable ones disabled with their reason, for example "Node.js 22 or newer is required (found 20.11.0)."), the download note when `needs_download`, Refresh agents, Choose folder (shows the path), Create | `agents_list`, `agents_refresh`, `pick_folder`, `chat_create`; errors inline |
| Header | agent and version (the user binary that runs), folder (with its state), status with detail, Start or Resume session, Start fresh, Stop session, Show log, context used and session cost when known | snapshot, `SessionUpdated`, `chat_start`, `chat_stop`, `chat_reveal_log` |
| Pickers | autonomy (Full auto, Auto-edit, Ask) with the note, the native mode, and "Full auto: no checkpoints until M2" while Full auto is chosen; one select or checkbox per option (Model, Effort, Mode, ModelConfig, Other), pending values marked; disabled unless `Ready` or `Running` (autonomy always enabled) | `chat_lists` on attach and on a new `revision` (debounced 200 ms); `chat_set_option`, `chat_set_autonomy` |
| Transcript | "Load earlier" at the top, and "Earlier part of this turn not loaded" when a page cut a turn; per turn: user text; agent messages (M0's sanitized `Markdown`); thoughts in `<details>` "Thought for 3.2 s"; tool blocks (title, kind, status, locations; text in `<pre>`; diffs with M0's `lib/diff.ts`; "Load full output" when an entry is truncated, "Show full details" when the event is); the plan as a checklist; permission cards (title, raw input summary, content, one button per option, Dismiss when no options; resolved cards say "Auto-approved", "Withdrawn by the agent", "Closed when the turn ended" or the choice); question cards (the prompt; Yes and No, one button per choice, or a text field and Submit; Dismiss; resolved ones show the answer); notices; error cards (message, redacted detail in `<pre>`, the actions below, Show log); the unsaved-rows marker; footer (stop reason label, tokens with scope, duration, cost) | history page plus live frames |
| Error card actions | start failures (`AgentUnavailable`, `FolderMissing`, `FolderNotAllowed`, `SpawnFailed`, `StartTimeout`, `ProtocolMismatch`, `AgentExited`): Retry (`chat_start`); a resume that failed: also Start fresh (`chat_start` with `fresh`); turn errors while the session stays `Ready` (`AgentError`, `ProtocolError`): no Retry (the user resends); `Storage` and `Internal`: none | |
| Auth card | the agent's message, each command with its variables and Copy, Retry | `snapshot.auth`; Retry = `chat_resume_queue` when held prompts exist, else `chat_start` |
| Queue | queued prompts (held ones marked) with Remove; "Send queued" when any is held | `chat_cancel_queued`, `chat_resume_queue` |
| Composer | textarea (Enter sends, Shift+Enter newline), Send, Stop (becomes Force stop while `Cancelling`); `/` opens the slash list filtered by the text after `/`, choosing inserts `/name ` | `chat_send`, `chat_interrupt`, `chat_force_stop`, `chat_lists` |
| Banners | storage paused (the 6.1 text) or failed; env fallback with Retry; the M0 database repair notice | `storage_status`, `env_status`, `env_refresh` |
| Still working | "Still working (no output for 1 minute)", the minutes counting up, with Stop, after 60 s without a frame during a turn | UI timer; nothing is ever killed automatically |
| Diagnostics | M0 info card and stress view | existing |

Stop reason labels: `max_tokens` "Stopped at the output limit", `max_turn_requests` "Stopped at the step limit",
`refusal` "The agent declined", `cancelled` "Stopped", `interrupted` "Interrupted" ("Interrupted when auracode closed"
when `recovered`, with Resume session), `error` "Failed", `other` "Ended".

### 7.4 UI state

- `lib/chat/transport.ts`: an interface over the IPC wrappers, so stores run in Vitest with a fake.
- `lib/chat/transcript.ts`: a pure reducer `(model, seq, turnId, event) -> model` over seq-ordered events (history
  items and live frames alike). Turns are grouped by seq position, from `TurnStarted` to its `TurnEnded`, for history
  and live frames the same way, so an untagged late event that landed inside a later turn's span renders there in
  both. A page with `cut_turn_id` starts inside that turn. Items keep their order of first appearance; deltas append by
  id; `ToolUpdate.content` `null` keeps and `[]` clears; open cards and questions are requests without a resolution.
  A tool still `pending` or `in_progress` in an ended turn shows as stopped when that turn ended `cancelled`,
  `interrupted` or `error`, or when the session has no process; otherwise it keeps its reported status (a background
  task may still update it). The `switch` is exhaustive. Golden transcripts produced from the replay goldens check
  that compacted history and live streams render the same.
- `lib/chat/chatStore.ts`: one external store per open chat (`useSyncExternalStore`, like M0's `StreamStore`); phases
  `attaching`, `live`, `failed`; the join protocol of 6.8; on a gap or channel end it detaches, then attaches again; a
  failed attach is retried when `storage_status` changes and on Retry; Esc while `Running` -> `chat_interrupt`, while
  `Cancelling` -> `chat_force_stop`; card and question buttons disable on click and a `request_not_pending` error
  removes the card; `revision` change -> `chat_lists`; unmount -> `chat_detach`.
- `lib/chat/appStore.ts`: agents, env status, chat list, storage status, the status channel. Zustand arrives with M2a;
  these stores are plain classes M2a can move into slices unchanged.

---

## 8. Edge cases: behaviour and the test for each

Owners: A (aura-process, aura-agents), B (aura-drivers), C (aura-core), D (desktop), M (manual). Ids from the first
draft keep their numbers; new cases are appended to each table.

### 8.1 Processes and agents

| # | Case | Behaviour | Test (owner) |
|---|---|---|---|
| P1 | agent binary missing | `Missing { reason }`; `chat_create` refuses; an existing chat fails to start with an `AgentUnavailable` card | `detect_missing_binary` (A), `start_fails_when_agent_missing` (C) |
| P2 | `--version` fails or hangs | `Broken { first stderr line }`; tree killed after 10 s | `detect_broken_binary`, `probe_timeout_kills_tree` (A) |
| P3 | Node missing or below 22 | `Missing { "Node.js 22 or newer is required (found X)." }` | `adapter_recipe_checks_node` (A) |
| P4 | user's claude or codex missing, node present; the user's variable empty or broken | `Missing` when no binary; an empty or broken variable is overridden with the detected binary; the bundled CLIs are never installed (`--omit=optional`), so the adapter cannot fall back to them | `adapter_requires_user_binary`, `empty_user_binary_env_is_overridden` (A), free probe 0 (phase 2) |
| P5 | several installs | `PATH` order wins | `find_program_follows_path_order` (A) |
| P6 | first start of an adapter | the install runs as its own step (10 min timeout, download detail); the 60 s start timeout covers only the launch | `install_marks_only_complete_installs` (A), `install_detail` (C) |
| P7 | agent exits before `initialize` | `StartFailed { AgentExited }` with the stderr tail (plus the dialect's hint), redacted by the actor | `agent_exits_before_initialize` (B) |
| P8 | agent never answers `initialize` | `StartTimeout`, process stopped | `initialize_timeout_stops_process` (B) |
| P9 | protocol version other than 1 | `ProtocolMismatch`, process stopped | fixture `protocol-mismatch` (B) |
| P10 | crash mid-turn | processes force-stopped, `Error { AgentExited, tail }`, `TurnEnded { Error }`, cards resolved `System`, `Stopped`, rows held, Retry resumes | fixture `eof-mid-turn` (B), `crash_mid_turn` (C) |
| P11 | stdout closes, process lives | 2 s for it to exit, then the tree is killed; treated as a crash | `stdout_eof_with_live_process_kills_tree` (B) |
| P12 | non-JSON noise on stdout | dropped, logged, never answered | `noise_lines_are_dropped` (A) |
| P13 | malformed JSON line | logged, no reply, connection stays | fixture `garbage` (B) |
| P14 | oversized notification | skipped with a notice; the next line is intact | `framer_oversized_line` (A), fixture `oversized` (B) |
| P15 | oversized response to our request | the request fails with `ProtocolError` | `oversized_response_fails_request` (B) |
| P16 | oversized request from the agent | answered -32603 | `oversized_request_is_answered` (B) |
| P17 | `"id"` after a huge `params` | the sniffer still finds it | `sniffer_finds_late_id` (A) |
| P18 | invalid UTF-8, CRLF, split multibyte characters | lossy decode, `\r` dropped, characters reassembled | `framer_*` (A, ported) |
| P19 | stderr flood | stdout unaffected; logging rate-limited | `stderr_flood_never_blocks_stdout` (A, ported) |
| P20 | slow consumer | the reader stops at 16 lines or 8 MiB, the agent blocks, memory stays bounded | `slow_consumer_blocks_agent_not_memory` (A, B) |
| P21 | grandchild in the agent's group | killed at shutdown | `group_member_is_killed` (A, real processes) |
| P22 | a declared daemon that called `setsid` | survives shutdown and the sweep with its subtree; listed as spared | `declared_daemon_survives` (A, Unix, testbin `spawn-setsid-daemon`) |
| P23 | job-control grandchild (own group, same session) | killed by the descendant walk | `job_control_child_is_killed` (A, Unix) |
| P24 | agent ignores SIGTERM | SIGKILL after the grace | `sigterm_ignored_then_killed` (A) |
| P25 | agent exits on EOF | no signal sent | `clean_eof_sends_no_signal` (A) |
| P26 | kill after exit (macOS zombie `EPERM`) | not an error | `kill_tree_after_exit` (A, ported) |
| P27 | last control dropped | tree killed | `dropping_control_kills_tree` (A, ported) |
| P28 | sweep: recycled pid | start time differs; never signalled | `sweep_skips_recycled_pid` (A) |
| P29 | sweep: live other instance | untouched | `sweep_skips_live_instance` (A) |
| P30 | sweep: dead instance | its group members within the heartbeat window, its recorded escaped processes, and (leader alive) its descendants are killed; file removed; notice | `sweep_kills_dead_instance_leftovers` (A, fake process table) |
| P31 | sweep after a reboot | file deleted, nothing killed | `sweep_after_reboot` (A) |
| P32 | registry write fails | the agent runs; warning | `registry_failure_is_a_warning` (A) |
| P33 | Windows job | every agent in a kill-on-close job without breakaway; Hermes needs its gateway already running, and its start failure carries the hint | `windows_wrappers_per_agent` (A, compile and unit), `hermes_breakaway_hint` (B); behaviour on Windows (M) |
| P34 | Windows: the user binary is an npm `.cmd` shim | `Missing` with the shim reason; a `.exe` elsewhere on `PATH` is used | `user_binary_requires_exe_on_windows` (A, pure on fake PATHs) |
| P35 | program path with spaces or non-ASCII | spawned by absolute path | `spawn_path_with_spaces` (A) |
| P36 | folder deleted or on an unplugged drive | `Failed { FolderMissing }`; list shows "folder missing" | `start_fails_when_folder_missing` (C) |
| P37 | folder is a file, relative, missing or not valid Unicode at create | `invalid_folder` | `create_chat_rejects_bad_folders` (C) |
| P38 | agent updated or removed while running | the running process continues; refresh re-probes | `refresh_keeps_live_sessions` (C) |
| P39 | hostile chat folder (a `.npmrc` redirecting `@agentclientprotocol`, a committed `node_modules/@agentclientprotocol/claude-agent-acp`) | never read: the install and the launch run in the adapter folder, and the launch cwd is never the chat folder | `adapter_never_runs_in_chat_folder` (A, testbin `fake-npm` records its cwd), free probe 1 (phase 2) |
| P40 | download interrupted (Esc, force stop, quit, a killed npm) | no marker; the next start cleans the folder and installs again | `interrupted_install_is_redone` (A, `fake-npm hang`) |
| P41 | two chats start the same adapter at once | one install; both wait for it | `concurrent_installs_share_one_attempt` (A) |
| P42 | adapter files damaged (a module missing at start) | marker removed; card "Press Retry to download it again"; Retry installs | `missing_module_forgets_install` (C, ScriptedDriver) |
| P43 | a PTY host or detached child that called `setsid` and matches no daemon | killed at shutdown and by `kill_tree` | `setsid_child_without_daemon_match_is_killed` (A, Unix, testbin `spawn-pty-child`) |
| P44 | a vendor daemon still parented under the agent in its own group | spared by its declaration, with its subtree | `daemon_subtree_is_spared` (A) |
| P45 | `kill_tree` called with no Tokio runtime (a run-event callback) | the group dies at once | `kill_tree_without_runtime` (A, from a plain `std::thread`) |
| P46 | agent exits with output still buffered while the actor is slow | every line is handled before `Crashed` or `Closed`; Windows exit is the direct child's, even with a lingering descendant | `exit_under_backpressure_loses_nothing` (B, FakeSpawner), `exit_detection_uses_direct_child` (A) |
| P47 | driver task panics or is aborted mid-turn | every recorded process is force-stopped; error card; `Stopped` | `driver_panic_kills_processes` (C) |
| P48 | the OS refuses the folder (macOS privacy, or no read permission) | `Failed { FolderNotAllowed }` with the System Settings hint; list "folder not allowed"; never reported as missing | `folder_permission_denied_is_not_missing` (C, a mode-000 folder), fresh-build check (M) |
| P49 | Windows: chat folder longer than 259 characters as the process cwd | `InvalidCwd` with the reason; adapters are unaffected (they run in their own folder) | `windows_long_cwd_rejected` (A, pure) |
| P50 | user binary below the floor | `Broken { "auracode needs Claude Code 2.1.309 or newer (found X)." }` | `version_floor` (A) |
| P51 | sweep: a terminal job reused the dead agent's group id after the heartbeat window | never signalled; logged as skipped | `sweep_ignores_processes_after_heartbeat_window` (A, fake process table) |

### 8.2 Environment

| # | Case | Behaviour | Test (owner) |
|---|---|---|---|
| E1 | profile prints around the markers | only the marked part is parsed | `env_ignores_profile_output` (A, fake shell) |
| E2 | rc file hangs interactively | killed at 10 s with its session, `-l -c` retry 5 s, then fallback and banner | `env_timeout_retries_then_falls_back` (A) |
| E3 | non-zero exit with both markers | accepted with a warning | `env_nonzero_exit_with_markers` (A) |
| E4 | `$SHELL` unset or not executable | passwd entry, then the platform default | `shell_choice_fallbacks` (A) |
| E5 | fish, tcsh, an unsupported shell | their argv forms; unsupported: platform default plus warning | `shell_argv_per_family` (A) |
| E6 | values with newlines and `=` | `env -0` keeps them | `env_values_with_newlines` (A) |
| E7 | every denylist entry; every kept example | removed or kept as listed; Windows case-insensitive | `scrub_removes_<name>`, `scrub_keeps_<name>` (A, one test each) |
| E8 | `NODE_OPTIONS` with and without a debugger hook | removed only with a hook | `node_options_debugger_hook` (A) |
| E9 | env values never logged | spawn notes and reports carry names only | `no_env_values_in_logs` (A) |
| E10 | Windows registry `Path` with `REG_EXPAND_SZ` | expanded, merged system, user, process | `windows_path_merge` (A, pure on every OS), `windows_registry_read` (A, Windows) |
| E11 | many sessions start at once | one shared resolution | `env_resolution_is_shared` (A) |
| E12 | user sets `CLAUDE_CODE_EXECUTABLE` or `CODEX_PATH` | an executable value is detected, shown and used; an empty or broken one is overridden | `user_launch_env_is_detected_and_forced` (A) |
| E13 | Finder launch with launchd's `PATH` | login-shell env; `~/.local/bin` agents found | `resolves_env_from_fake_login_shell` (A), Finder launch (M) |
| E14 | known dirs (including the asdf and mise shims and the `~/.npmrc` prefix) | appended when they exist and are missing | `known_dirs_appended` (A) |
| E15 | launch-context variables (`TERM_PROGRAM`, `TERM_SESSION_ID`, `npm_config_*`, `PNPM_*`, `TAURI_*`) | removed from the resolver shell's input and from the fallback; what the user's rc exports comes back | `launch_context_stripped_from_resolver` (A, fake shell) |
| E16 | sleep and wake during a turn | no spurious timeout (tokio's clock stops during macOS and Linux sleep); the turn continues | macOS (M); Windows manual pass |

### 8.3 ACP behaviour

| # | Case | Behaviour | Test (owner) |
|---|---|---|---|
| A1 | unknown `sessionUpdate` | ignored, logged once per tag; stream intact | fixture `unknown-update` (B) |
| A2 | update for another `sessionId` | ignored; while `session/new` is pending, buffered and applied once the id is known | `foreign_session_update_ignored`, fixture `new-session-early-update` (B) |
| A3 | `tool_call_update` before `tool_call` | synthesized `ToolCall` | `orphan_tool_update_synthesizes_call` (B) |
| A4 | chunks with and without `messageId` | minted ids and boundaries per 5.3.5 | fixture `turn-basic` (B) |
| A5 | image, audio, resource content | placeholder text or a link | `non_text_content` (B) |
| A6 | `fs/*`, `terminal/*`, `elicitation/create`, unknown requests | -32601 | fixture `agent-requests` (B) |
| A7 | unknown permission option kind | `Other`, shown, never auto-selected | `unknown_option_kind` (B, C) |
| A8 | permission withdrawn (`$/cancel_request`) | agent answered `cancelled`; card resolved `Agent` | fixture `permission-withdrawn` (B) |
| A9 | UI answers after a withdrawal | `request_not_pending`; no second reply | `late_answer_after_withdrawal` (C) |
| A10 | interrupt with an open permission | `cancelled` replies, `session/cancel`, reply awaited, `TurnEnded { Cancelled }` | fixture `cancel` (B) |
| A11 | permission request after the cancel | answered `cancelled` at once | `late_permission_after_cancel` (B) |
| A12 | agent says `end_turn` after (or racing) a cancel | `EndTurn` shown; the rows queued before the Stop stay held | `end_turn_after_cancel` (B), `interrupt_racing_end_turn_holds_queue` (C) |
| A13 | agent ignores the cancel | stays `Cancelling`; force stop ends it | `ignored_cancel_then_force_stop` (C, mock `/ignore-cancel`) |
| A14 | updates after the prompt reply | untagged, so stored outside any turn even when the next queued turn already started; a background permission request survives that turn's end | fixture `updates-after-reply` (B), `late_events_are_untagged` (C, mock `/background` with a queued prompt) |
| A15 | prompt answered with an error | `TurnEnded { Error }` plus `Error { AgentError }`; session stays `Ready` | fixture `prompt-error` (B) |
| A16 | unknown `stopReason` | `Other` plus a log line | fixture `unknown-update` (B) |
| A17 | auth required at open | `AuthRequired` with translated commands | fixture `auth-required` (B), `auth_card_lists_commands` (D) |
| A18 | auth required mid-turn | `TurnEnded { Error }`, the prompt back in the queue (held), the process stopped, `AuthRequired`; Retry starts exactly one process and resends the prompt | fixture `prompt-auth-error` (B), `auth_mid_turn_stops_process_then_retry_resends` (C) |
| A19 | claude-acp without `bypassPermissions` | `acceptEdits`, note, no auto-answers | fixture `claude-no-bypass` (B) |
| A20 | Hermes legacy `models` and `modes` | synthesized options; untyped `session/set_model`, `session/set_mode` | fixture `legacy-models-modes` (B) |
| A21 | Hermes `set_config_option` returns `[]` | list unchanged | `empty_config_reply_ignored` (B) |
| A22 | mode change while Hermes is busy | pending, sent after the turn, notice | fixture `mode-idle-only` (B) |
| A23 | resume supported, load only, neither, -32002, -32601, another error, a timeout | resume; load with replay suppressed; new plus notice for neither, -32002 and -32601; `StartFailed` keeping the id for another error or a timeout, with Start fresh | `open_resume_variants` (B), fixtures `resume-fallback`, `resume-error-keeps-id` (B), `start_fresh` (C) |
| A24 | replay during load or resume | transcript updates dropped; state updates applied | fixture `load-replay` (B) |
| A25 | a chosen option no longer offered | default kept; notice | `desired_option_not_offered` (B) |
| A26 | Full auto per dialect | claude bypass with prompts surfaced; Hermes `execute` answered, edits surfaced; codex full access; generic: the eight tool kinds only; never `allow_always`, `switch_mode`, kind `other` or no kind | `autonomy_candidates_per_dialect` (B), `policy_*` (C) |
| A27 | switching to Full auto with open cards | answered at once where the policy allows | `autonomy_switch_answers_open_cards` (C) |
| A28 | adversarial field sizes | every non-delta event within `EVENT_BUDGET` after normalizing (every field at its maximum, randomized multi-entry content), except the documented option-id case; full originals in blobs | `normalized_events_fit_budget` (C, property test) |
| A29 | `_auth/status_update` with an email | never in the log or the database | `auth_status_redacted` (A, B) |
| A30 | usage scope | per turn for claude-acp and codex-acp; Hermes deltas within a process | fixture `hermes-usage-session` (B) |
| A31 | batch arrays | dispatched in order | fixture `garbage` (B) |
| A32 | permission with no options | card with Dismiss (answers `cancelled`) | `permission_without_options` (B, D) |
| A33 | two permission requests at once | two independent cards | `concurrent_permissions` (B, C) |
| A34 | Hermes force stop | `session/cancel` first, up to 5 s for the reply, then the viewer is killed; gateway untouched; notice | `force_stop_waits_for_daemon_cancel` (C) |
| A35 | our wire format matches the schema | every request of 5.3.1 equals the JSON of the same request built with the schema crate's builders (resume's empty `mcpServers`, which the schema type skips, removed before comparing); updates built with the schema's builders parse with our wire types to the expected events; every `sessionUpdate` value in `schemars::schema_for!(SessionNotification)` (dev-dependency with `schemars` and `unstable`; 19 values, checked) is in `MAPPED_TAGS` | `conformance_requests`, `conformance_updates`, `every_update_tag_is_mapped` (B) |
| A36 | permission request before `Ready` (Hermes confirming a guarded model while `session/set_model` waits) | forwarded untagged; the card shows while `Starting`; the answer reaches the agent | fixture `permission-during-setup` (B), `card_before_ready` (C) |
| A37 | generic Full auto meets a question (kind `other`, `allow_once` choices) | asked, never auto-answered | fixture `question-kind-other` (B), `policy_never_answers_kind_other` (C) |
| A38 | the user picks a mode by hand | auto-answers off with the note; the candidate's mode or a new autonomy restores them | fixture `hand-picked-mode` (B) |
| A39 | a long replay during load or resume | the open deadline resets on every update; 10 minutes at most | fixture `open-replay-long` (B) |
| A40 | Hermes continued after a force stop or quit | at `Ready`, a notice and its latest reply outside any turn; a matching replay adds nothing | fixtures `hermes-continued-after-quit`, `hermes-replay-matches` (B), `replay_summary_compared_at_ready` (C) |
| A41 | the UI-request round trip (mock `/ask`) | question card; answer, dismiss, withdrawal; recovery resolves an open one | `mock_question` (B), `question_round_trip` (C), `question_card_flow` (D) |
| A42 | update to an earlier turn's tool (a background task) | applied to its block live; compaction keeps it as a `ToolUpdate`; the rendered transcript is the same before and after compaction | `cross_turn_tool_update_compaction` (C), `compacted_equals_live` (D) |

### 8.4 Sessions and persistence

| # | Case | Behaviour | Test (owner) |
|---|---|---|---|
| S1 | crash mid-turn, then relaunch | turn `interrupted`, synthetic resolutions and `TurnEnded { Interrupted, recovered: true }`, compacted, partial message kept | `recovery_interrupts_running_turns` (C) |
| S2 | crash during compaction | rolled back; redone at the next start | `compaction_is_idempotent` (C) |
| S3 | disk full | paused, banner, the writer keeps only the failed batch, backpressure reaches the agents (an agent exiting meanwhile loses nothing), resumes, nothing lost | `disk_full_pauses_without_loss` (C, `PRAGMA max_page_count`) |
| S4 | damaged database during a run | damage recorded, dead-letter file, `Failed` status | `corruption_switches_to_dead_letter` (C) |
| S5 | panic inside a writer batch | rolled back, isolated, thread continues | `writer_survives_panicking_batch` (C) |
| S6 | a poison op | isolated to the dead-letter file | `poison_op_is_isolated` (C) |
| S7 | attach at any point of a turn, including just before it ends and compacts | page plus frames rebuild the full transcript, no gap, no duplicate | `attach_has_no_gap_or_duplicate` (C, randomized attach points) |
| S8 | attach, list chats and read blobs while paused | served at once from saved data; the page marks `unsaved_from`; the view re-attaches on resume | `attach_while_paused_reads_saved_rows`, `reads_never_wait_behind_a_paused_writer` (C) |
| S9 | page reload | that webview's subscriptions dropped | `page_load_drops_subscriptions` (D) |
| S10 | channel send fails | subscriber removed | `failed_sink_is_removed` (C) |
| S11 | two subscribers | identical frames; the first permission answer wins | `two_subscribers` (C) |
| S12 | history paging | whole turns, byte budget, `has_more`, events outside turns, empty chat, a turn larger than a page (a generated 50 MB turn, compacted and not) paged by rows with `cut_turn_id` | `history_pages_*`, `huge_turn_pages_by_rows` (C) |
| S13 | compaction rules | per 6.3.1, FTS rows written | `compaction_*` (C) |
| S14 | a turn over 32 MiB | not merged; FTS capped; memory bounded | `huge_turn_skips_merge` (C, scaled-down limit) |
| S15 | queue | queued while running, next after a normal end; rows held at interrupt, stop, error, crash and restart; a prompt typed after a stop, crash or cancel waits behind the held rows; Send it now or Discard on the held head; remove; survives a restart held; Send queued releases | `queue_*` (C) |
| S16 | empty, oversized, or over-queued prompt | `prompt_empty`, `prompt_too_large`, `queue_full` | `send_validation` (C) |
| S17 | prompt while stopped or failed | starts the agent; dispatched at `Ready`, not held | `send_starts_agent` (C) |
| S18 | option change during a turn | sent at once, except deferred Hermes mode changes | `option_change_during_turn` (C) |
| S19 | autonomy change mid-turn | next decision uses it at once | `autonomy_change_is_immediate` (C) |
| S20 | 17th live agent | the agent idle longest stops and its slot passes to the start; `too_many_agents`, saying what the others do, only when none idles; a stopped chat resumes its session | `a_start_at_the_limit_stops_the_longest_idle_agent`, `a_start_is_refused_only_when_every_agent_is_busy`, `an_evicted_chat_resumes_its_session_on_the_next_message`, `background_work_is_not_stopped_to_make_room` (C) |
| S21 | idle 10 minutes without a viewer | graceful stop, except with an open card or question or a non-held queued row; restart with resume on next use | `idle_eviction` (C, paused time) |
| S22 | chat actor panics | processes killed, turn interrupted, subscribers end, next call recreates it | `actor_panic_is_contained` (C) |
| S23 | driver task panics | `Panicked`, processes force-stopped (P47), error card, `Stopped` | `driver_panic_is_contained` (C) |
| S24 | quit with running turns | cancel, graceful stop within 8 s, turns interrupted, outboxes handed to the store, store closed, registry file removed | `shutdown_all_bounded` (C), `quit_flow` (D) |
| S25 | quit while paused | asks first; when confirmed, bounded; unsaved events lost and logged (counts); consistent at the next start | `quit_while_paused` (C), `quit_asks_when_paused` (D) |
| S26 | seq near `u32::MAX` | new turns refused with a message | `seq_limit` (C) |
| S27 | titles | first line first, the agent's title replaces it | `titles` (C) |
| S28 | one native session in two chats | refused for the resume id before spawning and for the returned id at `Ready`, for every driver | `one_process_per_native_session` (C) |
| S29 | agent silent for an hour | no timeout anywhere; UI hint only | `running_turn_has_no_timeout` (C, paused time) |
| S30 | secrets in the stderr tail | redacted before `Error.detail` is stored | `stderr_tail_redacted_before_persist` (C) |
| S31 | permission or question while the chat is not viewed | status channel `needs_attention` | `needs_attention_on_permission` (C) |
| S32 | Esc during start | preparation (install and probes included) or process killed, `Stopped`, prompt held | `interrupt_during_start` (C) |
| S33 | late events from a force-stopped driver | never applied | `late_events_after_force_stop_ignored` (C) |
| S34 | actors never wait on each other | driver flooding while the actor is paused; force stop still works while storage is paused; a wedged driver is killed through its recorded control | `actor_and_driver_never_deadlock`, `force_stop_while_storage_paused`, `force_stop_bypasses_wedged_driver` (C, ScriptedDriver) |
| S35 | a second process opens the same profile | it fails at boot with `ProfileInUse`; `aura-cli --data-dir` refuses; the first process's transcript is untouched | `second_owner_is_refused` (C), `boot_reports_profile_in_use` (D) |
| S36 | quit with no live session and ops still queued | every op committed, the WAL folded back, the next launch reports a clean shutdown | `quit_without_sessions_commits_and_closes` (C, D) |
| S37 | crash with a permission card or question open outside a turn | recovery resolves it `System`; no clickable stale card | `recovery_resolves_outside_turn_requests` (C) |
| S38 | the user's answer crosses the agent's withdrawal | one resolution (the user's) plus the withdrawal notice | `answer_crossing_withdrawal` (C) |
| S39 | send while `Stopping` | queued, not held; the agent starts again once `Stopped` | `send_while_stopping_restarts` (C) |
| S40 | crash between removing a queued prompt and starting its turn | impossible: one `BeginTurn` transaction; after a crash the prompt is either queued or started | `begin_turn_is_atomic` (C, crash injection) |
| S41 | SIGTERM, SIGINT or SIGHUP | the quit flow; a second signal kills at once | `signal_starts_quit_flow` (D, Unix) |
| S42 | Start fresh | a new native session with the notice; the old row stays | `start_fresh` (C) |
| S43 | rows written by a newer build | unknown enum strings read as their fallback; an unparsable `turn_ended` still ends its turn; compaction keeps unparsable rows verbatim | `newer_rows_load_leniently` (C), lenient enum tests (P0) |

### 8.5 UI

| # | Case | Behaviour | Test (owner) |
|---|---|---|---|
| U1 | frame gap or channel end | detach, then a full re-attach | `gap_reattaches` (D, Vitest) |
| U2 | Esc twice | interrupt, then force stop | `escape_twice` (D) |
| U3 | permission card clicked | buttons disabled; removed on resolution or `request_not_pending` | `permission_card_flow` (D) |
| U4 | unavailable agent | disabled with its reason | `unavailable_agent_disabled` (D) |
| U5 | folder picker cancelled | nothing changes | `picker_cancel` (D) |
| U6 | compacted history and live stream | same rendered transcript, cross-turn updates included | `compacted_equals_live` (D, goldens) |
| U7 | truncated output | "Load full output" pages through `chat_blob` by `next_offset`; "Show full details" pages the event blob | `load_full_output` (D) |
| U8 | still working | shown after 60 s without frames, "no output for 1 minute" counting up; never kills | `still_working_indicator` (D, fake timers) |
| U9 | `revision` change | `chat_lists` refetched, debounced | `lists_refetch_on_revision` (D) |
| U10 | tool update content | `null` keeps, `[]` clears | `tool_update_content_semantics` (D) |
| U11 | a future event variant | typecheck fails (exhaustive `never`) | `pnpm typecheck` (D) |
| U12 | question card | answer, dismiss, `request_not_pending` removes it | `question_card_flow` (D) |
| U13 | a page that cut a turn | marker; "Load earlier" continues the same turn | `partial_turn_page` (D) |
| U14 | a page with unsaved rows | marker at the gap; re-attach when storage returns to `Ok` | `unsaved_marker_and_reattach` (D) |
| U15 | re-attach | detaches first; a new attach replaces the webview's older subscription to the chat | `reattach_detaches_first` (D), `attach_replaces_same_owner` (C) |
| U16 | a multibyte character at a 256 KiB page boundary | not split | `blob_pages_on_char_boundaries` (C) |

---

## 9. Work breakdown

### 9.1 P0: the contract commit (one implementer, 2 to 3 days, merged before anyone branches)

| Files | Content |
|---|---|
| `Cargo.toml`, `Cargo.lock`, every crate's `Cargo.toml` (new crates `aura-process`, `aura-agents`; the desktop gains both) | the dependencies and features of 1.1, including tokio `signal` and aura-drivers' `replay` feature; nobody else touches manifests or the lock until phase 2 |
| `crates/aura-protocol/src/{lib.rs,session.rs,lenient.rs}` | all of section 2, with round-trip, `is_stored`, `kind` and lenient-enum tests |
| `crates/aura-process/**` | every public item of 3.1. Stub bodies return errors such as `SpawnError::Spawn { source: io::Error::other("not implemented in this build") }`, never `todo!()` (clippy and `-D warnings` reject it). **Implemented:** `testing.rs` (`FakeSpawner`, `FakeControl`), `EnvResolver::fixed`, `ProtocolLog::disabled` and `disabled_with_id`, `strip_launch_context`, `child_env`, `scrub`, `secret_values`. **Functional stubs:** `EnvResolver::start` resolves at once to `process_env()` minus `strip_launch_context` as `Fallback`; `LeftoverRegistry::open` returns `disabled()`; `LogFactory::open` returns `ProtocolLog::disabled()`; `Redactor::standard()` replaces only the secret literals (A adds the key rules and the patterns), and the stub `EnvResolver::start` hands its env's `secret_values` to the config's redactor. |
| `crates/aura-agents/**` | the catalog data (complete), `Detector` signatures. **Implemented:** `Detector::fixed` (with `FixedAgent`, `Gate` and `forgotten`), `Detector::spec`, `parse_version`, `version_at_least`. **Functional stub:** `Detector::new` reports the mock `Ready` and every other agent `Missing { "detection is not built in this build" }`; `prepare_launch` works for `Builtin` |
| `crates/aura-drivers/src/{lib.rs,driver.rs,testing.rs,batch.rs,mock.rs}`, plus empty module files with their declarations for B (`jsonrpc.rs`, `acp/{mod,wire,map,options,dialect,permissions}.rs`, `mock_driver.rs`, `factory.rs`, `replay/{mod,fixture,fake_agent,golden,curate}.rs`) | section 5.1 complete, `event_channel` included; `ScriptedDriver`, `ScriptHandle`, `ScriptedDriverFactory` and `DirectSpawner` implemented (5.7); the tagged push and flush in `batch.rs`; the rename fallout |
| `crates/aura-core/{migrations/0002_sessions.sql,src/store.rs,src/store/salvage.rs,src/lib.rs,src/error.rs,src/owner.rs,src/chatstore/mod.rs,src/session/mod.rs}`, plus empty files for C's other modules | migration 0002 with its tests; `ProfileOwner` and `CoreError::ProfileInUse` implemented with a two-process test; the API skeletons of 6.1 and 6.4 |
| `apps/desktop/src-tauri/src/{commands.rs,error.rs,bindings.rs,demo.rs,boot.rs}`, `apps/desktop/src/{bindings.ts,lib/stream.ts,lib/session.ts}` | the command signatures of 7.1 returning `internal`, the error codes, boot taking the owner lock, regenerated bindings, the M0 TypeScript fallout |
| `docs/plan.md` | the corrections in appendix B |

Acceptance: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
`cargo clippy -p aura-process -p aura-agents --target x86_64-pc-windows-gnu -- -D warnings`, then
`rustup target add x86_64-unknown-linux-gnu` and `cargo clippy -p aura-process -p aura-agents --target
x86_64-unknown-linux-gnu -- -D warnings` (neither crate builds C code, so no cross compiler is needed; this is what
type-checks `ptable/linux.rs` before a push), `pnpm check`, the bindings check, and the v1-to-v2 migration test.
Every gate runs locally first. Pushes to `m1a-core-acp` then run CI on Linux and Windows (macOS runs only for `main`),
which adds the real Linux and Windows test runs; packages A and C repeat the two cross-target clippy runs before each
merge.

### 9.2 Frozen after P0 (changes go through one small commit on `m1a-core-acp` that everyone rebases on)

1. aura-protocol, all of section 2.
2. aura-drivers `driver.rs` (5.1), including `EventSender`, `EventReceiver`, `LoggedCommand`, and the test doubles of
   5.7.
3. aura-process public API (3.1), including `LaunchSpec`, `ProcessSpawner`, `ControlImpl`, `FakeSpawner`,
   `FakeAgentEnd`, `FakeControl`.
4. aura-agents: `catalog`, `get`, `AgentSpec`, `Recipe`, `UserBinary`, `Detector`'s methods, `LaunchError`, and the
   test double `FixedAgent` with `Gate`.
5. aura-core: `ProfileOwner`, `ChatStore` and its records (6.1), `SessionManager`, `ManagerDeps`, `FrameSink`,
   `StatusSink`, `SessionError` (6.4).
6. IPC command names, signatures and `AppErrorCode` values (7.1).
7. Migration 0002 and the protocol log record format (3.8).

### 9.3 Packages (parallel, separate worktrees, file-disjoint)

| Package | Exclusive files | Builds against | Acceptance |
|---|---|---|---|
| **A: processes, env, logs, agents** | `crates/aura-process/**` (bodies, private modules, `src/bin/ghost-testbin.rs`, `tests/**`), `crates/aura-agents/**` | P0 only | P1 to P5, P12, P14 (framer), P17 to P35, P39 to P41, P43 to P46 (A's parts), P49 to P51, E1 to E15, A29 (redactor); log rotation, retention, drop counting and redaction tests (emails, token shapes, credential and identity keys, token counts kept, secret literals); registry atomicity under concurrent register and unregister, the heartbeat; process-table parsers on captured `ps` and `/proc` text; the spike's ported process tests. Real-process tests run on macOS locally and on Linux and Windows in CI. |
| **B: drivers** | `crates/aura-drivers/src/{jsonrpc.rs,acp/**,mock_driver.rs,factory.rs,replay/**}`, `crates/aura-drivers/tests/**` (synthetic fixtures, goldens) | P0 (`FakeSpawner`, `DirectSpawner`) | P6 to P11, P13 to P16, P33 (hint), P46 (driver), A1 to A8, A10 to A12, A14 to A26, A29 to A33, A35 to A41 (B's parts); one test per mapping row and per reply row; every synthetic fixture replays to its golden; the driver contract (5.1) checked over every fixture, including exactly one `TurnEnded` per `Prompt`, tags, and nothing lost at exit; `MockDriver` behaviours; JSON-RPC parsing (requests, notifications, results, errors, batches, string and numeric ids, missing `jsonrpc`). **Early merge:** `MockDriver` and `StandardDriverFactory` land on `m1a-core-acp` by day 5 (B's files only), so C and D run the demo agent end to end before the final merge. |
| **C: storage and sessions** | `crates/aura-core/src/{chatstore/**,session/**}`, `crates/aura-core/tests/**` | P0 (`ScriptedDriver`, `FakeSpawner`); B's early `MockDriver` | P1, P6, P10, P36 to P38, P42, P47, P48, A7, A9, A12 to A14, A18, A23, A26 to A28, A33, A34, A36, A37, A40 to A42, S1 to S43 (C's parts), U15 and U16 (C's parts); writer group commit, atomic ops, ordering and shutdown; history pages for a generated 10,000-turn chat under 50 ms each on this Mac; the compaction property test. |
| **D: desktop and bare UI** | `apps/desktop/src-tauri/src/{commands.rs,sessions.rs,state.rs,lib.rs,error.rs,bindings.rs,boot.rs}`, `apps/desktop/src/**` | P0; a fake transport in Vitest until C merges, B's early `MockDriver` | S9, S24, S25, S35, S36, S41 (D's parts), U1 to U15, A17, A32 and A41 (cards); every `SessionError` maps to one `AppErrorCode`; the bindings test lists every new command and checks the UI-facing protocol types; `ChannelSink` removes itself on a send error; manual: every flow with the demo agent in `pnpm dev`. |

`ghost-testbin` modes (A): `json-lines <n>`, `flood-stderr <bytes>`, `ignore-sigterm`, `spawn-group-child <pidfile>`,
`spawn-setsid-daemon <pidfile>` (a forked child that calls `setsid` and re-executes itself as `ghost-testbin daemon
run`, which the test's `DaemonMatch` spares; macOS has no `setsid` command), `spawn-pty-child <pidfile>` (the same
without a daemon argv, standing in for a PTY host or a detached Node child), `spawn-job-child <pidfile>`,
`exit-before-read <code>`, `exit-with-buffered <lines>` (writes and exits at once), `oversized <bytes>
<request|response|notification>`, `noise`, `hang`, `print-env`, `print-cwd`, `fake-shell <ok|junk|hang|nonzero|no-marker>`,
`fake-npm <ok|hang|fail|partial>` (records its cwd and argv, writes a fake package with a `bin`). Every process test
kills what it spawned in a guard and asserts no leftover by pid and start time.

### 9.4 Order

```text
day 0-3    P0 (one implementer), merged to m1a-core-acp
day 3-9    A | B | C | D in parallel (B on FakeSpawner, C on ScriptedDriver, D on a fake transport)
day 5      B merges MockDriver and StandardDriverFactory; C and D rebase and run the demo agent end to end
day 9-10   merge A, then B, then C, then D (file-disjoint; Cargo.lock changed only in P0 until phase 2 adds aura-cli)
day 10-13  phase 2, integration (B's, C's and D's implementers together): aura-cli, end to end with the demo agent,
           the free probes, then the paid scenarios of section 10, recorded fixtures, fixes, the manual
           macOS pass; Linux and Windows CI on every push
```

### 9.5 Phase 2: integration

| Files | Content |
|---|---|
| `crates/aura-cli/**` (a new workspace member: its manifest and lock entries land here) | headless `SessionManager` over a temp data root (never the user's profile unless `--data-dir`, which takes the owner lock and refuses while the app holds it) and a fresh temp git repo as the chat folder; its processes register in the real data root's `run/` (3.5). `aura-cli agents` (env status, removed names, availability); `aura-cli probe <agent> [--env K=V] [--folder <dir>]` (free steps of 10.2; `--env` becomes `set_env`, so it overrides the user's value); `aura-cli smoke <agent> --scenario <name> --spend [--model M] [--effort E]` (refuses without `--spend`); `aura-cli fixture <log> --scenario <name> --out <file>` (curate, scan, print a review checklist); `aura-cli replay <fixture> [--update]` |
| `crates/aura-drivers/tests/fixtures/acp/{hermes,claude-acp,codex-acp}/**` | recorded, curated fixtures and goldens |

Acceptance: every free probe passes for the three agents; every paid scenario the owner approved passes and is
recorded; no leftover process after any run (`ps` diff); the manual pass of 10.4 on macOS.

---

## 10. Real-agent verification

Nothing here ran during design. Free steps cost nothing. The paid scenarios are approved: the owner approved M1a's
real-agent verification in the plan and, on 2026-10-24, asked not to be asked again. They run only after every free
probe passes, one scenario at a time, on the cheapest model each agent offers, and stop at the first failure.

### 10.1 Before anything

Record `hermes --version`, `claude --version`, `codex --version`, `node --version`, `npm --version`, a `ps -axo
pid=,ppid=,pgid=,lstart=,command=` snapshot, and the running Hermes gateway's pid if any. One fresh `git init` scratch
folder per agent, outside the repository and outside `~/Desktop` (macOS privacy prompts are checked separately in
10.4). Every Hermes probe creates a session in the owner's own Hermes history, and the M1a report says so (Hermes
keeps sessions in its gateway; a separate `HERMES_HOME` would start a second gateway, which is worse).

### 10.2 Free probes (`aura-cli probe <agent>`, for hermes, claude-acp and codex-acp)

0. Install (claude-acp, codex-acp): time and size of the one-time `npm ci` into `<data root>/adapters`; no
   `@anthropic-ai/claude-agent-sdk-<platform>` or `@openai/codex-<platform>` package is present; `npm prefix` run in
   the adapter folder prints that folder.
1. Spawn and `initialize` with the chat folder set to a hostile fixture repo (a `.npmrc` pointing `@agentclientprotocol`
   at `http://127.0.0.1:9`, and a committed fake `node_modules/@agentclientprotocol/claude-agent-acp` whose bin prints
   "HOSTILE"): the real adapter answers (agent info name and version), nothing reads the repo's npm files. Record the
   protocol version, capabilities, auth method ids and time to answer.
2. `session/new` in a never-trusted temp repo: option ids, categories and counts, legacy `models` count, modes, the
   current model (Hermes: `models.currentModelId`, read before any Hermes prompt and recorded for the report),
   and whether the agent refuses or asks about folder trust.
3. Wait 3 s for `available_commands_update`: count and largest line.
4. Set every option to its current value (no-op), then switch the model to the cheapest offered and back: the
   reported model changes both times (Hermes: while switched, a second `session/new` still reports the configured
   model if the switch stays inside the session; switching back restores it either way). Walk autonomy Full auto, Auto-edit, Ask and back, and check the mode values of
   5.5.2 (claude-acp: `bypassPermissions` if this user's settings allow it, else `acceptEdits` and the note). This
   confirms claude-acp's real mode ids, which the scratchpad sources do not list.
5. `session/cancel` while idle: harmless.
6. Close stdin: exit time and code; the `ps` diff shows nothing left of the agent's group or descendants; Hermes'
   gateway, if one runs, is still alive and listed as spared.
7. Restart with `session/resume` (or `session/load`): no transcript updates during the open, options back, desired
   values re-applied; for Hermes, `ReplaySummary` matches what was stored (no notice).
8. Auth required, codex-acp only: `--env CODEX_HOME=<empty temp dir>` gives -32000 and the auth card shows `codex
   login`; the user's real login is untouched. Claude and Hermes auth paths stay on synthetic fixtures (an empty
   config dir is not proven to bypass the keychain, and a fresh Hermes home would start its own gateway).
9. Interrupt during startup (after `initialize`), and during a first install in a fresh temp data root: `Stopped`, no
   leftover, the next start installs cleanly.
10. Finder launch of a built app: `env_status` is `LoginShell`, its `PATH` has `~/.local/bin`, Hermes starts.

Each probe's protocol log is curated into `probe-*` fixtures.

### 10.3 Paid scenarios (approved; cheapest model, lowest effort; 6 per agent, 18 in total)

| # | Scenario | Prompt (exact) | Checks |
|---|---|---|---|
| 1 | basic, after a model switch | switch to the cheapest model (free), then `Reply with exactly: ok` | streaming, `end_turn`, usage with the right scope, the turn ran on the switched model |
| 2 | permission (Ask) | `Create an empty file named aura-perm.txt in the current folder, then reply done.` | a card appears; Reject: no file, the agent carries on. Then `Create it now.` and Allow once: the file exists |
| 3 | interrupt | `Count from 1 to 300, one number per line.` | Stop after the first deltas: `Cancelled`, partial message kept, queued rows held; for Hermes no turn left running in its gateway |
| 4 | after interrupt | `What was the last number you wrote? Reply with only the number.` | the partial answer stayed in the agent's context |
| 5 | Full auto | `Create a file named aura-auto.txt containing hi.` | no card for claude-acp and codex-acp; for Hermes no card for edits in the workspace (`dont_ask`), "Auto-approved" lines only for command approvals; the file exists |
| 6 | resume | stop the session (free), `chat_start`, then `Which files did you create in this chat? Reply with the file names only.` | names both files; no duplicated transcript |

Hermes takes scenario 1's switch only if probe 4 showed that a switch stays inside the session; otherwise every Hermes
scenario runs on the model its own configuration selects, which probe 2 records.
Each run: `aura-cli smoke ... --spend` writes the protocol log, `aura-cli fixture` curates it, the diff is reviewed by
eye, the fixture scan passes, and fixture plus golden are committed.

Agent errors M1a shows generically: a rate-limit or quota error arrives as a prompt error and shows the agent's message
in an `AgentError` card (no limits meter for ACP agents in M1a); a user binary below its catalog floor is `Broken`
with the floor in its reason (P50).

### 10.4 What stays manual

- The bare UI with the demo agent is no longer manual: the chat smoke run (`.github/scripts/run-smoke.sh chat`, in CI
  on Linux and Windows, and on a Mac by hand) drives every block type, the permission cards, the question card (demo
  `/ask`), the error, crash (demo `/crash`) and auth cards, the queue held or sent as decision 11 says, Stop and Force
  stop, quit through the quit flow and reopen (history, the interrupted turn, Resume), and checks that no `-wal` file
  is left, reading the screens through their `data-testid` and `data-test-*` attributes. What stays manual: the bare
  UI on macOS with each real agent (`AURACODE_SMOKE_AGENT=<agent id>` runs one minimal chat with it, locally only),
  the crash card from `kill -9` on a real agent's pid, Ctrl+C under `pnpm dev` (the quit flow runs), the still
  working hint, the storage banner and the quit question while paused (optional: a small disk image as
  `AURACODE_DATA_DIR`).
- macOS privacy: a fresh build (its ad-hoc signature is new, so no grant applies) and a chat in a `~/Desktop` folder:
  deny the prompt and see `FolderNotAllowed` with the System Settings hint, never "folder missing"; allow it in System
  Settings, press Retry, and the agent starts.
- Windows (the owner's PC; CI runs the automated Windows tests): hidden consoles under real agents, job tree kills, the direct-child exit
  detection, Hermes with its gateway already running and the hint without it, the registry `PATH` merge after
  installing an agent while auracode runs, the one-time `npm.cmd` install, the `.exe` rule for user binaries, sleep and
  wake during a turn; the spike's checklist plus these steps.
- Linux real-agent runs; sleep and wake during a turn on macOS; a 30-minute run with steady output, sampling memory.

### 10.5 Unverified until implementation

| Assumption | If wrong |
|---|---|
| every Windows path (job wrappers, direct-child wait, registry merge, `npm.cmd` install, the shim rule, Hermes' hint) | the Windows rows stay pending; the owner's PC decides |
| claude-acp's mode ids (`default`, `acceptEdits`, `bypassPermissions`) | probe 4 prints the real list; the candidate lists change, nothing else |
| claude-acp reports a missing login as -32000 at `session/new` rather than only at the first prompt | the auth card appears after the first prompt instead; same handling |
| codex-acp drives Codex 0.164.0 through `CODEX_PATH`; claude-acp reuses the CLI's login | probes 1 and 2 settle both for free |
| both adapters start with `--omit=optional --ignore-scripts` installs | probe 0 and probe 1 show it; dropping `--ignore-scripts` for one adapter is a catalog change |
| Hermes' replay lets the whitespace-free reply hash match what auracode stored | probe 7 shows it; a false notice only adds a duplicate of the latest reply |
| the adapter download fits in 10 minutes, Hermes' gateway start in 60 s | both are catalog or constant values; Retry works |
| `preserve_order` breaks no existing test | P0 runs the whole suite right after enabling it |


### 10.6 Results (2026-10-25)

Run on macOS against Hermes Agent 0.22.5 (its free tier, `nous/welcome`), Claude Code 2.1.310 through claude-agent-acp
0.95.0 (Haiku 5.5, effort low) and codex-cli 0.164.0 through codex-acp 2.2.2 (gpt-6-luna, effort low). Every run
checked afterwards that Claude's daemon and Hermes' gateway were alive and unchanged and that nothing of auracode's
was left running.

| Agent | Free probes | Paid scenarios | Chat UI (`AURACODE_SMOKE_AGENT`) |
|---|---|---|---|
| hermes | pass; `probe-*` fixtures | 18 of 18 checks | pass |
| claude-acp | pass; `probe-*` fixtures | 20 of 20, including the model and effort switches | pass |
| codex-acp | pass; `probe-*` fixtures | 19 of 19; after-interrupt informational (below) | pass |

Bugs the runs found in auracode, each fixed with a test:

- A prompt went out while a mode change was still on its way, so it ran under the agent's previous mode: after a
  switch from Full auto to Ask, Hermes made the next edit without asking. A prompt now waits for the mode and option
  changes queued before it (5.2); an interrupt or a close ends a waiting prompt once.
- Hermes refuses a mode change that reaches its gateway before the previous one landed (-32603 with `details:
  "stale_generation"`). The driver sends it again after 250, 500 and 750 ms (`AcpDialect::mode_change_retryable`),
  instead of falling back as if the mode did not exist. Fixture `hermes/mode-retry-stale`.

What the agents do that the scenarios had assumed otherwise:

- Hermes' Ask (`default`) asks before edits made with its file tools; shell commands go to Hermes' own approval
  model (an auxiliary model call), which runs harmless ones such as `touch` without asking. Its permission scenario
  asks for `write_file`, and its Ask carries a note saying so.
- Codex's Ask (codex-acp's `read-only`, approvals `on-request`) asks only when the model requests more access;
  gpt-6-luna sometimes just said the folder was read-only. Its permission scenario names the command and asks it to
  request approval, and its Ask carries a note. A deterministic Ask waits for M1c's app-server driver, which sets the
  approval policy itself.
- After an interrupt and a restart, Codex's resumed thread holds every completed turn but not the interrupted
  answer, so after-interrupt is informational for codex-acp. auracode keeps the partial answer (scenario 3).
- Claude Code asked before a shell command in Ask, as the scenario assumed; its usage arrives per turn.

---

## Appendix A: constants

Timers and deadlines use tokio's clock, which does not advance while macOS or Linux sleeps.

| Name | Value | Where |
|---|---|---|
| `MAX_FRAME_BYTES` | 8 KiB | aura-protocol |
| `EVENT_BUDGET` | 7 KiB per non-delta event after normalizing | aura-protocol, normalizer |
| flush | 16 ms with a subscriber, 100 ms without, immediate after key events | session actor |
| `DRIVER_EVENT_CAPACITY`, `DRIVER_EVENT_BYTES` | 256 events, 8 MiB; driver outbox 64 events or 8 MiB before stdout pauses | driver |
| stdout | line cap 64 MiB; reader channel 16 lines or 8 MiB; 2 s for EOF after the exit | aura-process, driver |
| stderr | 16 KiB tail (2 KiB shown), 4 KiB per line, 256 KiB/s logged | aura-process |
| start timeout | 60 s for binaries and adapters | catalog |
| adapter install | 10 min; the `--version` check 30 s | aura-agents |
| open timeouts | 60 s without a reply or an update, 10 min in all | ACP driver |
| early-update buffer | 256 updates while `session/new` is pending | ACP driver |
| option request timeout | 30 s (IPC 35 s) | ACP driver |
| commands debounce | 250 ms | ACP driver |
| replay summary | last reply up to 64 KiB | ACP driver |
| shutdown plans | STANDARD 5 s / 3 s / 2 s; QUIT 3 s / 1 s / 2 s; FORCE TERM now / 2 s / 2 s | aura-process |
| cancel waits | stop 3 s, quit 2 s, force stop 200 ms; 5 s for each when the agent's turns survive its process (Hermes); a stop not done after 15 s is forced | session actor |
| quit | `shutdown_all` 8 s, then the store 5 s; 1 s at `RunEvent::Exit` | desktop |
| env resolution | 10 s interactive, 5 s retry, failed attempts retried after 60 s | aura-process |
| probes | 10 s, 64 KiB output | aura-agents |
| registry | heartbeat 60 s; sweep window from the entry's start to the heartbeat plus 60 s; SIGTERM, 2 s, SIGKILL | aura-process |
| live agents | 16; a start at the limit stops the agent idle longest, refused only when none idles; idle stop after 10 min | session manager |
| queue | 20 prompts; 1 MiB per prompt; preview 256 chars | session manager |
| subscribers | 4 per chat (one per webview), 16 per webview | session manager |
| attach | 5 s | session manager |
| writer | FIFO 1,024 ops and 32 MiB; batch 256 ops or 4 MiB; actor outbox 4 MiB; disk-full retry 5 s; busy backoff 100 ms to 2 s, paused after 10 s | chat store |
| history page | 512 KiB default, 64 KiB to 4 MiB, at least one row | chat store |
| compaction | 1 MiB pieces; no merge above 32 MiB per turn; FTS 1 MiB per message | chat store |
| normalizer | titles 256 chars; raw-input preview 512 bytes; locations 1 KiB; an entry never below 256 bytes; option names 64, then 24 chars; blobs up to 64 MiB; `chat_blob` 256 KiB per call | normalizer |
| protocol logs | record 256 KiB, file 8 MiB x 3, 256 MiB total, 14 days, queue 4,096 | aura-process |
| UI | still working after 60 s without frames; lists refetch debounce 200 ms | desktop |

## Appendix B: corrections P0 makes to `docs/plan.md`

1. Architecture: process lifecycle, env resolution and protocol logs live in the new `aura-process` crate;
   `aura-agents` keeps the catalog, detection and the adapter install.
2. Crates: no runtime `agent-client-protocol`; our JSON-RPC peer, with `agent-client-protocol-schema =1.12.0` for
   conformance tests; `serde_json` with `preserve_order`; no tokio-rusqlite in M1a (one writer thread).
3. Driver trait line: the actor contract (`AgentDriver::run`, commands in, events out, every event tagged with its
   turn); prompts are a list of parts; questions have an answer path (`RespondUi`, `UiResolved`); turns, queueing and
   persistence belong to the session manager.
4. ACP paragraph: `session/set_model` is gone from the schema (legacy dialect only); model and effort are config
   options by category; the client advertises `auth.terminal`, `session.notices`, `session.configOptions.boolean`.
5. AuraEvent list: `user_message_delta`, `prompt_lifecycle` (the command lifecycle event), `ui_resolved`;
   `session.updated` no longer carries slash commands; `TokenUsage` split with a scope; `StopReason::Interrupted` and
   `Other`; stored enums read unknown values leniently.
6. Engineering rules, processes: the descendant walk kills every descendant except the daemons each agent's catalog
   entry declares (Hermes' gateway, Claude's daemon), on every OS; Windows job breakaway for agents that declare
   daemons arrives in M1e, and M1a's Hermes on Windows needs its gateway already running; one auracode process owns a
   profile's sessions (`owner.lock`); M1a adds no `unsafe` module.
7. Agent rows: Hermes is a viewer of a gateway daemon (cancel before kill, with time for the cancel to land; modes
   `default`, `accept_edits`, `dont_ask`; Full auto = `dont_ask` plus auto-answers for command approvals); the Claude
   and Codex day-one fallbacks are the npm adapters installed into `<data root>/adapters` with `npm ci
   --omit=optional --ignore-scripts` from a lockfile committed per adapter version, and launched as `node <entry>`
   from there, with the user's binary forced through
   `CLAUDE_CODE_EXECUTABLE` or `CODEX_PATH`; never `npx` in the chat folder.
8. M1b line: the session actor owns the lifecycle event, so the Claude driver logs `command_lifecycle` instead of
   mapping it; M1b sets `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` through `LaunchSpec.set_env`.
9. Detection: the fallback known dirs add the asdf and mise shims and the `prefix=` of `~/.npmrc` (the plan's "npm
   prefix, shims"); nvm stays with the login shell; launch-context variables are stripped from the resolver's input.

## Appendix C: owner decisions

Decided on 2026-10-24 under the owner's standing instruction to choose the best option without asking. The
implementers build these; each can change later without touching a frozen interface.

1. **Full auto never overrides an agent's own "always ask" decisions** (Claude's bypass-immune prompts, Hermes'
   sensitive-path edits for `.env`, `.ssh`, `.git`; Claude with bypass disabled by the user's settings runs as Accept
   edits). Alternative: auto-answer them too, which makes Full auto quieter but answers prompts the agent deliberately
   raised. Changing it is a dialect-table edit (5.5.2).
2. **Full auto stays the default in M1a builds**, although its safety nets do not exist yet (checkpoints in M2,
   isolation in M4); claude-acp then runs `bypassPermissions` in any real folder. The autonomy picker says "Full auto:
   no checkpoints until M2". Alternative: Auto-edit as the default for new chats until M2. Changing it is one constant
   in the session manager. Kept: the owner chose Full auto as the default, the picker states the missing safety net,
   and every real-agent run uses a scratch folder.
3. **The adapters download from npm once per version** (claude-agent-acp 0.95.0, codex-acp 2.2.2), installed with
   `npm ci` from the lockfile committed for that version, so every transitive version and integrity hash is pinned
   in M1a instead of M1e (section 4); creating the chat after the download note is the consent.
4. **The paid verification run** (10.3) is approved: 18 short prompts on the cheapest models, after every free probe
   passes; Hermes on its configured model unless probe 4 shows a model switch stays inside the session; the Hermes
   probes create sessions in the owner's Hermes history, which the report states.
5. **Agents start on demand**, not when an old chat is opened (decision 10). Alternative: start on open, so the
   pickers are filled at once, at the cost of spawning agents and running the user's `SessionStart` hooks on every
   chat click. Changing it is one call in the actor's attach path.
6. **A stop holds the prompts queued before it** (decision 11) and a new prompt waits behind them. Alternative: continue with
   the queued prompts after a soft interrupt. Changing it is one branch in 6.5.3.
7. **Windows Hermes in M1a needs its gateway already running** (decision 19). Alternative: build the breakaway job now,
   as about 150 lines of `unsafe` Windows code; CI could run a test child but has no Hermes, so the real case waits
   for the owner's PC either way.

## Review decisions

Findings from the gaps review and the build review that this revision rejected, or solved differently than proposed.
Everything else was applied as proposed.

- Disk-full reads (gaps, major): solved without the in-memory backlog. While paused, reads go to a second channel and
  are served from saved data with an `unsaved_from` marker, instead of merging unsaved rows into pages; merging needed
  the backlog the build review called over-engineered.
- Second process on a profile (gaps, major): the second process fails clearly instead of opening read-only, which
  would add a mode check to every write path for a case nobody needs.
- Hermes work finished while auracode was away (gaps, major): partly. A notice plus the latest reply; the gateway's
  intermediate tool steps are not rebuilt, because the replay has no stable alignment with stored turns.
- First adapter install killed midway (gaps, major): Esc may still kill it. The install is marked only when complete
  and an unmarked folder is cleaned at the next start, so no separate npm cache and no Esc warning are needed.
- Windows npm shims (gaps, major): user binaries must be `.exe` in M1a; resolving shims to their targets waits for
  M1e's catalog work.
- Sweep and pid reuse (gaps, minor): a heartbeat time window instead of "leader or its live descendants only", which
  would leave every orphaned group member of a crashed session behind.
- `JobObjectBreakawayOk` (both, minor): deferred to M1e, one of the options both reviews offered.
- AppImage variables (gaps, minor): recorded for M5; no AppImage exists yet.
- App Nap assertion while a turn runs (gaps, minor): deferred to M2b's keep-awake work; App Nap can slow an agent in a
  hidden window until then, without losing anything.
- Windows timers across sleep (gaps, minor): only macOS and Linux are stated; Windows is checked in the manual pass.
- Permission requests before `Ready` (gaps, minor): forwarded untagged. Holding them would deadlock Hermes'
  guarded-model confirmation, which blocks `session/set_model`; answering `cancelled` would silently decline it.
- `ptable/windows.rs` needing `unsafe` (gaps, minor): moot; M1a has no Windows process table.
- Confirm once before the first adapter start (build, minor): the New chat screen states the download instead, and
  creating the chat is the consent; a separate dialog adds a click without adding information.
- `#[serde(other)]` widening TypeScript (build, minor): a hand-written lenient `Deserialize` instead of dropping
  forward compatibility for those enums.
- Runtime context for constructors (build, minor): explicit `tokio::runtime::Handle` parameters instead of
  `block_on` in `setup`.
- P0 size (build, major): one P0 budgeted at 2 to 3 days with its stubs listed, instead of P0a and P0b, to keep a
  single merge point.

---

*End of spec.*
