import type {
  AuthPrompt,
  Autonomy,
  BackgroundTask,
  ChatAttach,
  ChatSummary,
  ConfigValue,
  ErrorInfo,
  EventBatch,
  GhostEvent,
  HistoryPage,
  LimitWindow,
  QueuedPrompt,
  SendingPrompt,
  SessionControlChange,
  SessionInfo,
  SessionLists,
  SessionStatus,
  StorageStatus,
  TurnAgent,
  UiAnswer,
  UsageInfo,
} from "../../bindings";
import { errorMessage } from "../errors";
import { CommandError } from "../ipc";
import {
  applyEntries,
  compactToolUpdates,
  type Entry,
  emptyTranscript,
  type Transcript,
  withTasks,
} from "./transcript";
import type { Transport } from "./transport";

/**
 * One open chat view, as an external store for React (`useSyncExternalStore`).
 *
 * **Joining** (spec 6.8): the view's channel buffers frames while `chat_attach` runs; the answer
 * brings the snapshot, the newest history page and `next_seq`. The page is applied, then the
 * buffered frames from `next_seq` on, then live frames. A frame that starts past the expected seq,
 * or the channel ending, means frames were lost: the store detaches, drops its state and attaches
 * again. A failed attach is retried when the storage status changes and on Retry.
 *
 * **Everything else**: the queue and the header follow the live frames; cards and questions
 * disable their buttons while an answer is on its way, and a `request_not_pending` answer closes
 * them; the lists (options, slash commands, autonomy) are fetched on attach and again, debounced,
 * whenever the session's `revision` changes; Esc interrupts, then force-stops; "Load full output"
 * pages through a blob; "Still working" shows after a minute without frames during a turn.
 *
 * **Held prompts.** "Send queued" follows `SessionInfo.queue_held`. The snapshot and each queued
 * lifecycle event identify each row's hold; a later hold catches every prompt queued until then.
 * When frames cannot place a later hold against the prompts listed (one queued as the hold ran,
 * or a hold while some were already held), the view joins again for the snapshot's exact queue.
 */

export type ChatPhase = "attaching" | "live" | "failed";

/** A card or question with an answer on its way, or one that is no longer waiting. */
export interface RequestState {
  readonly busy: boolean;
  /** The shell said `request_not_pending`: the card closes without its resolution. */
  readonly gone: boolean;
  readonly error: string | null;
}

/** A full output loaded page by page. */
export interface BlobState {
  readonly text: string;
  readonly nextOffset: number;
  readonly total: number;
  readonly eof: boolean;
  readonly loading: boolean;
  readonly error: string | null;
}

export interface ChatState {
  readonly chatId: string;
  readonly phase: ChatPhase;
  /** Why the last attach failed. */
  readonly attachError: string | null;
  readonly chat: ChatSummary | null;
  readonly session: SessionInfo | null;
  readonly openTurnId: string | null;
  readonly steeringPending: boolean;
  readonly queue: readonly QueuedPrompt[];
  /**
   * The message on its way to the agent while its folder checkpoint saves: it shows in the
   * transcript as sending, with the canonical body and attachments from the persisted row.
   */
  readonly sending: SendingPrompt | null;
  readonly usage: UsageInfo | null;
  readonly limits: readonly LimitWindow[] | null;
  readonly lastError: ErrorInfo | null;
  readonly auth: AuthPrompt | null;
  readonly storage: StorageStatus | null;
  readonly transcript: Transcript;
  /**
   * The tasks the live session knows (the snapshot's, and any a live frame reports): a task history
   * says still runs but that is not here ran in an earlier process, and shows as stopped.
   */
  readonly liveTasks: ReadonlySet<string>;
  /**
   * The agent that ran each turn, by turn id: from history pages, and from the chat's agent when a
   * turn starts live. A turn missing here is the chat's current agent's.
   */
  readonly turnAgents: ReadonlyMap<string, TurnAgent>;
  readonly hasMore: boolean;
  readonly loadingEarlier: boolean;
  /** Advances only when an older page arrives, independently of live output. */
  readonly historyRevision: number;
  readonly earlierError: string | null;
  /** Rows that did not parse (written by a newer build), across the loaded pages. */
  readonly skipped: number;
  /** Saving is paused: rows from this seq to the attach point are not shown yet. */
  readonly unsavedFrom: number | null;
  readonly lists: SessionLists | null;
  /** Why the lists could not load, or an autonomy or native control change was refused. */
  readonly listsError: string | null;
  /** Why the last model, thinking level or other option change was refused. */
  readonly optionError: string | null;
  /** Option changes on their way (up to 35 s), by option id: the pickers show the value asked for. */
  readonly optionsSaving: ReadonlyMap<string, ConfigValue>;
  /** An autonomy change on its way. */
  readonly autonomySaving: Autonomy | null;
  readonly controlSaving: SessionControlChange | null;
  readonly requests: ReadonlyMap<string, RequestState>;
  readonly blobs: ReadonlyMap<string, BlobState>;
  /** The last failed send, shown under the composer. */
  readonly sendError: string | null;
  /** The last failed header, queue or picker action. */
  readonly actionError: string | null;
  /** Minutes without a frame while a turn runs, once there is at least one. */
  readonly stillWorkingMinutes: number | null;
}

/** How much `chat_blob` returns per call at most. */
export const BLOB_PAGE = 256 * 1024;

/** The page size "Load earlier" asks for: `chat_history`'s largest. */
export const EARLIER_PAGE_BYTES = 4 * 1024 * 1024;

/**
 * "Load earlier" keeps fetching pages until this many more turns have started, so one click
 * brings a real stretch of the chat even when a single turn spans hundreds of pages.
 */
export const EARLIER_TURNS = 20;

/** The most pages one "Load earlier" fetches: 512 MiB at most. */
export const EARLIER_MAX_PAGES = 128;

/** A new `revision` refetches the lists after this long without another one. */
export const LISTS_DEBOUNCE_MS = 200;

/** A change answered with lists that do not show it yet stays shown at most this long. */
export const OPTION_HOLD_MS = 1500;

/** "Still working" shows after this long without a frame during a turn. */
export const STILL_WORKING_MS = 60_000;

export interface ChatStoreOptions {
  /** Wall clock, for queued prompts' times and the still-working hint. */
  readonly now?: () => number;
  /** Whether the page is hidden (minimized, or behind the tray), and when that changes. */
  readonly visibility?: PageVisibility;
}

/** The page's visibility, as the store needs it. */
export interface PageVisibility {
  readonly hidden: () => boolean;
  /** Calls `listener` whenever the page is hidden or shown again; returns how to stop. */
  readonly onChange: (listener: () => void) => () => void;
}

/** `document.visibilityState`; always visible where there is no document (tests, servers). */
export const documentVisibility: PageVisibility = {
  hidden: () => typeof document !== "undefined" && document.visibilityState === "hidden",
  onChange: (listener) => {
    if (typeof document === "undefined") return () => {};
    document.addEventListener("visibilitychange", listener);
    return () => document.removeEventListener("visibilitychange", listener);
  },
};

function initialState(chatId: string): ChatState {
  return {
    chatId,
    phase: "attaching",
    attachError: null,
    chat: null,
    session: null,
    openTurnId: null,
    steeringPending: false,
    queue: [],
    sending: null,
    usage: null,
    limits: null,
    lastError: null,
    auth: null,
    storage: null,
    transcript: emptyTranscript(),
    liveTasks: new Set(),
    turnAgents: new Map(),
    hasMore: false,
    loadingEarlier: false,
    historyRevision: 0,
    earlierError: null,
    skipped: 0,
    unsavedFrom: null,
    lists: null,
    listsError: null,
    optionError: null,
    optionsSaving: new Map(),
    autonomySaving: null,
    controlSaving: null,
    requests: new Map(),
    blobs: new Map(),
    sendError: null,
    actionError: null,
    stillWorkingMinutes: null,
  };
}

/** `agents` with the agents a history page names for its turns. */
function withTurnAgents(
  agents: ReadonlyMap<string, TurnAgent>,
  page: HistoryPage,
): ReadonlyMap<string, TurnAgent> {
  const named = page.turn_agents ?? [];
  if (named.length === 0) return agents;
  const next = new Map(agents);
  for (const agent of named) next.set(agent.turn_id, agent);
  return next;
}

/** A turn that starts live runs on the chat's agent of that moment, which it keeps. */
function withLiveTurnAgent(state: ChatState, turnId: string): ReadonlyMap<string, TurnAgent> {
  if (state.turnAgents.has(turnId) || state.chat === null) return state.turnAgents;
  return new Map(state.turnAgents).set(turnId, {
    turn_id: turnId,
    agent_id: state.chat.agent_id,
    agent_name: state.session?.agent_name ?? state.chat.agent_name,
  });
}

function isCode(error: unknown, code: string): boolean {
  return error instanceof CommandError && error.code === code;
}

/**
 * Whether a change of status can come with a hold of the queue (spec 6.5.3 to 6.5.5): an interrupt,
 * a stop or quit, a force stop or a crash, a sign-in the agent needs, an error that ends a turn, or
 * a start that fails in the middle of one. A plain start, a dispatch and the end of a cancel hold
 * nothing.
 */
function mayHold(from: SessionStatus, to: SessionStatus): boolean {
  switch (to) {
    case "cancelling":
    case "stopping":
    case "stopped":
    case "auth_required":
      return true;
    case "ready":
      return from === "running";
    case "failed":
      return from !== "starting";
    case "starting":
    case "running":
      return false;
  }
}

/** The queue and subscription observed before asking the shell to change a queued prompt. */
interface QueueRequest {
  readonly generation: number;
  readonly revision: number;
  readonly attaching: boolean;
}

export class ChatStore {
  readonly chatId: string;
  readonly #transport: Transport;
  readonly #now: () => number;
  readonly #listeners = new Set<() => void>();
  #state: ChatState;
  readonly #visibility: PageVisibility;
  #stopWatchingVisibility: (() => void) | null = null;
  /** Changes made while the page was hidden that the view has not been told about yet. */
  #unrendered = false;

  /** Every attach gets a number; frames, ends and answers of older ones are ignored. */
  #generation = 0;
  /** Queue changes only; text, usage and unrelated session updates do not invalidate replies. */
  #queueRevision = 0;
  #subscriptionId: string | null = null;
  /** Frames that arrived while the attach call ran. */
  #buffer: EventBatch[] = [];
  #attaching = false;
  #expectedSeq = 0;
  /** Every applied entry, oldest first: "Load earlier" rebuilds the transcript from these. */
  #entries: Entry[] = [];
  /** The oldest loaded page's cut turn. */
  #cut: string | null = null;
  /** The attach's tasks, laid over the history again whenever the transcript is rebuilt. */
  #snapshotTasks: readonly BackgroundTask[] = [];
  /** How many of `#entries` came before the attach: the snapshot's tasks go between. */
  #attachedEntries = 0;
  #beforeSeq: number | null = null;
  #closed = false;

  #listsTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * Every request that answers with the lists (a refetch, an option or autonomy change) gets a
   * number, and only the newest one's answer replaces them: an older answer arriving late holds
   * an older state.
   */
  #listsRequest = 0;
  /** The number of each option's newest change, and of the newest autonomy change. */
  #optionRequests = new Map<string, number>();
  /**
   * Changes whose answer came with lists that do not show them yet (a driver answers before its
   * lists arrive): the picker keeps showing the value asked for until the refetch the session's
   * new revision brings lands, or `OPTION_HOLD_MS` passes.
   */
  #heldOptions = new Map<string, ReturnType<typeof setTimeout>>();
  #autonomyRequest = 0;
  #controlRequest = 0;
  #stillTimer: ReturnType<typeof setTimeout> | null = null;
  /** When the last frame arrived or the running turn started. */
  #lastActivity = 0;
  /** An interrupt was asked for and the turn has not ended: the next Esc force-stops. */
  #interruptAsked = false;
  /** A snapshot taken in `auth_required` is fresh: no need to join again for its commands. */
  #authRefreshed = false;
  /**
   * Prompts queued since the last `session_updated` (or the snapshot): a hold that update reports
   * may have run before or after them. Every flush ends with the update when it changed, so a
   * prompt listed before it was queued before any hold the next one reports.
   */
  #queuedSinceUpdate = new Set<string>();
  /** A join for state the frames do not carry is due once the current frame is applied. */
  #rejoinDue = false;
  /** The channel ended while its attach call ran: the attach's answer decides what follows. */
  #endedWhileAttaching = false;
  /** Joins that lost their subscription again before any live frame arrived. */
  #quickRejoins = 0;
  #rejoinTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(chatId: string, transport: Transport, options: ChatStoreOptions = {}) {
    this.chatId = chatId;
    this.#transport = transport;
    this.#now = options.now ?? (() => Date.now());
    this.#visibility = options.visibility ?? documentVisibility;
    this.#state = initialState(chatId);
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    this.#stopWatchingVisibility ??= this.#visibility.onChange(this.#visibilityChanged);
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) {
        this.#stopWatchingVisibility?.();
        this.#stopWatchingVisibility = null;
      }
    };
  };

  readonly getSnapshot = (): ChatState => this.#state;

  /** Opens the view: the first attach. */
  open(): Promise<void> {
    this.#closed = false;
    return this.#attach();
  }

  /** Closes the view (unmount): detaches and stops every timer. */
  close(): void {
    this.#closed = true;
    this.#generation += 1;
    this.#clearTimers();
    const subscription = this.#subscriptionId;
    this.#subscriptionId = null;
    if (subscription !== null) {
      this.#transport.detach(subscription).catch(() => undefined);
    }
  }

  /** Detaches, drops the view's state and attaches again: Retry, or a storage change. */
  async reattach(): Promise<void> {
    if (this.#closed) return;
    this.#quickRejoins = 0;
    await this.#rejoin();
  }

  async #rejoin(): Promise<void> {
    if (this.#rejoinTimer !== null) {
      clearTimeout(this.#rejoinTimer);
      this.#rejoinTimer = null;
    }
    this.#generation += 1;
    const subscription = this.#subscriptionId;
    this.#subscriptionId = null;
    // Detach first, so the shell never holds two views of this chat for this window.
    if (subscription !== null) {
      await this.#transport.detach(subscription).catch(() => undefined);
    }
    if (this.#closed) return;
    await this.#attach();
  }

  /**
   * Frames were lost (a gap) or the channel ended: join again. The first time at once; when joins
   * keep losing their subscription before any live frame, after a growing pause, so a broken
   * session never spins.
   */
  #lost(): void {
    if (this.#closed) return;
    const attempt = this.#quickRejoins;
    this.#quickRejoins += 1;
    if (attempt < 2) {
      void this.#rejoin();
      return;
    }
    this.#generation += 1;
    this.#set({ ...this.#state, phase: "attaching" });
    if (this.#rejoinTimer !== null) clearTimeout(this.#rejoinTimer);
    const delay = Math.min(250 * 2 ** (attempt - 2), 5000);
    this.#rejoinTimer = setTimeout(() => {
      this.#rejoinTimer = null;
      void this.#rejoin();
    }, delay);
  }

  /** The storage status changed: a failed attach retries; a page with unsaved rows re-attaches. */
  storageChanged(status: StorageStatus): void {
    const previous = this.#state.storage;
    if (previous !== null && sameStorage(previous, status)) return;
    this.#set({ ...this.#state, storage: status });
    if (this.#state.phase === "failed") {
      void this.reattach();
    } else if (this.#state.unsavedFrom !== null && status.state.state === "ok") {
      void this.reattach();
    }
  }

  // ---------- the join protocol ----------

  async #attach(): Promise<void> {
    const generation = ++this.#generation;
    this.#buffer = [];
    this.#attaching = true;
    this.#endedWhileAttaching = false;
    this.#set({ ...this.#state, phase: "attaching", attachError: null, controlSaving: null });
    let attached: ChatAttach;
    try {
      attached = await this.#transport.attach(
        this.chatId,
        (frame) => this.#onFrame(generation, frame),
        () => this.#onEnd(generation),
      );
    } catch (error) {
      if (generation !== this.#generation) return;
      this.#attaching = false;
      this.#set({ ...this.#state, phase: "failed", attachError: errorMessage(error) });
      return;
    }
    if (generation !== this.#generation || this.#closed) {
      // Replaced or closed while attaching: the shell drops this subscription.
      this.#transport.detach(attached.subscription_id).catch(() => undefined);
      return;
    }
    this.#attaching = false;
    this.#subscriptionId = attached.subscription_id;
    this.#applyAttach(attached);
    const buffered = this.#buffer;
    this.#buffer = [];
    for (const frame of buffered) {
      if (!this.#applyFrame(frame)) return;
    }
    this.#refreshLists();
    if (this.#endedWhileAttaching) this.#lost();
  }

  #applyAttach(attached: ChatAttach): void {
    const { snapshot, history } = attached;
    this.#cut = history.cut_turn_id;
    this.#beforeSeq = history.before_seq;
    this.#entries = pageEntries(history);
    if (history.unsaved_from !== null) {
      this.#entries.push({ seq: history.unsaved_from, gap: true });
    }
    this.#expectedSeq = attached.next_seq;
    this.#snapshotTasks = snapshot.tasks ?? [];
    this.#attachedEntries = this.#entries.length;
    this.#interruptAsked = false;
    this.#authRefreshed = snapshot.session.status === "auth_required";
    this.#queuedSinceUpdate = new Set();
    this.#lastActivity = this.#now();
    this.#set({
      ...this.#state,
      phase: "live",
      attachError: null,
      chat: snapshot.chat,
      session: snapshot.session,
      openTurnId: snapshot.open_turn_id,
      queue: snapshot.queue,
      sending: snapshot.session.sending ?? null,
      usage: snapshot.usage,
      limits: snapshot.limits,
      lastError: snapshot.last_error,
      auth: snapshot.auth,
      storage: snapshot.storage,
      transcript: withTasks(
        applyEntries(emptyTranscript(this.#cut), this.#entries),
        this.#snapshotTasks,
      ),
      liveTasks: new Set(this.#snapshotTasks.map((task) => task.task_id)),
      turnAgents: withTurnAgents(new Map(), history),
      hasMore: history.has_more,
      loadingEarlier: false,
      earlierError: null,
      skipped: history.skipped,
      unsavedFrom: history.unsaved_from,
      requests: new Map(),
      stillWorkingMinutes: null,
    });
    this.#armStillWorking();
  }

  #onFrame(generation: number, frame: EventBatch): void {
    if (generation !== this.#generation || this.#closed) return;
    if (this.#attaching) {
      this.#buffer.push(frame);
      return;
    }
    this.#applyFrame(frame);
  }

  #onEnd(generation: number): void {
    if (generation !== this.#generation || this.#closed) return;
    if (this.#attaching) {
      // A refused attach ends its channel too; its answer says what happened.
      this.#endedWhileAttaching = true;
      return;
    }
    // The shell dropped this view (its session stopped, or it was replaced): join again.
    this.#lost();
  }

  /** Applies one frame; a gap joins again and answers `false`. */
  #applyFrame(frame: EventBatch): boolean {
    const ahead = frame.seq_start - this.#expectedSeq;
    if (ahead > 0) {
      this.#lost();
      return false;
    }
    if (!this.#attaching) this.#quickRejoins = 0;
    const skip = Math.min(-ahead, frame.events.length);
    const events = frame.events.slice(skip);
    this.#expectedSeq = Math.max(this.#expectedSeq, frame.seq_start + frame.events.length);
    this.#lastActivity = this.#now();
    if (events.length === 0) return true;
    const entries: Entry[] = events.map((event, index) => ({
      seq: frame.seq_start + skip + index,
      turnId: null,
      event,
    }));
    this.#entries.push(...entries);
    let state: ChatState = {
      ...this.#state,
      transcript: applyEntries(this.#state.transcript, entries),
      stillWorkingMinutes: null,
    };
    for (const event of events) state = this.#applyLive(state, event);
    this.#set(state);
    this.#armStillWorking();
    return true;
  }

  /** The live state a frame's events change: the header, the queue, usage and limits. */
  #applyLive(state: ChatState, event: GhostEvent): ChatState {
    switch (event.type) {
      case "turn_started":
        return {
          ...state,
          sending: state.sending?.prompt_id === event.prompt_id ? null : state.sending,
          openTurnId: event.turn_id,
          turnAgents: withLiveTurnAgent(state, event.turn_id),
        };
      case "turn_ended":
        return state.openTurnId === event.turn_id ? { ...state, openTurnId: null } : state;
      case "session_updated": {
        const { type: _type, ...session } = event;
        return this.#sessionChanged(state, session);
      }
      case "prompt_lifecycle": {
        if (event.state === "sending") {
          // On its way: the transcript shows it as sending; it waits behind nothing.
          this.#queuedSinceUpdate.delete(event.prompt_id);
          return {
            ...state,
            queue: state.queue.filter((row) => row.prompt_id !== event.prompt_id),
          };
        }
        const returned = state.sending?.prompt_id === event.prompt_id ? state.sending : null;
        if (returned !== null) state = { ...state, sending: null };
        if (event.state === "queued") {
          // The core owns each row's hold. Older producers omit it and retain the old behavior.
          const index = state.queue.findIndex((row) => row.prompt_id === event.prompt_id);
          const previous = index < 0 ? undefined : state.queue[index];
          const queued: QueuedPrompt = {
            prompt_id: event.prompt_id,
            preview: event.preview ?? previous?.preview ?? "",
            held: event.held ?? false,
            created_at: previous?.created_at ?? returned?.created_at ?? this.#now(),
          };
          this.#queuedSinceUpdate.add(event.prompt_id);
          const queue = [...state.queue];
          // A capture that was stopped returns the Sending head before its waiting followers.
          if (index < 0 && returned !== null) queue.unshift(queued);
          else if (index < 0) queue.push(queued);
          else queue[index] = queued;
          return {
            ...state,
            queue,
          };
        }
        this.#queuedSinceUpdate.delete(event.prompt_id);
        return { ...state, queue: state.queue.filter((row) => row.prompt_id !== event.prompt_id) };
      }
      case "usage_updated": {
        const { type: _type, ...usage } = event;
        return { ...state, usage };
      }
      case "limits_updated":
        return { ...state, limits: event.windows };
      case "task_updated":
        return state.liveTasks.has(event.task_id)
          ? state
          : { ...state, liveTasks: new Set(state.liveTasks).add(event.task_id) };
      case "permission_resolved":
      case "ui_resolved":
        // The card is resolved now; whatever answer was on its way is done.
        return { ...state, requests: without(state.requests, event.request_id) };
      default:
        return state;
    }
  }

  #sessionChanged(state: ChatState, session: SessionInfo): ChatState {
    const previous = state.session;
    const recent = this.#queuedSinceUpdate;
    this.#queuedSinceUpdate = new Set();
    let queue = state.queue;
    let rejoin = false;
    if (!session.queue_held) {
      // Nothing is held.
      if (queue.some((row) => row.held)) queue = queue.map((row) => ({ ...row, held: false }));
    } else if (previous === null || !previous.queue_held) {
      // A hold ran since the last update and caught every prompt listed before it. One queued
      // since that update may have come just after the hold: only the snapshot can tell.
      queue = queue.map((row) =>
        row.held || recent.has(row.prompt_id) ? row : { ...row, held: true },
      );
      rejoin = queue.some((row) => !row.held);
    } else if (previous.status !== session.status && mayHold(previous.status, session.status)) {
      // Some prompts were held already, so a hold now changes nothing the frames show.
      rejoin = queue.some((row) => !row.held);
    }
    if (session.status !== "running" && session.status !== "cancelling") {
      this.#interruptAsked = false;
    }
    if (previous?.status !== "running" && session.status === "running") {
      this.#lastActivity = this.#now();
    }
    if (previous !== null && previous.revision !== session.revision) this.#scheduleLists();
    if (session.status === "auth_required" && state.auth === null && !this.#authRefreshed) {
      // The frames carry no sign-in commands: a fresh snapshot does.
      this.#authRefreshed = true;
      rejoin = true;
    } else if (session.status !== "auth_required") {
      this.#authRefreshed = false;
    }
    // A lifecycle or canonical start clears its own Sending identity before this update. If
    // Sending disappears otherwise, rejoin for the queue: capture can finish while background
    // work still blocks dispatch, or Stop/failure can return the prompt held.
    const sending = session.sending ?? null;
    if (sending !== null) queue = queue.filter((row) => row.prompt_id !== sending.prompt_id);
    if (state.sending !== null && sending === null) rejoin = true;
    if (rejoin) this.#rejoinSoon();
    return {
      ...state,
      session,
      queue,
      sending,
      auth: session.status === "auth_required" ? state.auth : null,
    };
  }

  /** Joins again once the current frame is applied: the snapshot carries what frames do not. */
  #rejoinSoon(): void {
    if (this.#rejoinDue) return;
    this.#rejoinDue = true;
    queueMicrotask(() => {
      this.#rejoinDue = false;
      void this.reattach();
    });
  }

  /**
   * Older history in one chunk: pages of up to 4 MiB until `EARLIER_TURNS` more turns have
   * started or the chat's start is reached, shown together. The same turn continues when a page
   * cut it. A failure keeps the pages already fetched and says why the rest did not load.
   */
  async loadEarlier(): Promise<void> {
    let before = this.#beforeSeq;
    if (!this.#state.hasMore || before === null || this.#state.loadingEarlier) return;
    const generation = this.#generation;
    this.#set({ ...this.#state, loadingEarlier: true, earlierError: null });
    let older: Entry[] = [];
    const pages: HistoryPage[] = [];
    let skipped = 0;
    let cut = this.#cut;
    let hasMore: boolean = this.#state.hasMore;
    let turns = 0;
    let fetched = 0;
    let failure: string | null = null;
    while (hasMore && before !== null && turns < EARLIER_TURNS && fetched < EARLIER_MAX_PAGES) {
      let page: HistoryPage;
      try {
        page = await this.#transport.history(this.chatId, before, EARLIER_PAGE_BYTES);
      } catch (error) {
        failure = errorMessage(error);
        break;
      }
      if (generation !== this.#generation) return;
      fetched += 1;
      const entries = pageEntries(page);
      turns += entries.filter(
        (entry) => "event" in entry && entry.event.type === "turn_started",
      ).length;
      // Folded as it arrives, so a turn of 50,000 tool updates never sits in memory whole.
      older = compactToolUpdates([...entries, ...older]);
      pages.push(page);
      skipped += page.skipped;
      cut = page.cut_turn_id;
      before = page.before_seq;
      hasMore = page.has_more;
    }
    if (generation !== this.#generation) return;
    if (fetched === 0) {
      this.#set({ ...this.#state, loadingEarlier: false, earlierError: failure });
      return;
    }
    this.#cut = cut;
    this.#beforeSeq = before;
    // Only the history before the attach folds: the snapshot's tasks sit between it and the live
    // frames since.
    const history = compactToolUpdates([
      ...older,
      ...this.#entries.slice(0, this.#attachedEntries),
    ]);
    this.#entries = [...history, ...this.#entries.slice(this.#attachedEntries)];
    this.#attachedEntries = history.length;
    // The snapshot's tasks are newer than history and older than the live frames since the attach.
    this.#set({
      ...this.#state,
      transcript: applyEntries(
        withTasks(
          applyEntries(emptyTranscript(this.#cut), this.#entries.slice(0, this.#attachedEntries)),
          this.#snapshotTasks,
        ),
        this.#entries.slice(this.#attachedEntries),
      ),
      turnAgents: pages.reduce(withTurnAgents, this.#state.turnAgents),
      hasMore,
      loadingEarlier: false,
      earlierError: failure,
      historyRevision: this.#state.historyRevision + 1,
      skipped: this.#state.skipped + skipped,
    });
  }

  // ---------- the lists ----------

  #scheduleLists(): void {
    if (this.#listsTimer !== null) clearTimeout(this.#listsTimer);
    this.#listsTimer = setTimeout(() => {
      this.#listsTimer = null;
      this.#refreshLists();
    }, LISTS_DEBOUNCE_MS);
  }

  #refreshLists(): void {
    const request = ++this.#listsRequest;
    this.#transport.lists(this.chatId).then(
      (lists) => {
        if (request !== this.#listsRequest || this.#closed) return;
        // The session's own lists: a change held for them now shows as they say.
        let optionsSaving = this.#state.optionsSaving;
        for (const [optionId, timer] of this.#heldOptions) {
          clearTimeout(timer);
          this.#optionRequests.delete(optionId);
          optionsSaving = without(optionsSaving, optionId);
        }
        this.#heldOptions.clear();
        this.#set({ ...this.#state, lists, listsError: null, optionsSaving });
      },
      (error: unknown) => {
        if (request !== this.#listsRequest || this.#closed) return;
        this.#set({ ...this.#state, listsError: errorMessage(error) });
      },
    );
  }

  /**
   * Changes an option. Its picker shows the value asked for until the answer arrives; a newer
   * change supersedes it, and the lists come from the newest request's answer only. An answer
   * whose lists do not show the change yet (they follow it) keeps the value asked for in view
   * until the refetch its revision brings, so the picker never snaps back. A refusal says why.
   */
  async setOption(optionId: string, value: ConfigValue): Promise<void> {
    const request = ++this.#listsRequest;
    this.#optionRequests.set(optionId, request);
    this.#release(optionId);
    const saving = new Map(this.#state.optionsSaving);
    saving.set(optionId, value);
    this.#set({ ...this.#state, optionsSaving: saving, optionError: null });
    const answer = await this.#listsAnswer(() =>
      this.#transport.setOption(this.chatId, optionId, value),
    );
    if (this.#closed) return;
    let state = this.#state;
    if (this.#optionRequests.get(optionId) === request) {
      if (
        !("error" in answer) &&
        request === this.#listsRequest &&
        !showsValue(answer.lists, optionId, value)
      ) {
        this.#hold(optionId, request);
      } else {
        this.#optionRequests.delete(optionId);
        state = { ...state, optionsSaving: without(state.optionsSaving, optionId) };
      }
    }
    if ("error" in answer) {
      this.#set({ ...state, optionError: answer.error });
      return;
    }
    this.#set({ ...this.#withAnswer(state, request, answer), optionError: null });
  }

  /** Keeps change `request` of `optionId` in view until the next lists, or for a moment. */
  #hold(optionId: string, request: number): void {
    this.#release(optionId);
    this.#heldOptions.set(
      optionId,
      setTimeout(() => {
        this.#heldOptions.delete(optionId);
        if (this.#closed || this.#optionRequests.get(optionId) !== request) return;
        this.#optionRequests.delete(optionId);
        this.#set({
          ...this.#state,
          optionsSaving: without(this.#state.optionsSaving, optionId),
        });
      }, OPTION_HOLD_MS),
    );
  }

  #release(optionId: string): void {
    const timer = this.#heldOptions.get(optionId);
    if (timer !== undefined) clearTimeout(timer);
    this.#heldOptions.delete(optionId);
  }

  /** Changes the autonomy, the same way as an option. */
  async setAutonomy(autonomy: Autonomy): Promise<void> {
    const request = ++this.#listsRequest;
    this.#autonomyRequest = request;
    this.#set({ ...this.#state, autonomySaving: autonomy });
    const answer = await this.#listsAnswer(() =>
      this.#transport.setAutonomy(this.chatId, autonomy),
    );
    if (this.#closed) return;
    let state = this.#state;
    if (this.#autonomyRequest === request) state = { ...state, autonomySaving: null };
    this.#set(this.#withAnswer(state, request, answer));
  }

  /** Native controls keep their requested value visible until the same session acknowledges it. */
  async setControl(change: SessionControlChange): Promise<void> {
    if (!this.#transport.setControl) {
      this.#set({
        ...this.#state,
        listsError: "Native session controls are unavailable on this connection.",
      });
      return;
    }
    const request = ++this.#listsRequest;
    const generation = this.#generation;
    const nativeId = this.#state.session?.native_session_id;
    const agentId = this.#state.chat?.agent_id;
    this.#controlRequest = request;
    this.#set({ ...this.#state, controlSaving: change, listsError: null });
    const answer = await this.#listsAnswer(() => this.#transport.setControl!(this.chatId, change));
    if (this.#closed || generation !== this.#generation) return;
    if (
      nativeId !== this.#state.session?.native_session_id ||
      agentId !== this.#state.chat?.agent_id
    ) {
      if (this.#controlRequest === request) this.#set({ ...this.#state, controlSaving: null });
      return;
    }
    if (this.#controlRequest !== request) return;
    const state = { ...this.#state, controlSaving: null };
    this.#set(this.#withAnswer(state, request, answer));
  }

  async #listsAnswer(
    call: () => Promise<SessionLists>,
  ): Promise<{ lists: SessionLists } | { error: string }> {
    try {
      return { lists: await call() };
    } catch (error) {
      return { error: errorMessage(error) };
    }
  }

  /** The lists of request `request`'s answer, unless a newer request was made; a refusal always shows. */
  #withAnswer(
    state: ChatState,
    request: number,
    answer: { lists: SessionLists } | { error: string },
  ): ChatState {
    if ("error" in answer) return { ...state, listsError: answer.error };
    if (request !== this.#listsRequest) return state;
    return { ...state, lists: answer.lists, listsError: null };
  }

  // ---------- prompts and the session ----------

  /** Sends the text exactly as typed. Answers whether it was taken (the composer then clears). */
  async send(text: string, context?: import("../composer").ComposerContext): Promise<boolean> {
    if (text.trim() === "" && !context?.attachments.length && !context?.mentions.length)
      return false;
    try {
      if (context && (context.attachments.length || context.mentions.length)) {
        if (!this.#transport.composer)
          throw new Error("Attachments are unavailable in this session.");
        await this.#transport.composer.send(this.chatId, { text, ...context });
      } else {
        await this.#transport.send(this.chatId, text);
      }
      this.#set({ ...this.#state, sendError: null });
      return true;
    } catch (error) {
      this.#set({ ...this.#state, sendError: errorMessage(error) });
      return false;
    }
  }

  /** Steer only the turn the user saw. No fallback to sending or queuing a prompt. */
  async steer(
    expectedTurnId: string,
    text: string,
    context?: import("../composer").ComposerContext,
  ): Promise<boolean> {
    if (this.#state.steeringPending) return false;
    if (
      this.#closed ||
      this.#state.phase !== "live" ||
      this.#state.session?.status !== "running" ||
      this.#state.openTurnId !== expectedTurnId ||
      !this.#state.lists?.capabilities?.steer ||
      !this.#transport.composer?.steer
    ) {
      this.#set({ ...this.#state, sendError: "This turn is no longer available for steering." });
      return false;
    }
    if (!text.trim() && !context?.attachments.length && !context?.mentions.length) return false;
    this.#set({ ...this.#state, steeringPending: true, sendError: null });
    try {
      const answer = await this.#transport.composer.steer(this.chatId, expectedTurnId, {
        text,
        attachments: [...(context?.attachments ?? [])],
        mentions: [...(context?.mentions ?? [])],
      });
      const accepted = answer.state === "accepted";
      this.#set({
        ...this.#state,
        sendError: accepted
          ? null
          : (answer.detail ??
            (answer.state === "rejected"
              ? "The agent did not accept this steering message. Your draft is kept."
              : "Delivery is unconfirmed. Your draft is kept; review the turn before sending again.")),
      });
      return accepted;
    } catch (error) {
      this.#set({ ...this.#state, sendError: errorMessage(error) });
      return false;
    } finally {
      this.#set({ ...this.#state, steeringPending: false });
    }
  }

  cancelQueued(promptId: string): Promise<void> {
    const request = this.#queueRequest();
    return this.#act(
      () => this.#transport.cancelQueued(this.chatId, promptId),
      (error) => {
        if (isCode(error, "request_not_pending")) {
          this.#queueActionFailed(promptId, error, request);
          return true;
        }
        return false;
      },
    );
  }

  /**
   * The words of a queued prompt as typed, for editing it in place; `null` when it is gone (it
   * started or was removed meanwhile) or could not be read, which `actionError` then says.
   */
  async queuedText(promptId: string): Promise<string | null> {
    const request = this.#queueRequest();
    try {
      const text = await this.#transport.queuedText(this.chatId, promptId);
      if (this.#state.actionError !== null) this.#set({ ...this.#state, actionError: null });
      return text;
    } catch (error) {
      this.#queueActionFailed(promptId, error, request);
      return null;
    }
  }

  /**
   * Saves new words for a queued prompt. Answers why they were not saved (shown under the editor,
   * which stays open with the words), or `null` once saved or once the prompt is gone.
   */
  async editQueued(promptId: string, text: string): Promise<string | null> {
    const request = this.#queueRequest();
    try {
      this.#queueChanged(await this.#transport.editQueued(this.chatId, promptId, text), request);
      return null;
    } catch (error) {
      if (!isCode(error, "request_not_pending")) return errorMessage(error);
      this.#queueActionFailed(promptId, error, request);
      return null;
    }
  }

  /** Moves a queued prompt to `index` (0 goes next). */
  async moveQueued(promptId: string, index: number): Promise<void> {
    const request = this.#queueRequest();
    try {
      this.#queueChanged(await this.#transport.moveQueued(this.chatId, promptId, index), request);
    } catch (error) {
      this.#queueActionFailed(promptId, error, request);
    }
  }

  #queueRequest(): QueueRequest {
    return {
      generation: this.#generation,
      revision: this.#queueRevision,
      attaching: this.#attaching,
    };
  }

  /**
   * Queue replies have no event sequence. If frames, another reply or a new subscription changed
   * the queue meanwhile, only an attach can order the saved edit/move against those changes.
   */
  #queueReplyIsCurrent(request: QueueRequest): boolean {
    if (this.#closed) return false;
    if (
      request.attaching ||
      this.#attaching ||
      request.generation !== this.#generation ||
      request.revision !== this.#queueRevision
    ) {
      this.#rejoinSoon();
      return false;
    }
    return true;
  }

  /** The answer is exact when the action ran, but frames can reach the view before it does. */
  #queueChanged(queue: readonly QueuedPrompt[], request: QueueRequest): void {
    if (this.#closed) return;
    this.#set({
      ...this.#state,
      queue: this.#queueReplyIsCurrent(request) ? queue : this.#state.queue,
      actionError: null,
    });
  }

  #queueActionFailed(promptId: string, error: unknown, request: QueueRequest): void {
    if (this.#closed) return;
    if (isCode(error, "request_not_pending")) {
      // It was absent when the action ran, but may have returned to the queue since then.
      if (!this.#queueReplyIsCurrent(request)) return;
      this.#set({
        ...this.#state,
        queue: this.#state.queue.filter((row) => row.prompt_id !== promptId),
      });
      return;
    }
    this.#set({ ...this.#state, actionError: errorMessage(error) });
  }

  resumeQueue(): Promise<void> {
    return this.#act(() => this.#transport.resumeQueue(this.chatId));
  }

  /**
   * "Send now": the prompt goes next, released if it was held; a running turn is interrupted
   * first. The shell's answer is the exact queue after the change.
   */
  async sendNow(promptId: string): Promise<void> {
    const request = this.#queueRequest();
    try {
      this.#queueChanged(await this.#transport.sendNow(this.chatId, promptId), request);
    } catch (error) {
      this.#queueActionFailed(promptId, error, request);
    }
  }

  start(fresh: boolean): Promise<void> {
    return this.#act(() => this.#transport.start(this.chatId, fresh));
  }

  /**
   * Starts a stopped chat's agent because the person opened the chat, so its modes and models are
   * there before the first message. Nobody asked for it, so a refusal (no free slot, the chat is
   * open in a terminal, its folder is gone) says nothing: sending a message still starts it and
   * says why not.
   */
  async warm(): Promise<void> {
    try {
      await this.#transport.start(this.chatId, false);
    } catch {
      // Left stopped.
    }
  }

  stop(): Promise<void> {
    return this.#act(() => this.#transport.stop(this.chatId));
  }

  interrupt(): Promise<void> {
    this.#interruptAsked = true;
    return this.#act(() => this.#transport.interrupt(this.chatId));
  }

  forceStop(): Promise<void> {
    return this.#act(() => this.#transport.forceStop(this.chatId));
  }

  /** Esc: interrupts a running (or starting) turn; the next Esc, while it cancels, force-stops. */
  escape(): Promise<void> | null {
    const status = this.#state.session?.status;
    if (status === "cancelling" || (status === "running" && this.#interruptAsked)) {
      return this.forceStop();
    }
    if (status === "running" || status === "starting") return this.interrupt();
    return null;
  }

  revealLog(): Promise<void> {
    return this.#act(() => this.#transport.revealLog(this.chatId));
  }

  async #act(
    call: () => Promise<void>,
    handled: (error: unknown) => boolean = () => false,
  ): Promise<void> {
    try {
      await call();
      if (this.#state.actionError !== null) this.#set({ ...this.#state, actionError: null });
    } catch (error) {
      if (!handled(error)) this.#set({ ...this.#state, actionError: errorMessage(error) });
    }
  }

  // ---------- cards and questions ----------

  /** Answers a permission card (`null` dismisses it). Its buttons stay disabled meanwhile. */
  respondPermission(requestId: string, optionId: string | null): Promise<void> {
    return this.#answer(requestId, () =>
      this.#transport.respondPermission(this.chatId, requestId, optionId),
    );
  }

  /**
   * Accepts a plan waiting for approval as the person edited it. Rejects where this agent
   * takes no edited plan, so the card can say so and keep the edits.
   */
  async acceptPlan(requestId: string, optionId: string, plan: string): Promise<void> {
    const accept = this.#transport.acceptPlan;
    if (!accept) throw new Error("This agent cannot take an edited plan.");
    await accept.call(this.#transport, this.chatId, requestId, optionId, plan);
  }

  /**
   * Declines a plan waiting for approval with the person's words, which the agent reads with
   * the refusal. Rejects where this agent takes none, so the card can say so and keep the note.
   */
  async declinePlan(requestId: string, optionId: string, note: string): Promise<void> {
    const decline = this.#transport.declinePlan;
    if (!decline) throw new Error("This agent cannot be sent a note with a declined plan.");
    await decline.call(this.#transport, this.chatId, requestId, optionId, note);
  }

  /** Answers a question (`null` dismisses it). */
  respondUi(requestId: string, answer: UiAnswer | null): Promise<void> {
    return this.#answer(requestId, () => this.#transport.respondUi(this.chatId, requestId, answer));
  }

  async #answer(requestId: string, call: () => Promise<void>): Promise<void> {
    if (this.#state.requests.get(requestId)?.busy) return;
    this.#setRequest(requestId, { busy: true, gone: false, error: null });
    try {
      await call();
      // Disabled until the resolution arrives in a frame.
    } catch (error) {
      if (isCode(error, "request_not_pending")) {
        this.#setRequest(requestId, { busy: false, gone: true, error: null });
      } else {
        this.#setRequest(requestId, { busy: false, gone: false, error: errorMessage(error) });
      }
    }
  }

  #setRequest(requestId: string, request: RequestState): void {
    const requests = new Map(this.#state.requests);
    requests.set(requestId, request);
    this.#set({ ...this.#state, requests });
  }

  // ---------- full outputs ----------

  /** Loads the next page of a truncated output (256 KiB, ending on a character boundary). */
  async loadBlob(sha256: string): Promise<void> {
    const current = this.#state.blobs.get(sha256);
    if (current?.loading || current?.eof) return;
    const offset = current?.nextOffset ?? 0;
    this.#setBlob(sha256, {
      text: current?.text ?? "",
      nextOffset: offset,
      total: current?.total ?? 0,
      eof: false,
      loading: true,
      error: null,
    });
    try {
      const page = await this.#transport.blob(this.chatId, sha256, offset, BLOB_PAGE);
      this.#setBlob(sha256, {
        text: (current?.text ?? "") + page.text,
        nextOffset: page.next_offset,
        total: page.total,
        eof: page.eof,
        loading: false,
        error: null,
      });
    } catch (error) {
      this.#setBlob(sha256, {
        text: current?.text ?? "",
        nextOffset: offset,
        total: current?.total ?? 0,
        eof: false,
        loading: false,
        error: errorMessage(error),
      });
    }
  }

  #setBlob(sha256: string, blob: BlobState): void {
    const blobs = new Map(this.#state.blobs);
    blobs.set(sha256, blob);
    this.#set({ ...this.#state, blobs });
  }

  // ---------- still working ----------

  /** Re-arms the hint: it shows a minute after the last frame of a running turn, then counts up. */
  #armStillWorking(): void {
    if (this.#stillTimer !== null) {
      clearTimeout(this.#stillTimer);
      this.#stillTimer = null;
    }
    if (this.#closed || this.#state.session?.status !== "running") {
      if (this.#state.stillWorkingMinutes !== null) {
        this.#set({ ...this.#state, stillWorkingMinutes: null });
      }
      return;
    }
    const quiet = this.#now() - this.#lastActivity;
    const minutes = Math.floor(quiet / STILL_WORKING_MS);
    if (minutes >= 1 && this.#state.stillWorkingMinutes !== minutes) {
      this.#set({ ...this.#state, stillWorkingMinutes: minutes });
    }
    const nextAt = (minutes + 1) * STILL_WORKING_MS - quiet;
    this.#stillTimer = setTimeout(() => {
      this.#stillTimer = null;
      this.#armStillWorking();
    }, nextAt);
  }

  #clearTimers(): void {
    for (const timer of this.#heldOptions.values()) clearTimeout(timer);
    this.#heldOptions.clear();
    if (this.#listsTimer !== null) clearTimeout(this.#listsTimer);
    if (this.#stillTimer !== null) clearTimeout(this.#stillTimer);
    if (this.#rejoinTimer !== null) clearTimeout(this.#rejoinTimer);
    this.#listsTimer = null;
    this.#stillTimer = null;
    this.#rejoinTimer = null;
  }

  /**
   * Every change updates the state at once, so nothing is lost and order is kept. While the page
   * is hidden (minimized, or closed to the tray) the view is not told: a stream would otherwise
   * render every frame nobody sees. It renders once, with everything, when the page shows again.
   */
  #set(state: ChatState): void {
    if (state.queue !== this.#state.queue) this.#queueRevision += 1;
    this.#state = state;
    if (this.#visibility.hidden()) {
      this.#unrendered = true;
      return;
    }
    this.#notify();
  }

  readonly #visibilityChanged = (): void => {
    if (this.#unrendered && !this.#visibility.hidden()) this.#notify();
  };

  #notify(): void {
    this.#unrendered = false;
    for (const listener of [...this.#listeners]) listener();
  }
}

function pageEntries(page: HistoryPage): Entry[] {
  return page.items.map((item) => ({ seq: item.seq, turnId: item.turn_id, event: item.event }));
}

function without<V>(map: ReadonlyMap<string, V>, key: string): ReadonlyMap<string, V> {
  if (!map.has(key)) return map;
  const copy = new Map(map);
  copy.delete(key);
  return copy;
}

/**
 * Whether `lists` show `value` for `optionId`, as current or as pending. Lists without the option
 * say nothing either way, so they count as showing it.
 */
export function showsValue(lists: SessionLists, optionId: string, value: ConfigValue): boolean {
  const option = lists.options.find((option) => option.id === optionId);
  if (!option) return true;
  const pending = option.pending;
  if (pending !== null && pending.type === value.type && pending.value === value.value) return true;
  if (option.kind.type === "select")
    return value.type !== "choice" || option.kind.current === value.value;
  return value.type !== "toggle" || option.kind.current === value.value;
}

function sameStorage(a: StorageStatus, b: StorageStatus): boolean {
  return a.unsaved_bytes === b.unsaved_bytes && JSON.stringify(a.state) === JSON.stringify(b.state);
}
