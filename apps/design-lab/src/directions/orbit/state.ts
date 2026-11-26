import type { AgentBrandId } from "@ghost/ui";
import type { Autonomy } from "../../kit";

/*
 * What the Orbit window keeps for itself: the composer's choices, messages typed while they
 * cannot go yet, and the lines the window adds to the chat (answers, autonomy changes, steers,
 * notices). The recordings end where they end; what the user does after them lives here, and the
 * agent's side of it runs through local.ts.
 */

/** What the composer sends with, and what is typed. */
export interface ComposerState {
  readonly agent: AgentBrandId;
  readonly model: string;
  readonly effort: string | null;
  readonly autonomy: Autonomy;
  readonly draft: string;
  readonly pickerOpen: boolean;
  /** Aside's Final confirm: always ask before rm, force pushes and writes outside the project. */
  readonly finalConfirm: boolean;
}

/** A message typed while the agent works: it goes out when the turn ends. */
export interface QueuedMessage {
  readonly id: number;
  readonly text: string;
}

export type AnswerKind = "allow_once" | "allow_chat" | "allow_always" | "deny";

/** An answer to a permission request, given on the card or with its keys. */
export interface PermissionAnswer {
  readonly kind: AnswerKind;
  /** The window's clock when it was given, ms. */
  readonly at: number;
}

/**
 * Where a line the window adds sits in the transcript: inside a turn after its first `after`
 * items (so whatever the turn adds later comes below it), after a turn's receipt (`"end"`), or
 * before the first turn (`turnId` null).
 */
export interface Anchor {
  readonly turnId: string | null;
  readonly after: number | "end";
}

/** A line the window adds to the chat. */
export type Extra =
  | {
      /**
       * A centered system event (Grok Bot): "Switched to [Codex] · handoff 540 tokens",
       * "Allowed once · cargo test -p aura-drivers · 0:09".
       */
      readonly kind: "event";
      readonly id: string;
      readonly anchor: Anchor;
      readonly icon: "check" | "x" | "stop" | "level" | "info" | null;
      /** The words, in text-3. */
      readonly text: string;
      /** An agent as an entity chip after the words: its 12px mark and its name in ink. */
      readonly agent?: AgentBrandId;
      readonly agentName?: string;
      /** A literal (a command, a path) in mono after the words. */
      readonly literal?: string;
      /** The rest, after a dot: a size, a clock. */
      readonly detail?: string;
    }
  | {
      /** A message sent into the running turn (Steer): shown at once, where it was sent. */
      readonly kind: "steer";
      readonly id: string;
      readonly anchor: Anchor;
      readonly text: string;
      /** How far into the turn it was sent, as a clock ("0:21"). */
      readonly clock: string;
    }
  | {
      /** The dashed warning notice (section 4.8): left-aligned, an orange diamond, one action. */
      readonly kind: "notice";
      readonly id: string;
      readonly anchor: Anchor;
      readonly text: string;
      readonly action: string | null;
    };

/** An extra before it has an id. */
export type NewExtra = Extra extends infer E
  ? E extends { readonly id: string }
    ? Omit<E, "id">
    : never
  : never;

/** The live end of a running turn (Grok Bot's pending row). */
export interface NowRow {
  readonly agent: AgentBrandId;
  readonly state: "thinking" | "tool" | "waiting" | "stopping";
  /** The verb ("Running", "Thinking"), the object as a literal, and when it began. */
  readonly verb: string;
  readonly object: string | null;
  readonly since: number;
  /**
   * How long a gap between two items lasts before the row shows, ms. Gaps between tool calls are
   * often a few hundred ms, and a row that flashes for 0.15 s reads as a glitch; the start of a
   * turn, a wait and a stop show at once.
   */
  readonly delayMs: number;
}

/** What a handoff carries to the next agent, for the handoff segment's Preview. */
export interface HandoffSummary {
  readonly title: string;
  readonly turns: number;
  readonly files: number;
  readonly added: number;
  readonly removed: number;
  /** The first line of the last answer, or null when the chat has none yet. */
  readonly lastAnswer: string | null;
  readonly tokens: number;
}
