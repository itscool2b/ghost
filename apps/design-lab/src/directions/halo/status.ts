import type { OrbState } from "@ghost/ui";
import {
  basename,
  type PermissionModel,
  type PlanModel,
  type ToolCallModel,
  type ToolKind,
  type Turn,
} from "../../kit";

/*
 * The status line under the transcript: what the agent is doing right now, as a verb and what it
 * acts on ("Running pnpm test", "Reading pacing.ts"). It is read at a glance, so it must not
 * flicker: a short thought between two tool calls keeps the tool's verb, and every verb stays up
 * for at least a second before the next one replaces it.
 */

export interface LiveStatus {
  /** What the hold keys on: "Reading", "Running", "Thinking". */
  readonly verb: string;
  /** What the verb acts on, or null. */
  readonly target: string | null;
  /** The target is a command or a search pattern, set in mono. */
  readonly mono: boolean;
  /** "wait" while the agent waits on you. */
  readonly tone: "work" | "wait";
}

/** A thought shorter than this between tool calls keeps the last tool's verb. */
export const TOOL_GAP_MS = 1500;

/** Each verb stays on the status line at least this long. */
export const VERB_HOLD_MS = 1000;

const VERBS: Readonly<Record<ToolKind, string>> = {
  read: "Reading",
  edit: "Editing",
  delete: "Deleting",
  move: "Moving",
  search: "Searching",
  execute: "Running",
  fetch: "Fetching",
  think: "Thinking",
  switch_mode: "Switching mode",
  other: "Working",
};

const FILE_KINDS: ReadonlySet<ToolKind> = new Set(["read", "edit", "delete", "move"]);

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

const status = (
  verb: string,
  target: string | null = null,
  mono = false,
  tone: LiveStatus["tone"] = "work",
): LiveStatus => ({ verb, target, mono, tone });

/** The command a call runs, or what it reads, searches or fetches. */
export function callStatus(call: ToolCallModel): LiveStatus {
  const verb = VERBS[call.kind] ?? VERBS.other;
  const target = call.target;
  switch (call.kind) {
    case "execute":
      return status(verb, call.terminal?.command ?? target, true);
    case "search":
      return status(verb, call.search?.pattern ?? target, true);
    case "fetch":
      return status(verb, target === null ? null : hostOf(target));
    default:
      return status(verb, target !== null && FILE_KINDS.has(call.kind) ? basename(target) : null);
  }
}

/** The pending permission in a turn, newest first. */
export function pendingPermission(turn: Turn): PermissionModel | null {
  for (let index = turn.items.length - 1; index >= 0; index -= 1) {
    const item = turn.items[index];
    if (item?.kind === "permission" && item.permission.status === "pending") return item.permission;
  }
  return null;
}

/** The command a permission asks to run, or its question. */
export function permissionTarget(permission: PermissionModel): {
  readonly text: string;
  readonly mono: boolean;
} {
  const tool = permission.tool;
  const command = tool?.terminal?.command ?? (tool?.kind === "execute" ? tool.target : null);
  return command ? { text: command, mono: true } : { text: permission.title, mono: false };
}

/** What the running turn is doing at `now` (timeline ms), before the hold is applied. */
export function liveStatus(
  turn: Turn,
  activity: OrbState,
  plan: PlanModel | null,
  now: number,
): LiveStatus {
  if (turn.status === "waiting" || activity === "waiting") {
    const permission = pendingPermission(turn);
    const target = permission ? permissionTarget(permission) : null;
    return status("Needs your approval", target?.text ?? null, target?.mono ?? false, "wait");
  }
  const last = turn.items[turn.items.length - 1];
  if (last?.kind === "tools") {
    const recent =
      last.status === "running" || (last.endedAt !== null && now - last.endedAt < TOOL_GAP_MS);
    if (activity === "tool" || (activity === "thinking" && recent)) {
      const call =
        [...last.calls]
          .reverse()
          .find(
            (candidate) => candidate.status === "in_progress" || candidate.status === "pending",
          ) ?? last.calls[last.calls.length - 1];
      if (call) return callStatus(call);
    }
  }
  if (activity === "streaming") return status("Writing");
  if (plan !== null && plan.turnId === turn.id && now - plan.updatedAt < TOOL_GAP_MS) {
    return status("Planning");
  }
  if (activity === "thinking" || activity === "tool") return status("Thinking");
  return status("Working");
}

/** A status on the line, and since when (timeline ms) its verb has been showing. */
export interface HeldStatus {
  readonly status: LiveStatus;
  readonly since: number;
}

function same(a: LiveStatus, b: LiveStatus): boolean {
  return a.verb === b.verb && a.target === b.target && a.mono === b.mono && a.tone === b.tone;
}

/**
 * The status to show at `now`, given what showed before: a new verb waits until the current one has
 * been up for `holdMs` (waiting on you never waits); a new target under the same verb shows at
 * once. A playhead that went backwards (a replay) starts over. With the playhead still (a paused
 * or static scene, where the clock never moves on), pass a hold of 0: changes then come from you,
 * and show at once.
 */
export function holdStatus(
  previous: HeldStatus | null,
  next: LiveStatus,
  now: number,
  holdMs: number = VERB_HOLD_MS,
): HeldStatus {
  if (previous === null || now < previous.since) return { status: next, since: now };
  if (previous.status.verb === next.verb) {
    return same(previous.status, next) ? previous : { status: next, since: previous.since };
  }
  if (next.tone === "wait" || now - previous.since >= holdMs) return { status: next, since: now };
  return previous;
}
