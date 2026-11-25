/** Synthetic UI harness. This module is loaded only by the development preview entry point. */
import { StrictMode, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { AgentEntry, ProjectSummary, SetupSnapshot } from "../bindings";
import { ProjectDialog } from "../components/chat/ProjectDialog";
import { AgentsScreen } from "../components/setup/AgentsScreen";
import { Onboarding } from "../components/setup/Onboarding";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
} from "../lib/appearance";
import { AppStore } from "../lib/chat/appStore";
import { chatSummary, FakeTransport } from "../lib/chat/testing";
import { MobileStore } from "../lib/mobile";
import { DEFAULT_GENERAL, type SetupApi } from "../lib/setup";
import { InstallApiContext } from "../lib/setup-actions/api";
import { WorkspaceStore } from "../lib/workspace";
import { createAccountsFixture } from "./accountsFixture";
import { cloneFixture } from "./cloneFixture";
import { fixtureFolderMatches } from "./folderMatches";
import { installFixture } from "./installFixture";
import { FakeMobile, mobileStatus } from "./mobileFixture";
import { remoteFixture } from "./remoteFixture";

const install = installFixture();

const params = new URLSearchParams(location.search);
const missing = params.has("missing");
// `?broken` shows Cursor found but unusable, so the calm failure state can be seen.
const broken = params.has("broken");
const agents: AgentEntry[] = [
  "hermes",
  "claude",
  "codex",
  "antigravity",
  "cursor",
  "grok",
  "omp",
  "opencode",
  "pi",
].map((id, index) => ({
  id,
  name:
    [
      "Hermes",
      "Claude Code",
      "Codex",
      "Antigravity",
      "Cursor",
      "Grok Build",
      "oh-my-pi",
      "OpenCode",
      "Pi",
    ][index] ?? id,
  description: "Synthetic detection fixture. No vendor command can run here.",
  availability:
    broken && id === "cursor"
      ? { state: "broken", reason: "The cursor-agent found here did not report a version." }
      : missing || !["claude", "codex", "pi"].includes(id)
        ? { state: "missing", reason: "Not installed in this fixture." }
        : {
            state: "ready",
            version:
              ({ claude: "2.1.310", codex: "0.162.1", pi: "0.70.0" } as Record<string, string>)[
                id
              ] ?? "1.0.0",
            path: `C:/fixture/${id}`,
          },
  launch: id,
  needs_download: false,
  last_verified: "2026-10-26",
  // Claude Code has a newer release: the Agents page's calm update line and its Update action.
  harness:
    id === "claude" && !missing
      ? {
          state: "update_available" as const,
          version: "2.1.310",
          latest: "2.1.314",
          source: "npm latest",
          checked_at: null,
        }
      : null,
}));
// What a version probe found for each installed agent, so the details show where it runs from,
// how it was installed and, for Codex, its latest release.
const PACKAGES: Readonly<Record<string, string>> = {
  claude: "@anthropic-ai/claude-code",
  codex: "@openai/codex",
  pi: "@earendil-works/pi-coding-agent",
};
for (const agent of agents) {
  const name = PACKAGES[agent.id];
  if (!name || agent.availability.state !== "ready") continue;
  const version = agent.availability.version ?? "";
  agent.installation = {
    agent_id: agent.id,
    provider: agent.id,
    host: { type: "native" },
    launch_path: `C:\\Users\\fixture\\AppData\\Roaming\\npm\\${agent.id}.cmd`,
    resolved_path: null,
    version,
    channel: { type: "npm", package: name },
    verification: { state: "verified", version, scope: "fixture" },
  };
  if (agent.id === "codex")
    agent.latest_release = { version, source: "npm latest", checked_at: null, stale: true };
}
// Claude Code and Codex each run directly or through their ACP adapter: one entry each, with the
// Connection setting in their details (Automatic picks the direct connection here).
const ADAPTERS: Readonly<Record<string, [string, string]>> = {
  claude: ["claude-acp", "@agentclientprotocol/claude-agent-acp 0.95.0 with your claude"],
  codex: ["codex-acp", "@agentclientprotocol/codex-acp 2.2.2 with your codex"],
};
for (const agent of agents) {
  const adapter = ADAPTERS[agent.id];
  if (!adapter) continue;
  agent.connections = {
    preferred: null,
    active: agent.availability.state === "ready" ? agent.id : null,
    options: [
      {
        id: agent.id,
        label: "Direct",
        description: `Your ${agent.name}, driven directly.`,
        availability: agent.availability,
        launch: agent.id,
        needs_download: false,
      },
      {
        id: adapter[0],
        label: "ACP adapter",
        description: `Your ${agent.name}, driven through its ACP adapter.`,
        availability: agent.availability,
        launch: adapter[1],
        needs_download: true,
      },
    ],
  };
}
/** Rust's answer to a Connection change: new sessions use the chosen connection, or Automatic's. */
const setConnection = (agentId: string, connection: string | null) => {
  const agent = agents.find((entry) => entry.id === agentId);
  if (!agent?.connections) throw new Error("This agent has one connection.");
  const runs = agent.availability.state === "ready";
  const chosen = connection ?? agent.id;
  const option = agent.connections.options.find((entry) => entry.id === chosen);
  agent.connections = {
    ...agent.connections,
    preferred: connection,
    active: runs ? chosen : null,
  };
  agent.needs_download = option?.needs_download ?? false;
  agent.launch = option?.launch ?? agent.launch;
};
// `?detect` resolves one agent per detection poll, so rows can be seen settling one by one.
let resolved = params.has("detect") ? 0 : agents.length;
const detected = (): AgentEntry[] =>
  agents.map((agent, index) =>
    index < resolved ? agent : { ...agent, availability: { state: "checking" } },
  );
/** What the fixture's updater "printed" last. */
let printed: string | null = null;
const transport = new FakeTransport({
  // Claude Code's details list one added account; nothing here stores or reads a token.
  accounts: createAccountsFixture(["Work"]),
  agentsList: async () => {
    const value = detected();
    resolved = Math.min(agents.length, resolved + 1);
    return value;
  },
  agentsRefresh: async () => {
    resolved = agents.length;
    return agents;
  },
  agentSetConnection: async (agentId, connection) => {
    setConnection(agentId, connection);
    return detected();
  },
  // Rust can update Claude Code's npm install, so its newer release has an Update action; the
  // others say why not, in their details. Reading this runs no vendor command.
  agentMaintenance: async (agentId) => ({
    auth_status: false,
    login_available: false,
    update_unavailable:
      agentId === "claude" ? null : "Synthetic fixture: no vendor command can run here.",
  }),
  // `?update=fail`: the vendor's command fails and leaves output behind. Otherwise Update finds
  // the newer release installed. Nothing here runs a command.
  agentUpdate: async (agentId) => {
    await new Promise((done) => setTimeout(done, 300));
    if (params.get("update") === "fail") {
      printed = [
        "npm error code EBUSY",
        "npm error syscall rename",
        "npm error path C:/fixture/claude.exe",
        "npm error errno -4082",
        "npm error EBUSY: resource busy or locked",
      ].join("\n");
      throw new Error("The vendor's command failed with exit code 1.");
    }
    printed = "changed 1 package in 4s";
    const claude = agents.find((agent) => agent.id === agentId);
    if (claude?.availability.state === "ready") {
      claude.availability = { ...claude.availability, version: "2.1.314" };
      if (claude.installation) claude.installation = { ...claude.installation, version: "2.1.314" };
      claude.harness = { state: "up_to_date", version: "2.1.314", checked_at: Date.now() };
    }
  },
  agentUpdateOutput: async () => printed,
  // `?working=2`: two of Claude Code's chats have a turn under way, so its Update waits.
  chatsList: async () =>
    Array.from({ length: Number(params.get("working") ?? 0) }, (_, index) =>
      chatSummary(`working-${index}`, {
        title: `Working chat ${index + 1}`,
        agent_id: "claude",
        agent_name: "Claude Code",
        status: "running",
      }),
    ),
  pickFolder: async () => "C:/fixture/Orbit",
});
// The phone page: Android not set up yet (`&phone=ready` has it set up). `window.deviceFixture`
// records what the page asked for.
const devices = (() => {
  const base = mobileStatus();
  return new FakeMobile(
    params.get("phone") === "ready"
      ? base
      : mobileStatus({ setup: { ...base.setup, adb: false, emulator: false, acceleration: null } }),
  );
})();
(window as { deviceFixture?: FakeMobile }).deviceFixture = devices;
Object.assign(transport, { mobile: new MobileStore(devices) });
const app = new AppStore(transport);
if (params.has("working")) void app.reloadChats();
// `?preview=clone`, `clone-running` and `clone-error` open the first-project step on the clone,
// filled in, and started for the last two; `setup` clones to completion when asked.
const mode = params.get("preview") ?? "setup";
const cloneMode = mode.startsWith("clone");
const cloneCalls: string[] = [];
const clone = {
  api: cloneFixture(
    mode === "clone-running" ? "running" : mode === "clone-error" ? "error" : "ready",
    cloneCalls,
  ),
  draft: cloneMode
    ? {
        remote: "https://github.com/auracode/orbit",
        parent: "C:\\Users\\fixture\\code",
        start: mode !== "clone",
      }
    : undefined,
};
const initial: SetupSnapshot = {
  cold_start: !cloneMode,
  notice: null,
  preferences: {
    revision: 0,
    step: cloneMode ? "project" : "welcome",
    motion: "full",
    appearance: getAppearancePreference(),
    theme: getThemePreference(),
    text_size: getTextSizePreference(),
    completed: false,
    first_project_id: null,
    tour_seen: [],
    general: { ...DEFAULT_GENERAL },
  },
};
// Each clone preview starts from its own saved setup, apart from the walk-through's.
const storageKey = cloneMode ? `setup-fixture-${mode}` : "setup-fixture";
let stored: SetupSnapshot = JSON.parse(
  sessionStorage.getItem(storageKey) ?? JSON.stringify(initial),
);
stored.preferences.appearance = getAppearancePreference();
const persist = () => sessionStorage.setItem(storageKey, JSON.stringify(stored));
/** Remote access for the page that links an assistant: nothing connects anywhere. */
const remote = remoteFixture();

const api: SetupApi = {
  snapshot: async () => {
    const value = structuredClone(stored);
    stored.cold_start = false;
    persist();
    return value;
  },
  apply: async (revision, action) => {
    if (sessionStorage.getItem("fail-setup") === "yes")
      throw new Error("Synthetic storage is full. Setup was not saved.");
    if (revision !== stored.preferences.revision) throw new Error("Setup changed in another view.");
    const next = structuredClone(stored);
    if (action.type === "visit") next.preferences.step = action.step;
    if (action.type === "motion") next.preferences.motion = action.value;
    if (action.type === "appearance") next.preferences.appearance = action.value;
    if (action.type === "theme") next.preferences.theme = action.value;
    if (action.type === "finish") next.preferences.completed = true;
    if (action.type === "project") next.preferences.first_project_id = action.project_id;
    if (action.type === "restart") {
      next.preferences.completed = false;
      next.preferences.step = "welcome";
    }
    next.preferences.revision++;
    stored = next;
    persist();
    return structuredClone(next);
  },
};
const projects: ProjectSummary[] = [];
const unsupported = async (): Promise<never> => {
  throw new Error("Unavailable in this UI fixture.");
};
const workspace = new WorkspaceStore({
  snapshot: async () => ({ projects: [...projects], chats: [] }),
  folderMatches: fixtureFolderMatches(() => ({ projects, chats: [] })),
  createProject: async ({ project, folder }) => {
    const value: ProjectSummary = {
      ...project,
      id: `project-${projects.length}`,
      created_at: 0,
      updated_at: 0,
      folders: [
        {
          id: "folder",
          project_id: `project-${projects.length}`,
          display_name: project.name,
          path: folder,
          folder_state: "ok",
          unavailable_reason: null,
          kind: "local",
          git_remote_url: null,
          git_default_branch: null,
        },
      ],
    };
    projects.push(value);
    return value;
  },
  updateProject: unsupported,
  addFolder: unsupported,
  rename: unsupported,
  pin: unsupported,
  setRole: unsupported,
  archive: unsupported,
  move: unsupported,
  preview: unsupported,
  createChat: unsupported,
});
Object.assign(window, {
  setupFixture: {
    methods: () => transport.methods(),
    projects: () => projects.length,
    folders: () => projects.map((project) => project.folders[0]?.path),
    cloneCalls: () => [...cloneCalls],
  },
});

function Preview() {
  const demo = useRef(false);
  const [view, setView] = useState(
    mode === "agents" || mode === "agents-installing"
      ? "agents"
      : mode === "new-project"
        ? "new-project"
        : "setup",
  );
  const [restart, setRestart] = useState(false);
  if (view === "new-project")
    return (
      <main className="setup-shell g-root">
        <ProjectDialog
          project={null}
          store={workspace}
          transport={transport}
          agents={agents}
          cloneApi={clone.api}
          onClose={() => setView("done")}
        />
      </main>
    );
  if (view === "agents")
    return (
      <div className="pane-shell g-root">
        <AgentsScreen
          app={app}
          transport={transport}
          onBack={() => setView("done")}
          onSetup={() => {
            setRestart(true);
            setView("setup");
          }}
        />
      </div>
    );
  if (view === "done")
    return (
      <main className="setup-shell">
        <h1>Workspace ready</h1>
        <button type="button" onClick={() => setView("agents")}>
          Agents
        </button>
      </main>
    );
  if (view === "demo")
    return (
      <main className="setup-shell">
        <h1>Local demo</h1>
        <p>Setup finished. In the app, New chat opens with the Demo agent chosen.</p>
      </main>
    );
  return (
    <Onboarding
      api={api}
      remote={remote}
      app={app}
      transport={transport}
      workspace={workspace}
      restart={restart}
      clone={clone}
      // The demo finishes setup like Skip, so its view follows setup's own end.
      onDone={() => setView(demo.current ? "demo" : "done")}
      onDemo={() => {
        demo.current = true;
      }}
    />
  );
}
/** Waits for a button by its accessible name, as a person would find it, then presses it. */
async function press(name: string) {
  for (let attempt = 0; attempt < 100; attempt++) {
    const button = [...document.querySelectorAll("button")].find(
      (node) => (node.getAttribute("aria-label") ?? node.textContent?.trim()) === name,
    );
    if (button && !button.disabled) {
      button.click();
      return;
    }
    await new Promise((done) => setTimeout(done, 50));
  }
}

export function mountSetupPreview(container: HTMLElement): void {
  createRoot(container).render(
    <StrictMode>
      <InstallApiContext.Provider value={install}>
        <Preview />
      </InstallApiContext.Provider>
    </StrictMode>,
  );
  // `?preview=agents-installing`: Grok Build's reviewed install, started, with npm at work.
  if (mode === "agents-installing")
    void press("Install Grok Build").then(() => press("Start installation"));
}
