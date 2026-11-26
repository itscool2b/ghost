import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { GOLDEN } from "../../fixture/golden";
import { sceneFor, workspaceFor } from "../../fixture/scenes";
import type { TimedEvent } from "../../fixture/types";
import { LAB_STATES, type LabState } from "../../kit";
import { eventsAt, reduceTranscript } from "../../model/transcript";
import type { ToolCallModel } from "../../model/types";
import { staticPlayer } from "../../model/usePlayer";
import { App } from "./App";
import { clip, diffStat, handoffFor } from "./handoff";
import { tokenizeLine } from "./syntax";
import { activeLabel, spanStyle } from "./ToolRun";
import { planSlot } from "./Transcript";
import { closeOpenInline, languageOf, splitPath, splitWords, thoughtParts } from "./text";

/** The golden chat's transcript at one moment (seconds), or at its end. */
function goldenAt(seconds?: number) {
  const time = seconds === undefined ? GOLDEN.duration : seconds * 1000;
  return reduceTranscript(GOLDEN.events.slice(0, eventsAt(GOLDEN.events, time)));
}

/** Noir's window for one lab state at one moment, server-rendered, as plain text-ish HTML. */
function render(state: LabState, seconds?: number): string {
  const scene = sceneFor(state);
  const events = scene.recording?.events ?? [];
  const duration = scene.recording?.duration ?? 0;
  const time = seconds === undefined ? duration : seconds * 1000;
  const model = reduceTranscript(events.slice(0, eventsAt(events, time)));
  const workspace = workspaceFor(scene, model);
  return renderToString(
    <App state={state} model={model} workspace={workspace} player={staticPlayer(duration, time)} />,
  ).replaceAll("<!-- -->", "");
}

/** Where `needle` first appears in `html`, failing the test when it does not. */
function at(html: string, needle: string): number {
  const index = html.indexOf(needle);
  expect(index, `"${needle}" is missing`).toBeGreaterThan(-1);
  return index;
}

describe("streaming markdown repair", () => {
  it("closes an open strong run and code span so they render styled at once", () => {
    expect(closeOpenInline("- **Jitter")).toBe("- **Jitter**");
    expect(closeOpenInline("uses `shouldDr")).toBe("uses `shouldDr`");
    expect(closeOpenInline("**Fixed `grid")).toBe("**Fixed `grid`**");
  });

  it("drops a marker that arrived with nothing after it yet", () => {
    expect(closeOpenInline("Text **")).toBe("Text ");
    expect(closeOpenInline("calls `")).toBe("calls ");
  });

  it("leaves balanced text and markers inside code alone", () => {
    const done = "- **Jitter tolerance.** `shouldDrawFrame` now accepts";
    expect(closeOpenInline(done)).toBe(done);
    expect(closeOpenInline("`a ** b`")).toBe("`a ** b`");
    expect(closeOpenInline("")).toBe("");
  });
});

describe("words for the streaming fade", () => {
  it("keeps every character and keys each word by its offset", () => {
    const words = splitWords("Found it. The flake", 10);
    expect(words.map((word) => word.text).join("")).toBe("Found it. The flake");
    expect(words.map((word) => word.key)).toEqual([10, 16, 20, 24]);
  });

  it("gives words the same keys when more text arrives", () => {
    const before = splitWords("one two ");
    const after = splitWords("one two three");
    expect(after.slice(0, before.length)).toEqual(before);
  });
});

describe("thought titles", () => {
  it("reads the bold line Codex opens a thought with", () => {
    expect(thoughtParts("**Planning the ACP client**\n\nI'll check how")).toEqual({
      title: "Planning the ACP client",
      body: "I'll check how",
    });
  });

  it("shows a title that is still streaming without its asterisks", () => {
    expect(thoughtParts("**Planning the").title).toBe("Planning the");
  });

  it("leaves a thought without a title line alone", () => {
    const text = "The test only fails on CI, **sometimes**.";
    expect(thoughtParts(text)).toEqual({ title: null, body: text });
    expect(thoughtParts("**Bold** start of a sentence").title).toBeNull();
  });
});

describe("paths and languages", () => {
  it("splits a path into its folder and name", () => {
    expect(splitPath("packages/ui/src/orb/pacing.ts")).toEqual({
      dir: "packages/ui/src/orb/",
      name: "pacing.ts",
    });
    expect(splitPath("README.md")).toEqual({ dir: "", name: "README.md" });
  });

  it("knows the languages the fixtures edit", () => {
    expect(languageOf("pacing.test.ts")).toBe("ts");
    expect(languageOf("crates/aura-drivers/src/acp.rs")).toBe("rust");
    expect(languageOf("Makefile")).toBe("");
  });
});

describe("monochrome syntax", () => {
  it("keeps every character and finds keywords, calls, strings and numbers", () => {
    const line = '  return Math.min(2, intervalMs / 4); // "slack"';
    const tokens = tokenizeLine(line, "ts");
    expect(tokens.map((token) => token.text).join("")).toBe(line);
    const kinds = new Map(tokens.map((token) => [token.text.trim(), token.kind]));
    expect(kinds.get("return")).toBe("keyword");
    expect(kinds.get("min")).toBe("call");
    expect(kinds.get("Math")).toBe("type");
    expect(kinds.get("2")).toBe("number");
    expect(kinds.get('// "slack"')).toBe("comment");
  });

  it("reads Rust lifetimes and macros, and keeps quotes in TypeScript as strings", () => {
    const rust = tokenizeLine("fn read<'a>(s: &'a str) { println!(\"{s}\"); }", "rust");
    expect(rust.find((token) => token.text === "'a")?.kind).toBe("keyword");
    expect(rust.find((token) => token.text === "println!")?.kind).toBe("call");
    const ts = tokenizeLine("const a = 'it is';", "ts");
    expect(ts.find((token) => token.text === "'it is'")?.kind).toBe("string");
  });

  it("treats # as a comment only in shell-like languages", () => {
    expect(tokenizeLine("# install", "sh")).toEqual([{ kind: "comment", text: "# install" }]);
    expect(tokenizeLine("#[derive(Debug)]", "rust").some((token) => token.kind === "comment")).toBe(
      false,
    );
  });
});

describe("tool runs", () => {
  const read: ToolCallModel = {
    id: "call-read",
    kind: "read",
    title: "Read packages/ui/src/orb/pacing.ts",
    status: "completed",
    label: "Read pacing.ts",
    pastLabel: "Read pacing.ts",
    target: "packages/ui/src/orb/pacing.ts",
    locations: [],
    rawInput: null,
    content: [],
    diffs: [],
    terminal: null,
    search: null,
    text: null,
    awaitingPermission: false,
    startedAt: 0,
    endedAt: 100,
  };

  it("keeps a finished call in the present tense while its run is the agent's latest step", () => {
    expect(activeLabel(read)).toBe("Reading pacing.ts");
    expect(activeLabel({ ...read, kind: "search", target: "performance.now" })).toBe(
      'Searching for "performance.now"',
    );
    expect(activeLabel({ ...read, status: "in_progress", label: "Reading now" })).toBe(
      "Reading now",
    );
    // A call with nothing to name keeps the agent's own words.
    expect(activeLabel({ ...read, target: null })).toBe("Read pacing.ts");
  });

  it("turns a terminal background into a quiet tag in the full colour", () => {
    const style = spanStyle({
      text: " RUN ",
      fg: "black",
      bg: "cyan",
      bold: true,
      dim: false,
      italic: false,
      underline: false,
      strikethrough: false,
      inverse: false,
    });
    expect(style?.color).toBe("#8fd0d8");
    expect(style?.backgroundColor).toBe("color-mix(in oklab, #8fd0d8 20%, transparent)");
    expect(style?.fontWeight).toBe(600);
  });
});

describe("the handoff block", () => {
  it("carries the prompt, the other agent's final answer and the files, and nothing else", () => {
    const handoff = handoffFor(goldenAt(), "claude-code", "codex");
    expect(handoff).not.toBeNull();
    if (!handoff) return;
    expect(handoff.fromName).toBe("Claude Code");
    expect(handoff.toName).toBe("Codex");
    expect([handoff.prompts, handoff.answers, handoff.files]).toEqual([1, 1, 2]);
    expect(handoff.block).toContain("User:\nThe orb pacing test fails about 1 in 20 runs on CI.");
    expect(handoff.block).toContain("Claude Code:\nFixed. The flake was in the test");
    // Only the final answer goes, not the explanation that came before the fix.
    expect(handoff.block).not.toContain("Found it.");
    expect(handoff.block).toContain(" 2 files changed, 59 insertions(+), 23 deletions(-)");
  });

  it("is only for a switch, in a chat that has something to hand over", () => {
    expect(handoffFor(goldenAt(), "claude-code", "claude-code")).toBeNull();
    expect(handoffFor(reduceTranscript([]), "claude-code", "codex")).toBeNull();
  });

  it("clips long answers at a word and says so", () => {
    expect(clip("short")).toBe("short");
    const clipped = clip("word ".repeat(400));
    expect(clipped.endsWith(" [cut]")).toBe(true);
    expect(clipped.length).toBeLessThanOrEqual(906);
  });

  it("draws the files as git diff --stat does", () => {
    expect(
      diffStat([
        { path: "a.ts", added: 2, removed: 1, isNew: false },
        { path: "lib/b.ts", added: 30, removed: 18, isNew: false },
      ]),
    ).toBe(
      [
        " a.ts     |  3 +-",
        ` lib/b.ts | 48 ${"+".repeat(15)}${"-".repeat(9)}`,
        " 2 files changed, 32 insertions(+), 19 deletions(-)",
      ].join("\n"),
    );
  });
});

describe("the plan", () => {
  it("joins its turn where it last changed: after the test run, before the answer", () => {
    const model = goldenAt();
    const turn = model.turns[0];
    expect(turn && model.plan).toBeTruthy();
    if (!turn || !model.plan) return;
    const slot = planSlot(turn.items, model.plan);
    expect(turn.items[slot]?.kind).toBe("tools");
    expect(turn.items[slot + 1]?.kind).toBe("message");
  });

  it("sits in the dock only while the agent works through it", () => {
    const working = render("streaming", 24);
    expect(working).toContain('class="n-plan-wrap"');
    expect(working).toContain("Add a regression test");
    expect(working).toContain("2 of 3");
    const done = render("streaming", 27.5);
    expect(done).not.toContain('class="n-plan-wrap"');
    expect(done).toContain("3 of 3 done");
  });
});

describe("the Noir window", () => {
  const emDash = String.fromCodePoint(0x2014);

  it("renders every lab state from the props alone, with no em dash in its copy", () => {
    for (const state of LAB_STATES) {
      const html = render(state);
      expect(html).toContain('class="noir"');
      expect(html).toContain(`data-state="${state}"`);
      expect(html.includes(emDash)).toBe(false);
    }
  });

  it("shows the finished turn: steps in order, serif lead-ins, the footer, no dock plan", () => {
    const html = render("settled");
    expect(html).toContain("Fix the flaky orb pacing test");
    expect(html).toContain("All 19 pacing tests pass.");
    expect(html).toContain('class="n-runin"');
    expect(html).toContain("2 files changed");
    expect(html).toContain("$0.21");
    expect(html).toContain("Thought for 6s");
    expect(html).not.toContain("n-caret");
    expect(html).not.toContain('class="n-plan-wrap"');
    expect(at(html, "Ran pnpm")).toBeLessThan(at(html, "3 of 3 done"));
    expect(at(html, "3 of 3 done")).toBeLessThan(at(html, "Fixed. The flake"));
    // History opens folded: the passed command shows its line, not its output.
    expect(html).not.toContain('class="n-term"');
    // The agent is done: its byline orb is a still poster, not a live one.
    expect(html).not.toContain("n-reply-orb-live");
  });

  it("streams: thinking with a caret and a live orb, then tools at work", () => {
    const thinking = render("streaming", 3);
    expect(thinking).toContain("Thinking");
    expect(thinking).toContain("n-caret");
    expect(thinking).toContain('aria-label="Stop"');
    expect(thinking).toContain("to stop");
    expect(thinking).toContain("n-reply-orb-live");
    expect(thinking).toContain('aria-busy="true"');
    expect(thinking).toContain("Claude Code is working");
    const working = render("streaming", 24);
    expect(working).toContain("Running pnpm --filter @ghost/ui test pacing");
  });

  it("keeps a passed command open while its turn runs, down to its last lines", () => {
    const html = render("streaming", 26.8);
    expect(html).toContain('class="n-term"');
    expect(html).toContain("exit 0");
    expect(html).toContain("data-short");
  });

  it("holds an exploration on what it is doing until the next step starts", () => {
    // 9.0 s: the first read is done and the second has not started.
    const between = render("streaming", 8.8);
    expect(between).toContain("Reading pacing.ts");
    expect(between).not.toContain("Read pacing.ts");
    // Once the answer starts, the run reads as its summary.
    expect(render("streaming", 12)).toContain("Read 2 files, searched 1 pattern");
  });

  it("shows a placeholder step before the agent's first output", () => {
    expect(render("streaming", 1)).toContain('data-kind="starting"');
    expect(render("streaming", 3)).not.toContain('data-kind="starting"');
  });

  it("says each chat's state in words where it needs the reader", () => {
    const html = render("streaming", 10);
    expect(html).toContain("n-chat-working");
    expect(html).toContain("Approve");
    expect(html).toContain("n-chat-unseen");
    expect(render("settled")).not.toContain("n-chat-working");
  });

  it("asks for approval with an amber card and safe keys, and hides the run it covers", () => {
    const html = render("permission");
    expect(html).toContain("Approval needed");
    expect(html).toContain("Run cargo test -p aura-drivers?");
    expect(html).toContain("Always allow");
    expect(html).toContain("⌘⏎");
    expect(html).toContain("Esc");
    expect(html).not.toContain("Waiting to run cargo test");
    expect(html).toContain("Auto-edit");
    // Esc belongs to the card here, so the composer does not offer it for stopping.
    expect(html).not.toContain("to stop");
    // What is typed now goes instead of the request.
    expect(html).toContain("Or tell Codex what to do instead");
    expect(html).toContain("Codex needs approval: Run cargo test -p aura-drivers?");
  });

  it("opens the picker with every agent in labelled groups, and quiet Update and Install", () => {
    const html = render("picker");
    for (const text of ["Claude Code", "Codex", "Grok Build", "Opus 5.5", "Extra high"]) {
      expect(html).toContain(text);
    }
    expect(html.match(/role="radiogroup"/g)?.length).toBe(3);
    expect(html).toContain('aria-label="Update Codex to 0.165.0"');
    expect(html).toContain("Install");
    expect(html).toContain("data-picker");
  });

  it("greets a new chat with the serif line, a project menu and a composer", () => {
    const html = render("empty");
    expect(html).toContain("What should we build in");
    expect(html).toContain("<em>auracode</em>");
    expect(html).toContain('aria-haspopup="menu"');
    // The caret is drawn only once the field really has focus (in the browser, on mount).
    expect(html).not.toContain("n-caret-idle");
    expect(html).toContain("data-composer-field");
    expect(html).not.toContain('role="log"');
  });

  it("sets notices and errors inside their turn, coloured by level", () => {
    const scene = sceneFor("streaming");
    const events: TimedEvent[] = [
      ...GOLDEN.events.slice(0, eventsAt(GOLDEN.events, 9000)),
      { at: 9100, event: { type: "notice", level: "warning", message: "Context is 80% full." } },
      {
        at: 9200,
        event: { type: "error", message: "The agent stopped answering.", recoverable: true },
      },
    ];
    const model = reduceTranscript(events);
    const html = renderToString(
      <App
        state="streaming"
        model={model}
        workspace={workspaceFor(scene, model)}
        player={staticPlayer(GOLDEN.duration, 9200)}
      />,
    );
    expect(html).toContain('data-level="warning"');
    expect(html).toContain("Context is 80% full.");
    expect(html).toContain('data-level="error"');
    expect(html).toContain('role="alert"');
  });

  it("opens a failed command's output with its exit code", () => {
    const scene = sceneFor("streaming");
    const events: TimedEvent[] = [
      ...GOLDEN.events.slice(0, eventsAt(GOLDEN.events, 22500)),
      {
        at: 23100,
        event: {
          type: "tool_call",
          tool_call_id: "call-failing",
          kind: "execute",
          title: "pnpm lint",
          status: "in_progress",
          locations: [],
          raw_input: { command: "pnpm lint" },
        },
      },
      {
        at: 23500,
        event: {
          type: "tool_update",
          tool_call_id: "call-failing",
          status: "failed",
          title: null,
          content: [
            {
              type: "terminal",
              output: "\u001b[31m1 error\u001b[39m",
              exit_code: 1,
              truncated: false,
            },
          ],
        },
      },
    ];
    const model = reduceTranscript(events);
    const html = renderToString(
      <App
        state="streaming"
        model={model}
        workspace={workspaceFor(scene, model)}
        player={staticPlayer(GOLDEN.duration, 23500)}
      />,
    ).replaceAll("<!-- -->", "");
    expect(html).toContain('data-status="failed"');
    expect(html).toContain("exit 1");
    expect(html).toContain("1 error");
    // A failure is never cut down to its tail.
    expect(html).not.toContain("data-short");
  });
});
