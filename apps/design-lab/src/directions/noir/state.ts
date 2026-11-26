import type { AgentBrandId } from "@ghost/ui";
import type { Autonomy } from "../../kit";

/** A message typed while it cannot go yet: during a turn, or here in the lab, where nothing runs. */
export interface QueuedMessage {
  readonly text: string;
  /** When the message switches agents, the agent whose work it hands over (by name). */
  readonly handoffFrom: string | null;
}

/** What the composer will send with, and what the user has typed: local to the window. */
export interface ComposerState {
  readonly agent: AgentBrandId;
  readonly model: string;
  readonly effort: string | null;
  readonly autonomy: Autonomy;
  readonly draft: string;
  readonly pickerOpen: boolean;
  /** Messages typed during a turn, sent when it ends. */
  readonly queue: readonly QueuedMessage[];
}

export type ComposerUpdate = (change: Partial<ComposerState>) => void;
