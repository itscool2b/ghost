import { describe, expect, it } from "vitest";
import {
  blocksOf,
  EMPTY_SPLIT,
  parseBlock,
  parseInline,
  plainText,
  type SplitCache,
  safeHref,
  splitBlocks,
  splitIncremental,
} from "./markdown";

function blockTexts(source: string): string[] {
  return splitBlocks(source).map((span) => source.slice(span.start, span.end));
}

const SAMPLE = `## Extracted a \`greet\` helper

I read \`src/main.rs\` and moved the greeting into its own function.

- \`greet(name)\` builds the message
- \`main\` now prints \`greet("world")\`

\`\`\`rust
fn greet(name: &str) -> String {

    format!("Hello, {name}!")
}
\`\`\`

| Stage | Budget |
| :--- | ---: |
| Parse | 0.50 ms |

Run \`cargo run\` to check it.
`;

describe("splitBlocks", () => {
  it("splits on blank lines but keeps a fenced block whole", () => {
    expect(blockTexts(SAMPLE)).toEqual([
      "## Extracted a `greet` helper",
      "I read `src/main.rs` and moved the greeting into its own function.",
      '- `greet(name)` builds the message\n- `main` now prints `greet("world")`',
      '```rust\nfn greet(name: &str) -> String {\n\n    format!("Hello, {name}!")\n}\n```',
      "| Stage | Budget |\n| :--- | ---: |\n| Parse | 0.50 ms |",
      "Run `cargo run` to check it.",
    ]);
  });

  it("runs an unclosed fence to the end, since it is still streaming", () => {
    expect(blockTexts("Intro\n\n```ts\nconst a = 1;\n\nconst b")).toEqual([
      "Intro",
      "```ts\nconst a = 1;\n\nconst b",
    ]);
  });

  it("starts a code block even without a blank line before it", () => {
    expect(blockTexts("Look:\n```\nx\n```\nafter")).toEqual(["Look:", "```\nx\n```", "after"]);
  });

  it("returns nothing for blank input", () => {
    expect(splitBlocks("")).toEqual([]);
    expect(splitBlocks("\n\n  \n")).toEqual([]);
  });
});

describe("splitIncremental", () => {
  it("matches a full split at every point of a growing text", () => {
    let cache: SplitCache = EMPTY_SPLIT;
    for (let end = 0; end <= SAMPLE.length; end += 1) {
      const source = SAMPLE.slice(0, end);
      cache = splitIncremental(cache, source);
      expect(blocksOf(cache), `after ${end} characters`).toEqual(blockTexts(source));
    }
  });

  it("matches in uneven chunks too, and reuses finished blocks", () => {
    let cache: SplitCache = EMPTY_SPLIT;
    let end = 0;
    let step = 1;
    while (end < SAMPLE.length) {
      end = Math.min(SAMPLE.length, end + step);
      step = (step * 7) % 23 || 1;
      const previous = cache;
      cache = splitIncremental(cache, SAMPLE.slice(0, end));
      expect(blocksOf(cache)).toEqual(blockTexts(SAMPLE.slice(0, end)));
      expect(cache.stable.slice(0, previous.stable.length)).toEqual(previous.stable);
    }
  });

  it("starts over when the text is not an extension of the previous one", () => {
    const first = splitIncremental(EMPTY_SPLIT, "one\n\ntwo\n\nthree");
    const other = splitIncremental(first, "alpha\n\nbeta");
    expect(blocksOf(other)).toEqual(["alpha", "beta"]);
  });
});

describe("parseBlock", () => {
  it("reads headings, paragraphs and rules", () => {
    expect(parseBlock("## 1. Where the time goes")).toEqual([
      { kind: "heading", level: 2, text: "1. Where the time goes" },
    ]);
    expect(parseBlock("# Title\nBody line one\nline two")).toEqual([
      { kind: "heading", level: 1, text: "Title" },
      { kind: "paragraph", text: "Body line one\nline two" },
    ]);
    expect(parseBlock("---")).toEqual([{ kind: "rule" }]);
  });

  it("reads fenced code, closed or still open", () => {
    expect(parseBlock("```rust\nfn x() {}\n```")).toEqual([
      { kind: "code", lang: "rust", code: "fn x() {}", closed: true },
    ]);
    expect(parseBlock("```python\ndef f():\n    pass")).toEqual([
      { kind: "code", lang: "python", code: "def f():\n    pass", closed: false },
    ]);
    expect(parseBlock("```")).toEqual([{ kind: "code", lang: "", code: "", closed: false }]);
  });

  it("reads nested, ordered and task lists", () => {
    expect(
      parseBlock("- Parse lazily\n  - one line at a time\n  - unknown events logged\n- Coalesce"),
    ).toEqual([
      {
        kind: "list",
        ordered: false,
        start: 1,
        items: [
          { text: "Parse lazily", depth: 0, checked: null },
          { text: "one line at a time", depth: 1, checked: null },
          { text: "unknown events logged", depth: 1, checked: null },
          { text: "Coalesce", depth: 0, checked: null },
        ],
      },
    ]);
    expect(parseBlock("3. Third\n4. Fourth")).toMatchObject([
      { kind: "list", ordered: true, start: 3 },
    ]);
    expect(parseBlock("- [x] Done\n- [ ] Not yet")).toMatchObject([
      {
        kind: "list",
        items: [
          { text: "Done", checked: true },
          { text: "Not yet", checked: false },
        ],
      },
    ]);
  });

  it("reads tables with alignment, padding short rows", () => {
    expect(
      parseBlock("| Stage | Budget |\n| :--- | ---: |\n| Parse | 0.50 ms |\n| Total |"),
    ).toEqual([
      {
        kind: "table",
        header: ["Stage", "Budget"],
        align: ["left", "right"],
        rows: [
          ["Parse", "0.50 ms"],
          ["Total", ""],
        ],
      },
    ]);
  });

  it("shows a table row without its delimiter yet as a paragraph", () => {
    expect(parseBlock("| Stage | Budget |")).toEqual([
      { kind: "paragraph", text: "| Stage | Budget |" },
    ]);
  });

  it("reads quotes", () => {
    expect(parseBlock("> First\n> second")).toEqual([{ kind: "quote", text: "First\nsecond" }]);
  });
});

describe("parseInline", () => {
  it("reads code, strong, emphasis and links", () => {
    expect(
      parseInline(
        "Parse **thousands of deltas**, re-render the _last_ block; see [notes](https://onghost.dev/docs).",
      ),
    ).toEqual([
      { kind: "text", text: "Parse " },
      { kind: "strong", children: [{ kind: "text", text: "thousands of deltas" }] },
      { kind: "text", text: ", re-render the " },
      { kind: "em", children: [{ kind: "text", text: "last" }] },
      { kind: "text", text: " block; see " },
      {
        kind: "link",
        href: "https://onghost.dev/docs",
        children: [{ kind: "text", text: "notes" }],
      },
      { kind: "text", text: "." },
    ]);
    expect(parseInline("Keep `requestAnimationFrame` under 4 ms")).toEqual([
      { kind: "text", text: "Keep " },
      { kind: "code", text: "requestAnimationFrame" },
      { kind: "text", text: " under 4 ms" },
    ]);
  });

  it("leaves snake_case, lone markers and arithmetic alone", () => {
    expect(plainText(parseInline("message_id and thought_id"))).toBe("message_id and thought_id");
    expect(parseInline("2 * 3 * 4")).toEqual([{ kind: "text", text: "2 * 3 * 4" }]);
    expect(parseInline("an `unclosed span")).toEqual([{ kind: "text", text: "an `unclosed span" }]);
  });

  it("never turns non-web URLs into links", () => {
    expect(parseInline("[click](javascript:alert(1))")).toEqual([
      { kind: "text", text: "[click](javascript:alert(1))" },
    ]);
    expect(safeHref("https://example.com/a?b=c")).toBe("https://example.com/a?b=c");
    for (const href of ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,x", "//x"]) {
      expect(safeHref(href), href).toBeNull();
    }
  });

  it("keeps Unicode and emoji intact", () => {
    const text = "déjà vu, 日本語, 👩‍💻 and 🇸🇮";
    expect(plainText(parseInline(text))).toBe(text);
  });

  it("bounds nesting", () => {
    const deep = `${"*a ".repeat(50)}x${" a*".repeat(50)}`;
    expect(() => parseInline(deep)).not.toThrow();
  });
});
