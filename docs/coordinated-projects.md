# Coordinated projects

Status: **an experiment**, off until Settings > Experiments > Coordinated projects is turned on (owner, 19 November 2026: "replicating cursor projects ... add it as an experimental feature so in settings u can see experiment features if u enable the whole UI and functionality should work"). Turning it off stops everything described here at once and keeps what the projects hold.

A coordinated project is one of the sidebar's projects with a coordinator: a chat that plans the project's work, hands it to worker chats, and keeps what must outlast any one conversation in the project instead of in a context window.

## What it is modelled on

Cursor's Projects (beta, 10 September 2026; multiplayer, 10 November 2026). From Cursor's own posts and docs:

- You oversee a Project by chatting with its coordinator. "The coordinator doesn't write code itself but directs other agents that do", so "it is never blocked and is always responsive to direction."
- Subagents start with a clean context: nothing reaches the next agent unless a file carries it.
- "Each Project maintains a set of files that sync across every cloud and local machine its agents use. Agents add research and artifacts, along with what they learn about the codebase and how you prefer work to be done."
- A task board shows what each agent is working on and the pull request behind each task.
- The coordinator can "watch a Slack channel, run on a schedule, or follow all your PRs".

That is how it answers context rot: the coordinator's context holds plans and reports, never the work; workers are disposable and each starts clean; and what was learned lives in files, not in a conversation that is later compacted.

The one detailed first-hand account (a three-day run in September 2026) found where that falls short: the project's notes file was rewritten 111 times and read once, decisions the person had made dropped out of it, and none of 32 worker assignments pointed at it. Ghost keeps Cursor's design and closes those gaps.

## How Ghost does it

| Cursor | Ghost |
|---|---|
| Coordinator chat that delegates and does not write code | The same: an ordinary chat with project tools, told to plan and delegate. This is an instruction, not a lock: see Limits. |
| Workers with a clean context | A new chat per worker. Ghost itself primes it with its part, the project as it stands (brief, decisions, the list of Context files, open tasks) and the coordinator's assignment, so priming never depends on the coordinator remembering to. |
| Shared files synced to every machine | Context files in the profile's database, read and written through tools. One device, so there is nothing to sync. Every write keeps the text it replaced (20 revisions a file), a rewrite names the revision it read and is refused when the file changed since, and an append never conflicts. |
| Notes that hold decisions | A decision log of its own. Entries are only ever added. The person's are shown to every agent as settled; the coordinator's are shown apart, as its account. The person can take an entry back: it stays in the log and agents stop being shown it. |
| Task board | To do, Doing, Review, Done. Tasks are numbered (`T-12`), carry notes, tags, the worker and a link to the pull request. Drag between columns, or use the task's dialog. |
| Reports from workers | When a worker's turn ends, however it ended, its report (what it filed with `project_report`, else its last message) is sent to the coordinator as an event. A report stays queued until its send succeeded, so quitting or a full queue loses none. |
| Schedules | Routines: a prompt the coordinator is sent every 5 minutes to every week. Watching pull requests or CI is a routine whose prompt says to check them, as Cursor's own timed subscription does. |
| Per-role model | A worker runs on the agent and model the coordinator names for it, else the coordinator chat's Build with (its model menu's Plan and build), else the coordinator's agent; the coordinator starts on the agent chosen for it, else the project's ([model-roles.md](model-roles.md)). |
| Cloud machines | None. Everything runs on this computer, in the agents already installed, and only while Ghost is open. |
| Slack, Linear, Sentry and webhook triggers | None. |
| Teammates, shared boards, agent-to-agent chat between people | None: one person, one device. |
| Thousands of subagents | At most 4 workers work at once (`limits::LIVE_WORKERS`); a project keeps up to 1,000 worker records. |

The page for a project opens from the board button beside its name in the sidebar: Board, Agents, Context, Decisions and Routines, with Pause and Open coordinator at the top. Everything on it can be changed by hand, and each change an agent makes shows as it lands. In a chat, a prompt Ghost sent for the project (the brief, an assignment, a report, a routine) reads as one quiet line that opens what was sent, not as the person's message.

## The tools

A coordinator's or a worker's agent gets one more MCP server, `ghost_project`, started the way the browser's and project memory's are: the app's own executable as a stdio helper (`--ghost-project-helper --mcp`) that reaches the running app over a loopback listener. Its instructions are part of the server's `initialize` reply, which an agent's client keeps in its system prompt, so they outlast a compaction of the conversation.

| Tool | Coordinator | Worker |
|---|---|---|
| `project_overview`, `project_context_read`, `project_context_append`, `project_context_write` | yes | yes |
| `project_task_update` | any task | its own task: column, notes and link |
| `project_decision`, `project_task_create` | yes | no |
| `project_worker_start`, `project_worker_message`, `project_worker_read`, `project_worker_stop` | yes | no |
| `project_routine_set`, `project_routine_remove` | yes | no |
| `project_report` | no | yes |

## Who may do what

- **Which chat is asking** comes from the grant, never from the call. A grant is made for one chat and one session, with a random token (two UUIDv7 values, compared in constant time) passed to the helper in its environment and never written to any configuration or log. Stopping, failing or replacing the session drops the grant and its listener. A call's arguments cannot name a chat, a project or a part; no tool takes one.
- **Which part the chat has** is read from the database on every call, so a chat that stopped being a coordinator or worker loses its tools at once. The tool list a helper shows is what an agent sees, not what it is allowed: the same check runs on each call.
- **The experiment's switch** is read on every call and every tick. Off: no chat is given the tools, running ones are refused, no report is passed on and no routine fires.
- **Only the person** pauses, resumes, changes the brief or takes a decision back.
- **A worker** cannot start, message or stop workers, cannot create tasks or routines, and cannot record decisions. A worker reads files, pages and tool output that may carry hostile text; what it writes to the decision log would be shown to every other agent as settled, so it says its choices in its report and the coordinator decides what to record.
- **A worker starts** in one of its own project's folders (the one named, else the coordinator's), in its own Git worktree and branch unless the coordinator asks otherwise, with the access level Settings gives every new chat. The coordinator cannot raise it. When a worktree cannot be made the coordinator is told why; Ghost never falls back to the shared folder by itself.
- **Runaway work** is bounded: at most 4 workers at once, and a project whose coordinator was sent 60 reports and routines within an hour is paused by Ghost, which says so on the page. Routines wait while their coordinator is working, so prompts never pile up.
- **Sizes** are bounded for the person and agents alike (`ghost_protocol::coordinator::limits`): a Context file 64 KiB, 200 files a project, a decision 2 KiB, 2,000 decisions, 3,000 tasks, 20 routines, a report 16 KiB, an assignment 24 KiB. A Context path is a name (letters, digits, dots, dashes, underscores, at most four folders deep, ending in `.md`), never a file system path, so nothing here can reach a folder.
- **Remote devices and bots** get nothing new. Coordinator and worker chats are ordinary chats, so a paired device or bot sees and can message them as it can any chat in the profile; the project tools, the page and its commands are not offered over the relay.

## Limits, said plainly

- **Agent-written text reaches other agents.** Context files and reports are written by agents and read by agents. Every place they are shown says they are accounts to check and never instructions from the person, and workers cannot write the decision log, but an agent that reads a poisoned Context file can still be misled by it. Every write is attributed and kept, and the person can read and restore any file.
- **"The coordinator does not edit files" is an instruction.** It runs with the same access as any chat; nothing stops it from editing if it ignores its instructions. Cursor's is the same.
- **A worker in a fresh worktree has no installed dependencies.** It is told so and installs what it needs; a folder's saved worktree setup is not run for workers.
- **Cost.** Each worker is a full agent session, billed as any chat's turns. Four parallel workers use about four times the tokens of one.
- **Only while Ghost is open.** A routine that came due while the app was closed runs once at the next start.
- **Agents without MCP.** The tools reach agents that take Ghost's own servers in Chat (as the browser's do). A chat shown as Terminal, a headless connection or an agent on another host gets no project tools.

## Where the code is

- `crates/ghost-protocol/src/coordinator.rs`: the types and limits.
- `crates/ghost-core/migrations/0019_coordinator.sql`, `crates/ghost-core/src/chatstore/coordinator.rs`: storage, and who may change what.
- `crates/ghost-core/src/coordinator_service.rs`: the grant, the helper, the tools and their instructions.
- `crates/ghost-core/src/session/coordinator.rs`: the engine (starting workers, passing reports on, routines, the hourly limit).
- `apps/desktop/src-tauri/src/coordinator_commands.rs`: the page's commands.
- `apps/desktop/src/components/project/`, `apps/desktop/src/lib/coordinator.ts`: the page.

## Verification

- `cargo test -p ghost-core coordinator`: storage rules and every refusal; the grant's token, scope and switch; and an end-to-end run over scripted agents (a coordinator is started, starts a worker with its tool, the worker is primed with the person's decision, and its report reaches the coordinator).
- `node apps/desktop/scripts/check-coordinator.mjs`: the sidebar, the page and events in a chat on a synthetic project, in Chromium and WebKit, in both appearances, with an accessibility audit.
