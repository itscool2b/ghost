import { describe, expect, it } from "vitest";
import type { SetupPreferences } from "../../bindings";
import { DEFAULT_GENERAL } from "../../lib/setup";
import { withAction } from "./preferences";
import { PAGE_GROUPS, PAGE_LABELS, SETTINGS_INDEX, searchSettings } from "./settingsIndex";

const base: SetupPreferences = {
  revision: 4,
  step: "project",
  motion: "full",
  appearance: "system",
  theme: "claude",
  text_size: "compact",
  completed: true,
  first_project_id: null,
  tour_seen: [],
  general: {
    ...DEFAULT_GENERAL,
    default_agent: "codex",
    default_model: "gpt-5.5-codex",
    default_effort: "high",
  },
};

describe("a waiting preference shows as the backend will save it", () => {
  it("clears a model and level that belonged to the previous agent, as Rust does", () => {
    const claude = withAction(base, { type: "default_agent", agent_id: "claude" });
    expect(claude.general).toMatchObject({
      default_agent: "claude",
      default_model: null,
      default_effort: null,
    });
    // The same agent again keeps both.
    expect(withAction(base, { type: "default_agent", agent_id: "codex" })).toBe(base);
    expect(
      withAction(base, { type: "default_model", model: "gpt-5.5-mini" }).general.default_effort,
    ).toBeNull();
    expect(
      withAction(base, { type: "default_model", model: "gpt-5.5-codex" }).general.default_effort,
    ).toBe("high");
  });
  it("applies every Settings change and leaves the saved object untouched", () => {
    const changed = [
      { type: "appearance", value: "dark" },
      { type: "theme", value: "codex" },
      { type: "motion", value: "reduced" },
      { type: "text_size", value: "comfortable" },
      { type: "default_autonomy", value: "ask" },
      { type: "automatic_checkpoints", enabled: true },
      { type: "notifications", enabled: false },
      { type: "sounds", enabled: true },
      { type: "editor", editor: "zed" },
      { type: "keep_awake", enabled: false },
      { type: "close_to_tray", enabled: false },
      { type: "keep_steps_open", enabled: true },
      { type: "copy_instructions_to_worktrees", enabled: false },
      { type: "agent_memory_writes", enabled: false },
      { type: "browser", enabled: false },
      { type: "browser_passwords", enabled: false },
    ] as const;
    const shown = changed.reduce(withAction, base);
    expect(shown).toMatchObject({
      appearance: "dark",
      theme: "codex",
      motion: "reduced",
      text_size: "comfortable",
    });
    expect(shown.general).toMatchObject({
      default_autonomy: "ask",
      automatic_checkpoints: true,
      notifications: false,
      sounds: true,
      editor: "zed",
      keep_awake: false,
      close_to_tray: false,
      keep_steps_open: true,
      copy_instructions_to_worktrees: false,
      agent_memory_writes: false,
      browser: false,
      browser_passwords: false,
    });
    expect(base.general.browser_passwords).toBe(true);
    expect(base.theme).toBe("claude");
  });
});

describe("Settings' search", () => {
  it("lists every page once, in its groups", () => {
    const pages = PAGE_GROUPS.flat();
    expect(new Set(pages).size).toBe(pages.length);
    expect(pages.sort()).toEqual(Object.keys(PAGE_LABELS).sort());
    for (const entry of SETTINGS_INDEX) expect(pages).toContain(entry.page);
  });
  it("finds a setting by its name, other words for it, or its page", () => {
    expect(searchSettings("theme")[0]).toMatchObject({ page: "appearance", title: "Theme" });
    expect(searchSettings("dark")[0]).toMatchObject({ page: "appearance", title: "Appearance" });
    expect(searchSettings("mcp")[0]).toMatchObject({ page: "plugins" });
    expect(searchSettings("worked for")[0]).toMatchObject({ title: "Keep steps open" });
    // A title that names it comes first; where new chats open is found by its other words too.
    expect(searchSettings("terminal")[0]).toMatchObject({ title: "Terminal font" });
    expect(searchSettings("terminal").map((entry) => entry.title)).toContain("Open new chats in");
    expect(searchSettings("tui")[0]).toMatchObject({ title: "Open new chats in" });
    expect(searchSettings("menu bar")[0]).toMatchObject({
      title: "Keep running in the background",
    });
    expect(searchSettings("mic")[0]).toMatchObject({ page: "voice" });
    expect(searchSettings("CLAUDE.md")[0]).toMatchObject({ page: "instructions" });
    expect(searchSettings("browser")[0]).toMatchObject({ page: "browser", title: "Browser use" });
    expect(searchSettings("exposed")[0]).toMatchObject({ title: "Tools for agents" });
    expect(searchSettings("passwords")[0]).toMatchObject({ title: "Save passwords" });
    expect(searchSettings("universal plugin")[0]).toMatchObject({ title: "Browser use" });
    // Every word must match; titles that start with the query come first.
    expect(searchSettings("keep")[0]?.title).toMatch(/^Keep/);
    expect(searchSettings("theme nonsense")).toEqual([]);
    expect(searchSettings("   ")).toEqual([]);
  });
});
