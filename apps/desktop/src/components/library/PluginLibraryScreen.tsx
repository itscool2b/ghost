import { AgentMark, StatusDot } from "@ghost/ui/graphite";
import { ArrowLeft, Plus, RefreshCw, Search } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  AgentPlugins,
  FeatureListing,
  PluginEntry,
  PluginLibrary,
  VendorAction,
  VendorProvider,
} from "../../bindings";
import { vendorApi as nativeVendorApi, type VendorApi } from "../../lib/config-vendor/api";
import { errorMessage } from "../../lib/errors";
import { type LibraryApi, libraryApi as nativeLibraryApi } from "../../lib/library/api";
import type {
  LibraryCopyReceipt,
  LibraryItem,
  LibrarySourceStatus,
} from "../../lib/library/bindings";
import { pluginsApi as nativePluginsApi, type PluginsApi } from "../../lib/plugins/api";
import {
  addsMarketplaces,
  ago,
  featureLabel,
  harnessText,
  harnessTone,
  sourceText,
} from "../../lib/plugins/format";
import { PluginLibraryLoader } from "../../lib/plugins/loading";
import type { SharedMcpApi } from "../../lib/sharedMcp";
import { Modal } from "../chat/Modal";
import { HistoryView, ItemDetail } from "./LibraryItems";
import { type MenuItem, RowMenu } from "./RowMenu";
import { type McpProject, SHARED_MCP_AGENTS, SharedMcpPanel } from "./SharedMcpPanel";
import { UniversalPlugins, type UniversalSettings } from "./UniversalPlugins";
import { VendorDialog, type VendorPreset } from "./VendorDialog";
import "./library.css";
import "./plugin-library.css";

const AVAILABLE_SHOWN = 20;

/**
 * The Plugins & MCP (Vercel's project pages): the agents on the left, and for the chosen agent
 * its harness status and what it supports, each feature as rows on hairlines with
 * where they came from. Rows read from the agent's own CLI where it lists them; the rest come from
 * its files. Every change runs the agent's own reviewed command in a terminal first.
 */
export function PluginLibraryScreen({
  onBack,
  onAgents,
  initialAgent = "shared",
  api = nativePluginsApi,
  libraryApi = nativeLibraryApi,
  vendorApi = nativeVendorApi,
  now = Date.now,
  embedded = false,
  titled = true,
  initialProjectId = null,
  projects = [],
  mcpApi,
  activeChatId,
  universal,
  select = null,
}: {
  readonly onBack: () => void;
  readonly onAgents?: () => void;
  readonly initialAgent?: string;
  readonly api?: PluginsApi;
  readonly libraryApi?: LibraryApi;
  readonly vendorApi?: VendorApi;
  readonly now?: () => number;
  readonly embedded?: boolean;
  /** Settings names the page itself: the library then shows only its description and Refresh. */
  readonly titled?: boolean;
  readonly initialProjectId?: string | null;
  readonly projects?: readonly McpProject[];
  readonly mcpApi?: SharedMcpApi;
  readonly activeChatId?: string | null;
  /** Settings' preferences: Universal plugins is offered with them. */
  readonly universal?: UniversalSettings;
  /** Opens a part of the page (a search result in it): a new `id` asks again. */
  readonly select?: { readonly id: number; readonly agent: string } | null;
}) {
  const [library, setLibrary] = useState<PluginLibrary | null>(null);
  // The page opens on MCP servers (owner, 10 November 2026: they were "hiding" under an agent's
  // plugin lists, with no plain way to add one).
  const [selected, setSelected] = useState(initialAgent);
  // An agent's own list asks for a server just for that agent: the shared list adds it.
  const [addFor, setAddFor] = useState<{ readonly id: number; readonly agent: string } | null>(
    null,
  );
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState<"refresh" | "update" | null>(null);
  const [vendor, setVendor] = useState<VendorPreset | null>(null);
  const [preview, setPreview] = useState<{
    item: LibraryItem;
    sources: LibrarySourceStatus[];
  } | null>(null);
  const [history, setHistory] = useState<{
    receipts: LibraryCopyReceipt[];
    sources: LibrarySourceStatus[];
  } | null>(null);
  // A search result opens its part of the page before Settings looks for its row.
  useLayoutEffect(() => {
    if (select) setSelected(select.agent);
  }, [select]);
  const alive = useRef(true);
  const loader = useRef(new PluginLibraryLoader());
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      loader.current.cancel();
    };
  }, []);
  const load = async (refresh: boolean) => {
    if (!alive.current) return;
    if (refresh) setBusy("refresh");
    setError(null);
    await loader.current.load(api, refresh, {
      value: (value) => {
        setLibrary(value);
        setLoading(false);
      },
      error: (reason) => setError(errorMessage(reason)),
      done: () => {
        setLoading(false);
        if (refresh) setBusy(null);
      },
    });
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: the first read only; Refresh reruns it.
  useEffect(() => {
    void load(false);
  }, [api]);
  const agent = library?.agents.find((entry) => entry.agent_id === selected) ?? null;
  const openItem = async (entry: PluginEntry) => {
    const ref = entry.library;
    if (!ref) return;
    setError(null);
    try {
      const inventory = await libraryApi.snapshot(null);
      const item = inventory.items.find(
        (candidate) =>
          candidate.item.source_id === ref.source_id &&
          candidate.item.kind === ref.kind &&
          candidate.item.key === ref.key,
      );
      if (!item) throw new Error("This item changed. Refresh the Plugins & MCP and try again.");
      if (alive.current) setPreview({ item, sources: inventory.sources });
    } catch (reason) {
      if (alive.current) setError(errorMessage(reason));
    }
  };
  const openHistory = async () => {
    setError(null);
    try {
      const [inventory, receipts] = await Promise.all([
        libraryApi.snapshot(null),
        libraryApi.history(null),
      ]);
      if (alive.current) setHistory({ receipts, sources: inventory.sources });
    } catch (reason) {
      if (alive.current) setError(errorMessage(reason));
    }
  };
  const update = async (target: AgentPlugins) => {
    setBusy("update");
    setStatus(null);
    setError(null);
    try {
      await api.update(target.agent_id);
      if (alive.current) setStatus(`${target.name} updated. Its plugins were read again.`);
      await load(false);
    } catch (reason) {
      if (alive.current) setError(errorMessage(reason));
    } finally {
      if (alive.current) setBusy(null);
    }
  };
  const Container = embedded ? "section" : "main";
  return (
    <Container
      className="library-screen plugins-screen g-root"
      data-embedded={embedded || undefined}
    >
      {!embedded ? (
        <>
          <div className="library-drag" data-tauri-drag-region="" />
          <header className="library-topbar">
            <button type="button" className="g-button g-button-ghost g-button-28" onClick={onBack}>
              <ArrowLeft size={14} aria-hidden="true" /> Back
            </button>
          </header>
        </>
      ) : null}
      <div className="library-heading">
        <div>
          {titled ? <h1>Plugins & MCP</h1> : null}
          <p>What each agent has installed, and the MCP servers your agents share.</p>
        </div>
        <div className="library-heading-actions">
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon"
            aria-label="Refresh Plugins & MCP"
            title="Refresh"
            onClick={() => void load(true)}
            disabled={loading || busy !== null}
          >
            <RefreshCw size={16} aria-hidden="true" />
          </button>
        </div>
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
      <div className="plugins-body">
        <nav className="plugins-agents" aria-label="Agents">
          <button
            type="button"
            aria-current={selected === "shared" ? "page" : undefined}
            onClick={() => setSelected("shared")}
          >
            MCP servers
          </button>
          {universal ? (
            <button
              type="button"
              data-testid="plugins-universal-nav"
              aria-current={selected === "universal" ? "page" : undefined}
              onClick={() => setSelected("universal")}
            >
              Universal plugins
            </button>
          ) : null}
          {(library?.agents ?? []).map((entry) => (
            <button
              type="button"
              key={entry.agent_id}
              aria-current={entry.agent_id === selected ? "page" : undefined}
              onClick={() => setSelected(entry.agent_id)}
            >
              <span className="plugins-agent-mark" aria-hidden="true">
                <AgentMark agent={entry.agent_id} size={16} ghost={!entry.installed} />
              </span>
              <span className="plugins-agent-name">{entry.name}</span>
              {harnessTone(entry.harness) === "volt" ? (
                <span className="plugins-agent-dot" title="Update available">
                  <StatusDot tone="volt" label="Update available" />
                </span>
              ) : null}
            </button>
          ))}
          {library?.notices.length ? (
            <p className="plugins-agents-note">{library.notices[library.notices.length - 1]}</p>
          ) : null}
        </nav>
        <section
          className="plugins-main"
          aria-label={
            agent
              ? agent.name
              : selected === "universal" && universal
                ? "Universal plugins"
                : "Plugins & MCP"
          }
        >
          {selected === "universal" && universal ? (
            <UniversalPlugins {...universal} />
          ) : selected === "shared" ? (
            <SharedMcpPanel
              projects={projects}
              agents={(library?.agents ?? []).map((entry) => ({
                id: entry.agent_id,
                name: entry.name,
                installed: entry.installed,
              }))}
              initialProjectId={initialProjectId}
              api={mcpApi}
              activeChatId={activeChatId}
              addFor={addFor}
            />
          ) : loading ? (
            <p className="plugins-empty" role="status">
              Reading what your agents use…
            </p>
          ) : agent ? (
            <AgentSection
              key={agent.agent_id}
              agent={agent}
              busy={busy}
              now={now}
              onUpdate={() => void update(agent)}
              onVendor={setVendor}
              onOpen={(entry) => void openItem(entry)}
              onHistory={() => void openHistory()}
              onRefresh={() => void load(true)}
              onAgents={onAgents}
              onAddMcp={
                SHARED_MCP_AGENTS.has(agent.agent_id)
                  ? () => {
                      setAddFor({ id: Date.now(), agent: agent.agent_id });
                      setSelected("shared");
                    }
                  : undefined
              }
            />
          ) : (
            <div className="plugins-empty">
              <h2>Nothing to show</h2>
              <p>Refresh to read your agents again.</p>
            </div>
          )}
        </section>
      </div>
      {vendor ? (
        <VendorDialog
          api={vendorApi}
          preset={vendor}
          capabilities={library?.agents.flatMap((agent) =>
            agent.management ? [agent.management] : [],
          )}
          onClose={() => {
            setVendor(null);
            void load(true);
          }}
        />
      ) : null}
      {preview ? (
        <Modal
          title={preview.item.label}
          size="lg"
          className="plugins-item-dialog"
          onClose={() => setPreview(null)}
        >
          <ItemDetail
            api={libraryApi}
            folder={null}
            item={preview.item}
            sources={preview.sources}
            complete={(message) => {
              setPreview(null);
              setStatus(message);
              void load(false);
            }}
            manage={() => setPreview(null)}
          />
        </Modal>
      ) : null}
      {history ? (
        <Modal
          title="Copy history"
          size="lg"
          className="plugins-item-dialog"
          onClose={() => setHistory(null)}
        >
          <HistoryView
            api={libraryApi}
            folder={null}
            receipts={history.receipts}
            sources={history.sources}
            complete={(message) => {
              setHistory(null);
              setStatus(message);
            }}
          />
        </Modal>
      ) : null}
    </Container>
  );
}

export function AgentSection({
  agent,
  busy,
  now,
  onUpdate,
  onVendor,
  onOpen,
  onHistory,
  onRefresh,
  onAgents,
  onAddMcp,
}: {
  readonly agent: AgentPlugins;
  readonly busy: "refresh" | "update" | null;
  readonly now: () => number;
  readonly onUpdate: () => void;
  readonly onVendor: (preset: VendorPreset) => void;
  readonly onOpen: (entry: PluginEntry) => void;
  readonly onHistory: () => void;
  readonly onRefresh: () => void;
  readonly onAgents?: () => void;
  /** Adds a server for this agent alone, where the shared list keeps it. */
  readonly onAddMcp?: () => void;
}) {
  const management = agent.management?.version === 1 ? agent.management : null;
  const provider = management?.provider;
  const tone = harnessTone(agent.harness);
  const install =
    management?.actions.find(
      (action) => action.kind !== "marketplace" && action.actions.includes("install"),
    )?.kind ?? null;
  const menu: MenuItem[] = [{ label: "Refresh listings", onSelect: onRefresh }];
  if (provider && addsMarketplaces(management))
    menu.push({
      label: agent.agent_id === "hermes" ? "Add skill source…" : "Add marketplace…",
      onSelect: () => onVendor({ provider, kind: "marketplace", action: "install", target: "" }),
    });
  menu.push({ label: "Copy history", onSelect: onHistory });
  if (onAgents) menu.push({ label: "Open Agents", onSelect: onAgents });
  if (provider)
    for (const route of management?.native_routes ?? [])
      menu.push({
        label: route.label,
        onSelect: () => onVendor({ provider, kind: "native", action: "open", target: route.id }),
      });
  return (
    <div className="plugins-agent">
      <header className="plugins-agent-head">
        <span className="plugins-agent-head-mark" aria-hidden="true">
          <AgentMark agent={agent.agent_id} size={32} ghost={!agent.installed} />
        </span>
        <div className="plugins-agent-title">
          <h2>{agent.name}</h2>
          <p>
            {agent.version ? <code>{agent.version}</code> : null}
            {agent.version ? " · " : null}
            {agent.updated_at !== null
              ? `Updated ${ago(agent.updated_at, now())}`
              : !agent.installed && agent.harness.state === "checking"
                ? "Checking installation…"
                : agent.installed
                  ? "Read from its files"
                  : "Not installed here"}
          </p>
        </div>
        <div className="plugins-agent-actions">
          {provider && install && agent.installed ? (
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              onClick={() => onVendor({ provider, kind: install, action: "install", target: "" })}
            >
              {install === "package" ? "Add package" : "Add plugin"}
            </button>
          ) : null}
          <RowMenu label={`${agent.name} actions`} items={menu} />
        </div>
      </header>
      <div
        className="plugins-harness"
        data-tone={tone ?? undefined}
        role={agent.harness.state === "update_available" ? "status" : undefined}
      >
        {tone ? <StatusDot tone={tone} /> : null}
        <span>{harnessText(agent.name, agent.harness)}</span>
        {agent.harness.state === "update_available" ? (
          <button
            type="button"
            className="g-button g-button-outline g-button-28"
            disabled={busy !== null}
            aria-label={`Update ${agent.name} to ${agent.harness.latest}`}
            onClick={onUpdate}
          >
            {busy === "update" ? "Updating…" : "Update"}
          </button>
        ) : null}
      </div>
      {agent.version_change ? (
        <p className="plugins-note">
          {agent.name} changed from <code>{agent.version_change.from}</code> to{" "}
          <code>{agent.version_change.to}</code>, so its plugins were read again.
        </p>
      ) : null}
      <p className="plugins-summary">{agent.summary}</p>
      {agent.notices.map((notice) => (
        <p key={notice} className="plugins-note">
          {notice}
        </p>
      ))}
      {/* MCP servers lead: under the plugin lists they took a long scroll to find. */}
      {[...agent.features]
        .sort((a, b) => Number(b.feature === "mcp_servers") - Number(a.feature === "mcp_servers"))
        .map((feature) => (
          <Feature
            key={feature.feature}
            agent={agent}
            feature={feature}
            provider={provider}
            onVendor={onVendor}
            onOpen={onOpen}
            onAddMcp={feature.feature === "mcp_servers" ? onAddMcp : undefined}
          />
        ))}
    </div>
  );
}

function Feature({
  agent,
  feature,
  provider,
  onVendor,
  onOpen,
  onAddMcp,
}: {
  readonly agent: AgentPlugins;
  readonly feature: FeatureListing;
  readonly provider: VendorProvider | undefined;
  readonly onVendor: (preset: VendorPreset) => void;
  readonly onOpen: (entry: PluginEntry) => void;
  readonly onAddMcp?: () => void;
}) {
  const [query, setQuery] = useState("");
  const label = featureLabel(feature.feature, agent.agent_id);
  const failed = feature.state.state === "failed";
  const matches = feature.available.filter((entry) =>
    `${entry.label} ${entry.description ?? ""} ${entry.marketplace ?? ""}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  const headingId = `plugins-${agent.agent_id}-${feature.feature}`;
  // The command a listing was read with is the heading's tooltip. The page says where rows came
  // from only when that is not the agent's own listing: its folder, a failure, or nothing.
  const source = sourceText(feature.state, feature.command);
  const plain = feature.state.state === "live";
  // Adding a marketplace sits where its list is, not only in the agent's menu.
  const adds =
    feature.feature === "marketplaces" &&
    provider !== undefined &&
    agent.installed &&
    addsMarketplaces(agent.management?.version === 1 ? agent.management : null);
  return (
    <section className="plugins-feature" aria-labelledby={headingId}>
      <div className="plugins-feature-head">
        <h3 id={headingId} title={plain ? source : undefined}>
          {label} <span className="plugins-count">{feature.entries.length}</span>
        </h3>
        {plain ? null : (
          <p className="plugins-source" data-failed={failed || undefined}>
            {source}
          </p>
        )}
        {onAddMcp ? (
          <button
            type="button"
            className="g-button g-button-outline g-button-28 plugins-add"
            data-testid="add-agent-mcp"
            onClick={onAddMcp}
          >
            <Plus size={14} aria-hidden="true" />
            Add MCP server
          </button>
        ) : null}
        {adds && provider ? (
          <button
            type="button"
            className="g-button g-button-outline g-button-28 plugins-add"
            data-testid="add-marketplace"
            onClick={() =>
              onVendor({ provider, kind: "marketplace", action: "install", target: "" })
            }
          >
            <Plus size={14} aria-hidden="true" />
            {agent.agent_id === "hermes" ? "Add skill source" : "Add marketplace"}
          </button>
        ) : null}
      </div>
      {feature.entries.length ? (
        <ul className="plugins-rows">
          {feature.entries.map((entry) => (
            <Row
              key={entry.key}
              agent={agent}
              feature={feature}
              entry={entry}
              provider={provider}
              onVendor={onVendor}
              onOpen={onOpen}
            />
          ))}
        </ul>
      ) : (
        <p className="plugins-none">
          {agent.installed ? `No ${label.toLowerCase()} yet.` : "Nothing here on this device."}
        </p>
      )}
      {feature.available_total > 0 ? (
        <div className="plugins-available">
          <div className="plugins-available-head">
            <h4>Available from your marketplaces</h4>
            <label className="library-search plugins-search">
              <Search size={16} aria-hidden="true" />
              <input
                className="g-input"
                type="search"
                aria-label={`Find an available ${agent.name} plugin`}
                placeholder="Find a plugin…"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
              />
            </label>
          </div>
          <ul className="plugins-rows">
            {matches.slice(0, AVAILABLE_SHOWN).map((entry) => (
              <Row
                key={entry.key}
                agent={agent}
                feature={feature}
                entry={entry}
                provider={provider}
                onVendor={onVendor}
                onOpen={onOpen}
              />
            ))}
          </ul>
          <p className="plugins-none">
            {matches.length === 0
              ? "No available plugin matches."
              : `Showing ${Math.min(AVAILABLE_SHOWN, matches.length)} of ${
                  query ? matches.length : feature.available_total
                }.`}
          </p>
        </div>
      ) : null}
    </section>
  );
}

function Row({
  agent,
  feature,
  entry,
  provider,
  onVendor,
  onOpen,
}: {
  readonly agent: AgentPlugins;
  readonly feature: FeatureListing;
  readonly entry: PluginEntry;
  readonly provider: VendorProvider | undefined;
  readonly onVendor: (preset: VendorPreset) => void;
  readonly onOpen: (entry: PluginEntry) => void;
}) {
  const capability =
    agent.management?.version === 1
      ? agent.management.actions.find((action) => action.feature === feature.feature)
      : null;
  const kind = capability?.kind ?? null;
  const vendor = (action: VendorAction) => () =>
    provider && kind ? onVendor({ provider, kind, action, target: entry.key }) : undefined;
  const items: MenuItem[] = [];
  if (entry.library) items.push({ label: "Preview and copy…", onSelect: () => onOpen(entry) });
  if (provider && kind && entry.installed && agent.installed) {
    if (capability?.actions.includes("enable") && capability.actions.includes("disable"))
      items.push(
        entry.enabled === false
          ? { label: "Enable…", onSelect: vendor("enable") }
          : { label: "Disable…", onSelect: vendor("disable") },
      );
    if (capability?.actions.includes("update"))
      items.push({ label: "Update…", onSelect: vendor("update") });
    if (capability?.actions.includes("remove"))
      items.push({ label: "Remove…", onSelect: vendor("remove") });
  }
  const facts = [
    entry.marketplace && entry.label !== entry.key ? entry.marketplace : null,
    entry.version,
    entry.kind,
    entry.scope && entry.scope !== "user" ? entry.scope : null,
    entry.local_fields.length
      ? `Keeps ${entry.local_fields.length === 1 ? "1 field" : `${entry.local_fields.length} fields`} on this device`
      : null,
  ].filter(Boolean);
  return (
    <li className="plugins-row">
      <div className="plugins-row-text">
        <strong>{entry.label}</strong>
        {entry.description || facts.length ? (
          <small>{[entry.description, ...facts].filter(Boolean).join(" · ")}</small>
        ) : null}
      </div>
      {entry.installed && entry.enabled !== null ? (
        <span className="plugins-row-state" data-enabled={entry.enabled}>
          {entry.enabled ? <StatusDot tone="green" /> : <span className="plugins-row-off" />}
          {entry.enabled ? "Enabled" : "Disabled"}
        </span>
      ) : null}
      {!entry.installed &&
      provider &&
      provider !== "grok" &&
      kind &&
      agent.installed &&
      capability?.actions.includes("install") ? (
        <button
          type="button"
          className="g-button g-button-outline g-button-28"
          aria-label={`Install ${entry.label}`}
          onClick={() => onVendor({ provider, kind, action: "install", target: entry.key })}
        >
          Install
        </button>
      ) : null}
      {/* Grok installs from a source, never a marketplace entry's name: its own CLI does. */}
      {!entry.installed && provider === "grok" && agent.installed ? (
        <button
          type="button"
          className="g-button g-button-outline g-button-28"
          onClick={() => onVendor({ provider, kind: "native", action: "open", target: "settings" })}
        >
          Install<span className="sr-only"> {entry.label}</span> in Grok…
        </button>
      ) : null}
      {items.length ? (
        <RowMenu label={`${entry.label} actions`} items={items} />
      ) : (
        <span className="plugins-row-spacer" aria-hidden="true" />
      )}
    </li>
  );
}
