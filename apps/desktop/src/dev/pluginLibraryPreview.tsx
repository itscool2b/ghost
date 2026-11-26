/**
 * Development-only Plugins & MCP fixtures. They read no native configuration, run no agent and
 * no CLI. Modes: `plugins` (Claude Code's live listings), `plugins-pi` (an agent without a plugin
 * system), `plugins-update` (an update is available and the version changed), `plugins-failed`
 * (Codex's CLI listing failed and its files answer instead).
 */
import "../styles.css";
import "../ui-tokens.css";
import "../chat.css";
import "../shell.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type {
  AgentPlugins,
  FeatureListing,
  McpDefinition,
  McpSnapshot,
  PluginEntry,
  PluginLibrary,
  PluginManagement,
} from "../bindings";
import { PluginLibraryScreen } from "../components/library/PluginLibraryScreen";
import type { LibraryApi } from "../lib/library/api";
import type { LibraryItem } from "../lib/library/bindings";
import type { PluginsApi } from "../lib/plugins/api";
import type { SharedMcpApi } from "../lib/sharedMcp";
import { syntheticVendor } from "./vendorFixture";

declare global {
  interface Window {
    pluginsFixture: {
      calls: string[];
      finishProvider?: (agentId: string) => void;
      finish?: () => void;
      supplyProjects?: () => void;
      unmount?: () => void;
    };
  }
}

const NOW = Date.UTC(2026, 9, 28, 12, 0);

const entry = (key: string, extra: Partial<PluginEntry> = {}): PluginEntry => ({
  key,
  label: key.split("@")[0] ?? key,
  description: null,
  version: null,
  marketplace: key.includes("@") ? (key.split("@")[1] ?? null) : null,
  scope: "user",
  enabled: null,
  installed: true,
  kind: null,
  local_fields: [],
  library: null,
  ...extra,
});

const files = (
  feature: FeatureListing["feature"],
  reason: string,
  entries: PluginEntry[],
): FeatureListing => ({
  feature,
  state: { state: "files", reason },
  command: null,
  entries,
  available: [],
  available_total: 0,
});

const SKILLS = "From the agent's skills folder.";
const FOLDER = "From the agent's folder.";
function recordedManagement(provider: "claude" | "codex" | "pi"): PluginManagement {
  return {
    version: 1,
    provider,
    actions:
      provider === "pi"
        ? [{ feature: "packages", kind: "package", actions: ["install", "update", "remove"] }]
        : [
            {
              feature: "plugins",
              kind: "plugin",
              actions:
                provider === "claude"
                  ? ["install", "update", "remove", "enable", "disable"]
                  : ["install", "remove"],
            },
            {
              feature: "marketplaces",
              kind: "marketplace",
              actions: ["install", "update", "remove"],
            },
          ],
    native_routes: [
      {
        id: "settings",
        label: "Open native customization",
        instructions: "Synthetic native route. No process is started.",
      },
    ],
  };
}

export function pluginFixture(mode: string): PluginLibrary {
  const available = [
    ["frontend-design", "Frontend design skill for UI and UX work"],
    ["security-review", "Reviews a change for security issues before merge"],
    ["release-notes", "Drafts release notes from merged pull requests"],
  ].map(([name, description]) =>
    entry(`${name}@anthropic-plugin-directory`, {
      description: description ?? null,
      installed: false,
      version: "1.2.0",
      kind: "git-subdir",
    }),
  );
  const claude: AgentPlugins = {
    management: recordedManagement("claude"),
    agent_id: "claude",
    name: "Claude Code",
    installed: true,
    version: "2.1.312",
    system: "marketplace",
    summary:
      "Plugins from marketplaces, plus skills, MCP servers, slash commands, subagents and hooks.",
    harness:
      mode === "plugins-update"
        ? {
            state: "update_available",
            version: "2.1.312",
            latest: "2.1.314",
            source: "npm latest",
            checked_at: NOW - 3_600_000,
          }
        : { state: "up_to_date", version: "2.1.312", checked_at: NOW - 3_600_000 },
    updated_at: NOW - 4 * 60_000,
    version_change: mode === "plugins-update" ? { from: "2.1.310", to: "2.1.312" } : null,
    notices: [],
    features: [
      {
        feature: "plugins",
        state: { state: "live" },
        command: "claude plugin list --json --available",
        entries: [
          entry("tools@team", { version: "1.0.0", enabled: true }),
          entry("review-kit@team", { version: "0.4.1", enabled: false }),
        ],
        available,
        available_total: 3599,
      },
      {
        feature: "marketplaces",
        state: { state: "live" },
        command: "claude plugin marketplace list --json",
        entries: [entry("team", { description: "owner/plugins", kind: "github" })],
        available: [],
        available_total: 0,
      },
      files("skills", SKILLS, [
        entry("review-changes", {
          library: {
            source_id: "user:claude",
            kind: "skill",
            key: "review-changes",
            revision: "synthetic",
          },
        }),
      ]),
      files(
        "mcp_servers",
        "`claude mcp list` connects to every server to check it, so these come from ~/.claude.json.",
        [
          entry("docs", { kind: "http" }),
          entry("github", {
            kind: "stdio",
            library: {
              source_id: "user:claude",
              kind: "mcp_server",
              key: "github",
              revision: "synthetic-mcp",
            },
          }),
        ],
      ),
      files("commands", FOLDER, [entry("ship"), entry("team:review")]),
      files("subagents", FOLDER, [entry("reviewer")]),
      files("hooks", FOLDER, [entry("PreToolUse")]),
    ],
  };
  const codex: AgentPlugins = {
    management: recordedManagement("codex"),
    agent_id: "codex",
    name: "Codex",
    installed: true,
    version: "0.162.1",
    system: "marketplace",
    summary: "Plugins from marketplaces, plus MCP servers, skills, subagents and hooks.",
    harness: {
      state: "unknown",
      version: "0.162.1",
      reason: "GitHub stable release did not answer. You may be offline; Ghost checks again later.",
    },
    updated_at: NOW - 60_000,
    version_change: null,
    notices: [],
    features: [
      {
        feature: "plugins",
        state: {
          state: "failed",
          reason: "`codex plugin list --json --available` failed: marketplace cache is corrupt.",
        },
        command: "codex plugin list --json --available",
        entries: [entry("tools@local-team", { kind: null })],
        available: [],
        available_total: 0,
      },
      {
        feature: "marketplaces",
        state: { state: "live" },
        command: "codex plugin marketplace list --json",
        entries: [entry("local-team", { kind: "local" })],
        available: [],
        available_total: 0,
      },
      {
        feature: "mcp_servers",
        state: { state: "live" },
        command: "codex mcp list --json",
        entries: [
          entry("private", {
            kind: "stdio",
            description: "node",
            enabled: true,
            local_fields: ["env/TOKEN"],
          }),
        ],
        available: [],
        available_total: 0,
      },
      files("skills", SKILLS, []),
      files("subagents", FOLDER, []),
      files("hooks", FOLDER, [entry("SessionStart")]),
    ],
  };
  const pi: AgentPlugins = {
    management: recordedManagement("pi"),
    agent_id: "pi",
    name: "Pi",
    installed: true,
    version: "1.0.2",
    system: "packages",
    summary:
      "Pi has no plugin marketplace. It installs packages from npm or git, and reads skills, MCP servers and prompt templates from its folder.",
    harness: { state: "up_to_date", version: "1.0.2", checked_at: NOW - 7_200_000 },
    updated_at: NOW - 2 * 60_000,
    version_change: null,
    notices: [],
    features: [
      {
        feature: "packages",
        state: { state: "live" },
        command: "pi list --no-approve",
        entries: [
          entry("npm:pi-tools@1.0.0", { label: "npm:pi-tools@1.0.0", kind: "npm" }),
          entry("git:github.com/team/pi-ext@v1.2.0", {
            label: "git:github.com/team/pi-ext@v1.2.0",
            marketplace: null,
            kind: "git",
            description: "Some resources are filtered",
          }),
        ],
        available: [],
        available_total: 0,
      },
      files("skills", SKILLS, [entry("release-check")]),
      files(
        "mcp_servers",
        "`pi mcp list` starts every server to check it, so these come from mcp.json.",
        [],
      ),
      files("commands", FOLDER, [entry("release")]),
    ],
  };
  const opencode: AgentPlugins = {
    management: null,
    agent_id: "opencode",
    name: "OpenCode",
    installed: false,
    version: null,
    system: "declared",
    summary:
      "OpenCode loads npm plugins named in its config and local plugin files, plus MCP servers, skills, commands and agents.",
    harness: { state: "not_installed" },
    updated_at: null,
    version_change: null,
    notices: [],
    features: [
      files(
        "plugins",
        "OpenCode installs its plugins and checks its servers whenever it runs, so Ghost reads ~/.config/opencode instead.",
        [],
      ),
    ],
  };
  const nativeAgent = (
    agent_id: string,
    name: string,
    reviewed: boolean,
    management: PluginManagement | null = null,
  ): AgentPlugins => ({
    agent_id,
    name,
    installed: management !== null,
    version: management ? "0.5.0" : null,
    management,
    system: "declared",
    summary:
      "Native customization capabilities vary by agent and installed version. Reviewed file inventory and named native routes are shown where available.",
    harness: management
      ? { state: "unknown", version: "0.5.0", reason: "Synthetic installed-agent fixture." }
      : { state: "not_installed" },
    updated_at: null,
    version_change: null,
    notices: [],
    features: [
      ...(reviewed
        ? [
            files(
              "mcp_servers",
              "Read from reviewed native configuration paths. This inventory does not start the agent or connect servers.",
              management ? [entry("team_docs", { kind: "stdio" })] : [],
            ),
            files("skills", "Read from the agent's native skills folder.", []),
          ]
        : []),
      {
        feature: "plugins",
        state: {
          state: "unavailable",
          reason:
            "This agent has no qualified structured plugin-management interface in Ghost. Use its native customization where available.",
        },
        command: null,
        entries: [],
        available: [],
        available_total: 0,
      },
    ],
  });
  // Grok Build and Hermes list their own plugins and marketplaces (Hermes: skill sources),
  // with reviewed actions, as Rust assembles them since November 2026.
  const listed = (
    agent_id: "grok" | "hermes",
    name: string,
    version: string,
    features: FeatureListing[],
  ): AgentPlugins => ({
    agent_id,
    name,
    installed: true,
    version,
    management: {
      version: 1,
      provider: agent_id,
      actions: [
        {
          feature: "plugins",
          kind: "plugin",
          actions: ["install", "update", "remove", "enable", "disable"],
        },
        {
          feature: "marketplaces",
          kind: "marketplace",
          actions: agent_id === "grok" ? ["install", "update", "remove"] : ["install", "remove"],
        },
      ],
      native_routes: [
        {
          id: "settings",
          label: "Open native customization",
          instructions: "Synthetic native route. No process is started.",
        },
      ],
    },
    system: "marketplace",
    summary:
      agent_id === "grok"
        ? "Plugins from marketplace sources, plus skills and MCP servers."
        : "Plugins from Hermes's catalog or GitHub, skill sources (taps), skills and MCP servers.",
    harness: { state: "up_to_date", version, checked_at: NOW - 60_000 },
    updated_at: NOW - 60_000,
    version_change: null,
    notices: [],
    features,
  });
  const grok = listed("grok", "Grok Build", "1.0.70", [
    {
      feature: "plugins",
      state: { state: "live" },
      command: "grok plugin list --json --available",
      entries: [entry("tools", { marketplace: "team", version: "1.2.0", enabled: true })],
      available: [
        entry("vercel", {
          installed: false,
          marketplace: "xAI Official",
          description: "Vercel deployment platform integration.",
        }),
        entry("supabase", {
          installed: false,
          marketplace: "xAI Official",
          description: "Supabase database, auth and storage.",
        }),
      ],
      available_total: 351,
    },
    {
      feature: "marketplaces",
      state: { state: "live" },
      command: "grok plugin marketplace list --json",
      entries: [
        entry("xAI Official", { kind: "git", description: "xai-org/plugin-marketplace" }),
        entry("claude-plugins-official", {
          kind: "git",
          description: "anthropics/claude-plugins-official",
        }),
      ],
      available: [],
      available_total: 0,
    },
    files("skills", "Read from the agent's native skills folder.", []),
  ]);
  const hermes = listed("hermes", "Hermes", "0.22.5", [
    {
      feature: "plugins",
      state: { state: "live" },
      command: "hermes plugins list --json",
      entries: [
        entry("browser-firecrawl", {
          version: "1.0.0",
          enabled: true,
          kind: "bundled",
          description: "Firecrawl cloud browser backend.",
        }),
      ],
      available: [],
      available_total: 0,
    },
    files("marketplaces", "Hermes skill sources (taps), read from skills/.hub/taps.json.", [
      entry("team/skills"),
    ]),
  ]);
  return {
    agents: [
      claude,
      codex,
      pi,
      opencode,
      hermes,
      grok,
      nativeAgent("cursor", "Cursor", true),
      nativeAgent("antigravity", "Antigravity", false),
      nativeAgent("omp", "oh-my-pi", false),
    ],
    notices: [],
  };
}

export function mountPluginLibraryPreview(container: HTMLElement, mode: string) {
  window.pluginsFixture = { calls: [] };
  const calls = window.pluginsFixture.calls;
  let mcp: McpSnapshot = {
    revision: 1,
    servers: [
      {
        definition: {
          id: "6a83dedb-b8d1-4a0d-98d2-7f058ac6f737",
          name: "project_docs",
          project_id: "7cc769fb-ae40-44c1-8baa-fc2210f3bebe",
          enabled: true,
          transport: { type: "http", url: "https://mcp.example.test" },
          env_keys: [],
          header_keys: ["Authorization"],
        },
        revision: 1,
        missing_credentials: ["headers/Authorization"],
      },
    ],
  };
  function mutate(expected: number, work: () => void) {
    if (expected !== mcp.revision)
      throw new Error("Shared MCP changed elsewhere. Refresh and review.");
    work();
    mcp = { ...mcp, revision: mcp.revision + 1 };
    return structuredClone(mcp);
  }
  const mcpApi: SharedMcpApi = {
    snapshot: async () => structuredClone(mcp),
    save: async (expected, definition) =>
      mutate(expected, () => {
        calls.push(`mcp:save:${definition.name}`);
        const at = mcp.servers.findIndex(
          (s) =>
            s.definition.id === definition.id && s.definition.project_id === definition.project_id,
        );
        const record = {
          definition,
          revision: at < 0 ? 1 : (mcp.servers[at]?.revision ?? 0) + 1,
          missing_credentials: definition.header_keys
            .map((k) => `headers/${k}`)
            .concat(definition.env_keys.map((k) => `env/${k}`)),
        };
        if (at < 0) mcp.servers.push(record);
        else mcp.servers[at] = record;
      }),
    remove: async (expected, definition) =>
      mutate(expected, () => {
        mcp.servers = mcp.servers.filter(
          (s) =>
            s.definition.id !== definition.id || s.definition.project_id !== definition.project_id,
        );
        calls.push("mcp:remove");
      }),
    credentials: async (expected, definition, credentials) =>
      mutate(expected, () => {
        calls.push("mcp:credentials");
        const row = mcp.servers.find((s) => s.definition.id === definition.id);
        if (row)
          row.missing_credentials = definition.header_keys
            .filter((k) => !credentials.headers[k])
            .map((k) => `headers/${k}`)
            .concat(definition.env_keys.filter((k) => !credentials.env[k]).map((k) => `env/${k}`));
      }),
    import: async (expected, text) =>
      mutate(expected, () => {
        const doc = JSON.parse(text) as { definitions: McpDefinition[] };
        mcp.servers = doc.definitions.map((definition) => ({
          definition,
          revision: 1,
          missing_credentials: [],
        }));
        calls.push("mcp:import");
      }),
    importNative: async (_expected, projectId) => {
      calls.push(`mcp:import-native:${projectId ?? "all"}`);
      throw new Error("Synthetic native import review. No real files are read.");
    },
    export: async () =>
      JSON.stringify({ version: 1, definitions: mcp.servers.map((s) => s.definition) }, null, 2),
    test: async () => {
      calls.push("mcp:test");
      return {
        state: "connected",
        message: "Synthetic MCP initialization succeeded. No native process was started.",
      };
    },
    sessionAction: async (_chat, name, action) => {
      calls.push(`mcp:session:${action}`);
      return {
        state: action === "restart" ? "restarting" : "observed",
        message: "Synthetic session response. No native process was started.",
        authorization_url: null,
        servers:
          action === "status"
            ? [{ name, state: "connected", authentication: "oauth", tool_count: 2 }]
            : [],
      };
    },
  };
  const complete = pluginFixture(mode);
  let library = complete;
  const pending: {
    update: (value: PluginLibrary) => void;
    resolve: (value: PluginLibrary) => void;
  }[] = [];
  if (mode === "plugins-progressive") {
    library = {
      ...complete,
      agents: complete.agents.map((agent) => ({
        ...agent,
        installed: agent.agent_id === "pi" ? false : agent.installed,
        harness: { state: "checking" },
        updated_at: null,
        features: agent.features.map((feature) =>
          feature.command
            ? {
                ...feature,
                command: null,
                state: {
                  state: "files",
                  reason: "Reading the agent listing. Cached and file entries are available.",
                },
              }
            : feature,
        ),
      })),
    };
    window.pluginsFixture.finishProvider = (agentId) => {
      library = {
        ...library,
        agents: library.agents.map((agent) =>
          agent.agent_id === agentId
            ? (complete.agents.find((entry) => entry.agent_id === agentId) ?? agent)
            : agent,
        ),
      };
      for (const request of pending) request.update(library);
    };
    window.pluginsFixture.finish = () => {
      for (const request of pending) request.resolve(library);
    };
  }
  const api: PluginsApi = {
    snapshot: async (refresh, onUpdate = () => {}) => {
      calls.push(`snapshot:${refresh}`);
      if (mode === "plugins-progressive") {
        onUpdate(library);
        return new Promise((resolve) => pending.push({ update: onUpdate, resolve }));
      }
      return library;
    },
    update: async (agentId) => {
      calls.push(`update:${agentId}`);
      library = {
        ...library,
        agents: library.agents.map((agent) =>
          agent.agent_id === agentId
            ? {
                ...agent,
                version: "2.1.314",
                harness: { state: "up_to_date", version: "2.1.314", checked_at: NOW },
                version_change: { from: "2.1.312", to: "2.1.314" },
              }
            : agent,
        ),
      };
    },
  };
  const item: LibraryItem = {
    item: { source_id: "user:claude", kind: "skill", key: "review-changes" },
    provider: "claude",
    scope: "user",
    label: "review-changes",
    relative_path: "skills/review-changes/SKILL.md",
    revision: "synthetic",
    bytes: 120,
    copy_blocked: null,
  };
  // Claude Code's own GitHub server: its token is in env, so it shares but does not copy.
  const server: LibraryItem = {
    item: { source_id: "user:claude", kind: "mcp_server", key: "github" },
    provider: "claude",
    scope: "user",
    label: "github",
    relative_path: ".claude.json",
    revision: "synthetic-mcp",
    bytes: 132,
    copy_blocked:
      "This server has authentication, environment, or vendor options that require manual setup. No values will be copied.",
  };
  const libraryApi: LibraryApi = {
    snapshot: async () => ({
      items: [item, server],
      sources: ["claude", "codex", "pi"].map((provider) => ({
        id: `user:${provider}`,
        provider: provider as "claude",
        scope: "user" as const,
        available: true,
        message: null,
      })),
      notices: [],
    }),
    history: async () => [],
    preview: async (_folder, requested) =>
      requested.key === "github"
        ? {
            item: server,
            text: '{\n  "command": "npx",\n  "args": ["-y", "@modelcontextprotocol/server-github"],\n  "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "(hidden)" }\n}',
            files: [".claude.json"],
            notices: [],
          }
        : {
            item,
            text: "# Review changes\n\nRead the surrounding code before suggesting a change.\n",
            files: ["SKILL.md"],
            notices: [],
          },
    prepare: async () => {
      throw new Error("Synthetic fixture: copies are not prepared here.");
    },
    cancel: async () => {},
    apply: async () => {
      throw new Error("Synthetic fixture.");
    },
    rollback: async () => {
      throw new Error("Synthetic fixture.");
    },
    share: async (_folder, shared) => {
      const calls = (window as { libraryShares?: string[] }).libraryShares ?? [];
      Object.assign(window, { libraryShares: [...calls, shared.key] });
      return { revision: 1, servers: [] } as unknown as Awaited<ReturnType<LibraryApi["share"]>>;
    },
  };
  const initial =
    mode === "plugins-mcp"
      ? "shared"
      : mode === "plugins-pi"
        ? "pi"
        : mode === "plugins-failed"
          ? "codex"
          : "claude";
  const root = createRoot(container);
  window.pluginsFixture.unmount = () => root.unmount();
  const delayedProjects = new URLSearchParams(window.location.search).has("delayedProjects");
  const fixtureProjects = [{ id: "7cc769fb-ae40-44c1-8baa-fc2210f3bebe", name: "Aura project" }];
  let projects = delayedProjects ? [] : fixtureProjects;
  window.pluginsFixture.supplyProjects = () => {
    projects = fixtureProjects;
    render();
  };
  const render = () =>
    root.render(
      <StrictMode>
        <div className="pane-shell g-root">
          <PluginLibraryScreen
            initialAgent={initial}
            api={api}
            libraryApi={libraryApi}
            vendorApi={{
              ...syntheticVendor,
              review: async (request) => {
                const review = await syntheticVendor.review(request);
                return request.kind === "native"
                  ? {
                      ...review,
                      program: `C:/fixture/${request.provider}.exe`,
                      arguments: request.provider === "hermes" ? ["chat"] : [],
                      config_directory: `C:/fixture/.${request.provider}`,
                    }
                  : review;
              },
            }}
            mcpApi={mcpApi}
            projects={projects}
            initialProjectId={delayedProjects ? null : fixtureProjects[0]?.id}
            activeChatId="synthetic-chat"
            now={() => NOW}
            onBack={() => calls.push("back")}
            onAgents={() => calls.push("agents")}
          />
        </div>
      </StrictMode>,
    );
  render();
}
