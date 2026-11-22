import type {
  AgentAuthStatus,
  AgentEntry,
  AgentMaintenance,
  ChatStatusUpdate,
  ChatSummary,
  EnvStatus,
  StorageStatus,
} from "../../bindings";
import { errorMessage } from "../errors";
import type { NewChatContext } from "./newChatContext";
import type { Transport } from "./transport";

/**
 * What the whole window knows about chats, as an external store for React: the agents, the
 * environment agents start with, the chat list (kept current by the status channel), and the
 * storage status. Plain classes, so M2a can move them into Zustand slices unchanged.
 *
 * The storage status is polled (every 2 s while saving is paused or failed, every 5 s otherwise),
 * and so are the env while it resolves and the agents while one is still being checked: none of
 * them has a channel of its own.
 *
 * A status update carries no folder state, so the list reloads when a chat's start fails (the
 * folder may be missing or not allowed) and when a chat listed with a folder problem gets ready
 * (its start got past the folder check). Updates that arrive while a reload is on its way are
 * applied again on top of its answer, and only the newest reload's answer counts.
 */

export interface AppSnapshot {
  readonly agents: readonly AgentEntry[] | null;
  readonly agentsRefreshing: boolean;
  readonly env: EnvStatus | null;
  readonly envError: string | null;
  /** Newest activity first. */
  readonly chats: readonly ChatSummary[] | null;
  /**
   * `chats` is the list this profile last showed (lib/launchCache.ts), there for the window's
   * first frame until the database answers. It reads as the real list will; nothing may take it
   * as proof that a chat exists.
   */
  readonly chatsCached: boolean;
  readonly chatsError: string | null;
  readonly storage: StorageStatus | null;
}

/** Storage is polled this often while it saves, and `STORAGE_ALERT_POLL_MS` while it does not. */
export const STORAGE_POLL_MS = 5000;
export const STORAGE_ALERT_POLL_MS = 2000;
/** The env and the agents are polled this often while they are still being worked out. */
export const DETECTION_POLL_MS = 1000;
/**
 * A release check still running keeps the agents polled at `DETECTION_POLL_MS` this long after a
 * start only: startup's checks finish well within it (ten seconds per agent at most), while an
 * agent installed later waits for the 15-minute release round and needs no fast polling.
 */
export const RELEASE_POLL_WINDOW_MS = 120_000;
/** Release metadata arrives independently of initial installation detection. */
export const CATALOG_POLL_MS = 60_000;
/**
 * A chat list that could not be read is read again after these waits, then every last one, until
 * it is: just after an update or a restart the database is still opening (migrations, the
 * integrity check), and the list must not stay empty when it would answer a moment later.
 */
export const CHATS_RETRY_MS = [500, 1000, 2000, 4000, 8000] as const;
/** A status channel that ended is subscribed again after this long. */
export const STATUS_RESUBSCRIBE_MS = 1000;

const EMPTY: AppSnapshot = {
  agents: null,
  agentsRefreshing: false,
  env: null,
  envError: null,
  chats: null,
  chatsCached: false,
  chatsError: null,
  storage: null,
};

/** Newest activity first; equal times keep their order. */
function byActivity(chats: readonly ChatSummary[]): ChatSummary[] {
  return [...chats].sort((a, b) => b.updated_at - a.updated_at);
}

/** `chats` with one chat's row changed by a status update. */
function withUpdate(chats: readonly ChatSummary[], update: ChatStatusUpdate): ChatSummary[] {
  return byActivity(
    chats.map((chat) =>
      chat.id === update.chat_id
        ? {
            ...chat,
            status: update.status,
            needs_attention: update.needs_attention,
            title: update.title ?? chat.title,
            updated_at: update.updated_at,
          }
        : chat,
    ),
  );
}

/** Whether an update can change what the list knows about a chat's folder. */
function foldersMayChange(known: ChatSummary, update: ChatStatusUpdate): boolean {
  if (update.status === "failed") return known.status !== "failed";
  return known.folder_state !== "ok" && (update.status === "ready" || update.status === "running");
}

export class AppStore {
  readonly #transport: Transport;
  readonly #listeners = new Set<() => void>();
  #state: AppSnapshot = EMPTY;
  #running = false;
  /**
   * Every start and stop gets a number. A poll still waiting for its answer when the store stops
   * (React's development mode starts, stops and starts again at once) must not re-arm in the
   * next run, or that run would poll twice.
   */
  #run = 0;
  #timers = new Set<ReturnType<typeof setTimeout>>();
  /**
   * Every status subscription gets a number: a new one replaces the webview's earlier one, whose
   * channel then ends, and that end must not subscribe yet again.
   */
  #statusGeneration = 0;
  /** Every chat list reload gets a number; `#reloading` is the newest one while it is on its way. */
  #reloads = 0;
  #reloading: number | null = null;
  #refreshing: Promise<void> | null = null;
  /** Status updates that arrived while `#reloading` was on its way, newest last. */
  #missed: ChatStatusUpdate[] = [];

  /** When polling last started, for `RELEASE_POLL_WINDOW_MS`. */
  #startedAt = 0;

  constructor(transport: Transport) {
    this.#transport = transport;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): AppSnapshot => this.#state;

  /**
   * Shows the list this profile last showed, before anything was read. The first read replaces
   * it; a list already read is never replaced by it.
   */
  hydrate(chats: readonly ChatSummary[]): void {
    if (this.#state.chats !== null) return;
    this.#set({ ...this.#state, chats: byActivity(chats), chatsCached: true });
  }

  /** Subscribes to the status channel and loads everything once; then keeps polling. */
  start(): void {
    if (this.#running) return;
    this.#running = true;
    this.#startedAt = Date.now();
    const run = ++this.#run;
    this.#subscribeStatus(run);
    void this.reloadChats();
    void this.#pollStorage(run);
    void this.#pollEnv(run);
    void this.#pollAgents(run);
  }

  stop(): void {
    this.#running = false;
    this.#run += 1;
    this.#statusGeneration += 1;
    for (const timer of this.#timers) clearTimeout(timer);
    this.#timers.clear();
  }

  /** Runs `next` after `delay`, unless run `run` has ended by then (or already has). */
  #later(run: number, delay: number, next: () => void): void {
    if (!this.#running || run !== this.#run) return;
    const timer = setTimeout(() => {
      this.#timers.delete(timer);
      if (this.#running && run === this.#run) next();
    }, delay);
    this.#timers.add(timer);
  }

  #subscribeStatus(run: number): void {
    const generation = ++this.#statusGeneration;
    this.#transport
      .statusSubscribe(
        (update) => {
          if (generation === this.#statusGeneration) this.applyStatus(update);
        },
        () => {
          if (generation !== this.#statusGeneration) return;
          // The shell ended the channel: subscribe again, and catch up on what was missed.
          this.#later(run, STATUS_RESUBSCRIBE_MS, () => {
            this.#subscribeStatus(run);
            void this.reloadChats();
          });
        },
      )
      .catch(() => {
        if (generation !== this.#statusGeneration) return;
        this.#later(run, STATUS_RESUBSCRIBE_MS, () => this.#subscribeStatus(run));
      });
  }

  /** Failed list reads in a row, for `CHATS_RETRY_MS`. */
  #chatFailures = 0;

  async reloadChats(): Promise<void> {
    const reload = ++this.#reloads;
    this.#reloading = reload;
    this.#missed = [];
    try {
      const chats = await this.#transport.chatsList();
      if (reload !== this.#reloads) return;
      // The list may have been read before an update that arrived meanwhile.
      const next = this.#missed.reduce(withUpdate, byActivity(chats));
      this.#chatFailures = 0;
      this.#set({ ...this.#state, chats: next, chatsCached: false, chatsError: null });
    } catch (error) {
      if (reload !== this.#reloads) return;
      this.#set({ ...this.#state, chatsError: errorMessage(error) });
      // Read again until it answers; a newer reload meanwhile makes this one moot.
      const wait = CHATS_RETRY_MS[Math.min(this.#chatFailures, CHATS_RETRY_MS.length - 1)] ?? 8000;
      this.#chatFailures += 1;
      this.#later(this.#run, wait, () => {
        if (reload === this.#reloads) void this.reloadChats();
      });
    } finally {
      if (reload === this.#reloads) {
        this.#reloading = null;
        this.#missed = [];
      }
    }
  }

  /**
   * One status channel update: the chat's row changes; a chat this list lacks reloads it, and so
   * does one whose folder state the update may have changed.
   */
  applyStatus(update: ChatStatusUpdate): void {
    if (this.#reloading !== null) this.#missed.push(update);
    const chats = this.#state.chats;
    const known = chats?.find((chat) => chat.id === update.chat_id);
    if (chats === null || known === undefined) {
      void this.reloadChats();
      return;
    }
    this.#set({ ...this.#state, chats: withUpdate(chats, update) });
    if (foldersMayChange(known, update)) void this.reloadChats();
  }

  /** A chat this window just created (or opened) joins the list at once. */
  addChat(chat: ChatSummary): void {
    const others = (this.#state.chats ?? []).filter((known) => known.id !== chat.id);
    this.#set({ ...this.#state, chats: byActivity([chat, ...others]) });
  }

  async #pollStorage(run: number): Promise<void> {
    try {
      const storage = await this.#transport.storageStatus();
      if (!sameStorage(this.#state.storage, storage)) this.#set({ ...this.#state, storage });
    } catch {
      // Only a refused call fails here; the next poll tries again.
    }
    const saving = this.#state.storage?.state.state === "ok";
    this.#later(
      run,
      saving ? STORAGE_POLL_MS : STORAGE_ALERT_POLL_MS,
      () => void this.#pollStorage(run),
    );
  }

  async #pollEnv(run: number): Promise<void> {
    try {
      const env = await this.#transport.envStatus();
      this.#set({ ...this.#state, env, envError: null });
    } catch (error) {
      this.#set({ ...this.#state, envError: errorMessage(error) });
    }
    if (this.#state.env?.state.state === "resolving") {
      this.#later(run, DETECTION_POLL_MS, () => void this.#pollEnv(run));
    }
  }

  async #pollAgents(run: number): Promise<void> {
    try {
      const agents = await this.#transport.agentsList();
      this.#set({ ...this.#state, agents });
    } catch {
      // Agents stay as they were; the next poll or Refresh tries again.
    }
    // Quickly while an installation or its release check is still being worked out, so an update
    // notice shows soon after a start.
    const early = Date.now() - this.#startedAt < RELEASE_POLL_WINDOW_MS;
    const checking = this.#state.agents?.some(
      (agent) =>
        agent.availability.state === "checking" || (early && agent.harness?.state === "checking"),
    );
    this.#later(
      run,
      checking ? DETECTION_POLL_MS : CATALOG_POLL_MS,
      () => void this.#pollAgents(run),
    );
  }

  /** "Refresh agents": the env again and every probe (15 s at most). */
  refreshAgents(): Promise<void> {
    // A second caller waits for the refresh under way: an update that just finished needs its
    // result, and returning at once left its row on the old version.
    this.#refreshing ??= (async () => {
      this.#set({ ...this.#state, agentsRefreshing: true });
      try {
        const agents = await this.#transport.agentsRefresh();
        this.#set({ ...this.#state, agents, agentsRefreshing: false });
      } catch {
        this.#set({ ...this.#state, agentsRefreshing: false });
      } finally {
        this.#refreshing = null;
      }
    })();
    return this.#refreshing;
  }

  /**
   * An agent's Connection setting (`null`: Automatic). Rust answers every agent from the last
   * detection, so the lists change at once; a failure is thrown for the caller to show.
   */
  async setAgentConnection(agentId: string, connection: string | null): Promise<void> {
    const agents = await this.#transport.agentSetConnection(agentId, connection);
    this.#set({ ...this.#state, agents });
  }

  /** The env banner's Retry: a new resolution, followed until it ends. */
  async retryEnv(): Promise<void> {
    const run = this.#run;
    try {
      const env = await this.#transport.envRefresh();
      this.#set({ ...this.#state, env, envError: null });
    } catch (error) {
      this.#set({ ...this.#state, envError: errorMessage(error) });
    }
    this.#later(run, DETECTION_POLL_MS, () => void this.#pollEnv(run));
  }

  #set(state: AppSnapshot): void {
    this.#state = state;
    for (const listener of [...this.#listeners]) listener();
  }
}

function sameStorage(a: StorageStatus | null, b: StorageStatus): boolean {
  return (
    a !== null &&
    a.unsaved_bytes === b.unsaved_bytes &&
    JSON.stringify(a.state) === JSON.stringify(b.state)
  );
}

/** Whether an agent can be chosen for a new chat, and why not. */
export function agentChoice(agent: AgentEntry): {
  readonly enabled: boolean;
  readonly reason: string | null;
} {
  switch (agent.availability.state) {
    case "ready":
    case "checking":
      // A chat created while its agent is still checked waits for the detection at its start.
      return { enabled: true, reason: null };
    case "missing":
    case "broken":
      return { enabled: false, reason: agent.availability.reason };
  }
}

export interface NewChatState {
  readonly agentId: string | null;
  readonly folder: string | null;
  readonly picking: boolean;
  readonly creating: boolean;
  readonly error: string | null;
  readonly maintenance: AgentMaintenance | null;
  readonly maintenanceBusy: "checking" | "auth" | "update" | null;
  readonly auth: AgentAuthStatus | null;
  readonly maintenanceMessage: string | null;
  readonly maintenanceError: string | null;
}

/** The New chat screen: an agent, a folder from the picker, and Create. */
export class NewChatDraft {
  static #nextKey = 0;
  readonly key = ++NewChatDraft.#nextKey;
  readonly context: NewChatContext | null;
  readonly #transport: Transport;
  readonly #listeners = new Set<() => void>();
  #selection = 0;
  #state: NewChatState = {
    agentId: null,
    folder: null,
    picking: false,
    creating: false,
    error: null,
    maintenance: null,
    maintenanceBusy: null,
    auth: null,
    maintenanceMessage: null,
    maintenanceError: null,
  };

  constructor(transport: Transport, context: NewChatContext | null = null) {
    this.#transport = transport;
    this.context = context;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): NewChatState => this.#state;

  chooseAgent(agentId: string): void {
    const selection = ++this.#selection;
    this.#set({
      ...this.#state,
      agentId,
      error: null,
      maintenance: null,
      maintenanceBusy: "checking",
      auth: null,
      maintenanceMessage: null,
      maintenanceError: null,
    });
    void this.#transport
      .agentMaintenance(agentId)
      .then((maintenance) => {
        if (selection === this.#selection)
          this.#set({ ...this.#state, maintenance, maintenanceBusy: null });
      })
      .catch((error) => {
        if (selection === this.#selection)
          this.#set({
            ...this.#state,
            maintenanceError: errorMessage(error),
            maintenanceBusy: null,
          });
      });
  }

  async checkAuth(): Promise<void> {
    const { agentId, maintenance, maintenanceBusy } = this.#state;
    if (agentId === null || !maintenance?.auth_status || maintenanceBusy !== null) return;
    const selection = this.#selection;
    this.#set({ ...this.#state, maintenanceBusy: "auth", maintenanceError: null });
    try {
      const auth = await this.#transport.agentAuthStatus(agentId);
      if (selection === this.#selection) this.#set({ ...this.#state, auth, maintenanceBusy: null });
    } catch (error) {
      if (selection === this.#selection)
        this.#set({ ...this.#state, maintenanceError: errorMessage(error), maintenanceBusy: null });
    }
  }

  async updateAgent(): Promise<boolean> {
    const { agentId, maintenance, maintenanceBusy } = this.#state;
    if (
      agentId === null ||
      maintenance?.update_unavailable !== null ||
      maintenanceBusy !== null ||
      this.#state.creating
    )
      return false;
    const selection = this.#selection;
    this.#set({
      ...this.#state,
      maintenanceBusy: "update",
      maintenanceMessage: null,
      maintenanceError: null,
    });
    try {
      await this.#transport.agentUpdate(agentId);
      if (selection === this.#selection)
        this.#set({
          ...this.#state,
          maintenanceMessage: "Update complete. The installed version was checked again.",
          maintenanceBusy: null,
        });
      return true;
    } catch (error) {
      if (selection === this.#selection)
        this.#set({ ...this.#state, maintenanceError: errorMessage(error), maintenanceBusy: null });
      return false;
    }
  }

  /** The system folder picker; cancelling it changes nothing. */
  async chooseFolder(): Promise<string | null> {
    if (this.#state.picking) return null;
    this.#set({ ...this.#state, picking: true });
    try {
      const folder = await this.#transport.pickFolder();
      this.#set({
        ...this.#state,
        picking: false,
        folder: folder ?? this.#state.folder,
        error: folder === null ? this.#state.error : null,
      });
      return folder;
    } catch (error) {
      this.#set({ ...this.#state, picking: false, error: errorMessage(error) });
      return null;
    }
  }

  /** Creates the chat (which starts its agent). Answers it, or `null` with the error shown. */
  async create(): Promise<ChatSummary | null> {
    const { agentId, folder, creating } = this.#state;
    if (agentId === null || folder === null || creating || this.#state.maintenanceBusy === "update")
      return null;
    this.#set({ ...this.#state, creating: true, error: null });
    try {
      const chat = await this.#transport.chatCreate(agentId, folder);
      this.#set({ ...this.#state, creating: false });
      return chat;
    } catch (error) {
      this.#set({ ...this.#state, creating: false, error: errorMessage(error) });
      return null;
    }
  }

  #set(state: NewChatState): void {
    this.#state = state;
    for (const listener of [...this.#listeners]) listener();
  }
}
