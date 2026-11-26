import type { AgentEntry, ChatSummary, SessionStatus } from "../bindings";

/**
 * Agent updates as the window's notices show them. Detection compares each installed agent with
 * its latest release (at startup, then every 15 minutes); an agent with a newer release gets a
 * notice that stays until its X. Dismissing lasts for this run of Ghost only, so the notice
 * returns at the next start until the agent is updated, and a newer release than the one
 * dismissed shows again at once.
 */
export interface AgentUpdate {
  readonly agentId: string;
  readonly name: string;
  /** The installed version. */
  readonly version: string;
  readonly latest: string;
  /** Where the installation detection found runs from, when it knows. */
  readonly path: string | null;
  /** Dismissals are per release: `claude@2.1.400`. */
  readonly key: string;
}

export function availableUpdates(agents: readonly AgentEntry[] | null): AgentUpdate[] {
  const updates: AgentUpdate[] = [];
  for (const agent of agents ?? []) {
    const harness = agent.harness;
    if (harness?.state !== "update_available") continue;
    updates.push({
      agentId: agent.id,
      name: agent.name,
      version: harness.version,
      latest: harness.latest,
      path: agent.installation?.launch_path ?? null,
      key: `${agent.id}@${harness.latest}`,
    });
  }
  return updates;
}

const WORKING: ReadonlySet<SessionStatus> = new Set([
  "running",
  "starting",
  "cancelling",
  "stopping",
]);

/** How many chats have a turn under way, on any agent: what an app restart would stop. */
export function chatsWorking(chats: readonly ChatSummary[] | null): number {
  return (chats ?? []).filter((chat) => WORKING.has(chat.status)).length;
}

/**
 * How many chats on this agent have a turn under way. Replacing an agent's files under a running
 * turn can fail (Windows holds a running program's files) or break the turn, so an update waits.
 */
export function workingChats(chats: readonly ChatSummary[] | null, agentId: string): number {
  return (chats ?? []).filter((chat) => chat.agent_id === agentId && WORKING.has(chat.status))
    .length;
}

/** The releases dismissed in this run of Ghost, as an external store for React. */
export class UpdateNotices {
  #dismissed: ReadonlySet<string> = new Set();
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly getSnapshot = (): ReadonlySet<string> => this.#dismissed;

  dismiss(key: string): void {
    if (this.#dismissed.has(key)) return;
    this.#dismissed = new Set([...this.#dismissed, key]);
    for (const listener of [...this.#listeners]) listener();
  }
}

/** One per window: it lives as long as Ghost runs, so a restart shows every notice again. */
export const updateNotices = new UpdateNotices();
