import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { logoSide } from "./AgentMark";
import { BotMark } from "./BotMark";
import grokBot from "./bot-logos/grok-bot.png?inline";
import hermes from "./bot-logos/hermes.svg?raw";
import muse from "./bot-logos/muse.png?inline";
import openclaw from "./bot-logos/openclaw.svg?raw";
import { BOTS, type BotId, botNamed } from "./bots";

const mark = (bot: BotId, size?: number) =>
  renderToStaticMarkup(createElement(BotMark, size === undefined ? { bot } : { bot, size }));

/** A PNG's width and height, read from its header. */
const pngSize = (address: string) => {
  const bytes = Uint8Array.from(atob(address.split(",")[1] ?? ""), (char) => char.charCodeAt(0));
  const view = new DataView(bytes.buffer);
  expect([...bytes.slice(1, 4)].map((byte) => String.fromCharCode(byte)).join("")).toBe("PNG");
  return [view.getUint32(16), view.getUint32(20)];
};

describe("the assistants linked as bots", () => {
  it("names four, each once", () => {
    expect(BOTS.map((bot) => bot.id)).toEqual(["grok", "muse", "hermes", "openclaw"]);
    expect(new Set(BOTS.map((bot) => bot.name)).size).toBe(BOTS.length);
  });

  it("finds the assistant a link's name names, and none in a name of the owner's own", () => {
    expect(botNamed("Grok Bot")).toBe("grok");
    expect(botNamed("my grok")).toBe("grok");
    expect(botNamed("Muse")).toBe("muse");
    expect(botNamed("Hermes at home")).toBe("hermes");
    expect(botNamed("OpenClaw")).toBe("openclaw");
    expect(botNamed("open claw")).toBe("openclaw");
    expect(botNamed("Research bot")).toBeNull();
    expect(botNamed("")).toBeNull();
    // Every default name finds its own assistant.
    for (const bot of BOTS) expect(botNamed(bot.name)).toBe(bot.id);
  });

  it("shows Grok Bot, Muse and Hermes as their own app icons, each filling the tile", () => {
    const sources = new Set<string>();
    for (const bot of ["grok", "muse", "hermes"] as const) {
      const html = mark(bot, 28);
      expect(html).toContain(`data-bot="${bot}"`);
      expect(html).toContain('data-app-icon=""');
      expect(html).toContain('width="28" height="28"');
      // Never another company's mark or a coding agent's plate.
      expect(html).not.toContain("data-agent=");
      expect(html).not.toContain("<svg");
      const source = html.match(/src="([^"]+)"/)?.[1] ?? "";
      expect(source).not.toBe("");
      sources.add(source);
    }
    expect(sources.size).toBe(3);
  });

  it("keeps OpenClaw's lobster inside the tile, with the agent marks' clear space", () => {
    const html = mark("openclaw", 36);
    expect(html).not.toContain("data-app-icon");
    expect(html).toContain(`width="${logoSide(36)}" height="${logoSide(36)}"`);
    expect(html).toContain('src="data:image/svg+xml,');
  });

  it("keeps the tile between 12 and 64px and its name for a pointer", () => {
    expect(mark("grok", 4)).toContain("width:12px;height:12px");
    expect(mark("muse", 200)).toContain("width:64px;height:64px");
    expect(mark("hermes")).toContain('title="Hermes"');
    expect(mark("grok")).toContain('aria-hidden="true"');
  });

  it("stores the raster icons square and sharp at the largest tile on a 2x screen", () => {
    for (const icon of [grokBot, muse]) {
      const [width, height] = pngSize(icon);
      expect(width).toBe(height);
      expect(width).toBeGreaterThanOrEqual(128);
      expect(width).toBeLessThanOrEqual(512);
    }
  });

  it("stores Hermes' icon as a still picture that runs and loads nothing", () => {
    expect(hermes).toMatch(
      /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg" viewBox="0 0 1024 1024">/,
    );
    expect(hermes).not.toMatch(/<(script|style|animate|animateTransform|image|foreignObject|a)\b/);
    expect(hermes).not.toMatch(/\bon[a-z]+=|href=|https?:\/\/(?!www\.w3\.org\/2000\/svg)/);
    // Its own rounded corners are the only reference.
    expect(hermes.match(/url\(#[^)]+\)/g)).toEqual(["url(#icon-silhouette)"]);
  });

  it("stores OpenClaw's mark as a still picture that runs and loads nothing", () => {
    expect(openclaw).toMatch(/^<svg [^>]*role="img"[^>]*aria-label="OpenClaw"/);
    expect(openclaw).toContain("<title>OpenClaw</title>");
    // Its own gradient is the only reference; no script, style, animation, raster or link.
    expect(openclaw).not.toMatch(
      /<(script|style|animate|animateTransform|image|foreignObject|a)\b/,
    );
    expect(openclaw).not.toMatch(/\bon[a-z]+=|href=|https?:\/\/(?!www\.w3\.org\/2000\/svg)/);
    expect(openclaw.match(/url\(#[^)]+\)/g)?.every((use) => use === "url(#lobster-gradient)")).toBe(
      true,
    );
  });
});
