import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { APPROVAL_REQUEST_ID } from "../../fixture/approval";
import { GOLDEN, GOLDEN_PROMPT } from "../../fixture/golden";
import { sceneFor, workspaceFor } from "../../fixture/scenes";
import type { TimedEvent } from "../../fixture/types";
import { BASE_WORKSPACE } from "../../fixture/workspace";
import { LAB_STATES, type LabState } from "../../lab/states";
import { reduceTranscript } from "../../model/transcript";
import type { PermissionModel, Turn } from "../../model/types";
import { staticPlayer } from "../../model/usePlayer";
import { App } from "./App";
import { buildHandoff } from "./Handoff";
import { languageOf, tokenizeLine } from "./highlight";
import { closeOpenMarkers } from "./ink";
import { hintFor, optionForKey } from "./Permission";
import { effortFor, recall } from "./Picker";
import { arrange, autonomyOfMode } from "./Transcript";
import { DiffStat } from "./ui";

/** Server-renders Aurora in a lab state, at the end of its recording (or at `at` ms). */
function render(state: LabState, at?: number): string {
  const scene = sceneFor(state);
  const events = scene.recording?.events ?? [];
  const time = at ?? scene.recording?.duration ?? 0;
  const model = reduceTranscript(events.filter((entry) => entry.at <= time));
  const workspace = workspaceFor(scene, model);
  return renderToString(
    <App state={state} model={model} workspace={workspace} player={staticPlayer(time, time)} />,
  );
}

/** Visible text of server markup, tags dropped and entities decoded enough for these checks. */
function text(html: string): string {
  return html
    .replace(/<[^>]+>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'");
}

function modelAt(state: LabState, at?: number) {
  const scene = sceneFor(state);
  const events = scene.recording?.events ?? [];
  const time = at ?? scene.recording?.duration ?? 0;
  return reduceTranscript(events.filter((entry) => entry.at <= time));
}

describe("Aurora renders every lab state", () => {
  it.each(LAB_STATES)("%s renders without throwing", (state) => {
    expect(render(state).length).toBeGreaterThan(1000);
  });

  it("settles the golden turn: thought, tools, answer, footer, and the plan in the footer", () => {
    const shown = text(render("settled"));
    expect(shown).toContain("Thought for 6s");
    expect(shown).toContain("Read 2 files, searched 1 pattern");
    expect(shown).toContain("Edited 2 files");
    expect(shown).toContain("Done in 48s");
    expect(shown).toContain("2 files changed");
    expect(shown).toContain("$0.21");
    expect(shown).toContain("Plan 3 of 3");
    expect(shown).toContain("Claude Code · Opus 5.5 · High");
  });

  it("folds a passing command to its command line once the turn moves on", () => {
    const shown = text(render("settled"));
    expect(shown).toContain("pnpm --filter @ghost/ui test pacing");
    expect(shown).toContain("Exit 0");
    expect(shown).not.toContain("19 passed");
  });

  it("keeps a running command open, with its output", () => {
    const shown = text(render("streaming", 24_000));
    expect(shown).toContain("pnpm --filter @ghost/ui test pacing");
    expect(shown).toContain("Running");
    expect(shown).toContain("vitest run pacing");
  });

  it("docks the live plan as one line, and only while its turn runs", () => {
    const live = text(render("streaming", 24_000));
    expect(live).toContain("Add a regression test");
    expect(live).toContain("2 of 3");
    // Folded by default: the other steps open on click.
    expect(live).not.toContain("Reproduce the flake");
    expect(text(render("settled"))).not.toContain("Add a regression test");
  });

  it("streams: a live thought counts its seconds, never 0s, and Send is Stop", () => {
    const early = text(render("streaming", 1_800));
    expect(early).toContain("Thinking");
    expect(early).not.toMatch(/Thinking\s*0s/);
    expect(text(render("streaming", 2_800))).toMatch(/Thinking\s*1s/);
    const html = render("streaming", 7_500);
    expect(html).toContain('aria-label="Stop"');
    expect(html).not.toContain('aria-label="Send"');
  });

  it("asks for approval with a card that stands in for the run it holds", () => {
    const html = render("permission");
    const shown = text(html);
    expect(shown).toContain("Codex needs your approval");
    expect(shown).toContain("cargo test -p aura-drivers");
    for (const label of ["Allow once", "Always allow", "Reject", "esc"]) {
      expect(shown).toContain(label);
    }
    expect(shown).not.toContain("Waiting to run");
    expect(html).toContain('placeholder="Enter to allow, Esc to reject, or type a reply"');
    // The reason comes from the turn's own mode (on-request), not the composer.
    expect(shown).toContain("Auto-edit asks before running commands.");
  });

  it("leaves out a zero side of a diff stat", () => {
    const shown = text(render("permission"));
    expect(shown).toContain("+69");
    expect(shown).not.toContain("−0");
    expect(text(renderToString(<DiffStat added={38} removed={0} />))).toBe("+38");
    expect(text(renderToString(<DiffStat added={0} removed={0} />))).toBe("+0");
  });

  it("shows what a waiting chat wants in the sidebar", () => {
    expect(text(render("settled"))).toContain("Approve");
  });

  it("opens the picker with every agent, the models and the efforts", () => {
    const html = render("picker");
    const shown = text(html);
    for (const agent of BASE_WORKSPACE.agents) expect(shown).toContain(agent.name);
    for (const label of ["Opus 5.5", "Sonnet 5.5", "Haiku 5.5", "Extra high", "Install"]) {
      expect(shown).toContain(label);
    }
    expect(shown).toContain("High · Default");
    expect(html).toContain('role="listbox"');
    expect(html).toContain('role="option"');
  });

  it("greets a new chat", () => {
    const shown = text(render("empty"));
    expect(shown).toContain("What should we build?");
    expect(shown).toContain("Fix a failing test");
  });

  it("names the conversation for screen readers and keeps the badge on the turn's own orb", () => {
    const html = render("settled");
    expect(html).toContain('aria-label="Conversation"');
    expect(html).toContain('data-tip="Full auto"');
    expect(html).toContain('aria-live="polite"');
  });
});

describe("a turn that fails", () => {
  it("shows its notices and how it ended", () => {
    const events: TimedEvent[] = [
      {
        at: 0,
        event: { type: "user_message", message_id: "m1", text: "Tidy the tray", agent: "pi" },
      },
      { at: 100, event: { type: "turn_started", turn_id: "t1" } },
      {
        at: 200,
        event: { type: "notice", level: "warning", message: "Pi never asks before acting." },
      },
      { at: 300, event: { type: "message_delta", message_id: "a", text: "Looking at **tray" } },
      { at: 400, event: { type: "error", message: "The agent exited early.", recoverable: false } },
      { at: 500, event: { type: "turn_ended", turn_id: "t1", stop_reason: "error", usage: null } },
    ];
    const model = reduceTranscript(events);
    const scene = sceneFor("settled");
    const html = renderToString(
      <App
        state="settled"
        model={model}
        workspace={workspaceFor(scene, model)}
        player={staticPlayer(500, 500)}
      />,
    );
    const shown = text(html);
    expect(shown).toContain("Pi never asks before acting.");
    expect(shown).toContain("The agent exited early.");
    expect(shown).toContain("Failed after");
    expect(html).toContain('role="alert"');
  });
});

describe("permission cards and the runs they hold", () => {
  const turn = (): Turn => {
    const last = modelAt("permission").turns.at(-1);
    if (!last) throw new Error("no turn");
    return last;
  };
  const card = (current: Turn): PermissionModel => {
    const item = current.items.find((candidate) => candidate.kind === "permission");
    if (item?.kind !== "permission") throw new Error("no card");
    return item.permission;
  };

  it("hides the held run while the card waits", () => {
    const current = turn();
    const arrangement = arrange(current, null);
    const run = current.items.find(
      (item) => item.kind === "tools" && item.calls.some((call) => call.awaitingPermission),
    );
    expect(run && arrangement.hiddenRuns.has(run.id)).toBe(true);
    expect([...arrangement.cards.values()]).toContain("card");
  });

  it("brings the run back with the answer as a chip once answered here", () => {
    const current = turn();
    const option = optionForKey(card(current).options, "reject");
    if (!option) throw new Error("no reject option");
    const arrangement = arrange(current, { requestId: APPROVAL_REQUEST_ID, option });
    expect(arrangement.hiddenRuns.size).toBe(0);
    expect([...arrangement.cards.values()]).toEqual(["none"]);
    expect([...arrangement.runAnswers.values()]).toEqual([
      { allowed: false, label: "Rejected", local: true },
    ]);
  });

  it("gives each key one answer, with Command while the composer holds text", () => {
    const options = card(turn()).options;
    expect(optionForKey(options, "allow")?.kind).toBe("allow_once");
    expect(optionForKey(options, "always")?.kind).toBe("allow_always");
    expect(optionForKey(options, "reject")?.kind).toBe("reject_once");
    const allow = options[0];
    if (!allow) throw new Error("no option");
    expect(hintFor(allow, options)).toBe("↵");
    expect(hintFor(allow, options, true)).toBe("⌘ ↵");
  });
});

describe("what the next message goes with", () => {
  it("reads a session's mode as the autonomy it means", () => {
    expect(autonomyOfMode("bypassPermissions")).toBe("full_auto");
    expect(autonomyOfMode("on-request")).toBe("auto_edit");
    expect(autonomyOfMode("acceptEdits")).toBe("auto_edit");
    expect(autonomyOfMode("default")).toBe("ask");
    expect(autonomyOfMode("something-new")).toBeNull();
    expect(autonomyOfMode(null)).toBeNull();
  });

  it("recalls an agent's last model and effort, else its defaults", () => {
    const empty = new Map();
    expect(recall(BASE_WORKSPACE, empty, "codex")).toEqual({
      agent: "codex",
      model: "gpt-5.5-codex",
      effort: "medium",
    });
    const remembered = new Map([["codex" as const, { model: "gpt-5.5-mini", effort: "high" }]]);
    expect(recall(BASE_WORKSPACE, remembered, "codex")).toEqual({
      agent: "codex",
      model: "gpt-5.5-mini",
      effort: "high",
    });
    // Not installed: nothing to select.
    expect(recall(BASE_WORKSPACE, empty, "cursor")).toBeNull();
  });

  it("keeps an effort the model offers, else falls back to the model's default", () => {
    const haiku = BASE_WORKSPACE.agents[0]?.models[2] ?? null;
    expect(effortFor(haiku, "low")).toBe("low");
    expect(effortFor(haiku, "max")).toBe("low");
    expect(effortFor(null, "high")).toBeNull();
  });

  it("hands off the chat so far: what was asked, the answer, the files", () => {
    const model = reduceTranscript(GOLDEN.events);
    const handoff = buildHandoff(model, [], "claude-code", "pi");
    expect(handoff.asked).toEqual([GOLDEN_PROMPT]);
    expect(handoff.answers).toHaveLength(1);
    expect(handoff.answers[0]?.agent).toBe("claude-code");
    expect(handoff.answers[0]?.text.startsWith("Fixed. The flake was in the test")).toBe(true);
    expect(handoff.answers[0]?.text.length).toBeLessThanOrEqual(281);
    expect(handoff.answers[0]?.text).not.toContain("`");
    expect(handoff.files.map((file) => file.path)).toEqual([
      "packages/ui/src/orb/pacing.ts",
      "packages/ui/src/orb/pacing.test.ts",
    ]);
    // The receiving agent's own answers are not handed back to it.
    expect(buildHandoff(model, [], "pi", "claude-code").answers).toHaveLength(0);
  });
});

describe("closeOpenMarkers", () => {
  it("closes bold, code and italics that a stream left open, innermost first", () => {
    expect(closeOpenMarkers("**Jitter tole")).toBe("**Jitter tole**");
    expect(closeOpenMarkers("reads `performance.n")).toBe("reads `performance.n`");
    expect(closeOpenMarkers("**now `slack")).toBe("**now `slack`**");
    expect(closeOpenMarkers("an *early")).toBe("an *early*");
  });

  it("keeps closers before trailing spaces, and drops a marker with nothing after it", () => {
    expect(closeOpenMarkers("*foo ")).toBe("*foo* ");
    expect(closeOpenMarkers("done **")).toBe("done ");
    expect(closeOpenMarkers("a `")).toBe("a ");
  });

  it("leaves balanced text and snake_case alone", () => {
    for (const done of ["**bold** and `code`", "frame_interval_ms", "2 * 3", ""]) {
      expect(closeOpenMarkers(done)).toBe(done);
    }
  });
});

describe("tokenizeLine", () => {
  it("never loses a character", () => {
    const lines = [
      "export function shouldDrawFrame(now: number, lastDrawAt: number, fps: number): boolean {",
      "  return elapsed >= interval - slackMs(interval); // 0x1f, 16.7ms",
      "let line = serde_json::to_vec(&frame)?; line.push(b'\\n'); fn x<'a>(s: &'a str) {}",
      "pnpm --filter @ghost/ui test pacing # comment",
      // A template literal with a placeholder, built so the source has no "${" in a plain string.
      `\`template $${"{"}literal}\` + 'single' + "double`,
    ];
    for (const line of lines) {
      for (const language of ["ts", "rust", "shell", "plain"] as const) {
        expect(
          tokenizeLine(line, language)
            .map((token) => token.text)
            .join(""),
        ).toBe(line);
      }
    }
  });

  it("tints keywords, calls, strings and comments", () => {
    const kinds = tokenizeLine('const x = run("a"); // done', languageOf("ts")).map(
      (token) => `${token.kind}:${token.text}`,
    );
    expect(kinds).toContain("kw:const");
    expect(kinds).toContain("fn:run");
    expect(kinds).toContain('str:"a"');
    expect(kinds).toContain("com:// done");
  });
});
