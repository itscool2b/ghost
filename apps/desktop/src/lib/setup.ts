import { setMotionPreference } from "@ghost/ui";
import {
  type AgentEntry,
  type ConfigChoice,
  commands,
  type DetectedEditors,
  type GeneralPreferences,
  type SetupAction,
} from "../bindings";
import { applySetupAppearance } from "./appearance";
import { applyStepsPreference } from "./chat/stepsPreference";
import { agentsList, settle } from "./ipc";

async function withAppearance(result: ReturnType<typeof commands.setupSnapshot>) {
  const snapshot = await settle(result);
  applySetupAppearance(snapshot.preferences);
  applyStepsPreference(snapshot.preferences);
  return snapshot;
}

export const setupApi = {
  snapshot: () => withAppearance(commands.setupSnapshot()),
  apply: (revision: number, action: SetupAction) =>
    withAppearance(commands.setupApply(revision, action)),
};
export type SetupApi = typeof setupApi;
export { setMotionPreference };

/**
 * What Settings' General page lists: the detected agents, the models one has reported, and the
 * thinking levels a model offered (`null`: the model the agent starts on).
 */
export const generalApi = {
  agents: (): Promise<AgentEntry[]> => agentsList(),
  models: (agentId: string): Promise<ConfigChoice[]> => settle(commands.agentModels(agentId)),
  thinking: (agentId: string, model: string | null): Promise<ConfigChoice[]> =>
    settle(commands.agentThinkingLevels(agentId, model)),
  /** The installed editors, for Settings > Chats > Open files in. */
  editors: (): Promise<DetectedEditors> => settle(commands.editorsDetected()),
};
export type GeneralApi = typeof generalApi;

/**
 * Which agent a fresh New chat selects from Settings' default: `"wait"` while detection has not
 * said whether that agent can run, the agent once it can, and `null` when there is no default,
 * the person already chose, or the agent cannot run here (New chat then waits for a choice).
 */
export function preselectAgent(
  defaultAgent: string | null,
  agents: readonly AgentEntry[] | null,
  chosen: string | null,
): string | "wait" | null {
  if (defaultAgent === null || chosen !== null) return null;
  const agent = agents?.find((entry) => entry.id === defaultAgent);
  if (agents === null || agent?.availability.state === "checking") return "wait";
  return agent?.availability.state === "ready" ? defaultAgent : null;
}

/**
 * Rust's defaults: full auto, background services on, automatic checkpoints off, Git-ignored
 * instruction files copied into new worktree chats, agents editing project memory directly, and the
 * Browser plugin on with saved sign-ins, and the Mobile plugin on.
 */
export const DEFAULT_GENERAL: GeneralPreferences = {
  default_agent: null,
  default_model: null,
  default_effort: null,
  default_autonomy: "full_auto",
  automatic_checkpoints: false,
  notifications: true,
  sounds: false,
  keep_awake: true,
  close_to_tray: true,
  copy_instructions_to_worktrees: true,
  agent_memory_writes: true,
  keep_steps_open: false,
  editor: null,
  browser: true,
  browser_passwords: true,
  mobile: true,
  game: true,
  automatic_updates: true,
  coordinated_projects: false,
  speech_voice: null,
};
