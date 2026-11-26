import { type AgentBrandId, getBrand } from "../brand";
import antigravity from "./agent-logos/antigravity.svg?raw";
import claudeCode from "./agent-logos/claude-code.svg?raw";
import codex from "./agent-logos/codex.svg?raw";
import cursor from "./agent-logos/cursor.svg?raw";
import grok from "./agent-logos/grok.svg?raw";
import hermes from "./agent-logos/hermes.svg?raw";
import ohMyPi from "./agent-logos/oh-my-pi.svg?raw";
import opencode from "./agent-logos/opencode.svg?raw";
import pi from "./agent-logos/pi.svg?raw";

/*
 * The coding agents' own marks, as their makers publish them (third-party/brand-marks.md records
 * each source, guideline and term). Each file is the vendor's shapes, flattened to plain path,
 * circle, rect, ellipse or polygon elements with no raster, script, style, id or reference, and
 * names its agent. The marks are trademarks of their owners, are not covered by Ghost's
 * license, and only say which agent a chat uses. An agent without one (a custom ACP agent) keeps
 * its two-letter code (AgentMark).
 */
type LogoId = Exclude<AgentBrandId, "generic">;

const SOURCES: Readonly<Partial<Record<LogoId, string>>> = {
  "claude-code": claudeCode,
  codex,
  antigravity,
  grok,
  hermes,
  cursor,
  opencode,
  pi,
  "oh-my-pi": ohMyPi,
};

/**
 * Optical size within the logo box, so a compact symbol and one that fills its square read as the
 * same weight in a row. Uniform scaling only: a mark is never stretched, cropped or redrawn.
 */
const SCALE: Readonly<Partial<Record<LogoId, number>>> = {
  "claude-code": 0.96,
  antigravity: 0.9,
  cursor: 0.94,
  opencode: 0.88,
  pi: 0.8,
};

/**
 * Marks their makers show turned: the angle in degrees about the viewBox centre, and the viewBox of
 * the turned artwork (measured from its ink). Nous Research's site shows the Hermes wing at 45
 * degrees, so it is drawn that way too; the file keeps the wing upright, as published.
 */
const TURN: Readonly<Partial<Record<LogoId, { angle: number; frame: string }>>> = {
  hermes: { angle: 45, frame: "-18.15 23.8 78.11 33.64" },
};

export type LogoShapeTag = "path" | "circle" | "rect" | "ellipse" | "polygon";

export interface LogoShape {
  readonly tag: LogoShapeTag;
  /** The element's attributes as written in the file (`fill-rule`, not `fillRule`). */
  readonly attributes: Readonly<Record<string, string>>;
}

export interface AgentLogo {
  readonly id: LogoId;
  readonly name: string;
  readonly viewBox: string;
  readonly shapes: readonly LogoShape[];
  /** Optical size within the logo box, above 0.5 and at most 1. */
  readonly scale: number;
  /** A mark its maker shows turned: the angle about the viewBox centre and the turned frame. */
  readonly turn?: { readonly angle: number; readonly frame: string };
}

/** The stored SVG files by brand id, exactly as committed. */
export const AGENT_LOGO_SOURCES: Readonly<Partial<Record<LogoId, string>>> = SOURCES;

const ROOT = /^<svg\b([^>]*)>([\s\S]*)<\/svg>\s*$/;
const TITLE = /<title>[^<]*<\/title>/g;
const SHAPE = /<(path|circle|rect|ellipse|polygon)\b([^>]*?)\/>/g;
const ATTRIBUTE = /([a-zA-Z][a-zA-Z-]*)="([^"]*)"/g;

function attributesOf(text: string): Record<string, string> {
  const found: Record<string, string> = {};
  for (const [, name, value] of text.matchAll(ATTRIBUTE)) if (name) found[name] = value ?? "";
  return found;
}

/**
 * Reads a stored mark: its viewBox and its flat list of shapes. Anything else inside the root
 * (groups, gradients, text, comments) is refused, so what is drawn is exactly what is reviewed.
 */
export function parseLogo(svg: string): { viewBox: string; shapes: LogoShape[] } {
  const root = ROOT.exec(svg.trim());
  if (!root) throw new Error("Not a single <svg> element");
  const viewBox = attributesOf(root[1] ?? "").viewBox;
  if (!viewBox || !/^-?[\d.]+ -?[\d.]+ [\d.]+ [\d.]+$/.test(viewBox))
    throw new Error("Missing or unusual viewBox");
  const body = root[2] ?? "";
  const rest = body.replace(TITLE, "").replace(SHAPE, "").trim();
  if (rest) throw new Error(`Unexpected content: ${rest.slice(0, 40)}`);
  const shapes = [...body.matchAll(SHAPE)].map(([, tag, attributes]) => ({
    tag: tag as LogoShapeTag,
    attributes: attributesOf(attributes ?? ""),
  }));
  if (shapes.length === 0) throw new Error("No shapes");
  return { viewBox, shapes };
}

/** Every agent with a usable published mark, by brand id. */
export const AGENT_LOGOS: Readonly<Partial<Record<LogoId, AgentLogo>>> = Object.fromEntries(
  (Object.entries(SOURCES) as [LogoId, string][]).map(([id, svg]) => {
    const { viewBox, shapes } = parseLogo(svg);
    const name = getBrand(id).name;
    const turn = TURN[id];
    return [id, { id, name, viewBox, shapes, scale: SCALE[id] ?? 1, ...(turn ? { turn } : {}) }];
  }),
);

/** The agent's published mark, by id or alias ("claude", "Claude Code"); null when it has none. */
export function agentLogo(agent: string): AgentLogo | null {
  const brand = getBrand(agent);
  if (brand.id === "generic") return null;
  return AGENT_LOGOS[brand.id] ?? null;
}
