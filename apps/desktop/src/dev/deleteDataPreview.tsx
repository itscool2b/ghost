/**
 * Development-only: Settings' "Delete all Ghost data" on a synthetic review. No file, key,
 * worktree or process is touched; applying only records the call.
 *
 *   ?preview=delete-data             Windows, nothing unsaved
 *   ?preview=delete-data-worktrees   Windows, two worktrees with unsaved work
 *   ?preview=delete-data-macos       macOS, with "Also move Ghost to the Trash"
 *   &case=fail                       applying fails (the review is used up)
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  AppInfo,
  DataCleanupChoice,
  DataCleanupReview,
  SetupSnapshot,
  UpdateStatus,
} from "../bindings";
import { SettingsScreen } from "../components/settings/SettingsScreen";
import {
  getAppearancePreference,
  getTextSizePreference,
  getThemePreference,
} from "../lib/appearance";
import type { DataCleanupApi } from "../lib/dataCleanup";
import { DEFAULT_GENERAL, type GeneralApi, type SetupApi } from "../lib/setup";
import type { UpdatesApi } from "../lib/updates";
import { oneProfile } from "./profilesFixture";

type Mode = "delete-data" | "delete-data-worktrees" | "delete-data-macos";

const WINDOWS_ROOT = "C:\\Users\\you\\AppData\\Local\\si.auracode.desktop";
const MAC_ROOT = "/Users/you/Library/Application Support/si.auracode.desktop";

function review(mode: Mode): DataCleanupReview {
  const mac = mode === "delete-data-macos";
  const root = mac ? MAC_ROOT : WINDOWS_ROOT;
  const sep = mac ? "/" : "\\";
  const worktree = (name: string) => `${root}${sep}profiles${sep}local${sep}worktrees${sep}${name}`;
  const repository = mac ? "/Users/you/code/shop" : "C:\\Users\\you\\code\\shop";
  const unsaved = mode !== "delete-data";
  return {
    token: "preview",
    scope: "machine",
    items: mac
      ? [
          { kind: "data", path: root, bytes: 1_288_490_189 },
          { kind: "app", path: "/Users/you/Library/Caches/si.auracode.desktop", bytes: 48_234_496 },
          { kind: "app", path: "/Users/you/Library/WebKit/si.auracode.desktop", bytes: 3_145_728 },
          {
            kind: "app",
            path: "/Users/you/Library/Preferences/si.auracode.desktop.plist",
            bytes: 2048,
          },
        ]
      : [
          { kind: "data", path: root, bytes: 1_288_490_189 },
          {
            kind: "app",
            path: "C:\\Users\\you\\AppData\\Roaming\\si.auracode.desktop",
            bytes: 312,
          },
          {
            kind: "temporary",
            path: "C:\\Users\\you\\AppData\\Local\\Temp\\Ghost-0.2.0-updater-k3d9",
            bytes: 9_437_184,
          },
        ],
    worktrees: [
      ...(unsaved
        ? [
            {
              path: worktree("4f1c2a9b7e01"),
              repository,
              branch: "ghost/fix-login-redirect",
              summary: "3 uncommitted files, 1 commit only on ghost/fix-login-redirect",
              unsaved: true,
            },
            {
              path: worktree("9d0b6c3e2f44"),
              repository,
              branch: null,
              summary: "2 commits on no branch",
              unsaved: true,
            },
          ]
        : []),
      {
        path: worktree("c7e5a1d08b32"),
        repository,
        branch: "ghost/update-readme",
        summary: "Nothing unsaved",
        unsaved: false,
      },
    ],
    secure_store: mac ? "the macOS keychain" : "Windows Credential Manager",
    app_bundle: mac ? "/Applications/Ghost.app" : null,
  };
}

const calls: string[] = [];
const params = new URLSearchParams(location.search);

function api(mode: Mode): DataCleanupApi {
  return {
    review: async () => {
      calls.push("review");
      return review(mode);
    },
    apply: async (token: string, choice: DataCleanupChoice) => {
      calls.push(`apply:${token}:${JSON.stringify(choice)}`);
      if (params.get("case") === "fail")
        throw new Error("The cleanup could not start: the program was not found.");
      return null;
    },
  };
}

const info = (mode: Mode): AppInfo => ({
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: mode === "delete-data-macos" ? "macos" : "windows",
  os_version: mode === "delete-data-macos" ? "15.4" : "11",
  arch: mode === "delete-data-macos" ? "aarch64" : "x86_64",
  data_dir: mode === "delete-data-macos" ? MAC_ROOT : WINDOWS_ROOT,
  device: null,
  profile: null,
  profile_switching: false,
  db: null,
  glass: "none",
  launch: { type: "normal" },
  notices: [],
  recovery_notice: null,
  fatal: null,
  media: true,
  media_notice: null,
});

const snapshot: SetupSnapshot = {
  cold_start: false,
  notice: null,
  preferences: {
    revision: 0,
    step: "motion",
    motion: "full",
    completed: true,
    first_project_id: null,
    tour_seen: [],
    appearance: getAppearancePreference(),
    theme: getThemePreference(),
    text_size: getTextSizePreference(),
    general: { ...DEFAULT_GENERAL },
  },
};
const setup: SetupApi = {
  snapshot: async () => structuredClone(snapshot),
  apply: async () => structuredClone(snapshot),
};
const general: GeneralApi = {
  agents: async () => [],
  models: async () => [],
  thinking: async () => [],
  editors: async () => ({ editors: [], opens_in: null }),
};
const status: UpdateStatus = {
  channel: "stable",
  phase: "idle",
  current_version: "0.1.1570",
  automatic: true,
  version: null,
  notes: null,
  downloaded_bytes: 0,
  total_bytes: null,
  message: null,
};
const updates: UpdatesApi = {
  status: async () => structuredClone(status),
  check: async () => structuredClone(status),
  download: async () => structuredClone(status),
  install: async () => structuredClone(status),
};

Object.assign(window, { deleteDataFixture: { calls: () => [...calls] } });

function Preview({ mode }: { readonly mode: Mode }) {
  const [back, setBack] = useState(false);
  if (back)
    return (
      <main className="g-root">
        <h1>Back to workspace</h1>
      </main>
    );
  return (
    <SettingsScreen
      profiles={oneProfile}
      info={info(mode)}
      initialPage="privacy"
      setup={setup}
      general={general}
      updates={updates}
      reveal={async () => {}}
      dataCleanup={api(mode)}
      deleteDataOpenOnLoad
      onBack={() => setBack(true)}
      onSetup={() => {}}
      onDiagnostics={() => {}}
    />
  );
}

export function mountDeleteDataPreview(container: HTMLElement, mode: string): void {
  const known: Mode =
    mode === "delete-data-worktrees" || mode === "delete-data-macos" ? mode : "delete-data";
  createRoot(container).render(
    <StrictMode>
      <Preview mode={known} />
    </StrictMode>,
  );
}
