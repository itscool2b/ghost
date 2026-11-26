import { ArrowLeft, ChevronDown, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { AgentEntry } from "../../bindings";
import { workingChats } from "../../lib/agentUpdates";
import { realAgents } from "../../lib/chat/agentLineup";
import type { AppStore } from "../../lib/chat/appStore";
import type { Transport } from "../../lib/chat/transport";
import { AgentConnect, type ConnectAgent, connectState } from "./AgentConnect";
import { type AgentPhase, AgentRow } from "./AgentRow";
import "./agents.css";
import "./setup.css";

/**
 * What each agent's row shows, kept by the page for the picture of agents joined to Ghost. A
 * row that has not reported yet is drawn from detection alone.
 */
function useConnectAgents(agents: readonly AgentEntry[] | null) {
  const [phases, setPhases] = useState<ReadonlyMap<string, { phase: AgentPhase; busy: boolean }>>(
    new Map(),
  );
  const onPhase = useCallback((agentId: string, phase: AgentPhase, busy: boolean) => {
    setPhases((current) => {
      const before = current.get(agentId);
      if (before?.phase === phase && before.busy === busy) return current;
      return new Map(current).set(agentId, { phase, busy });
    });
  }, []);
  const connect: ConnectAgent[] = [];
  for (const agent of realAgents(agents ?? [])) {
    const reported = phases.get(agent.id);
    const detected: AgentPhase =
      agent.availability.state === "ready"
        ? "ready"
        : agent.availability.state === "checking"
          ? "checking"
          : agent.availability.state === "broken"
            ? "broken"
            : "missing";
    const state = connectState(reported?.phase ?? detected, reported?.busy ?? false);
    if (state !== null) connect.push({ id: agent.id, name: agent.name, state });
  }
  return { connect, onPhase };
}

/**
 * Setup's roster: what this computer has first; the agents it lacks fold into one row until
 * asked. Rows resolve in place as detection finishes.
 */
export function AgentRoster({
  agents,
  transport,
  onRefresh,
}: {
  readonly agents: readonly AgentEntry[];
  readonly transport: Transport;
  readonly onRefresh: () => void | Promise<void>;
}) {
  const [everyAgent, setEveryAgent] = useState(false);
  const absent = (agent: AgentEntry) =>
    agent.availability.state === "missing" || agent.availability.state === "broken";
  const folded = agents.some((agent) => agent.availability.state === "ready")
    ? agents.filter(absent)
    : [];
  // Unfolded, the absent agents follow the ones this computer has, so a found agent never moves
  // down among them.
  const present = agents.filter((agent) => !folded.includes(agent));
  const shown = everyAgent ? [...present, ...folded] : present;
  const { connect, onPhase } = useConnectAgents(agents);
  return (
    <>
      {connect.length > 0 ? <AgentConnect agents={connect} /> : null}
      <ul className="agent-list" data-compact="">
        {shown.map((agent, index) => (
          <AgentRow
            key={agent.id}
            agent={agent}
            transport={transport}
            onRefresh={onRefresh}
            compact
            index={index}
            onPhase={onPhase}
          />
        ))}
        {folded.length > 0 ? (
          <li className="agent-list-more">
            <button
              type="button"
              aria-expanded={everyAgent}
              onClick={() => setEveryAgent(!everyAgent)}
            >
              {everyAgent ? "Hide the agents not installed" : `Show ${folded.length} not installed`}
              <ChevronDown size={14} aria-hidden="true" />
            </button>
          </li>
        ) : null}
      </ul>
    </>
  );
}

/** Agents (Vercel's project list): one row per harness, one action each, a menu for the rest. */
export function AgentsScreen({
  app,
  transport,
  onBack,
  onSetup,
  embedded = false,
}: {
  readonly app: AppStore;
  readonly transport: Transport;
  readonly onBack: () => void;
  readonly onSetup: () => void;
  /**
   * Settings > Agents: no bar or title of its own, and the store keeps running for the chats
   * mounted behind Settings, so this page neither starts nor stops it.
   */
  readonly embedded?: boolean;
}) {
  const state = useSyncExternalStore(app.subscribe, app.getSnapshot, app.getSnapshot);
  useEffect(() => {
    if (embedded) return;
    app.start();
    return () => app.stop();
  }, [app, embedded]);
  const agents = state.agents;
  const { connect, onPhase } = useConnectAgents(agents);
  // The counts are about this computer, as setup's are: the built-in demo is not installed here.
  const counted = agents === null ? null : realAgents(agents);
  const installed = counted?.filter((agent) => agent.availability.state === "ready").length ?? 0;
  const looking = counted?.filter((agent) => agent.availability.state === "checking").length ?? 0;
  const refresh = () => app.refreshAgents();
  const summary = (
    <p aria-live="polite">
      {agents === null
        ? "Looking for the agents on this computer."
        : looking > 0
          ? `${installed} found so far. Still looking for ${looking}.`
          : `${installed} of ${counted?.length ?? 0} installed on this computer.`}
    </p>
  );
  const refreshButton = (
    <button
      type="button"
      className="g-button g-button-outline"
      disabled={state.agentsRefreshing}
      onClick={() => void refresh()}
    >
      {state.agentsRefreshing ? (
        <span className="agents-refreshing" aria-hidden="true">
          <span className="ui-pulse" />
        </span>
      ) : (
        <RefreshCw size={14} aria-hidden="true" />
      )}
      {state.agentsRefreshing ? "Refreshing…" : "Refresh"}
    </button>
  );
  const list =
    agents === null ? (
      <div className="agent-list-empty" role="status">
        <span className="ui-pulse" aria-hidden="true" />
        <p>Looking for agents…</p>
      </div>
    ) : (
      <ul className="agent-list">
        {agents.map((agent, index) => (
          <AgentRow
            key={agent.id}
            agent={agent}
            transport={transport}
            working={workingChats(state.chats, agent.id)}
            onRefresh={refresh}
            onConnection={(agentId, connection) => app.setAgentConnection(agentId, connection)}
            index={index}
            onPhase={onPhase}
          />
        ))}
      </ul>
    );
  if (embedded)
    return (
      <div className="agents-page" data-embedded="" data-setting="Agents">
        {connect.length > 0 ? <AgentConnect agents={connect} /> : null}
        <div className="agents-head">
          {summary}
          {refreshButton}
        </div>
        {list}
      </div>
    );
  return (
    <main className="setup-shell g-root agents-screen">
      <div className="setup-drag" data-tauri-drag-region="" />
      <div className="agents-page">
        <header className="agents-bar">
          <button type="button" className="g-button g-button-ghost g-button-28" onClick={onBack}>
            <ArrowLeft size={14} aria-hidden="true" /> Back to chats
          </button>
          <button type="button" className="g-button g-button-ghost g-button-28" onClick={onSetup}>
            Run setup again
          </button>
        </header>
        <div className="agents-head">
          <div>
            <h1>Agents</h1>
            {summary}
          </div>
          {refreshButton}
        </div>
        {connect.length > 0 ? <AgentConnect agents={connect} /> : null}
        {list}
      </div>
    </main>
  );
}
