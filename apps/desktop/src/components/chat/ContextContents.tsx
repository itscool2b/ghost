import { AgentMark, CopyButton } from "@ghost/ui/graphite";
import { Check, Circle, FileText, Search } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";
import type {
  AgentEntry,
  FileMention,
  McpLoadState,
  ProjectFilePreview,
  SessionLoaded,
} from "../../bindings";
import type { ChatState } from "../../lib/chat/chatStore";
import { autonomyLabel, costLabel, levelName, tokensLabel } from "../../lib/chat/present";
import type { ComposerTransport } from "../../lib/composer";
import { composerDraft } from "../../lib/composerDraft";
import { errorMessage } from "../../lib/errors";
import { agentVersion, formatDuration } from "../../lib/format";

export type ContextTab = "Changes" | "Files" | "Plan" | "Session";

export function ContextContents({
  tab,
  state,
  api,
  agent,
}: {
  readonly tab: Exclude<ContextTab, "Changes">;
  readonly state: ChatState;
  readonly api?: ComposerTransport;
  readonly agent: AgentEntry | null;
}) {
  if (tab === "Files") return <Files state={state} api={api} />;
  if (tab === "Session") return <Session state={state} agent={agent} />;
  const plan = state.transcript.segments.findLast((segment) => segment.plan?.length)?.plan;
  return (
    <div className="context-panel-content">
      {plan ? (
        <>
          <p className="context-plan-count">
            {plan.filter((entry) => entry.status === "completed").length}
            <span> / {plan.length} complete</span>
          </p>
          <ol className="context-plan">
            {plan.map((entry, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: The agent replaces the entire plan without stable step IDs.
              <li key={`${index}:${entry.content}`} data-status={entry.status}>
                <span aria-hidden="true">
                  {entry.status === "completed" ? <Check size={14} /> : <Circle size={12} />}
                </span>
                <div>
                  {entry.content}
                  <small>{entry.status.replaceAll("_", " ")}</small>
                </div>
              </li>
            ))}
          </ol>
        </>
      ) : (
        <div className="context-empty">
          <strong>No plan reported</strong>
          <p>The agent's plan appears here when it shares one.</p>
        </div>
      )}
    </div>
  );
}

function Files({ state, api }: { readonly state: ChatState; readonly api?: ComposerTransport }) {
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<FileMention[]>([]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [file, setFile] = useState<ProjectFilePreview | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const fileLines = file?.text?.split("\n", 2001) ?? [];
  const touched = useMemo(() => {
    const paths = new Map<string, string>();
    const folder = (state.chat?.folder ?? "").replaceAll("\\", "/").replace(/\/$/, "");
    const add = (path: string, status: string) => {
      const normalized = path.replaceAll("\\", "/");
      const relative = normalized.startsWith(`${folder}/`)
        ? normalized.slice(folder.length + 1)
        : normalized;
      // Only present portable relative paths. Rust independently enforces the boundary on selection.
      if (
        relative.startsWith("/") ||
        /^[a-z]:/i.test(relative) ||
        relative.split("/").includes("..")
      )
        return;
      if (paths.size < 100 || paths.has(relative)) paths.set(relative, status);
    };
    for (const item of state.transcript.items.values()) {
      if (item.kind !== "tool") continue;
      for (const location of item.locations)
        add(location.path, item.toolKind === "edit" ? "M" : "R");
      for (const content of item.content)
        if (content.type === "diff") add(content.path, content.old_text === null ? "A" : "M");
    }
    return [...paths].sort(([a], [b]) => a.localeCompare(b));
  }, [state.transcript.items, state.chat?.folder]);
  useEffect(() => {
    if (!api) return;
    let stale = false;
    setSearchError(null);
    const timer = setTimeout(() => {
      void api
        .mentions(state.chatId, query)
        .then((value) => {
          if (!stale) setMatches(value.filter((item) => !item.directory));
        })
        .catch((error) => {
          if (!stale) {
            setMatches([]);
            setSearchError(errorMessage(error));
          }
        });
    }, 120);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [api, state.chatId, query]);
  useEffect(() => {
    void revision;
    setFile(null);
    setError(null);
    setMessage(null);
    if (!path || !api?.file) return;
    let stale = false;
    setLoading(true);
    void api
      .file(state.chatId, path)
      .then((value) => {
        if (!stale) setFile(value);
      })
      .catch((error) => {
        if (!stale) setError(errorMessage(error));
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [api, path, state.chatId, revision]);
  const row = (name: string, status?: string) => (
    <button
      type="button"
      key={name}
      className="context-file-row"
      aria-pressed={path === name}
      disabled={!api?.file}
      title={name}
      onClick={() => {
        setPath(name);
        setRevision((value) => value + 1);
      }}
    >
      <FileText size={14} />
      <span>{name}</span>
      <small>{status}</small>
    </button>
  );
  return (
    <div className="context-panel-content">
      <label className="context-file-search">
        <Search size={14} aria-hidden="true" />
        <input
          className="g-input"
          aria-label="Find project files"
          placeholder="Find a file…"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
        />
      </label>
      {!query && touched.length ? (
        <section aria-label="Files in the loaded conversation">
          <h3 className="context-group-label">
            {state.hasMore ? "Touched in loaded history" : "Touched in this chat"}
          </h3>
          {touched.map(([name, status]) => row(name, status))}
        </section>
      ) : null}
      <section aria-label="Project files">
        <h3 className="context-group-label">Project files</h3>
        {matches.map((item) => row(item.path))}
        {!matches.length && !searchError ? (
          <p className="context-hint">No matching project files.</p>
        ) : null}
        <p className="context-hint">
          Up to 100 matches. Hidden, generated and credential files are excluded.
        </p>
        {searchError ? (
          <p role="alert" className="ui-error">
            {searchError}
          </p>
        ) : null}
      </section>
      {loading ? (
        <p role="status" className="context-hint">
          Opening file…
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="ui-error">
          {error}
        </p>
      ) : null}
      {file ? (
        <section className="context-file-preview" aria-label="Read-only file preview">
          <header>
            <strong title={file.path}>{file.path.split("/").at(-1)}</strong>
            <span>{file.bytes.toLocaleString()} bytes</span>
            <CopyButton text={file.path} label="Copy file path" />
          </header>
          {file.text === null ? (
            <p className="context-hint">
              This file is binary or uses a text encoding the viewer does not support.
            </p>
          ) : (
            // biome-ignore lint/a11y/noNoninteractiveTabindex: Keyboard users must be able to scroll the read-only code region.
            <section className="context-code-view" tabIndex={0} aria-label={file.path}>
              <pre>
                <span className="context-code-lines" aria-hidden="true">
                  {fileLines
                    .slice(0, 2000)
                    .map((_line, index) => index + 1)
                    .join("\n")}
                </span>
                <code>{fileLines.slice(0, 2000).join("\n")}</code>
              </pre>
            </section>
          )}
          {file.truncated || fileLines.length > 2000 ? (
            <p className="context-hint">
              Preview limited to 128 KiB or 2,000 lines. The original file is unchanged.
            </p>
          ) : null}
          {api ? (
            <button
              type="button"
              className="g-button g-button-28"
              onClick={() => {
                const draft = composerDraft(state.chatId, api);
                const saved = draft.getSnapshot();
                if (!saved.ready) {
                  setMessage("Wait for your saved draft to open, then try again.");
                  return;
                }
                if (saved.mentions.length >= 32) {
                  setMessage("Remove a file mention before adding another.");
                  return;
                }
                draft.update({ mentions: [...new Set([...saved.mentions, file.path])] });
                setMessage("Added to your draft.");
              }}
            >
              Mention in message
            </button>
          ) : null}
          {message ? (
            <p role="status" className="context-hint">
              {message}
            </p>
          ) : null}
        </section>
      ) : null}
    </div>
  );
}

const MCP_STATE: Record<McpLoadState, string> = {
  connected: "Connected",
  starting: "Starting",
  needs_sign_in: "Needs sign-in",
  failed: "Failed",
  off: "Off",
  unknown: "State not reported",
};

/** One kind of thing a session loaded, folded to its count until opened. */
function LoadedGroup({
  name,
  items,
}: {
  readonly name: string;
  readonly items: readonly string[];
}) {
  if (items.length === 0) return null;
  return (
    <details className="context-loaded-group">
      <summary>
        {name} <small className="context-loaded-count">{items.length}</small>
      </summary>
      <ul>
        {items.map((item, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: an agent can list one name twice.
          <li key={`${index}:${item}`} className="context-loaded-item">
            {item}
          </li>
        ))}
      </ul>
    </details>
  );
}

/**
 * What the running session loaded, as its agent reports it: MCP servers with their state (a dot
 * and its word), skills, plugins and subagents, each folded to its count. MCP servers open when
 * one failed or waits for a sign-in.
 */
function Loaded({
  loaded,
  started,
}: {
  readonly loaded: SessionLoaded | null;
  readonly started: boolean;
}) {
  const heading = useId();
  if (!started) return null;
  const empty =
    !loaded ||
    (loaded.mcp_servers.length === 0 &&
      loaded.skills.length === 0 &&
      loaded.plugins.length === 0 &&
      loaded.agents.length === 0 &&
      loaded.tools === null);
  return (
    <section className="context-loaded" aria-labelledby={heading}>
      <h3 id={heading} className="context-group-label">
        Loaded
      </h3>
      {empty || !loaded ? (
        <p className="context-hint">
          {loaded ? "The agent reported nothing loaded." : "The agent has reported nothing yet."}
        </p>
      ) : (
        <>
          {loaded.mcp_servers.length ? (
            <details
              className="context-loaded-group"
              open={loaded.mcp_servers.some(
                (server) => server.state === "failed" || server.state === "needs_sign_in",
              )}
            >
              <summary>
                MCP servers{" "}
                <small className="context-loaded-count">{loaded.mcp_servers.length}</small>
              </summary>
              <ul>
                {loaded.mcp_servers.map((server) => (
                  <li
                    key={server.name}
                    className="context-loaded-item"
                    data-test-mcp-state={server.state}
                  >
                    <span
                      className="context-loaded-state"
                      data-state={server.state}
                      aria-hidden="true"
                    />
                    <span>{server.name}</span>
                    <small className="context-loaded-note">
                      {MCP_STATE[server.state]}
                      {server.source ? ` · ${server.source}` : ""}
                    </small>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          <LoadedGroup name="Skills" items={loaded.skills} />
          <LoadedGroup
            name="Plugins"
            items={loaded.plugins.map((plugin) =>
              plugin.version ? `${plugin.name} ${plugin.version}` : plugin.name,
            )}
          />
          <LoadedGroup name="Subagents" items={loaded.agents} />
          {loaded.tools !== null ? (
            <p className="context-hint">
              {loaded.tools} {loaded.tools === 1 ? "tool" : "tools"} available, MCP tools included
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

function Session({
  state,
  agent,
}: {
  readonly state: ChatState;
  readonly agent: AgentEntry | null;
}) {
  const session = state.session;
  const usage = state.usage;
  const choiceLabel = (category: string) => {
    const option = state.lists?.options.find((option) => option.category === category);
    if (option?.kind.type !== "select") return null;
    const current = option.kind.current;
    const choice = option.kind.choices.find((choice) => choice.value === current);
    if (!choice) return current || null;
    return category === "effort" ? levelName(choice) : choice.name;
  };
  const recent = state.transcript.segments.findLast((segment) => segment.end !== null);
  const tokens = state.transcript.segments.findLast((segment) => segment.end?.usage)?.end?.usage;
  const context =
    usage?.context_used !== null && usage?.context_used !== undefined && usage.context_size
      ? Math.max(0, Math.min(100, (usage.context_used / usage.context_size) * 100))
      : null;
  const properties = [
    ["Model", choiceLabel("model") ?? session?.model ?? "Not reported"],
    ["Thinking", choiceLabel("effort") ?? session?.effort ?? "Not reported"],
    ["Autonomy", autonomyLabel(session?.autonomy ?? "full_auto")],
    ["Folder", state.chat?.folder ?? "Unavailable"],
    [
      "Version",
      agentVersion(
        agent?.name ?? state.chat?.agent_name ?? "",
        session?.agent_version ??
          (agent?.availability.state === "ready" ? agent.availability.version : null) ??
          "Not reported",
      ),
    ],
  ];
  return (
    <div className="context-panel-content">
      <dl className="context-properties">
        <div>
          <dt>Agent</dt>
          <dd>
            <AgentMark agent={state.chat?.agent_id ?? "generic"} size={16} />
            {agent?.name ?? state.chat?.agent_name ?? "Agent"}
          </dd>
        </div>
        {properties.map(([name, value]) => (
          <div key={name}>
            <dt>{name}</dt>
            <dd title={value}>{value}</dd>
          </div>
        ))}
        <div>
          <dt>Session</dt>
          <dd className="context-session-id" title={session?.native_session_id ?? undefined}>
            {session?.native_session_id ?? "Not started"}
            {session?.native_session_id ? (
              <CopyButton text={session.native_session_id} label="Copy session ID" />
            ) : null}
          </dd>
        </div>
      </dl>
      <div className="context-usage-tiles">
        <div>
          <span>Session cost</span>
          <strong>{costLabel(usage) ?? "Not reported"}</strong>
        </div>
        <div>
          <span>Last turn</span>
          <strong>
            {recent?.end?.durationMs !== null && recent?.end?.durationMs !== undefined
              ? formatDuration(recent.end.durationMs)
              : "Not reported"}
          </strong>
        </div>
      </div>
      <p className="context-hint">
        {tokens ? tokensLabel(tokens) : "Token usage has not been reported."}
      </p>
      {context !== null ? (
        <div className="context-capacity">
          <progress max={100} value={context} aria-label="Context used" />
          <span>
            {Math.round(context)}% of {usage?.context_size?.toLocaleString()} context tokens
          </span>
        </div>
      ) : null}
      {state.limits?.map((limit) => (
        <div className="context-capacity" key={limit.label}>
          <label>
            {limit.label}
            <progress max={100} value={Math.max(0, Math.min(100, limit.used_percent ?? 0))} />
          </label>
          <span>
            {limit.used_percent === null
              ? "Usage unavailable"
              : `${Math.round(limit.used_percent)}% used`}
            {limit.resets_at ? ` · resets ${new Date(limit.resets_at).toLocaleString()}` : ""}
          </span>
        </div>
      ))}
      <Loaded
        loaded={state.lists?.loaded ?? null}
        started={state.lists?.capabilities != null || state.lists?.loaded != null}
      />
      <p className="context-hint">
        {state.queue.length} queued {state.queue.length === 1 ? "message" : "messages"}
        {session?.queue_held ? " · paused" : ""}
      </p>
    </div>
  );
}
