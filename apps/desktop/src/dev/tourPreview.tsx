/**
 * Development-only fixture of the tour (`?preview=tour`): the real chats screen over invented
 * projects and chats, with a browser and a device that are stand-ins. No native command runs.
 *
 * `&offer` lets the screen offer the tour by itself, as the app does; `&go` takes it at once;
 * `&seen=a,b` starts from a profile that has seen those stops; `&nochats` is a profile without a
 * chat; `&arrive` flies the wing in as setup's end does. `window.tourFixture` reports what the
 * profile has seen and which controls the tour names, for `scripts/check-tour.mjs`.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { AppInfo, ProjectFolder, ProjectSummary, WorkspaceSnapshot } from "../bindings";
import { ChatsScreen } from "../components/chat/ChatsScreen";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
} from "../lib/appearance";
import { BrowserStore } from "../lib/browser";
import { AppStore } from "../lib/chat/appStore";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  historyPage,
  items,
  numbered,
  sessionInfo,
} from "../lib/chat/testing";
import { GameStore } from "../lib/game";
import { MobileStore } from "../lib/mobile";
import { DEFAULT_GENERAL, type SetupApi } from "../lib/setup";
import { speechPlayer } from "../lib/speech";
import { NOT_TOURED, TOUR_STOPS } from "../lib/tour/stops";
import { type WorkspaceApi, WorkspaceStore } from "../lib/workspace";
import { worktreeRecoveryApi } from "../lib/worktreeRecovery";
import { FixtureBrowser } from "./browserPreview";
import { fixtureFolderMatches } from "./folderMatches";
import { FakeGame } from "./gameFixture";
import { FakeMobile } from "./mobileFixture";
import { withPreviewContext } from "./previewContext";
import { speechFixture } from "./speechFixture";

declare global {
  interface Window {
    tourFixture?: {
      /** The stops the fixture's profile has saved as seen. */
      readonly seen: () => readonly string[];
      readonly stops: readonly string[];
      readonly notToured: Readonly<Record<string, string>>;
    };
  }
}

const params = new URLSearchParams(location.search);
const folder = (id: string, project: string, name: string): ProjectFolder => ({
  id,
  project_id: project,
  display_name: name,
  path: `C:/Projects/${name}`,
  folder_state: "ok",
  unavailable_reason: null,
  kind: "git",
  git_remote_url: null,
  git_default_branch: "main",
});
const project = (id: string, name: string): ProjectSummary => ({
  id,
  name,
  folders: [folder(`${id}-folder`, id, name)],
  color: null,
  default_agent: null,
  default_model: null,
  created_at: 1,
  updated_at: 1,
});
const projects = [project("shop", "storefront"), project("docs", "handbook")];
const chats = params.has("nochats")
  ? []
  : [
      chatSummary("tour-checkout", {
        title: "Fix the checkout total",
        folder: "C:/Projects/storefront",
        status: "ready",
        agent_id: "claude",
        agent_name: "Claude Code",
      }),
      chatSummary("tour-search", {
        title: "Add search to the catalogue",
        folder: "C:/Projects/storefront",
        status: "stopped",
        agent_id: "codex",
        agent_name: "Codex",
      }),
      chatSummary("tour-guide", {
        title: "Rewrite the onboarding guide",
        folder: "C:/Projects/handbook",
        status: "stopped",
        agent_id: "claude",
        agent_name: "Claude Code",
      }),
    ];
const snapshot: WorkspaceSnapshot = {
  projects,
  chats: chats.map((chat, index) => ({
    chat,
    project_id: index === 2 ? "docs" : "shop",
    folder_id: index === 2 ? "docs-folder" : "shop-folder",
    pinned: false,
    archived: false,
  })),
};
const info: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "windows",
  os_version: "11",
  arch: "x86_64",
  data_dir: "C:/fixture/ghost",
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
// One finished turn in every chat, so an answer's own controls (the speaker) are there to visit.
const answered = [
  ev.turnStarted("tour-turn"),
  ev.user("tour-turn.u", "Why is the checkout total a cent off?"),
  ev.message(
    "tour-answer",
    "The total was rounded per line before tax. It now rounds once, at the end, and the three totals that were a cent off match the receipts.",
  ),
  ev.turnEnded("tour-turn"),
];
const unavailable = async (): Promise<never> => {
  throw new Error("This fixture only shows the tour.");
};

// The profile's saved setup: finished, with whatever the address says it has seen of the tour.
let revision = 1;
let seen: string[] = (params.get("seen") ?? "").split(",").filter(Boolean);
const saved = () => ({
  cold_start: false,
  notice: null,
  preferences: {
    revision,
    step: "project" as const,
    motion: "full" as const,
    appearance: getAppearancePreference(),
    theme: getThemePreference(),
    text_size: getTextSizePreference(),
    completed: true,
    first_project_id: "shop",
    tour_seen: [...seen],
    general: { ...DEFAULT_GENERAL },
  },
});
const setup: SetupApi = {
  snapshot: async () => saved(),
  apply: async (at, action) => {
    if (at !== revision) throw new Error("Setup changed in another view.");
    if (action.type !== "tour_seen") throw new Error("This fixture only saves the tour.");
    seen = [...new Set([...seen, ...action.stops])];
    revision += 1;
    return saved();
  },
};

/** One stand-in browser and device for the page (StrictMode builds the state twice). */
let browser: FixtureBrowser | null = null;
let mobile: FakeMobile | null = null;
let game: FakeGame | null = null;

function Preview() {
  const [fixture] = useState(() => {
    browser ??= new FixtureBrowser();
    mobile ??= new FakeMobile();
    // The tour's chat is a game's, so the header shows the Game button the tour stops at.
    game ??= Object.assign(new FakeGame(), { everyChat: true });
    const transport = withPreviewContext(
      new FakeTransport({
        chatsList: async () => structuredClone(chats),
        attach: async (id) => {
          const chat = chats.find((entry) => entry.id === id);
          if (!chat) throw new Error("Unknown fixture chat.");
          return attachResult(
            id,
            answered.length,
            historyPage(id, items(numbered(answered.map((event) => [null, event])))),
            {
              chat,
              session: sessionInfo({ status: chat.status === "ready" ? "ready" : "stopped" }),
            },
          );
        },
        send: unavailable,
        chatCreate: unavailable,
        start: unavailable,
      }),
    );
    const api: WorkspaceApi = {
      snapshot: async () => structuredClone(snapshot),
      createProject: unavailable,
      updateProject: unavailable,
      addFolder: unavailable,
      rename: unavailable,
      pin: unavailable,
      setRole: unavailable,
      archive: unavailable,
      move: unavailable,
      preview: unavailable,
      folderMatches: fixtureFolderMatches(() => snapshot),
      createChat: unavailable,
    };
    worktreeRecoveryApi.list = async () => [];
    const whole = Object.assign(transport, {
      browser: new BrowserStore(browser),
      mobile: new MobileStore(mobile),
      game: new GameStore(game),
    });
    return { transport: whole, app: new AppStore(whole), workspace: new WorkspaceStore(api) };
  });
  // Settings is not part of this fixture: its button only says it was pressed.
  const [request, setRequest] = useState(params.has("go") ? { id: "go" } : undefined);
  return (
    <ChatsScreen
      app={fixture.app}
      transport={fixture.transport}
      workspace={fixture.workspace}
      info={info}
      setup={setup}
      offersTour={params.has("offer")}
      tourArrive={params.has("arrive")}
      tourRequest={request}
      openChatRequest={params.has("open") ? { id: "tour-checkout" } : undefined}
      onDiagnostics={() => {}}
      onRecoveryDismissed={() => {}}
      onSettings={() => setRequest({ id: String(Date.now()) })}
      onCustomize={() => {}}
      onDashboard={() => {}}
      onAutomations={() => {}}
    />
  );
}

export function mountTourPreview(container: HTMLElement): void {
  window.speechFixture = speechFixture();
  speechPlayer.use(window.speechFixture.api);
  window.tourFixture = {
    seen: () => [...seen],
    stops: TOUR_STOPS.map((stop) => stop.id),
    notToured: NOT_TOURED,
  };
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
