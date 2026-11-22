/**
 * Synthetic service data for Settings' Plugins & MCP, Instructions and Project memory pages (what
 * Customize held). No native operations run.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { AppInfo } from "../bindings";
import { type CustomizeServices, SettingsScreen } from "../components/settings/SettingsScreen";
import { MobileStore } from "../lib/mobile";
import type { SetupApi } from "../lib/setup";
import { WorkspaceStore } from "../lib/workspace";
import { createBrowserSettingsFixture } from "./browserSettingsFixture";
import { createInstructionsFixture } from "./instructionsFixture";
import { FakeMobile } from "./mobileFixture";
import { pluginFixture } from "./pluginLibraryPreview";
import { oneProfile } from "./profilesFixture";
import { createProjectMemoryFixture } from "./projectMemoryPreview";
import { createSetupFixture } from "./setupFixture";

const unsupported = async (): Promise<never> => {
  throw new Error("This synthetic preview does not run native operations.");
};
export function createCustomizeFixture() {
  const workspace = new WorkspaceStore({
    snapshot: async () => ({
      projects: [
        {
          id: "fixture-project",
          name: "auracode",
          color: null,
          default_agent: "claude",
          default_model: null,
          created_at: 1,
          updated_at: 1,
          folders: [
            {
              id: "fixture-folder",
              project_id: "fixture-project",
              display_name: "auracode",
              path: "/fixture/auracode",
              folder_state: "ok",
              unavailable_reason: null,
              kind: "local",
              git_remote_url: null,
              git_default_branch: null,
            },
          ],
        },
      ],
      chats: [],
    }),
    createProject: unsupported,
    updateProject: unsupported,
    addFolder: unsupported,
    rename: unsupported,
    pin: unsupported,
    setRole: unsupported,
    archive: unsupported,
    move: unsupported,
    preview: unsupported,
    folderMatches: unsupported,
    createChat: unsupported,
  });
  const services: CustomizeServices & { readonly setup: SetupApi } = {
    instructions: createInstructionsFixture("universal").api,
    setup: createSetupFixture(),
    memory: createProjectMemoryFixture().api,
    browser: createBrowserSettingsFixture().store,
    mobile: new MobileStore(new FakeMobile()),
    plugins: {
      api: {
        snapshot: async () => pluginFixture("plugins"),
        update: unsupported,
      },
      libraryApi: {
        snapshot: async () => ({ items: [], sources: [], notices: [] }),
        history: async () => [],
        preview: unsupported,
        prepare: unsupported,
        cancel: async () => {},
        apply: unsupported,
        rollback: unsupported,
        share: unsupported,
      },
      vendorApi: {
        capabilities: async () => [],
        review: unsupported,
        cancel: async () => {},
        terminal: () => {
          throw new Error("No native terminals in the preview.");
        },
      },
      mcpApi: {
        snapshot: async () => ({ revision: 1, servers: [] }),
        save: unsupported,
        remove: unsupported,
        credentials: unsupported,
        import: unsupported,
        export: unsupported,
        test: unsupported,
        sessionAction: unsupported,
        importNative: unsupported,
      },
    },
  };
  void workspace.refresh();
  return { workspace, services };
}

const info = {
  os: "windows",
  os_version: "11",
  arch: "x86_64",
  app_version: "0.1.0",
  data_dir: null,
  profile: null,
  device: null,
} as unknown as AppInfo;

/** `?preview=customize`: Settings opened from the sidebar's Customize, on Plugins & MCP. */
export function mountCustomizePreview(container: HTMLElement) {
  const { workspace, services } = createCustomizeFixture();
  Object.assign(window, { customizeFixture: { refresh: () => workspace.refresh() } });
  createRoot(container).render(
    <StrictMode>
      <SettingsScreen
        profiles={oneProfile}
        info={info}
        initialPage="plugins"
        workspace={workspace}
        projectId="fixture-project"
        services={services}
        setup={services.setup}
        onBack={() => {
          location.href = "/?preview=settled";
        }}
        onSetup={() => {}}
        onDiagnostics={() => {}}
      />
    </StrictMode>,
  );
}
