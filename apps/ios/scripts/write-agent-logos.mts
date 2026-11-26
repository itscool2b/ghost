/**
 * Writes the agents' marks and Ghost's wing for the iPhone app from the desktop's own sources, so
 * nothing is copied by hand:
 *   - Sources/GhostMarks/AgentLogos.generated.swift: every stored mark (agentLogos.ts and the SVG
 *     files it imports) as its viewBox, the square AgentLogo.tsx draws it in, its shapes with their
 *     fills, fill rule and opacity, its optical scale and the Hermes turn.
 *   - Sources/GhostMarks/AgentBrands.generated.swift: brand ids and names and their aliases
 *     (brand.ts), two-letter codes (AgentMark.tsx), the agents whose white marks sit on the brand
 *     plate (graphite.css), and the state labels screen readers hear (orb/states.ts).
 *   - Sources/GhostMarks/GhostLogoPaths.generated.swift: the wing (logoPaths.ts).
 *   - Tests/GhostMarksTests/MarkGoldens.generated.swift: agentCode, getBrand and logoSide on samples,
 *     for the Swift ports to match.
 *
 *   node apps/ios/scripts/write-agent-logos.mts          write the files
 *   node apps/ios/scripts/write-agent-logos.mts --check  fail when they are out of date (CI)
 */
import "./register-ts.mjs";
import { readFileSync } from "node:fs";
import { emit, finish, repo, swiftHeader, swiftNumber, swiftString } from "./lib.mjs";

const SCRIPT = "node apps/ios/scripts/write-agent-logos.mts";
const CHECK = `${SCRIPT} --check`;
const UI = "../../../packages/ui/src";

// Static imports resolve before register-ts.mjs runs, so the sources load afterwards.
const { AGENT_LOGOS } = await import(`${UI}/graphite/agentLogos.ts`);
const { AGENT_BRAND_IDS, AGENT_BRANDS, getBrand } = await import(`${UI}/brand.ts`);
const { ORB_STATE_LABELS, ORB_STATES } = await import(`${UI}/orb/states.ts`);
const { agentCode, logoSide } = await import(`${UI}/graphite/AgentMark.tsx`);
const { logoViewBox } = await import(`${UI}/graphite/AgentLogo.tsx`);
const paths = await import(`${UI}/graphite/logoPaths.ts`);

interface Shape {
  readonly tag: string;
  readonly attributes: Readonly<Record<string, string>>;
}

interface Logo {
  readonly id: string;
  readonly name: string;
  readonly viewBox: string;
  readonly shapes: readonly Shape[];
  readonly scale: number;
  readonly turn?: { readonly angle: number; readonly frame: string };
}

/** The only path commands the stored marks may use, which SVGPathParser reads. */
const PATH_COMMANDS = /^[MLHVCZmlhvcz0-9eE.,+\-\s]*$/;

function numbers(text: string, count: number, what: string): number[] {
  const values = text
    .trim()
    .split(/[\s,]+/)
    .map(Number);
  if (values.length !== count || values.some((value) => !Number.isFinite(value))) {
    throw new Error(`${what}: expected ${count} numbers, got "${text}"`);
  }
  return values;
}

function number(
  attributes: Readonly<Record<string, string>>,
  name: string,
  what: string,
  fallback?: number,
) {
  const value = attributes[name];
  if (value === undefined) {
    if (fallback === undefined) throw new Error(`${what}: missing ${name}`);
    return fallback;
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${what}: ${name} is not a number: ${value}`);
  return parsed;
}

const NAMED_FILLS: Readonly<Record<string, number>> = { white: 0xffffff, black: 0x000000 };

function fill(value: string | undefined, what: string): string {
  if (value === undefined) return ".color(RGBA(hex: 0x000000))";
  if (value === "currentColor") return ".foreground";
  const hex = /^#([0-9a-f]{6})$/i.exec(value)?.[1];
  const named = NAMED_FILLS[value.toLowerCase()];
  const digits = hex ? Number.parseInt(hex, 16) : named;
  if (digits === undefined) throw new Error(`${what}: unsupported fill ${value}`);
  return `.color(RGBA(hex: 0x${digits.toString(16).padStart(6, "0")}))`;
}

const ALLOWED = new Set([
  "d",
  "fill",
  "fill-rule",
  "clip-rule",
  "fill-opacity",
  "opacity",
  "cx",
  "cy",
  "r",
  "rx",
  "ry",
  "x",
  "y",
  "width",
  "height",
  "points",
]);

function geometry({ tag, attributes }: Shape, what: string): string {
  const n = (name: string, fallback?: number) =>
    swiftNumber(number(attributes, name, what, fallback));
  switch (tag) {
    case "path": {
      const d = attributes.d ?? "";
      if (!d || !PATH_COMMANDS.test(d))
        throw new Error(`${what}: path uses commands beyond M L H V C Z`);
      return `.path(${swiftString(d)})`;
    }
    case "rect": {
      const rx = attributes.rx ?? attributes.ry ?? "0";
      const ry = attributes.ry ?? attributes.rx ?? "0";
      return `.rect(x: ${n("x", 0)}, y: ${n("y", 0)}, width: ${n("width")}, height: ${n("height")}, rx: ${swiftNumber(Number(rx))}, ry: ${swiftNumber(Number(ry))})`;
    }
    case "circle":
      return `.ellipse(cx: ${n("cx", 0)}, cy: ${n("cy", 0)}, rx: ${n("r")}, ry: ${n("r")})`;
    case "ellipse":
      return `.ellipse(cx: ${n("cx", 0)}, cy: ${n("cy", 0)}, rx: ${n("rx")}, ry: ${n("ry")})`;
    case "polygon": {
      const values = (attributes.points ?? "")
        .trim()
        .split(/[\s,]+/)
        .map(Number);
      if (values.length < 6 || values.length % 2 !== 0 || values.some((v) => !Number.isFinite(v))) {
        throw new Error(`${what}: bad polygon points`);
      }
      const d = values
        .reduce<string[]>((parts, value, i) => {
          if (i % 2 === 0) parts.push(`${i === 0 ? "M" : "L"}${value} ${values[i + 1]}`);
          return parts;
        }, [])
        .join("");
      return `.path(${swiftString(`${d}Z`)})`;
    }
    default:
      throw new Error(`${what}: unsupported element <${tag}>`);
  }
}

function box(text: string, what: string): string {
  const [x, y, w, h] = numbers(text, 4, what).map(swiftNumber);
  return `CGRect(x: ${x}, y: ${y}, width: ${w}, height: ${h})`;
}

const logos = Object.values(AGENT_LOGOS as Record<string, Logo>).map((logo) => {
  const shapes = logo.shapes.map((shape, index) => {
    const what = `${logo.id} shape ${index}`;
    for (const name of Object.keys(shape.attributes)) {
      if (!ALLOWED.has(name)) throw new Error(`${what}: unexpected attribute ${name}`);
    }
    const rule = shape.attributes["fill-rule"] ?? "nonzero";
    if (rule !== "nonzero" && rule !== "evenodd") throw new Error(`${what}: fill-rule ${rule}`);
    const opacity =
      number(shape.attributes, "opacity", what, 1) *
      number(shape.attributes, "fill-opacity", what, 1);
    return (
      `                LogoShape(${geometry(shape, what)}, fill: ${fill(shape.attributes.fill, what)}, ` +
      `evenOdd: ${rule === "evenodd"}, opacity: ${swiftNumber(opacity)})`
    );
  });
  let turn = "nil";
  if (logo.turn) {
    const [x, y, w, h] = numbers(logo.viewBox, 4, `${logo.id} viewBox`);
    turn = `LogoTurn(degrees: ${swiftNumber(logo.turn.angle)}, centre: CGPoint(x: ${swiftNumber((x ?? 0) + (w ?? 0) / 2)}, y: ${swiftNumber((y ?? 0) + (h ?? 0) / 2)}))`;
  }
  return [
    `        ${swiftString(logo.id)}: AgentLogo(`,
    `            id: ${swiftString(logo.id)},`,
    `            name: ${swiftString(logo.name)},`,
    `            viewBox: ${box(logo.viewBox, `${logo.id} viewBox`)},`,
    `            drawBox: ${box(logoViewBox(logo), `${logo.id} drawn box`)},`,
    `            scale: ${swiftNumber(logo.scale)},`,
    `            turn: ${turn},`,
    `            shapes: [\n${shapes.join(",\n")},\n            ]`,
    "        ),",
  ].join("\n");
});

emit(
  repo("apps/ios/GhostKit/Sources/GhostMarks/AgentLogos.generated.swift"),
  `${swiftHeader(SCRIPT, "packages/ui/src/graphite/agentLogos.ts, agent-logos/*.svg and AgentLogo.tsx", CHECK)}
import CoreGraphics
import GhostTheme

extension AgentLogo {
    /// Every agent with a published mark, by brand id, as the vendor publishes it
    /// (third-party/brand-marks.md).
    static let all: [String: AgentLogo] = [
${logos.join("\n")}
    ]
}
`,
);

// Brand aliases live in a private map in brand.ts: read it from the source, strictly, and
// check every pair against getBrand so a misread fails here.
const brandSource = readFileSync(repo("packages/ui/src/brand.ts"), "utf8");
const aliasBlock =
  /const ALIASES: ReadonlyMap<string, AgentBrandId> = new Map<string, AgentBrandId>\(\[([\s\S]*?)\]\);/.exec(
    brandSource,
  )?.[1];
if (aliasBlock === undefined) throw new Error("brand.ts: the ALIASES map is missing");
const PAIR = /\["([^"\\]+)", "([^"\\]+)"\],?/g;
if (aliasBlock.replace(PAIR, "").trim() !== "")
  throw new Error("brand.ts: ALIASES holds more than pairs");
const aliases = [...aliasBlock.matchAll(PAIR)].map(([, alias = "", id = ""]) => {
  if (getBrand(alias).id !== id)
    throw new Error(`brand.ts: alias ${alias} does not resolve to ${id}`);
  return [alias, id] as const;
});

// The agents whose white marks sit on the brand plate, from graphite.css.
const graphiteCss = readFileSync(repo("packages/ui/src/graphite/graphite.css"), "utf8");
const plateRule =
  /\.g-agent-mark:where\(\s*((?:\[data-logo\]\[data-agent="[a-z-]+"\],?\s*)+)\):where\(\[data-state="idle"\], \[data-ghost\]\) \{\s*background-color: var\(--g-brand-plate\);\s*\}/.exec(
    graphiteCss,
  )?.[1];
if (plateRule === undefined) throw new Error("graphite.css: the brand plate rule is missing");
const plated = new Set([...plateRule.matchAll(/data-agent="([a-z-]+)"/g)].map((match) => match[1]));

const brands = (AGENT_BRAND_IDS as readonly string[]).map((id) => {
  const brand = AGENT_BRANDS[id];
  if (id !== "generic" && !(AGENT_LOGOS as Record<string, Logo>)[id]) {
    throw new Error(`${id} has a brand but no stored mark`);
  }
  const code = id === "generic" ? "nil" : swiftString(agentCode(id));
  return `        AgentBrand(id: ${swiftString(id)}, name: ${swiftString(brand.name)}, code: ${code}, plate: ${plated.has(id)})`;
});
for (const id of plated) {
  if (!(AGENT_BRAND_IDS as readonly string[]).includes(id ?? ""))
    throw new Error(`graphite.css plates unknown ${id}`);
}

const labels = (ORB_STATES as readonly string[]).map(
  (state) => `        case .${state}: ${swiftString(ORB_STATE_LABELS[state])}`,
);

emit(
  repo("apps/ios/GhostKit/Sources/GhostMarks/AgentBrands.generated.swift"),
  `${swiftHeader(SCRIPT, "packages/ui/src/brand.ts, graphite/AgentMark.tsx, graphite/graphite.css and orb/states.ts", CHECK)}
extension AgentBrand {
    /// Every designed brand in display order, the generic one last.
    static let all: [AgentBrand] = [
${brands.join(",\n")},
    ]

    /// Other names the same agents go by: binary names, registry ids, vendor-prefixed ids.
    static let aliases: [String: String] = [
${aliases.map(([alias, id]) => `        ${swiftString(alias)}: ${swiftString(id)},`).join("\n")}
    ]
}

extension AgentState {
    /// The short words a screen reader hears after the agent's name (\`ORB_STATE_LABELS\`).
    public var label: String {
        switch self {
${labels.join("\n")}
        }
    }
}
`,
);

const [width, height] = paths.LOGO_VIEWBOX as readonly [number, number];
for (const d of [paths.LOGO_OUTLINE_PATH, ...paths.LOGO_EYE_PATHS, ...paths.LOGO_SMALL_EYE_PATHS]) {
  if (!PATH_COMMANDS.test(d))
    throw new Error("logoPaths.ts: path uses commands beyond M L H V C Z");
}
if (paths.LOGO_PATH !== paths.LOGO_OUTLINE_PATH + paths.LOGO_EYE_PATHS.join("")) {
  throw new Error("logoPaths.ts: LOGO_PATH is no longer the outline and the eyes");
}
emit(
  repo("apps/ios/GhostKit/Sources/GhostMarks/GhostLogoPaths.generated.swift"),
  `${swiftHeader(SCRIPT, "packages/ui/src/graphite/logoPaths.ts", CHECK)}
import CoreGraphics

/// The Ghost wing: a flying wing seen from above, with two eyes. One even-odd shape whose holes are
/// the eyes.
enum GhostLogoPaths {
    /// The wing's coordinate space; it fills the width and sits centred in the height.
    static let viewBox = CGSize(width: ${swiftNumber(width)}, height: ${swiftNumber(height)})

    /// The outline, without its eyes.
    static let outline = ${swiftString(paths.LOGO_OUTLINE_PATH)}

    /// The two eyes, left then right.
    static let eyes = [
${paths.LOGO_EYE_PATHS.map((d: string) => `        ${swiftString(d)},`).join("\n")}
    ]

    /// The small variant's eyes, half again as large, which stay open at icon sizes.
    static let smallEyes = [
${paths.LOGO_SMALL_EYE_PATHS.map((d: string) => `        ${swiftString(d)},`).join("\n")}
    ]

    /// The widest the wing is drawn with the small variant's eyes.
    static let smallMaxWidth: CGFloat = ${swiftNumber(paths.LOGO_SMALL_MAX)}
}
`,
);

/** Ids that exercise getBrand and agentCode: every brand and alias, then the awkward ones. */
const SAMPLES = [
  ...(AGENT_BRAND_IDS as readonly string[]),
  ...aliases.map(([alias]) => alias),
  ...(AGENT_BRAND_IDS as readonly string[]).map((id) => AGENT_BRANDS[id].name as string),
  "CLAUDE_CODE",
  " codex ",
  "claude.code",
  "Claude  Code",
  "claude\tcode",
  "\u00a0claude\u2003",
  "\ufeffpi\u3000",
  "Open Code",
  "my-acp-agent",
  "gemini-cli",
  "future-agent",
  "agent",
  "",
  "!!",
  "GENERIC",
  "a",
  "x1",
  "9lives",
  "constructor",
  "__proto__",
  "ñandú",
  "e\u0301clair",
  "ß-agent",
  "\ufb01le",
  "İstanbul",
  "Ωmega",
  "日本語",
  "½-agent",
];

const sides = Array.from({ length: 256 - 12 + 1 }, (_, i) => logoSide(12 + i));
emit(
  repo("apps/ios/GhostKit/Tests/GhostMarksTests/MarkGoldens.generated.swift"),
  `${swiftHeader(SCRIPT, "packages/ui/src/brand.ts and graphite/AgentMark.tsx", CHECK)}
/// What the desktop's getBrand, agentCode and logoSide return, for the Swift ports to match.
enum MarkGoldens {
    /// An agent id, then getBrand(id).id, getBrand(id).name and agentCode(id).
    static let agents: [(agent: String, brand: String, name: String, code: String)] = [
${[...new Set(SAMPLES)]
  .map(
    (id) =>
      `        (${swiftString(id)}, ${swiftString(getBrand(id).id)}, ${swiftString(getBrand(id).name)}, ${swiftString(agentCode(id))}),`,
  )
  .join("\n")}
    ]

    /// logoSide(size) for sizes 12 through 256.
    static let logoSides: [Int] = [${sides.join(", ")}]
}
`,
);

finish(SCRIPT);
