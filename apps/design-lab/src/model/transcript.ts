import type { AgentBrandId, AuraIntensity, OrbState } from "@ghost/ui";
import type {
  Cost,
  LabEvent,
  LimitWindow,
  PermissionOption,
  PlanEntry,
  StopReason,
  TimedEvent,
  TokenUsage,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  UsageInfo,
  Value,
} from "../fixture/types";
import { type AnsiLine, parseAnsi } from "./ansi";
import { diffFile, type FileDiff } from "./diff";
import { shareStructure } from "./share";
import { familyOf, runSummary, type ToolFamily, toolLabel, toolTarget } from "./tools";
import type {
  FileChange,
  NoticeModel,
  PermissionModel,
  PermissionStatus,
  PlanModel,
  SearchMatch,
  SessionModel,
  ToolCallModel,
  TranscriptModel,
  Turn,
  TurnItem,
  TurnStatus,
  UserMessageModel,
} from "./types";

/**
 * The transcript reducer: a chat's timed events in, the `TranscriptModel` every direction renders
 * out. Pure: the same events always give the same model.
 *
 * Semantics follow aura-protocol (ACP vocabulary):
 * - A `user_message` opens a turn; the agent's `turn_started` names it. A `turn_started` with no
 *   user message before it opens a turn of its own, and output with no turn at all opens an
 *   implicit one, so nothing an agent sends is ever dropped.
 * - Deltas append to their message or thought by id. A message or thought also counts as finished
 *   once anything else follows it in the turn, or the turn ends: ACP never says so explicitly.
 * - Tool updates merge into their call: a status or title replaces the old one when present, and
 *   non-empty content replaces the old content. An update for a call never announced (a dropped
 *   frame) still shows up, as a generic tool.
 * - When a turn ends, its unanswered permission requests are cancelled, and calls still marked
 *   running take the turn's outcome (completed, or failed when the turn was cancelled or failed).
 *
 * Pass the previous model as `previous` to keep unchanged objects identical (see share.ts).
 */
export function reduceTranscript(
  events: readonly TimedEvent[],
  previous?: TranscriptModel | null,
): TranscriptModel {
  const draft = new Draft();
  for (const entry of events) draft.apply(entry.at, entry.event);
  const next = draft.finish(events.length);
  return previous ? shareStructure(previous, next) : next;
}

interface DraftText {
  kind: "thought" | "message";
  id: string;
  text: string;
  completed: boolean;
  startedAt: number;
  endedAt: number | null;
  reportedMs: number | null;
}

interface DraftRun {
  kind: "tools";
  family: ToolFamily;
  callIds: string[];
}

interface DraftPermissionRef {
  kind: "permission";
  requestId: string;
}

type DraftItem = DraftText | DraftRun | DraftPermissionRef;

interface DraftTurn {
  id: string;
  /** The agent's turn_started has arrived (or the turn began with it). */
  named: boolean;
  agent: AgentBrandId | null;
  user: UserMessageModel | null;
  items: DraftItem[];
  startedAt: number;
  endedAt: number | null;
  stopReason: StopReason | null;
  usage: TokenUsage | null;
  cost: Cost | null;
  contextUsed: number | null;
  contextSize: number | null;
  model: string | null;
  effort: string | null;
}

interface DraftCall {
  id: string;
  turn: DraftTurn;
  kind: ToolKind;
  title: string;
  status: ToolStatus;
  locations: readonly ToolLocation[];
  rawInput: Value | null;
  content: readonly ToolContent[];
  startedAt: number;
  endedAt: number | null;
}

interface DraftPermission {
  requestId: string;
  turn: DraftTurn | null;
  toolCallId: string | null;
  title: string;
  options: readonly PermissionOption[];
  status: PermissionStatus;
  selectedOptionId: string | null;
  requestedAt: number;
  resolvedAt: number | null;
}

const EMPTY_SESSION: SessionModel = {
  nativeSessionId: null,
  title: null,
  model: null,
  effort: null,
  mode: null,
};

function isRunningStatus(status: ToolStatus): boolean {
  return status === "pending" || status === "in_progress";
}

function turnStatusFor(stopReason: StopReason): TurnStatus {
  switch (stopReason) {
    case "cancelled":
      return "cancelled";
    case "error":
    case "refusal":
      return "failed";
    default:
      return "completed";
  }
}

class Draft {
  readonly turns: DraftTurn[] = [];
  readonly texts = new Map<string, DraftText>();
  readonly calls = new Map<string, DraftCall>();
  readonly permissions = new Map<string, DraftPermission>();
  plan: { entries: readonly PlanEntry[]; turnId: string | null; updatedAt: number } | null = null;
  usage: UsageInfo | null = null;
  limits: readonly LimitWindow[] = [];
  session: SessionModel = EMPTY_SESSION;
  notices: NoticeModel[] = [];
  lastAt: number | null = null;
  implicitTurns = 0;

  apply(rawAt: number, event: LabEvent): void {
    const at = Number.isFinite(rawAt) ? rawAt : (this.lastAt ?? 0);
    this.lastAt = at;
    switch (event.type) {
      case "user_message":
        this.turns.push({
          ...this.newTurn(`turn-for-${event.message_id}`, at, false),
          agent: event.agent,
          user: { id: event.message_id, text: event.text, at },
        });
        break;
      case "turn_started": {
        const last = this.turns[this.turns.length - 1];
        if (last && !last.named && last.endedAt === null) {
          last.id = event.turn_id;
          last.named = true;
          last.startedAt = at;
        } else {
          this.turns.push({ ...this.newTurn(event.turn_id, at, true), agent: last?.agent ?? null });
        }
        break;
      }
      case "turn_ended": {
        const turn =
          this.findTurn(event.turn_id) ?? this.openTurn() ?? this.turns[this.turns.length - 1];
        if (!turn || turn.endedAt !== null) break;
        turn.endedAt = at;
        turn.stopReason = event.stop_reason;
        turn.usage = event.usage;
        turn.model = this.session.model;
        turn.effort = this.session.effort;
        this.finishOpenText(turn, at);
        for (const permission of this.permissions.values()) {
          if (permission.turn === turn && permission.status === "pending") {
            permission.status = "cancelled";
            permission.resolvedAt = at;
          }
        }
        break;
      }
      case "message_delta":
      case "thought_delta": {
        const kind = event.type === "message_delta" ? "message" : "thought";
        const id = event.type === "message_delta" ? event.message_id : event.thought_id;
        const item = this.textItem(kind, id, at);
        item.text += event.text;
        break;
      }
      case "message_completed":
      case "thought_completed": {
        const kind = event.type === "message_completed" ? "message" : "thought";
        const id = event.type === "message_completed" ? event.message_id : event.thought_id;
        const item = this.textItem(kind, id, at);
        if (!item.completed) {
          item.completed = true;
          item.endedAt = at;
        }
        if (event.type === "thought_completed" && event.duration_ms !== null) {
          item.reportedMs = event.duration_ms;
        }
        break;
      }
      case "tool_call": {
        const existing = this.calls.get(event.tool_call_id);
        const call = existing ?? this.addCall(event.tool_call_id, event.kind, event.title, at);
        call.kind = event.kind;
        call.title = event.title;
        call.locations = event.locations;
        call.rawInput = event.raw_input;
        this.setStatus(call, event.status, at);
        break;
      }
      case "tool_update": {
        const call =
          this.calls.get(event.tool_call_id) ??
          this.addCall(event.tool_call_id, "other", event.title ?? "Tool", at);
        if (event.title !== null) call.title = event.title;
        if (event.content.length > 0) call.content = event.content;
        if (event.status !== null) this.setStatus(call, event.status, at);
        break;
      }
      case "permission_requested": {
        const existing = this.permissions.get(event.request_id);
        const turn = existing?.turn ?? this.activeTurn(at);
        if (!existing) this.append(turn, { kind: "permission", requestId: event.request_id }, at);
        this.permissions.set(event.request_id, {
          requestId: event.request_id,
          turn,
          toolCallId: event.tool_call_id,
          title: event.title,
          options: event.options,
          status: "pending",
          selectedOptionId: null,
          requestedAt: existing?.requestedAt ?? at,
          resolvedAt: null,
        });
        break;
      }
      case "permission_resolved": {
        const permission = this.permissions.get(event.request_id);
        if (permission?.status !== "pending") break;
        permission.resolvedAt = at;
        if (event.decision.type === "cancelled") {
          permission.status = "cancelled";
          break;
        }
        const optionId = event.decision.option_id;
        const option = permission.options.find((candidate) => candidate.option_id === optionId);
        permission.selectedOptionId = optionId;
        permission.status =
          option === undefined
            ? "cancelled"
            : option.kind === "allow_once" || option.kind === "allow_always"
              ? "allowed"
              : "rejected";
        break;
      }
      case "plan_updated":
        this.plan = { entries: event.entries, turnId: this.openTurn()?.id ?? null, updatedAt: at };
        break;
      case "usage_updated": {
        this.usage = {
          context_used: event.context_used,
          context_size: event.context_size,
          cost: event.cost,
        };
        const turn = this.openTurn();
        if (turn) {
          turn.cost = event.cost ?? turn.cost;
          turn.contextUsed = event.context_used ?? turn.contextUsed;
          turn.contextSize = event.context_size ?? turn.contextSize;
        }
        break;
      }
      case "limits_updated":
        this.limits = event.windows;
        break;
      case "session_updated":
        this.session = {
          nativeSessionId: event.native_session_id,
          title: event.title,
          model: event.model,
          effort: event.effort,
          mode: event.mode,
        };
        break;
      case "notice":
        this.notices.push({
          level: event.level,
          message: event.message,
          at,
          turnId: this.openTurn()?.id ?? null,
        });
        break;
      case "error":
        this.notices.push({
          level: "error",
          message: event.message,
          at,
          turnId: this.openTurn()?.id ?? null,
        });
        break;
      default:
        // `ui_request` (agent questions that are not permissions) has no lab fixture yet, and
        // events of unknown types (from untyped JSON) are ignored rather than fatal.
        break;
    }
  }

  private newTurn(id: string, at: number, named: boolean): DraftTurn {
    return {
      id,
      named,
      agent: null,
      user: null,
      items: [],
      startedAt: at,
      endedAt: null,
      stopReason: null,
      usage: null,
      cost: null,
      contextUsed: null,
      contextSize: null,
      model: null,
      effort: null,
    };
  }

  private findTurn(id: string): DraftTurn | undefined {
    for (let index = this.turns.length - 1; index >= 0; index -= 1) {
      const turn = this.turns[index];
      if (turn?.id === id) return turn;
    }
    return undefined;
  }

  /** The last turn while it is still running. */
  openTurn(): DraftTurn | undefined {
    const last = this.turns[this.turns.length - 1];
    return last && last.endedAt === null ? last : undefined;
  }

  /** The running turn, or a new implicit one for output that arrives outside any turn. */
  private activeTurn(at: number): DraftTurn {
    const open = this.openTurn();
    if (open) return open;
    this.implicitTurns += 1;
    const turn = {
      ...this.newTurn(`implicit-${this.implicitTurns}`, at, true),
      agent: this.turns[this.turns.length - 1]?.agent ?? null,
    };
    this.turns.push(turn);
    return turn;
  }

  /** Adds an item to a turn; whatever text was still open before it counts as finished. */
  private append(turn: DraftTurn, item: DraftItem, at: number): void {
    this.finishOpenText(turn, at);
    turn.items.push(item);
  }

  private finishOpenText(turn: DraftTurn, at: number): void {
    for (const item of turn.items) {
      if ((item.kind === "message" || item.kind === "thought") && !item.completed) {
        item.completed = true;
        item.endedAt = at;
      }
    }
  }

  private textItem(kind: "message" | "thought", id: string, at: number): DraftText {
    const key = `${kind}:${id}`;
    const existing = this.texts.get(key);
    if (existing) return existing;
    const item: DraftText = {
      kind,
      id,
      text: "",
      completed: false,
      startedAt: at,
      endedAt: null,
      reportedMs: null,
    };
    this.append(this.activeTurn(at), item, at);
    this.texts.set(key, item);
    return item;
  }

  private addCall(id: string, kind: ToolKind, title: string, at: number): DraftCall {
    const turn = this.activeTurn(at);
    const call: DraftCall = {
      id,
      turn,
      kind,
      title,
      status: "pending",
      locations: [],
      rawInput: null,
      content: [],
      startedAt: at,
      endedAt: null,
    };
    this.calls.set(id, call);
    const last = turn.items[turn.items.length - 1];
    const family = familyOf(kind);
    if (last?.kind === "tools" && last.family === family) last.callIds.push(id);
    else this.append(turn, { kind: "tools", family, callIds: [id] }, at);
    return call;
  }

  private setStatus(call: DraftCall, status: ToolStatus, at: number): void {
    call.status = status;
    if (isRunningStatus(status)) call.endedAt = null;
    else if (call.endedAt === null) call.endedAt = at;
  }

  finish(eventCount: number): TranscriptModel {
    const callModels = new Map<string, ToolCallModel>();
    const pendingByCall = new Set<string>();
    for (const permission of this.permissions.values()) {
      if (permission.status === "pending" && permission.toolCallId !== null) {
        pendingByCall.add(permission.toolCallId);
      }
    }
    const callModel = (id: string): ToolCallModel | null => {
      const cached = callModels.get(id);
      if (cached) return cached;
      const call = this.calls.get(id);
      if (!call) return null;
      const model = buildCall(call, pendingByCall.has(id));
      callModels.set(id, model);
      return model;
    };
    const permissionModel = (permission: DraftPermission): PermissionModel => ({
      requestId: permission.requestId,
      toolCallId: permission.toolCallId,
      title: permission.title,
      options: permission.options,
      status: permission.status,
      selectedOptionId: permission.selectedOptionId,
      requestedAt: permission.requestedAt,
      resolvedAt: permission.resolvedAt,
      tool: permission.toolCallId === null ? null : callModel(permission.toolCallId),
    });

    const turns = this.turns.map((turn) =>
      buildTurn(turn, callModel, this.permissions, permissionModel),
    );
    const pendingPermissions = [...this.permissions.values()]
      .filter((permission) => permission.status === "pending")
      .map(permissionModel);
    const activity = activityOf(turns);

    return {
      turns,
      plan: this.plan ? planModel(this.plan) : null,
      pendingPermissions,
      session: this.session,
      usage: this.usage,
      limits: this.limits,
      activity,
      aura: auraFor(activity),
      notices: this.notices,
      eventCount,
      lastEventAt: this.lastAt,
    };
  }
}

/** Diffs and parsed terminal output, cached per content object: fixture content never changes. */
const diffCache = new WeakMap<object, FileDiff>();
const ansiCache = new WeakMap<object, readonly AnsiLine[]>();

function cachedDiff(content: Extract<ToolContent, { type: "diff" }>): FileDiff {
  const cached = diffCache.get(content);
  if (cached) return cached;
  const diff = diffFile(content.path, content.old_text, content.new_text);
  diffCache.set(content, diff);
  return diff;
}

function cachedAnsi(content: Extract<ToolContent, { type: "terminal" }>): readonly AnsiLine[] {
  const cached = ansiCache.get(content);
  if (cached) return cached;
  const lines = parseAnsi(content.output);
  ansiCache.set(content, lines);
  return lines;
}

const SEARCH_LINE = /^(.+?):(\d+):(.*)$/;

function parseMatches(text: string): SearchMatch[] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "")
    .map((line) => {
      const match = SEARCH_LINE.exec(line);
      if (!match) return { path: line.trim(), line: null, text: "" };
      return { path: match[1] ?? "", line: Number(match[2]), text: match[3] ?? "" };
    });
}

function buildCall(call: DraftCall, awaitingPermission: boolean): ToolCallModel {
  const turnEnded = call.turn.endedAt !== null;
  // A call still marked running when its turn ended takes the turn's outcome.
  let status = call.status;
  if (turnEnded && isRunningStatus(status)) {
    const outcome = call.turn.stopReason ? turnStatusFor(call.turn.stopReason) : "completed";
    status = outcome === "completed" ? "completed" : "failed";
  }
  const endedAt = isRunningStatus(status) ? null : (call.endedAt ?? call.turn.endedAt);
  const target = toolTarget(call.kind, call.title, call.locations, call.rawInput);

  const diffs: FileDiff[] = [];
  const texts: string[] = [];
  let terminalContent: Extract<ToolContent, { type: "terminal" }> | null = null;
  for (const content of call.content) {
    if (content.type === "diff") diffs.push(cachedDiff(content));
    else if (content.type === "text") texts.push(content.text);
    else terminalContent = content;
  }
  const text = texts.length > 0 ? texts.join("\n\n") : null;

  return {
    id: call.id,
    kind: call.kind,
    title: call.title,
    status,
    label: isRunningStatus(status)
      ? toolLabel(
          call.kind,
          target,
          call.title,
          awaitingPermission && !turnEnded ? "waiting" : "active",
        )
      : toolLabel(call.kind, target, call.title, "past"),
    pastLabel: toolLabel(call.kind, target, call.title, "past"),
    target,
    locations: call.locations,
    rawInput: call.rawInput,
    content: call.content,
    diffs,
    terminal: terminalContent
      ? {
          command: call.kind === "execute" ? target : null,
          output: terminalContent.output,
          lines: cachedAnsi(terminalContent),
          exitCode: terminalContent.exit_code,
          truncated: terminalContent.truncated,
          running: isRunningStatus(status),
        }
      : null,
    search:
      call.kind === "search" ? { pattern: target, matches: text ? parseMatches(text) : [] } : null,
    text,
    awaitingPermission: awaitingPermission && !turnEnded,
    startedAt: call.startedAt,
    endedAt,
  };
}

function buildTurn(
  turn: DraftTurn,
  callModel: (id: string) => ToolCallModel | null,
  permissions: ReadonlyMap<string, DraftPermission>,
  permissionModel: (permission: DraftPermission) => PermissionModel,
): Turn {
  const ended = turn.endedAt !== null;
  const items: TurnItem[] = [];
  let waiting = false;
  for (const item of turn.items) {
    if (item.kind === "thought") {
      const finished = item.completed || ended;
      const endedAt = item.endedAt ?? turn.endedAt;
      items.push({
        kind: "thought",
        id: item.id,
        text: item.text,
        streaming: !finished,
        startedAt: item.startedAt,
        endedAt: finished ? endedAt : null,
        durationMs: finished
          ? (item.reportedMs ?? Math.max(0, (endedAt ?? item.startedAt) - item.startedAt))
          : null,
      });
    } else if (item.kind === "message") {
      const finished = item.completed || ended;
      items.push({
        kind: "message",
        id: item.id,
        text: item.text,
        streaming: !finished,
        startedAt: item.startedAt,
        endedAt: finished ? (item.endedAt ?? turn.endedAt) : null,
      });
    } else if (item.kind === "tools") {
      const calls = item.callIds
        .map(callModel)
        .filter((call): call is ToolCallModel => call !== null);
      const running = calls.filter((call) => isRunningStatus(call.status));
      const files: string[] = [];
      let added = 0;
      let removed = 0;
      for (const call of calls) {
        for (const diff of call.diffs) {
          if (!files.includes(diff.path)) files.push(diff.path);
          added += diff.added;
          removed += diff.removed;
        }
      }
      const endTimes = calls.map((call) => call.endedAt);
      const allEnded = calls.length > 0 && endTimes.every((time) => time !== null);
      items.push({
        kind: "tools",
        id: item.callIds[0] ?? "",
        family: item.family,
        calls,
        summary: runSummary(calls),
        label: running[running.length - 1]?.label ?? runSummary(calls),
        status:
          running.length > 0
            ? running.every((call) => call.awaitingPermission)
              ? "waiting"
              : "running"
            : calls.some((call) => call.status === "failed")
              ? "failed"
              : "completed",
        added,
        removed,
        files,
        startedAt: calls[0]?.startedAt ?? turn.startedAt,
        endedAt: allEnded ? Math.max(...endTimes.map((time) => time ?? 0)) : null,
      });
    } else if (item.kind === "permission") {
      const permission = permissions.get(item.requestId);
      if (!permission) continue;
      if (permission.status === "pending") waiting = true;
      items.push({
        kind: "permission",
        id: item.requestId,
        permission: permissionModel(permission),
      });
    }
  }

  const status: TurnStatus =
    ended && turn.stopReason ? turnStatusFor(turn.stopReason) : waiting ? "waiting" : "running";

  return {
    id: turn.id,
    agent: turn.agent,
    user: turn.user,
    items,
    status,
    startedAt: turn.startedAt,
    endedAt: turn.endedAt,
    stopReason: turn.stopReason,
    footer:
      ended && turn.stopReason && turn.endedAt !== null
        ? footerOf(turn, turn.stopReason, turn.endedAt, items)
        : null,
  };
}

function footerOf(
  turn: DraftTurn,
  stopReason: StopReason,
  endedAt: number,
  items: readonly TurnItem[],
): Turn["footer"] {
  const files = new Map<string, FileChange>();
  let added = 0;
  let removed = 0;
  for (const item of items) {
    if (item.kind !== "tools") continue;
    for (const call of item.calls) {
      for (const diff of call.diffs) {
        const previous = files.get(diff.path);
        files.set(diff.path, {
          path: diff.path,
          added: (previous?.added ?? 0) + diff.added,
          removed: (previous?.removed ?? 0) + diff.removed,
          isNew: previous?.isNew ?? diff.isNew,
        });
        added += diff.added;
        removed += diff.removed;
      }
    }
  }
  return {
    durationMs: Math.max(0, endedAt - turn.startedAt),
    stopReason,
    usage: turn.usage,
    cost: turn.cost,
    contextUsed: turn.contextUsed,
    contextSize: turn.contextSize,
    files: [...files.values()],
    added,
    removed,
    model: turn.model,
    effort: turn.effort,
  };
}

function planModel(plan: {
  entries: readonly PlanEntry[];
  turnId: string | null;
  updatedAt: number;
}): PlanModel {
  const completed = plan.entries.filter((entry) => entry.status === "completed").length;
  return {
    entries: plan.entries,
    completed,
    total: plan.entries.length,
    current:
      plan.entries.find((entry) => entry.status === "in_progress") ??
      plan.entries.find((entry) => entry.status === "pending") ??
      null,
    done: plan.entries.length > 0 && completed === plan.entries.length,
    turnId: plan.turnId,
    updatedAt: plan.updatedAt,
  };
}

/** What the agent is doing, from the last turn: the orb's state. */
export function activityOf(turns: readonly Turn[]): OrbState {
  const turn = turns[turns.length - 1];
  if (!turn) return "idle";
  switch (turn.status) {
    case "completed":
      return "done";
    case "failed":
      return "error";
    case "cancelled":
      return "idle";
    case "waiting":
      return "waiting";
    default:
      break;
  }
  const last = turn.items[turn.items.length - 1];
  if (!last) return "thinking";
  // Between steps (a finished thought, message or tool run) the agent is working out what next.
  if (last.kind === "message") return last.streaming ? "streaming" : "thinking";
  if (last.kind === "tools") return last.status === "running" ? "tool" : "thinking";
  return "thinking";
}

/** The aura field follows the orb: working glows, waiting turns amber, everything else is calm. */
export function auraFor(activity: OrbState): AuraIntensity {
  switch (activity) {
    case "thinking":
    case "streaming":
    case "tool":
      return "active";
    case "waiting":
      return "alert";
    default:
      return "calm";
  }
}

/**
 * How many events of a time-ordered timeline have arrived by `time` (ms). Binary search. Works for
 * any timeline of `{ at }` entries: a chat recording, or a discovery scan.
 */
export function eventsAt(events: readonly { readonly at: number }[], time: number): number {
  let low = 0;
  let high = events.length;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if ((events[mid]?.at ?? Number.POSITIVE_INFINITY) <= time) low = mid + 1;
    else high = mid;
  }
  return low;
}

/** The empty transcript: a new chat. Last in the module, since it runs the reducer on load. */
export const EMPTY_TRANSCRIPT: TranscriptModel = reduceTranscript([]);
