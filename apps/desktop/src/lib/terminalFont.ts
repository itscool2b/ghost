/**
 * The font Ghost's terminals draw with: the person's own terminal app's by default (owner, 17
 * November 2026: the terminal should be "the default terminal of the system itself"), read once
 * per run by the native side (`terminal_system_font`), or Ghost's own Geist Mono. The size
 * follows the same choice unless the person picks one. Both choices are this device's, like the
 * chat interface: fonts are installed per computer.
 */
import { commands, type SystemTerminalFont } from "../bindings";
import { inTauri, settle } from "./ipc";

export type TerminalFontChoice = "system" | "auracode";

export interface TerminalFontPreference {
  readonly font: TerminalFontChoice;
  /** CSS pixels, or `null` to follow the font choice. */
  readonly size: number | null;
}

/** Where a terminal sits: a terminal chat, a shell in the docked panel, or a tool's sheet. */
export type TerminalVariant = "chat" | "shell" | "tool";

export interface TerminalFontOptions {
  readonly fontFamily: string;
  readonly fontSize: number;
  readonly lineHeight: number;
}

export const TERMINAL_SIZES: readonly number[] = [11, 12, 13, 14, 15, 16, 18];

const KEY = "auracode.terminal-font";
const SIZE_MIN = 9;
const SIZE_MAX = 28;

/** Ghost's own sizes and line heights, as before a choice existed. */
const OWN: Record<TerminalVariant, { size: number; lineHeight: number }> = {
  chat: { size: 13.5, lineHeight: 1.35 },
  shell: { size: 12.5, lineHeight: 1.44 },
  tool: { size: 12.5, lineHeight: 1.44 },
};

/** Terminal apps set lines at about 1.2 of the font size; a chat's agent interface breathes a little. */
const SYSTEM_LINE_HEIGHT: Record<TerminalVariant, number> = { chat: 1.25, shell: 1.2, tool: 1.2 };

/** Icons that prompts and agents draw (Nerd Font symbols), after the chosen face. */
const SYMBOLS = '"Symbols Nerd Font Mono", "Symbols Nerd Font"';
const OWN_STACK = '"Geist Mono Variable", ui-monospace, monospace';

/** CSS's generic families, which must not be quoted. */
const GENERIC = new Set(["monospace", "ui-monospace"]);

export interface FontStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export function parsePreference(text: string | null): TerminalFontPreference {
  const fallback: TerminalFontPreference = { font: "system", size: null };
  if (!text) return fallback;
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value !== "object" || value === null) return fallback;
    const { font, size } = value as { font?: unknown; size?: unknown };
    return {
      font: font === "auracode" ? "auracode" : "system",
      size:
        typeof size === "number" && Number.isFinite(size) && size >= SIZE_MIN && size <= SIZE_MAX
          ? size
          : null,
    };
  } catch {
    return fallback;
  }
}

/** A family as CSS takes it: generic keywords bare, names quoted (the native side refuses quotes). */
export function cssFamily(family: string): string {
  return GENERIC.has(family) ? family : `"${family}"`;
}

/** What a terminal of `variant` draws with, from the choice and what the system said. */
export function resolveFont(
  preference: TerminalFontPreference,
  system: SystemTerminalFont | null,
  variant: TerminalVariant,
  ownStack = OWN_STACK,
): TerminalFontOptions {
  const own = OWN[variant];
  const followSystem = preference.font === "system" && system !== null;
  const family = followSystem && system.family ? system.family : null;
  const fontFamily = family ? `${cssFamily(family)}, ${SYMBOLS}, ${ownStack}` : ownStack;
  const systemSize = followSystem ? system.size_px : null;
  const fontSize = preference.size ?? systemSize ?? own.size;
  const lineHeight = followSystem ? SYSTEM_LINE_HEIGHT[variant] : own.lineHeight;
  return { fontFamily, fontSize, lineHeight };
}

/** The choice and the system's font as an external store for React and the open terminals. */
export class TerminalFonts {
  readonly #storage: FontStorage | null;
  readonly #listeners = new Set<() => void>();
  readonly #fetch: () => Promise<SystemTerminalFont | null>;
  #version = 0;
  #system: SystemTerminalFont | null = null;
  #loading: Promise<SystemTerminalFont | null> | null = null;

  constructor(storage: FontStorage | null, fetch: () => Promise<SystemTerminalFont | null>) {
    this.#storage = storage;
    this.#fetch = fetch;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Changes whenever the choice or the system's font does, for useSyncExternalStore. */
  readonly getVersion = (): number => this.#version;

  preference(): TerminalFontPreference {
    try {
      return parsePreference(this.#storage?.getItem(KEY) ?? null);
    } catch {
      return parsePreference(null);
    }
  }

  set(next: TerminalFontPreference): void {
    const clean = parsePreference(JSON.stringify(next));
    try {
      if (clean.font === "system" && clean.size === null) this.#storage?.removeItem(KEY);
      else this.#storage?.setItem(KEY, JSON.stringify(clean));
    } catch {
      /* A blocked storage keeps the default: the system's font. */
    }
    this.#changed();
  }

  /** What the person's terminal app uses, once read; `null` until then or when unknown. */
  system(): SystemTerminalFont | null {
    return this.#system;
  }

  /** Reads the system's font once per run; open terminals repaint when it arrives. */
  load(): Promise<SystemTerminalFont | null> {
    this.#loading ??= this.#fetch()
      .catch(() => null)
      .then((system) => {
        this.#system = system;
        if (system) this.#changed();
        return system;
      });
    return this.#loading;
  }

  resolve(variant: TerminalVariant, ownStack?: string): TerminalFontOptions {
    return resolveFont(this.preference(), this.#system, variant, ownStack);
  }

  #changed(): void {
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
  }
}

export const terminalFonts = new TerminalFonts(
  typeof localStorage === "undefined" ? null : localStorage,
  () => (inTauri() ? settle(commands.terminalSystemFont()) : Promise.resolve(null)),
);
