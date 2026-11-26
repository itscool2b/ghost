/*
 * The assistants Ghost names where a bot is linked (Settings, Remote access; setup; the website):
 * personal agents that run somewhere else and reach this computer's coding agents through a bot
 * link (docs/remote-sessions.md). Any assistant that takes an MCP server can be linked; these four
 * are the ones named, each with its maker's mark (third-party/brand-marks.md).
 */
export type BotId = "grok" | "muse" | "hermes" | "openclaw";

export interface BotKind {
  readonly id: BotId;
  /** The assistant's name, as its maker writes it. */
  readonly name: string;
  readonly maker: string;
}

export const BOTS: readonly BotKind[] = [
  { id: "grok", name: "Grok Bot", maker: "xAI" },
  { id: "muse", name: "Muse", maker: "Meta" },
  { id: "hermes", name: "Hermes", maker: "Nous Research" },
  { id: "openclaw", name: "OpenClaw", maker: "OpenClaw" },
];

const NAMED: readonly [RegExp, BotId][] = [
  [/grok/i, "grok"],
  [/muse/i, "muse"],
  [/hermes/i, "hermes"],
  [/open\s*claw/i, "openclaw"],
];

/**
 * The assistant a bot link's name names, for its mark. A link records only the name its owner
 * gave it; one that names none of these keeps the plain bot glyph.
 */
export function botNamed(name: string): BotId | null {
  return NAMED.find(([pattern]) => pattern.test(name))?.[1] ?? null;
}
