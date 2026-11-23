import type {
  BackgroundTask,
  ErrorKind,
  NoticeLevel,
  O7Event,
  PermissionDecision,
  PermissionRequest,
  PlanEntry,
  ResolvedBy,
  SessionStatus,
  SteeringState,
  StopReason,
  TokenUsage,
  ToolCall,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  ToolUpdate,
  Truncation,
  UiAnswer,
  UiRequest,
  UsageInfo,
  Value,
} from "../../bindings";
import { splitHandoff } from "./handoff";
import { type MemoryContext, splitMemoryContext } from "./memoryContext";
import { mergeTask, newestTurnKey, type TaskItem, withStep } from "./tasks";

/**
 * A chat's transcript, rebuilt from its events in seq order by a pure reducer. History pages and
 * live frames go through the same reducer, so a compacted page and the live stream it came from
 * render the same.
 *
 * Turns are grouped by position, from a `turn_started` to its `turn_ended`, never by the turn tag
 * a stored row carries: an untagged late event that landed inside a later turn's span renders in
 * that turn, in history and live alike. Events between turns form outside segments. Items keep the
 * order of their first appearance and are updated in place by id (a message's deltas, a tool's
 * updates, a request's resolution), even when the update arrives in a later turn. A turn's plan and
 * usage are its last ones, shown with the turn rather than at a position, since compaction keeps
 * only the last of each.
 */

/** One event at its place in the chat: a history item, or an event of a live frame. */
export interface EventEntry {
  readonly seq: number;
  /** The stored row's turn tag; live frames carry none. Grouping never reads it. */
  readonly turnId: string | null;
  readonly event: O7Event;
}

/** Where saving paused: rows from `seq` up to the attach point are not saved yet, so not shown. */
export interface GapEntry {
  readonly seq: number;
  readonly gap: true;
}

export type Entry = EventEntry | GapEntry;

export interface UserItem {
  readonly kind: "user";
  readonly key: string;
  /** What the person sent. Project memory attached after it is in `memory`, not here. */
  readonly text: string;
  /** Project memory o7 attached to this message as reference context, when it attached any. */
  readonly memory?: MemoryContext;
  /** The handoff this message carried to a new agent, before the person's words. */
  readonly handoff?: string;
  /** With `memory` or `handoff`: the whole saved message, which later pieces are appended to. */
  readonly sent?: string;
  readonly steering?: { readonly state: SteeringState; readonly detail: string | null };
}

/** A pending delivery cannot be claimed after its turn ends or the session is recovered. */
export function steeringState(item: UserItem, settled: boolean): SteeringState | undefined {
  const state = item.steering?.state;
  return settled && state === "pending" ? "unconfirmed" : state;
}

export interface MessageItem {
  readonly kind: "message";
  readonly key: string;
  readonly text: string;
  readonly completed: boolean;
}

export interface ThoughtItem {
  readonly kind: "thought";
  readonly key: string;
  readonly text: string;
  readonly completed: boolean;
  readonly durationMs: number | null;
}

export interface ToolItem {
  readonly kind: "tool";
  readonly key: string;
  readonly id: string;
  readonly toolKind: ToolKind;
  readonly title: string;
  readonly status: ToolStatus;
  readonly locations: readonly ToolLocation[];
  readonly rawInput: Value | null;
  readonly toolName: string | null;
  readonly content: readonly ToolContent[];
  /** More than entry text was cut; `blob` holds the original event's JSON ("Show full details"). */
  readonly truncated: Truncation | null;
}

export interface PermissionResolution {
  readonly decision: PermissionDecision;
  readonly by: ResolvedBy;
}

export interface PermissionItem {
  readonly kind: "permission";
  readonly key: string;
  readonly request: PermissionRequest;
  /** `null` while the card waits for an answer. */
  readonly resolution: PermissionResolution | null;
}

export interface QuestionResolution {
  /** `null`: dismissed, withdrawn or closed. */
  readonly answer: UiAnswer | null;
  readonly by: ResolvedBy;
}

export interface QuestionItem {
  readonly kind: "question";
  readonly key: string;
  readonly request: UiRequest;
  readonly resolution: QuestionResolution | null;
}

export interface NoticeItem {
  readonly kind: "notice";
  readonly key: string;
  readonly level: NoticeLevel;
  readonly message: string;
}

export interface ErrorItem {
  readonly kind: "error";
  readonly key: string;
  readonly errorKind: ErrorKind;
  readonly message: string;
  readonly detail: string | null;
  readonly recoverable: boolean;
}

/** The unsaved-rows marker. */
export interface GapItem {
  readonly kind: "gap";
  readonly key: string;
}

export type Item =
  | UserItem
  | MessageItem
  | ThoughtItem
  | ToolItem
  | PermissionItem
  | QuestionItem
  | NoticeItem
  | ErrorItem
  | GapItem;

export interface TurnEnd {
  readonly stopReason: StopReason;
  readonly usage: TokenUsage | null;
  readonly durationMs: number | null;
  /** Ended by recovery at startup: o7 quit or crashed while the turn ran. */
  readonly recovered: boolean;
}

/** A turn, or the events between two turns. */
export interface Segment {
  readonly key: string;
  /** `null` for the events between turns. */
  readonly turnId: string | null;
  readonly promptId: string | null;
  /** A history page cut this turn: its earlier part is in an older page. */
  readonly partial: boolean;
  readonly itemKeys: readonly string[];
  readonly plan: readonly PlanEntry[] | null;
  /** The turn's last usage report (context and cost). */
  readonly usage: UsageInfo | null;
  /** `null` while the turn runs (and always for the events between turns). */
  readonly end: TurnEnd | null;
}

export interface Transcript {
  readonly segments: readonly Segment[];
  readonly items: ReadonlyMap<string, Item>;
  /** Background tasks by id: not items, since the dock and the task panel show them (tasks.ts). */
  readonly tasks: ReadonlyMap<string, TaskItem>;
}

/**
 * An empty transcript. A history page that cut a turn (`cut_turn_id`) starts inside that turn, so
 * its first items land in it rather than between turns.
 */
export function emptyTranscript(cutTurnId: string | null = null): Transcript {
  return {
    segments: cutTurnId === null ? [] : [turnSegment(`turn:${cutTurnId}`, cutTurnId, null, true)],
    items: new Map(),
    tasks: new Map(),
  };
}

/**
 * The live session's tasks (the snapshot's) over the ones history gave: a task whose records are
 * all in an older page still shows, and a known one takes its newest record.
 */
export function withTasks(model: Transcript, tasks: readonly BackgroundTask[]): Transcript {
  if (tasks.length === 0) return model;
  const next = new Map(model.tasks);
  const turnKey = newestTurnKey(model.segments);
  tasks.forEach((task, index) => {
    next.set(task.task_id, mergeTask(next.get(task.task_id), task, index - tasks.length, turnKey));
  });
  return { ...model, tasks: next };
}

/** Applies entries in seq order. Returns `model` itself when there are none. */
export function applyEntries(model: Transcript, entries: readonly Entry[]): Transcript {
  if (entries.length === 0) return model;
  const draft = new Draft(model);
  for (const entry of entries) draft.apply(entry);
  return draft.finish();
}

/**
 * Folds each tool's run of updates into its first one, giving the same transcript with far fewer
 * entries. A tool's place comes from its first event, and an update only overrides the fields it
 * sets, so the later updates' set fields laid over the first update say the same thing. A
 * `tool_call` replaces the whole item, so updates fold only within the stretch after one. A
 * command's streamed output can store tens of thousands of updates, hundreds of megabytes.
 */
export function compactToolUpdates(entries: readonly Entry[]): Entry[] {
  const folded: Entry[] = [];
  const firstUpdate = new Map<string, number>();
  for (const entry of entries) {
    if ("gap" in entry) {
      folded.push(entry);
      continue;
    }
    const { event } = entry;
    if (event.type === "tool_call") firstUpdate.delete(event.tool_call_id);
    if (event.type !== "tool_update") {
      folded.push(entry);
      continue;
    }
    const at = firstUpdate.get(event.tool_call_id);
    const first = at === undefined ? undefined : folded[at];
    if (at === undefined || !first || "gap" in first || first.event.type !== "tool_update") {
      firstUpdate.set(event.tool_call_id, folded.length);
      folded.push(entry);
      continue;
    }
    const merged: Record<string, unknown> = { ...first.event };
    for (const [field, value] of Object.entries(event)) {
      if (value !== null && value !== undefined) merged[field] = value;
    }
    folded[at] = { ...first, event: merged as O7Event };
  }
  return folded;
}

/** The reducer for one event: `(model, seq, turnId, event) -> model`. */
export function reduce(
  model: Transcript,
  seq: number,
  turnId: string | null,
  event: O7Event,
): Transcript {
  return applyEntries(model, [{ seq, turnId, event }]);
}

/**
 * The title of a tool call that was never announced: the one the chat store gives the call it
 * synthesizes for such a tool when it compacts a turn, so the block reads the same live and from
 * history.
 */
export const UNANNOUNCED_TOOL_TITLE = "Tool";

/** A tool call that was never announced: ACP's defaults, with the chat store's title. */
function unannouncedTool(key: string, id: string): ToolItem {
  return {
    kind: "tool",
    key,
    id,
    toolKind: "other",
    title: UNANNOUNCED_TOOL_TITLE,
    status: "pending",
    locations: [],
    rawInput: null,
    toolName: null,
    content: [],
    truncated: null,
  };
}

function toolFromCall(key: string, call: ToolCall): ToolItem {
  return {
    kind: "tool",
    key,
    id: call.tool_call_id,
    toolKind: call.kind,
    title: call.title,
    status: call.status,
    locations: call.locations,
    rawInput: call.raw_input,
    toolName: call.tool_name,
    content: call.content,
    truncated: call.truncated,
  };
}

/** ACP's update semantics: `null` keeps a field; `content: []` clears the content. */
function updateTool(tool: ToolItem, update: ToolUpdate): ToolItem {
  return {
    ...tool,
    toolKind: update.kind ?? tool.toolKind,
    title: update.title ?? tool.title,
    status: update.status ?? tool.status,
    locations: update.locations ?? tool.locations,
    rawInput: update.raw_input ?? tool.rawInput,
    toolName: update.tool_name ?? tool.toolName,
    content: update.content ?? tool.content,
    truncated: update.truncated ?? tool.truncated,
  };
}

function turnSegment(
  key: string,
  turnId: string,
  promptId: string | null,
  partial: boolean,
): Segment {
  return { key, turnId, promptId, partial, itemKeys: [], plan: null, usage: null, end: null };
}

/** Copy-on-write working state for a batch of entries: each structure is copied at most once. */
class Draft {
  readonly #baseItems: ReadonlyMap<string, Item>;
  #items: Map<string, Item> | null = null;
  readonly #baseTasks: ReadonlyMap<string, TaskItem>;
  #tasks: Map<string, TaskItem> | null = null;
  readonly #segments: Segment[];
  /** The `itemKeys` arrays this draft made itself, by segment, so it may push to them. */
  readonly #ownedKeys = new Map<number, string[]>();

  constructor(base: Transcript) {
    this.#baseItems = base.items;
    this.#baseTasks = base.tasks;
    this.#segments = [...base.segments];
  }

  #setTask(id: string, change: (previous: TaskItem | undefined) => TaskItem): void {
    this.#tasks ??= new Map(this.#baseTasks);
    this.#tasks.set(id, change(this.#tasks.get(id)));
  }

  apply(entry: Entry): void {
    if ("gap" in entry) {
      this.#place(`gap:${entry.seq}`, entry.seq, { kind: "gap", key: `gap:${entry.seq}` });
      return;
    }
    const { seq, event } = entry;
    switch (event.type) {
      case "turn_started": {
        // A turn whose end never arrived stays as it is; the new one starts after it.
        this.#push(
          turnSegment(
            this.#freeKey(`turn:${event.turn_id}`, seq),
            event.turn_id,
            event.prompt_id,
            false,
          ),
        );
        break;
      }
      case "turn_ended": {
        const end: TurnEnd = {
          stopReason: event.stop_reason,
          usage: event.usage,
          durationMs: event.duration_ms,
          recovered: event.recovered,
        };
        const open = this.#openTurn();
        if (open === null) {
          // Its start is not in the loaded history: the end alone still says how it ended.
          const key = this.#freeKey(`turn:${event.turn_id}`, seq);
          this.#push({ ...turnSegment(key, event.turn_id, null, true), end });
        } else {
          this.#setSegment(open, { end });
        }
        break;
      }
      case "user_message_delta": {
        const key = `user:${event.message_id}`;
        const previous = this.#get(key);
        const sent =
          (previous?.kind === "user" ? (previous.sent ?? previous.text) : "") + event.text;
        const { text: words, memory } = splitMemoryContext(sent);
        // A message sent after a change of agent starts with the handoff the agent received.
        const split = splitHandoff(words);
        const text = split ? split.text : words;
        this.#place(key, seq, {
          kind: "user",
          key,
          text,
          ...(memory ? { memory } : {}),
          ...(split ? { handoff: split.handoff } : {}),
          // Without attached memory or a handoff the message is its own whole text.
          ...(memory || split ? { sent } : {}),
          ...(previous?.kind === "user" && previous.steering
            ? { steering: previous.steering }
            : {}),
        });
        break;
      }
      case "steering_status": {
        const key = `user:${event.message_id}`;
        const previous = this.#get(key);
        this.#place(key, seq, {
          ...(previous?.kind === "user" ? previous : { text: "" }),
          kind: "user",
          key,
          steering: { state: event.state, detail: event.detail },
        });
        break;
      }
      case "message_delta": {
        const key = `message:${event.message_id}`;
        const previous = this.#get(key);
        const message = previous?.kind === "message" ? previous : null;
        this.#place(key, seq, {
          kind: "message",
          key,
          text: (message?.text ?? "") + event.text,
          completed: message?.completed ?? false,
        });
        break;
      }
      case "message_completed": {
        const key = `message:${event.message_id}`;
        const previous = this.#get(key);
        this.#place(key, seq, {
          kind: "message",
          key,
          text: previous?.kind === "message" ? previous.text : "",
          completed: true,
        });
        break;
      }
      case "thought_delta": {
        const key = `thought:${event.thought_id}`;
        const previous = this.#get(key);
        const thought = previous?.kind === "thought" ? previous : null;
        this.#place(key, seq, {
          kind: "thought",
          key,
          text: (thought?.text ?? "") + event.text,
          completed: thought?.completed ?? false,
          durationMs: thought?.durationMs ?? null,
        });
        break;
      }
      case "thought_completed": {
        const key = `thought:${event.thought_id}`;
        const previous = this.#get(key);
        const thought = previous?.kind === "thought" ? previous : null;
        this.#place(key, seq, {
          kind: "thought",
          key,
          text: thought?.text ?? "",
          completed: true,
          durationMs: event.duration_ms ?? thought?.durationMs ?? null,
        });
        break;
      }
      case "tool_call": {
        const { type: _type, ...call } = event;
        const key = `tool:${call.tool_call_id}`;
        this.#place(key, seq, toolFromCall(key, call));
        break;
      }
      case "tool_update": {
        const { type: _type, ...update } = event;
        const key = `tool:${update.tool_call_id}`;
        const previous = this.#get(key);
        const tool =
          previous?.kind === "tool" ? previous : unannouncedTool(key, update.tool_call_id);
        this.#place(key, seq, updateTool(tool, update));
        break;
      }
      case "permission_requested": {
        const { type: _type, ...request } = event;
        const key = `permission:${request.request_id}`;
        this.#place(key, seq, { kind: "permission", key, request, resolution: null });
        break;
      }
      case "permission_resolved": {
        const key = `permission:${event.request_id}`;
        const previous = this.#get(key);
        // A resolution whose request is in an older page has no card to close yet.
        if (previous?.kind === "permission") {
          this.#place(key, seq, {
            ...previous,
            resolution: { decision: event.decision, by: event.by },
          });
        }
        break;
      }
      case "ui_request": {
        const { type: _type, ...request } = event;
        const key = `question:${request.request_id}`;
        this.#place(key, seq, { kind: "question", key, request, resolution: null });
        break;
      }
      case "ui_resolved": {
        const key = `question:${event.request_id}`;
        const previous = this.#get(key);
        if (previous?.kind === "question") {
          this.#place(key, seq, {
            ...previous,
            resolution: { answer: event.answer, by: event.by },
          });
        }
        break;
      }
      case "plan_updated":
        this.#setSegment(this.#target(seq), { plan: event.entries });
        break;
      case "task_updated": {
        const { type: _type, ...task } = event;
        const turnKey = newestTurnKey(this.#segments);
        this.#setTask(task.task_id, (previous) => mergeTask(previous, task, seq, turnKey));
        break;
      }
      case "task_step":
        this.#setTask(event.task_id, (previous) =>
          withStep(previous, event.task_id, event.step, seq),
        );
        break;
      case "usage_updated": {
        // Stored only inside a turn: between turns it is live header state, never history.
        const open = this.#openTurn();
        if (open !== null) {
          const { type: _type, ...usage } = event;
          this.#setSegment(open, { usage });
        }
        break;
      }
      case "limits_updated":
      case "session_updated":
      case "prompt_lifecycle":
        // Live state the chat store keeps (header, queue); never stored, so never history.
        break;
      case "notice": {
        const key = `notice:${seq}`;
        this.#place(key, seq, { kind: "notice", key, level: event.level, message: event.message });
        break;
      }
      case "error": {
        const key = `error:${seq}`;
        this.#place(key, seq, {
          kind: "error",
          key,
          errorKind: event.kind,
          message: event.message,
          detail: event.detail,
          recoverable: event.recoverable,
        });
        break;
      }
      default: {
        // Exhaustive: a new event type fails to compile here until it is handled.
        const unreachable: never = event;
        throw new Error(`unhandled event: ${JSON.stringify(unreachable)}`);
      }
    }
  }

  finish(): Transcript {
    return {
      segments: this.#segments,
      items: this.#items ?? this.#baseItems,
      tasks: this.#tasks ?? this.#baseTasks,
    };
  }

  #get(key: string): Item | undefined {
    return (this.#items ?? this.#baseItems).get(key);
  }

  /** Sets an item; a new one joins the segment events land in now. */
  #place(key: string, seq: number, item: Item): void {
    const known = (this.#items ?? this.#baseItems).has(key);
    this.#items ??= new Map(this.#baseItems);
    this.#items.set(key, item);
    if (known) return;
    const index = this.#target(seq);
    let keys = this.#ownedKeys.get(index);
    if (keys === undefined) {
      const segment = this.#segments[index];
      if (!segment) return;
      keys = [...segment.itemKeys];
      this.#segments[index] = { ...segment, itemKeys: keys };
      this.#ownedKeys.set(index, keys);
    }
    keys.push(key);
  }

  /** The turn still running at the end, if any. */
  #openTurn(): number | null {
    const index = this.#segments.length - 1;
    const last = this.#segments[index];
    return last && last.turnId !== null && last.end === null ? index : null;
  }

  /** Where an event lands: the running turn, the events after the last turn, or a new run of them. */
  #target(seq: number): number {
    const index = this.#segments.length - 1;
    const last = this.#segments[index];
    if (last && (last.turnId === null || last.end === null)) return index;
    return this.#push({
      key: this.#freeKey(`outside:${seq}`, seq),
      turnId: null,
      promptId: null,
      partial: false,
      itemKeys: [],
      plan: null,
      usage: null,
      end: null,
    });
  }

  #push(segment: Segment): number {
    const keys = [...segment.itemKeys];
    this.#segments.push({ ...segment, itemKeys: keys });
    const index = this.#segments.length - 1;
    this.#ownedKeys.set(index, keys);
    return index;
  }

  #setSegment(index: number, change: Partial<Segment>): void {
    const segment = this.#segments[index];
    if (segment) this.#segments[index] = { ...segment, ...change };
  }

  /** `key`, or `key#seq` when a segment already has it (a turn whose end went missing). */
  #freeKey(key: string, seq: number): string {
    return this.#segments.some((segment) => segment.key === key) ? `${key}#${seq}` : key;
  }
}

/** Whether a session in this status has an agent process. */
export function hasProcess(status: SessionStatus): boolean {
  switch (status) {
    case "starting":
    case "ready":
    case "running":
    case "cancelling":
    case "stopping":
      return true;
    case "stopped":
    case "failed":
    case "auth_required":
      return false;
  }
}

/** What a tool's status mark shows: its own status, or `stopped` when nothing will finish it. */
export type ToolDisplayStatus = ToolStatus | "stopped";

/**
 * A tool still pending or running shows as stopped when its turn ended cancelled, interrupted or
 * failed, or when the session has no process; otherwise it keeps what the agent reported (a
 * background task may still update it).
 */
export function toolDisplayStatus(
  tool: ToolItem,
  end: TurnEnd | null,
  live: boolean,
): ToolDisplayStatus {
  if (tool.status !== "pending" && tool.status !== "in_progress") return tool.status;
  if (!live) return "stopped";
  switch (end?.stopReason) {
    case "cancelled":
    case "interrupted":
    case "error":
      return "stopped";
    default:
      return tool.status;
  }
}

/** Why a turn ended, as its footer says it. */
export function stopReasonLabel(end: TurnEnd): string {
  switch (end.stopReason) {
    case "end_turn":
      return "Done";
    case "max_tokens":
      return "Stopped at the output limit";
    case "max_turn_requests":
      return "Stopped at the step limit";
    case "refusal":
      return "The agent declined";
    case "cancelled":
      return "Stopped";
    case "interrupted":
      return end.recovered ? "Interrupted when o7 closed" : "Interrupted";
    case "error":
      return "Failed";
    case "other":
      return "Ended";
  }
}

/**
 * An item as the transcript shows it: tools carry the status their mark shows, and thoughts
 * whether the agent is thinking them now. The agent's reasoning shows as its own row, folded to
 * "Thought for 4 s" once done (owner, 19 November 2026: make "it thinking, tool calling, etc."
 * sexier; on 2 November empty "Thought for 0 ms" rows were why reasoning was hidden, and a thought
 * with nothing to show still is: see `shownThought`).
 */
export type RenderedItem =
  | Exclude<Item, ToolItem | ThoughtItem>
  | (ToolItem & { display: ToolDisplayStatus })
  | (ThoughtItem & { thinking: boolean });

/** A thought shorter than this, with no words, is not worth a row. */
const QUIET_THOUGHT_MS = 1000;

/**
 * Whether a thought gets a row: while the agent thinks it, or once done when it has words or
 * took a second or more. One with neither (an agent that keeps its reasoning to itself, a blink)
 * stays in the model only.
 */
export function shownThought(thought: ThoughtItem, thinking: boolean): boolean {
  return thinking || thought.text.trim() !== "" || (thought.durationMs ?? 0) >= QUIET_THOUGHT_MS;
}

/** A thought is being thought while it is not done in a turn still running on a live session. */
function thinkingNow(thought: ThoughtItem, end: TurnEnd | null, live: boolean): boolean {
  return !thought.completed && end === null && live;
}

export interface RenderedSegment {
  readonly key: string;
  readonly turnId: string | null;
  readonly partial: boolean;
  readonly items: readonly RenderedItem[];
  readonly plan: readonly PlanEntry[] | null;
  readonly usage: UsageInfo | null;
  readonly end: TurnEnd | null;
}

/**
 * The transcript as the screen shows it, as plain data: what "compacted history and live stream
 * render the same" compares. `live`: the session has a process.
 */
export function renderTranscript(model: Transcript, live: boolean): RenderedSegment[] {
  return model.segments.map((segment) =>
    renderSegment(segment, itemsOf(model, segment, live), live),
  );
}

/** A segment's items the screen shows, in order: every kind, thoughts only where they say something. */
function itemsOf(model: Transcript, segment: Segment, live: boolean): Item[] {
  return segment.itemKeys.flatMap((key): Item[] => {
    const item = model.items.get(key);
    if (!item) return [];
    if (item.kind !== "thought") return [item];
    return shownThought(item, thinkingNow(item, segment.end, live)) ? [item] : [];
  });
}

function renderSegment(segment: Segment, items: readonly Item[], live: boolean): RenderedSegment {
  return {
    key: segment.key,
    turnId: segment.turnId,
    partial: segment.partial,
    plan: segment.plan,
    usage: segment.usage,
    end: segment.end,
    items: items.map(
      (item): RenderedItem =>
        item.kind === "tool"
          ? { ...item, display: toolDisplayStatus(item, segment.end, live) }
          : item.kind === "thought"
            ? { ...item, thinking: thinkingNow(item, segment.end, live) }
            : item,
    ),
  };
}

/**
 * `renderTranscript` that reuses each rendered segment whose segment and shown items did not
 * change, so a frame re-renders only the turn it touched (a memoized turn view compares by
 * identity). A thought with no row (see `shownThought`) leaves its segment's rendering as it was.
 */
export function renderCache(): (model: Transcript, live: boolean) => RenderedSegment[] {
  type Cached = {
    readonly live: boolean;
    readonly segment: Segment;
    readonly items: Item[];
    readonly rendered: RenderedSegment;
  };
  let previous = new Map<string, Cached>();
  return (model, live) => {
    const next = new Map<string, Cached>();
    const rendered = model.segments.map((segment) => {
      const items = itemsOf(model, segment, live);
      const cached = previous.get(segment.key);
      const reuse =
        cached !== undefined &&
        cached.live === live &&
        sameShown(cached.segment, segment) &&
        cached.items.length === items.length &&
        cached.items.every((item, index) => item === items[index]);
      const result = reuse ? cached.rendered : renderSegment(segment, items, live);
      next.set(segment.key, { live, segment, items, rendered: result });
      return result;
    });
    previous = next;
    return rendered;
  };
}

/** Whether two versions of a segment show the same: everything but its item keys compared. */
function sameShown(a: Segment, b: Segment): boolean {
  return (
    a === b ||
    (a.turnId === b.turnId &&
      a.partial === b.partial &&
      a.plan === b.plan &&
      a.usage === b.usage &&
      a.end === b.end)
  );
}

/** The newest error card in the transcript, whose actions are the ones that still apply. */
export function lastErrorKey(model: Transcript): string | null {
  for (let index = model.segments.length - 1; index >= 0; index -= 1) {
    const keys = model.segments[index]?.itemKeys ?? [];
    for (let at = keys.length - 1; at >= 0; at -= 1) {
      const key = keys[at];
      if (key !== undefined && model.items.get(key)?.kind === "error") return key;
    }
  }
  return null;
}

/** Whether the transcript holds a turn, so a start can resume an earlier session. */
export function hasTurns(model: Transcript): boolean {
  return model.segments.some((segment) => segment.turnId !== null);
}
