import type { AgentBrandId, AuraIntensity, OrbState } from "@ghost/ui";
import type {
  Cost,
  LimitWindow,
  NoticeLevel,
  PermissionOption,
  PlanEntry,
  StopReason,
  TokenUsage,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  UsageInfo,
  Value,
} from "../fixture/types";
import type { AnsiLine } from "./ansi";
import type { FileDiff } from "./diff";
import type { ToolFamily } from "./tools";

/**
 * The transcript every direction renders, built from a chat's events by `reduceTranscript`. It is
 * plain immutable data. Objects that did not change between two reductions keep their identity, so
 * `React.memo` on a block skips finished blocks while the last one streams.
 *
 * Times (`at`, `startedAt`, `endedAt`) are ms on the recording's timeline, the same clock as the
 * player (`usePlayhead`), so a live counter is `playhead - startedAt`.
 */
export interface TranscriptModel {
  readonly turns: readonly Turn[];
  /** The agent's latest plan (each update replaces the whole plan), or null if it never sent one. */
  readonly plan: PlanModel | null;
  /** Permission requests still waiting for the user, oldest first. */
  readonly pendingPermissions: readonly PermissionModel[];
  /** What the agent last reported about the session. */
  readonly session: SessionModel;
  /** Context window and spend, latest report wins. */
  readonly usage: UsageInfo | null;
  /** Account rate-limit windows, latest report wins. */
  readonly limits: readonly LimitWindow[];
  /** What the agent is doing now: drive the orb (`<Orb state={model.activity}>`) and the sidebar dot. */
  readonly activity: OrbState;
  /** The aura field's intensity for that activity (`<AuraField intensity={model.aura}>`). */
  readonly aura: AuraIntensity;
  /** Agent notices and errors, oldest first. */
  readonly notices: readonly NoticeModel[];
  /** Events applied so far. */
  readonly eventCount: number;
  /** When the last applied event arrived, or null before the first. */
  readonly lastEventAt: number | null;
}

export interface SessionModel {
  readonly nativeSessionId: string | null;
  readonly title: string | null;
  /** The model id ("claude-opus-5-5"); look up its label in the workspace's agent models. */
  readonly model: string | null;
  /** The effort id ("high"). */
  readonly effort: string | null;
  /** The agent's own permission mode name ("bypassPermissions", "on-request"). */
  readonly mode: string | null;
}

/**
 * `running` while the agent works, `waiting` while it waits for a permission answer, then how it
 * ended: `completed` (including the token and request limits, see `stopReason`), `cancelled` by the
 * user, or `failed` (an error or a refusal).
 */
export type TurnStatus = "running" | "waiting" | "completed" | "cancelled" | "failed";

export interface Turn {
  readonly id: string;
  /** The agent that ran the turn (from the user's message), or null when unknown. */
  readonly agent: AgentBrandId | null;
  /** What the user sent, or null for a turn the agent started on its own. */
  readonly user: UserMessageModel | null;
  /** The agent's output in order: thoughts, messages, tool runs, permission cards. */
  readonly items: readonly TurnItem[];
  readonly status: TurnStatus;
  /** When the turn started: the agent's `turn_started`, else the user's message. */
  readonly startedAt: number;
  readonly endedAt: number | null;
  readonly stopReason: StopReason | null;
  /** Duration, tokens, cost, files changed; null until the turn ends. */
  readonly footer: TurnFooter | null;
}

export interface UserMessageModel {
  readonly id: string;
  readonly text: string;
  readonly at: number;
}

export type TurnItem = ThoughtItem | MessageItem | ToolRunItem | PermissionItem;

export interface ThoughtItem {
  readonly kind: "thought";
  readonly id: string;
  readonly text: string;
  /** True while thought text is still arriving. */
  readonly streaming: boolean;
  readonly startedAt: number;
  readonly endedAt: number | null;
  /**
   * How long the agent thought ("Thought for 6s"): the duration it reported, else measured on the
   * timeline. Null while it is still thinking: show `playhead - startedAt` live instead.
   */
  readonly durationMs: number | null;
}

export interface MessageItem {
  readonly kind: "message";
  readonly id: string;
  /** Markdown; `parseMarkdown(text, streaming)` turns it into blocks. */
  readonly text: string;
  /** True while message text is still arriving. */
  readonly streaming: boolean;
  readonly startedAt: number;
  readonly endedAt: number | null;
}

/** Consecutive tool calls of one family (see tools.ts), shown as one group. */
export interface ToolRunItem {
  readonly kind: "tools";
  /** The first call's id. */
  readonly id: string;
  readonly family: ToolFamily;
  readonly calls: readonly ToolCallModel[];
  /**
   * The run's line right now: what runs ("Reading pacing.test.ts", or "Waiting to run cargo test
   * -p aura-drivers" while a permission request holds it), else `summary`.
   */
  readonly label: string;
  /** Past-tense summary of every call: "Read 2 files, searched 1 pattern". */
  readonly summary: string;
  /**
   * `running` while a call runs; `waiting` when every unfinished call waits for a permission
   * answer; `failed` when a call failed; otherwise `completed`.
   */
  readonly status: "running" | "waiting" | "completed" | "failed";
  /** Lines added and removed by the run's edits, and the files they touched (in order). */
  readonly added: number;
  readonly removed: number;
  readonly files: readonly string[];
  readonly startedAt: number;
  readonly endedAt: number | null;
}

export interface ToolCallModel {
  readonly id: string;
  readonly kind: ToolKind;
  /** The agent's own title for the call. */
  readonly title: string;
  readonly status: ToolStatus;
  /**
   * The call's line right now: "Reading pacing.ts" while it runs, "Waiting to run cargo test" while
   * a permission request holds it, "Read pacing.ts" once it is done.
   */
  readonly label: string;
  /** The call as a finished line, whatever its status: "Read pacing.ts", "Ran pnpm test". */
  readonly pastLabel: string;
  /** What the call acts on: a path, a pattern, a command, or a URL. */
  readonly target: string | null;
  readonly locations: readonly ToolLocation[];
  readonly rawInput: Value | null;
  /** The latest content as the agent sent it (each update replaces the previous content). */
  readonly content: readonly ToolContent[];
  /** Diffs in the content, with counts and hunks. */
  readonly diffs: readonly FileDiff[];
  /** The latest terminal output, or null. */
  readonly terminal: TerminalModel | null;
  /** Search results parsed from a search call's text, or null for other kinds. */
  readonly search: SearchModel | null;
  /** Any other text content, joined; null when there is none. */
  readonly text: string | null;
  /** True while a permission request for this call waits for the user. */
  readonly awaitingPermission: boolean;
  readonly startedAt: number;
  readonly endedAt: number | null;
}

export interface TerminalModel {
  /** The command that ran, when the agent said. */
  readonly command: string | null;
  /** Raw output, ANSI escapes included. */
  readonly output: string;
  /** The output as styled lines (see ansi.ts). */
  readonly lines: readonly AnsiLine[];
  readonly exitCode: number | null;
  /** True when the output was cut for the UI; the full output lives on disk. */
  readonly truncated: boolean;
  /** True while the command still runs. */
  readonly running: boolean;
}

export interface SearchModel {
  readonly pattern: string | null;
  /** `path:line:text` results; lines that do not parse are kept with a null line. */
  readonly matches: readonly SearchMatch[];
}

export interface SearchMatch {
  readonly path: string;
  readonly line: number | null;
  readonly text: string;
}

export type PermissionStatus = "pending" | "allowed" | "rejected" | "cancelled";

export interface PermissionModel {
  readonly requestId: string;
  readonly toolCallId: string | null;
  /** The question, as the agent asks it ("Run cargo test -p aura-drivers?"). */
  readonly title: string;
  /** Answers in the agent's order: allow once, allow always, reject, ... */
  readonly options: readonly PermissionOption[];
  readonly status: PermissionStatus;
  readonly selectedOptionId: string | null;
  readonly requestedAt: number;
  readonly resolvedAt: number | null;
  /** The call the request is about, when the agent named one. */
  readonly tool: ToolCallModel | null;
}

/** An inline permission card, placed where the agent asked. */
export interface PermissionItem {
  readonly kind: "permission";
  readonly id: string;
  readonly permission: PermissionModel;
}

export interface PlanModel {
  readonly entries: readonly PlanEntry[];
  readonly completed: number;
  readonly total: number;
  /** The entry in progress, else the first pending one; null when everything is done. */
  readonly current: PlanEntry | null;
  /** True when every entry is completed. */
  readonly done: boolean;
  /** The turn that sent the latest update. */
  readonly turnId: string | null;
  readonly updatedAt: number;
}

export interface FileChange {
  readonly path: string;
  readonly added: number;
  readonly removed: number;
  /** True when the turn created the file. */
  readonly isNew: boolean;
}

export interface TurnFooter {
  /** From the turn's start to its end. */
  readonly durationMs: number;
  readonly stopReason: StopReason;
  /** Input, output, cached and reasoning tokens, when the agent reports them. */
  readonly usage: TokenUsage | null;
  /** The spend the agent last reported during the turn. */
  readonly cost: Cost | null;
  readonly contextUsed: number | null;
  readonly contextSize: number | null;
  /** Files the turn's edits changed, in the order they were first edited (the files chip). */
  readonly files: readonly FileChange[];
  readonly added: number;
  readonly removed: number;
  /** The model and effort in effect when the turn ended. */
  readonly model: string | null;
  readonly effort: string | null;
}

export interface NoticeModel {
  readonly level: NoticeLevel | "error";
  readonly message: string;
  readonly at: number;
  /** The turn it arrived in, or null outside any turn. */
  readonly turnId: string | null;
}
