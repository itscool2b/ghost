/**
 * Writes the iPhone app's palettes, apps/ios/O7Kit/Sources/O7Theme/Palettes.generated.swift, from
 * the desktop's themes, never by hand:
 *   - Main (the case `aura`, its saved name) is the base palette itself: GRAPHITE_COLORS and
 *     GRAPHITE_LIGHT_COLORS (tokens.ts), plus the
 *     values only tokens.css declares (the ink ladder, tints, the brand plate). Light cascades over
 *     dark, as the stylesheet does.
 *   - Every other theme lays themeTokens() (themes.ts) over Main for each appearance, as
 *     themes.css does on the desktop.
 *   - The sent message's paper comes from apps/desktop/src/ui-tokens.css: fg-1 mixed into the
 *     pane, 10% in dark and 7% in light, with fg-1 for the words in both.
 *
 *   node apps/ios/scripts/write-palettes.mts          write the file
 *   node apps/ios/scripts/write-palettes.mts --check  fail when it is out of date (CI)
 */
import { readFileSync } from "node:fs";
import { THEME_IDS, THEMES, themeTokens } from "../../../packages/ui/src/graphite/themes.ts";
import {
  GRAPHITE_COLORS,
  GRAPHITE_LIGHT_COLORS,
} from "../../../packages/ui/src/graphite/tokens.ts";
import { emit, finish, repo, swiftHeader, swiftString } from "./lib.mjs";

const SCRIPT = "node apps/ios/scripts/write-palettes.mts";
const OUTPUT = repo("apps/ios/O7Kit/Sources/O7Theme/Palettes.generated.swift");

type Appearance = "dark" | "light";
type Tokens = Map<string, string>;

/** `--g-*` declarations in a piece of CSS, by name without the prefix. */
function declarations(css: string): Tokens {
  const found: Tokens = new Map();
  for (const match of css.matchAll(/--g-([\w-]+)\s*:\s*([^;]+);/g)) {
    found.set(match[1] ?? "", (match[2] ?? "").replace(/\s+/g, " ").trim());
  }
  return found;
}

const tokensCss = readFileSync(repo("packages/ui/src/graphite/tokens.css"), "utf8");
const LIGHT_SELECTOR = ':root[data-theme="light"]';
const rootBlock = /:root \{([\s\S]*?)\n\}/.exec(tokensCss.split(LIGHT_SELECTOR)[0] ?? "")?.[1];
const lightBlock = /:root\[data-theme="light"\] \{([^}]+)\}/.exec(tokensCss)?.[1];
if (!rootBlock || !lightBlock) throw new Error("tokens.css: the :root or light block is missing");

/** Graphite in each appearance: the stylesheet's cascade, checked against tokens.ts. */
const GRAPHITE: Record<Appearance, Tokens> = {
  dark: declarations(rootBlock),
  light: new Map([...declarations(rootBlock), ...declarations(lightBlock)]),
};
for (const [appearance, mirror] of [
  ["dark", GRAPHITE_COLORS],
  ["light", GRAPHITE_LIGHT_COLORS],
] as const) {
  for (const [name, value] of Object.entries(mirror)) {
    const css = GRAPHITE[appearance].get(name);
    if (css !== value) {
      throw new Error(`tokens.ts and tokens.css disagree on ${appearance} --g-${name}`);
    }
  }
}

interface Rgba {
  readonly hex: number;
  readonly alpha: number;
}

/** A `#rrggbb` or `rgb(r g b / a)` value. */
function parse(value: string, what: string): Rgba {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex?.[1]) return { hex: Number.parseInt(hex[1], 16), alpha: 1 };
  const rgb = /^rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)$/.exec(value);
  if (rgb) {
    const [r, g, b] = [rgb[1], rgb[2], rgb[3]].map(Number);
    const alpha = Number(rgb[4]);
    if ([r, g, b].some((c) => c === undefined || c > 255) || !(alpha >= 0 && alpha <= 1)) {
      throw new Error(`${what}: out of range: ${value}`);
    }
    return { hex: ((r ?? 0) << 16) | ((g ?? 0) << 8) | (b ?? 0), alpha };
  }
  throw new Error(`${what}: not a #rrggbb or rgb(r g b / a) colour: ${value}`);
}

const channel = (color: Rgba, shift: number) => (color.hex >> shift) & 255;

/** CSS `color-mix(in srgb, a <amount>, b)` of two opaque colours, rounded to 8 bits. */
function mix(a: Rgba, amount: number, b: Rgba): Rgba {
  if (a.alpha !== 1 || b.alpha !== 1) throw new Error("mix: only opaque colours");
  const [r, g, bl] = [16, 8, 0].map((shift) =>
    Math.round(channel(a, shift) * amount + channel(b, shift) * (1 - amount)),
  );
  return { hex: ((r ?? 0) << 16) | ((g ?? 0) << 8) | (bl ?? 0), alpha: 1 };
}

interface Field {
  readonly name: string;
  readonly doc: string;
  readonly token?: string;
  readonly derive?: (color: (token: string) => Rgba, appearance: Appearance) => Rgba;
}

const field = (name: string, token: string, doc: string): Field => ({ name, token, doc });

/** Every colour a palette holds, in declaration order, with the token it comes from. */
const FIELDS: readonly Field[] = [
  field("window", "window", "The opaque window under everything (`--g-window`)."),
  field("page", "bg", "The page (`--g-bg`)."),
  field("pane", "glass-layer", "The solid pane content sits on (`--g-glass-layer`)."),
  field(
    "raised",
    "glass-pop",
    "A raised layer on the pane: the composer, cards (`--g-glass-pop`).",
  ),
  field("surface1", "surface-1", "Surface 1 (`--g-surface-1`); in dark each surface is lighter."),
  field("surface2", "surface-2", "Surface 2 (`--g-surface-2`)."),
  field("surface3", "surface-3", "Surface 3 (`--g-surface-3`), the agent tile."),
  field("surface4", "surface-4", "Surface 4 (`--g-surface-4`)."),
  field("overlay", "overlay", "Menus and popovers, which are solid (`--g-overlay`)."),
  field(
    "brandPlate",
    "brand-plate",
    "The plate under the agents' white marks, dark in both appearances (`--g-brand-plate`).",
  ),
  field("lineFaint", "line-faint", "The faintest divider (`--g-line-faint`)."),
  field("line", "line", "An ordinary divider (`--g-line`)."),
  field(
    "lineStrong",
    "line-strong",
    "A strong line: control outlines and the agent tile's ring (`--g-line-strong`).",
  ),
  field("fg1", "fg-1", "Primary text (`--g-fg-1`)."),
  field("fg2", "fg-2", "Secondary text (`--g-fg-2`)."),
  field("fg3", "fg-3", "Tertiary text and meta lines (`--g-fg-3`)."),
  field("fg4", "fg-4", "Disabled text (`--g-fg-4`)."),
  field("primary", "primary", "A filled control at rest (`--g-primary`)."),
  field(
    "primaryHover",
    "primary-hover",
    "A filled control under the pointer (`--g-primary-hover`).",
  ),
  field(
    "primaryPressed",
    "primary-active",
    "A filled control while pressed (`--g-primary-active`).",
  ),
  field("onPrimary", "on-light", "The label on a filled control (`--g-on-light`)."),
  field(
    "primaryDisabled",
    "fill-primary-disabled",
    "A filled control that cannot act yet (`--g-fill-primary-disabled`).",
  ),
  field("accent", "volt", "The theme's accent, volt: attention and focus (`--g-volt`)."),
  field("accentTint", "volt-tint", "A faint wash of the accent (`--g-volt-tint`)."),
  field("focus", "focus", "Focus rings (`--g-focus`)."),
  field("selection", "selection", "Selected text and rows (`--g-selection`)."),
  field("hover", "hover", "A solid row under the pointer (`--g-hover`)."),
  field("selected", "selected", "A solid selected row (`--g-selected`)."),
  field("green", "green", "Success. The same in every theme (`--g-green`)."),
  field("greenTint", "green-tint", "A success card's fill (`--g-green-tint`)."),
  field("red", "red", "Failure and destructive actions (`--g-red`)."),
  field("redHover", "red-hover", "A destructive control under the pointer (`--g-red-hover`)."),
  field("redTint", "red-tint", "An error card's fill (`--g-red-tint`)."),
  field("amber", "warning", "Caution: away, waiting (`--g-warning`)."),
  field("amberTint", "warning-tint", "A caution card's fill (`--g-warning-tint`)."),
  field("onDanger", "on-danger", "The label on a red fill (`--g-on-danger`)."),
  field("inkCard", "glass-card", "Ink ladder: a card's faint fill (`--g-glass-card`)."),
  field("inkControl", "glass-control", "Ink ladder: a quiet control (`--g-glass-control`)."),
  field("inkSelected", "glass-selected", "Ink ladder: a selected item (`--g-glass-selected`)."),
  field(
    "inkHover",
    "glass-control-hover",
    "Ink ladder: a control under the pointer (`--g-glass-control-hover`).",
  ),
  field(
    "inkActive",
    "glass-control-active",
    "Ink ladder: a pressed control (`--g-glass-control-active`).",
  ),
  field("rim", "glass-rim", "Ink ladder: a layer's hairline rim (`--g-glass-rim`)."),
  field(
    "rimStrong",
    "glass-rim-strong",
    "Ink ladder: a stronger rim, as on focus (`--g-glass-rim-strong`).",
  ),
  {
    name: "paper",
    doc: "The sent message's bubble: fg-1 mixed into the pane, 10% in dark and 7% in light.",
    derive: (color, appearance) =>
      mix(color("fg-1"), appearance === "dark" ? 0.1 : 0.07, color("glass-layer")),
  },
  {
    name: "onPaper",
    doc: "The words on the paper: fg-1 in both appearances.",
    derive: (color) => color("fg-1"),
  },
  field("codeBackground", "code-bg", "Behind code blocks (`--g-code-bg`)."),
  field("codeText", "code-fg", "Code's own text, between fg-1 and fg-2 (`--g-code-fg`)."),
  field(
    "syntaxKeyword",
    "code-keyword",
    "highlight.js keyword, selector-tag, built_in, doctag, template-tag and type (`--g-code-keyword`).",
  ),
  field(
    "syntaxString",
    "code-string",
    "highlight.js string, regexp, template-variable, symbol, link, selector-attr and selector-pseudo (`--g-code-string`).",
  ),
  field(
    "syntaxNumber",
    "code-number",
    "highlight.js number, literal and bullet (`--g-code-number`).",
  ),
  field(
    "syntaxTitle",
    "code-function",
    "highlight.js title, section, selector-id, selector-class and name (`--g-code-function`).",
  ),
  field(
    "syntaxAttr",
    "code-bright",
    "highlight.js attr, attribute, property, params and variable (`--g-code-bright`).",
  ),
  field("syntaxComment", "code-dim", "highlight.js comment, quote and meta (`--g-code-dim`)."),
  field("syntaxAddition", "green", "highlight.js addition (`--g-green`)."),
  field("syntaxDeletion", "red-hover", "highlight.js deletion (`--g-red-hover`)."),
  field("terminalBackground", "terminal-bg", "Behind terminal output (`--g-terminal-bg`)."),
  field("terminalText", "terminal-fg", "Terminal text (`--g-terminal-fg`)."),
  field("terminalCursor", "terminal-cursor", "The terminal's cursor (`--g-terminal-cursor`)."),
  field(
    "terminalCursorText",
    "terminal-cursor-text",
    "Text under the terminal's cursor (`--g-terminal-cursor-text`).",
  ),
  field(
    "terminalSelection",
    "terminal-selection",
    "Selected terminal text (`--g-terminal-selection`).",
  ),
  field("diffHunk", "diff-hunk", "A diff's hunk header band (`--g-diff-hunk`)."),
  field("diffAddLine", "diff-add-line", "An added line (`--g-diff-add-line`)."),
  field(
    "diffAddWord",
    "diff-add-word",
    "The changed words on an added line (`--g-diff-add-word`).",
  ),
  field("diffDeleteLine", "diff-del-line", "A removed line (`--g-diff-del-line`)."),
  field(
    "diffDeleteWord",
    "diff-del-word",
    "The changed words on a removed line (`--g-diff-del-word`).",
  ),
  field("scrim", "scrim", "Behind a modal (`--g-scrim`)."),
];

/** The terminal's sixteen ANSI colours, in SGR order (30 to 37, then 90 to 97). */
const ANSI = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "bright-black",
  "bright-red",
  "bright-green",
  "bright-yellow",
  "bright-blue",
  "bright-magenta",
  "bright-cyan",
  "bright-white",
].map((name) => `ansi-${name}`);

/** A theme id as a Swift case name: `oh_my_pi` is `ohMyPi`. */
function caseName(id: string): string {
  return id.replace(/[_-]([a-z])/g, (_, letter: string) => letter.toUpperCase());
}

function swiftColor({ hex, alpha }: Rgba): string {
  const digits = `0x${hex.toString(16).padStart(6, "0")}`;
  return alpha === 1 ? `RGBA(hex: ${digits})` : `RGBA(hex: ${digits}, alpha: ${alpha})`;
}

const palettes: string[] = [];
const cases: string[] = [];
for (const id of THEME_IDS) {
  const theme = THEMES.find((candidate) => candidate.id === id);
  if (id !== "aura" && !theme) throw new Error(`THEMES has no ${id}`);
  for (const appearance of ["dark", "light"] as const) {
    const tokens = new Map(GRAPHITE[appearance]);
    if (theme) {
      for (const [name, value] of Object.entries(themeTokens(theme[appearance], appearance))) {
        if (!tokens.has(name)) throw new Error(`${id} ${appearance} sets unknown --g-${name}`);
        tokens.set(name, value);
      }
    }
    const label = `${id} ${appearance}`;
    const color = (token: string): Rgba => {
      const value = tokens.get(token);
      if (value === undefined) throw new Error(`${label}: --g-${token} is missing`);
      return parse(value, `${label} --g-${token}`);
    };
    // The palette leaves out --g-outline, which is the strong line in every theme.
    if (tokens.get("outline") !== tokens.get("line-strong")) {
      throw new Error(`${label}: --g-outline is no longer --g-line-strong`);
    }
    const name = `${caseName(id)}${appearance === "dark" ? "Dark" : "Light"}`;
    const args = [
      `theme: .${caseName(id)}`,
      `isDark: ${appearance === "dark"}`,
      ...FIELDS.map((f) => {
        const value = f.derive ? f.derive(color, appearance) : color(f.token ?? "");
        return `${f.name}: ${swiftColor(value)}`;
      }),
      `ansi: [\n${ANSI.map((token) => `            ${swiftColor(color(token))},`).join("\n")}\n        ]`,
    ];
    palettes.push(
      `    /// ${theme?.name ?? "Main"}, ${appearance}.\n` +
        `    static let ${name} = Palette(\n${args.map((arg) => `        ${arg}`).join(",\n")}\n    )`,
    );
  }
  cases.push(`        case .${caseName(id)}: dark ? ${caseName(id)}Dark : ${caseName(id)}Light`);
}

const properties = [
  "    /// The theme this palette belongs to.",
  "    public let theme: ThemeID",
  "    /// Whether this is the dark appearance.",
  "    public let isDark: Bool",
  ...FIELDS.flatMap((f) => [`    /// ${f.doc}`, `    public let ${f.name}: RGBA`]),
  "    /// The terminal's sixteen ANSI colours, in SGR order: black, red, green, yellow, blue,",
  "    /// magenta, cyan and white, then their bright forms (`--g-ansi-*`).",
  "    public let ansi: [RGBA]",
];

const themeCases = THEME_IDS.map((id) =>
  caseName(id) === id ? `    case ${id}` : `    case ${caseName(id)} = ${swiftString(id)}`,
);
const names = THEME_IDS.map((id) => {
  const theme = THEMES.find((candidate) => candidate.id === id);
  return `        case .${caseName(id)}: ${swiftString(theme?.name ?? "Main")}`;
});
const agents = THEME_IDS.map((id) => {
  const theme = THEMES.find((candidate) => candidate.id === id);
  return `        case .${caseName(id)}: ${theme ? swiftString(theme.agent) : "nil"}`;
});

const swift = `${swiftHeader(
  SCRIPT,
  "packages/ui/src/graphite/themes.ts, tokens.ts and tokens.css, and apps/desktop/src/ui-tokens.css",
  `${SCRIPT} --check`,
)}
/// The desktop's themes, Main first (its case is aura, the saved name): o7's own, then one per
/// supported agent.
public enum ThemeID: String, CaseIterable, Sendable {
${themeCases.join("\n")}

    /// The name Settings shows.
    public var name: String {
        switch self {
${names.join("\n")}
        }
    }

    /// The agent whose mark Settings shows beside the theme; Main shows o7's own wing.
    public var agent: String? {
        switch self {
${agents.join("\n")}
        }
    }
}

/// Every colour the app takes from a theme in one appearance, as the desktop's \`--g-*\` tokens
/// resolve there.
public struct Palette: Sendable, Hashable {
${properties.join("\n")}
}

extension Palettes {
    static func generated(_ id: ThemeID, dark: Bool) -> Palette {
        switch id {
${cases.join("\n")}
        }
    }

${palettes.join("\n\n")}
}
`;

emit(OUTPUT, swift);
finish(SCRIPT);
