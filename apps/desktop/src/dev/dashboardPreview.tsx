/**
 * Scripted Dashboard fixture (`?preview=dashboard`, `&case=empty`, `&case=live`, `&case=busy`):
 * the Dashboard open over an empty inset pane, as it opens over the chats in the app, with a
 * Dashboard button in the pane to open it again once it is closed (`&closed` starts closed).
 * Never reads the process table or stops anything:
 * the snapshot is invented, and Stop removes the row and what it started. `live` moves a chat from
 * Working to Needs you and back every few seconds, so the cards' moves between lanes can be
 * watched; `busy` adds a day's worth of finished chats. AI line edits shows an invented summer of
 * Claude Code and Codex edits (none with `empty`), and `counting` keeps it counting for a moment.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  BackgroundTask,
  ChatSummary,
  LineEditBucket,
  ProcessesSnapshot,
  ProcessRow,
} from "../bindings";
import { Dashboard } from "../components/dashboard/Dashboard";
import type { AppStore } from "../lib/chat/appStore";
import { DashboardCleared } from "../lib/dashboardCleared";
import type { LineEditsApi } from "../lib/lineEdits";
import type { ProcessesApi } from "../lib/processes";
import type { WorkspaceStore } from "../lib/workspace";

const MINUTE = 60_000;

const summary = (
  id: string,
  title: string,
  agent: [string, string],
  status: ChatSummary["status"],
  minutesAgo: number,
  extra: Partial<ChatSummary> = {},
): ChatSummary =>
  ({
    id,
    title,
    agent_id: agent[0],
    agent_name: agent[1],
    folder: "C:\\work\\auracode",
    folder_state: "ok",
    status,
    needs_attention: false,
    created_at: Date.now() - (minutesAgo + 30) * MINUTE,
    updated_at: Date.now() - minutesAgo * MINUTE,
    ...extra,
  }) as ChatSummary;

const CLAUDE: [string, string] = ["claude", "Claude Code"];
const CODEX: [string, string] = ["codex", "Codex"];
const PI: [string, string] = ["pi", "Pi"];
const GROK: [string, string] = ["grok", "Grok Build"];

const task = (id: string, extra: Partial<BackgroundTask>): BackgroundTask => ({
  task_id: id,
  kind: "agent",
  title: "Task",
  agent_type: "Explore",
  tool_call_id: null,
  parent_task_id: null,
  backgrounded: true,
  status: "running",
  activity: null,
  tool_uses: null,
  tokens: null,
  duration_ms: null,
  started_at: Date.now() - 74_000,
  ended_at: null,
  prompt: null,
  summary: null,
  summary_truncated: null,
  ...extra,
});

function fixture(kind: string | null) {
  const empty = kind === "empty";
  const row = (
    pid: number,
    parent: number | null,
    name: string,
    command: string,
    minutes: number,
    megabytes: number,
  ): ProcessRow => ({
    pid,
    parent,
    name,
    command,
    started_ms: Date.now() - minutes * MINUTE,
    cpu_ms: 0,
    memory_bytes: megabytes * 1024 * 1024,
  });
  let rows: ProcessRow[] = [
    row(
      4120,
      null,
      "claude.exe",
      "claude -p --input-format stream-json --output-format stream-json --verbose",
      12,
      214,
    ),
    row(4388, 4120, "bash.exe", "bash -c cargo test -p ghost-desktop", 2, 9),
    row(5102, 4388, "cargo.exe", "cargo test -p ghost-desktop --lib dictation", 2, 61),
    row(5630, null, "ghost.exe", "C:\\Users\\me\\AppData\\Local\\auracode\\ghost.exe", 1, 96),
  ];
  const codex: ProcessRow[] = [
    row(6610, null, "codex.exe", "codex app-server", 40, 88),
    row(6702, 6610, "node.exe", "node scripts/check-dictation.mjs", 0, 134),
  ];
  const opened = Date.now();
  const api: ProcessesApi = {
    snapshot: async () => {
      // Each program has used a steady share of one core since the page opened: 4% to 36%.
      const busy = (list: ProcessRow[]) =>
        list.map((entry, index) => ({
          ...entry,
          cpu_ms: Math.round((Date.now() - opened) * (0.04 + index * 0.08)),
        }));
      const snapshot: ProcessesSnapshot = {
        taken_at_ms: Date.now(),
        chats: empty
          ? []
          : [
              {
                chat_id: "chat-claude",
                agents: [{ pid: 4120, processes: busy(rows) }],
                tasks: [
                  task("explore-dictation", {
                    title: "Explore dictation feature end to end",
                    activity: "Reading audio.rs",
                    tool_uses: 12,
                  }),
                  task("tests", {
                    kind: "command",
                    agent_type: null,
                    title: "Run the desktop tests",
                    activity: null,
                    started_at: Date.now() - 41_000,
                  }),
                  task("explore-themes", {
                    title: "Explore theming system and changelog",
                    status: "completed",
                    started_at: Date.now() - 600_000,
                    duration_ms: 41_000,
                  }),
                ],
              },
              {
                chat_id: "chat-codex",
                agents: [{ pid: 6610, processes: busy(codex) }],
                tasks: [],
              },
            ],
      };
      return snapshot;
    },
    stop: async (_chatId, pid) => {
      const gone = new Set([pid]);
      for (const entry of rows)
        if (entry.parent !== null && gone.has(entry.parent)) gone.add(entry.pid);
      rows = rows.filter((entry) => !gone.has(entry.pid));
    },
  };
  let chats: ChatSummary[] = empty
    ? []
    : [
        summary("chat-review", "Review the sync conflict resolver", CLAUDE, "running", 2, {
          needs_attention: true,
        }),
        summary("chat-grok", "Port the release workflow to Linux runners", GROK, "failed", 9),
        summary("chat-claude", "Fix dictation on Windows", CLAUDE, "running", 0),
        summary("chat-codex", "Check the dictation fixture", CODEX, "running", 0),
        summary("chat-pi", "Tidy the changelog check", PI, "ready", 14),
        summary("chat-themes", "Add a theme for every agent", CODEX, "stopped", 180),
        // In its own worktree, whose folder is an app-owned name: the card names the project.
        summary("chat-worktree", "Rework the sync queue", CODEX, "running", 1, {
          folder: "C:\\Users\\me\\AppData\\Local\\auracode\\worktrees\\3f9a2c1d7e4b",
        }),
        // Archived: put away, so not on the board.
        summary("chat-archived", "Try a second sidebar layout", PI, "ready", 5),
        // Quiet for two days: in the sidebar, not on the board.
        summary("chat-old", "Sketch the onboarding globe", PI, "stopped", 2 * 24 * 60),
        ...(kind === "busy"
          ? Array.from({ length: 11 }, (_, index) =>
              summary(
                `chat-day-${index}`,
                `Earlier task ${index + 1}`,
                CLAUDE,
                "ready",
                30 + index * 40,
              ),
            )
          : []),
      ];
  const organization = {
    data: {
      projects: [{ id: "project-auracode", name: "auracode", color: null, folders: [] }],
      chats: chats.map((chat) => ({
        chat,
        project_id: "project-auracode",
        folder_id: "folder-auracode",
        archived: chat.id === "chat-archived",
      })),
    },
    error: null,
    cached: false,
    pending: 0,
  };
  const workspace = {
    subscribe: () => () => {},
    getSnapshot: () => organization,
  } as unknown as WorkspaceStore;
  const listeners = new Set<() => void>();
  // useSyncExternalStore needs the same object back until something changes.
  let state = { chats };
  if (kind === "live") {
    setInterval(() => {
      chats = chats.map((chat) =>
        chat.id === "chat-codex"
          ? { ...chat, needs_attention: !chat.needs_attention, updated_at: Date.now() }
          : chat,
      );
      state = { chats };
      for (const listener of listeners) listener();
    }, 3000);
  }
  const app = {
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getSnapshot: () => state,
  } as unknown as AppStore;
  return { api, app, workspace };
}

/**
 * A year of invented line edits: a busy stretch from about five months ago to two months ago, a
 * few days since, and the odd day before, the same on every load.
 */
function lineEditsFixture(kind: string | null): LineEditsApi {
  let seed = 7;
  const random = () => {
    seed = (seed * 16807) % 2147483647;
    return seed / 2147483647;
  };
  const buckets: LineEditBucket[] = [];
  if (kind !== "empty") {
    const today = new Date();
    for (let back = 370; back >= 0; back -= 1) {
      const day = new Date(today.getFullYear(), today.getMonth(), today.getDate() - back);
      const busy = back >= 60 && back <= 140;
      const recent = back <= 14 && back % 4 === 1;
      const odd = back === 365 || back === 300;
      if (!(busy ? random() < 0.72 : recent || odd)) continue;
      const lines = Math.round((busy ? 400 + random() * 9000 : 120 + random() * 900) / 10) * 10;
      const at = day.getTime() + 10 * 60 * MINUTE;
      const codex = busy && random() < 0.35 ? Math.round(lines * random() * 0.6) : 0;
      buckets.push({ agent: "claude", at, lines: lines - codex });
      if (codex > 0) buckets.push({ agent: "codex", at: at + 2 * 60 * MINUTE, lines: codex });
    }
  }
  let reads = 0;
  return {
    read: async (since) => {
      reads += 1;
      return {
        buckets: buckets.filter((bucket) => bucket.at >= since),
        complete: kind !== "counting" || reads > 3,
      };
    },
  };
}

/** The pane the Dashboard opens over, with the button that opens it, as the sidebar's does. */
function DashboardPreview({
  start,
  ...fixtures
}: Omit<Parameters<typeof Dashboard>[0], "onClose" | "onOpenChat"> & { readonly start: boolean }) {
  const [open, setOpen] = useState(start);
  return (
    <div className="pane-shell g-root">
      <main
        style={{
          flex: 1,
          padding: "var(--g-space-24)",
          borderRadius: "var(--ui-pane-radius)",
          background: "var(--ui-pane-fill)",
        }}
      >
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="dashboard-button"
          aria-haspopup="dialog"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          Dashboard
        </button>
      </main>
      {open ? (
        <Dashboard
          {...fixtures}
          onClose={() => setOpen(false)}
          onOpenChat={(id) => {
            // The checks read which chat a card asked to open.
            document.body.dataset.openedChat = id;
          }}
        />
      ) : null}
    </div>
  );
}

export function mountDashboardPreview(container: HTMLElement) {
  const params = new URLSearchParams(location.search);
  const kind = params.get("case");
  const { api, app, workspace } = fixture(kind);
  const lineEdits = lineEditsFixture(kind);
  // The app's own frame (App.tsx): the inset pane fills the window and the Dashboard opens over it.
  container.style.height = "100vh";
  createRoot(container).render(
    <StrictMode>
      <DashboardPreview
        start={!params.has("closed")}
        app={app}
        workspace={workspace}
        api={api}
        lineEdits={lineEdits}
        // Clearing lasts for the page only, so each check starts from the full board.
        cleared={new DashboardCleared(null)}
        stopChat={async (id) => {
          // The checks read which chat's agent a card asked to stop.
          document.body.dataset.stoppedChat = id;
        }}
      />
    </StrictMode>,
  );
}
