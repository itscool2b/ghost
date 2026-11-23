# Bots that manage a whole coding task through o7, and chats that are ready on click

The owner's plan of 23 November 2026, from two rounds of research and their own inputs.

## Status

| Phase | State |
|---|---|
| A: a chat is ready when you click into it | Merged, #180 (23 November 2026). Checked on the debug build with the scripted agent and with Claude Code (no prompt sent, no usage spent). A long plan also shows as the plan card again. |
| 0: the link correct and measurable | Merged, #181. What the live documentation changed: OpenClaw's `--timeout` flag exists and Hermes' tool timeout defaults to 300 seconds, so only Grok Bot's steps were wrong. It also fixed a `send` that could read a chat before it moved. |
| 1: the loop | First part merged with #182: safer waits (45 by default, 200 at most), the `inbox` tool, and results that say what ran and what still fails, as a third tool set. The call-back followed in #183: a link names its assistant's webhook, and o7 tells it when a chat it acted on finishes, fails or asks. A reading's branch, uncommitted files and usage followed in #190; a pull request link is left out (o7 keeps none for a branch). |
| 2: managing a whole task | Merged with #190: `task` makes a chat on its own branch, briefs it, and when the work is done has a second agent review it; the bot is told once. The idle stop needs no change: an agent stopped between turns resumes on the next message. |
| 3: approvals and scope | Merged with #187: read-only links, approving an agent's permission requests and turning on full access as separate rights off by default, renewing a link's address, and expiry after 90 days unused. Approval from o7's own notification is not separate: the person answers in the chat or on the phone page, which already works. |
| 5: stay reachable | Merged with #191: opening at sign-in in the background (Windows' Run key, a macOS launch agent, a Linux autostart entry), keeping the computer awake while a phone or a live bot link can reach it, and a bot told plainly that the computer is away, since when, and that nothing was done. The macOS launch agent is untested: no Mac here. |
| 6: pictures | Merged with #192: `look` (full control, tool set 5) shows a chat's browser tab, or its own emulator, as a JPEG of at most 1600 pixels; never a phone. Not from screenshots the agent already took (history keeps none), so the picture is taken when asked. |
| 7: later work and outside signals | First part merged with #193: `task` with `when: "tonight"` queues work that o7 starts between 1 and 6 in the morning on the computer's clock, one at a time and three a night, with `inbox` listing the queue and `cancel` taking one off. A task's pull request followed in #194: its bot is told when checks fail or pass, of new reviews and comments (words only from the repository's own people), and of the merge; a reading names it. Automations followed in #195: an address that starts a task in one project with the person's instruction, what a service sends handed over as data, three at once and twenty a day. Phase 7 is done. |
| 4, 8 | Not started. |

Still true of every phase: no real Grok Bot or Muse has been run against the link.

## Context

Two things, one plan.

**1. The bot link has no value yet.** The owner wants a personal assistant (Grok Bot first, then Hermes, OpenClaw, Muse) to take a task and manage all of it through o7, on the owner's own subscriptions, with little token overhead. Almost nobody uses the link today. Two research rounds (kept outside the repository) and the owner's inputs say why:

- **One direction only.** A bot must hold a call open or poll; o7 never calls back. That is where the tokens go: an estimated 10 to 20 bot turns per ten-minute coding turn, against 2 with a call-back.
- **Nothing worth reading comes back.** A bot gets text about chats. o7 already computes what ran, the first error, diff stats, screenshots and usage state, and gives a bot none of it.
- **The bot rarely picks the tool, and setup names no use case.** Setup is seven to nine steps; the pitch promises a call-back that does not exist.
- **Defects.** Grok Bot's setup text points at the Grok chat app; waits exceed client limits (OpenClaw 60 s, Claude 240 s); `memory` is described as 16 KiB but the code allows 4; errors name tools the bot was never given; any link can answer permission prompts and a full link can set `full_auto`.
- **Owner's inputs (2026-11-23).** Grok Bot does not refuse the connector. The job people want is the bot managing the whole task, not typing prompts by proxy. The result should be "genius": whatever is best throughout.
- **Not wanted by anyone in the evidence:** coding live through a bot.

**2. A chat is not ready when you click into it.** After a restart the agent only starts once a message is sent, so Plan mode cannot be chosen first. Cause found in the code: while stopped, an access or model change is kept for the next start, but a mode change is refused ("Change the mode once the agent is running and idle.", `crates/o7-core/src/session/actor/controls.rs:321-325`), and nothing starts the agent on open.

Nothing in the research was run against a real assistant. Phase 0 does that.

## The principle: o7 does the managing, the bot does the deciding

The bot is the expensive brain that holds the owner's context; every step it drives costs a full pass over that context. So o7 carries the procedure and hands the bot only decisions.

- One call starts a whole task. o7 resolves the project from recent activity, picks the agent with usage headroom, gives the chat its own worktree, and runs plan, build, checks and a second agent's review as one job.
- The bot hears back only at decision points: a question, an approval above its policy, a failed check o7 could not fix, done with proof.
- Every message to the bot is one screen: verdict first, proof next, the one thing to decide last. Detail stays in o7 behind an id.
- What code can check (tests passed, diff stat, CI state, PR opened) o7 checks itself and never asks the bot to read.
- The bot's knowledge of the owner flows into the brief; the outcome flows back as one line for it to remember.
- The model that reads agent output never holds the approval gate.

## The flow being built

1. Owner to bot, by text or voice: "ship the login fix in the web repo".
2. Bot makes one `task` call (goal, optional project). o7 answers within a safe wait: the finished result, or "started, you will be told".
3. Bot ends its turn. o7 works on the owner's own CLI logins.
4. o7's outbox posts one signed event at each decision point.
5. Bot wakes, reads one result (status, one-line brief, commands and outcome, first error, diff stat, branch or PR link, usage state, a screenshot when the change is visual), and decides: answer, follow up, accept, ask for the PR.
6. Bot tells the owner once. Prompts above the link's policy reach the owner through o7's own notification.

## Phases (one PR each, branch `codex/<topic>` from `origin/main`)

### Phase A, first and separate: a chat is ready when you click into it
- **Keep the mode for the start.** Extend `keep_control_for_start` (`crates/o7-core/src/session/actor/controls.rs:316`) to hold a workflow/mode choice, Plan included, as access already is and as options are (`set_option_waiting`, `actor.rs`); apply it in the start's setup before the first prompt. The menu shows the kept choice at once.
- **Start on open.** When a stopped chat stays in front for about a second, the view calls `store.start(false)` (resume, not fresh): `apps/desktop/src/components/chat/ChatView.tsx` near 623, `apps/desktop/src/lib/chat/chatStore.ts:1132`, `SessionManager::start` (`crates/o7-core/src/session/mod.rs:1187`). Skip chats that failed, need sign-in, are shown in a terminal or lack their folder. Existing limits bound it: 16 live agents, the longest idle is stopped to make room, 10 minute idle stop.
- **Check first:** in the protocol logs, that a resume with no prompt spends no usage for Claude, Codex and one ACP agent. An agent where it does gets the first part only.
- Tests: actor tests for a mode kept across a start, and that leaving Plan still needs approval; a view test for the dwell and the skips; dev build: restart, open a chat, pick Plan, send, confirm the turn ran in Plan. Changelog line.

### Phase 0: make today's link correct and measurable
- Setup text: Grok Bot adds the connector in its own app (Connect apps, Add a custom connector, Remote HTTPS). `apps/desktop/src/components/settings/RemotePanel.tsx` (about 330-380), onboarding copy, `docs/remote-sessions.md`.
- Check OpenClaw's and Hermes' timeout settings against their live documentation (done: both snippets were right).
- `apps/relay/src/tools.json` memory size; legacy tool names in errors (`crates/o7-remote/src/bot.rs:320, 660`, `apps/desktop/src-tauri/src/remote.rs:484, 587, 647, 684`).
- `readOnlyHint`/`destructiveHint` on every tool; honour cancel so an abandoned `wait` frees its relay slot (`apps/relay/src/index.ts:648-713`).
- Per-link counters on the computer only (calls, turns started, waits that ran out, last error), beside `last_used_at` in `crates/o7-remote/src/registry.rs`, shown in Settings. No network telemetry.
- "Test the link": the dialog confirms the first call that arrives.
- Pitch reworded until Phase 1 makes the call-back true.

### Phase 1: the loop
- **Safe waits:** default about 45 s, cap about 200 s, "still working" as a normal result (`crates/o7-remote/src/bot/lean.rs:31-37`).
- **Outbox:** the desktop, not the relay, posts a fixed-shape, signed, deduplicated event on finished, needs answer, needs approval, failed. Reuse the detector behind desktop notices (`apps/desktop/src-tauri/src/native_shell/mod.rs:136-163`, `native_shell/model.rs:15-27`) and `Backend::watch` (`crates/o7-remote/src/backend.rs:155-223`). Adapters in order: Grok Bot routine webhook, Hermes `deliver_only` route or `hermes send`, OpenClaw `/hooks/agent`, generic signed webhook. Keys per link in the OS secure store, never on the relay or in logs. Agent text enters an event only as a capped, delimited field.
- **`inbox` tool:** everything that needs the bot or the owner, across chats, in one call.
- **Proof-bearing result** for `send`, `wait`, `inbox`, `read`: status word, one-line brief, commands with outcome, last failing command and first error (`work()` and `CommandRun`, `crates/o7-core/src/conversation.rs:60-99, 245-293`), changed files and diff stat without checkpoints (`crates/o7-protocol/src/git.rs:17-25, 238-253`), branch or PR link, usage-limit state (`crates/o7-protocol/src/agents.rs:185-200`). "Waiting" splits into question, permission, background work.
- Tool list stays under the relay test's size ceiling; older tool sets keep answering (bump the link's `v`).

### Phase 2: managing a whole task
- **`task` tool (the conductor):** one call with a goal. o7 runs the procedure in the principle above, built on Plan and build roles and the coordinator, and returns a task id; the outbox reports per task. `send` stays for direct steering.
- **Worktree for bot-started chats:** today they share the folder (`apps/desktop/src-tauri/src/remote.rs:880-897`). Reuse `crates/o7-core/src/session/worktree_chat.rs` and the publish flow `crates/o7-core/src/gitworkspaces/publish.rs`, so a bot can ask for a PR.
- **Second-agent review:** resolved by o7 from a chat id; the bot never carries the diff.
- **A waiting bot keeps its agent:** a pending outbox subscription counts against the idle stop (`crates/o7-core/src/session/mod.rs:218-227`).

### Phase 3: approvals and scope
- Per-link policy enforced in Rust on the computer: none, questions only, listed classes. `full_auto` becomes a right the link is created with. A read-only level.
- Approve or deny from o7's own notification, bound to the request id; first answer wins.
- Link expiry when idle and rotation; the bearer-header mode the relay already accepts (`apps/relay/src/index.ts:597-604`) is documented and used for Muse.

### Phase 4: the front door
- One `SKILL.md` in this repository: when to use o7, the loop, five to ten recipes (fix and prove, second-agent review, what needs me, queue for tonight, bug report to task). Its description is tested for trigger rate per assistant.
- Settings produces one install line per assistant; setup goes toward one paste.
- New pitch, example prompts, tour stop (`apps/desktop/src/lib/tour/stops.ts`, `data-tour`), website copy, docs. Listings on the review-free registries.

### Phase 5: stay reachable
- Opt-in start at login; opt-in hold on idle sleep while a bot link exists (`apps/desktop/src-tauri/src/native_shell/power.rs:11-20`); a plain "computer asleep" answer.

### Phase 6: pictures
- Optional image in `bot::Answer` with relay passthrough, full control only, from screenshots the chat's agent already took (`crates/o7-core/src/browser_service/helper.rs:62-72`, `mobile_service/helper.rs:65-75`). Check the message size limit first.

### Phase 7: later work and outside signals
- "Run when idle or tonight": fixed budget per night, one worktree per job, never merges, one morning list (per-chat queue, `crates/o7-core/src/session/mod.rs:1098-1162`).
- "CI failed on a branch a chat made" and "review comment on its PR" through the outbox. Pre-approved automations: fixed instruction, fixed permission profile, trigger text as labelled untrusted data.

### Phase 8: optional doors
- Local `o7 send|wait|inbox|answer` over a loopback grant for Hermes and OpenClaw on the same machine. OAuth 2.1 for directory listings. MCP Events and Tasks last.

**Not building:** live pair-coding through a bot, unattended merge or deploy, a bot action that switches accounts at a limit, A2A, a single do-everything tool, a voice stack in o7.

## Rules for every phase
- Security read on each relay or remote change; test the refusals; state any tradeoff taken.
- App PRs add a changelog line under the US Eastern merge day; anything people see adds its tour stop; `docs/remote-sessions.md` stays current.
- Relay: preview, live test, production by hand, then merge. Ask before any CI dispatch.
- Each phase is tested on the debug build before it merges.
- Outside our control: o7 drives Claude through `claude -p`, which Anthropic planned to reprice in May 2026 (paused).

## Verification
- Rust tests in WSL (no game running first, shut down after); relay unit tests with the tool-size assertion.
- `node apps/relay/tests/e2e.mjs --host <main checkout>\target\debug\examples\demo_host.exe` and `apps/relay/tests/native.mjs <debug exe> --prompt` against the preview relay, every link level, plus refusals (wrong workspace, read-only link sending, approval above policy, expired link).
- Outbox: a local stand-in receiver checks signature, dedup, shape and one event per state change; then local Hermes and OpenClaw installs.
- Measured before and after Phase 1: tool calls per coding turn, bytes per result, timeouts.
- Dev build by hand for Phase A, Settings, setup, tour and notifications, light and dark.
- A real Grok Bot and a real Muse need an account of each; until one is run, both stay marked unverified in the docs.
