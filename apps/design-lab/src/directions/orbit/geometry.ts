import type { OrbState } from "@ghost/ui";
import { type ActivitySegment, IDLE_TIMELINE, type MarkTimeline } from "@ghost/ui/orbit";
import type { TranscriptModel, Turn } from "../../kit";

export * from "@ghost/ui/orbit";

/**
 * The last turn's activity as segments: thinking before and between outputs, streaming while a
 * message arrives, tool while a call runs, waiting while a request is open, then done or error.
 */
export function markTimeline(model: TranscriptModel): MarkTimeline {
  const turn: Turn | undefined = model.turns[model.turns.length - 1];
  if (!turn) return IDLE_TIMELINE;
  const segments: ActivitySegment[] = [];
  const ripples: { at: number; tone: "ink" | "signal" }[] = [];
  let cursor = turn.startedAt;
  let streamedChars = 0;
  const push = (from: number, to: number | null, state: OrbState): void => {
    if (to !== null && to <= from) return;
    segments.push({ from, to, state });
  };
  const gap = (until: number): void => {
    if (until > cursor) push(cursor, until, "thinking");
  };
  for (const item of turn.items) {
    if (item.kind === "thought") {
      gap(item.startedAt);
      push(item.startedAt, item.endedAt, "thinking");
      cursor = Math.max(cursor, item.endedAt ?? Number.POSITIVE_INFINITY);
    } else if (item.kind === "message") {
      gap(item.startedAt);
      push(item.startedAt, item.endedAt, "streaming");
      streamedChars += item.text.length;
      cursor = Math.max(cursor, item.endedAt ?? Number.POSITIVE_INFINITY);
    } else if (item.kind === "tools") {
      gap(item.startedAt);
      for (const call of item.calls) {
        if (call.awaitingPermission) continue;
        ripples.push({ at: call.startedAt, tone: "ink" });
        if (call.endedAt !== null) ripples.push({ at: call.endedAt, tone: "ink" });
      }
      if (item.status === "waiting") {
        // Held by a permission request: the request's own item says when waiting began.
        cursor = Math.max(cursor, item.startedAt);
      } else {
        push(item.startedAt, item.endedAt, "tool");
        cursor = Math.max(cursor, item.endedAt ?? Number.POSITIVE_INFINITY);
      }
    } else {
      const permission = item.permission;
      gap(permission.requestedAt);
      if (permission.status === "pending") {
        ripples.push({ at: permission.requestedAt, tone: "signal" });
        push(permission.requestedAt, null, "waiting");
        cursor = Number.POSITIVE_INFINITY;
      } else {
        cursor = Math.max(cursor, permission.resolvedAt ?? permission.requestedAt);
      }
    }
  }
  if (turn.endedAt !== null) {
    if (Number.isFinite(cursor)) gap(turn.endedAt);
    const state: OrbState =
      turn.status === "failed" ? "error" : turn.status === "cancelled" ? "idle" : "done";
    push(turn.endedAt, null, state);
  } else if (Number.isFinite(cursor)) {
    push(cursor, null, "thinking");
  }
  segments.sort((left, right) => left.from - right.from);
  return { segments, ripples, streamedChars };
}
