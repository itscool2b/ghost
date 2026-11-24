import type { OrbState } from "@o7/ui";
import type { ToolKind } from "../../bindings";
import type { ChatState } from "./chatStore";
import { hasProcess, type RenderedSegment, toolDisplayStatus } from "./transcript";

/** Visible protocol activity, with requests taking precedence over a concurrent tool or stream. */
export function chatActivity(state: ChatState): OrbState {
  const status = state.session?.status ?? state.chat?.status ?? "stopped";
  if (status === "failed") return "error";
  if (status === "auth_required") return "waiting";
  if (
    !hasProcess(status) ||
    status === "ready" ||
    status === "stopping" ||
    status === "cancelling"
  ) {
    return "idle";
  }
  for (const item of state.transcript.items.values()) {
    if (
      (item.kind === "permission" ||
        (item.kind === "question" && item.request.prompt.type !== "notify")) &&
      item.resolution === null
    ) {
      const request = state.requests.get(item.request.request_id);
      if (!request?.gone && !request?.busy) return "waiting";
    }
  }
  const segment = state.transcript.segments.at(-1);
  if (status !== "running") return "idle";
  if (!segment) return "thinking";
  for (let index = segment.itemKeys.length - 1; index >= 0; index--) {
    const item = state.transcript.items.get(segment.itemKeys[index] ?? "");
    if (segment.end === null && item?.kind === "message" && !item.completed) return "streaming";
    if (segment.end === null && item?.kind === "thought" && !item.completed) return "thinking";
    if (item?.kind === "tool") {
      const display = toolDisplayStatus(item, segment.end, true);
      if (display === "pending" || display === "in_progress") return "tool";
    }
  }
  return "thinking";
}

/**
 * Whether a running turn's newest shown output is its answer still streaming in. That is then the
 * live part of the turn, so the working line steps aside until it ends. Reasoning does not: it
 * shows on the working line itself (`liveReasoning`).
 */
export function streamingText(segment: RenderedSegment | undefined): boolean {
  if (segment === undefined || segment.end !== null) return false;
  const newest = segment.items.at(-1);
  return newest?.kind === "message" && !newest.completed;
}

/** Whether a running turn's newest shown output is reasoning the agent is thinking now. */
export function thinkingNow(segment: RenderedSegment | undefined): boolean {
  if (segment === undefined || segment.end !== null) return false;
  const newest = segment.items.at(-1);
  return newest?.kind === "thought" && newest.thinking;
}

/** The longest stretch of the reasoning's newest words the working line shows. */
const REASONING_SHOWN = 160;

/**
 * The newest words of the reasoning a running turn is thinking now, as the working line shows
 * them under its verb: the last line with words, its end kept when it is long. Null while the
 * agent is not thinking, or thinks without words.
 */
export function liveReasoning(segment: RenderedSegment | undefined): string | null {
  if (segment === undefined || segment.end !== null) return null;
  const newest = segment.items.at(-1);
  if (newest?.kind !== "thought" || !newest.thinking) return null;
  const line = newest.text
    .split("\n")
    .map((part) => part.replace(/[*_`#>]+/g, "").trim())
    .filter((part) => part !== "")
    .at(-1);
  if (line === undefined) return null;
  return line.length > REASONING_SHOWN ? `…${line.slice(-REASONING_SHOWN).trimStart()}` : line;
}

/** The kind of a turn's latest call, which the working line's words lean towards. */
export function lastToolKind(segment: RenderedSegment | undefined): ToolKind | null {
  const items = segment?.items ?? [];
  for (let index = items.length - 1; index >= 0; index--) {
    const item = items[index];
    if (item?.kind === "tool") return item.toolKind;
  }
  return null;
}
