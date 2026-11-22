/** Development-only settings harness. No native process, updater or network request. */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  AgentEntry,
  AppInfo,
  InstructionsTarget,
  SetupSnapshot,
  UpdateStatus,
} from "../bindings";
import { type Page, SettingsScreen } from "../components/settings/SettingsScreen";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
  rememberPreviewTheme,
} from "../lib/appearance";
import { AppStore } from "../lib/chat/appStore";
import { FakeTransport } from "../lib/chat/testing";
import type { DictationApi } from "../lib/dictation";
import { DEFAULT_GENERAL, type GeneralApi, type SetupApi } from "../lib/setup";
import type { UpdatesApi } from "../lib/updates";
import { createCustomizeFixture } from "./customizePreview";
import { oneProfile } from "./profilesFixture";
import { remoteFixture } from "./remoteFixture";

// `?media=off`: Linux without GStreamer's plugins, where the window makes no sound.
const mediaOff = new URLSearchParams(location.search).get("media") === "off";
const info: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: mediaOff ? "linux" : "windows",
  os_version: mediaOff ? "Omarchy" : "11",
  arch: "x86_64",
  data_dir: "C:/fixture/auracode",
  device: null,
  profile: null,
  profile_switching: false,
  db: null,
  glass: "none",
  launch: { type: "normal" },
  notices: [],
  recovery_notice: null,
  fatal: null,
  media: !mediaOff,
  media_notice: mediaOff
    ? "Audio and video are off here: GStreamer's base and good plugins are missing. Install gst-plugins-base and gst-plugins-good, then restart o7."
    : null,
};
const calls: string[] = [];
const preferences: SetupSnapshot = {
  cold_start: false,
  notice: null,
  preferences: {
    revision: 0,
    step: "motion",
    motion: "full",
    appearance: getAppearancePreference(),
    theme: getThemePreference(),
    text_size: getTextSizePreference(),
    completed: true,
    first_project_id: null,
    tour_seen: [],
    general: { ...DEFAULT_GENERAL },
  },
};
const setup: SetupApi = {
  snapshot: async () => structuredClone(preferences),
  apply: async (revision, action) => {
    calls.push("save");
    // A slow disk: lets a check choose again while a save is still in flight.
    if (sessionStorage.getItem("settings-slow-save"))
      await new Promise((done) => setTimeout(done, 400));
    if (sessionStorage.getItem("settings-fail-save"))
      throw new Error("Preferences could not be saved. Your previous setting is unchanged.");
    if (revision !== preferences.preferences.revision)
      throw new Error("Preferences changed. Reload and retry.");
    const general = preferences.preferences.general;
    if (action.type === "motion") preferences.preferences.motion = action.value;
    if (action.type === "appearance") preferences.preferences.appearance = action.value;
    if (action.type === "theme") {
      preferences.preferences.theme = action.value;
      rememberPreviewTheme(action.value);
    }
    if (action.type === "text_size") preferences.preferences.text_size = action.value;
    if (action.type === "default_agent") {
      if (general.default_agent !== action.agent_id) {
        general.default_model = null;
        general.default_effort = null;
      }
      general.default_agent = action.agent_id;
    }
    if (action.type === "default_model") {
      if (general.default_model !== action.model) general.default_effort = null;
      general.default_model = action.model;
    }
    if (action.type === "default_effort") general.default_effort = action.effort;
    if (action.type === "default_autonomy") general.default_autonomy = action.value;
    if (action.type === "automatic_checkpoints") general.automatic_checkpoints = action.enabled;
    if (action.type === "notifications") general.notifications = action.enabled;
    if (action.type === "sounds") general.sounds = action.enabled;
    if (action.type === "editor") general.editor = action.editor;
    if (action.type === "keep_awake") general.keep_awake = action.enabled;
    if (action.type === "close_to_tray") general.close_to_tray = action.enabled;
    if (action.type === "copy_instructions_to_worktrees")
      general.copy_instructions_to_worktrees = action.enabled;
    if (action.type === "agent_memory_writes") general.agent_memory_writes = action.enabled;
    if (action.type === "browser") general.browser = action.enabled;
    if (action.type === "browser_passwords") general.browser_passwords = action.enabled;
    if (action.type === "mobile") general.mobile = action.enabled;
    if (action.type === "automatic_updates") general.automatic_updates = action.enabled;
    if (action.type === "coordinated_projects") general.coordinated_projects = action.enabled;
    preferences.preferences.revision++;
    return structuredClone(preferences);
  },
};
const agent = (id: string, name: string, ready: boolean): AgentEntry => ({
  id,
  name,
  description: name,
  availability: ready
    ? { state: "ready", version: "1.0.0", path: `/usr/local/bin/${id}` }
    : { state: "missing", reason: "Not installed." },
  launch: id,
  needs_download: false,
  last_verified: "",
  installation: null,
  latest_release: null,
});
// Codex has reported its models in this run; Claude Code has not started a chat yet.
const general: GeneralApi = {
  agents: async () => [
    agent("claude", "Claude Code", true),
    agent("codex", "Codex", true),
    agent("pi", "Pi", false),
  ],
  models: async (agentId) =>
    agentId === "codex"
      ? [
          { value: "gpt-5.5-codex", name: "GPT-5.5 Codex", description: null, group: null },
          { value: "gpt-5.5-mini", name: "GPT-5.5 mini", description: null, group: null },
        ]
      : [],
  // Codex ran its default model and GPT-5.5 mini here; their levels are known.
  thinking: async (agentId, model) =>
    agentId === "codex" && model !== "gpt-5.5-codex"
      ? ["default", "low", "medium", "high"].map((value) => ({
          value,
          name: value === "default" ? "Default" : value,
          description: null,
          group: null,
        }))
      : [],
  // Cursor and Zed are installed; files open in Cursor until Settings picks another.
  editors: async () => ({ editors: ["cursor", "zed"], opens_in: "cursor" }),
};
let status: UpdateStatus = {
  channel: new URLSearchParams(location.search).has("staging") ? "staging" : "stable",
  phase: new URLSearchParams(location.search).has("unsigned") ? "disabled" : "idle",
  current_version: "0.1.1570",
  automatic: true,
  version: null,
  notes: null,
  downloaded_bytes: 0,
  total_bytes: null,
  message: null,
};
const updates: UpdatesApi = {
  status: async () => {
    calls.push("status");
    return structuredClone(status);
  },
  check: async () => {
    calls.push("check");
    status = {
      ...status,
      phase: "available",
      version: "0.2.0",
      notes: "Synthetic update fixture. No release is downloaded.",
    };
    return structuredClone(status);
  },
  download: async () => {
    calls.push("download");
    status = { ...status, phase: "downloading", total_bytes: 2048, downloaded_bytes: 1024 };
    await new Promise((done) => setTimeout(done, 800));
    status = { ...status, phase: "ready", downloaded_bytes: 2048 };
    return structuredClone(status);
  },
  install: async () => {
    calls.push("install");
    status = {
      ...status,
      phase: "waiting_for_idle",
      message: "A project terminal is open. Close it, then retry when ready.",
    };
    return structuredClone(status);
  },
};
Object.assign(window, {
  settingsFixture: {
    calls: () => [...calls],
    motion: () => preferences.preferences.motion,
    theme: () => preferences.preferences.theme,
    textSize: () => preferences.preferences.text_size,
    general: () => structuredClone(preferences.preferences.general),
  },
});
// Settings > Agents lists these through the same store the chats use; nothing is installed.
function agentsFixture() {
  const listed = [
    agent("claude", "Claude Code", true),
    agent("codex", "Codex", true),
    agent("pi", "Pi", false),
  ];
  const transport = new FakeTransport({
    agentsList: async () => listed,
    agentsRefresh: async () => listed,
    agentSetConnection: async () => listed,
    agentMaintenance: async () => ({
      auth_status: false,
      login_available: false,
      update_unavailable: "Synthetic fixture: no vendor command can run here.",
    }),
  });
  const app = new AppStore(transport);
  app.start();
  return { app, transport };
}

// Voice's microphone test: the words of one sentence, then what finishing heard. No audio.
let heard: (event: Parameters<Parameters<DictationApi["start"]>[1]>[0]) => void = () => {};
const dictation: DictationApi = {
  model: async () => ({
    installed: true,
    name: "Parakeet TDT-CTC 110M",
    bytes: 113552320,
    supported: true,
  }),
  start: async (_id, receive) => {
    heard = receive;
    receive({ state: "preparing" });
    receive({
      state: "listening",
      seconds: 1,
      level: 0.4,
      speaking: true,
      text: { settled: "", stable: "Testing one", tentative: "two" },
    });
  },
  // As the engine does: finishing settles the words, cancelling ends the recording.
  stop: async () => {
    heard({ state: "finishing", text: { settled: "Testing one two", stable: "", tentative: "" } });
  },
  cancel: async () => {
    heard({ state: "cancelled" });
  },
};
Object.assign(window, {
  dictationFixture: {
    complete: () => heard({ state: "complete", text: "Testing one, two, three." }),
  },
});

const remote = remoteFixture();

function Preview() {
  const params = new URLSearchParams(location.search);
  const legacy = params.has("legacy-instructions");
  const [closed, setClosed] = useState(false);
  const [customize] = useState(() => createCustomizeFixture());
  const [agents] = useState(() => agentsFixture());
  const target: InstructionsTarget =
    params.get("scope") === "chat"
      ? { type: "chat", chat_id: "fixture-chat" }
      : params.get("scope") === "folder"
        ? { type: "folder", folder_id: "fixture-folder" }
        : { type: "global" };
  if (closed)
    return (
      <main className="g-root">
        <h1>Back to workspace</h1>
      </main>
    );
  const page = (params.get("page") as Page | null) ?? (legacy ? "instructions" : "general");
  return (
    <SettingsScreen
      profiles={oneProfile}
      initialPage={page}
      instructionsTarget={target}
      info={info}
      setup={setup}
      general={general}
      updates={updates}
      dictation={dictation}
      remote={remote}
      workspace={customize.workspace}
      projectId={target.type === "global" ? null : "fixture-project"}
      activeChatId={target.type === "chat" ? target.chat_id : undefined}
      chatLabel={target.type === "chat" ? "Fixture chat folder" : undefined}
      services={customize.services}
      agents={agents}
      reveal={async () => {
        calls.push("reveal");
      }}
      onBack={() => setClosed(true)}
      onSetup={() => {
        calls.push("setup");
      }}
      onDiagnostics={() => {
        calls.push("diagnostics");
      }}
    />
  );
}
export function mountSettingsPreview(container: HTMLElement): void {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
