/** Development-only Diagnostics fixture: the shell, the home view and the stress view, no IPC. */
import "../styles.css";
import "../ui-tokens.css";
import "../diagnostics.css";
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { AppInfo, StressConfig } from "../bindings";
import { ErrorScreen } from "../components/ErrorScreen";
import { Home } from "../components/Home";
import { Sidebar } from "../components/Sidebar";
import { StressView } from "../components/StressView";
import { StreamStore } from "../lib/session";
import { DEFAULT_STRESS } from "../lib/stress";

const info: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "windows",
  os_version: "11",
  arch: "x86_64",
  data_dir: "C:/Users/fixture/AppData/Roaming/si.auracode.desktop",
  device: { name: "Fixture computer" } as AppInfo["device"],
  profile: null,
  profile_switching: false,
  db: null,
  glass: "none",
  launch: { type: "normal" },
  notices: ["GHOST_STRESS was ignored: orbs must be between 1 and 24."],
  recovery_notice:
    "Your database was damaged, so Ghost rebuilt it. Nothing was lost; the damaged copy was kept at C:/fixture/backups/aura.db.corrupt-1.",
  fatal: null,
  media: true,
  media_notice: null,
};
// A store that never streams: the fixture shows the idle panels only.
const store = new StreamStore({
  start: () => Promise.reject(new Error("Unavailable in this fixture.")),
  cancel: () => Promise.resolve(),
});

function Preview() {
  const [view, setView] = useState(new URLSearchParams(location.search).get("view") ?? "home");
  const [config, setConfig] = useState<StressConfig>(DEFAULT_STRESS);
  if (view === "error")
    return (
      <ErrorScreen
        title="Ghost couldn't open your data"
        message="The database at C:/Users/fixture/AppData/Roaming/si.auracode.desktop/profiles/local/aura.db is locked by another process (os error 33)."
        dataDir={info.data_dir}
        os={info.os}
        notices={info.notices}
      />
    );
  return (
    <div className="app diagnostics-shell">
      <Sidebar brandId="claude-code" onBrandChange={() => {}} orbState="idle" onChats={() => {}} />
      <main className="main">
        <div className="drag-strip" data-tauri-drag-region="" />
        <div className="main-scroll">
          {view === "stress" ? (
            <StressView
              store={store}
              config={config}
              onConfigChange={setConfig}
              autoStart={false}
              onExit={() => setView("home")}
            />
          ) : (
            <Home
              info={info}
              store={store}
              onOpenStress={() => setView("stress")}
              onRecoveryDismissed={() => {}}
            />
          )}
        </div>
      </main>
    </div>
  );
}

const root = document.getElementById("root");
if (root)
  createRoot(root).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
