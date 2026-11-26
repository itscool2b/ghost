import { getBrand } from "@ghost/ui";
import type {
  AgentInfo,
  DiscoveredAgent,
  DiscoveryModel,
  DiscoveryTimeline,
  TimedDiscoveryEvent,
} from "../fixture/types";

/**
 * The discovery reducer: a scan's timed events in, the `DiscoveryModel` a direction renders out.
 * Pure, like the transcript reducer. Every candidate is listed from the start, in display order, as
 * `looking` until an event settles it; an event for an agent outside the candidates is added at the
 * end, so nothing the scan reports is dropped. A finished scan leaves no agent `looking`: whatever
 * it never reported counts as missing.
 */
export function reduceDiscovery(
  timeline: DiscoveryTimeline,
  count: number = timeline.events.length,
  agents: readonly AgentInfo[] = [],
): DiscoveryModel {
  const nameOf = (id: string): string =>
    agents.find((agent) => agent.id === id)?.name ?? getBrand(id).name;
  const byId = new Map<string, DiscoveredAgent>();
  const order: string[] = [];
  const ensure = (id: DiscoveredAgent["id"]): DiscoveredAgent => {
    const existing = byId.get(id);
    if (existing) return existing;
    const fresh: DiscoveredAgent = {
      id,
      name: nameOf(id),
      status: "looking",
      path: null,
      version: null,
      signedIn: null,
      installCommand: null,
      at: null,
    };
    byId.set(id, fresh);
    order.push(id);
    return fresh;
  };
  for (const id of timeline.candidates) ensure(id);

  let status: DiscoveryModel["status"] = "idle";
  let locations: readonly string[] = [];
  let startedAt: number | null = null;
  let finishedAt: number | null = null;
  const events: readonly TimedDiscoveryEvent[] = timeline.events.slice(0, Math.max(0, count));
  for (const { at, event } of events) {
    switch (event.type) {
      case "scan_started":
        status = "scanning";
        locations = event.locations;
        startedAt = at;
        finishedAt = null;
        break;
      case "agent_found":
        byId.set(event.agent, {
          ...ensure(event.agent),
          status: "found",
          path: event.path,
          version: event.version,
          signedIn: event.signedIn,
          installCommand: null,
          at,
        });
        break;
      case "agent_missing":
        byId.set(event.agent, {
          ...ensure(event.agent),
          status: "missing",
          path: null,
          version: null,
          signedIn: null,
          installCommand: event.installCommand,
          at,
        });
        break;
      case "scan_finished":
        status = "done";
        finishedAt = at;
        break;
      default:
        break;
    }
  }

  const listed = order.map((id) => byId.get(id)).filter((agent) => agent !== undefined);
  return {
    status,
    locations,
    agents:
      status === "done"
        ? listed.map(
            (agent): DiscoveredAgent =>
              agent.status === "looking" ? { ...agent, status: "missing", at: finishedAt } : agent,
          )
        : listed,
    startedAt,
    finishedAt,
  };
}

/** Agents found so far, missing so far, and still being looked for. */
export function discoveryCounts(model: DiscoveryModel): {
  readonly found: number;
  readonly missing: number;
  readonly looking: number;
} {
  let found = 0;
  let missing = 0;
  let looking = 0;
  for (const agent of model.agents) {
    if (agent.status === "found") found += 1;
    else if (agent.status === "missing") missing += 1;
    else looking += 1;
  }
  return { found, missing, looking };
}
