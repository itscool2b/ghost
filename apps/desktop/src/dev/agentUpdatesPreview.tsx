/**
 * Agent update notices over the real chat screen (`?preview=agent-updates`). Nothing is installed:
 * the agents, their releases and the vendor update are synthetic.
 *
 * - Claude Code has a newer release and can update; Update succeeds and detection then finds it
 *   up to date. `&case=fail` makes the update fail once; `&case=unchanged` finishes it while
 *   detection still finds the old version (another copy first on PATH). After a failure, Run in
 *   terminal plays the vendor's output in the terminal sheet and the update then passes.
 * - Codex has a newer release that another tool manages, so it points to Settings > Agents.
 * - Cursor has a newer release while one of its chats works, so its Update waits.
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { AgentEntry, AppInfo, HarnessStatus } from "../bindings";
import { ChatsScreen } from "../components/chat/ChatsScreen";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
} from "../lib/appearance";
import { AppStore } from "../lib/chat/appStore";
import { chatSummary, FakeTransport } from "../lib/chat/testing";
import { DEFAULT_GENERAL, type SetupApi } from "../lib/setup";
import type { TerminalTransport } from "../lib/terminal";
import { fakeUpdates } from "./appUpdateFixture";

const entry = (id: string, name: string, harness: HarnessStatus): AgentEntry =>
  ({
    id,
    name,
    description: name,
    availability: {
      state: "ready",
      version: harness.state === "update_available" ? harness.version : "1.0.0",
      path: `C:\\bin\\${id}.exe`,
    },
    launch: id,
    needs_download: false,
    last_verified: "2026-11-01",
    installation: {
      launch_path: `C:\\Users\\Example\\AppData\\Roaming\\npm\\${id}.cmd`,
      version: "x",
    },
    latest_release: null,
    harness,
    connections: null,
  }) as unknown as AgentEntry;

const newer = (version: string, latest: string): HarnessStatus => ({
  state: "update_available",
  version,
  latest,
  source: "npm",
  checked_at: Date.now(),
});

const info: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "windows",
  os_version: "11",
  arch: "x86_64",
  data_dir: "C:/fixture/auracode",
  device: null,
  profile: { id: "local", kind: "local", display_name: "Local", created_at: 1 },
  profile_switching: false,
  db: null,
  glass: "none",
  launch: { type: "normal" },
  notices: [],
  recovery_notice: null,
  fatal: null,
  media: true,
  media_notice: null,
};

const unavailable = async (): Promise<never> => {
  throw new Error("This fixture only shows update notices.");
};

const setup: SetupApi = {
  snapshot: async () => ({
    cold_start: false,
    notice: null,
    preferences: {
      revision: 1,
      step: "project",
      motion: "off",
      appearance: getAppearancePreference(),
      theme: getThemePreference(),
      text_size: getTextSizePreference(),
      completed: true,
      first_project_id: null,
      tour_seen: [],
      general: { ...DEFAULT_GENERAL },
    },
  }),
  apply: unavailable,
};

export function mountAgentUpdatesPreview(container: HTMLElement): void {
  const kind = new URLSearchParams(location.search).get("case");
  let claude: HarnessStatus = newer("2.1.310", "2.1.400");
  let failures = kind === "fail" ? 1 : 0;
  const chats = [
    chatSummary("cursor-working", {
      title: "Port the settings page",
      agent_id: "cursor",
      agent_name: "Cursor",
      status: "running",
    }),
    chatSummary("claude-idle", {
      title: "Tidy the changelog check",
      agent_id: "claude",
      agent_name: "Claude Code",
      status: "stopped",
    }),
  ];
  const agents = () => [
    entry("claude", "Claude Code", claude),
    entry("codex", "Codex", newer("0.40.0", "0.41.2")),
    entry("cursor", "Cursor", newer("1.4.0", "1.5.0")),
    entry("pi", "Pi", { state: "up_to_date", version: "0.9.0", checked_at: Date.now() }),
  ];
  const transport = new FakeTransport({
    chatsList: async () => structuredClone(chats),
    agentsList: async () => agents(),
    agentsRefresh: async () => agents(),
    agentMaintenance: async (agentId) => ({
      auth_status: false,
      login_available: false,
      update_unavailable:
        agentId === "codex"
          ? "Homebrew manages this installation of Codex. Update it with brew upgrade codex."
          : null,
    }),
    agentUpdate: async (agentId) => {
      await new Promise((done) => setTimeout(done, 400));
      if (failures > 0) {
        failures -= 1;
        throw new Error("npm could not replace claude.exe: it is in use.");
      }
      if (agentId === "claude" && kind !== "unchanged")
        claude = { state: "up_to_date", version: "2.1.400", checked_at: Date.now() };
    },
    send: unavailable,
    chatCreate: unavailable,
    start: unavailable,
  });
  // The vendor's update, played in the terminal sheet: its output, then a passing exit.
  let played = false;
  const updateTerminal: TerminalTransport = {
    reserve: async () => "update-fixture",
    start: async (id) => id,
    read: async () => {
      if (played) return { bytes: [], done: true, exit_code: 0, truncated: false };
      played = true;
      await new Promise((done) => setTimeout(done, 300));
      claude = { state: "up_to_date", version: "2.1.400", checked_at: Date.now() };
      return {
        bytes: Array.from(
          new TextEncoder().encode(
            "Current version: 2.1.310\r\nChecking for updates...\r\nUpdating to 2.1.400...\r\nSuccessfully updated to 2.1.400\r\n",
          ),
        ),
        done: false,
        exit_code: null,
        truncated: false,
      };
    },
    write: async () => {},
    resize: async () => {},
    close: async () => {},
  };
  transport.updateTerminal = updateTerminal;
  const app = new AppStore(transport);
  // `&app-update=<case>`: the app itself has a newer version too (appUpdateFixture.ts).
  const appUpdate = new URLSearchParams(location.search).get("app-update");
  createRoot(container).render(
    <StrictMode>
      <ChatsScreen
        app={app}
        transport={transport}
        info={info}
        setup={setup}
        updates={appUpdate ? fakeUpdates(appUpdate) : undefined}
        onDiagnostics={() => {}}
        onRecoveryDismissed={() => {}}
        onAgents={() => {
          // The checks read that Settings > Agents was asked for.
          document.body.dataset.openedAgents = "";
        }}
      />
    </StrictMode>,
  );
}
