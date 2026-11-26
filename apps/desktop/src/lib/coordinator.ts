import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  type ContextFile,
  type CoordinationMember,
  type CoordinatorAction,
  type CoordinatorChanged,
  type CoordinatorSnapshot,
  type CoordinatorStart,
  type CoordinatorTask,
  type CoordinatorTaskStatus,
  commands,
} from "../bindings";
import { errorMessage } from "./errors";
import { inTauri, settle } from "./ipc";

/**
 * Coordinated projects (an experiment), as Rust exposes them (src-tauri/src/coordinator_commands.rs).
 * Everything here is the person acting in the window. A coordinator's or a worker's own changes
 * come through its chat's tools instead, and reach the page through `subscribe`.
 */
export interface CoordinatorApi {
  snapshot(projectId: string): Promise<CoordinatorSnapshot>;
  /** Every coordinator and worker chat, for the sidebar's marks. */
  members(): Promise<CoordinationMember[]>;
  /** One Context file: as it reads now, or at one of its kept revisions. */
  file(projectId: string, path: string, revision: string | null): Promise<ContextFile>;
  start(input: CoordinatorStart): Promise<CoordinatorSnapshot>;
  /** One change; answers the project as it reads afterwards. */
  apply(projectId: string, action: CoordinatorAction): Promise<CoordinatorSnapshot>;
  /** Calls `changed` after every committed change, the page's or an agent's. */
  subscribe(changed: (change: CoordinatorChanged) => void): () => void;
}

/** The window event Rust sends after each committed change (coordinator_commands.rs). */
export const COORDINATOR_CHANGED = "auracode-coordinator-changed";

function subscribeCoordinator(changed: (change: CoordinatorChanged) => void): () => void {
  if (!inTauri()) return () => {};
  let disposed = false;
  let stop: UnlistenFn | null = null;
  void listen<CoordinatorChanged>(COORDINATOR_CHANGED, ({ payload }) => {
    if (!disposed) changed(payload);
  })
    .then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    })
    .catch(() => {});
  return () => {
    disposed = true;
    stop?.();
    stop = null;
  };
}

export const coordinatorApi: CoordinatorApi = {
  snapshot: (projectId) => settle(commands.coordinatorSnapshot(projectId)),
  members: () => settle(commands.coordinatorMembers()),
  file: (projectId, path, revision) => settle(commands.coordinatorFile(projectId, path, revision)),
  start: (input) => settle(commands.coordinatorStart(input)),
  apply: (projectId, action) => settle(commands.coordinatorApply(projectId, action)),
  subscribe: subscribeCoordinator,
};

/** The limits Rust holds every writer to (`ghost_protocol::coordinator::limits`), for the page's meters. */
export const CONTEXT_FILE_BYTES = 64 * 1024;
export const DECISION_BYTES = 2 * 1024;
export const BRIEF_BYTES = 8 * 1024;
export const ROUTINE_MIN_MINUTES = 5;
export const ROUTINE_MAX_MINUTES = 7 * 24 * 60;

export const TASK_COLUMNS: readonly { status: CoordinatorTaskStatus; name: string }[] = [
  { status: "todo", name: "To do" },
  { status: "doing", name: "Doing" },
  { status: "review", name: "Review" },
  { status: "done", name: "Done" },
];

/** A task's name everywhere: `T-12`. */
export function taskName(task: Pick<CoordinatorTask, "number">): string {
  return `T-${task.number}`;
}

/** Who wrote something, as the page says it: "you", "the coordinator" or a worker's name. */
export function sourceName(source: string): string {
  if (source === "you") return "You";
  if (source === "coordinator") return "Coordinator";
  if (source.startsWith("worker:")) return source.slice("worker:".length);
  return source;
}

/** How often a routine runs, in words: "every 5 minutes", "every 2 hours", "every day". */
export function everyWords(minutes: number): string {
  if (minutes % (24 * 60) === 0) {
    const days = minutes / (24 * 60);
    return days === 1 ? "every day" : days === 7 ? "every week" : `every ${days} days`;
  }
  if (minutes % 60 === 0) {
    const hours = minutes / 60;
    return hours === 1 ? "every hour" : `every ${hours} hours`;
  }
  return `every ${minutes} minutes`;
}

export const ROUTINE_PRESETS: readonly { minutes: number; name: string }[] = [
  { minutes: 5, name: "Every 5 minutes" },
  { minutes: 15, name: "Every 15 minutes" },
  { minutes: 60, name: "Every hour" },
  { minutes: 6 * 60, name: "Every 6 hours" },
  { minutes: 24 * 60, name: "Every day" },
  { minutes: 7 * 24 * 60, name: "Every week" },
];

export interface ProjectCoordination {
  readonly snapshot: CoordinatorSnapshot | null;
  /** What the last read or change said when it failed, until the next one. */
  readonly error: string | null;
  /** A change this page asked for is still running. */
  readonly busy: boolean;
  /** Applies one change and shows the project as it reads afterwards. False when it was refused. */
  readonly apply: (action: CoordinatorAction) => Promise<boolean>;
  readonly start: (input: Omit<CoordinatorStart, "project_id">) => Promise<boolean>;
  readonly clearError: () => void;
}

/**
 * A project's coordination as its page follows it: read when the page opens, again after every
 * committed change to this project (an agent's included), and when the window regains focus.
 */
export function useProjectCoordination(
  api: CoordinatorApi,
  projectId: string,
): ProjectCoordination {
  const [snapshot, setSnapshot] = useState<CoordinatorSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  // A read that returns after a newer one started is dropped.
  const reads = useRef(0);

  const read = useCallback(() => {
    const mine = ++reads.current;
    api.snapshot(projectId).then(
      (next) => {
        if (alive.current && mine === reads.current) setSnapshot(next);
      },
      (failure) => {
        if (alive.current && mine === reads.current) setError(errorMessage(failure));
      },
    );
  }, [api, projectId]);

  useEffect(() => {
    alive.current = true;
    setSnapshot(null);
    setError(null);
    read();
    const stop = api.subscribe((change) => {
      if (change.project_id === "" || change.project_id === projectId) read();
    });
    window.addEventListener("focus", read);
    return () => {
      alive.current = false;
      stop();
      window.removeEventListener("focus", read);
    };
  }, [api, projectId, read]);

  const run = useCallback(async (work: () => Promise<CoordinatorSnapshot>) => {
    setBusy(true);
    setError(null);
    try {
      const next = await work();
      reads.current++;
      if (alive.current) setSnapshot(next);
      return true;
    } catch (failure) {
      if (alive.current) setError(errorMessage(failure));
      return false;
    } finally {
      if (alive.current) setBusy(false);
    }
  }, []);

  const apply = useCallback(
    (action: CoordinatorAction) => run(() => api.apply(projectId, action)),
    [api, projectId, run],
  );
  const start = useCallback(
    (input: Omit<CoordinatorStart, "project_id">) =>
      run(() => api.start({ ...input, project_id: projectId })),
    [api, projectId, run],
  );
  const clearError = useCallback(() => setError(null), []);
  return { snapshot, error, busy, apply, start, clearError };
}

/** Every coordinator and worker chat by id, kept current, for the sidebar. Empty while off. */
export function useCoordinationMembers(
  api: CoordinatorApi,
  enabled: boolean,
): ReadonlyMap<string, CoordinationMember> {
  const [members, setMembers] = useState<ReadonlyMap<string, CoordinationMember>>(new Map());
  useEffect(() => {
    if (!enabled) {
      setMembers(new Map());
      return;
    }
    let live = true;
    const read = () => {
      api.members().then(
        (found) => {
          if (live) setMembers(new Map(found.map((member) => [member.chat_id, member])));
        },
        () => undefined,
      );
    };
    read();
    const stop = api.subscribe(read);
    return () => {
      live = false;
      stop();
    };
  }, [api, enabled]);
  return members;
}
