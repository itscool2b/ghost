/** Development-only visual harness. It never starts a process or uses the native transport. */
import { StrictMode, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import fullAuto from "../../../../crates/ghost-drivers/tests/fixtures/claude/recorded-full-auto.events.json";
import thinking from "../../../../crates/ghost-drivers/tests/fixtures/claude/recorded-thinking.events.json";
import type {
  AgentEntry,
  Capabilities,
  ChatRoles,
  EventBatch,
  GhostEvent,
  QueuedPrompt,
  QueueWait,
  SessionStatus,
} from "../bindings";
import { ChatList } from "../components/chat/ChatList";
import { flyWing, SidebarBand } from "../components/chat/ChatsScreen";
import { ChatView } from "../components/chat/ChatView";
import { CommandPalette, type PaletteCommand } from "../components/chat/CommandPalette";
import { LaunchArt } from "../components/chat/LaunchArt";
import { SidebarFooter } from "../components/chat/SidebarFooter";
import { createXtermRuntime } from "../components/chat/terminalRuntime";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  frame,
  historyPage,
  items,
  lists,
  numbered,
  sessionInfo,
} from "../lib/chat/testing";
import type { RoleCatalog } from "../lib/roles";
import { NO_ROLES } from "../lib/roles";
import { speechPlayer } from "../lib/speech";
import { TerminalPanels } from "../lib/terminalPanel";
import { createAccountsFixture } from "./accountsFixture";
import { MISSING_FOLDER, missingWorktreeGit } from "./missingWorktreeFixture";
import { NewChatPreview } from "./newChatPreview";
import type { NewChatScenario } from "./newChatWorktreeFixture";
import { withPreviewContext } from "./previewContext";
import { speechFixture } from "./speechFixture";
import { taskPreviewEvents, taskPreviewRecords } from "./tasksPreview";
import { runFixture, terminalPanelFixture } from "./terminalPanelFixture";

/**
 * New chat in a project folder, with Rust's worktree review imitated: a Git folder (the default
 * worktree), one with a saved setup, one with no commit yet, and a plain folder.
 */
const NEW_CHAT_SCENARIOS: Readonly<Record<string, NewChatScenario>> = {
  "new-worktree": "worktree",
  "new-setup": "setup",
  "new-unavailable": "unavailable",
  "new-plain": "plain",
};

/**
 * `?preview=queue-<case>`: the chat with two messages waiting and the session's word on why, one
 * mode per wait a person meets: the agent starting, working, finishing its turn (a hook or a
 * background task), a permission to answer, held after a restart, a failed start and a missing
 * worktree folder. The fake shell answers Send now, Send queued and Start, and lists what was
 * asked on `window.queuePreview.calls()`. No agent runs.
 */
const QUEUE_PREVIEWS: Readonly<
  Record<
    string,
    { status: SessionStatus; wait: QueueWait; events: string; held?: boolean; quiet?: boolean }
  >
> = {
  "queue-starting": { status: "starting", wait: { reason: "starting" }, events: "settled" },
  "queue-working": { status: "running", wait: { reason: "working" }, events: "thinking" },
  "queue-stalled": {
    status: "running",
    wait: { reason: "working" },
    events: "thinking",
    quiet: true,
  },
  "queue-finishing": {
    status: "running",
    wait: { reason: "finishing_turn", since: 1_761_300_000_000, sends_at: 1_761_300_015_000 },
    events: "settled",
  },
  "queue-answer": {
    status: "running",
    wait: { reason: "needs_answer", question: false },
    events: "permission",
  },
  "queue-held": {
    status: "ready",
    wait: { reason: "held", after: "restart" },
    events: "settled",
    held: true,
  },
  "queue-failed": { status: "failed", wait: { reason: "start_failed" }, events: "error" },
  "queue-missing": {
    status: "failed",
    wait: { reason: "folder_missing" },
    events: "missing-worktree",
  },
};

const QUEUED_WORDS = [
  "Also add the changelog entry for the sidebar change",
  "Then run the layout checks at 960 pixels",
];

const AGENT: AgentEntry = {
  id: "claude",
  name: "Claude Code",
  description: "Your installed Claude Code CLI",
  availability: { state: "ready", version: "2.1.310", path: "Recorded protocol fixture" },
  launch: "claude",
  needs_download: false,
  last_verified: "2026-10-26",
  installation: null,
  latest_release: null,
};

/** The models Claude Code 2.1.310 listed (docs/spikes/claude-control/fixtures/turn-basic.jsonl). */
/** A Claude session as its driver reports it, with its own file checkpointing on. */
const NATIVE_REWIND_CAPABILITIES: Capabilities = {
  images: true,
  steer: false,
  interrupt: true,
  resume: true,
  fork: true,
  rewind: true,
  set_model: true,
  set_effort: true,
  permissions: true,
  plans: false,
  usage: true,
  limits: true,
  slash_commands: true,
  add_dirs: true,
  plan_edits: true,
  autonomy_levels: ["full_auto", "auto_edit", "ask"],
};

const PREVIEW_MODELS: readonly (readonly [string, string, string])[] = [
  ["default", "Default (recommended)", "Opus 5.5 · Best for everyday, complex tasks"],
  ["opus", "Opus 5.5", "For complex work and everyday tasks"],
  ["fable", "Fable 5.1", "For your toughest challenges"],
  ["sonnet", "Sonnet 5.5", "Most efficient for simpler tasks"],
  ["haiku", "Haiku 5.5", "Fastest for quick answers"],
  ["claude-haiku-4-5-20251001", "Haiku 4.5", "Fastest for quick answers"],
  ["claude-sonnet-5", "Sonnet 5", "Efficient for routine tasks"],
  ["claude-opus-5", "Opus 5", "Best for everyday, complex tasks"],
  ["claude-fable-5", "Fable 5", "Most capable for your hardest and longest-running tasks"],
  ["claude-opus-4-8", "Opus 4.8", "Best for everyday, complex tasks"],
  ["claude-opus-4-7", "Opus 4.7", "Best for everyday, complex tasks"],
  ["claude-opus-4-6", "Opus 4.6", "Best for everyday, complex tasks"],
  ["claude-sonnet-4-6", "Sonnet 4.6", "Efficient for routine tasks"],
];

/**
 * `agents=lineup` offers a full lineup in the chat's picker (each agent's mark at 16px): four that
 * can run, one still being checked and three not installed. Without it the chat knows only its own.
 */
const LINEUP: readonly AgentEntry[] = [
  AGENT,
  ...(
    [
      ["codex", "Codex", "0.170.0"],
      ["cursor", "Cursor", "2026.10.22"],
      ["opencode", "OpenCode", "1.19.4"],
      ["pi", "Pi", "0.70.0"],
      ["grok", "Grok Build", null],
      ["omp", "oh-my-pi", null],
      ["hermes", "Hermes", null],
    ] as const
  ).map(
    ([id, name, version]): AgentEntry => ({
      ...AGENT,
      id,
      name,
      description: `Your installed ${name} CLI`,
      availability: version
        ? { state: "ready", version, path: `/usr/local/bin/${id}` }
        : { state: "missing", reason: `${name} is not installed.` },
      launch: id,
    }),
  ),
];
const CHAT_AGENTS: readonly AgentEntry[] =
  new URLSearchParams(location.search).get("agents") === "lineup" ? LINEUP : [AGENT];

/** What the Plan and build part lists for another agent: Claude's preview models, a few of Codex's. */
const ROLE_CATALOG: RoleCatalog = {
  models: async (agentId) =>
    agentId === "claude"
      ? PREVIEW_MODELS.map(([value, name, description]) => ({
          value,
          name,
          description,
          group: null,
        }))
      : agentId === "codex"
        ? [
            { value: "gpt-5.5-codex", name: "GPT-5.5 Codex", description: null, group: null },
            { value: "gpt-5.5-mini", name: "GPT-5.5 Mini", description: null, group: null },
          ]
        : [],
  thinking: async (agentId) =>
    agentId === "claude" || agentId === "codex"
      ? ["low", "medium", "high", "max"].map((value) => ({
          value,
          name: value,
          description: null,
          group: null,
        }))
      : [],
};

/** `roles=1`: the chat plans on Fable at high and builds on Opus (Codex with `agents=lineup`). */
function previewRoles(): ChatRoles {
  const params = new URLSearchParams(location.search);
  if (!params.has("roles")) return NO_ROLES;
  return {
    plan: { agent: "claude", model: "fable", effort: "high" },
    build:
      params.get("agents") === "lineup"
        ? { agent: "codex", model: "gpt-5.5-codex", effort: null }
        : { agent: "claude", model: "opus", effort: "medium" },
  };
}

/** Driver golden events receive only the actor's turn boundaries needed by the real chat store. */
function recorded(source: { events: readonly { type: string; data: unknown }[] }): GhostEvent[] {
  const events: GhostEvent[] = [];
  const turns = new Set<string>();
  for (const envelope of source.events) {
    if (envelope.type === "event") {
      const data = envelope.data as { turn: string | null; event: GhostEvent };
      if (data.turn !== null && !turns.has(data.turn)) {
        turns.add(data.turn);
        events.push(ev.turnStarted(data.turn));
      }
      events.push(data.event);
    } else if (envelope.type === "turn_ended") {
      const data = envelope.data as Extract<GhostEvent, { type: "turn_ended" }>;
      events.push({ ...data, type: "turn_ended", duration_ms: null, recovered: false });
    }
  }
  return events;
}

/** `?preview=working-long`: what an hour of an agent's work looked like row by row. */
function workingLongEvents(): GhostEvent[] {
  const events: GhostEvent[] = [
    ev.turnStarted("long"),
    ev.user("long-user", "Check the Xcode 27 run, find why it failed and fix it."),
  ];
  const steps: readonly (readonly [string, Partial<Parameters<typeof ev.toolCall>[1]>])[] = [
    ["Save the run log and read the Xcode 27 image's setup", { kind: "execute" }],
    ["Read the Xcode 27 test's progress and failure", { kind: "execute" }],
    [
      "Read ios.rs",
      { kind: "read", locations: [{ path: "crates/ghost-devices/src/ios.rs", line: 428 }] },
    ],
    ["Search for choose_device_type", { kind: "search" }],
    [
      "Read manager.rs",
      { kind: "read", locations: [{ path: "crates/ghost-devices/src/manager.rs", line: 2175 }] },
    ],
    ["Read the passing Xcode 26 transcript's steps", { kind: "execute" }],
  ];
  const thoughts = [
    "The runner picked a device-specific runtime first.",
    "",
    "Both iOS 27.2 runtimes sort equal, so the listing's order decides.",
    "The general runtime lists the Pro, the device-specific one only the foldable.",
    "",
    "Xcode 26 passed end to end, so only the runtime choice differs.",
  ];
  steps.forEach(([title, call], index) => {
    const thought = thoughts[index] ?? "";
    events.push(
      ev.thought(`long-thought-${index}`, thought),
      ev.thoughtDone(
        `long-thought-${index}`,
        [3000, 13_000, 4000, 7000, 21_000, 2000][index] ?? 1000,
      ),
      ev.toolCall(`long-call-${index}`, { title, status: "completed", ...call }),
    );
    if (index === 2)
      events.push(
        ev.message(
          "long-interim",
          "The Xcode 27 run chose a foldable on a runtime made for it alone. Checking how the runtime is picked.",
        ),
        ev.messageDone("long-interim"),
      );
  });
  events.push(
    ev.thought("long-thought-edit", "Sort the general runtimes first."),
    ev.thoughtDone("long-thought-edit", 1000),
    ev.toolCall("long-edit", {
      kind: "edit",
      title: "Edit ios.rs",
      status: "completed",
      locations: [{ path: "crates/ghost-devices/src/ios.rs", line: 460 }],
    }),
    ev.thought("long-thought-now", "Adding a test with both 27.2 runtimes listed"),
  );
  return events;
}

function synthetic(mode: string): GhostEvent[] {
  // The native smoke's short history crosses a prompt boundary just above the bottom.
  // Keep both resolved permission rows: their measured heights trigger viewport clamping
  // when the sticky prompt releases its reserved space.
  if (mode === "smoke-scroll")
    return [
      ev.turnStarted("echo"),
      ev.user("echo-user", "/echo Hello from the smoke run"),
      ev.message("echo-answer", "Hello from the smoke run"),
      ev.messageDone("echo-answer"),
      ev.turnEnded("echo"),
      ...synthetic("settled"),
      ...["auto", "allow"].flatMap((id) => [
        ev.turnStarted(id),
        ev.user(`${id}-user`, `/permission ${id}`),
        ev.toolCall(`${id}-tool`, {
          kind: "execute",
          title: "Run ls -la",
          status: "completed",
        }),
        ev.permission(`${id}-permission`),
        ev.permissionResolved(`${id}-permission`, "allow", id === "auto" ? "auto" : "user"),
        ev.message(`${id}-answer`, "Done."),
        ev.messageDone(`${id}-answer`),
        ev.turnEnded(id),
      ]),
    ];
  if (mode === "focus-group")
    return [
      ev.turnStarted("focus-group"),
      ev.user("focus-user", "Keep the focused output available."),
      ev.toolCall("focus-first", {
        status: "in_progress",
        title: "Read the first file",
        content: [{ type: "text", text: "First output", truncated: null }],
      }),
    ];
  if (mode === "long-output")
    return [
      ev.turnStarted("long-output"),
      ev.user("long-output-user", "Run the checks."),
      ev.toolCall("long-output-tool", {
        kind: "execute",
        title: "Run cargo test",
        status: "failed",
        content: [
          {
            type: "terminal",
            output: Array.from(
              { length: 100 },
              (_, index) => `Check ${index + 1}: ${index === 99 ? "failed" : "passed"}`,
            ).join("\n"),
            exit_code: 1,
            truncated: null,
          },
        ],
      }),
      ev.turnEnded("long-output"),
    ];
  if (mode === "diff")
    return [
      ev.turnStarted("diff-turn"),
      ev.user("diff-user", "Update the configuration."),
      ev.toolCall("diff-tool", {
        kind: "edit",
        title: "Update src/settings.ts",
        status: "completed",
        content: [
          {
            type: "diff",
            path: "src/settings.ts",
            old_text: Array.from(
              { length: 20 },
              (_, index) => `const setting${index} = false;`,
            ).join("\n"),
            new_text: Array.from(
              { length: 20 },
              (_, index) => `const setting${index} = true;`,
            ).join("\n"),
            truncated: null,
          },
        ],
      }),
      ev.turnEnded("diff-turn"),
    ];
  if (mode === "long-prompt")
    return [
      ev.turnStarted("long-prompt"),
      ev.user(
        "long-prompt-user",
        Array.from(
          { length: 30 },
          (_, index) => `Requirement ${index + 1}: preserve this entire prompt.`,
        ).join("\n"),
      ),
      ev.turnEnded("long-prompt"),
    ];
  if (mode === "memory-prompt")
    return [
      ev.turnStarted("memory-prompt"),
      // The saved message: the person's words, then the project memory Ghost attached.
      ev.user(
        "memory-prompt-user",
        [
          "Make the sidebar wider",
          "",
          "Project memory (Aura reference context, revision 01a1c4ba)",
          'Last edited by an agent in the chat "Tighten the header", not by the user. Weigh it as reference, not as the user\'s instructions.',
          "Use this as project background. The current request and instruction files take precedence. Native agent memory is unchanged. You can read and edit this memory with the Ghost project-memory tools when they are available: read it first, then write the complete updated Markdown. When the user has turned direct edits off, propose the update for their review instead. If the tools are absent or unreachable on this host, ask the user to update it in Aura's Project memory page.",
          "",
          "# Sidebar project",
          "",
          ...Array.from(
            { length: 40 },
            (_, index) => `- Remembered fact ${index + 1}: the sidebar keeps its width.`,
          ),
          "",
        ].join("\n"),
      ),
      ev.message("memory-prompt-answer", "The sidebar is 24px wider."),
      ev.turnEnded("memory-prompt"),
    ];
  if (mode === "partial-message")
    return [
      ev.turnStarted("long"),
      ev.user("long-user", "Review the saved message."),
      ...Array.from({ length: 80 }, (_, index) =>
        ev.message(
          "whole-message",
          `Paragraph ${index === 20 || index === 60 ? 40 : index}: ${"A saved message in the same turn. ".repeat(4)}\n\n`,
        ),
      ),
      ev.messageDone("whole-message"),
      ev.turnEnded("long"),
    ];
  // A long turn the first history page cuts inside its work (`cut_turn_id`, owner, 9 November
  // 2026): what loaded still folds under "Worked for" above the answer, counting no steps until
  // Load earlier brings the rest.
  if (mode === "partial-work")
    return [
      ev.turnStarted("long"),
      ev.user("long-user", "Review every file in the sidebar."),
      ...Array.from({ length: 40 }, (_, index) =>
        ev.toolCall(`long-read-${index}`, {
          kind: "read",
          title: `Read src/sidebar/part-${index}.tsx`,
          status: "completed",
          locations: [{ path: `src/sidebar/part-${index}.tsx`, line: null }],
          content: [
            { type: "text", text: `export const part${index} = ${index};`, truncated: null },
          ],
        }),
      ),
      ev.message("long-interim", "Now the styles."),
      ev.messageDone("long-interim"),
      ev.toolCall("long-edit", {
        kind: "edit",
        title: "Edit src/sidebar/sidebar.css",
        status: "completed",
        locations: [{ path: "src/sidebar/sidebar.css", line: 4 }],
      }),
      ev.message("long-answer", "Every sidebar file is reviewed, and the styles now match."),
      ev.messageDone("long-answer"),
      ev.turnEnded("long", "end_turn", { durationMs: 7_200_000 }),
    ];
  if (mode === "large-turn")
    return [
      ev.turnStarted("large"),
      ev.user("large-user", "Review these files."),
      ev.thought("large-thought", "Inspecting the files."),
      ev.thoughtDone("large-thought", 1000),
      ...Array.from({ length: 1000 }, (_, index) =>
        ev.toolCall(`large-tool-${index}`, {
          title: `Read file ${index}`,
          status: "completed",
          content: [{ type: "text", text: `Saved output ${index}`, truncated: null }],
        }),
      ),
      ev.turnEnded("large"),
    ];
  if (mode === "partial-history")
    return [
      ev.turnStarted("long"),
      ev.user("long-user", "Review the saved messages."),
      ...Array.from({ length: 80 }, (_, index) => [
        ev.message(
          `part-${index}`,
          `Paragraph ${index}: ${"A saved message in the same turn. ".repeat(4)}`,
        ),
        ev.messageDone(`part-${index}`),
      ]).flat(),
      ev.turnEnded("long"),
    ];
  if (mode === "history-question")
    return [
      ...synthetic("history"),
      ev.turnStarted("question-turn"),
      {
        type: "ui_request",
        request_id: "draft-question",
        prompt: { type: "input", message: "Describe the change", placeholder: null },
      },
    ];
  if (mode === "history")
    return Array.from({ length: 500 }, (_, index) => [
      ev.turnStarted(`history-${index}`),
      ev.user(`user-${index}`, `Review file ${index + 1}.`),
      ev.message(
        `message-${index}`,
        `Review ${index + 1}: the saved conversation remains available.\n\n${"This paragraph gives this turn a different measured height. ".repeat(1 + (index % 7))}\n\n[Review documentation](https://example.test/review/${index})`,
      ),
      ev.messageDone(`message-${index}`),
      ev.turnEnded(`history-${index}`),
    ]).flat();
  const events = [
    ev.turnStarted("first"),
    ev.user("u1", "Make the project list easier to scan, and keep the active chat visible."),
    ev.thought("thought", "I will check the sidebar layout and its existing keyboard behavior."),
    ev.thoughtDone("thought", 2400),
    ev.toolCall("read", {
      title: "Read Sidebar.tsx",
      status: "completed",
      locations: [{ path: "src/components/Sidebar.tsx", line: 18 }],
      content: [
        {
          type: "text",
          text: 'export function Sidebar() {\n  return <nav aria-label="Projects">...</nav>;\n}',
          truncated: null,
        },
      ],
    }),
    ev.toolCall("test", {
      kind: "execute",
      title: "pnpm test -- sidebar",
      status: "completed",
      content: [
        {
          type: "terminal",
          output: "✓ keeps the selected chat visible\n✓ preserves keyboard focus\n\n2 tests passed",
          exit_code: 0,
          truncated: null,
        },
      ],
    }),
    ev.message(
      "answer",
      'The sidebar now keeps the active chat visible and gives each project a clearer heading.\n\n- **Keyboard focus** stays on the same chat when new activity arrives.\n- **Needs you** collects pending requests without duplicating chats.\n\n```tsx\n<nav aria-label="Projects">\n  <ProjectList selected={chat.id} />\n</nav>\n```\n\nBoth sidebar tests pass.',
    ),
    ev.messageDone("answer"),
    ev.turnEnded("first", "end_turn", { durationMs: 18400 }),
  ];
  if (mode === "settled") return events;
  // File names an agent writes, as links to the files (missing features D, owner's plan).
  if (mode === "file-links")
    return [
      ...events,
      ev.turnStarted("files"),
      ev.user("u-files", "Where did the scroll jump come from?"),
      ev.toolCall("files-edit", {
        kind: "edit",
        title: "Edit src/styles.css",
        status: "completed",
        locations: [{ path: "src/styles.css", line: 4 }],
        content: [
          {
            type: "diff",
            path: "src/styles.css",
            old_text: ".row {\n  display: flex;\n  gap: 8px;\n  height: auto;\n}",
            new_text: ".row {\n  display: flex;\n  gap: 8px;\n  height: 32px;\n}",
            truncated: null,
          },
        ],
      }),
      // `&changed=7`: more edited files, for the changed-files card's row.
      ...extraEdits(Number(new URLSearchParams(location.search).get("changed") ?? 1) - 1),
      ev.message(
        "files-answer",
        "The jump came from `src/components/Sidebar.tsx:18`, where the list rendered again, so src/styles.css:4 now fixes the row height. [ChatView.tsx:120](src/components/ChatView.tsx#L120) passes the selected id, and `Sidebar.tsx` is the only component that changed shape.\n\nNot files: `console.log`, `src/missing.ts` and version 1.2.3. The old helper was in src/removed.ts:9.",
      ),
      ev.messageDone("files-answer"),
      ev.turnEnded("files", "end_turn", { durationMs: 9200 }),
    ];
  // Code colours, math and a diagram in one answer (missing features A, owner's plan).
  if (mode === "rich-answer")
    return [
      ...events,
      ev.turnStarted("rich"),
      ev.user("u-rich", "Show me the fix, the cost and how a click flows."),
      ev.message(
        "rich-answer",
        'The scroll fix needs one change and a test. Here is the change:\n\n```ts\nexport function reveal(row: HTMLElement, list: HTMLElement): void {\n  // Keep the open chat in view without moving focus.\n  if (!list.contains(row)) throw new Error("row is not in the list");\n  row.scrollIntoView({ block: "nearest" });\n  row.focus({ preventScroll: true });\n}\n```\n\nAnd the Rust side, for the native list:\n\n```rust\npub fn clamp_index(index: usize, len: usize) -> Option<usize> {\n    (len > 0).then(|| index.min(len - 1)) // 0 rows: nothing to show\n}\n```\n\nWith $n$ rows of height $h$, the list scrolls at most $\\max(0, nh - H)$ pixels, so the cost per frame is\n\n$$\nT(n) = \\sum_{i=1}^{n} \\frac{h_i}{v} \\le \\frac{nh}{v}\n$$\n\nHow a click reaches the row:\n\n```mermaid\nflowchart LR\n  Click[Sidebar click] --> Select[Select chat]\n  Select --> Reveal[Reveal row]\n  Reveal --> Focus[Focus without scroll]\n```',
      ),
      ev.messageDone("rich-answer"),
      // A diagram that asks for Mermaid's ELK layout, which Ghost leaves out (lib/elkStub.ts).
      ev.message(
        "rich-elk",
        "The same flow, laid out with ELK:\n\n```mermaid\n---\nconfig:\n  layout: elk\n---\nflowchart TD\n  Click --> Select\n```",
      ),
      ev.messageDone("rich-elk"),
    ];
  // What a call shows besides its output: Claude's to-do list as the turn's plan, a web search's
  // links, an MCP call's input and JSON reply, an edit's line counts, a long command, and a
  // compacted conversation (missing features A, owner's plan of 4 November 2026).
  if (mode === "tool-details")
    return [
      ...events,
      {
        type: "notice",
        level: "info",
        message:
          "Claude Code compacted the conversation from 153k tokens as its context filled, and continues from its summary.",
      },
      ev.turnStarted("details"),
      ev.user("u-details", "Find the open sidebar bugs and fix the first one."),
      ev.toolCall("todo", {
        title: "Update the to-do list",
        status: "completed",
        tool_name: "TodoWrite",
        raw_input: {
          todos: [
            {
              content: "Search for open sidebar bugs",
              status: "completed",
              activeForm: "Searching",
            },
            { content: "Fix the scroll position bug", status: "in_progress", activeForm: "Fixing" },
            { content: "Run the sidebar tests", status: "pending", activeForm: "Running tests" },
          ],
        },
      }),
      {
        type: "plan_updated",
        entries: [
          { content: "Search for open sidebar bugs", status: "completed", priority: null },
          { content: "Fix the scroll position bug", status: "in_progress", priority: null },
          { content: "Run the sidebar tests", status: "pending", priority: null },
        ],
      },
      ev.toolCall("search", {
        kind: "fetch",
        title: "Search the web for react sidebar scroll into view",
        status: "completed",
        tool_name: "WebSearch",
        raw_input: { query: "react sidebar scroll into view" },
        content: [
          {
            type: "text",
            text: 'Web search results for query: "react sidebar scroll into view"\n\nLinks: [{"title":"Element: scrollIntoView() method","url":"https://developer.mozilla.org/en-US/docs/Web/API/Element/scrollIntoView"},{"title":"Keeping the selected item visible [React]","url":"https://react.dev/learn/manipulating-the-dom-with-refs"}]\n\nscrollIntoView with block nearest keeps the item visible without jumping.',
            truncated: null,
          },
        ],
      }),
      ev.toolCall("issues", {
        title: "github: search_issues",
        status: "completed",
        tool_name: "mcp__github__search_issues",
        raw_input: {
          query: "is:open label:sidebar",
          limit: 5,
          sort: { by: "updated", order: "desc" },
        },
        content: [
          {
            type: "text",
            text: '{"total_count":2,"items":[{"number":412,"title":"Sidebar loses the open chat","state":"open"},{"number":398,"title":"Project headers wrap","state":"open"}]}',
            truncated: null,
          },
        ],
      }),
      ev.toolCall("fix", {
        kind: "edit",
        title: "Edit Sidebar.tsx",
        status: "completed",
        tool_name: "Edit",
        locations: [{ path: "src/components/Sidebar.tsx", line: 24 }],
        raw_input: {
          file_path: "src/components/Sidebar.tsx",
          old_string: "selected",
          new_string: "selected",
        },
        content: [
          {
            type: "diff",
            path: "src/components/Sidebar.tsx",
            old_text: "const row = rows[selected];\nrow.focus();\n",
            new_text:
              'const row = rows[selected];\nrow.scrollIntoView({ block: "nearest" });\nrow.focus({ preventScroll: true });\n',
            truncated: null,
          },
        ],
      }),
      ev.toolCall("check", {
        kind: "execute",
        title: "pnpm --filter @ghost/desktop exec vitest run …",
        status: "completed",
        tool_name: "Bash",
        raw_input: {
          command:
            "pnpm --filter @ghost/desktop exec vitest run \\\n  src/components/Sidebar.test.tsx \\\n  --reporter dot",
          description: "Run the sidebar tests",
        },
        content: [
          { type: "terminal", output: "..\n\n2 tests passed", exit_code: 0, truncated: null },
        ],
      }),
      ev.message(
        "details-answer",
        "Fixed #412: the open chat now scrolls into view without stealing focus, and the sidebar tests pass.",
      ),
      ev.messageDone("details-answer"),
    ];
  // A running turn that started subagents and background commands (tasksPreview.ts).
  if (mode === "tasks" || mode === "tasks-many")
    return [...events, ...taskPreviewEvents(mode === "tasks-many")];
  // A long turn still running, as an agent that thinks between its calls works: reasoning before
  // nearly every call, one word to the person midway, and the agent thinking again at its end.
  if (mode === "working-long") return [...events, ...workingLongEvents()];
  // A later start failed: the chat's worktree folder was deleted outside the app.
  if (mode === "missing-worktree")
    return [
      ...events,
      ev.error(
        "folder_missing",
        `The chat's folder ${MISSING_FOLDER} can't be found. Restore it (or reconnect its drive), then press Retry.`,
      ),
    ];
  events.push(ev.turnStarted("next"), ev.user("u2", "Check the small window layout too."));
  if (mode === "streaming") return events;
  if (mode === "plan")
    events.push({
      type: "permission_requested",
      request_id: "exit-plan",
      tool_call_id: null,
      title: "ExitPlanMode",
      kind: "switch_mode",
      raw_input: {
        plan: [
          "# Keep the active chat visible",
          "",
          "## Context",
          "",
          "The sidebar loses the open chat when a project has many chats.",
          "",
          "## Changes",
          "",
          "1. Scroll the open chat into view when it changes.",
          "2. Give each project a clearer heading row.",
          "3. Keep keyboard focus on the same chat when new activity arrives.",
          "",
          "## Verification",
          "",
          "- `pnpm --filter @ghost/desktop check:chat`",
          "- Resize to 960 x 640 and check nothing overflows.",
        ].join("\n"),
      },
      content: [],
      locations: [],
      options: [
        { option_id: "allow", kind: "allow_once", label: "Allow once" },
        { option_id: "deny", kind: "reject_once", label: "Deny" },
      ],
      truncated: null,
    });
  else if (mode === "permission")
    events.push(
      ev.toolCall("cmd", { kind: "execute", title: "pnpm test -- sidebar" }),
      ev.permission("permission"),
    );
  else if (mode === "error")
    events.push(
      ev.error(
        "agent_exited",
        "The agent stopped unexpectedly.",
        "The visible transcript is still saved.",
      ),
      ev.turnEnded("next", "error"),
    );
  else
    events.push(
      ev.thought(
        "next-thought",
        "Checking the narrow layout and the focus order.\n\nThe sidebar truncates long titles at 960 pixels, so the active chat needs its own scroll-into-view rather than relying on the list.",
      ),
      ev.thoughtDone("next-thought", 2400),
    );
  return events;
}

const STREAMED_ANSWER = `The composer now holds its shape at 960 pixels, and the sidebar keeps long titles on one line.

## What changed

- **Composer** keeps the agent, model and effort on one quiet row, and drops the model under 560 pixels.
- **Sidebar** truncates titles with an ellipsis and keeps the running state at the trailing edge.
- **Focus order** goes from the transcript to the composer, then to its controls.

\`\`\`css
.chat-composer {
  border: 1px solid var(--g-line-strong);
  border-radius: 16px;
  background: var(--g-surface-2);
}
\`\`\`

| Width | Composer | Sidebar |
| --- | --- | --- |
| 1440 | Full row | 264 px |
| 960 | Model hidden | 264 px |

All 24 layout tests pass. The narrow window keeps every control reachable from the keyboard, and the transcript column stays at 720 pixels wide until the window is narrower than the column.

The right panel still overlaps the composer below 880 pixels of chat width. It should float over the chat with a shadow there instead of squeezing the transcript, which keeps the reading column stable while files are reviewed.

Next, the permission card takes the composer's place while a request waits, so the answer is always where your hands already are.`;

/** A live turn played out over about ten seconds: a thought, four tools, then a long answer. */
function streamingScript(): { delay: number; events: GhostEvent[] }[] {
  const steps: { delay: number; events: GhostEvent[] }[] = [];
  const words = (text: string, size: number) => {
    const parts = text.split(/(?<= )/);
    const chunks: string[] = [];
    for (let index = 0; index < parts.length; index += size)
      chunks.push(parts.slice(index, index + size).join(""));
    return chunks;
  };
  for (const chunk of words(
    "Checking how the composer behaves at 960 pixels, then the sidebar truncation and the focus order. ",
    2,
  ))
    steps.push({ delay: 70, events: [ev.thought("stream-thought", chunk)] });
  steps.push({ delay: 120, events: [ev.thoughtDone("stream-thought", 1900)] });
  const file = "src/components/chat/Composer.tsx";
  steps.push({
    delay: 200,
    events: [
      ev.toolCall("stream-read", {
        kind: "read",
        title: `Read ${file}`,
        locations: [{ path: file, line: 124 }],
      }),
    ],
  });
  steps.push({
    delay: 500,
    events: [
      ev.toolUpdate("stream-read", {
        status: "completed",
        content: [
          {
            type: "text",
            text: 'return (\n  <div className="chat-composer">\n    <ContextChips context={context} />\n  </div>\n);',
            truncated: null,
          },
        ],
      }),
    ],
  });
  steps.push({
    delay: 150,
    events: [ev.toolCall("stream-search", { kind: "search", title: 'Search "chat-composer"' })],
  });
  steps.push({
    delay: 400,
    events: [
      ev.toolUpdate("stream-search", {
        status: "completed",
        content: [{ type: "text", text: "src/chat.css:1175\nsrc/shell.css:640", truncated: null }],
      }),
    ],
  });
  steps.push({
    delay: 150,
    events: [ev.toolCall("stream-test", { kind: "execute", title: "pnpm test -- layout" })],
  });
  const lines: string[] = [];
  for (let index = 1; index <= 10; index++) {
    lines.push(`✓ layout ${index} keeps its column at 960 pixels`);
    steps.push({
      delay: 120,
      events: [
        ev.toolUpdate("stream-test", {
          content: [
            { type: "terminal", output: lines.join("\n"), exit_code: null, truncated: null },
          ],
        }),
      ],
    });
  }
  steps.push({
    delay: 200,
    events: [
      ev.toolUpdate("stream-test", {
        status: "completed",
        content: [
          {
            type: "terminal",
            output: `${lines.join("\n")}\n\n24 tests passed`,
            exit_code: 0,
            truncated: null,
          },
        ],
      }),
    ],
  });
  steps.push({
    delay: 150,
    events: [
      ev.toolCall("stream-edit", {
        kind: "edit",
        title: "Edit src/chat.css",
        status: "completed",
        content: [
          {
            type: "diff",
            path: "src/chat.css",
            old_text: ".chat-composer {\n  border-radius: 18px;\n  box-shadow: var(--o-e1);\n}",
            new_text: ".chat-composer {\n  border-radius: 16px;\n}",
            truncated: null,
          },
        ],
      }),
    ],
  });
  for (const chunk of words(STREAMED_ANSWER, 3))
    steps.push({ delay: 30, events: [ev.message("stream-answer", chunk)] });
  steps.push({ delay: 100, events: [ev.messageDone("stream-answer")] });
  steps.push({
    delay: 50,
    events: [ev.turnEnded("next", "end_turn", { durationMs: 21400 })],
  });
  return steps;
}

function Preview() {
  const requested = new URLSearchParams(location.search).get("preview") ?? "settled";
  const mode = [
    "settled",
    "smoke-scroll",
    "streaming",
    "thinking",
    "permission",
    "plan",
    "error",
    "new",
    "launch",
    "history",
    "history-question",
    "partial-history",
    "partial-message",
    "partial-work",
    "large-turn",
    "long-prompt",
    "memory-prompt",
    "long-output",
    "focus-group",
    "diff",
    "recorded-thinking",
    "recorded-tools",
    "terminal-panel",
    "missing-worktree",
    "tasks",
    "tasks-many",
    "working-long",
    "tool-details",
    "rich-answer",
    "file-links",
    ...Object.keys(NEW_CHAT_SCENARIOS),
    ...Object.keys(QUEUE_PREVIEWS),
  ].includes(requested)
    ? requested
    : "settled";
  const queued = QUEUE_PREVIEWS[mode] ?? null;
  // The chat's transcript: a queue case shows the chat it waits in.
  const shown = queued?.events ?? mode;
  // `queue-stalled`: the store's clock runs a minute ahead once the chat has joined, so the
  // still-working hint shows at once instead of after a real minute.
  const [now] = useState(() => {
    if (!queued?.quiet) return undefined;
    let calls = 0;
    return () => Date.now() + (calls++ === 0 ? 0 : 61_000);
  });
  // `?preview=terminal-panel&tabs=2&height=280`: the settled chat with its docked terminals open.
  const [terminals] = useState(() => {
    const params = new URLSearchParams(location.search);
    return new TerminalPanels(
      terminalPanelFixture({ refuse: params.get("refuse"), failReview: params.get("fail-review") }),
      createXtermRuntime,
    );
  });
  useEffect(() => {
    if (mode !== "terminal-panel") return;
    const params = new URLSearchParams(location.search);
    const count = Math.max(1, Math.min(8, Number(params.get("tabs") ?? "2") || 1));
    void (async () => {
      for (let index = 0; index < count; index++)
        await terminals.newTab("preview", index % 2 === 1 ? "command_prompt" : "power_shell");
      const first = terminals.panel("preview").tabs[0];
      if (first) terminals.select("preview", first);
      const height = Number(params.get("height"));
      if (height) terminals.setHeight("preview", height);
    })();
    return () => void terminals.dispose();
  }, [mode, terminals]);
  const scenario = NEW_CHAT_SCENARIOS[mode] ?? null;
  const [roles, setRoles] = useState<ChatRoles>(previewRoles);
  const [palette, setPalette] = useState(false);
  const [chatCommands, setChatCommands] = useState<readonly PaletteCommand[]>([]);
  const [collapsed, setCollapsed] = useState(false);
  const [projectNew, setProjectNew] = useState(false);
  // `launch`: the start screen until New chat opens the composer.
  const [launching, setLaunching] = useState(mode === "launch");
  const [selected, setSelected] = useState<string | null>(
    mode === "new" || mode === "launch" || scenario ? null : "preview",
  );
  // `&stopped`: the settled chat after its agent stopped, its last models and levels still offered.
  const stopped = mode === "settled" && new URLSearchParams(location.search).has("stopped");
  // Earlier turns without auracode checkpoints, in a chat whose agent keeps its own (Claude).
  const nativeRewind =
    mode === "settled" && new URLSearchParams(location.search).has("native-rewind");
  const status: SessionStatus =
    queued?.status ??
    (stopped
      ? "stopped"
      : mode === "thinking" ||
          mode === "streaming" ||
          mode === "permission" ||
          mode === "plan" ||
          mode === "history-question" ||
          mode === "focus-group" ||
          mode === "tasks" ||
          mode === "tasks-many" ||
          mode === "working-long"
        ? "running"
        : mode === "error" || mode === "missing-worktree"
          ? "failed"
          : "ready");
  const chat = useMemo(
    () =>
      chatSummary("preview", {
        title: "Make the sidebar easier to scan",
        agent_id: "claude",
        agent_name: AGENT.name,
        folder: shown === "missing-worktree" ? MISSING_FOLDER : "/workspace/auracode",
        folder_state: shown === "missing-worktree" ? "missing" : "ok",
        status,
        needs_attention: shown === "permission",
        // A real time, so the sidebar's row shows how long ago the chat last moved.
        updated_at: Date.now() - 4 * 60_000,
      }),
    [shown, status],
  );
  const publishPreview = useRef<((updates: GhostEvent[]) => void) | null>(null);
  const attached = useRef(false);
  useEffect(() => {
    if (mode !== "streaming") return;
    // Plays one live turn once the chat has joined, as an agent would stream it.
    const steps = streamingScript();
    let index = 0;
    let timer: ReturnType<typeof setTimeout>;
    const next = () => {
      const step = steps[index];
      if (!step) return;
      timer = setTimeout(() => {
        publishPreview.current?.(step.events);
        index++;
        next();
      }, step.delay);
    };
    const wait = setInterval(() => {
      if (!attached.current) return;
      clearInterval(wait);
      timer = setTimeout(next, 600);
    }, 50);
    return () => {
      clearInterval(wait);
      clearTimeout(timer);
    };
  }, [mode]);
  useEffect(() => {
    if (mode !== "focus-group") return;
    const step = (event: Event) => {
      if ((event as CustomEvent).detail === "add")
        publishPreview.current?.([
          ev.toolCall("focus-second", {
            status: "in_progress",
            title: "Read the second file",
            content: [{ type: "text", text: "Second output", truncated: null }],
          }),
        ]);
      else if ((event as CustomEvent).detail === "finish")
        publishPreview.current?.([
          ev.toolUpdate("focus-first", { status: "completed" }),
          ev.toolUpdate("focus-second", { status: "completed" }),
          ev.turnEnded("focus-group"),
        ]);
    };
    window.addEventListener("auracode-preview-tool-step", step);
    return () => window.removeEventListener("auracode-preview-tool-step", step);
  }, [mode]);
  useEffect(() => {
    if (mode !== "thinking") return;
    // Browser acceptance advances one live turn deterministically, without sending a prompt.
    const step = (event: Event) => {
      switch ((event as CustomEvent).detail) {
        case "think":
          // Reasoning streaming in: its newest words show on the working line.
          publishPreview.current?.([ev.thought("working-live", "Weighing the narrow layout")]);
          break;
        case "tools":
          publishPreview.current?.([
            ev.thoughtDone("working-live", 3000),
            ev.toolCall("working-read", { title: "Read first.ts", status: "completed" }),
            // A blink of reasoning with no words gets no row, so it does not split the stretch.
            ev.thought("working-hidden", ""),
            ev.thoughtDone("working-hidden", 0),
            ev.toolCall("working-search", { kind: "search", title: "Search for sidebar" }),
          ]);
          break;
        case "text":
          publishPreview.current?.([
            ev.toolUpdate("working-search", { status: "completed" }),
            ev.message("working-answer", "A streamed answer."),
          ]);
          break;
        case "rest":
          publishPreview.current?.([ev.messageDone("working-answer")]);
          break;
        case "finish":
          publishPreview.current?.([ev.turnEnded("next", "end_turn", { durationMs: 12_000 })]);
          break;
      }
    };
    window.addEventListener("auracode-preview-working-step", step);
    return () => window.removeEventListener("auracode-preview-working-step", step);
  }, [mode]);
  const transport = useMemo(() => {
    const events =
      mode === "recorded-thinking"
        ? recorded(thinking)
        : mode === "recorded-tools"
          ? recorded(fullAuto)
          : synthetic(mode === "terminal-panel" ? "settled" : shown);
    let seq = events.length;
    // A queue case: two messages waiting, and the session's word on why.
    let queue: QueuedPrompt[] = queued
      ? QUEUED_WORDS.map((preview, index) => ({
          prompt_id: `queued-${index + 1}`,
          preview,
          held: queued.held === true,
          created_at: index + 1,
        }))
      : [];
    let wait: QueueWait | null = queued?.wait ?? null;
    const queueCalls: string[] = [];
    // Each preview send is its own turn, as a session would make it.
    let sends = 0;
    Object.assign(window, { queuePreview: { calls: () => [...queueCalls] } });
    const header = () =>
      sessionInfo({
        agent_name: AGENT.name,
        status: currentStatus,
        queue_held: queue.some((row) => row.held),
        queue_wait: queue.length > 0 ? wait : null,
        revision: choices.revision,
      });
    const history = items(numbered(events.map((event) => [null, event] as const)));
    const before =
      mode === "partial-message"
        ? 42
        : mode === "partial-history"
          ? 82
          : mode === "partial-work"
            ? 22
            : mode.startsWith("history")
              ? 1250
              : null;
    const initialHistory = before === null ? history : history.slice(before);
    let receive: ((batch: EventBatch) => void) | null = null;
    let currentStatus: SessionStatus = status;
    let choices = lists(1, {
      // A plan comes from Claude Code, which takes a plan back as the person edited it;
      // `&edits=none` is an agent that does not.
      capabilities:
        nativeRewind ||
        (mode === "plan" && new URLSearchParams(location.search).get("edits") !== "none")
          ? NATIVE_REWIND_CAPABILITIES
          : null,
      // What Claude Code reports loading: the Session tab lists it.
      loaded: {
        mcp_servers: [
          { name: "docs", state: "connected", source: "user" },
          { name: "tickets", state: "needs_sign_in", source: "project" },
          { name: "plugin:context7:context7", state: "failed", source: "plugin" },
        ],
        skills: ["deploy", "release-notes"],
        plugins: [
          { name: "vercel", version: "0.50.0" },
          { name: "code-simplifier", version: null },
        ],
        agents: ["Explore", "Plan", "general-purpose"],
        tools: 38,
      },
      // Claude Code lists /compact, which the header menu offers as Compact conversation.
      commands:
        mode === "tool-details"
          ? [
              {
                name: "compact",
                description: "Summarize the conversation to free context",
                input_hint: null,
              },
            ]
          : [],
      controls: {
        version: 1,
        access: {
          requested: "default",
          effective: "default",
          pending: null,
          native_mode: "default",
          note: null,
          choices: [
            {
              value: "default",
              name: "Ask for approval",
              description: "Review actions before they run.",
              legacy_autonomy: null,
            },
            {
              value: "bypassPermissions",
              name: "Full access",
              description: "Allow actions without permission prompts.",
              legacy_autonomy: null,
            },
          ],
        },
        workflow: {
          requested: "execute",
          effective: "execute",
          pending: null,
          native_mode: null,
          note: null,
          choices: [
            {
              value: "execute",
              name: "Execute",
              description: "Make changes using the chosen access level.",
              legacy_autonomy: null,
            },
            {
              value: "plan",
              name: "Plan",
              description: "Review the work before implementation.",
              legacy_autonomy: null,
            },
          ],
        },
      },
      options: [
        {
          id: "model",
          name: "Model",
          category: "model",
          description: null,
          pending: null,
          kind: {
            type: "select",
            current: "sonnet",
            // Claude Code's list as its driver reports it: a dozen models, so the menu must keep
            // the thinking level in view above a list that scrolls.
            choices: PREVIEW_MODELS.map(([value, name, description]) => ({
              value,
              name,
              description,
              group: null,
            })),
          },
        },
        {
          id: "effort",
          name: "Effort",
          category: "effort",
          description: null,
          pending: null,
          kind: {
            type: "select",
            current: "medium",
            // As Claude's driver names them: its default, then the bare level ids.
            choices: [
              { value: "default", name: "Default", description: null, group: null },
              ...["low", "medium", "high", "xhigh", "max"].map((value) => ({
                value,
                name: value,
                description: null,
                group: null,
              })),
            ],
          },
        },
      ],
    });
    const publish = (updates: GhostEvent[]) => {
      receive?.(frame("preview", seq, updates));
      seq += updates.length;
    };
    const context = withPreviewContext(
      new FakeTransport({
        // Claude Code chats show the account picker: one added account beside the main one.
        accounts: createAccountsFixture(["Work"]),
        lists: async () => choices,
        setControl: async (_chatId, change) => {
          if (currentStatus !== "ready" || queue.length > 0)
            throw new Error("Wait until the agent is idle.");
          const control = choices.controls?.[change.type];
          if (!control?.choices?.some((choice) => choice.value === change.value))
            throw new Error("Unsupported native control.");
          if (
            change.type === "workflow" &&
            control.effective === "plan" &&
            change.value === "execute" &&
            !change.implementation_approved
          )
            throw new Error("Approve implementation first.");
          choices = {
            ...choices,
            revision: choices.revision + 1,
            controls: {
              ...choices.controls,
              [change.type]: { ...control, requested: change.value, effective: change.value },
            },
          };
          return choices;
        },
        setOption: async (_chatId, id, value) => {
          const option = choices.options.find((option) => option.id === id);
          if (option?.kind.type !== "select" || value.type !== "choice")
            throw new Error(`This agent has no option "${id}".`);
          if (!option.kind.choices.some((choice) => choice.value === value.value))
            throw new Error(`"${value.value}" isn't one of the choices for ${option.name}.`);
          if (currentStatus === "starting" || currentStatus === "stopping")
            throw new Error(
              "Wait for the agent to finish starting or stopping, then choose again.",
            );
          const before = choices;
          choices = {
            ...before,
            revision: before.revision + 1,
            options: before.options.map((option) =>
              option.id === id && option.kind.type === "select"
                ? { ...option, kind: { ...option.kind, current: value.value } }
                : option,
            ),
          };
          // A stopped chat keeps the choice for its next start and answers with it.
          if (currentStatus === "stopped" || currentStatus === "failed") return choices;
          // A running agent answers first and sends the lists that show the change after it,
          // with a new revision, as the real drivers do.
          setTimeout(() => publish([ev.session(header())]), 40);
          return before;
        },
        attach: async (chatId, onFrame) => {
          publishPreview.current = publish;
          receive = onFrame;
          attached.current = true;
          return attachResult(
            chatId,
            seq,
            historyPage(chatId, initialHistory, {
              has_more: before !== null,
              before_seq: before,
              cut_turn_id: mode.startsWith("partial-") ? "long" : null,
            }),
            {
              chat: { ...chat, status: currentStatus },
              session: header(),
              queue,
              tasks:
                mode === "tasks" || mode === "tasks-many"
                  ? taskPreviewRecords(mode === "tasks-many")
                  : [],
            },
          );
        },
        history: async (chatId) => historyPage(chatId, history.slice(0, before ?? 0)),
        pickFolder: async () => "/workspace/auracode",
        stop: async () => {
          currentStatus = "stopped";
          publish([ev.session(header())]);
        },
        // Send now and Send: the message goes, and the next one waits behind its turn.
        sendNow: async (_chatId, promptId) => {
          queueCalls.push(`sendNow:${promptId}`);
          queue = queue.filter((row) => row.prompt_id !== promptId);
          currentStatus = "running";
          wait = queue.some((row) => !row.held)
            ? { reason: "working" }
            : { reason: "held", after: "restart" };
          publish([ev.lifecycle(promptId, "started"), ev.session(header())]);
          return queue;
        },
        // Discard: the message leaves the queue; the next one goes unless it is held too.
        cancelQueued: async (_chatId, promptId) => {
          queueCalls.push(`discard:${promptId}`);
          queue = queue.filter((row) => row.prompt_id !== promptId);
          wait = queue[0]?.held ? { reason: "held", after: "restart" } : { reason: "working" };
          publish([ev.lifecycle(promptId, "cancelled"), ev.session(header())]);
        },
        resumeQueue: async () => {
          queueCalls.push("resumeQueue");
          queue = queue.map((row) => ({ ...row, held: false }));
          wait = { reason: "working" };
          publish([ev.session(header())]);
        },
        start: async () => {
          queueCalls.push("start");
          currentStatus = "starting";
          wait = { reason: "starting" };
          publish([ev.session(header())]);
        },
        respondPermission: async (_chatId, requestId, optionId) =>
          publish([ev.permissionResolved(requestId, optionId)]),
        acceptPlan: async (_chatId, requestId, optionId, plan) =>
          publish([ev.permissionResolved(requestId, optionId, "user", plan)]),
        send: async (_chatId, text) => {
          sends += 1;
          const turn = sends === 1 ? "preview-send" : `preview-send-${sends}`;
          const answer = sends === 1 ? "preview-answer" : `preview-answer-${sends}`;
          publish([
            ev.turnStarted(turn),
            ev.user(sends === 1 ? "preview-user" : `preview-user-${sends}`, text),
            ev.message(answer, "This is a local UI preview. No prompt was sent to an agent."),
            ev.messageDone(answer),
            ev.turnEnded(turn),
          ]);
          return { prompt_id: turn, queued: false };
        },
      }),
      { nativeRewind },
    );
    // `?preview=terminal-panel&run=1`: a worktree chat whose folder setup saved a Run command.
    if (mode === "terminal-panel" && new URLSearchParams(location.search).has("run"))
      Object.assign(context, { run: runFixture() });
    return shown === "missing-worktree"
      ? Object.assign(context, { git: missingWorktreeGit() })
      : context;
  }, [mode, shown, queued, status, chat, nativeRewind]);
  return (
    // Mirrors ChatsScreen's frame: the drag bands, the sidebar toggle, the panes.
    <div
      className="chat-app"
      data-glass="none"
      data-os={new URLSearchParams(location.search).get("os") ?? "windows"}
      data-sidebar-collapsed={collapsed}
    >
      <aside className="chat-sidebar" aria-label="Sidebar">
        <SidebarBand collapsed={collapsed} onToggle={() => setCollapsed((value) => !value)} />
        <ChatList
          onCustomize={() => {
            location.href = "/?preview=customize";
          }}
          onAutomations={() => {
            location.href = "/?preview=settings&page=remote";
          }}
          onProjectNewChat={() => {
            setProjectNew(true);
            setLaunching(false);
            setSelected(null);
          }}
          organization={{
            projects: [
              {
                id: "project-auracode",
                name: "auracode",
                color: null,
                default_agent: "claude",
                default_model: null,
                created_at: 1,
                updated_at: 1,
                folders: [
                  {
                    id: "folder-auracode",
                    project_id: "project-auracode",
                    display_name: "auracode",
                    path: "/workspace/auracode",
                    folder_state: "ok",
                    unavailable_reason: null,
                    kind: "local",
                    git_remote_url: null,
                    git_default_branch: "main",
                  },
                ],
              },
            ],
            chats: [chat, chatSummary("older")].map((item) => ({
              chat: item,
              project_id: "project-auracode",
              folder_id: "folder-auracode",
              pinned: false,
              archived: false,
            })),
          }}
          onSearch={() => setPalette(true)}
          chats={[
            chat,
            chatSummary("older", {
              title: "Set up the project",
              folder: chat.folder,
              agent_name: "Codex",
              agent_id: "codex",
              updated_at: Date.now() - 3 * 3_600_000,
            }),
          ]}
          error={null}
          selected={selected}
          onSelect={() => setSelected("preview")}
          onNewChat={() =>
            flyWing(() => {
              setProjectNew(false);
              setLaunching(false);
              setSelected(null);
            })
          }
        />
        <SidebarFooter
          onSettings={() =>
            alert("This is a development-only protocol fixture. No agents run here.")
          }
        />
      </aside>
      <main className="chat-main">
        <div className="chat-drag" />
        {selected === null && launching ? (
          <LaunchArt />
        ) : selected === null ? (
          <NewChatPreview
            base={AGENT}
            scenario={projectNew ? "worktree" : scenario}
            projectContext={projectNew}
            onCreated={() => setSelected("preview")}
          />
        ) : (
          <ChatView
            onCommands={setChatCommands}
            terminals={terminals}
            chatId="preview"
            transport={transport}
            listed={chat}
            storage={null}
            agents={CHAT_AGENTS}
            now={now}
            roles={roles}
            roleCatalog={ROLE_CATALOG}
            onSetRole={async (role, choice) => setRoles((value) => ({ ...value, [role]: choice }))}
          />
        )}
      </main>
      {palette ? (
        <CommandPalette
          transport={transport}
          chatId={selected}
          onClose={() => setPalette(false)}
          onChat={() => setSelected("preview")}
          commands={[
            ...(selected !== null ? chatCommands : []),
            {
              id: "new",
              title: "New chat",
              detail: "Choose an agent and project",
              run: () => {
                setLaunching(false);
                setSelected(null);
              },
            },
          ]}
        />
      ) : null}
    </div>
  );
}

export function mountPreview(container: HTMLElement): void {
  // Read aloud's stand-in: a voice that is built in and makes no sound.
  window.speechFixture = speechFixture();
  speechPlayer.use(window.speechFixture.api);
  const preview = <Preview />;
  createRoot(container).render(
    new URLSearchParams(location.search).has("strict") ? (
      <StrictMode>{preview}</StrictMode>
    ) : (
      preview
    ),
  );
}

/** Completed edits to `count` more files, so the changed-files card has names to fit. */
function extraEdits(count: number): GhostEvent[] {
  const names = [
    "src/lib/dashboard.ts",
    "src/components/dashboard/DashboardScreen.tsx",
    "src/components/dashboard/dashboard.css",
    "src/shell.css",
    "src/App.tsx",
    "docs/design/visual-language.md",
    "src/components/chat/ChatList.tsx",
    "src/main.tsx",
    "src/lib/processes.ts",
  ];
  return names.slice(0, Math.max(0, count)).map((path, index) =>
    ev.toolCall(`files-extra-${index}`, {
      kind: "edit",
      title: `Edit ${path}`,
      status: "completed",
      locations: [{ path, line: 1 }],
      content: [{ type: "diff", path, old_text: "a", new_text: "b", truncated: null }],
    }),
  );
}
