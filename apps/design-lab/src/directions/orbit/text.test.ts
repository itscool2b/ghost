import { describe, expect, it } from "vitest";
import { GOLDEN, GOLDEN_TEST_OUTPUT } from "../../fixture/golden";
import { reduceTranscript } from "../../model/transcript";
import type { ToolCallModel } from "../../model/types";
import {
  closeOpenInline,
  commandParts,
  diffSquares,
  formatClock,
  languageLabel,
  languageOfPath,
  middleEllipsis,
  plainText,
  splitCents,
  splitCount,
  splitPath,
  splitWords,
  testSummary,
  thoughtSummary,
  toolParts,
} from "./text";

function goldenCalls(): ToolCallModel[] {
  const model = reduceTranscript(GOLDEN.events);
  return (model.turns[0]?.items ?? []).flatMap((item) => (item.kind === "tools" ? item.calls : []));
}

describe("paths, clocks and numbers", () => {
  it("splits a path into its folder and its name", () => {
    expect(splitPath("packages/ui/src/orb/pacing.ts")).toEqual({
      dir: "packages/ui/src/orb/",
      name: "pacing.ts",
    });
    expect(splitPath("README.md")).toEqual({ dir: "", name: "README.md" });
  });

  it("shows waits as clocks", () => {
    expect([0, 999, 9000, 65_000, 720_000, -5, Number.NaN].map(formatClock)).toEqual([
      "0:00",
      "0:00",
      "0:09",
      "1:05",
      "12:00",
      "0:00",
      "0:00",
    ]);
  });

  it("builds superscript cents and small units instead of asking the font", () => {
    expect(splitCents(0.21)).toEqual({ whole: "$0", cents: "21" });
    expect(splitCents(1234.5)).toEqual({ whole: "$1,234", cents: "50" });
    expect(splitCents(0.2, "EUR")).toEqual({ whole: "0", cents: "20" });
    expect(splitCents(null)).toBeNull();
    expect(splitCents(-1)).toBeNull();
    expect(splitCount(6959)).toEqual({ value: "6.9", unit: "k" });
    expect(splitCount(999_999)).toEqual({ value: "999.9", unit: "k" });
    expect(splitCount(950)).toEqual({ value: "950", unit: "" });
    expect(splitCount(1_200_000)).toEqual({ value: "1.2", unit: "M" });
  });

  it("splits diffstats into five squares by share, a side with lines never empty", () => {
    expect(diffSquares(59, 23)).toEqual(["add", "add", "add", "add", "del"]);
    expect(diffSquares(1, 1)).toEqual(["add", "del", "rest", "rest", "rest"]);
    expect(diffSquares(0, 0)).toEqual(["rest", "rest", "rest", "rest", "rest"]);
    expect(diffSquares(100, 0)).toEqual(["add", "add", "add", "add", "add"]);
    expect(diffSquares(0, 40)).toEqual(["del", "del", "del", "del", "del"]);
    expect(diffSquares(3, 0)).toEqual(["add", "add", "rest", "rest", "rest"]);
    expect(diffSquares(50, 1)).toContain("del");
    expect(diffSquares(59, 23, 10)).toHaveLength(10);
  });

  it("leaves the middle out of a long id", () => {
    expect(middleEllipsis("019e3f2a7f5f")).toBe("019e…7f5f");
    expect(middleEllipsis("short")).toBe("short");
  });
});

describe("streaming markdown", () => {
  it("closes an open bold run or code span so words arrive styled", () => {
    expect(closeOpenInline("- **Jitter")).toBe("- **Jitter**");
    expect(closeOpenInline("uses `shouldDr")).toBe("uses `shouldDr`");
    expect(closeOpenInline("Text **")).toBe("Text ");
    expect(closeOpenInline("calls `")).toBe("calls ");
    const done = "- **Jitter tolerance.** `shouldDrawFrame` now accepts";
    expect(closeOpenInline(done)).toBe(done);
    expect(closeOpenInline("`a ** b`")).toBe("`a ** b`");
  });

  it("keys words by their offset, so earlier words keep their keys as text arrives", () => {
    const before = splitWords("one two ");
    const after = splitWords("one two three");
    expect(after.slice(0, before.length)).toEqual(before);
    expect(splitWords("Found it. The", 10).map((word) => word.key)).toEqual([10, 16, 20]);
    expect(
      splitWords("a  b")
        .map((word) => word.text)
        .join(""),
    ).toBe("a  b");
  });

  it("reads a thought's bold title line as its summary", () => {
    expect(thoughtSummary("**Planning the ACP client**\n\nI'll check how")).toEqual({
      summary: "Planning the ACP client",
      body: "I'll check how",
    });
    expect(thoughtSummary("**Planning the").summary).toBe("Planning the");
    expect(thoughtSummary("**Bold** start of a sentence").summary).toBeNull();
    expect(thoughtSummary("The test only fails on CI").summary).toBeNull();
  });
});

describe("tools as label plus literal", () => {
  it("names each golden call with a verb, a literal and a result", () => {
    const [readPacing, readTest, grep, editPacing, , run] = goldenCalls();
    expect(readPacing && toolParts(readPacing)).toEqual({
      verb: "Read",
      object: "pacing.ts",
      result: null,
    });
    expect(readTest && toolParts(readTest).object).toBe("pacing.test.ts");
    expect(grep && toolParts(grep)).toEqual({
      verb: "Searched",
      object: "performance.now",
      result: "3 matches",
    });
    expect(editPacing && toolParts(editPacing).verb).toBe("Edited");
    expect(run && toolParts(run)).toEqual({
      verb: "Ran",
      object: "pnpm --filter @ghost/ui test pacing",
      result: "19 passed",
    });
  });

  it("reads test runner summaries", () => {
    expect(testSummary(GOLDEN_TEST_OUTPUT)).toEqual({ passed: 19, failed: 0 });
    expect(testSummary("test result: FAILED. 12 passed; 2 failed; 0 ignored")).toEqual({
      passed: 12,
      failed: 2,
    });
    expect(testSummary("Tests  2 failed | 17 passed (19)")).toEqual({ passed: 17, failed: 2 });
    expect(testSummary("no summary here")).toBeNull();
  });

  it("labels languages and splits commands", () => {
    expect(languageLabel("ts")).toBe("TypeScript");
    expect(languageLabel("constructor")).toBe("constructor");
    expect(languageOfPath("crates/aura-drivers/src/acp.rs")).toBe("rs");
    expect(languageOfPath("Makefile")).toBe("");
    expect(commandParts("pnpm --filter @ghost/ui test")).toEqual({
      binary: "pnpm",
      args: "--filter @ghost/ui test",
    });
    expect(commandParts("ls")).toEqual({ binary: "ls", args: "" });
  });
});

describe("copying an answer", () => {
  it("drops markdown marks but keeps the words, list markers and code", () => {
    const source = [
      "## The fix",
      "",
      "- **Pacing** now waits for `frame()` before it draws.",
      "- See [the test](https://example.com/t) and *why* it failed.",
      "",
      "```ts",
      "const a = 2 ** 3; // `kept`",
      "```",
    ].join("\n");
    expect(plainText(source)).toBe(
      [
        "The fix",
        "",
        "- Pacing now waits for frame() before it draws.",
        "- See the test and why it failed.",
        "",
        "const a = 2 ** 3; // `kept`",
      ].join("\n"),
    );
  });

  it("leaves marks inside inline code and lone asterisks alone", () => {
    expect(plainText("Run `a **b** c` then 2 * 3 * 4")).toBe("Run a **b** c then 2 * 3 * 4");
    expect(plainText("snake_case_name stays")).toBe("snake_case_name stays");
    expect(plainText("")).toBe("");
  });
});
