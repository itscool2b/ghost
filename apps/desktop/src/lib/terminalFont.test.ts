import { describe, expect, it, vi } from "vitest";
import type { SystemTerminalFont } from "../bindings";
import { cssFamily, parsePreference, resolveFont, TerminalFonts } from "./terminalFont";

const KITTY: SystemTerminalFont = {
  family: "JetBrainsMono Nerd Font",
  size_px: 14.67,
  size_points: 11,
  source: "kitty",
};
const OWN = '"Geist Mono Variable", monospace';

function memory(initial: Record<string, string> = {}) {
  const items = new Map(Object.entries(initial));
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
    removeItem: (key: string) => void items.delete(key),
  };
}

describe("the terminals' font", () => {
  it("follows the person's terminal app by default, its symbols and Ghost's face behind it", () => {
    const font = resolveFont({ font: "system", size: null }, KITTY, "chat", OWN);
    expect(font).toEqual({
      fontFamily: `"JetBrainsMono Nerd Font", "Symbols Nerd Font Mono", "Symbols Nerd Font", ${OWN}`,
      fontSize: 14.67,
      lineHeight: 1.25,
    });
    expect(resolveFont({ font: "system", size: null }, KITTY, "shell", OWN).lineHeight).toBe(1.2);
  });

  it("keeps Ghost's own look until the system answers, or when chosen", () => {
    for (const [preference, system] of [
      [{ font: "system", size: null }, null],
      [{ font: "auracode", size: null }, KITTY],
    ] as const) {
      expect(resolveFont(preference, system, "chat", OWN)).toEqual({
        fontFamily: OWN,
        fontSize: 13.5,
        lineHeight: 1.35,
      });
      expect(resolveFont(preference, system, "tool", OWN).fontSize).toBe(12.5);
    }
  });

  it("takes a chosen size over the system's, and the system's family without a size", () => {
    expect(resolveFont({ font: "system", size: 16 }, KITTY, "shell", OWN).fontSize).toBe(16);
    const sizeless = { ...KITTY, size_px: null, size_points: null };
    expect(resolveFont({ font: "system", size: null }, sizeless, "shell", OWN).fontSize).toBe(12.5);
    const familyless = { ...KITTY, family: null };
    expect(resolveFont({ font: "system", size: null }, familyless, "chat", OWN).fontFamily).toBe(
      OWN,
    );
  });

  it("quotes names and leaves CSS's generic families bare", () => {
    expect(cssFamily("Fira Code")).toBe('"Fira Code"');
    expect(cssFamily("ui-monospace")).toBe("ui-monospace");
  });

  it("reads only sane stored choices", () => {
    expect(parsePreference(null)).toEqual({ font: "system", size: null });
    expect(parsePreference("{")).toEqual({ font: "system", size: null });
    expect(parsePreference('{"font":"auracode","size":14}')).toEqual({
      font: "auracode",
      size: 14,
    });
    for (const size of [2, 99, "14", null])
      expect(parsePreference(JSON.stringify({ font: "system", size })).size).toBeNull();
  });

  it("stores the choice on this device, asks the system once and tells open terminals", async () => {
    const storage = memory();
    const fetch = vi.fn(async () => KITTY);
    const fonts = new TerminalFonts(storage, fetch);
    const listener = vi.fn();
    fonts.subscribe(listener);
    await Promise.all([fonts.load(), fonts.load()]);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fonts.system()).toEqual(KITTY);
    expect(listener).toHaveBeenCalledTimes(1);
    fonts.set({ font: "auracode", size: 15 });
    expect(storage.items.get("auracode.terminal-font")).toBe('{"font":"auracode","size":15}');
    expect(fonts.resolve("chat", OWN)).toEqual({ fontFamily: OWN, fontSize: 15, lineHeight: 1.35 });
    fonts.set({ font: "system", size: null });
    expect(storage.items.has("auracode.terminal-font")).toBe(false);
    expect(listener).toHaveBeenCalledTimes(3);
  });

  it("survives a failed lookup and a blocked storage", async () => {
    const fonts = new TerminalFonts(
      {
        getItem: () => {
          throw new Error("blocked");
        },
        setItem: () => {
          throw new Error("blocked");
        },
        removeItem: () => {},
      },
      async () => {
        throw new Error("no IPC");
      },
    );
    expect(await fonts.load()).toBeNull();
    expect(fonts.preference()).toEqual({ font: "system", size: null });
    expect(() => fonts.set({ font: "auracode", size: null })).not.toThrow();
  });
});
