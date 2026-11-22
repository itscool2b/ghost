import "./styles.css";
import "./ui-tokens.css";
import "./shell.css";
import "./chat.css";
import "./styles/chat-images.css";
import "./context-panel.css";
import "./diagnostics.css";
import "./motion.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { installQuitQuestion } from "./components/QuitQuestionDialog";
import { initializeAppearance } from "./lib/appearance";
import { AppStore } from "./lib/chat/appStore";
import { tauriTransport } from "./lib/chat/transport";
import { installDraftQuit } from "./lib/draftQuit";
import { cancelDemoStream, inTauri, startDemoStream } from "./lib/ipc";
import { readLaunchLists } from "./lib/launchCache";
import { installProfileEvents } from "./lib/profiles";
import { StreamStore } from "./lib/session";
import { setMotionPreference, setupApi } from "./lib/setup";
import { WorkspaceStore, workspaceApi } from "./lib/workspace";

// The platform before the first paint: type weights are tuned per platform (tokens.css), and a
// weight that changed after paint would reflow every line. App confirms it from the native side.
{
  const agent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  const preview = import.meta.env.DEV ? new URLSearchParams(location.search).get("os") : null;
  document.documentElement.dataset.os =
    preview ?? (/Mac/.test(agent) ? "macos" : /Windows/.test(agent) ? "windows" : "linux");
}
initializeAppearance();
installProfileEvents();
const store = new StreamStore({ start: startDemoStream, cancel: cancelDemoStream });
const chats = new AppStore(tauriTransport);
const workspace = new WorkspaceStore(workspaceApi);
installDraftQuit();
installQuitQuestion();

const container = document.getElementById("root");
// The native window stays hidden until App paints. Resolve the active profile's appearance first,
// including after a profile switch reload; an unavailable profile uses the system theme. Beside
// it, read the sidebar this profile last showed (two small files, never the database), so the
// first frame the window shows already holds the person's projects and chats.
const appearanceReady =
  inTauri() && !(import.meta.env.DEV && new URLSearchParams(location.search).has("preview"))
    ? installProfileEvents()
        .then(() => Promise.all([setupApi.snapshot().catch(() => null), readLaunchLists()]))
        .then(([setup, lists]) => {
          if (lists.chats !== null) chats.hydrate(lists.chats);
          if (lists.workspace !== null) workspace.hydrate(lists.workspace);
          // A profile that finished setup opens on its chats, so what setup's page applied on its
          // way past is applied here.
          if (setup !== null) setMotionPreference(setup.preferences.motion);
          return (
            setup !== null &&
            setup.preferences.completed &&
            !setup.cold_start &&
            setup.notice === null
          );
        })
        .catch(() => false)
    : Promise.resolve(false);
void appearanceReady.then((setUp) => {
  if (container) {
    if (import.meta.env.DEV && new URLSearchParams(location.search).has("preview")) {
      const mode = new URLSearchParams(location.search).get("preview");
      if (new URLSearchParams(location.search).has("glass"))
        void import("./dev/glassPreview").then(({ emulateGlass }) => emulateGlass());
      if (new URLSearchParams(location.search).has("quit"))
        void import("./dev/quitPreview").then(({ mountQuitPreview }) => mountQuitPreview());
      if (mode === "project-chat")
        void import("./dev/projectChatPreview").then(({ mountProjectChatPreview }) =>
          mountProjectChatPreview(container),
        );
      else if (mode === "customize")
        void import("./dev/customizePreview").then(({ mountCustomizePreview }) =>
          mountCustomizePreview(container),
        );
      else if (mode === "project-memory")
        void import("./dev/projectMemoryPreview").then(({ mountProjectMemoryPreview }) =>
          mountProjectMemoryPreview(container),
        );
      else if (mode === "header-labels")
        void import("./dev/headerLabelsPreview").then(({ mountHeaderLabelsPreview }) =>
          mountHeaderLabelsPreview(container),
        );
      else if (mode === "setup-recovery")
        void import("./dev/setupRecoveryPreview").then(({ mountSetupRecoveryPreview }) =>
          mountSetupRecoveryPreview(container),
        );
      else if (
        mode === "plugins" ||
        mode === "plugins-mcp" ||
        mode === "plugins-pi" ||
        mode === "plugins-update" ||
        mode === "plugins-failed" ||
        mode === "plugins-progressive"
      )
        void import("./dev/pluginLibraryPreview").then(({ mountPluginLibraryPreview }) =>
          mountPluginLibraryPreview(container, mode),
        );
      else if (mode === "tour")
        void import("./dev/tourPreview").then(({ mountTourPreview }) =>
          mountTourPreview(container),
        );
      else if (mode === "agent-marks")
        void import("./dev/agentMarksPreview").then(({ mountAgentMarksPreview }) =>
          mountAgentMarksPreview(container),
        );
      else if (mode === "accounts")
        void import("./dev/accountsPreview").then(({ mountAccountsPreview }) =>
          mountAccountsPreview(container),
        );
      else if (mode === "images")
        void import("./dev/imagesPreview").then(({ mountImagesPreview }) =>
          mountImagesPreview(container),
        );
      else if (mode === "questions")
        void import("./dev/questionsPreview").then(({ mountQuestionsPreview }) =>
          mountQuestionsPreview(container),
        );
      else if (mode === "coordinator")
        void import("./dev/coordinatorPreview").then(({ mountCoordinatorPreview }) =>
          mountCoordinatorPreview(container),
        );
      else if (mode === "dashboard")
        void import("./dev/dashboardPreview").then(({ mountDashboardPreview }) =>
          mountDashboardPreview(container),
        );
      else if (mode === "opening")
        void import("./dev/openingPreview").then(({ mountOpeningPreview }) =>
          mountOpeningPreview(container),
        );
      else if (mode === "agent-updates")
        void import("./dev/agentUpdatesPreview").then(({ mountAgentUpdatesPreview }) =>
          mountAgentUpdatesPreview(container),
        );
      else if (mode === "browser")
        void import("./dev/browserPreview").then(({ mountBrowserPreview }) =>
          mountBrowserPreview(container),
        );
      else if (mode === "device")
        void import("./dev/devicePreview").then(({ mountDevicePreview }) =>
          mountDevicePreview(container),
        );
      else if (mode === "canvas")
        void import("./dev/canvasPreview").then(({ mountCanvasPreview }) =>
          mountCanvasPreview(container),
        );
      else if (mode === "terminal-chat")
        void import("./dev/terminalChatPreview").then(({ mountTerminalChatPreview }) =>
          mountTerminalChatPreview(container),
        );
      else if (mode === "dictation")
        void import("./dev/dictationPreview").then(({ mountDictationPreview }) =>
          mountDictationPreview(container),
        );
      else if (mode === "steering")
        void import("./dev/steeringPreview").then(({ mountSteeringPreview }) =>
          mountSteeringPreview(container),
        );
      else if (mode === "worktree-setup")
        void import("./dev/worktreeSetupPreview").then(({ mountWorktreeSetupPreview }) =>
          mountWorktreeSetupPreview(container),
        );
      else if (mode === "profiles")
        void import("./dev/profilesPreview").then(({ mountProfilesPreview }) =>
          mountProfilesPreview(container),
        );
      else if (mode === "project-tools")
        void import("./dev/projectToolsPreview").then(({ mountProjectToolsPreview }) =>
          mountProjectToolsPreview(container),
        );
      else if (mode === "git" || mode === "git-missing" || mode === "git-conflict")
        void import("./dev/gitPreview").then(({ mountGitPreview }) => mountGitPreview(container));
      else if (mode === "chat-options")
        void import("./dev/chatOptionsPreview").then(({ mountChatOptionsPreview }) =>
          mountChatOptionsPreview(container),
        );
      else if (mode?.startsWith("instructions"))
        void import("./dev/instructionsPreview").then(({ mountInstructionsPreview }) =>
          mountInstructionsPreview(container, mode),
        );
      else if (mode === "settings")
        void import("./dev/settingsPreview").then(({ mountSettingsPreview }) =>
          mountSettingsPreview(container),
        );
      else if (mode?.startsWith("delete-data"))
        void import("./dev/deleteDataPreview").then(({ mountDeleteDataPreview }) =>
          mountDeleteDataPreview(container, mode),
        );
      else if (
        mode === "setup" ||
        mode === "agents" ||
        mode === "agents-installing" ||
        mode === "new-project" ||
        mode === "clone" ||
        mode === "clone-running" ||
        mode === "clone-error"
      )
        void import("./dev/setupPreview").then(({ mountSetupPreview }) =>
          mountSetupPreview(container),
        );
      else void import("./dev/preview").then(({ mountPreview }) => mountPreview(container));
    } else
      createRoot(container).render(
        <StrictMode>
          <ErrorBoundary>
            <App
              store={store}
              chats={chats}
              transport={tauriTransport}
              workspace={workspace}
              setUp={setUp}
            />
          </ErrorBoundary>
        </StrictMode>,
      );
  }
});
