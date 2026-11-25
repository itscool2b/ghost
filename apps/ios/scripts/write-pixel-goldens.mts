/**
 * Writes the pixel geometry goldens the Swift port is tested against,
 * apps/ios/O7Kit/Tests/O7MarksTests/PixelGoldens.generated.swift, by running the desktop's own
 * pixels.ts: the loader's cells, the done check and the centred dot at every size a loader or a
 * mark can take, and the loader's frames.
 *
 *   node apps/ios/scripts/write-pixel-goldens.mts          write the file
 *   node apps/ios/scripts/write-pixel-goldens.mts --check  fail when it is out of date (CI)
 */
import {
  CHECK_GRID,
  centredSquare,
  checkRects,
  LOADER_ORDER,
  LOADER_STEP_MS,
  loaderFrame,
  loaderRects,
  type PixelRect,
} from "../../../packages/ui/src/graphite/pixels.ts";
import { emit, finish, repo, swiftHeader, swiftString } from "./lib.mjs";

const SCRIPT = "node apps/ios/scripts/write-pixel-goldens.mts";
const OUTPUT = repo("apps/ios/O7Kit/Tests/O7MarksTests/PixelGoldens.generated.swift");

/** PixelLoader clamps to 8...128 and AgentMark to 12...256: cover both. */
const SIZES = { from: 8, through: 256 } as const;
const FRACTIONS = [0.25, 0.375, 0.5, 0.75] as const;
const STEPS = { from: -12, upTo: 24 } as const;

const sizes = Array.from({ length: SIZES.through - SIZES.from + 1 }, (_, i) => SIZES.from + i);

/** Rectangles as flat x, y, w, h runs. */
function flat(rects: readonly PixelRect[]): number[] {
  return rects.flatMap((r) => [r.x, r.y, r.w, r.h]);
}

function array(name: string, doc: string, values: readonly number[]): string {
  const lines: string[] = [];
  for (let i = 0; i < values.length; i += 16)
    lines.push(`        ${values.slice(i, i + 16).join(", ")},`);
  return `    /// ${doc}\n    static let ${name}: [Int] = [\n${lines.join("\n")}\n    ]`;
}

const steps = Array.from({ length: STEPS.upTo - STEPS.from }, (_, i) => STEPS.from + i);

const body = [
  `    static let sizes: ClosedRange<Int> = ${SIZES.from}...${SIZES.through}`,
  `    static let steps: Range<Int> = ${STEPS.from}..<${STEPS.upTo}`,
  `    static let fractions: [Double] = [${FRACTIONS.join(", ")}]`,
  `    static let loaderOrder: [Int] = [${LOADER_ORDER.join(", ")}]`,
  `    static let stepMilliseconds: [String: Int] = [${Object.entries(LOADER_STEP_MS)
    .map(([tempo, ms]) => `${swiftString(tempo)}: ${ms}`)
    .join(", ")}]`,
  `    static let checkGrid: [String] = [${CHECK_GRID.map(swiftString).join(", ")}]`,
  array(
    "loaderFrames",
    "loaderFrame(step) for each step, six levels each.",
    steps.flatMap((step) => loaderFrame(step)),
  ),
  array(
    "loaderRects",
    "loaderRects(size) for each size: six cells of x, y, w, h.",
    sizes.flatMap((size) => flat(loaderRects(size))),
  ),
  array(
    "checkRects",
    "checkRects(size) for each size: five cells of x, y, w, h, in drawing order.",
    sizes.flatMap((size) => flat(checkRects(size))),
  ),
  array(
    "centredSquares",
    "centredSquare(size, fraction) for each size, then each fraction: x, y, w, h.",
    sizes.flatMap((size) => FRACTIONS.flatMap((fraction) => flat([centredSquare(size, fraction)]))),
  ),
];

const swift = `${swiftHeader(SCRIPT, "packages/ui/src/graphite/pixels.ts", `${SCRIPT} --check`)}
/// What the desktop's pixels.ts computes, for the Swift port to match exactly.
enum PixelGoldens {
${body.join("\n\n")}
}
`;

emit(OUTPUT, swift);
finish(SCRIPT);
