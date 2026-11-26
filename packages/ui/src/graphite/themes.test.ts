import { describe, expect, it } from "vitest";
import { THEME_IDS, THEMES, themesCss, themeTokens } from "./themes";
import themesFile from "./themes.css?raw";
import themesSource from "./themes.ts?raw";
import { contrastRatio, GRAPHITE_COLORS, GRAPHITE_LIGHT_COLORS, relativeLuminance } from "./tokens";
import tokensCss from "./tokens.css?raw";

function declarations(css: string): Map<string, string> {
  const found = new Map<string, string>();
  for (const match of css.matchAll(/(--g-[\w-]+)\s*:\s*([^;]+);/g)) {
    found.set(match[1] ?? "", (match[2] ?? "").replace(/\s+/g, " ").trim());
  }
  return found;
}

const REDUCED = "@media (prefers-reduced-motion: reduce)";
const base = {
  dark: declarations(
    (tokensCss.split(REDUCED)[0] ?? "").split(':root[data-theme="light"]')[0] ?? "",
  ),
  light: declarations(tokensCss.match(/:root\[data-theme="light"\] \{([^}]+)\}/)?.[1] ?? ""),
};
const BASE_COLORS = { dark: GRAPHITE_COLORS, light: GRAPHITE_LIGHT_COLORS } as const;

function channels(value: string): readonly [number, number, number, number] {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex?.[1]) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgb = /^rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)$/.exec(value);
  if (!rgb) throw new Error(`not a #rrggbb or rgb(r g b / a) colour: ${value}`);
  return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), Number(rgb[4])];
}

function over(top: string, under: string): string {
  const [r, g, b, a] = channels(top);
  const [ur, ug, ub] = channels(under);
  return `#${[
    [r, ur],
    [g, ug],
    [b, ub],
  ]
    .map(([t = 0, u = 0]) =>
      Math.round(t * a + u * (1 - a))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

/** Every theme in both appearances: its own values over Graphite's. */
const CASES = THEMES.flatMap((theme) =>
  (["dark", "light"] as const).map((appearance) => {
    const own = themeTokens(theme[appearance], appearance);
    const all = new Map(base[appearance]);
    for (const [name, value] of Object.entries(own)) all.set(`--g-${name}`, value);
    const color = (name: string) => {
      const value = all.get(`--g-${name}`);
      if (value === undefined) throw new Error(`--g-${name} is missing`);
      return value;
    };
    return { label: `${theme.name} ${appearance}`, appearance, own, color };
  }),
);

describe("Desktop themes", () => {
  it("lists every theme once, Aura first, with a name and an agent", () => {
    expect(THEME_IDS[0]).toBe("aura");
    expect(THEMES.map((theme) => theme.id)).toEqual(THEME_IDS.slice(1));
    for (const theme of THEMES) {
      expect(theme.name.length, theme.id).toBeGreaterThan(1);
      expect(theme.agent.length, theme.id).toBeGreaterThan(1);
    }
  });

  it("writes themes.css from themes.ts", () => {
    // Out of date? Run `node packages/ui/scripts/write-themes.mts`.
    expect(themesFile.replace(/\r\n/g, "\n")).toBe(themesCss());
  });

  it("sets only tokens Graphite declares, with plain colour values", () => {
    for (const { label, own, appearance } of CASES) {
      for (const [name, value] of Object.entries(own)) {
        expect(base[appearance].has(`--g-${name}`), `${label}: --g-${name}`).toBe(true);
        expect(value, `${label}: --g-${name}`).toMatch(
          /^(#[0-9a-f]{6}|rgb\(\d+ \d+ \d+ \/ 0?\.\d+\))$/,
        );
      }
    }
  });

  it("keeps text readable on the page and every surface", () => {
    for (const { label, appearance, color } of CASES) {
      const surfaces = ["bg", "surface-1", "surface-2", "surface-3", "surface-4"];
      const texts =
        appearance === "light"
          ? ["fg-1", "fg-2", "fg-3", "volt", "green", "red", "warning"]
          : ["fg-1", "fg-2", "volt", "green", "red"];
      for (const surface of surfaces)
        for (const fg of texts)
          expect(
            contrastRatio(color(fg), color(surface)),
            `${label}: ${fg} on ${surface}`,
          ).toBeGreaterThanOrEqual(4.5);
      for (const surface of ["bg", "surface-1", "surface-2", "surface-3"])
        expect(
          contrastRatio(color("fg-3"), color(surface)),
          `${label}: fg-3 on ${surface}`,
        ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("keeps outlines and focus visible, and filled controls' labels readable", () => {
    for (const { label, appearance, color } of CASES) {
      for (const surface of ["surface-1", "surface-2"])
        for (const role of ["outline", "focus"])
          expect(
            contrastRatio(color(role), color(surface)),
            `${label}: ${role} on ${surface}`,
          ).toBeGreaterThanOrEqual(3);
      for (const fill of ["primary", "primary-hover", "primary-active"])
        expect(
          contrastRatio(color("on-light"), color(fill)),
          `${label}: label on ${fill}`,
        ).toBeGreaterThanOrEqual(4.5);
      // Dark: the destructive pill takes the same dark label as the primary one.
      if (appearance === "dark")
        for (const fill of ["red", "red-hover"])
          expect(
            contrastRatio(color("on-light"), color(fill)),
            `${label}: label on ${fill}`,
          ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("keeps code readable where it is set", () => {
    for (const { label, color } of CASES) {
      for (const name of [
        "code-fg",
        "code-bright",
        "code-dim",
        "code-keyword",
        "code-string",
        "code-number",
        "code-function",
        "terminal-fg",
      ])
        expect(
          contrastRatio(color(name), color("code-bg")),
          `${label}: ${name} on code-bg`,
        ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("orders the dark surface and text ladders", () => {
    for (const { label, appearance, color } of CASES) {
      if (appearance !== "dark") continue;
      const lum = (name: string) => relativeLuminance(color(name));
      const ladder = ["bg", "surface-1", "surface-2", "surface-3", "surface-4"];
      for (let i = 1; i < ladder.length; i++)
        expect(lum(ladder[i] ?? "bg"), `${label}: ${ladder[i]}`).toBeGreaterThan(
          lum(ladder[i - 1] ?? "bg"),
        );
      expect(lum("fg-1"), label).toBeGreaterThan(lum("fg-2"));
      expect(lum("fg-2"), label).toBeGreaterThan(lum("fg-3"));
      expect(lum("fg-3"), label).toBeGreaterThan(lum("fg-4"));
    }
  });

  it("keeps text readable on the theme's glass over the worst plausible window material", () => {
    for (const { label, appearance, color } of CASES) {
      const material = appearance === "light" ? "#d4d4d4" : "#3a3a3a";
      const tinted = over(color("window-tint"), material);
      const layer = over(color("glass-layer"), tinted);
      const card = over(color("glass-card"), layer);
      const bases = {
        tinted,
        layer,
        card,
        "control on card": over(color("glass-control"), card),
        bar: over(color("glass-bar"), layer),
        pop: over(color("glass-pop"), layer),
        opaque: over(color("glass-layer"), color("window")),
      };
      for (const [baseName, under] of Object.entries(bases))
        for (const fg of ["fg-1", "fg-2", "fg-3"])
          expect(
            contrastRatio(color(fg), under),
            `${label}: ${fg} on ${baseName}`,
          ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("leaves Graphite's own palette as Aura", () => {
    // Aura is the base: no themes.css block, the TypeScript mirror unchanged.
    expect(themesFile).not.toContain('data-palette="aura"');
    expect(BASE_COLORS.dark.bg).toBe("#131313");
  });

  it("contains no em dash", () => {
    const dash = String.fromCharCode(0x2014);
    expect(themesSource.includes(dash)).toBe(false);
    expect(themesFile.includes(dash)).toBe(false);
  });
});
