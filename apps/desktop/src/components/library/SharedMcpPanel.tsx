import { AgentMark } from "@ghost/ui/graphite";
import { useEffect, useId, useRef, useState } from "react";
import type { McpActionResult, McpDefinition, McpSessionAction, McpSnapshot } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { type SharedMcpApi, sharedMcpApi } from "../../lib/sharedMcp";
import { Modal } from "../chat/Modal";
import { Switch } from "../settings/SettingsRow";
import { RowMenu } from "./RowMenu";
import "./shared-mcp.css";
import { Select } from "../Select";

export interface McpProject {
  readonly id: string;
  readonly name: string;
}

/** An agent a server can be limited to: its catalog ID and name, and whether it is installed. */
export interface McpAgent {
  readonly id: string;
  readonly name: string;
  readonly installed: boolean;
}

/**
 * The agents whose launches take shared servers (Rust's `session::shared_mcp::prepare_launch`).
 * The others start without them and say so in their chat.
 */
export const SHARED_MCP_AGENTS: ReadonlySet<string> = new Set([
  "claude",
  "codex",
  "pi",
  "opencode",
  "cursor",
  "grok",
  "hermes",
]);

/** "Every agent", or the chosen agents by name ("Claude Code, Codex"). */
export function agentsSummary(
  agents: readonly string[] | null | undefined,
  known: readonly McpAgent[],
): string {
  if (!agents) return "Every agent";
  return agents.map((id) => known.find((agent) => agent.id === id)?.name ?? id).join(", ");
}
export function SharedMcpPanel({
  projects = [],
  agents = [],
  initialProjectId = null,
  api = sharedMcpApi,
  activeChatId,
  addFor = null,
}: {
  readonly projects?: readonly McpProject[];
  readonly agents?: readonly McpAgent[];
  readonly initialProjectId?: string | null;
  readonly api?: SharedMcpApi;
  readonly activeChatId?: string | null;
  /** Asked from an agent's own list: opens the editor for a server only that agent loads. */
  readonly addFor?: { readonly id: number; readonly agent: string } | null;
}) {
  const [snapshot, setSnapshot] = useState<McpSnapshot | null>(null);
  const [explicitScope, setExplicitScope] = useState<string | null>(null);
  const scope = explicitScope ?? initialProjectId ?? projects[0]?.id ?? "";
  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [sessionResult, setSessionResult] = useState<McpActionResult | null>(null);
  const [nativeImport, setNativeImport] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<McpDefinition | null>(null);
  const [credentials, setCredentials] = useState<McpDefinition | null>(null);
  const [document, setDocument] = useState<{ mode: "import" | "export"; text: string } | null>(
    null,
  );
  const alive = useRef(true);
  function openEditor(definition: McpDefinition) {
    setExplicitScope(scope);
    setEditing(definition);
  }
  const blank = (only: string | null): McpDefinition => ({
    id: crypto.randomUUID(),
    name: "",
    project_id: scope || null,
    enabled: true,
    transport: { type: "stdio", command: "", args: [] },
    env_keys: [],
    header_keys: [],
    agents: only ? [only] : null,
  });
  // biome-ignore lint/correctness/useExhaustiveDependencies: One request opens one editor.
  useEffect(() => {
    if (addFor) openEditor(blank(addFor.agent));
  }, [addFor?.id]);
  useEffect(() => {
    alive.current = true;
    void api
      .snapshot()
      .then((v) => {
        if (alive.current) setSnapshot(v);
      })
      .catch((e) => {
        if (alive.current) setError(errorMessage(e));
      });
    return () => {
      alive.current = false;
    };
  }, [api]);
  async function run(work: () => Promise<McpSnapshot | undefined>) {
    setBusy(true);
    setError("");
    try {
      const value = await work();
      if (alive.current && value) setSnapshot(value);
    } catch (e) {
      if (alive.current) setError(errorMessage(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  }
  const visible =
    snapshot?.servers.filter((s) => s.definition.project_id === (scope || null)) ?? [];
  const sessionAction = (definition: McpDefinition, action: McpSessionAction) =>
    void run(async () => {
      if (!activeChatId) return;
      const result = await api.sessionAction(activeChatId, definition.name, action);
      if (alive.current) setSessionResult(result);
    });
  return (
    <div className="plugins-agent shared-mcp-panel">
      <header className="plugins-agent-head">
        <div className="plugins-agent-title">
          <h2>MCP servers</h2>
          <p>For every agent, or only the ones you choose</p>
        </div>
        <button
          className="g-button g-button-primary g-button-28"
          type="button"
          disabled={busy || !snapshot}
          onClick={() => openEditor(blank(null))}
        >
          Add server
        </button>
        <RowMenu
          label="MCP servers actions"
          items={[
            {
              label: "Import",
              onSelect: () => {
                setExplicitScope(scope);
                setDocument({ mode: "import", text: "" });
              },
            },
            {
              label: "Export",
              onSelect: () =>
                void run(async () => {
                  const text = await api.export();
                  if (alive.current) setDocument({ mode: "export", text });
                }),
            },
            { label: "Refresh", onSelect: () => void run(() => api.snapshot()) },
          ]}
        />
      </header>
      <p className="plugins-summary">
        Add a server once and every agent can use it, or only the agents you choose. Signing in to a
        server stays on this device. An agent's own servers are listed on its page.
      </p>
      <div className="shared-mcp-toolbar">
        <label>
          Scope{" "}
          <Select
            className="g-input"
            value={scope}
            aria-label="MCP scope"
            // All projects shows by default until projects are found; picking it pins it.
            repick
            onChange={(e) => setExplicitScope(e.currentTarget.value)}
          >
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            <option value="">All projects</option>
          </Select>
        </label>
      </div>
      {error ? (
        <p className="library-feedback" role="alert">
          {error}
        </p>
      ) : null}
      {status ? (
        <p className="library-feedback" role="status">
          {status}
        </p>
      ) : null}
      {sessionResult ? (
        <div className="library-feedback" role="status">
          <p>{sessionResult.message}</p>
          {sessionResult.servers.map((server) => (
            <p key={server.name}>
              {server.name}: {server.state}, {server.authentication}, {server.tool_count} tools
            </p>
          ))}
          {sessionResult.authorization_url ? (
            <button
              type="button"
              className="g-button g-button-outline"
              onClick={() =>
                void navigator.clipboard
                  .writeText(sessionResult.authorization_url ?? "")
                  .catch((e) => setError(errorMessage(e)))
              }
            >
              Copy sign-in link
            </button>
          ) : null}
        </div>
      ) : null}
      {!snapshot ? (
        <p role="status">Reading shared MCP…</p>
      ) : !visible.length ? (
        <p className="plugins-none">
          No servers here yet. Add one, or paste the config from a server's README.
        </p>
      ) : (
        <ul className="plugins-rows">
          {visible.map((record) => (
            <li className="plugins-row shared-mcp-row" key={record.definition.id}>
              <div className="plugins-row-text">
                <strong>{record.definition.name}</strong>
                <small>
                  {record.definition.transport.type} ·{" "}
                  <span data-testid="shared-mcp-agents">
                    {agentsSummary(record.definition.agents, agents)}
                  </span>{" "}
                  ·{" "}
                  {record.definition.enabled
                    ? record.missing_credentials.length
                      ? "Needs local authentication"
                      : "Configured, session connection unverified"
                    : "Disabled"}
                </small>
                {record.missing_credentials.length ? (
                  <small>Required on this device: {record.missing_credentials.join(", ")}</small>
                ) : null}
              </div>
              <div className="shared-mcp-actions">
                <Switch
                  label={`${record.definition.name} enabled`}
                  checked={record.definition.enabled}
                  disabled={busy}
                  onChange={(enabled) =>
                    void run(() => api.save(snapshot.revision, { ...record.definition, enabled }))
                  }
                />
                {record.definition.env_keys.length || record.definition.header_keys.length ? (
                  <button
                    type="button"
                    className="g-button g-button-outline g-button-28"
                    disabled={busy}
                    onClick={() => setCredentials(record.definition)}
                  >
                    Authentication
                  </button>
                ) : null}
                <button
                  type="button"
                  className="g-button g-button-outline g-button-28"
                  disabled={busy}
                  onClick={() => openEditor(record.definition)}
                >
                  Edit
                </button>
                <RowMenu
                  label={`${record.definition.name} actions`}
                  items={[
                    {
                      label: "Test connection",
                      onSelect: () =>
                        void run(async () => {
                          const result = await api.test(
                            snapshot.revision,
                            record.definition,
                            activeChatId,
                          );
                          if (alive.current)
                            setStatus(`${record.definition.name}: ${result.message}`);
                        }),
                    },
                    ...(activeChatId
                      ? [
                          {
                            label: "Session status",
                            onSelect: () => sessionAction(record.definition, "status"),
                          },
                          {
                            label: "Sign in",
                            onSelect: () => sessionAction(record.definition, "login"),
                          },
                          {
                            label: "Restart active chat",
                            onSelect: () => sessionAction(record.definition, "restart"),
                          },
                        ]
                      : []),
                    ...(scope === "" && projects.length
                      ? [
                          {
                            label: "Create project override",
                            onSelect: () =>
                              openEditor({
                                ...record.definition,
                                project_id: initialProjectId ?? projects[0]?.id ?? null,
                              }),
                          },
                        ]
                      : []),
                    {
                      label: "Remove",
                      onSelect: () =>
                        void run(() => api.remove(snapshot.revision, record.definition)),
                    },
                  ]}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
      <details className="plugins-more">
        <summary>How shared servers work</summary>
        <ul>
          <li>
            A server added for a project applies to all its folders and worktrees, and replaces the
            all-projects server with the same ID there.
          </li>
          <li>
            Each agent's own configuration stays as it is. A server of the same name in it is shown
            for review, never overwritten.
          </li>
          <li>Changes apply when an idle chat restarts.</li>
          <li>Sign-ins and tokens stay on this device and are never exported.</li>
          <li>
            Test checks that the server starts. Which tools an agent then sees, and its own sign-in,
            show in that agent's chat.
          </li>
          <li>
            Codex, Claude Code and Pi load shared servers in their chats. Other agents load the
            kinds of server they say they support. Remote hosts and restricted fallback connections
            cannot load them.
          </li>
        </ul>
      </details>
      {editing && snapshot ? (
        <McpEditor
          definition={editing}
          projects={projects}
          agents={agents}
          busy={busy}
          onClose={() => setEditing(null)}
          onSave={(definition) =>
            void run(async () => {
              const result = await api.save(snapshot.revision, definition);
              if (alive.current) setEditing(null);
              setStatus("Definition saved. Restart compatible idle chats to apply this revision.");
              return result;
            })
          }
          onPaste={(text, projectId) =>
            void run(async () => {
              const result = await api.importNative(snapshot.revision, projectId, text);
              if (alive.current) setEditing(null);
              setStatus("Servers added. Restart compatible idle chats to load them.");
              return result;
            })
          }
        />
      ) : null}
      {credentials && snapshot ? (
        <CredentialEditor
          definition={credentials}
          busy={busy}
          onClose={() => setCredentials(null)}
          onSave={(values) =>
            void run(async () => {
              const result = await api.credentials(snapshot.revision, credentials, values);
              if (alive.current) setCredentials(null);
              return result;
            })
          }
        />
      ) : null}
      {document && snapshot ? (
        <Modal
          title={
            document.mode === "import"
              ? "Import shared MCP definitions"
              : "Export shared MCP definitions"
          }
          size="lg"
          onClose={() => setDocument(null)}
        >
          <p>
            Portable definitions include credential names only. Authentication values stay on this
            device. Imported definitions retain their Ghost IDs and scopes.
          </p>
          {document.mode === "import" ? (
            <label className="shared-mcp-field">
              Import format
              <Select
                className="g-input"
                value={nativeImport ? "native" : "aura"}
                aria-label="Import format"
                onChange={(e) => setNativeImport(e.currentTarget.value === "native")}
              >
                <option value="aura">Ghost portable definitions</option>
                <option value="native">Native mcpServers JSON</option>
              </Select>
            </label>
          ) : null}
          <label className="shared-mcp-field">
            Definition JSON
            <textarea
              className="g-input"
              aria-label="Definition JSON"
              rows={16}
              readOnly={document.mode === "export"}
              value={document.text}
              onChange={(e) => setDocument({ ...document, text: e.currentTarget.value })}
            />
          </label>
          {document.mode === "import" ? (
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={busy || !document.text.trim()}
              onClick={() =>
                void run(async () => {
                  const result = nativeImport
                    ? await api.importNative(snapshot.revision, scope || null, document.text)
                    : await api.import(snapshot.revision, document.text);
                  if (alive.current) setDocument(null);
                  return result;
                })
              }
            >
              Import definitions
            </button>
          ) : (
            <button
              type="button"
              className="g-button g-button-outline"
              onClick={() =>
                void navigator.clipboard
                  .writeText(document.text)
                  .catch((e) => setError(errorMessage(e)))
              }
            >
              Copy export
            </button>
          )}
        </Modal>
      ) : null}
    </div>
  );
}
function McpEditor({
  definition,
  projects,
  agents,
  busy,
  onClose,
  onSave,
  onPaste,
}: {
  readonly definition: McpDefinition;
  readonly projects: readonly McpProject[];
  readonly agents: readonly McpAgent[];
  readonly busy: boolean;
  readonly onClose: () => void;
  readonly onSave: (definition: McpDefinition) => void;
  /** Adds every server a pasted `mcpServers` config names, in the chosen scope. */
  readonly onPaste: (text: string, projectId: string | null) => void;
}) {
  const [value, setValue] = useState(definition);
  // A new server can be typed in or pasted as the JSON its README gives.
  const adding = definition.name === "";
  const [pasting, setPasting] = useState(false);
  const [pasted, setPasted] = useState("");
  const [args, setArgs] = useState(
    definition.transport.type === "stdio" ? definition.transport.args.join("\n") : "",
  );
  // The chosen agents survive switching to Every agent and back while the dialog is open.
  const [every, setEvery] = useState(!definition.agents);
  const [chosen, setChosen] = useState<ReadonlySet<string>>(() => new Set(definition.agents ?? []));
  const [keys, setKeys] = useState(
    (definition.transport.type === "stdio" ? definition.env_keys : definition.header_keys).join(
      "\n",
    ),
  );
  return (
    <Modal
      title={definition.name ? "Edit MCP server" : "Add MCP server"}
      size="lg"
      onClose={onClose}
    >
      {adding ? (
        <fieldset className="shared-mcp-how">
          <legend>How to add it</legend>
          <label>
            <input
              type="radio"
              name="mcp-how"
              checked={!pasting}
              onChange={() => setPasting(false)}
            />
            Fill in the form
          </label>
          <label>
            <input
              type="radio"
              name="mcp-how"
              checked={pasting}
              onChange={() => setPasting(true)}
            />
            Paste a config
          </label>
        </fieldset>
      ) : null}
      {adding && pasting ? (
        <form
          className="shared-mcp-form"
          onSubmit={(e) => {
            e.preventDefault();
            onPaste(pasted, value.project_id);
          }}
        >
          <label className="shared-mcp-field">
            Config JSON
            <textarea
              className="g-input shared-mcp-paste"
              rows={10}
              spellCheck={false}
              required
              placeholder={
                '{\n  "mcpServers": {\n    "docs": { "command": "npx", "args": ["-y", "docs-mcp"] }\n  }\n}'
              }
              value={pasted}
              onChange={(e) => setPasted(e.currentTarget.value)}
            />
          </label>
          <p className="plugins-note">
            The <code>mcpServers</code> block a server's README gives you. Every server in it is
            added for every agent. Secret values in it stay on this device.
          </p>
          <button
            type="submit"
            className="g-button g-button-primary"
            disabled={busy || !pasted.trim()}
          >
            Add from config
          </button>
        </form>
      ) : null}
      <form
        className="shared-mcp-form"
        hidden={adding && pasting}
        onSubmit={(e) => {
          e.preventDefault();
          if (!every && chosen.size === 0) return;
          onSave({
            ...value,
            agents: every ? null : [...chosen].sort(),
            transport:
              value.transport.type === "stdio"
                ? { ...value.transport, args: args.split("\n").filter(Boolean) }
                : value.transport,
            env_keys: value.transport.type === "stdio" ? keys.split("\n").filter(Boolean) : [],
            header_keys: value.transport.type !== "stdio" ? keys.split("\n").filter(Boolean) : [],
          });
        }}
      >
        <label className="shared-mcp-field">
          Name
          <input
            className="g-input"
            required
            value={value.name}
            pattern="[A-Za-z0-9_.-]+"
            onChange={(e) => setValue({ ...value, name: e.currentTarget.value })}
          />
        </label>
        <label className="shared-mcp-field">
          Scope
          <Select
            className="g-input"
            value={value.project_id ?? ""}
            aria-label="Scope"
            onChange={(e) => setValue({ ...value, project_id: e.currentTarget.value || null })}
          >
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            <option value="">All projects</option>
          </Select>
        </label>
        <AgentScope
          agents={agents}
          every={every}
          chosen={chosen}
          onEvery={setEvery}
          onToggle={(id) =>
            setChosen((current) => {
              const next = new Set(current);
              if (!next.delete(id)) next.add(id);
              return next;
            })
          }
        />
        <label className="shared-mcp-field">
          Transport
          <Select
            className="g-input"
            value={value.transport.type}
            aria-label="Transport"
            onChange={(e) => {
              const type = e.currentTarget.value;
              setValue({
                ...value,
                transport:
                  type === "stdio"
                    ? { type, command: "", args: [] }
                    : { type: type === "sse" ? "sse" : "http", url: "" },
              });
              setKeys("");
            }}
          >
            <option value="stdio">Stdio</option>
            <option value="http">Streamable HTTP</option>
            <option value="sse">Legacy SSE</option>
          </Select>
        </label>
        {value.transport.type === "stdio" ? (
          <>
            <label className="shared-mcp-field">
              Executable
              <input
                className="g-input"
                required
                value={value.transport.command}
                onChange={(e) =>
                  setValue({
                    ...value,
                    transport: { type: "stdio", command: e.currentTarget.value, args: [] },
                  })
                }
              />
            </label>
            <label className="shared-mcp-field">
              Arguments, one per line
              <textarea
                className="g-input"
                rows={4}
                value={args}
                onChange={(e) => setArgs(e.currentTarget.value)}
              />
            </label>
          </>
        ) : (
          <label className="shared-mcp-field">
            Endpoint URL
            <input
              className="g-input"
              type="url"
              required
              value={value.transport.url}
              onChange={(e) =>
                setValue({
                  ...value,
                  transport: {
                    ...(value.transport as { type: "http" | "sse"; url: string }),
                    url: e.currentTarget.value,
                  },
                })
              }
            />
          </label>
        )}
        <label className="shared-mcp-field">
          {value.transport.type === "stdio"
            ? "Local environment credential names"
            : "Local header credential names"}
          , one per line
          <textarea
            className="g-input"
            rows={3}
            value={keys}
            onChange={(e) => setKeys(e.currentTarget.value)}
          />
        </label>
        <p className="plugins-note">
          Store secrets using Authentication after saving. Portable commands, arguments and URLs
          must contain no secret values. Changing the executable, endpoint or credential names
          clears previous local authentication.
        </p>
        <button
          type="submit"
          className="g-button g-button-primary"
          disabled={busy || (!every && chosen.size === 0)}
        >
          Save definition
        </button>
      </form>
    </Modal>
  );
}
/**
 * Which agents load the server: every agent (with the ones installed later), or the ones chosen.
 * Each supported agent is a checkbox with its mark; one not installed says so, one whose launches
 * cannot take shared servers is shown but cannot be chosen, and an agent this device does not
 * know (chosen on a newer device) stays chosen under its ID.
 */
function AgentScope({
  agents,
  every,
  chosen,
  onEvery,
  onToggle,
}: {
  readonly agents: readonly McpAgent[];
  readonly every: boolean;
  readonly chosen: ReadonlySet<string>;
  readonly onEvery: (every: boolean) => void;
  readonly onToggle: (id: string) => void;
}) {
  const name = useId();
  const hint = useId();
  const unknown = [...chosen].filter((id) => !agents.some((agent) => agent.id === id));
  const rows: readonly McpAgent[] = [
    ...agents,
    ...unknown.map((id) => ({ id, name: id, installed: false })),
  ];
  const empty = !every && chosen.size === 0;
  return (
    <fieldset className="shared-mcp-agents" aria-describedby={hint}>
      <legend>Agents</legend>
      <label className="shared-mcp-choice">
        <input type="radio" name={name} checked={every} onChange={() => onEvery(true)} />
        <span>
          Every agent
          <small>Includes agents you install later.</small>
        </span>
      </label>
      <label className="shared-mcp-choice">
        <input type="radio" name={name} checked={!every} onChange={() => onEvery(false)} />
        <span>
          Only the agents I choose
          <small>Other agents start without this server.</small>
        </span>
      </label>
      {every ? null : (
        <ul className="shared-mcp-agent-list" aria-label="Agents that load this server">
          {rows.map((agent) => {
            const foreign = unknown.includes(agent.id);
            const supported = foreign || SHARED_MCP_AGENTS.has(agent.id);
            const note = foreign
              ? "Not on this device"
              : !supported
                ? "Can't load shared servers"
                : !agent.installed
                  ? "Not installed"
                  : null;
            return (
              <li key={agent.id}>
                <label className="shared-mcp-agent" data-disabled={supported ? undefined : ""}>
                  <input
                    type="checkbox"
                    checked={chosen.has(agent.id)}
                    disabled={!supported}
                    onChange={() => onToggle(agent.id)}
                  />
                  <span aria-hidden="true">
                    <AgentMark agent={agent.id} size={16} />
                  </span>
                  <span className="shared-mcp-agent-text">
                    <span>{agent.name}</span>
                    {note ? <small>{note}</small> : null}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
      )}
      <p id={hint} className={empty ? "ui-error" : "plugins-note"}>
        {empty ? "Choose at least one agent." : "Changes apply when an idle chat starts again."}
      </p>
    </fieldset>
  );
}

function CredentialEditor({
  definition,
  busy,
  onClose,
  onSave,
}: {
  readonly definition: McpDefinition;
  readonly busy: boolean;
  readonly onClose: () => void;
  readonly onSave: (values: {
    env: Record<string, string>;
    headers: Record<string, string>;
  }) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>({});
  const group = definition.transport.type === "stdio" ? "env" : "headers";
  const keys = group === "env" ? definition.env_keys : definition.header_keys;
  return (
    <Modal title={`Authentication: ${definition.name}`} onClose={onClose}>
      <p>
        Saved values are never displayed or exported. Supply all required values when replacing
        authentication. OAuth is controlled by each agent's native MCP session.
      </p>
      <form
        className="shared-mcp-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave({
            env: group === "env" ? values : {},
            headers: group === "headers" ? values : {},
          });
        }}
      >
        {keys.map((key) => (
          <label key={key} className="shared-mcp-field">
            {key}
            <input
              className="g-input"
              type="password"
              autoComplete="off"
              value={values[key] ?? ""}
              required
              onChange={(e) => setValues({ ...values, [key]: e.currentTarget.value })}
            />
          </label>
        ))}
        {keys.length ? (
          <button type="submit" className="g-button g-button-primary" disabled={busy}>
            Save local authentication
          </button>
        ) : (
          <p>
            No local credential fields are declared. Add required environment or header names to the
            definition, or sign in through a compatible agent's native OAuth flow.
          </p>
        )}
        <button
          type="button"
          className="g-button g-button-ghost"
          disabled={busy}
          onClick={() => onSave({ env: {}, headers: {} })}
        >
          Clear local authentication
        </button>
      </form>
    </Modal>
  );
}
