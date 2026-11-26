import { AgentMark } from "@ghost/ui/graphite";
import { ChevronRight } from "lucide-react";
import { useEffect, useId, useState } from "react";
import type { AgentEntry, ChatRole, ChatRoles, ConfigChoice, RoleModel } from "../../bindings";
import { planRole, type RoleCatalog, roleLabel } from "../../lib/roles";
import { Select } from "../Select";
import {
  defaultAgentChoices,
  defaultLevelChoices,
  defaultModelChoices,
} from "../settings/GeneralPanel";

/** An agent's models and the levels of one of them, as it last listed them. */
function useChoices(catalog: RoleCatalog, agentId: string | null, model: string | null) {
  const [models, setModels] = useState<readonly ConfigChoice[]>([]);
  const [levels, setLevels] = useState<readonly ConfigChoice[]>([]);
  useEffect(() => {
    let live = true;
    setModels([]);
    if (agentId)
      catalog.models(agentId).then(
        (value) => {
          if (live) setModels(value);
        },
        () => {
          // No chat of this agent has run yet: it has not listed its models.
        },
      );
    return () => {
      live = false;
    };
  }, [catalog, agentId]);
  useEffect(() => {
    let live = true;
    setLevels([]);
    if (agentId)
      catalog.thinking(agentId, model).then(
        (value) => {
          if (live) setLevels(value);
        },
        () => {
          // Not known until the agent has run that model.
        },
      );
    return () => {
      live = false;
    };
  }, [catalog, agentId, model]);
  return { models, levels };
}

/**
 * Plan and build, the model menu's last part (owner, 19 November 2026: models by role are "a chat
 * by chat thing not a settings thing"). Plan with is a model and thinking level of the chat's own
 * agent, set when Plan is turned on; Build with is any ready agent, and its model and thinking
 * level, offered when a plan is accepted. Its line says what is chosen while it is folded.
 */
export function RolePicker({
  roles,
  agentId,
  agentName,
  agents,
  catalog,
  onSet,
}: {
  readonly roles: ChatRoles | null;
  /** The chat's agent, by id even before the agent list arrives: Plan with is one of its models. */
  readonly agentId: string | null;
  readonly agentName: string;
  readonly agents: readonly AgentEntry[] | null;
  readonly catalog: RoleCatalog;
  /** Saves a choice for this chat; it rejects with the reason it was refused. */
  readonly onSet: (role: ChatRole, choice: RoleModel | null) => Promise<void>;
}) {
  const id = useId();
  const [error, setError] = useState<string | null>(null);
  const plan = planRole(roles, agentId);
  const build = roles?.build ?? null;
  const planChoices = useChoices(catalog, agentId, plan?.model ?? null);
  const buildChoices = useChoices(catalog, build?.agent ?? null, build?.model ?? null);
  const set = (role: ChatRole, choice: RoleModel | null) => {
    setError(null);
    onSet(role, choice).catch((failure: unknown) =>
      setError(failure instanceof Error ? failure.message : String(failure)),
    );
  };
  // Plan with is always the chat's own agent, so its line names only the model and level.
  const shortPlan = plan ? roleLabel({ ...plan, agent: "" }, null, planChoices.models) : null;
  const shortBuild = build ? roleLabel(build, agents, buildChoices.models) : null;
  const summary =
    shortPlan || shortBuild
      ? [shortPlan ? `Plan: ${shortPlan}` : null, shortBuild ? `Build: ${shortBuild}` : null]
          .filter(Boolean)
          .join(", ")
      : "Same model throughout";
  return (
    <details className="chat-picker-roles" data-testid="chat-picker-roles">
      <summary aria-describedby={`${id}-summary`}>
        <ChevronRight size={14} aria-hidden="true" className="chat-picker-roles-chevron" />
        <span>Plan and build</span>
        <small id={`${id}-summary`} data-testid="chat-picker-roles-summary">
          {summary}
        </small>
      </summary>
      <div className="chat-picker-roles-body">
        <p className="chat-picker-roles-lead">
          Plan with applies when you turn Plan on. Build with is offered when you accept a plan.
        </p>
        {agentId ? (
          <RoleRow
            kind="plan"
            title="Plan with"
            choice={plan}
            fixedAgent={{ id: agentId, name: agentName }}
            agents={agents}
            models={planChoices.models}
            levels={planChoices.levels}
            onChange={(choice) => set("plan", choice)}
          />
        ) : null}
        <RoleRow
          kind="build"
          title="Build with"
          choice={build}
          fixedAgent={null}
          agents={agents}
          models={buildChoices.models}
          levels={buildChoices.levels}
          onChange={(choice) => set("build", choice)}
        />
        {error ? (
          <p className="ui-error" role="alert" data-testid="chat-picker-roles-error">
            {error}
          </p>
        ) : null}
      </div>
    </details>
  );
}

function RoleRow({
  kind,
  title,
  choice,
  fixedAgent,
  agents,
  models,
  levels,
  onChange,
}: {
  readonly kind: ChatRole;
  readonly title: string;
  readonly choice: RoleModel | null;
  /** Plan with stays on the chat's agent; Build with chooses one. */
  readonly fixedAgent: { readonly id: string; readonly name: string } | null;
  readonly agents: readonly AgentEntry[] | null;
  readonly models: readonly ConfigChoice[];
  readonly levels: readonly ConfigChoice[];
  readonly onChange: (choice: RoleModel | null) => void;
}) {
  const agentId = fixedAgent?.id ?? choice?.agent ?? null;
  const model = choice?.model ?? null;
  const agentChoices = defaultAgentChoices(agents ?? [], choice?.agent ?? null);
  const modelChoices = defaultModelChoices(models, model);
  const levelChoices = defaultLevelChoices(levels, choice?.effort ?? null);
  const agentName =
    agents?.find((entry) => entry.id === agentId)?.name ?? fixedAgent?.name ?? agentId ?? "";
  // A Plan with that names nothing more than the chat's own agent changes nothing: it is cleared.
  const next = (change: Partial<RoleModel>): RoleModel | null => {
    if (!agentId) return null;
    const value: RoleModel = {
      agent: agentId,
      model: change.model !== undefined ? change.model : model,
      effort: change.effort !== undefined ? change.effort : (choice?.effort ?? null),
    };
    return fixedAgent && value.model === null && value.effort === null ? null : value;
  };
  const showModels = agentId !== null && (fixedAgent !== null || choice !== null);
  return (
    <div className="chat-picker-role" data-testid={`chat-role-${kind}`}>
      <span className="chat-picker-role-title">{title}</span>
      <span className="chat-picker-role-controls">
        {fixedAgent ? null : (
          <span className="chat-picker-role-agent">
            {choice ? (
              <span className="chat-picker-role-mark" aria-hidden="true">
                <AgentMark agent={choice.agent} size={14} />
              </span>
            ) : null}
            <Select
              className="g-select"
              aria-label={`${title}: agent`}
              data-testid={`chat-role-${kind}-agent`}
              value={choice?.agent ?? ""}
              onChange={(event) => {
                const agent = event.currentTarget.value;
                onChange(agent ? { agent, model: null, effort: null } : null);
              }}
            >
              <option value="">Not set</option>
              {agentChoices.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
            </Select>
          </span>
        )}
        {showModels && modelChoices.length > 0 ? (
          <Select
            className="g-select"
            aria-label={`${title}: model`}
            data-testid={`chat-role-${kind}-model`}
            value={model ?? ""}
            onChange={(event) =>
              onChange(next({ model: event.currentTarget.value || null, effort: null }))
            }
          >
            <option value="">{fixedAgent ? "The chat's model" : "Its default model"}</option>
            {modelChoices.map((entry) => (
              <option key={entry.value} value={entry.value}>
                {entry.name}
              </option>
            ))}
          </Select>
        ) : null}
        {showModels && levelChoices.length > 0 && (fixedAgent === null || model !== null) ? (
          <Select
            className="g-select"
            aria-label={`${title}: thinking`}
            data-testid={`chat-role-${kind}-thinking`}
            value={choice?.effort ?? ""}
            onChange={(event) => onChange(next({ effort: event.currentTarget.value || null }))}
          >
            <option value="">Default thinking</option>
            {levelChoices.map((entry) => (
              <option key={entry.value} value={entry.value}>
                {entry.name}
              </option>
            ))}
          </Select>
        ) : null}
        {showModels && modelChoices.length === 0 ? (
          <span className="chat-picker-role-note">
            {agentName} lists its models once one of its chats has started.
          </span>
        ) : null}
      </span>
    </div>
  );
}
