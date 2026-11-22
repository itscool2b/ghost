import {
  type ChatMove,
  type ChatRole,
  commands,
  type ProjectChatCreate,
  type ProjectCreate,
  type ProjectInput,
  type RoleModel,
  type WorkspaceSnapshot,
} from "../bindings";
import { errorMessage } from "./errors";
import { settle } from "./ipc";

/** Presentation transport only. Rust owns validation, persistence and session/folder safeguards. */
export const workspaceApi = {
  snapshot: () => settle(commands.workspaceSnapshot({ include_archived: true, project_id: null })),
  createProject: (request: ProjectCreate) => settle(commands.projectCreate(request)),
  updateProject: (id: string, input: ProjectInput) => settle(commands.projectUpdate(id, input)),
  addFolder: (id: string, path: string) => settle(commands.projectAddFolder(id, path)),
  rename: (id: string, title: string) => settle(commands.chatRename(id, title)),
  pin: (id: string, pinned: boolean) => settle(commands.chatPin(id, pinned)),
  /** A chat's Plan with or Build with, or none to clear it. */
  setRole: (id: string, role: ChatRole, choice: RoleModel | null) =>
    settle(commands.chatSetRole(id, role, choice)),
  archive: (id: string, archived: boolean) => settle(commands.chatArchive(id, archived)),
  move: (id: string, request: ChatMove) => settle(commands.chatMove(id, request)),
  preview: (folderId: string) => settle(commands.chatCreatePreview(folderId)),
  /** What a picked folder already is: its projects (Rust compares canonical folders) and
   * whether it is a managed worktree. */
  folderMatches: (path: string) => settle(commands.workspaceFolderMatches(path)),
  createChat: (request: ProjectChatCreate) => settle(commands.chatProjectCreate(request)),
};
export type WorkspaceApi = typeof workspaceApi;
export interface WorkspaceView {
  readonly data: WorkspaceSnapshot | null;
  /**
   * `data` is the workspace this profile last showed (lib/launchCache.ts), there for the window's
   * first frame until the database answers. Nothing may take it as proof of what exists.
   */
  readonly cached: boolean;
  /** Why the last read of the workspace failed. An operation's own error goes to its caller. */
  readonly error: string | null;
  /** Operations running now, from every screen. None waits for another here. */
  readonly pending: number;
}
export const EMPTY_WORKSPACE: WorkspaceView = {
  data: null,
  cached: false,
  error: null,
  pending: 0,
};

/** How one operation ended, for the caller that started it only. */
export type WorkspaceOutcome<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: string };

/** The waits before reading the workspace again after a failed read; the last repeats. */
export const WORKSPACE_RETRY_MS = [500, 1000, 2000, 4000, 8000] as const;

export class WorkspaceStore {
  readonly api: WorkspaceApi;
  #state: WorkspaceView = EMPTY_WORKSPACE;
  #listeners = new Set<() => void>();
  #revision = 0;
  /** Failed reads in a row, and the read waiting to try again. */
  #failures = 0;
  #retry: ReturnType<typeof setTimeout> | null = null;
  #pending = 0;
  /** The newest read, which a superseded one waits for. */
  #latest: Promise<void> = Promise.resolve();
  readonly #waits: readonly number[];
  constructor(api: WorkspaceApi, waits: readonly number[] = WORKSPACE_RETRY_MS) {
    this.api = api;
    this.#waits = waits;
  }
  subscribe = (listener: () => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };
  getSnapshot = () => this.#state;
  #publish(state: WorkspaceView) {
    this.#state = state;
    for (const listener of this.#listeners) listener();
  }
  /**
   * Shows the workspace this profile last showed, before anything was read. The first read
   * replaces it; a workspace already read is never replaced by it.
   */
  hydrate(data: WorkspaceSnapshot): void {
    if (this.#state.data !== null) return;
    this.#publish({ ...this.#state, data, cached: true });
  }

  /** Reads the workspace. A read started later answers for this one, so it never lands stale. */
  refresh(): Promise<void> {
    const revision = ++this.#revision;
    if (this.#retry !== null) {
      clearTimeout(this.#retry);
      this.#retry = null;
    }
    const read = this.#read(revision);
    this.#latest = read;
    return read;
  }

  async #read(revision: number): Promise<void> {
    try {
      const data = await this.api.snapshot();
      if (revision !== this.#revision) return this.#latest;
      this.#failures = 0;
      this.#publish({ ...this.#state, data, cached: false, error: null });
    } catch (error) {
      if (revision !== this.#revision) return this.#latest;
      this.#publish({ ...this.#state, error: errorMessage(error) });
      // Read again until it answers: the database may still be opening after a restart.
      const wait = this.#waits[Math.min(this.#failures, this.#waits.length - 1)] ?? 8000;
      this.#failures += 1;
      this.#retry = setTimeout(() => {
        this.#retry = null;
        if (revision === this.#revision) void this.refresh();
      }, wait);
    }
  }

  /** Stops a waiting retry (the profile closed). */
  dispose(): void {
    if (this.#retry !== null) clearTimeout(this.#retry);
    this.#retry = null;
    this.#revision += 1;
    this.#latest = Promise.resolve();
  }
  /**
   * Runs `operation` at once, beside any other: nothing is dropped or made to wait here, because
   * Rust orders the changes that must not overlap. The caller gets its own value or error. The
   * workspace is read again either way, so a change made before a failure still shows.
   */
  async perform<T>(operation: () => Promise<T>): Promise<WorkspaceOutcome<T>> {
    this.#pending += 1;
    this.#publish({ ...this.#state, pending: this.#pending });
    try {
      const value = await operation();
      await this.refresh();
      return { ok: true, value };
    } catch (error) {
      void this.refresh();
      return { ok: false, error: errorMessage(error) };
    } finally {
      this.#pending -= 1;
      this.#publish({ ...this.#state, pending: this.#pending });
    }
  }
}
