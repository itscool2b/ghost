/** Real chat-screen routing over synthetic workspace metadata. No native commands or processes. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { AppInfo, ProjectFolder, ProjectSummary, WorkspaceSnapshot } from "../bindings";
import { ChatsScreen } from "../components/chat/ChatsScreen";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
} from "../lib/appearance";
import { AppStore } from "../lib/chat/appStore";
import {
  attachResult,
  chatSummary,
  FakeTransport,
  historyPage,
  sessionInfo,
} from "../lib/chat/testing";
import { DEFAULT_GENERAL, type SetupApi } from "../lib/setup";
import { type WorkspaceApi, WorkspaceStore } from "../lib/workspace";
import { worktreeRecoveryApi } from "../lib/worktreeRecovery";
import { fixtureFolderMatches } from "./folderMatches";
import { withPreviewContext } from "./previewContext";

const SOURCE =
  "C:/Users/Example/Work/Customer Platforms/October Delivery/Shared Engineering Workspaces/Client Portal/desktop-and-web-application";
const TREE = "C:/Users/Example/AppData/Local/auracode/profiles/local/worktrees/9d6cd062e86e";
const folder = (id: string, project: string, name: string, path: string): ProjectFolder => ({
  id,
  project_id: project,
  display_name: name,
  path,
  folder_state: "ok",
  unavailable_reason: null,
  kind: id === "notes" ? "local" : "git",
  git_remote_url: null,
  git_default_branch: id === "notes" ? null : "main",
});
const project = (id: string, name: string, folders: ProjectFolder[]): ProjectSummary => ({
  id,
  name,
  folders,
  color: null,
  default_agent: null,
  default_model: null,
  created_at: 1,
  updated_at: 1,
});
const folders = [
  folder("source", "client", "portal-source", SOURCE),
  // Matches Rust workspace_view's format, including the full source path, not just its basename.
  folder("tree", "client", `ghost/9d6cd062 (worktree from ${SOURCE})`, TREE),
  folder("documents", "client", "Design notes", "C:/Projects/Client Portal/design-notes"),
  folder("notes", "notes", "notes", "C:/Projects/notes"),
];
const chats = [
  chatSummary("header-tree", {
    title: "Review release checklist",
    folder: TREE,
    status: "stopped",
  }),
  chatSummary("header-source", { title: "Local source folder", folder: SOURCE, status: "stopped" }),
  chatSummary("header-documents", {
    title: "Local second folder",
    folder: "C:/Projects/Client Portal/design-notes",
    status: "stopped",
  }),
  chatSummary("header-notes", {
    title: "Local matching names",
    folder: "C:/Projects/notes",
    status: "stopped",
  }),
];
const snapshot: WorkspaceSnapshot = {
  projects: [
    project("client", "Client portal", folders.slice(0, 3)),
    project("notes", "notes", folders.slice(3)),
  ],
  chats: chats.map((chat, index) => ({
    chat,
    project_id: index === 3 ? "notes" : "client",
    folder_id: ["tree", "source", "documents", "notes"][index] ?? null,
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
  throw new Error("This fixture only displays existing chats.");
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
      first_project_id: "client",
      tour_seen: [],
      general: { ...DEFAULT_GENERAL },
    },
  }),
  apply: unavailable,
};

export function mountHeaderLabelsPreview(container: HTMLElement): void {
  const transport = withPreviewContext(
    new FakeTransport({
      chatsList: async () => structuredClone(chats),
      attach: async (id) => {
        const chat = chats.find((entry) => entry.id === id);
        if (!chat) throw new Error("Unknown fixture chat.");
        return attachResult(id, 1, historyPage(id, []), {
          chat,
          session: sessionInfo({ status: "stopped" }),
        });
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
  // New chat mounts briefly before the real navigation request opens its saved chat.
  worktreeRecoveryApi.list = async () => [];
  const app = new AppStore(transport);
  const workspace = new WorkspaceStore(api);
  createRoot(container).render(
    <StrictMode>
      <ChatsScreen
        app={app}
        transport={transport}
        workspace={workspace}
        info={info}
        setup={setup}
        openChatRequest={{ id: "header-tree" }}
        onDiagnostics={() => {}}
        onRecoveryDismissed={() => {}}
      />
    </StrictMode>,
  );
}
