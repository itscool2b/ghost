import type { BackgroundTask, GhostEvent } from "../bindings";
import { backgroundTask, ev } from "../lib/chat/testing";

/**
 * `?preview=tasks`: a running turn that started background work, as Claude Code reports it. A
 * running Explore subagent with its steps (and a command it started that failed), a backgrounded
 * command, and a finished agent with its report. `tasks-many` adds two more, so the strip collapses
 * them into one pill. Synthetic words only; no agent runs.
 */

const now = Date.now();
const ago = (seconds: number) => now - seconds * 1000;

const AUDIT_REPORT = `## Composer layout

The composer holds its shape from 960 to 1440 pixels. Two things need attention.

- **Account chip.** At 960 pixels it takes the model pill's room first; give way to the model instead.
- **Queue rows.** A long queued message wraps to three lines under 720 pixels; clamp it to one.

\`\`\`css
.chat-account-chip {
  flex-shrink: 4;
}
\`\`\`

Everything else, focus order included, matches the design.`;

/** Each task's final record, as the live session's snapshot lists them. */
export function taskPreviewRecords(many: boolean): BackgroundTask[] {
  const records = [
    backgroundTask("explore", {
      title: "Map account switching code",
      agent_type: "Explore",
      tool_call_id: "toolu-explore",
      activity: "Reading crates/ghost-core/src/session/agent_accounts.rs",
      tokens: 48_210,
      tool_uses: 7,
      started_at: ago(72),
      prompt:
        "Find where the desktop switches a chat's account.\n\n- Start from `docs/agent-accounts.md`.\n- Follow the switch into the Claude driver.\n- Report the files and functions involved, briefly.",
    }),
    backgroundTask("account-tests", {
      kind: "command",
      title: "Run the account tests",
      parent_task_id: "explore",
      status: "failed",
      started_at: ago(48),
      ended_at: ago(31),
      summary: 'Background command "Run the account tests" failed with exit code 1',
    }),
    backgroundTask("browser", {
      kind: "command",
      title: "Run website browser tests",
      tool_call_id: "toolu-browser",
      started_at: ago(41),
    }),
    backgroundTask("audit", {
      title: "Audit the composer layout",
      agent_type: "general-purpose",
      tool_call_id: "toolu-audit",
      status: "completed",
      started_at: ago(190),
      ended_at: ago(18),
      duration_ms: 172_000,
      tokens: 91_400,
      tool_uses: 23,
      prompt: "Check the composer at 960 and 1440 pixels in both appearances.",
      summary: AUDIT_REPORT,
    }),
  ];
  if (many)
    records.push(
      backgroundTask("lint", {
        kind: "command",
        title: "Lint the desktop sources",
        status: "completed",
        started_at: ago(130),
        ended_at: ago(96),
        summary: 'Background command "Lint the desktop sources" completed (exit code 0)',
      }),
      backgroundTask("docs", {
        title: "Check the protocol notes",
        agent_type: "Explore",
        status: "stopped",
        started_at: ago(150),
        ended_at: ago(60),
        tool_uses: 4,
      }),
    );
  return records;
}

/** The turn that started them, then each task's record and steps. */
export function taskPreviewEvents(many: boolean): GhostEvent[] {
  const records = taskPreviewRecords(many);
  const record = (id: string) => records.find((task) => task.task_id === id) as BackgroundTask;
  const started = (id: string): GhostEvent => ({
    type: "task_updated",
    ...record(id),
    status: "running",
    ended_at: null,
    summary: null,
    activity: null,
  });
  const ended = (id: string): GhostEvent => ({ type: "task_updated", ...record(id), prompt: null });
  return [
    ev.turnStarted("next"),
    ev.user("u2", "Find where accounts switch, audit the composer, and run the browser suite."),
    ev.toolCall("toolu-audit", {
      kind: "other",
      title: "Audit the composer layout",
      tool_name: "Agent",
      status: "completed",
      content: [{ type: "text", text: "Async agent launched.", truncated: null }],
    }),
    started("audit"),
    ev.taskStep("audit", "audit-1", { title: "Read Composer.tsx" }),
    ev.taskStep("audit", "audit-2", {
      kind: "text",
      title: "",
      tool_kind: null,
      status: null,
      text: "The composer row is a flex line; the account chip shrinks after the model pill.",
    }),
    ev.taskStep("audit", "audit-3", {
      title: "node scripts/check-layout.mjs --modes settled",
      tool_kind: "execute",
    }),
    ev.toolCall("toolu-explore", {
      kind: "other",
      title: "Map account switching code",
      tool_name: "Agent",
      status: "completed",
      content: [{ type: "text", text: "Async agent launched.", truncated: null }],
    }),
    started("explore"),
    ev.taskStep("explore", "explore-1", {
      kind: "text",
      title: "",
      tool_kind: null,
      status: null,
      text: "I'll start with the account docs, then follow the switch into the driver.",
    }),
    ev.taskStep("explore", "explore-2", { title: "Read agent-accounts.md" }),
    ev.taskStep("explore", "explore-3", {
      title: "Search for switch_account",
      tool_kind: "search",
    }),
    ev.taskStep("explore", "explore-4", { title: "Find **/accounts*.rs", tool_kind: "search" }),
    ev.taskStep("explore", "explore-5", { title: "Read session.rs" }),
    ev.taskStep("explore", "explore-6", {
      kind: "text",
      title: "",
      tool_kind: null,
      status: null,
      text: "The switch goes through the session actor: `set_account` restarts the agent on the **chosen account** and keeps the queue held.",
    }),
    ev.taskStep("explore", "explore-7", {
      title: "pnpm vitest run accounts",
      tool_kind: "execute",
      status: "failed",
    }),
    started("account-tests"),
    ended("account-tests"),
    ev.taskStep("explore", "explore-8", { title: "Read agent_accounts.rs", status: "in_progress" }),
    ev.task("explore", { ...record("explore"), prompt: null }),
    ev.toolCall("toolu-browser", {
      kind: "execute",
      title: "Run website browser tests",
      tool_name: "Bash",
      status: "completed",
      raw_input: {
        command: "corepack pnpm --filter @ghost/web test:browser",
        description: "Run website browser tests",
        run_in_background: true,
      },
      content: [
        {
          type: "terminal",
          output: "Command running in background with ID: browser.",
          exit_code: null,
          truncated: null,
        },
      ],
    }),
    started("browser"),
    ended("audit"),
    ...(many
      ? [started("lint"), ended("lint"), started("docs"), ended("docs")]
      : ([] as GhostEvent[])),
  ];
}
