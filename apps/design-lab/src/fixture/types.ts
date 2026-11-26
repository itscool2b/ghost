import type { AgentBrandId } from "@ghost/ui";
import type { AuraEvent } from "./m0-protocol";

/*
 * The event model is aura-protocol's AuraEvent as of M0, the vocabulary the fixtures were recorded
 * in (./m0-protocol.ts, a snapshot of the tauri-specta types). The lab stays on it on purpose: M2a
 * ports the chosen direction into the app against the live protocol, so the prototype does not
 * chase every protocol change.
 */
export type {
  AuraEvent,
  Cost,
  LimitWindow,
  NoticeLevel,
  PermissionDecision,
  PermissionOption,
  PermissionOptionKind,
  PermissionRequest,
  PlanEntry,
  PlanPriority,
  PlanStatus,
  SessionInfo,
  StopReason,
  TokenUsage,
  ToolCall,
  ToolContent,
  ToolKind,
  ToolLocation,
  ToolStatus,
  ToolUpdate,
  UsageInfo,
  Value,
} from "./m0-protocol";

/**
 * What the user sent. Not an AuraEvent: the UI already knows what it sent, and the core stores it
 * with the turn. The lab puts it in the timeline so one list describes the whole chat.
 */
export interface UserMessageEvent {
  readonly type: "user_message";
  readonly message_id: string;
  readonly text: string;
  /** The agent the message went to. A chat can switch agents between turns. */
  readonly agent: AgentBrandId;
}

/** Anything a recorded chat timeline holds. */
export type LabEvent = AuraEvent | UserMessageEvent;

/** One event and when it arrived, in ms from the start of the recording. */
export interface TimedEvent {
  readonly at: number;
  readonly event: LabEvent;
}

/** A recorded chat: its events in arrival order. */
export interface ChatRecording {
  readonly chatId: string;
  readonly events: readonly TimedEvent[];
  /** When the last event arrives, in ms. */
  readonly duration: number;
}

/** Autonomy levels, mirroring aura-protocol's `Autonomy` (not exported to TypeScript yet). */
export type Autonomy = "full_auto" | "auto_edit" | "ask";

/**
 * The live mini-orb on a sidebar row: a turn running, waiting for approval, finished while the chat
 * was not open (done, unseen), failed, or nothing going on.
 */
export type ChatStatus = "running" | "needs_approval" | "done_unseen" | "error" | "idle";

export interface ChatSummary {
  readonly id: string;
  readonly projectId: string;
  readonly title: string;
  /** The agent that drives the chat now. */
  readonly agent: AgentBrandId;
  readonly status: ChatStatus;
  /** Relative time for the row ("now", "4m", "Tue"). Fixed text, so screenshots never drift. */
  readonly updated: string;
  /**
   * Lines the chat's last turn changed, when it changed any: for ledgers and notifications
   * ("+248 −31"). Optional; directions that do not show outcomes ignore it.
   */
  readonly changes?: { readonly added: number; readonly removed: number };
}

export interface Project {
  readonly id: string;
  readonly name: string;
  /** The project's colour, `#rrggbb`. */
  readonly color: string;
  /** Whether the sidebar shows the project's chats. */
  readonly expanded: boolean;
  readonly chats: readonly ChatSummary[];
}

export interface AgentEffort {
  /** The value the agent takes ("high", "xhigh"). */
  readonly id: string;
  /** What the picker shows ("High", "Extra high"). */
  readonly label: string;
}

export interface AgentModel {
  /** The id the agent reports in `session_updated` ("claude-opus-5-5"). */
  readonly id: string;
  /** What the picker and the composer pill show ("Opus 5.5"). */
  readonly label: string;
  /** One line under the label in the picker. */
  readonly description: string;
  /** Effort levels, lowest first. Empty when the model has no effort setting. */
  readonly efforts: readonly AgentEffort[];
  readonly defaultEffort: string | null;
}

export interface AgentInfo {
  readonly id: AgentBrandId;
  readonly name: string;
  readonly installed: boolean;
  /** Installed version, or null when the agent is not installed. */
  readonly version: string | null;
  /** A newer version the vendor offers, or null when the install is current. */
  readonly update: string | null;
  /** Whether the agent is signed in, or null when it is not installed. */
  readonly signedIn: boolean | null;
  /** Models in picker order. Empty for agents that are not installed. */
  readonly models: readonly AgentModel[];
  readonly defaultModel: string | null;
  /** The autonomy levels the agent supports. Pi never asks, so it only has Full auto. */
  readonly autonomy: readonly Autonomy[];
}

/** The composer under the transcript: who the next message goes to, and how. */
export interface Composer {
  readonly agent: AgentBrandId;
  /** An `AgentModel.id` of that agent. */
  readonly model: string;
  /** An `AgentEffort.id` of that model, or null when the model has no effort setting. */
  readonly effort: string | null;
  readonly autonomy: Autonomy;
  /** Text already typed. */
  readonly draft: string;
  /** Whether the agent > model > effort picker is open (the `picker` state). */
  readonly pickerOpen: boolean;
}

/** The tabs of the right panel, the second pane beside the transcript. */
export type RightPanelTab = "changes" | "files" | "plan" | "session";

/** Which edits the Changes tab shows: this turn's, the whole chat's, or the branch's since main. */
export type ChangesScope = "turn" | "chat" | "branch";

/** The right panel, when it is open (the `changes` state opens it on the turn's diffs). */
export interface RightPanel {
  readonly tab: RightPanelTab;
  readonly scope: ChangesScope;
  /** The file whose diff is open, or null for none. */
  readonly expandedFile: string | null;
}

/**
 * First-run agent discovery (the `onboarding` state): the app looks for each agent it knows on
 * this machine and reports what it finds. Not an AuraEvent: detection runs in the core before any
 * chat exists. Timed like a recording, so the player drives it.
 */
export type DiscoveryEvent =
  | {
      readonly type: "scan_started";
      /** Where the scan looks, in order ("PATH", "~/.local/bin"). */
      readonly locations: readonly string[];
    }
  | {
      readonly type: "agent_found";
      readonly agent: AgentBrandId;
      /** The binary that answered. */
      readonly path: string;
      readonly version: string;
      readonly signedIn: boolean;
    }
  | {
      readonly type: "agent_missing";
      readonly agent: AgentBrandId;
      /** A command that installs it, shown so it can be copied. */
      readonly installCommand: string;
    }
  | { readonly type: "scan_finished" };

/** One discovery event and when it arrived, in ms from the start of the scan. */
export interface TimedDiscoveryEvent {
  readonly at: number;
  readonly event: DiscoveryEvent;
}

/** A recorded scan: its events in arrival order. */
export interface DiscoveryTimeline {
  /** The agents looked for, in display order. */
  readonly candidates: readonly AgentBrandId[];
  readonly events: readonly TimedDiscoveryEvent[];
  /** How long the scan plays, in ms: at least until its last event arrives. */
  readonly duration: number;
}

/** What the scan knows about one agent so far. */
export interface DiscoveredAgent {
  readonly id: AgentBrandId;
  readonly name: string;
  /** Still being looked for, found on this machine, or reported missing. */
  readonly status: "looking" | "found" | "missing";
  readonly path: string | null;
  readonly version: string | null;
  /** Whether it is signed in, or null until found. */
  readonly signedIn: boolean | null;
  readonly installCommand: string | null;
  /** When it was found or reported missing, ms on the scan's timeline; null while looking. */
  readonly at: number | null;
}

/** The scan at the playhead, reduced from its events (see src/model/discovery.ts). */
export interface DiscoveryModel {
  readonly status: "idle" | "scanning" | "done";
  readonly locations: readonly string[];
  /** Every candidate in display order, with what is known about it so far. */
  readonly agents: readonly DiscoveredAgent[];
  readonly startedAt: number | null;
  readonly finishedAt: number | null;
}

/** Everything around the transcript: sidebar, agent dock, composer. */
export interface Workspace {
  readonly projects: readonly Project[];
  /** The agent dock, in display order. */
  readonly agents: readonly AgentInfo[];
  /** The chat in the main pane, or null for a new chat with no messages yet. */
  readonly activeChatId: string | null;
  /** The project the active chat (or the new chat) belongs to. */
  readonly activeProjectId: string;
  readonly composer: Composer;
  /** Signed out: the base app needs no account. */
  readonly account: { readonly kind: "local"; readonly label: string };
  /** The right panel when it is open (the `changes` state), else null. */
  readonly rightPanel: RightPanel | null;
  /** First-run agent discovery at the playhead (the `onboarding` state), else null. */
  readonly discovery: DiscoveryModel | null;
}
