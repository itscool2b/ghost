import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SessionLists } from "../../bindings";
import { lists } from "../../lib/chat/testing";
import { AccessPicker } from "./AccessPicker";

/** What a running agent reports: lists fetched after it started carry its capabilities. */
const running = {
  images: false,
  steer: false,
  interrupt: false,
  resume: false,
  fork: false,
  rewind: false,
  set_model: false,
  set_effort: false,
  permissions: true,
  plans: false,
  usage: false,
  limits: false,
  slash_commands: false,
  add_dirs: false,
  plan_edits: false,
  autonomy_levels: [],
} satisfies SessionLists["capabilities"];

const nativeLists = (): SessionLists =>
  lists(1, {
    capabilities: running,
    controls: {
      version: 1,
      access: {
        requested: "review",
        effective: "review",
        pending: null,
        native_mode: "nativeReview",
        note: null,
        choices: [
          {
            value: "review",
            name: "Review every action",
            description: "Native access policy",
            legacy_autonomy: null,
          },
        ],
      },
      workflow: {
        requested: "plan",
        effective: "plan",
        pending: null,
        native_mode: null,
        note: null,
        choices: [
          { value: "plan", name: "Plan", description: null, legacy_autonomy: null },
          { value: "execute", name: "Execute", description: null, legacy_autonomy: null },
        ],
      },
    },
  });
const render = (overrides: Partial<Parameters<typeof AccessPicker>[0]> = {}) =>
  renderToStaticMarkup(
    <AccessPicker
      lists={nativeLists()}
      autonomy="full_auto"
      saving={null}
      status="ready"
      error={null}
      onSetAutonomy={() => {}}
      onSetControl={() => {}}
      {...overrides}
    />,
  );
describe("native access and planning", () => {
  it("offers advertised native access without inventing Full access", () => {
    const html = render();
    expect(html).toContain("Review every action");
    expect(html).not.toContain("Full access");
    expect(html).toContain("Approve and execute");
    expect(html).toContain("Approve execution before changing access.");
  });
  it("keeps an unavailable native setting visible without a legacy full-access fallback", () => {
    const state = nativeLists();
    if (!state.controls?.access) throw new Error("Missing fixture control");
    state.controls.access.effective = null;
    expect(render({ lists: state })).toContain("Access unavailable");
    expect(render({ lists: state })).not.toContain("Full access");
  });
  it("disables mode transitions while queued or awaiting acknowledgement", () => {
    for (const state of [
      { transitionBlocked: true },
      {
        controlSaving: {
          type: "workflow" as const,
          value: "execute",
          implementation_approved: true,
        },
      },
    ]) {
      const html = render(state);
      expect(html.match(/<button[^>]*disabled/g)).toHaveLength(3);
    }
  });
  it("takes a mode for the next start while the agent is stopped, and not while it starts", () => {
    const stopped = render({ status: "stopped" });
    const modes = (html: string) =>
      html.slice(html.indexOf('aria-label="Working mode"')).match(/<button[^>]*>/g) ?? [];
    expect(modes(stopped).some((button) => button.includes("disabled"))).toBe(false);
    expect(stopped).not.toContain("Change mode once the agent has started.");
    const starting = render({ status: "starting" });
    expect(modes(starting).every((button) => button.includes("disabled"))).toBe(true);
    expect(starting).toContain("Change mode once the agent has started.");
  });
  it("reports unavailable native planning and offers no fabricated choices", () => {
    const html = render({ lists: lists(1, { capabilities: running }) });
    expect(html).toContain("Native Plan mode is not available on this connection.");
    expect(html).toContain('aria-label="Mode unavailable. Working mode"');
    expect(html).not.toContain('aria-label="Execute. Working mode"');
    expect(html).not.toContain('data-testid="picker-access-choice"');
  });
  it("keeps advertised legacy access subject to the same idle admission", () => {
    const state = lists(1, {
      capabilities: {
        images: false,
        steer: false,
        interrupt: false,
        resume: false,
        fork: false,
        rewind: false,
        set_model: false,
        set_effort: false,
        permissions: true,
        plans: false,
        usage: false,
        limits: false,
        slash_commands: false,
        add_dirs: false,
        plan_edits: false,
        autonomy_levels: ["ask", "full_auto"],
      },
    });
    for (const gate of [{ status: "running" as const }, { transitionBlocked: true }]) {
      expect(render({ lists: state, ...gate }).match(/<button[^>]*disabled/g)).toHaveLength(2);
    }
    expect(render({ lists: state }).match(/<button[^>]*disabled/g)).toBeNull();
  });
});

/** Claude Code's access as the session lists it before its agent runs. */
const waitingLists = (requested: string, values: readonly string[]): SessionLists => {
  const state = lists(1);
  if (!state.controls) throw new Error("Missing fixture controls");
  state.controls.access = {
    ...state.controls.access,
    requested,
    effective: requested,
    choices: values.map((value) => ({
      value,
      name: { default: "Ask for approval", bypassPermissions: "Full access" }[value] ?? value,
      description: null,
      legacy_autonomy: null,
    })),
  };
  return state;
};

describe("access before the agent runs", () => {
  it("names the level a starting chat asks for instead of calling it unavailable", () => {
    const html = render({ lists: lists(1), status: "starting" });
    expect(html).not.toContain("Access unavailable");
    expect(html).toContain('aria-label="Full access. Access level"');
    expect(html).toContain("Access controls load when the agent starts.");
    expect(html).not.toContain("does not offer access controls");
    expect(html).toContain('aria-label="Mode. Working mode"');
    expect(html).toContain("Modes load when the agent starts.");
  });
  it("still says a running agent offers no access control", () => {
    const html = render({ lists: lists(1, { capabilities: running }), status: "ready" });
    expect(html).toContain('aria-label="Access unavailable. Access level"');
    expect(html).toContain("This connection does not offer access controls.");
  });
  it("does not judge a just-ready agent by the lists fetched while it started", () => {
    // Natively, 6 November 2026: the status turned ready 200 ms before the lists were fetched
    // again, and the pill flashed "Access unavailable".
    const html = render({ lists: lists(1), status: "ready" });
    expect(html).not.toContain("Access unavailable");
    expect(html).toContain('aria-label="Full access. Access level"');
    expect(html).not.toContain("does not offer access controls");
    expect(html).toContain('aria-label="Mode. Working mode"');
  });
  it("offers a stopped chat's access for its next start", () => {
    for (const status of ["stopped", "failed", "auth_required"] as const) {
      const html = render({
        lists: waitingLists("default", ["default", "bypassPermissions"]),
        status,
      });
      expect(html).toContain('aria-label="Ask for approval. Access level"');
      expect(html).toContain("Applies when the agent starts.");
      expect(html.match(/<button[^>]*disabled[^>]*data-testid="picker-access-choice"/g)).toBeNull();
      expect(html).toContain('data-testid="picker-access-choice"');
    }
  });
  it("names a starting agent's access and opens it once the agent is ready", () => {
    const html = render({
      lists: waitingLists("default", ["default", "bypassPermissions"]),
      status: "starting",
    });
    expect(html).toContain('aria-label="Ask for approval. Access level"');
    expect(html).toContain("Access opens once the agent has started.");
    expect(html).not.toContain("Applies when the agent starts.");
    expect(html.match(/<button[^>]*disabled[^>]*data-testid="picker-access-choice"/g)).toHaveLength(
      2,
    );
  });
  it("leaves access unchangeable while the agent stops or works", () => {
    for (const status of ["stopping", "running"] as const) {
      const html = render({
        lists: waitingLists("default", ["default", "bypassPermissions"]),
        status,
      });
      expect(
        html.match(/<button[^>]*disabled[^>]*data-testid="picker-access-choice"/g),
      ).toHaveLength(2);
      expect(html).not.toContain("Access opens once the agent has started.");
    }
  });
  it("names a level on its way that the agent's own choices do not list", () => {
    const html = render({
      lists: {
        ...waitingLists("bypassPermissions", ["default", "bypassPermissions"]),
        capabilities: running,
      },
      status: "ready",
      controlSaving: { type: "access", value: "legacy:ask" },
    });
    expect(html).toContain('aria-label="Ask for approval. Access level"');
    expect(html).toContain("Saving…");
    expect(html).not.toContain("Access unavailable");
  });
  it("names a saved native setting no known choice offers as plain Access", () => {
    const state = waitingLists("acceptEdits", []);
    const html = render({ lists: state, status: "stopped" });
    expect(html).toContain('aria-label="Access. Access level"');
    expect(html).not.toContain("Full access");
  });
  it("shows the one Applies note when the session says the same", () => {
    const state = waitingLists("default", ["default"]);
    state.autonomy = { ...state.autonomy, note: "Applies when the agent starts." };
    const html = render({ lists: state, status: "stopped" });
    expect(html.match(/Applies when the agent starts\./g)).toHaveLength(1);
    // Natively: the stopped chat's note stayed beside "Access opens…" while the agent started.
    const starting = render({ lists: state, status: "starting" });
    expect(starting).toContain("Access opens once the agent has started.");
    expect(starting).not.toContain("Applies when the agent starts.");
  });
});
