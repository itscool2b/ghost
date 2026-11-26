import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { createFrameClock } from "../frameClock";
import { orbitClock, createFrameClock as orbitCreateFrameClock } from "../orbit/clock";
import { CopyButton as OrbitCopyButton } from "../orbit/index";
import { CopyButton } from "./CopyButton";
import graphiteCss from "./graphite.css?raw";
import * as graphite from "./index";
import { Keycap } from "./Keycap";
import { Logo, Wordmark } from "./Logo";
import { LOGO_PATH, LOGO_SMALL_PATH } from "./logoPaths";
import { PixelLoader } from "./PixelLoader";
import { StatusDot } from "./StatusDot";

/*
 * The website prerenders with React Router, so every component must render on a server: no window
 * and no document. These run in Node, where touching either throws.
 */
describe("server rendering", () => {
  it("renders the logo in currentColor, decorative unless named, small variant at 40px and below", () => {
    const html = renderToString(createElement(Logo, { size: 16 }));
    expect(html).toContain('fill="currentColor"');
    expect(html).toContain('fill-rule="evenodd"');
    expect(html).toContain('aria-hidden="true"');
    // The logo is far wider than tall: the size is its width, and the height follows.
    expect(html).toContain('width="16" height="7"');
    expect(html).toContain(LOGO_SMALL_PATH);
    const named = renderToString(createElement(Logo, { size: 64, label: "auracode" }));
    expect(named).toContain('width="64" height="27"');
    expect(named).toContain('role="img"');
    expect(named).toContain('aria-label="auracode"');
    expect(named).toContain(LOGO_PATH);
  });

  it("sets the wordmark as real text after a logo not quite twice as wide as the type is tall", () => {
    const html = renderToString(createElement(Wordmark, { size: 16 }));
    expect(html).toContain('<span class="g-wordmark-text">Ghost</span>');
    expect(html).toContain("font-size:16px");
    expect(html).toContain('width="30"');
    expect(renderToString(createElement(Wordmark, { size: 32 }))).toContain('width="61"');
    expect(renderToString(createElement(Wordmark, { mark: false }))).not.toContain("<svg");
  });

  it("renders status dots as circles and live ones as the loader", () => {
    const dot = renderToString(createElement(StatusDot, { tone: "volt" }));
    expect(dot).toContain('data-tone="volt"');
    expect(dot).toContain('aria-hidden="true"');
    const labelled = renderToString(createElement(StatusDot, { tone: "red", label: "Failed" }));
    expect(labelled).toContain('role="img"');
    const live = renderToString(createElement(StatusDot, { tone: "green", live: true }));
    expect(live).toContain("g-loader g-status-live");
    expect(live.match(/<rect/g)).toHaveLength(6);
    expect(live).toContain("g-px-green");
    // Every still status dot is a flat circle, the owner's call on 2026-10-28.
    expect(graphiteCss).toMatch(/.g-status-dot {[^}]*border-radius: 50%;/);
  });

  it("renders a loader's six cells at their static pattern", () => {
    const html = renderToString(createElement(PixelLoader, { size: 16, label: "Working" }));
    expect(html.match(/<rect/g)).toHaveLength(6);
    expect(html).toContain('aria-label="Working"');
  });

  it("renders keys as kbd, and key symbols as drawn glyphs with their names", () => {
    expect(renderToString(createElement(Keycap, {}, "K"))).toBe('<kbd class="g-keycap">K</kbd>');
    // Geist Mono has no ⌘ or ↵, so the cap draws them and names them for screen readers, as hidden
    // text: a kbd may not carry aria-label.
    const command = renderToString(createElement(Keycap, {}, "⌘"));
    expect(command).toContain('<svg class="g-keycap-glyph"');
    expect(command).toContain('<span class="g-keycap-name">Command</span>');
    expect(command).not.toContain("⌘");
    expect(command).not.toContain("aria-label");
    expect(renderToString(createElement(Keycap, {}, "↵"))).toContain(">Enter</span>");
    // A label the caller gives wins over the symbol's own name, and names a plain key too.
    expect(renderToString(createElement(Keycap, { label: "Return" }, "↵"))).toContain(
      ">Return</span>",
    );
    expect(renderToString(createElement(Keycap, { label: "Control" }, "Ctrl"))).toContain(
      '<span aria-hidden="true" class="g-keycap-shown">Ctrl</span>',
    );
  });

  it("styles the copy button with Graphite classes and keeps the Orbit hooks", () => {
    const html = renderToString(createElement(CopyButton, { text: "pnpm test", caption: "Copy" }));
    expect(html).toContain('class="g-copy o-copy"');
    expect(html).toContain('aria-label="Copy"');
    expect(html).toContain('data-state="ready"');
    expect(OrbitCopyButton).toBe(CopyButton);
  });
});

describe("module surface", () => {
  it("shares one frame clock between Orbit and Graphite", () => {
    expect(orbitCreateFrameClock).toBe(createFrameClock);
    expect(graphite.frameClock).toBe(orbitClock);
  });

  it("exports the components and helpers the website and desktop build on", () => {
    for (const name of [
      "AgentMark",
      "CopyButton",
      "Keycap",
      "PixelLoader",
      "Logo",
      "StatusDot",
      "Wordmark",
      "agentCode",
      "logoSvg",
      "LOGO_PATH",
      "GRAPHITE_COLORS",
      "contrastRatio",
    ]) {
      expect(graphite, name).toHaveProperty(name);
    }
  });

  it("no longer ships the dither field, the water or its swimmers (no animal or water motifs)", () => {
    for (const name of [
      "DitherField",
      "renderDitherFrame",
      "paintSprites",
      "DITHER_PALETTES",
      "Shallows",
      "Swimmers",
      "Creature",
      "swimPose",
    ]) {
      expect(graphite, name).not.toHaveProperty(name);
    }
    for (const retired of ["g-dither", "g-shallows", "g-swimmer", "g-creature", "--g-water"]) {
      expect(graphiteCss).not.toContain(retired);
    }
  });
});
