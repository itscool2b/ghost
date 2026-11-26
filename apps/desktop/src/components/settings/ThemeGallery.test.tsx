import { GRAPHITE_THEMES, THEME_IDS, THEMES, themeTokens } from "@ghost/ui/graphite";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ThemeGallery } from "./ThemeGallery";

const markup = (value: (typeof THEME_IDS)[number], disabled = false) =>
  renderToStaticMarkup(<ThemeGallery value={value} disabled={disabled} onChange={() => {}} />);

describe("Settings' theme gallery", () => {
  it("offers Aura, then every agent's theme, each named by the theme alone", () => {
    const html = markup("aura");
    const values = [...html.matchAll(/type="radio"[^>]*value="([^"]+)"/g)].map((match) => match[1]);
    expect(values).toEqual([...THEME_IDS]);
    const names = [...html.matchAll(/class="settings-theme-label">([^<]+)</g)].map(
      (match) => match[1],
    );
    expect(names).toEqual(["Main", ...THEMES.map((theme) => theme.name)]);
    // The mark beside each name is hidden, so a screen reader hears "Claude Code", not
    // "Claude Code, Idle Claude Code".
    expect(html.match(/class="settings-theme-mark" aria-hidden="true"/g)).toHaveLength(10);
    expect(html).toContain('<legend class="sr-only">Theme</legend>');
  });

  it("checks the chosen theme only, and disables the whole group while preferences load", () => {
    const html = markup("grok");
    expect(html.match(/checked=""/g)).toHaveLength(1);
    expect(html).toMatch(/checked="" value="grok"/);
    expect(html).not.toMatch(/<fieldset[^>]*disabled/);
    expect(markup("grok", true)).toMatch(/<fieldset class="settings-themes" disabled=""/);
  });

  it("paints each swatch from its own theme's tokens", () => {
    // Server rendering has no window, so the appearance in effect resolves to dark.
    const swatches = [
      ...markup("aura").matchAll(/class="settings-theme-swatch" style="([^"]+)"/g),
    ].map((match) => match[1] ?? "");
    expect(swatches).toHaveLength(10);
    const expected: readonly Readonly<Record<string, string>>[] = [
      GRAPHITE_THEMES.dark,
      ...THEMES.map((theme) => themeTokens(theme.dark, "dark")),
    ];
    swatches.forEach((style, index) => {
      const tokens = expected[index] ?? {};
      expect(style).toContain(`--swatch-bg:${tokens.bg}`);
      expect(style).toContain(`--swatch-card:${tokens["surface-2"]}`);
      expect(style).toContain(`--swatch-accent:${tokens.primary}`);
      expect(style).toContain(`--swatch-mark:${tokens.volt}`);
    });
  });
});
