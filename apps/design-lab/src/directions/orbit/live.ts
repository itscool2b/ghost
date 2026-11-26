import type { AgentBrandId } from "@ghost/ui";
import type { PermissionModel, TranscriptModel } from "../../kit";
import type { NowRow } from "./state";

/*
 * What the live end of a chat says, from the model alone: pure, so it is tested in Node.
 */

/** The command a permission request is about, for the event line. */
export function requestTarget(permission: PermissionModel): string {
  return permission.tool?.target ?? permission.title.replace(/^Run\s+/i, "").replace(/\?$/, "");
}

/** About how many tokens a handoff carries: the chat's words, at four characters a token. */
export function handoffTokens(model: TranscriptModel): number {
  let chars = 0;
  for (const turn of model.turns) {
    chars += turn.user?.text.length ?? 0;
    for (const item of turn.items) if (item.kind === "message") chars += item.text.length;
  }
  return Math.max(10, Math.round(chars / 4 / 10) * 10);
}

/**
 * The live end of the last turn (Grok Bot's pending row), from the model. It shows only when
 * nothing on screen is live already, so the agent's state is said once: a streaming thought or
 * message and a running tool are their own live lines. Between them the agent is thinking, or it
 * waits on you.
 */
/**
 * How long a gap between items lasts before the now-row shows in it, ms. The golden turn's gaps
 * run 0.75 to 2.7 s; at 0.7 s the row flashed for 50 to 100 ms before the next item replaced it,
 * and for 0.4 s between the last answer and the receipt. At 1.2 s it shows only in the gaps where
 * the agent is really thinking, and where it does show it has the geometry of the line that
 * replaces it, so the hand-over reads as one status line updating in place.
 */
export const NOW_ROW_DELAY_MS = 1200;

export function modelNowRow(model: TranscriptModel, agent: AgentBrandId): NowRow | null {
  const turn = model.turns[model.turns.length - 1];
  if (!turn || (turn.status !== "running" && turn.status !== "waiting")) return null;
  const pending = model.pendingPermissions[0];
  if (pending) {
    return {
      agent,
      state: "waiting",
      verb: "Waiting on you",
      object: null,
      since: pending.requestedAt,
      delayMs: 0,
    };
  }
  const last = turn.items[turn.items.length - 1];
  if (!last) {
    // The start of a turn: the row shows at once, where the first item will appear.
    return {
      agent,
      state: "thinking",
      verb: "Thinking",
      object: null,
      since: turn.startedAt,
      delayMs: 0,
    };
  }
  if ((last.kind === "thought" || last.kind === "message") && last.streaming) return null;
  if (last.kind === "tools" && last.status === "running") return null;
  const since =
    last.kind === "permission"
      ? (last.permission.resolvedAt ?? last.permission.requestedAt)
      : (last.endedAt ?? turn.startedAt);
  return {
    agent,
    state: "thinking",
    verb: "Thinking",
    object: null,
    since,
    delayMs: NOW_ROW_DELAY_MS,
  };
}
