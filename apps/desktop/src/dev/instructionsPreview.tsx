/** Development-only Instructions harness: Settings on the Instructions page with fixture answers. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { AppInfo } from "../bindings";
import { SettingsScreen } from "../components/settings/SettingsScreen";
import { createInstructionsFixture, type InstructionsScenario } from "./instructionsFixture";
import { oneProfile } from "./profilesFixture";
import { createSetupFixture } from "./setupFixture";

const info: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "windows",
  os_version: "11",
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
  media: true,
  media_notice: null,
};

const SCENARIOS: Readonly<Record<string, InstructionsScenario>> = {
  instructions: "universal",
  "instructions-per-agent": "per-agent",
  "instructions-conflict": "conflict",
  "instructions-drift": "drift",
  "instructions-large": "large",
  "instructions-review": "review",
  "instructions-recovery": "recovery",
};

declare global {
  interface Window {
    instructionsFixture?: { calls: () => string[] };
  }
}

const blocked = () => alert("This is a development-only fixture. Nothing else opens here.");

export function mountInstructionsPreview(container: HTMLElement, mode: string): void {
  const scenario = SCENARIOS[mode] ?? "universal";
  const fixture = createInstructionsFixture(scenario);
  window.instructionsFixture = { calls: fixture.calls };
  createRoot(container).render(
    <StrictMode>
      <SettingsScreen
        profiles={oneProfile}
        info={info}
        initialPage="instructions"
        instructions={fixture.api}
        setup={createSetupFixture()}
        instructionsTarget={
          scenario === "drift" ? { type: "global" } : { type: "folder", folder_id: "app" }
        }
        instructionFolders={[
          { id: "app", label: "auracode" },
          { id: "site", label: "Website" },
        ]}
        instructionsOpenOnLoad={
          scenario === "conflict"
            ? { kind: "switch" }
            : scenario === "review"
              ? {
                  kind: "review",
                  text: "# auracode\n\nRust for every backend piece; React and TypeScript for interfaces.\n\n## Commands\n\n- `pnpm lint`, `pnpm typecheck` and `pnpm test` before every commit.\n- `cargo test -p ghost-config` for configuration changes.\n- `node scripts/check-instructions.mjs` after changing the Instructions page.\n\n## Style\n\n- Plain sentences. No em dashes.\n- Keep functions small and name things plainly.\n",
                }
              : undefined
        }
        onBack={blocked}
        onSetup={blocked}
        onDiagnostics={blocked}
      />
    </StrictMode>,
  );
}
