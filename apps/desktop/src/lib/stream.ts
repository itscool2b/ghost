import type {
  EventBatch,
  GhostEvent,
  LimitWindow,
  PermissionRequest,
  PlanEntry,
  SessionInfo,
  StopReason,
  TokenUsage,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  UsageInfo,
} from "../bindings";

/**
 * The transcript of one session, rebuilt from its frames by `applyBatch`.
 *
 * Frames carry session-wide sequence numbers (`seq_start` plus the event's index), so the reducer
 * can drop frames it has already applied (a duplicate or an overlapping replay) and count the
 * events it never saw (a gap). Deltas append to their message or thought by id; everything else
 * replaces or records state. Pure and immutable: every call returns a new state and leaves the old
 * one untouched, which is what React's external-store subscription needs.
 */

/** Order of blocks in the transcript, by first appearance. */
export interface ItemRef {
  readonly kind: "thought" | "message" | "tool";
  readonly id: string;
}

export interface MessageBlock {
  readonly id: string;
  readonly text: string;
  readonly completed: boolean;
}

export interface ThoughtBlock {
  readonly id: string;
  readonly text: string;
  readonly completed: boolean;
  readonly durationMs: number | null;
}

export interface ToolBlock {
  readonly id: string;
  readonly kind: ToolKind;
  readonly title: string;
  readonly status: ToolStatus;
  readonly locations: readonly ToolLocation[];
  readonly content: readonly ToolContent[];
}

export interface TurnInfo {
  readonly id: string;
  readonly ended: boolean;
  readonly stopReason: StopReason | null;
  readonly usage: TokenUsage | null;
}

export interface StreamNotice {
  readonly level: "info" | "warning" | "error";
  readonly message: string;
}

export interface StreamState {
  /** Adopted from the first frame; frames for other sessions are counted in `foreign` and ignored. */
  readonly sessionId: string | null;
  /** The sequence number the next frame should start at. */
  readonly nextSeq: number;
  /** Frames that contributed at least one new event. */
  readonly frames: number;
  /** Events applied. */
  readonly events: number;
  /** Events dropped because an earlier frame already applied them. */
  readonly duplicates: number;
  /** Frames that started past `nextSeq`: each is one gap. */
  readonly gaps: number;
  /** Events skipped over by gaps. */
  readonly missed: number;
  /** Frames dropped because they belong to another session. */
  readonly foreign: number;
  readonly items: readonly ItemRef[];
  readonly messages: ReadonlyMap<string, MessageBlock>;
  readonly thoughts: ReadonlyMap<string, ThoughtBlock>;
  readonly tools: ReadonlyMap<string, ToolBlock>;
  readonly plan: readonly PlanEntry[];
  readonly usage: UsageInfo | null;
  readonly limits: readonly LimitWindow[];
  readonly session: SessionInfo | null;
  /** Permission requests still waiting for a decision. */
  readonly permissions: readonly PermissionRequest[];
  /** Agent notices and errors, newest last, capped at `MAX_NOTICES`. */
  readonly notices: readonly StreamNotice[];
  readonly turn: TurnInfo | null;
}

/** Notices kept per session; older ones are dropped. */
export const MAX_NOTICES = 50;

const U32 = 2 ** 32;

export function initialStreamState(): StreamState {
  return {
    sessionId: null,
    nextSeq: 0,
    frames: 0,
    events: 0,
    duplicates: 0,
    gaps: 0,
    missed: 0,
    foreign: 0,
    items: [],
    messages: new Map(),
    thoughts: new Map(),
    tools: new Map(),
    plan: [],
    usage: null,
    limits: [],
    session: null,
    permissions: [],
    notices: [],
    turn: null,
  };
}

/**
 * `a - b` in sequence-number space: u32 arithmetic that wraps (the batcher wraps at u32::MAX
 * instead of overflowing), read as a signed 32-bit distance (RFC 1982 serial numbers).
 */
export function seqDistance(a: number, b: number): number {
  return (a - b) | 0;
}

/** Applies one frame. Returns `state` itself when the frame changes nothing visible. */
export function applyBatch(state: StreamState, batch: EventBatch): StreamState {
  if (state.sessionId !== null && batch.chat_id !== state.sessionId) {
    return { ...state, foreign: state.foreign + 1 };
  }
  const count = batch.events.length;
  if (count === 0) return state;

  // How far this frame starts past the next expected event: > 0 is a gap, < 0 an overlap.
  const ahead = seqDistance(batch.seq_start, state.nextSeq);
  const skip = ahead < 0 ? Math.min(-ahead, count) : 0;
  if (skip === count) {
    return { ...state, sessionId: batch.chat_id, duplicates: state.duplicates + count };
  }

  const draft = new Draft(state);
  for (let index = skip; index < count; index += 1) {
    const event = batch.events[index];
    if (event) draft.apply(event);
  }
  return draft.finish({
    sessionId: batch.chat_id,
    nextSeq: (batch.seq_start + count) % U32,
    frames: state.frames + 1,
    events: state.events + (count - skip),
    duplicates: state.duplicates + skip,
    gaps: state.gaps + (ahead > 0 ? 1 : 0),
    missed: state.missed + Math.max(ahead, 0),
  });
}

/** Applies a sequence of frames, in order. */
export function applyBatches(state: StreamState, batches: readonly EventBatch[]): StreamState {
  return batches.reduce(applyBatch, state);
}

type Counters = Pick<
  StreamState,
  "sessionId" | "nextSeq" | "frames" | "events" | "duplicates" | "gaps" | "missed"
>;

/** Copy-on-write working state for one frame: each map is copied at most once per frame. */
class Draft {
  readonly #base: StreamState;
  #items: ItemRef[] | null = null;
  #messages: Map<string, MessageBlock> | null = null;
  #thoughts: Map<string, ThoughtBlock> | null = null;
  #tools: Map<string, ToolBlock> | null = null;
  #plan: readonly PlanEntry[];
  #usage: UsageInfo | null;
  #limits: readonly LimitWindow[];
  #session: SessionInfo | null;
  #permissions: readonly PermissionRequest[];
  #notices: readonly StreamNotice[];
  #turn: TurnInfo | null;

  constructor(base: StreamState) {
    this.#base = base;
    this.#plan = base.plan;
    this.#usage = base.usage;
    this.#limits = base.limits;
    this.#session = base.session;
    this.#permissions = base.permissions;
    this.#notices = base.notices;
    this.#turn = base.turn;
  }

  apply(event: GhostEvent): void {
    switch (event.type) {
      case "turn_started":
        this.#turn = { id: event.turn_id, ended: false, stopReason: null, usage: null };
        break;
      case "turn_ended":
        this.#turn = {
          id: event.turn_id,
          ended: true,
          stopReason: event.stop_reason,
          usage: event.usage,
        };
        break;
      case "message_delta": {
        const messages = this.messages();
        const previous = messages.get(event.message_id);
        if (!previous) this.addItem("message", event.message_id);
        messages.set(event.message_id, {
          id: event.message_id,
          text: (previous?.text ?? "") + event.text,
          completed: previous?.completed ?? false,
        });
        break;
      }
      case "message_completed": {
        const messages = this.messages();
        const previous = messages.get(event.message_id);
        if (!previous) this.addItem("message", event.message_id);
        messages.set(event.message_id, {
          id: event.message_id,
          text: previous?.text ?? "",
          completed: true,
        });
        break;
      }
      case "thought_delta": {
        const thoughts = this.thoughts();
        const previous = thoughts.get(event.thought_id);
        if (!previous) this.addItem("thought", event.thought_id);
        thoughts.set(event.thought_id, {
          id: event.thought_id,
          text: (previous?.text ?? "") + event.text,
          completed: previous?.completed ?? false,
          durationMs: previous?.durationMs ?? null,
        });
        break;
      }
      case "thought_completed": {
        const thoughts = this.thoughts();
        const previous = thoughts.get(event.thought_id);
        if (!previous) this.addItem("thought", event.thought_id);
        thoughts.set(event.thought_id, {
          id: event.thought_id,
          text: previous?.text ?? "",
          completed: true,
          durationMs: event.duration_ms ?? previous?.durationMs ?? null,
        });
        break;
      }
      case "tool_call": {
        const tools = this.tools();
        const previous = tools.get(event.tool_call_id);
        if (!previous) this.addItem("tool", event.tool_call_id);
        tools.set(event.tool_call_id, {
          id: event.tool_call_id,
          kind: event.kind,
          title: event.title,
          status: event.status,
          locations: event.locations,
          content: event.content,
        });
        break;
      }
      case "tool_update": {
        const tools = this.tools();
        const previous = tools.get(event.tool_call_id);
        // An update for a call never announced (a dropped frame) still shows up, as a generic tool.
        if (!previous) this.addItem("tool", event.tool_call_id);
        tools.set(event.tool_call_id, {
          id: event.tool_call_id,
          kind: event.kind ?? previous?.kind ?? "other",
          title: event.title ?? previous?.title ?? "Tool",
          status: event.status ?? previous?.status ?? "in_progress",
          locations: event.locations ?? previous?.locations ?? [],
          // ACP semantics: `null` keeps the previous content, a list replaces it (`[]` clears it).
          content: event.content ?? previous?.content ?? [],
        });
        break;
      }
      case "permission_requested": {
        const { type: _type, ...request } = event;
        this.#permissions = [
          ...this.#permissions.filter((p) => p.request_id !== request.request_id),
          request,
        ];
        break;
      }
      case "permission_resolved":
        this.#permissions = this.#permissions.filter((p) => p.request_id !== event.request_id);
        break;
      case "ui_request":
      case "ui_resolved":
        // The demo stream has no UI for agent questions; the demo agent never asks.
        break;
      case "user_message_delta":
      case "steering_status":
      case "prompt_lifecycle":
        // The demo stream plays agent turns only: no user messages, no prompt queue.
        break;
      case "task_updated":
      case "task_step":
        // The demo agent starts no background tasks.
        break;
      case "plan_updated":
        this.#plan = event.entries;
        break;
      case "usage_updated": {
        const { type: _type, ...usage } = event;
        this.#usage = usage;
        break;
      }
      case "limits_updated":
        this.#limits = event.windows;
        break;
      case "session_updated": {
        const { type: _type, ...session } = event;
        this.#session = session;
        break;
      }
      case "notice":
        this.addNotice({ level: event.level, message: event.message });
        break;
      case "error":
        this.addNotice({ level: "error", message: event.message });
        break;
      default: {
        // Exhaustive: a new event type fails to compile here until it is handled.
        const unreachable: never = event;
        throw new Error(`unhandled event: ${JSON.stringify(unreachable)}`);
      }
    }
  }

  finish(counters: Counters): StreamState {
    const base = this.#base;
    return {
      ...base,
      ...counters,
      items: this.#items ?? base.items,
      messages: this.#messages ?? base.messages,
      thoughts: this.#thoughts ?? base.thoughts,
      tools: this.#tools ?? base.tools,
      plan: this.#plan,
      usage: this.#usage,
      limits: this.#limits,
      session: this.#session,
      permissions: this.#permissions,
      notices: this.#notices,
      turn: this.#turn,
    };
  }

  private addItem(kind: ItemRef["kind"], id: string): void {
    this.#items ??= [...this.#base.items];
    this.#items.push({ kind, id });
  }

  private addNotice(notice: StreamNotice): void {
    this.#notices = [...this.#notices, notice].slice(-MAX_NOTICES);
  }

  private messages(): Map<string, MessageBlock> {
    this.#messages ??= new Map(this.#base.messages);
    return this.#messages;
  }

  private thoughts(): Map<string, ThoughtBlock> {
    this.#thoughts ??= new Map(this.#base.thoughts);
    return this.#thoughts;
  }

  private tools(): Map<string, ToolBlock> {
    this.#tools ??= new Map(this.#base.tools);
    return this.#tools;
  }
}

/** The transcript's main answer: the last message that has started. */
export function lastMessage(state: StreamState): MessageBlock | null {
  for (let index = state.items.length - 1; index >= 0; index -= 1) {
    const item = state.items[index];
    if (item?.kind === "message") return state.messages.get(item.id) ?? null;
  }
  return null;
}

/** What the agent is doing right now, for the orb and the aura. */
export type Activity = "idle" | "thinking" | "streaming" | "tool" | "done" | "error";

export function activityOf(state: StreamState): Activity {
  const turn = state.turn;
  if (!turn) return "idle";
  if (turn.ended) return turn.stopReason === "error" ? "error" : "done";
  const last = state.items[state.items.length - 1];
  if (!last) return "thinking";
  switch (last.kind) {
    case "thought":
      return "thinking";
    case "tool": {
      // Between a finished tool and the next block the agent is deciding: thinking.
      const status = state.tools.get(last.id)?.status;
      return status === "completed" || status === "failed" ? "thinking" : "tool";
    }
    case "message":
      return "streaming";
  }
}
