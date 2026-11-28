import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AgentEntry, AppInfo } from "../../bindings";
import { DEFAULT_GENERAL, preselectAgent } from "../../lib/setup";
import {
  AppearancePage,
  autonomyDetail,
  ChatsPage,
  defaultAgentChoices,
  defaultLevelChoices,
  defaultModelChoices,
  GeneralPage,
} from "./GeneralPanel";
import type { Preferences } from "./preferences";

const ready = { state: "ready", version: "1.0.0", path: "/bin/agent" } as const;
const agent = (id: string, availability: AgentEntry["availability"] = ready): AgentEntry => ({
  id,
  name: id === "claude" ? "Claude Code" : id === "codex" ? "Codex" : id,
  description: id,
  availability,
  launch: id,
  needs_download: false,
  last_verified: "",
  installation: null,
  latest_release: null,
});

describe("Settings' General defaults", () => {
  it("keeps full auto, background services, worktree instruction copies, agents' memory edits and Browser use with saved passwords on, with automatic checkpoints off", () => {
    expect(DEFAULT_GENERAL).toEqual({
      default_agent: null,
      default_model: null,
      default_effort: null,
      default_autonomy: "full_auto",
      automatic_checkpoints: false,
      notifications: true,
      sounds: false,
      keep_awake: true,
      close_to_tray: true,
      copy_instructions_to_worktrees: true,
      agent_memory_writes: true,
      keep_steps_open: false,
      editor: null,
      browser: true,
      browser_passwords: true,
      mobile: true,
      game: true,
      automatic_updates: true,
      coordinated_projects: false,
      speech_voice: null,
    });
  });

  it("offers the agents that can run here and never drops the saved one", () => {
    const agents = [
      agent("claude"),
      agent("codex"),
      agent("pi", { state: "missing", reason: "Not installed." }),
    ];
    expect(defaultAgentChoices(agents, null).map((choice) => choice.id)).toEqual([
      "claude",
      "codex",
    ]);
    expect(defaultAgentChoices(agents, "pi").at(-1)).toEqual({ id: "pi", name: "pi (not found)" });
    expect(defaultAgentChoices([], "codex")).toEqual([{ id: "codex", name: "codex (not found)" }]);
  });

  it("offers the models the agent reported, keeping a saved one it no longer lists", () => {
    const reported = [{ value: "small", name: "Small", description: null, group: null }];
    expect(defaultModelChoices(reported, null)).toEqual([{ value: "small", name: "Small" }]);
    expect(defaultModelChoices(reported, "retired")).toEqual([
      { value: "small", name: "Small" },
      { value: "retired", name: "retired" },
    ]);
    expect(defaultModelChoices([], null)).toEqual([]);
  });

  it("offers the levels the model offered in plain words, keeping a saved one", () => {
    const reported = ["default", "low", "xhigh"].map((value) => ({
      value,
      name: value === "default" ? "Default" : value,
      description: null,
      group: null,
    }));
    // The model's own default is the select's first entry, so it is not listed twice.
    expect(defaultLevelChoices(reported, null)).toEqual([
      { value: "low", name: "Low" },
      { value: "xhigh", name: "Extra high" },
    ]);
    expect(defaultLevelChoices(reported, "max").at(-1)).toEqual({ value: "max", name: "Max" });
    expect(defaultLevelChoices([], null)).toEqual([]);
  });

  it("says what each autonomy level does in one line", () => {
    expect(autonomyDetail("full_auto")).toContain("without asking");
    expect(autonomyDetail("full_auto")).not.toContain("checkpoint");
    for (const level of ["full_auto", "auto_edit", "ask"] as const)
      expect(autonomyDetail(level)).not.toMatch(/\n|\u2014/);
  });

  it("selects the default agent in a fresh New chat only once it can run", () => {
    const agents = [agent("codex"), agent("claude", { state: "checking" })];
    expect(preselectAgent("codex", agents, null)).toBe("codex");
    expect(preselectAgent("claude", agents, null)).toBe("wait");
    expect(preselectAgent("codex", null, null)).toBe("wait");
    expect(preselectAgent("pi", agents, null)).toBeNull();
    // A choice already made (the dock, a click) is never replaced; no default, no selection.
    expect(preselectAgent("codex", agents, "claude")).toBeNull();
    expect(preselectAgent(null, agents, null)).toBeNull();
  });

  it("splits General, Appearance and Chats, and disables nothing but loading", () => {
    const info = { os: "windows", data_dir: "C:/data" } as AppInfo;
    // Not loaded yet: nothing is editable.
    const loading: Preferences = {
      saved: null,
      shown: null,
      error: null,
      change: () => {},
      reload: () => {},
    };
    const lists = {
      agents: async () => [],
      models: async () => [],
      thinking: async () => [],
      editors: async () => ({ editors: [], opens_in: null }),
    };
    const general = renderToStaticMarkup(
      <GeneralPage info={info} prefs={loading} lists={lists} onSetup={() => {}} />,
    );
    const appearance = renderToStaticMarkup(<AppearancePage prefs={loading} />);
    const chats = renderToStaticMarkup(<ChatsPage prefs={loading} />);
    expect(general).toContain('aria-label="Default agent"');
    expect(general).toContain('<span class="ui-select-value">Choose each time</span>');
    // No default agent yet: no model row.
    expect(general).not.toContain('aria-label="Default model"');
    for (const level of ["Ask", "Auto-edit", "Full auto"])
      expect(general).toContain(`>${level}</span>`);
    for (const name of ["Notifications", "Keep awake", "Keep running in the tray"])
      expect(general).toContain(`aria-label="${name}"`);
    expect(appearance).toContain('name="settings-appearance"');
    for (const value of ["System", "Light", "Dark", "Compact", "Comfortable"])
      expect(appearance).toContain(`>${value}</span>`);
    for (const name of ["Keep steps open", "Automatic checkpoints"])
      expect(chats).toContain(`aria-label="${name}"`);
    expect(chats).toContain("folds its work under Worked for");
    expect(chats).toContain("per-turn file undo is unavailable for new messages");
    expect(chats).toContain("Saved checkpoints remain available in Changes");
    for (const html of [general, appearance, chats])
      expect(html.match(/disabled=""/g)?.length).toBeGreaterThanOrEqual(2);
    // Loaded and saving: every control stays enabled, so keyboard focus never drops.
    const saved = {
      cold_start: false,
      notice: null,
      preferences: {
        revision: 3,
        step: "project",
        motion: "full",
        appearance: "system",
        theme: "claude",
        text_size: "compact",
        completed: true,
        first_project_id: null,
        tour_seen: [] as string[],
        general: { ...DEFAULT_GENERAL, keep_steps_open: true },
      },
    } as const;
    const ready: Preferences = { ...loading, saved, shown: saved.preferences };
    for (const html of [
      renderToStaticMarkup(
        <GeneralPage info={info} prefs={ready} lists={lists} onSetup={() => {}} />,
      ),
      renderToStaticMarkup(<AppearancePage prefs={ready} />),
      renderToStaticMarkup(<ChatsPage prefs={ready} />),
    ])
      expect(html).not.toContain('disabled=""');
    expect(renderToStaticMarkup(<ChatsPage prefs={ready} />)).toContain(
      "shows its messages, tools and thinking",
    );
  });

  it("says once why Sounds cannot play where the window makes no sound, and waits", () => {
    const notice =
      "Audio and video are off here: GStreamer's base and good plugins are missing. Install gst-plugins-base and gst-plugins-good, then restart Ghost.";
    const saved = {
      cold_start: false,
      notice: null,
      preferences: {
        revision: 1,
        step: "project",
        motion: "full",
        appearance: "system",
        theme: "claude",
        text_size: "compact",
        completed: true,
        first_project_id: null,
        tour_seen: [] as string[],
        general: { ...DEFAULT_GENERAL, sounds: true },
      },
    } as const;
    const prefs: Preferences = {
      saved,
      shown: saved.preferences,
      error: null,
      change: () => {},
      reload: () => {},
    };
    const lists = {
      agents: async () => [],
      models: async () => [],
      thinking: async () => [],
      editors: async () => ({ editors: [], opens_in: null }),
    };
    const page = (info: AppInfo) =>
      renderToStaticMarkup(
        <GeneralPage info={info} prefs={prefs} lists={lists} onSetup={() => {}} />,
      );
    const silent = page({
      os: "linux",
      data_dir: "/data",
      media: false,
      media_notice: notice,
    } as AppInfo);
    const shown = notice.replaceAll("'", "&#x27;");
    expect(silent).toContain(shown);
    expect(silent.split(shown)).toHaveLength(2);
    expect(silent).not.toContain("A short sound when a chat finishes");
    expect(silent).toMatch(/aria-label="Sounds"[^>]*disabled=""/);
    // Every other switch stays usable.
    expect(silent.match(/disabled=""/g)).toHaveLength(1);
    const playing = page({
      os: "linux",
      data_dir: "/data",
      media: true,
      media_notice: null,
    } as AppInfo);
    expect(playing).toContain("A short sound when a chat finishes, fails or needs you.");
    expect(playing).not.toContain('disabled=""');
  });
});
