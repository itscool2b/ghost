/**
 * The Graphite values code needs as numbers or hex strings: build steps rasterise brand images from
 * them (the website social card), and contrast is checked in tests. tokens.css is the source of
 * truth for styling; tokens.test.ts fails if the two drift.
 */

/** Solid colours, as lowercase `#rrggbb`, named after their `--g-*` custom property. */
export const GRAPHITE_COLORS = {
  bg: "#131313",
  "surface-1": "#171717",
  "surface-2": "#1b1b1b",
  "surface-3": "#212121",
  "surface-4": "#272727",
  "line-faint": "#1f1f1f",
  line: "#282828",
  "line-strong": "#6b6b6b",
  "fg-1": "#f5f5f5",
  "fg-2": "#ababab",
  "fg-3": "#909090",
  "fg-4": "#666666",
  "on-light": "#131313",
  primary: "#f5f5f5",
  "primary-hover": "#e0e0e0",
  "primary-active": "#cccccc",
  volt: "#d4d4d4",
  green: "#3dd68c",
  red: "#ff5c5c",
  "red-hover": "#ff7a7a",
  "diff-hunk": "#0f0f0f",
  selection: "#333333",
  focus: "#d4d4d4",
  "code-fg": "#d4d4d4",
  "code-bright": "#ededed",
  "code-dim": "#858585",
  "code-keyword": "#ff8f8f",
  "code-string": "#8fb8ff",
  "code-number": "#c7a6ff",
  "code-function": "#ededed",
  "ansi-black": "#4d4d4d",
  "ansi-red": "#ff6b6b",
  "ansi-green": "#3dd68c",
  "ansi-yellow": "#e6cc6e",
  "ansi-blue": "#7aa7ff",
  "ansi-magenta": "#d69cff",
  "ansi-cyan": "#5cd6d6",
  "ansi-white": "#d4d4d4",
  "ansi-bright-black": "#858585",
  "ansi-bright-red": "#ff8f8f",
  "ansi-bright-green": "#6ee7a8",
  "ansi-bright-yellow": "#f2dc8c",
  "ansi-bright-blue": "#a3c2ff",
  "ansi-bright-magenta": "#e4bdff",
  "ansi-bright-cyan": "#8ae6e6",
  "ansi-bright-white": "#ededed",
  "project-slate": "#8a98a6",
  "project-sand": "#a59e92",
  "project-sage": "#8c9c88",
  "project-clay": "#a88e80",
  "shade-near": "#080808",
  "shade-far": "#121212",
  hover: "#1f1f1f",
  selected: "#272727",
  outline: "#6b6b6b",
  overlay: "#1b1b1b",
  warning: "#e6cc6e",
  "warning-tint": "#2a2517",
  "green-tint": "#14251b",
  "red-tint": "#2d191d",
  "on-danger": "#18181b",
  "code-bg": "#171717",
  "terminal-bg": "#131313",
  "terminal-fg": "#d4d4d4",
  "terminal-cursor": "#f5f5f5",
  "terminal-cursor-text": "#131313",
  "terminal-selection": "#333333",
  window: "#0e0e0e",
} as const;

/** Light counterpart, with the same semantic keys. */
export const GRAPHITE_LIGHT_COLORS = {
  bg: "#ffffff",
  "surface-1": "#ffffff",
  "surface-2": "#f8f8f8",
  "surface-3": "#f3f3f3",
  "surface-4": "#ececec",
  "line-faint": "#efefef",
  line: "#e5e5e5",
  "line-strong": "#8a8a8a",
  "fg-1": "#1c1c1c",
  "fg-2": "#505050",
  "fg-3": "#676767",
  "fg-4": "#a0a0a0",
  "on-light": "#ffffff",
  primary: "#1c1c1c",
  "primary-hover": "#363636",
  "primary-active": "#000000",
  volt: "#3f3f3f",
  green: "#166534",
  red: "#b91c1c",
  "red-hover": "#991b1b",
  "diff-hunk": "#f6f6f6",
  selection: "#e1e1e1",
  focus: "#6e6e6e",
  "code-fg": "#24292f",
  "code-bright": "#18181b",
  "code-dim": "#63636d",
  "code-keyword": "#b42318",
  "code-string": "#0550ae",
  "code-number": "#6b21a8",
  "code-function": "#18181b",
  "ansi-black": "#24292f",
  "ansi-red": "#b42318",
  "ansi-green": "#166534",
  "ansi-yellow": "#7a5200",
  "ansi-blue": "#1d4ed8",
  "ansi-magenta": "#7e22ce",
  "ansi-cyan": "#0e6d79",
  "ansi-white": "#52525b",
  "ansi-bright-black": "#63636d",
  "ansi-bright-red": "#991b1b",
  "ansi-bright-green": "#14532d",
  "ansi-bright-yellow": "#854d0e",
  "ansi-bright-blue": "#1e40af",
  "ansi-bright-magenta": "#6b21a8",
  "ansi-bright-cyan": "#155e75",
  "ansi-bright-white": "#18181b",
  "project-slate": "#8a98a6",
  "project-sand": "#a59e92",
  "project-sage": "#8c9c88",
  "project-clay": "#a88e80",
  "shade-near": "#e4e4e7",
  "shade-far": "#f0f0f2",
  hover: "#f3f3f3",
  selected: "#ececec",
  outline: "#8a8a8a",
  overlay: "#ffffff",
  warning: "#92400e",
  "warning-tint": "#fff7e6",
  "green-tint": "#edf7ef",
  "red-tint": "#fef0f0",
  "on-danger": "#fafafa",
  "code-bg": "#f6f6f6",
  "terminal-bg": "#fafafa",
  "terminal-fg": "#24292f",
  "terminal-cursor": "#1c1c1c",
  "terminal-cursor-text": "#fafafa",
  "terminal-selection": "#e1e1e1",
  window: "#f5f5f5",
} as const;

export const GRAPHITE_THEMES = { dark: GRAPHITE_COLORS, light: GRAPHITE_LIGHT_COLORS } as const;

export type GraphiteColor = keyof typeof GRAPHITE_COLORS;

/** Durations in ms, mirroring `--g-dur-*` (authored; see docs/design/visual-language.md). */
export const GRAPHITE_DURATIONS = {
  glide: 240,
  bloom: 2400,
  micro: 80,
  fast: 120,
  base: 160,
  letter: 250,
  panel: 180,
  settle: 120,
  row: 240,
  dim: 120,
  step: 180,
  headline: 240,
  reveal: 240,
  count: 240,
  tick: 1100,
  shimmer: 1400,
  glint: 3000,
  cycle: 3000,
  timer: 7500,
  exit: 120,
  wave: 80,
  ink: 560,
  dissolve: 320,
  found: 1600,
  flight: 640,
  arrive: 1100,
  land: 360,
  hover: 4800,
  blink: 5600,
} as const;

/** Staggers in ms, mirroring `--g-stagger*`. */
export const GRAPHITE_STAGGERS = { word: 0, letter: 0, row: 0 } as const;

/** Cubic-bezier control points, mirroring `--g-ease*`. */
export const GRAPHITE_EASINGS = {
  ease: [0.2, 0, 0, 1],
  out: [0.2, 0, 0, 1],
  quint: [0.2, 0, 0, 1],
  smooth: [0.2, 0, 0, 1],
  settle: [0.2, 0, 0, 1],
  soft: [0.2, 0, 0, 1],
  step: [0.2, 0, 0, 1],
  "in-out": [0.2, 0, 0, 1],
  reveal: [0.2, 0, 0, 1],
  in: [0.2, 0, 0, 1],
  glide: [0.32, 0.72, 0, 1],
  expo: [0.16, 1, 0.3, 1],
} as const satisfies Record<string, readonly [number, number, number, number]>;

function channel(value: number): number {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance of a `#rrggbb` colour; NaN for anything else. */
export function relativeLuminance(hex: string): number {
  const match = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!match?.[1]) return Number.NaN;
  const n = Number.parseInt(match[1], 16);
  return (
    0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255)
  );
}

/** WCAG contrast ratio between two `#rrggbb` colours, from 1 to 21. */
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
