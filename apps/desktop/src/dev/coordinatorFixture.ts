/**
 * A synthetic coordinated project for previews (`?preview=coordinator`, `&case=empty`,
 * `&case=paused`). Nothing is stored and no agent runs: the project lives in memory, and each
 * change is applied the way Rust applies it (`crates/ghost-core/src/chatstore/coordinator.rs`),
 * with the same refusals where the page can reach them.
 *
 * `window.coordinatorFixture` lets a check act as an agent: `agentWrites(path, text)` replaces a
 * Context file as a worker would, and `calls()` lists what the page asked for.
 */
import type {
  ContextFile,
  ContextRevision,
  CoordinationMember,
  CoordinatorAction,
  CoordinatorChanged,
  CoordinatorSnapshot,
  CoordinatorStart,
} from "../bindings";
import type { CoordinatorApi } from "../lib/coordinator";

const MINUTE = 60_000;
export const PROJECT_ID = "project-1";
export const COORDINATOR_CHAT = "chat-coordinator";
export const WORKER_CHATS = ["chat-worker-api", "chat-worker-docs", "chat-worker-old"] as const;

interface StoredFile {
  content: string;
  revision: string;
  source: string;
  updated_at: number;
  history: (ContextRevision & { content: string | null })[];
}

declare global {
  interface Window {
    coordinatorFixture?: {
      calls(): string[];
      snapshot(): CoordinatorSnapshot;
      /** Replaces (or creates) a Context file as the worker `api` would. */
      agentWrites(path: string, content: string): void;
    };
  }
}

export function fakeCoordinator(kind: string | null): CoordinatorApi {
  const now = Date.now();
  const empty = kind === "empty";
  let revisions = 0;
  const nextRevision = () => `rev-${++revisions}`;
  const files = new Map<string, StoredFile>();
  const put = (path: string, content: string | null, source: string, at = Date.now()) => {
    const revision = nextRevision();
    const stored = files.get(path);
    const history = [
      {
        revision,
        source,
        bytes: new TextEncoder().encode(content ?? "").length,
        created_at: at,
        content,
      },
      ...(stored?.history ?? []),
    ].slice(0, 20);
    if (content === null) files.delete(path);
    else files.set(path, { content, revision, source, updated_at: at, history });
    return revision;
  };
  let state: Omit<CoordinatorSnapshot, "files"> = {
    project_id: PROJECT_ID,
    coordinator_chat_id: empty ? null : COORDINATOR_CHAT,
    brief: empty ? "" : "Move the billing service off the old queue without a minute of downtime.",
    paused: kind === "paused",
    pause_reason:
      kind === "paused"
        ? "Ghost paused this project: its coordinator was sent 60 reports and routines within an hour. Look at what the agents are doing, then resume."
        : null,
    workers: empty
      ? []
      : [
          {
            chat_id: WORKER_CHATS[0],
            name: "api-tests",
            task: 2,
            report: null,
            turns: 0,
            pending: false,
            closed: false,
            created_at: now - 9 * MINUTE,
            reported_at: null,
          },
          {
            chat_id: WORKER_CHATS[1],
            name: "docs",
            task: 3,
            report:
              "Rewrote docs/queue.md for the new queue and added the rollback steps. Checked every command against staging. Left: the diagram, which needs the final topic names from T-2.",
            turns: 2,
            pending: false,
            closed: false,
            created_at: now - 42 * MINUTE,
            reported_at: now - 6 * MINUTE,
          },
          {
            chat_id: WORKER_CHATS[2],
            name: "inventory",
            task: 1,
            report: "Listed every producer and consumer of the old queue in research/producers.md.",
            turns: 1,
            pending: false,
            closed: true,
            created_at: now - 190 * MINUTE,
            reported_at: now - 170 * MINUTE,
          },
        ],
    tasks: empty
      ? []
      : [
          {
            number: 4,
            title: "Cut the consumers over behind a flag",
            body: "One consumer at a time. Done when the old queue has had no reads for an hour.",
            status: "todo",
            tags: ["risky"],
            worker: null,
            link: null,
            source: "coordinator",
            updated_at: now - 30 * MINUTE,
          },
          {
            number: 5,
            title: "Delete the old queue's configuration",
            body: "",
            status: "todo",
            tags: [],
            worker: null,
            link: null,
            source: "you",
            updated_at: now - 12 * MINUTE,
          },
          {
            number: 2,
            title: "Contract tests for the new queue's producers",
            body: "Cover every producer in research/producers.md.",
            status: "doing",
            tags: ["api"],
            worker: "api-tests",
            link: null,
            source: "coordinator",
            updated_at: now - 9 * MINUTE,
          },
          {
            number: 3,
            title: "Rewrite the queue runbook",
            body: "",
            status: "review",
            tags: ["docs"],
            worker: "docs",
            link: "https://github.com/example/billing/pull/412",
            source: "worker:docs",
            updated_at: now - 6 * MINUTE,
          },
          {
            number: 1,
            title: "List every producer and consumer of the old queue",
            body: "",
            status: "done",
            tags: [],
            worker: "inventory",
            link: null,
            source: "coordinator",
            updated_at: now - 170 * MINUTE,
          },
        ],
    decisions: empty
      ? []
      : [
          {
            number: 3,
            text: "Workers open draft pull requests; nothing merges without the person.",
            source: "coordinator",
            retracted: false,
            created_at: now - 40 * MINUTE,
          },
          {
            number: 2,
            text: "No dual writes: cut over one consumer at a time behind a flag. Dual writes were turned down, since the two queues order differently.",
            source: "you",
            retracted: false,
            created_at: now - 150 * MINUTE,
          },
          {
            number: 1,
            text: "TypeScript for every new service file, not Python: the rest of billing is TypeScript.",
            source: "you",
            retracted: false,
            created_at: now - 180 * MINUTE,
          },
        ],
    routines: empty
      ? []
      : [
          {
            id: "routine-1",
            title: "Watch the pull requests",
            prompt:
              "Check this project's open pull requests. Fix failing checks, answer review comments, and tell me only what needs me.",
            every_minutes: 15,
            enabled: true,
            next_at: now + 6 * MINUTE,
            last_at: now - 9 * MINUTE,
          },
        ],
  };
  let nextTask = 6;
  let nextDecision = 4;
  if (!empty) {
    put(
      "how-to-test.md",
      "# How to test\n\nRun `pnpm test --filter billing` from the repository root. The queue tests need Docker running.\n",
      "worker:inventory",
      now - 160 * MINUTE,
    );
    put(
      "research/producers.md",
      "# Producers and consumers of the old queue\n\n- invoices: produces `invoice.created`\n- dunning: consumes `invoice.overdue`\n",
      "worker:inventory",
      now - 170 * MINUTE,
    );
    put(
      "preferences.md",
      "# How the work should be done\n\nSmall pull requests, one per task. Never touch the payments module.\n",
      "you",
      now - 175 * MINUTE,
    );
  }

  const calls: string[] = [];
  const listeners = new Set<(change: CoordinatorChanged) => void>();
  const changed = () => {
    for (const listener of listeners) listener({ project_id: PROJECT_ID });
  };
  const summary = (content: string) =>
    (
      content
        .split("\n")
        .map((line) => line.trim().replace(/^#+/u, "").trim())
        .find(Boolean) ?? ""
    ).slice(0, 80);
  const snapshot = (): CoordinatorSnapshot =>
    structuredClone({
      ...state,
      files: [...files.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([path, file]) => ({
          path,
          revision: file.revision,
          bytes: new TextEncoder().encode(file.content).length,
          summary: summary(file.content),
          source: file.source,
          updated_at: file.updated_at,
        })),
    });
  const refuse = (message: string): never => {
    throw new Error(message);
  };
  const pathRule = /^[A-Za-z0-9][A-Za-z0-9._-]*(\/[A-Za-z0-9][A-Za-z0-9._-]*){0,3}$/u;

  const apply = (action: CoordinatorAction) => {
    const at = Date.now();
    switch (action.type) {
      case "pause":
        state = { ...state, paused: action.paused, pause_reason: null };
        break;
      case "brief":
        state = { ...state, brief: action.text };
        break;
      case "task_save": {
        if (action.link && !/^https?:\/\/\S+$/u.test(action.link))
          refuse("A task's link is one http or https address of at most 400 characters.");
        if (action.number === null) {
          if (!action.title?.trim())
            refuse("A task needs a one-line title of at most 200 characters.");
          state = {
            ...state,
            tasks: [
              ...state.tasks,
              {
                number: nextTask++,
                title: action.title?.trim() ?? "",
                body: action.body ?? "",
                status: action.status ?? "todo",
                tags: action.tags ?? [],
                worker: null,
                link: action.link || null,
                source: "you",
                updated_at: at,
              },
            ],
          };
        } else {
          state = {
            ...state,
            tasks: state.tasks.map((task) =>
              task.number === action.number
                ? {
                    ...task,
                    title: action.title?.trim() ?? task.title,
                    body: action.body ?? task.body,
                    status: action.status ?? task.status,
                    tags: action.tags ?? task.tags,
                    link: action.link === null ? task.link : action.link || null,
                    source: "you",
                    updated_at: at,
                  }
                : task,
            ),
          };
        }
        break;
      }
      case "task_move": {
        const moved = state.tasks.find((task) => task.number === action.number);
        if (!moved) refuse(`There is no task T-${action.number} in this project.`);
        const rest = state.tasks.filter((task) => task.number !== action.number);
        const before = rest.findIndex((task) => task.number === action.before);
        const next = {
          ...(moved as (typeof state.tasks)[number]),
          status: action.status,
          source: "you",
          updated_at: at,
        };
        if (before < 0) rest.push(next);
        else rest.splice(before, 0, next);
        state = { ...state, tasks: rest };
        break;
      }
      case "task_delete":
        state = { ...state, tasks: state.tasks.filter((task) => task.number !== action.number) };
        break;
      case "file_save": {
        if (!pathRule.test(action.path) || !action.path.endsWith(".md"))
          refuse(
            "Name a Context file like notes.md or research/api.md: letters, digits, dots, dashes and underscores, at most four folders deep and 120 characters, ending in .md.",
          );
        const stored = files.get(action.path);
        if (!stored && action.expected_revision !== null)
          refuse(
            `${action.path} does not exist any more. Create it by writing with no base revision.`,
          );
        if (stored && action.expected_revision === null)
          refuse(
            `${action.path} already exists at revision ${stored.revision}, written by ${stored.source}. Read it, then write with that revision as the base, or append to it.`,
          );
        if (stored && stored.revision !== action.expected_revision)
          refuse(
            `${action.path} changed since revision ${action.expected_revision}: it is now revision ${stored.revision}, written by ${stored.source}. Read it again and apply your change to the current text, or add to its end with an append, which never conflicts.`,
          );
        put(action.path, action.content, "you", at);
        break;
      }
      case "file_append": {
        const stored = files.get(action.path);
        put(action.path, `${stored?.content ?? ""}${action.text}\n`, "you", at);
        break;
      }
      case "file_delete":
        put(action.path, null, "you", at);
        break;
      case "file_restore": {
        const old = files
          .get(action.path)
          ?.history.find((entry) => entry.revision === action.revision);
        if (!old || old.content === null) refuse("That revision of the file is not kept any more.");
        put(action.path, old?.content ?? "", "you", at);
        break;
      }
      case "decision_add":
        state = {
          ...state,
          decisions: [
            {
              number: nextDecision++,
              text: action.text.trim(),
              source: "you",
              retracted: false,
              created_at: at,
            },
            ...state.decisions,
          ],
        };
        break;
      case "decision_retract":
        state = {
          ...state,
          decisions: state.decisions.map((decision) =>
            decision.number === action.number ? { ...decision, retracted: true } : decision,
          ),
        };
        break;
      case "routine_save":
        if (action.every_minutes < 5)
          refuse("A routine runs every 5 minutes at the most often and once a week at the least.");
        state = {
          ...state,
          routines:
            action.id === null
              ? [
                  ...state.routines,
                  {
                    id: `routine-${state.routines.length + 2}`,
                    title: action.title,
                    prompt: action.prompt,
                    every_minutes: action.every_minutes,
                    enabled: action.enabled,
                    next_at: at + action.every_minutes * MINUTE,
                    last_at: null,
                  },
                ]
              : state.routines.map((routine) =>
                  routine.id === action.id
                    ? {
                        ...routine,
                        title: action.title,
                        prompt: action.prompt,
                        every_minutes: action.every_minutes,
                        enabled: action.enabled,
                      }
                    : routine,
                ),
        };
        break;
      case "routine_delete":
        state = {
          ...state,
          routines: state.routines.filter((routine) => routine.id !== action.id),
        };
        break;
      case "worker_close":
        state = {
          ...state,
          workers: state.workers.map((worker) =>
            worker.chat_id === action.chat_id ? { ...worker, closed: true } : worker,
          ),
        };
        break;
    }
  };

  window.coordinatorFixture = {
    calls: () => [...calls],
    snapshot,
    agentWrites: (path, content) => {
      put(path, content, "worker:api-tests");
      changed();
    },
  };
  const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
  return {
    snapshot: async () => snapshot(),
    members: async (): Promise<CoordinationMember[]> => [
      ...(state.coordinator_chat_id
        ? [
            {
              chat_id: state.coordinator_chat_id,
              project_id: PROJECT_ID,
              role: "coordinator" as const,
              name: "coordinator",
            },
          ]
        : []),
      ...state.workers.map((worker) => ({
        chat_id: worker.chat_id,
        project_id: PROJECT_ID,
        role: "worker" as const,
        name: worker.name,
      })),
    ],
    file: async (_project, path, revision): Promise<ContextFile> => {
      const stored = files.get(path);
      const history = (stored?.history ?? []).map(({ content: _content, ...entry }) => entry);
      if (revision !== null) {
        const old = stored?.history.find((entry) => entry.revision === revision);
        if (!old || old.content === null)
          throw new Error("That revision of the file is not kept any more.");
        return {
          path,
          revision,
          content: old.content,
          source: old.source,
          updated_at: old.created_at,
          history,
        };
      }
      if (!stored) throw new Error(`There is no Context file ${path}.`);
      return {
        path,
        revision: stored.revision,
        content: stored.content,
        source: stored.source,
        updated_at: stored.updated_at,
        history,
      };
    },
    start: async (input: CoordinatorStart) => {
      calls.push(`start:${input.agent_id}:${input.folder_id}`);
      await pause(150);
      if (!input.brief.trim()) throw new Error("Say what the project is for.");
      state = { ...state, coordinator_chat_id: COORDINATOR_CHAT, brief: input.brief };
      changed();
      return snapshot();
    },
    apply: async (_project, action) => {
      calls.push(action.type);
      await pause(40);
      apply(action);
      changed();
      return snapshot();
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
