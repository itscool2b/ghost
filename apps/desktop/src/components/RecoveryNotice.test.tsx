import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppInfo } from "../bindings";
import type { DismissState } from "../lib/dismiss";
import { InfoCard } from "./InfoCard";
import { dismissRecovery, RecoveryNoticeView } from "./RecoveryNotice";

const NOTICE =
  "Your database was damaged, so Ghost rebuilt it. Nothing was lost; the damaged copy was kept at /data/profiles/local/backups/aura.db.corrupt-1.";

const INFO: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "macos",
  os_version: "26.6.2",
  arch: "aarch64",
  data_dir: "/data",
  device: null,
  profile: null,
  profile_switching: false,
  db: null,
  glass: "liquid_glass",
  launch: { type: "normal" },
  notices: [],
  recovery_notice: NOTICE,
  fatal: null,
  media: true,
  media_notice: null,
};

const ENABLED_BUTTON = /<button type="button" class="[^"]*">Dismiss<\/button>/;

function lastOf(states: readonly DismissState[]): DismissState {
  const last = states[states.length - 1];
  if (last === undefined) throw new Error("no state was reported");
  return last;
}

/** The notice as the info card's list shows it in `state`. */
function view(state: DismissState): string {
  return renderToStaticMarkup(
    <ul>
      <RecoveryNoticeView message={NOTICE} state={state} onDismiss={() => {}} />
    </ul>,
  );
}

describe("the database repair notice", () => {
  it("is listed with a Dismiss button, beside notices that cannot be dismissed", () => {
    const html = renderToStaticMarkup(
      <InfoCard
        info={{ ...INFO, notices: ["GHOST_STRESS was ignored."] }}
        onRecoveryDismissed={() => {}}
      />,
    );
    expect(html).toContain(NOTICE);
    expect(html).toContain("GHOST_STRESS was ignored.");
    expect(html.match(/>Dismiss<\/button>/g)).toHaveLength(1);
    expect(html).toMatch(ENABLED_BUTTON);
  });

  it("is not shown when there is no repair to report", () => {
    const html = renderToStaticMarkup(
      <InfoCard info={{ ...INFO, recovery_notice: null }} onRecoveryDismissed={() => {}} />,
    );
    expect(html).not.toContain("Dismiss");
    expect(html).not.toContain('aria-label="Notices"');
  });

  it("disables its button while the shell is asked", () => {
    const html = view({ status: "dismissing" });
    expect(html).toContain(NOTICE);
    expect(html).toMatch(/<button type="button" class="[^"]*" disabled="">Dismissing…<\/button>/);
    expect(html).not.toContain('role="alert"');
  });

  it("is gone once dismissed", () => {
    expect(view({ status: "dismissed" })).toBe("<ul></ul>");
  });
});

describe("dismissing the database repair notice", () => {
  beforeEach(() => {
    // Tauri's mocks install the IPC bridge on `window`, which Node lacks.
    vi.stubGlobal("window", globalThis);
  });

  afterEach(() => {
    clearMocks();
    Reflect.deleteProperty(globalThis, "__TAURI_INTERNALS__");
    Reflect.deleteProperty(globalThis, "__TAURI_EVENT_PLUGIN_INTERNALS__");
    vi.unstubAllGlobals();
  });

  it("asks the shell to acknowledge the repair, naming no path, then hides the notice", async () => {
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
      return null;
    });
    const states: DismissState[] = [];
    let dismissed = 0;
    await dismissRecovery(
      (state) => states.push(state),
      () => {
        dismissed += 1;
      },
    );
    expect(calls.map((call) => call.cmd)).toEqual(["acknowledge_recovery"]);
    expect(calls[0]?.payload ?? {}).toEqual({});
    expect(states).toEqual([{ status: "dismissing" }, { status: "dismissed" }]);
    expect(dismissed).toBe(1);
    expect(view(lastOf(states))).toBe("<ul></ul>");
  });

  it("keeps the notice and shows the shell's error when it cannot dismiss it", async () => {
    const message =
      "Could not dismiss the notice: Could not access /data/backups/aura.db.corrupt-1.json: Permission denied (os error 13)";
    mockIPC(() => Promise.reject({ code: "acknowledge_failed", message }));
    const states: DismissState[] = [];
    let dismissed = 0;
    await dismissRecovery(
      (state) => states.push(state),
      () => {
        dismissed += 1;
      },
    );
    expect(dismissed).toBe(0);
    expect(lastOf(states)).toEqual({ status: "shown", problem: message });

    const html = view(lastOf(states));
    expect(html).toContain(NOTICE);
    expect(html).toContain(`<span role="alert" class="notice-problem">${message}</span>`);
    expect(html).toMatch(ENABLED_BUTTON);
  });

  it("keeps the notice when Tauri refuses the call", async () => {
    const denial = "Command acknowledge_recovery not allowed by ACL";
    mockIPC(() => Promise.reject(denial));
    const states: DismissState[] = [];
    await dismissRecovery(
      (state) => states.push(state),
      () => {
        throw new Error("the notice must stay");
      },
    );
    expect(lastOf(states)).toEqual({ status: "shown", problem: denial });
    expect(view(lastOf(states))).toContain(denial);
  });
});
