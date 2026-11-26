import { afterEach, describe, expect, it, vi } from "vitest";
import { createAppearanceController, isAppearance, isTheme } from "./appearance";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

function system(dark: boolean) {
  const target = Object.assign(new EventTarget(), { matches: dark });
  return {
    media: target as unknown as MediaQueryList,
    change: (next: boolean) => {
      target.matches = next;
      target.dispatchEvent(new Event("change"));
    },
  };
}

describe("appearance preferences", () => {
  it("uses preview overrides before rendering, persists an explicit preview change and rejects stale snapshots", async () => {
    vi.resetModules();
    vi.stubEnv("DEV", true);
    const os = system(false);
    const localStorage = { getItem: vi.fn(() => "light"), setItem: vi.fn() };
    const root = { dataset: {}, style: {} };
    vi.stubGlobal("window", {
      location: { search: "?preview=settings&theme=dark" },
      localStorage,
      matchMedia: () => os.media,
    });
    vi.stubGlobal("document", { documentElement: root });
    const appearance = await import("./appearance");
    appearance.initializeAppearance();
    expect(root).toEqual({
      dataset: { theme: "dark" },
      style: { colorScheme: "dark" },
    });
    appearance.applySetupAppearance({ revision: 4, appearance: "light" });
    appearance.applySetupAppearance({ revision: 3, appearance: "dark" });
    expect(appearance.getAppearancePreference()).toBe("light");
    expect(localStorage.setItem).toHaveBeenCalledWith("auracode.preview.appearance", "light");
  });

  it("never restores or persists another profile's appearance through browser storage", async () => {
    vi.resetModules();
    const os = system(false);
    const localStorage = { getItem: vi.fn(() => "dark"), setItem: vi.fn() };
    const root = { dataset: {}, style: {} };
    vi.stubGlobal("window", { location: { search: "" }, localStorage, matchMedia: () => os.media });
    vi.stubGlobal("document", { documentElement: root });
    const appearance = await import("./appearance");
    appearance.initializeAppearance();
    expect(appearance.getAppearancePreference()).toBe("system");
    // The first frame wears the default theme, Main, before the profile answers: Main is the
    // base palette, so it names none.
    expect(root.dataset).toEqual({ theme: "light" });
    appearance.applySetupAppearance({ revision: 0, appearance: "dark" });
    expect(root.dataset).toEqual({ theme: "dark" });
    expect(localStorage.getItem).not.toHaveBeenCalled();
    expect(localStorage.setItem).not.toHaveBeenCalled();
  });
  it("applies the saved theme as a palette beside the appearance, Main as none, and ignores stale snapshots", async () => {
    vi.resetModules();
    const os = system(true);
    const localStorage = { getItem: vi.fn(() => "claude"), setItem: vi.fn() };
    const root: { dataset: Record<string, string>; style: object } = { dataset: {}, style: {} };
    vi.stubGlobal("window", { location: { search: "" }, localStorage, matchMedia: () => os.media });
    vi.stubGlobal("document", { documentElement: root });
    const appearance = await import("./appearance");
    const seen: string[] = [];
    appearance.subscribeTheme(() => seen.push(appearance.getThemePreference()));
    // Production starts on the default, Main, and never reads a theme from storage.
    expect(appearance.getThemePreference()).toBe("aura");
    expect(root.dataset).toEqual({ theme: "dark" });
    appearance.applySetupAppearance({ revision: 2, appearance: "light", theme: "grok" });
    expect(root.dataset).toEqual({ theme: "light", palette: "grok" });
    appearance.applySetupAppearance({ revision: 1, appearance: "dark", theme: "codex" });
    expect(root.dataset).toEqual({ theme: "light", palette: "grok" });
    // A theme this build lacks, or none at all, reads as the default.
    appearance.applySetupAppearance({
      revision: 3,
      appearance: "light",
      theme: "future_agent" as never,
    });
    expect(root.dataset).toEqual({ theme: "light" });
    appearance.setThemePreference("oh_my_pi");
    expect(root.dataset.palette).toBe("oh_my_pi");
    appearance.applySetupAppearance({ revision: 4, appearance: "system" });
    expect(root.dataset).toEqual({ theme: "dark" });
    // A saved agent theme is kept, and names its palette.
    appearance.applySetupAppearance({ revision: 5, appearance: "system", theme: "claude" });
    expect(root.dataset).toEqual({ theme: "dark", palette: "claude" });
    expect(seen).toEqual(["grok", "aura", "oh_my_pi", "aura", "claude"]);
    expect(appearance.getResolvedAppearance()).toBe("dark");
    expect(localStorage.getItem).not.toHaveBeenCalled();
    expect(localStorage.setItem).not.toHaveBeenCalled();
  });

  it("previews a theme from the query or the last one chosen, and keeps a new choice", async () => {
    vi.resetModules();
    vi.stubEnv("DEV", true);
    const os = system(false);
    const stored = new Map([["auracode.preview.palette", "hermes"]]);
    const localStorage = {
      getItem: vi.fn((key: string) => stored.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => void stored.set(key, value)),
    };
    const root: { dataset: Record<string, string>; style: object } = { dataset: {}, style: {} };
    const start = async (search: string) => {
      vi.resetModules();
      root.dataset = {};
      vi.stubGlobal("window", { location: { search }, localStorage, matchMedia: () => os.media });
      vi.stubGlobal("document", { documentElement: root });
      const appearance = await import("./appearance");
      appearance.initializeAppearance();
      return appearance;
    };
    expect((await start("?preview=settings")).getThemePreference()).toBe("hermes");
    expect(root.dataset).toEqual({ theme: "light", palette: "hermes" });
    const queried = await start("?preview=plan&palette=pi");
    expect(queried.getThemePreference()).toBe("pi");
    expect(root.dataset.palette).toBe("pi");
    queried.rememberPreviewTheme("cursor");
    expect(stored.get("auracode.preview.palette")).toBe("cursor");
    expect((await start("?preview=settings&palette=nope")).getThemePreference()).toBe("cursor");
  });

  it("sets the chat's text size from the profile, Compact as none, and previews Comfortable", async () => {
    vi.resetModules();
    const os = system(false);
    const root: { dataset: Record<string, string>; style: object } = { dataset: {}, style: {} };
    const localStorage = { getItem: vi.fn(() => null), setItem: vi.fn() };
    vi.stubGlobal("window", { location: { search: "" }, localStorage, matchMedia: () => os.media });
    vi.stubGlobal("document", { documentElement: root });
    const appearance = await import("./appearance");
    appearance.initializeAppearance();
    expect(appearance.getTextSizePreference()).toBe("compact");
    expect(root.dataset.textSize).toBeUndefined();
    appearance.applySetupAppearance({ revision: 1, appearance: "dark", text_size: "comfortable" });
    expect(root.dataset.textSize).toBe("comfortable");
    expect(appearance.getTextSizePreference()).toBe("comfortable");
    // A profile without the setting, or an older snapshot, reads as Compact.
    appearance.applySetupAppearance({ revision: 2, appearance: "dark" });
    expect(root.dataset.textSize).toBeUndefined();
    expect(appearance.getTextSizePreference()).toBe("compact");
    // Development previews take `&textSize=comfortable`.
    vi.resetModules();
    vi.stubEnv("DEV", true);
    root.dataset = {};
    vi.stubGlobal("window", {
      location: { search: "?preview=settled&textSize=comfortable" },
      localStorage,
      matchMedia: () => os.media,
    });
    const preview = await import("./appearance");
    preview.initializeAppearance();
    expect(root.dataset.textSize).toBe("comfortable");
    expect(preview.getTextSizePreference()).toBe("comfortable");
  });

  it("names every theme it knows and nothing else", () => {
    for (const valid of ["aura", "claude", "codex", "grok", "pi", "antigravity", "cursor"])
      expect(isTheme(valid)).toBe(true);
    for (const valid of ["opencode", "oh_my_pi", "hermes"]) expect(isTheme(valid)).toBe(true);
    for (const invalid of [null, "", "oh-my-pi", "Claude", "system", {}, 1])
      expect(isTheme(invalid)).toBe(false);
  });

  it("follows OS changes only while System is selected, including returning to System", () => {
    const os = system(false);
    const apply = vi.fn();
    const persist = vi.fn();
    const appearance = createAppearanceController(os.media, "system", apply, persist);
    expect(apply).toHaveBeenLastCalledWith("light");
    os.change(true);
    expect(apply).toHaveBeenLastCalledWith("dark");
    appearance.set("light");
    os.change(false);
    os.change(true);
    expect(apply).toHaveBeenCalledTimes(3);
    expect(appearance.resolved()).toBe("light");
    appearance.set("system");
    expect(apply).toHaveBeenLastCalledWith("dark");
    expect(persist.mock.calls).toEqual([["light"], ["system"]]);
    appearance.dispose();
  });

  it("notifies live views after the root theme changes and releases listeners", () => {
    const os = system(true);
    let rootTheme = "";
    const appearance = createAppearanceController(os.media, "dark", (value) => {
      rootTheme = value;
    });
    const observed: string[] = [];
    const unsubscribe = appearance.subscribe(() => observed.push(rootTheme));
    appearance.set("light");
    expect(observed).toEqual(["light"]);
    unsubscribe();
    appearance.set("system");
    expect(observed).toEqual(["light"]);
    appearance.dispose();
    os.change(false);
    expect(rootTheme).toBe("dark");
  });

  it("resolves a newly opened profile independently from the previous profile", () => {
    const os = system(false);
    const previous = createAppearanceController(os.media, "dark", () => {});
    previous.dispose();
    const apply = vi.fn();
    const next = createAppearanceController(os.media, "system", apply);
    expect(next.preference()).toBe("system");
    expect(apply).toHaveBeenLastCalledWith("light");
    next.dispose();
    for (const invalid of [null, "", "auto", "DARK", {}, 1])
      expect(isAppearance(invalid)).toBe(false);
  });
});
