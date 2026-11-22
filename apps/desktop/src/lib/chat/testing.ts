import type {
  AgentAuthStatus,
  AgentEntry,
  AgentMaintenance,
  Autonomy,
  AutonomyState,
  BackgroundTask,
  BlobText,
  ChatAttach,
  ChatSnapshot,
  ChatStatusUpdate,
  ChatSummary,
  ConfigValue,
  EnvStatus,
  EventBatch,
  HistoryItem,
  HistoryPage,
  O7Event,
  PromptAck,
  QueuedPrompt,
  SessionControlChange,
  SessionInfo,
  SessionLists,
  StorageStatus,
  TaskStep,
  ToolCall,
  ToolUpdate,
  UiAnswer,
} from "../../bindings";
import type { AgentAccountsApi } from "../agentAccounts";
import type { TerminalTransport } from "../terminal";
import { type EventEntry, UNANNOUNCED_TOOL_TITLE } from "./transcript";
import type { Transport } from "./transport";

/**
 * Test support for the chat stores and the transcript: a scripted shell, builders for the protocol
 * values, and the compaction rules of the M1a spec (6.3.1) as o7-core's `compact.rs` applies
 * them, so compacted history can be checked against the live stream before the real chat store
 * exists in this package. Imported by tests only.
 */

// ---------- builders ----------

export function sessionInfo(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    status: "ready",
    detail: null,
    native_session_id: null,
    agent_name: "Demo agent",
    agent_version: null,
    title: null,
    model: null,
    effort: null,
    mode: null,
    autonomy: "full_auto",
    queue_held: false,
    revision: 1,
    queue_wait: null,
    sending: null,
    worktree_setup: null,
    ...overrides,
  };
}

export function chatSummary(id: string, overrides: Partial<ChatSummary> = {}): ChatSummary {
  return {
    id,
    title: null,
    agent_id: "mock",
    agent_host: { type: "native" },
    agent_name: "Demo agent",
    folder: "/Users/me/code/app",
    folder_state: "ok",
    status: "ready",
    needs_attention: false,
    created_at: 1,
    updated_at: 1,
    ...overrides,
  };
}

export function snapshot(chatId: string, overrides: Partial<ChatSnapshot> = {}): ChatSnapshot {
  return {
    chat: chatSummary(chatId),
    session: sessionInfo(),
    queue: [],
    open_turn_id: null,
    usage: null,
    limits: null,
    last_error: null,
    auth: null,
    storage: { state: { state: "ok" }, unsaved_bytes: 0 },
    ...overrides,
  };
}

export function historyPage(
  chatId: string,
  items: readonly HistoryItem[],
  overrides: Partial<HistoryPage> = {},
): HistoryPage {
  return {
    chat_id: chatId,
    items: [...items],
    has_more: false,
    before_seq: null,
    cut_turn_id: null,
    unsaved_from: null,
    skipped: 0,
    ...overrides,
  };
}

/** History items from entries, as a page lists them. */
export function items(entries: readonly EventEntry[]): HistoryItem[] {
  return entries.map((entry) => ({
    seq: entry.seq,
    turn_id: entry.turnId,
    at: 0,
    event: entry.event,
  }));
}

export function attachResult(
  chatId: string,
  nextSeq: number,
  history: HistoryPage,
  overrides: Partial<ChatSnapshot> = {},
): ChatAttach {
  return {
    subscription_id: `sub-${chatId}-${nextSeq}`,
    next_seq: nextSeq,
    snapshot: snapshot(chatId, overrides),
    history,
  };
}

export function frame(chatId: string, seqStart: number, events: readonly O7Event[]): EventBatch {
  return { chat_id: chatId, seq_start: seqStart, events: [...events] };
}

export function autonomyState(requested: Autonomy = "full_auto"): AutonomyState {
  return { requested, native_mode: null, auto_answer: { type: "never" }, note: null };
}

export function lists(revision: number, overrides: Partial<SessionLists> = {}): SessionLists {
  return {
    revision,
    capabilities: null,
    options: [],
    commands: [],
    loaded: null,
    autonomy: autonomyState(),
    controls: {
      version: 1,
      access: {
        requested: null,
        effective: null,
        pending: null,
        choices: [],
        native_mode: null,
        note: null,
      },
      workflow: {
        requested: null,
        effective: null,
        pending: null,
        choices: [],
        native_mode: null,
        note: null,
      },
    },
    ...overrides,
  };
}

/** Events in the shapes drivers and the session actor send. */
/** A running subagent's record, as a driver sends it. */
export function backgroundTask(
  id: string,
  overrides: Partial<BackgroundTask> = {},
): BackgroundTask {
  return {
    task_id: id,
    kind: "agent",
    title: `Task ${id}`,
    agent_type: null,
    tool_call_id: null,
    parent_task_id: null,
    backgrounded: true,
    status: "running",
    activity: null,
    tool_uses: null,
    tokens: null,
    duration_ms: null,
    started_at: null,
    ended_at: null,
    prompt: null,
    summary: null,
    summary_truncated: null,
    ...overrides,
  };
}

export const ev = {
  turnStarted: (turnId: string, promptId: string | null = null): O7Event => ({
    type: "turn_started",
    turn_id: turnId,
    prompt_id: promptId,
  }),
  turnEnded: (
    turnId: string,
    stopReason: Extract<O7Event, { type: "turn_ended" }>["stop_reason"] = "end_turn",
    extra: { recovered?: boolean; durationMs?: number | null } = {},
  ): O7Event => ({
    type: "turn_ended",
    turn_id: turnId,
    stop_reason: stopReason,
    usage: null,
    duration_ms: extra.durationMs ?? null,
    recovered: extra.recovered ?? false,
  }),
  user: (messageId: string, text: string): O7Event => ({
    type: "user_message_delta",
    message_id: messageId,
    text,
  }),
  message: (messageId: string, text: string): O7Event => ({
    type: "message_delta",
    message_id: messageId,
    text,
  }),
  messageDone: (messageId: string): O7Event => ({
    type: "message_completed",
    message_id: messageId,
  }),
  thought: (thoughtId: string, text: string): O7Event => ({
    type: "thought_delta",
    thought_id: thoughtId,
    text,
  }),
  thoughtDone: (thoughtId: string, durationMs: number | null): O7Event => ({
    type: "thought_completed",
    thought_id: thoughtId,
    duration_ms: durationMs,
  }),
  toolCall: (id: string, overrides: Partial<ToolCall> = {}): O7Event => ({
    type: "tool_call",
    tool_call_id: id,
    kind: "read",
    title: `Read ${id}`,
    status: "in_progress",
    locations: [],
    raw_input: null,
    tool_name: null,
    content: [],
    truncated: null,
    ...overrides,
  }),
  task: (id: string, overrides: Partial<BackgroundTask> = {}): O7Event => ({
    type: "task_updated",
    ...backgroundTask(id, overrides),
  }),
  taskStep: (taskId: string, stepId: string, overrides: Partial<TaskStep> = {}): O7Event => ({
    type: "task_step",
    task_id: taskId,
    step: {
      step_id: stepId,
      kind: "tool",
      title: `Read ${stepId}`,
      tool_kind: "read",
      status: "completed",
      text: null,
      truncated: null,
      ...overrides,
    },
  }),
  toolUpdate: (id: string, overrides: Partial<ToolUpdate> = {}): O7Event => ({
    type: "tool_update",
    tool_call_id: id,
    status: null,
    title: null,
    kind: null,
    locations: null,
    raw_input: null,
    tool_name: null,
    content: null,
    truncated: null,
    ...overrides,
  }),
  permission: (
    requestId: string,
    options: Extract<O7Event, { type: "permission_requested" }>["options"] = [
      { option_id: "allow", kind: "allow_once", label: "Allow once" },
      { option_id: "reject", kind: "reject_once", label: "Reject" },
    ],
  ): O7Event => ({
    type: "permission_requested",
    request_id: requestId,
    tool_call_id: null,
    title: "Run npm test",
    kind: "execute",
    raw_input: { command: "npm test" },
    content: [],
    locations: [],
    options,
    truncated: null,
  }),
  permissionResolved: (
    requestId: string,
    optionId: string | null,
    by: Extract<O7Event, { type: "permission_resolved" }>["by"] = "user",
    /** The plan as the person edited it, on a plan accepted with edits. */
    plan?: string,
  ): O7Event => ({
    type: "permission_resolved",
    request_id: requestId,
    decision:
      optionId === null
        ? { type: "cancelled" }
        : plan === undefined
          ? { type: "selected", option_id: optionId }
          : { type: "selected", option_id: optionId, plan },
    by,
  }),
  question: (requestId: string): O7Event => ({
    type: "ui_request",
    request_id: requestId,
    prompt: { type: "select", message: "Which color?", options: ["Red", "Green", "Blue"] },
  }),
  questionResolved: (
    requestId: string,
    answer: UiAnswer | null,
    by: Extract<O7Event, { type: "ui_resolved" }>["by"] = "user",
  ): O7Event => ({ type: "ui_resolved", request_id: requestId, answer, by }),
  plan: (...contents: string[]): O7Event => ({
    type: "plan_updated",
    entries: contents.map((content, index) => ({
      content,
      status: index === 0 ? "completed" : "pending",
      priority: null,
    })),
  }),
  usage: (amount: number): O7Event => ({
    type: "usage_updated",
    context_used: 1000,
    context_size: 200_000,
    cost: { amount, currency: "USD" },
  }),
  limits: (): O7Event => ({ type: "limits_updated", windows: [] }),
  session: (overrides: Partial<SessionInfo> = {}): O7Event => ({
    type: "session_updated",
    ...sessionInfo(overrides),
  }),
  lifecycle: (
    promptId: string,
    state: Extract<O7Event, { type: "prompt_lifecycle" }>["state"],
    preview: string | null = null,
    held: boolean | null = null,
  ): O7Event => ({ type: "prompt_lifecycle", prompt_id: promptId, state, preview, held }),
  notice: (message: string): O7Event => ({ type: "notice", level: "info", message }),
  error: (
    kind: Extract<O7Event, { type: "error" }>["kind"],
    message: string,
    detail: string | null = null,
  ): O7Event => ({ type: "error", kind, message, detail, recoverable: false }),
} as const;

/** Entries numbered from `first`: `[turn tag, event]` pairs as a driver tags them. */
export function numbered(
  tagged: readonly (readonly [string | null, O7Event])[],
  first = 0,
): EventEntry[] {
  return tagged.map(([turnId, event], index) => ({ seq: first + index, turnId, event }));
}

// ---------- the compaction rules (spec 6.3.1) ----------

/** Never stored: live header state, and usage outside a turn. */
export function isStored(entry: EventEntry): boolean {
  switch (entry.event.type) {
    case "session_updated":
    case "prompt_lifecycle":
    case "limits_updated":
      return false;
    case "usage_updated":
      return entry.turnId !== null;
    default:
      return true;
  }
}

/**
 * The call compaction synthesizes for a tool whose turn holds updates but no call: ACP's defaults
 * with the chat store's title (o7-core `compact.rs`, `default_tool`).
 */
function bareCall(id: string): ToolCall {
  return {
    tool_call_id: id,
    kind: "other",
    title: UNANNOUNCED_TOOL_TITLE,
    status: "pending",
    locations: [],
    raw_input: null,
    tool_name: null,
    content: [],
    truncated: null,
  };
}

function applyUpdate(call: ToolCall, update: ToolUpdate): ToolCall {
  return {
    ...call,
    kind: update.kind ?? call.kind,
    title: update.title ?? call.title,
    status: update.status ?? call.status,
    locations: update.locations ?? call.locations,
    raw_input: update.raw_input ?? call.raw_input,
    tool_name: update.tool_name ?? call.tool_name,
    content: update.content ?? call.content,
    truncated: update.truncated ?? call.truncated,
  };
}

function mergeUpdates(first: ToolUpdate, next: ToolUpdate): ToolUpdate {
  return {
    tool_call_id: first.tool_call_id,
    status: next.status ?? first.status,
    title: next.title ?? first.title,
    kind: next.kind ?? first.kind,
    locations: next.locations ?? first.locations,
    raw_input: next.raw_input ?? first.raw_input,
    tool_name: next.tool_name ?? first.tool_name,
    content: next.content ?? first.content,
    truncated: next.truncated ?? first.truncated,
  };
}

const UTF8 = new TextEncoder();

/**
 * One turn's rows as o7-core's `compact.rs` keeps them: each message, thought and user text
 * concatenated into pieces of at most `maxPiece` bytes at the seqs of their pieces' first deltas;
 * each tool minted in the turn (`<turn id>.` prefix) or called in it as one call with its final
 * state, at its call's seq (the call anchors it even after an update that came first) or, with no
 * call, synthesized at its first update's seq; updates to tools minted elsewhere merged into one
 * update at the first one's seq; the last plan and the last usage; everything else unchanged.
 */
function compactTurn(turnId: string, rows: readonly EventEntry[], maxPiece: number): EventEntry[] {
  /** A kept row; a text piece grows while the turn's deltas are read. */
  type Row = { seq: number; turnId: string | null; event: O7Event };
  const called = new Set(
    rows.flatMap((row) => (row.event.type === "tool_call" ? [row.event.tool_call_id] : [])),
  );
  const kept: Row[] = [];
  const pieces = new Map<string, Row>();
  const own = new Map<string, { seq: number; anchored: boolean; call: ToolCall }>();
  const foreign = new Map<string, { seq: number; update: ToolUpdate }>();
  let plan: Row | null = null;
  let usage: Row | null = null;
  // A task: its first row as it is, its latest state at its latest row with the summary kept.
  const tasks = new Map<
    string,
    {
      first: Row;
      latest: { seq: number; task: BackgroundTask } | null;
      summary: Pick<BackgroundTask, "summary" | "summary_truncated"> | null;
    }
  >();
  // A step: its latest state at its first row.
  const steps = new Map<string, Row>();
  for (const row of rows) {
    const { event } = row;
    switch (event.type) {
      case "task_updated": {
        const { type: _type, ...task } = event;
        const known = tasks.get(task.task_id);
        const summary =
          task.summary !== null
            ? { summary: task.summary, summary_truncated: task.summary_truncated }
            : (known?.summary ?? null);
        if (known) tasks.set(task.task_id, { ...known, latest: { seq: row.seq, task }, summary });
        else tasks.set(task.task_id, { first: { ...row }, latest: null, summary });
        break;
      }
      case "task_step": {
        const key = `${event.task_id}\u0000${event.step.step_id}`;
        const known = steps.get(key);
        if (known) known.event = event;
        else steps.set(key, { ...row });
        break;
      }
      case "user_message_delta":
      case "message_delta":
      case "thought_delta": {
        const id = event.type === "thought_delta" ? event.thought_id : event.message_id;
        const key = `${event.type}:${id}`;
        const piece = pieces.get(key);
        const pieceText = piece ? deltaText(piece.event) : "";
        const bytes = UTF8.encode(pieceText).length + UTF8.encode(event.text).length;
        if (piece && bytes <= maxPiece) {
          piece.event = withText(piece.event, pieceText + event.text);
        } else {
          const fresh: Row = { seq: row.seq, turnId, event };
          pieces.set(key, fresh);
          kept.push(fresh);
        }
        break;
      }
      case "tool_call": {
        const { type: _type, ...call } = event;
        const known = own.get(call.tool_call_id);
        own.set(call.tool_call_id, {
          seq: known?.anchored ? known.seq : row.seq,
          anchored: true,
          call,
        });
        break;
      }
      case "tool_update": {
        const { type: _type, ...update } = event;
        const id = update.tool_call_id;
        if (called.has(id) || id.startsWith(`${turnId}.`)) {
          const known = own.get(id);
          own.set(id, {
            seq: known?.seq ?? row.seq,
            anchored: known?.anchored ?? false,
            call: applyUpdate(known?.call ?? bareCall(id), update),
          });
        } else {
          const known = foreign.get(id);
          foreign.set(id, {
            seq: known?.seq ?? row.seq,
            update: known ? mergeUpdates(known.update, update) : update,
          });
        }
        break;
      }
      case "plan_updated":
        plan = { ...row };
        break;
      case "usage_updated":
        usage = { ...row };
        break;
      default:
        kept.push({ ...row });
    }
  }
  for (const { seq, call } of own.values()) {
    kept.push({ seq, turnId, event: { type: "tool_call", ...call } });
  }
  for (const { seq, update } of foreign.values()) {
    kept.push({ seq, turnId, event: { type: "tool_update", ...update } });
  }
  for (const { first, latest, summary } of tasks.values()) {
    kept.push(first);
    if (latest) {
      const task: BackgroundTask = {
        ...latest.task,
        prompt: null,
        ...(latest.task.summary === null && summary ? summary : {}),
      };
      kept.push({ seq: latest.seq, turnId, event: { type: "task_updated", ...task } });
    }
  }
  kept.push(...steps.values());
  if (plan) kept.push(plan);
  if (usage) kept.push(usage);
  return kept.sort((a, b) => a.seq - b.seq);
}

function deltaText(event: O7Event): string {
  return event.type === "user_message_delta" ||
    event.type === "message_delta" ||
    event.type === "thought_delta"
    ? event.text
    : "";
}

function withText(event: O7Event, text: string): O7Event {
  return event.type === "user_message_delta" ||
    event.type === "message_delta" ||
    event.type === "thought_delta"
    ? { ...event, text }
    : event;
}

/**
 * The history a live stream leaves once every turn is compacted: the stored events only, rows
 * outside turns unchanged, each turn compacted on its own (the rows tagged with it).
 */
export function compactHistory(live: readonly EventEntry[], maxPiece = 1024 * 1024): EventEntry[] {
  const stored = live.filter(isStored);
  const outside = stored.filter((entry) => entry.turnId === null);
  const byTurn = new Map<string, EventEntry[]>();
  for (const entry of stored) {
    if (entry.turnId === null) continue;
    const rows = byTurn.get(entry.turnId) ?? [];
    rows.push(entry);
    byTurn.set(entry.turnId, rows);
  }
  const compacted = [...byTurn].flatMap(([turnId, rows]) => compactTurn(turnId, rows, maxPiece));
  return [...outside, ...compacted].sort((a, b) => a.seq - b.seq);
}

// ---------- a scripted shell ----------

/** An attach the test answers: frames can arrive before it resolves, as in the real join. */
export interface PendingAttach {
  readonly chatId: string;
  readonly onFrame: (frame: EventBatch) => void;
  readonly onEnd: () => void;
  readonly resolve: (attach: ChatAttach) => void;
  readonly reject: (error: unknown) => void;
}

export interface FakeCall {
  readonly method: keyof Transport;
  readonly args: readonly unknown[];
}

/**
 * Records every call. Each method answers with its handler from `handlers` when there is one,
 * else a plain default; `attach` waits for the test (`attaches`), and status subscriptions are
 * kept so the test can push updates.
 */
export class FakeTransport implements Transport {
  readonly calls: FakeCall[] = [];
  readonly attaches: PendingAttach[] = [];
  readonly statusChannels: {
    readonly onUpdate: (update: ChatStatusUpdate) => void;
    readonly onEnd: () => void;
  }[] = [];
  handlers: Partial<Transport>;
  /** Added agent accounts; absent unless a test gives some. */
  accounts?: AgentAccountsApi;
  /** An agent's update in the terminal sheet; absent unless a test gives one. */
  updateTerminal?: TerminalTransport;

  constructor(handlers: Partial<Transport> = {}) {
    this.handlers = handlers;
    this.accounts = handlers.accounts;
    this.updateTerminal = handlers.updateTerminal;
  }

  /** The calls of one method, in order. */
  callsOf(method: keyof Transport): readonly (readonly unknown[])[] {
    return this.calls.filter((call) => call.method === method).map((call) => call.args);
  }

  /** The method names called so far, in order. */
  methods(): (keyof Transport)[] {
    return this.calls.map((call) => call.method);
  }

  lastAttach(): PendingAttach {
    const last = this.attaches[this.attaches.length - 1];
    if (!last) throw new Error("nothing attached");
    return last;
  }

  #record(method: keyof Transport, args: readonly unknown[]): void {
    this.calls.push({ method, args });
  }

  agentsList(): Promise<AgentEntry[]> {
    this.#record("agentsList", []);
    return this.handlers.agentsList?.() ?? Promise.resolve([]);
  }

  agentsRefresh(): Promise<AgentEntry[]> {
    this.#record("agentsRefresh", []);
    return this.handlers.agentsRefresh?.() ?? Promise.resolve([]);
  }

  agentMaintenance(agentId: string): Promise<AgentMaintenance> {
    this.#record("agentMaintenance", [agentId]);
    return (
      this.handlers.agentMaintenance?.(agentId) ??
      Promise.resolve({
        auth_status: false,
        login_available: false,
        update_unavailable: "No vendor commands.",
      })
    );
  }

  agentAuthStatus(agentId: string): Promise<AgentAuthStatus> {
    this.#record("agentAuthStatus", [agentId]);
    return this.handlers.agentAuthStatus?.(agentId) ?? Promise.resolve({ state: "unsupported" });
  }

  agentUpdate(agentId: string): Promise<void> {
    this.#record("agentUpdate", [agentId]);
    return this.handlers.agentUpdate?.(agentId) ?? Promise.resolve();
  }

  agentUpdateOutput(agentId: string): Promise<string | null> {
    this.#record("agentUpdateOutput", [agentId]);
    return this.handlers.agentUpdateOutput?.(agentId) ?? Promise.resolve(null);
  }

  agentSetConnection(agentId: string, connection: string | null): Promise<AgentEntry[]> {
    this.#record("agentSetConnection", [agentId, connection]);
    return this.handlers.agentSetConnection?.(agentId, connection) ?? Promise.resolve([]);
  }

  envStatus(): Promise<EnvStatus> {
    this.#record("envStatus", []);
    return this.handlers.envStatus?.() ?? Promise.resolve(READY_ENV);
  }

  envRefresh(): Promise<EnvStatus> {
    this.#record("envRefresh", []);
    return (
      this.handlers.envRefresh?.() ??
      Promise.resolve({ state: { state: "resolving" }, removed: [] })
    );
  }

  pickFolder(): Promise<string | null> {
    this.#record("pickFolder", []);
    return this.handlers.pickFolder?.() ?? Promise.resolve(null);
  }

  chatsList(): Promise<ChatSummary[]> {
    this.#record("chatsList", []);
    return this.handlers.chatsList?.() ?? Promise.resolve([]);
  }

  chatCreate(agentId: string, folder: string): Promise<ChatSummary> {
    this.#record("chatCreate", [agentId, folder]);
    return (
      this.handlers.chatCreate?.(agentId, folder) ??
      Promise.resolve(chatSummary("new-chat", { agent_id: agentId, folder, status: "starting" }))
    );
  }

  statusSubscribe(
    onUpdate: (update: ChatStatusUpdate) => void,
    onEnd: () => void,
  ): Promise<string> {
    this.#record("statusSubscribe", []);
    this.statusChannels.push({ onUpdate, onEnd });
    return (
      this.handlers.statusSubscribe?.(onUpdate, onEnd) ??
      Promise.resolve(`status-${this.statusChannels.length}`)
    );
  }

  storageStatus(): Promise<StorageStatus> {
    this.#record("storageStatus", []);
    return this.handlers.storageStatus?.() ?? Promise.resolve(OK_STORAGE);
  }

  attach(
    chatId: string,
    onFrame: (frame: EventBatch) => void,
    onEnd: () => void,
  ): Promise<ChatAttach> {
    this.#record("attach", [chatId]);
    const handled = this.handlers.attach?.(chatId, onFrame, onEnd);
    if (handled) return handled;
    return new Promise((resolve, reject) => {
      this.attaches.push({ chatId, onFrame, onEnd, resolve, reject });
    });
  }

  detach(subscriptionId: string): Promise<void> {
    this.#record("detach", [subscriptionId]);
    return this.handlers.detach?.(subscriptionId) ?? Promise.resolve();
  }

  history(chatId: string, beforeSeq: number, maxBytes?: number): Promise<HistoryPage> {
    this.#record("history", [chatId, beforeSeq]);
    return (
      this.handlers.history?.(chatId, beforeSeq, maxBytes) ??
      Promise.resolve(historyPage(chatId, []))
    );
  }

  lists(chatId: string): Promise<SessionLists> {
    this.#record("lists", [chatId]);
    return this.handlers.lists?.(chatId) ?? Promise.resolve(lists(1));
  }

  send(chatId: string, text: string): Promise<PromptAck> {
    this.#record("send", [chatId, text]);
    return (
      this.handlers.send?.(chatId, text) ?? Promise.resolve({ prompt_id: "p1", queued: false })
    );
  }

  cancelQueued(chatId: string, promptId: string): Promise<void> {
    this.#record("cancelQueued", [chatId, promptId]);
    return this.handlers.cancelQueued?.(chatId, promptId) ?? Promise.resolve();
  }

  queuedText(chatId: string, promptId: string): Promise<string> {
    this.#record("queuedText", [chatId, promptId]);
    return this.handlers.queuedText?.(chatId, promptId) ?? Promise.resolve("");
  }

  editQueued(chatId: string, promptId: string, text: string): Promise<QueuedPrompt[]> {
    this.#record("editQueued", [chatId, promptId, text]);
    return this.handlers.editQueued?.(chatId, promptId, text) ?? Promise.resolve([]);
  }

  moveQueued(chatId: string, promptId: string, index: number): Promise<QueuedPrompt[]> {
    this.#record("moveQueued", [chatId, promptId, index]);
    return this.handlers.moveQueued?.(chatId, promptId, index) ?? Promise.resolve([]);
  }

  resumeQueue(chatId: string): Promise<void> {
    this.#record("resumeQueue", [chatId]);
    return this.handlers.resumeQueue?.(chatId) ?? Promise.resolve();
  }

  sendNow(chatId: string, promptId: string): Promise<QueuedPrompt[]> {
    this.#record("sendNow", [chatId, promptId]);
    return this.handlers.sendNow?.(chatId, promptId) ?? Promise.resolve([]);
  }

  interrupt(chatId: string): Promise<void> {
    this.#record("interrupt", [chatId]);
    return this.handlers.interrupt?.(chatId) ?? Promise.resolve();
  }

  forceStop(chatId: string): Promise<void> {
    this.#record("forceStop", [chatId]);
    return this.handlers.forceStop?.(chatId) ?? Promise.resolve();
  }

  start(chatId: string, fresh: boolean): Promise<void> {
    this.#record("start", [chatId, fresh]);
    return this.handlers.start?.(chatId, fresh) ?? Promise.resolve();
  }

  stop(chatId: string): Promise<void> {
    this.#record("stop", [chatId]);
    return this.handlers.stop?.(chatId) ?? Promise.resolve();
  }

  respondPermission(chatId: string, requestId: string, optionId: string | null): Promise<void> {
    this.#record("respondPermission", [chatId, requestId, optionId]);
    return this.handlers.respondPermission?.(chatId, requestId, optionId) ?? Promise.resolve();
  }

  acceptPlan(chatId: string, requestId: string, optionId: string, plan: string): Promise<void> {
    this.#record("acceptPlan", [chatId, requestId, optionId, plan]);
    return this.handlers.acceptPlan?.(chatId, requestId, optionId, plan) ?? Promise.resolve();
  }

  respondUi(chatId: string, requestId: string, answer: UiAnswer | null): Promise<void> {
    this.#record("respondUi", [chatId, requestId, answer]);
    return this.handlers.respondUi?.(chatId, requestId, answer) ?? Promise.resolve();
  }

  setOption(chatId: string, optionId: string, value: ConfigValue): Promise<SessionLists> {
    this.#record("setOption", [chatId, optionId, value]);
    return this.handlers.setOption?.(chatId, optionId, value) ?? Promise.resolve(lists(2));
  }

  setControl(chatId: string, change: SessionControlChange): Promise<SessionLists> {
    this.#record("setControl", [chatId, change]);
    return this.handlers.setControl?.(chatId, change) ?? Promise.resolve(lists(2));
  }

  setAutonomy(chatId: string, autonomy: Autonomy): Promise<SessionLists> {
    this.#record("setAutonomy", [chatId, autonomy]);
    return (
      this.handlers.setAutonomy?.(chatId, autonomy) ??
      Promise.resolve(lists(2, { autonomy: autonomyState(autonomy) }))
    );
  }

  blob(chatId: string, sha256: string, offset: number, len: number): Promise<BlobText> {
    this.#record("blob", [chatId, sha256, offset, len]);
    return (
      this.handlers.blob?.(chatId, sha256, offset, len) ??
      Promise.resolve({ sha256, offset, next_offset: offset, total: 0, text: "", eof: true })
    );
  }

  revealLog(chatId: string): Promise<void> {
    this.#record("revealLog", [chatId]);
    return this.handlers.revealLog?.(chatId) ?? Promise.resolve();
  }
}

export const READY_ENV: EnvStatus = {
  state: { state: "ready", source: "zsh login shell", took_ms: 120, vars: 40 },
  removed: [],
};

export const OK_STORAGE: StorageStatus = { state: { state: "ok" }, unsaved_bytes: 0 };

/** Resolves once every pending promise callback has run. */
export async function settled(): Promise<void> {
  for (let round = 0; round < 10; round += 1) await Promise.resolve();
}
