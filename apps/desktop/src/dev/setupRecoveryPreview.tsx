/** In-memory admission and process-owner fixture. No native commands, Git or agents run here. */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  ChatSummary,
  GhostEvent,
  ManagedWorktree,
  QueuedPrompt,
  TerminalChunk,
  WorktreeSetupAttempt,
} from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import { createXtermRuntime } from "../components/chat/terminalRuntime";
import { WorktreeRecoveryList } from "../components/chat/WorktreeRecoveryList";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  frame,
  historyPage,
  sessionInfo,
} from "../lib/chat/testing";
import type { Transport } from "../lib/chat/transport";
import type { ProjectToolsApi } from "../lib/projectTools";
import { TerminalPanels } from "../lib/terminalPanel";
import type { WorktreeRecoveryApi } from "../lib/worktreeRecovery";
import type { WorktreeSetupApi } from "../lib/worktreeSetup";

export function mountSetupRecoveryPreview(container: HTMLElement) {
  const chat = chatSummary("setup-chat", {
    agent_id: "mock",
    agent_name: "Demo agent",
    status: "stopped",
    folder: "C:/fixture/worktrees/setup",
  });
  const calls: string[] = [];
  let attempt: WorktreeSetupAttempt = {
    worktree_id: "setup-tree",
    generation: 1,
    state: "awaiting",
    detail: null,
  };
  let queue: QueuedPrompt[] = [];
  let sequence = 0;
  let promptSequence = 0;
  let status: "stopped" | "ready" = "stopped";
  let receive: ((batch: ReturnType<typeof frame>) => void) | null = null;
  let terminalRead: ((chunk: TerminalChunk) => void) | null = null;
  const info = () =>
    sessionInfo({
      status,
      worktree_setup: { ...attempt },
      queue_held: queue.some((row) => row.held),
      queue_wait: queue.length
        ? attempt.state !== "succeeded"
          ? { reason: "setup", state: attempt.state, detail: attempt.detail }
          : { reason: "held", after: "error" }
        : null,
    });
  const publish = (events: GhostEvent[] = []) => {
    const all = [...events, ev.session(info())];
    receive?.(frame(chat.id, sequence, all));
    sequence += all.length;
  };
  const release = () => {
    if (attempt.state !== "succeeded") return;
    while (queue[0] && !queue[0].held) {
      const row = queue.shift();
      if (!row) break;
      calls.push(`prompt:${row.preview}`);
      publish([ev.lifecycle(row.prompt_id, "started")]);
    }
  };
  const holdQueue = () => {
    const events: GhostEvent[] = [];
    queue = queue.map((row) => {
      if (row.held) return row;
      events.push(ev.lifecycle(row.prompt_id, "queued", row.preview, true));
      return { ...row, held: true };
    });
    return events;
  };
  const ownerComplete = (code: number, generation = attempt.generation) => {
    if (generation !== attempt.generation || attempt.state !== "running") return;
    attempt = {
      ...attempt,
      state: code === 0 ? "succeeded" : "failed",
      detail: code === 0 ? null : `Setup exited with code ${code}. Retry setup.`,
    };
    if (code === 0) {
      status = "ready";
      release();
    }
    publish(code === 0 ? [] : holdQueue());
    terminalRead?.({ bytes: [], done: true, exit_code: code, truncated: false });
    terminalRead = null;
  };
  const setup: WorktreeSetupApi = {
    read: async () => ({
      source: "C:/fixture/source",
      input: { files: [".env"], command: { program: "node", args: ["setup.js"] } },
    }),
    save: async (_chat, input) => ({ source: "C:/fixture/source", input }),
    preview: async (_chat, worktreeId, commandOnly) => {
      calls.push(`setup-preview:${commandOnly ? "command-only" : "files"}`);
      return {
        token: "review",
        worktree_id: worktreeId,
        source: "C:/fixture/source",
        destination: chat.folder,
        files: [],
        command: { program: "node", args: ["setup.js"] },
        warnings: ["This fixture runs no program."],
        expires_at: Date.now() + 600000,
      };
    },
    apply: async () => {
      calls.push("setup-apply");
      attempt = { ...attempt, generation: attempt.generation + 1, state: "awaiting", detail: null };
      publish();
      return { copied: [], command_token: `command-${attempt.generation}`, warning: null };
    },
    terminal: () => ({
      reserve: async () => "setup-terminal",
      start: async () => {
        calls.push("setup-start");
        attempt = { ...attempt, state: "running" };
        publish();
        return "setup-terminal";
      },
      read: () =>
        new Promise((resolve) => {
          terminalRead = resolve;
        }),
      write: async () => {},
      resize: async () => {},
      close: async () => {
        calls.push("setup-close");
        if (attempt.state === "running") ownerComplete(1);
        terminalRead?.({ bytes: [], done: true, exit_code: null, truncated: false });
      },
    }),
  };
  const transport = Object.assign(
    new FakeTransport({
      attach: async (_chat, onFrame) => {
        receive = onFrame;
        return attachResult(chat.id, sequence, historyPage(chat.id, []), {
          chat: { ...chat, status },
          session: info(),
          queue: [...queue],
        });
      },
      send: async (_chat, text) => {
        const prompt_id = `queued-${++promptSequence}`;
        const row = {
          prompt_id,
          preview: text,
          held: attempt.state === "failed" || attempt.state === "interrupted",
          created_at: Date.now(),
        };
        queue.push(row);
        publish([ev.lifecycle(prompt_id, "queued", text, row.held)]);
        release();
        return { prompt_id, queued: true };
      },
      queuedText: async (_chat, id) => queue.find((row) => row.prompt_id === id)?.preview ?? "",
      editQueued: async (_chat, id, text) => {
        queue = queue.map((row) => (row.prompt_id === id ? { ...row, preview: text } : row));
        return [...queue];
      },
      moveQueued: async (_chat, id, index) => {
        const from = queue.findIndex((row) => row.prompt_id === id);
        const row = queue.splice(from, 1)[0];
        if (row) queue.splice(index, 0, row);
        return [...queue];
      },
      start: async () => {
        calls.push("explicit-start");
        if (attempt.state !== "succeeded") throw new Error("Setup must succeed first.");
      },
      resumeQueue: async () => {
        queue = queue.map((row) => ({ ...row, held: false }));
        release();
        publish();
      },
      sendNow: async (_chat, id) => {
        const from = queue.findIndex((row) => row.prompt_id === id);
        const row = from < 0 ? undefined : queue.splice(from, 1)[0];
        if (!row) throw new Error("This queued message is no longer available.");
        queue.unshift({ ...row, held: false });
        release();
        publish();
        return [...queue];
      },
    }),
    { setup },
  );
  const projectTools: ProjectToolsApi = {
    review: async () => {
      throw new Error("No shell is needed for this fixture.");
    },
    terminalReview: async () => {
      throw new Error("No shell is needed for this fixture.");
    },
    openEditor: async () => {},
    terminal: () => {
      throw new Error("No shell is needed for this fixture.");
    },
  };
  const terminals = new TerminalPanels(projectTools, createXtermRuntime);
  const retained: ManagedWorktree = {
    id: "orphan-tree",
    chat_id: "never-saved",
    source: "C:/fixture/source",
    path: "C:/fixture/retained",
    branch: "ghost/retained",
    base_commit: "a".repeat(40),
    state: "recovery_required",
    detail: "The chat could not be saved and rollback retained changed files.",
    recovery: { type: "create" },
    created_at: 1,
  };
  let record = retained;
  const recovery: WorktreeRecoveryApi = {
    list: async () => [record],
    open: async (id) => {
      calls.push(`open-folder:${id}`);
    },
    preview: async (id, action) => {
      calls.push(`recovery-review:${id}`);
      return {
        token: "recovery-review",
        worktree: record,
        action,
        folder: {
          root: record.path,
          branch: record.branch,
          head: record.base_commit,
          files: [],
          blockers: [],
          has_submodules: false,
          is_linked_worktree: true,
        },
        target: null,
        destination: record.path,
        warnings: ["All retained files stay in place."],
        expires_at: Date.now() + 600000,
      };
    },
    recreate: async () => {
      throw new Error("The retained folder exists.");
    },
    apply: async (id) => {
      calls.push(`recovery-apply:${id}`);
      record = { ...record, state: "ready", detail: null };
      return { worktree: record, recovery: record.path, commit: null, error: null };
    },
  };
  Object.assign(window, {
    setupRecoveryFixture: {
      calls: () => [...calls],
      complete: ownerComplete,
      restart: () => {
        attempt = {
          ...attempt,
          state: "interrupted",
          detail: "Setup was interrupted by a restart.",
        };
        publish(holdQueue());
      },
      queue: () => [...queue],
    },
  });
  createRoot(container).render(
    <Fixture
      record={record}
      getRecord={() => record}
      chat={chat}
      recovery={recovery}
      transport={transport}
      terminals={terminals}
    />,
  );
}

function Fixture({
  record,
  getRecord,
  chat,
  recovery,
  transport,
  terminals,
}: {
  readonly record: ManagedWorktree;
  readonly getRecord: () => ManagedWorktree;
  readonly chat: ChatSummary;
  readonly recovery: WorktreeRecoveryApi;
  readonly transport: Transport;
  readonly terminals: TerminalPanels;
}) {
  const [records, setRecords] = useState([record]);
  return (
    <main
      className="g-root"
      style={{ height: "100vh", display: "grid", gridTemplateColumns: "320px 1fr" }}
    >
      <aside style={{ overflowY: "auto" }}>
        <h1 className="sr-only">Setup and recovery fixture</h1>
        <WorktreeRecoveryList
          records={records}
          api={recovery}
          onChanged={() => setRecords([getRecord()])}
        />
      </aside>
      <ChatView
        chatId={chat.id}
        transport={transport}
        listed={chat}
        storage={null}
        agents={[]}
        terminals={terminals}
      />
    </main>
  );
}
