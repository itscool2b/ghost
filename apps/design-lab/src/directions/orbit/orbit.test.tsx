import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { APPROVAL } from "../../fixture/approval";
import { DISCOVERY } from "../../fixture/discovery";
import { GOLDEN } from "../../fixture/golden";
import { sceneDuration, sceneFor, workspaceFor } from "../../fixture/scenes";
import { LAB_STATES, type LabState, type LabTheme } from "../../kit";
import { eventsAt, reduceTranscript } from "../../model/transcript";
import { staticPlayer } from "../../model/usePlayer";
import { App } from "./App";
import { labelLines } from "./Globe";
import { handoffTokens, modelNowRow, NOW_ROW_DELAY_MS, requestTarget } from "./live";
import { failedHistory, localEvents, SCRIPTED_NOTE, scriptFor } from "./local";
import { meta } from "./meta";
import { withParam } from "./nav";
import { changedFiles } from "./RightPanel";
import { agentCode, projectTone } from "./Sidebar";
import { lineTone } from "./Tools";

const FOLDER = fileURLToPath(new URL(".", import.meta.url));

/** Orbit's window for one lab state at one moment (seconds, or the state's own end). */
function render(state: LabState, theme: LabTheme = "dark", seconds?: number): string {
  const scene = sceneFor(state);
  const events = scene.recording?.events ?? [];
  const duration = sceneDuration(scene);
  const time = seconds === undefined ? duration : seconds * 1000;
  const model = reduceTranscript(events.slice(0, eventsAt(events, time)));
  const discovered = scene.discovery ? eventsAt(scene.discovery.events, time) : undefined;
  const workspace = workspaceFor(scene, model, discovered);
  return renderToString(
    <App
      state={state}
      model={model}
      workspace={workspace}
      player={staticPlayer(duration, time)}
      theme={theme}
    />,
  )
    .replaceAll("<!-- -->", "")
    .replaceAll("&#x27;", "'")
    .replaceAll("&quot;", '"');
}

/** The rendered page's words, without markup. */
function text(html: string): string {
  return html
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

describe("Orbit's name card", () => {
  it("renders every lab state, in both themes, on a neutral desk", () => {
    expect(meta.states).toEqual(LAB_STATES);
    expect(meta.themes).toEqual(["dark", "light"]);
    expect(meta.wallpaper).toBe("satin");
    expect(meta.tagline).toContain("one orange for look here");
  });
});

describe("every state in both themes", () => {
  const emDash = String.fromCodePoint(0x2014);
  for (const state of LAB_STATES) {
    for (const theme of ["dark", "light"] as const) {
      it(`${state} in ${theme}`, () => {
        const html = render(state, theme, state === "onboarding" ? 1.6 : undefined);
        expect(html).toContain('class="o-root"');
        expect(html).toContain(`data-theme="${theme}"`);
        expect(html).toContain(`data-state="${state}"`);
        expect(html.includes(emDash)).toBe(false);
      });
    }
  }
});

describe("the settled turn", () => {
  const shown = text(render("settled"));

  it("shows the prompt as the one ink block, then the byline in three tones", () => {
    expect(shown).toContain("The orb pacing test fails about 1 in 20 runs on CI.");
    expect(render("settled")).toContain('class="o-user-block o-t-body"');
    expect(shown).toContain("Claude Code Opus 5.5 High");
  });

  it("folds finished work to single lines with numbers", () => {
    expect(shown).toContain("Thought for 6s");
    expect(shown).toContain("Read 2 files, searched 1 pattern");
    expect(shown).toContain("Edited 2 files");
    expect(shown).toContain("Ran pnpm --filter @ghost/ui test pacing 19 passed");
    expect(shown).toContain("Plan complete 3 of 3 steps");
  });

  it("ends on the receipt: duration, tokens, cost and the files chip", () => {
    expect(shown).toContain("Done in 48s · 4.8k in · 2.1k out · $0.21");
    expect(shown).toContain("2 files +59 −23");
    expect(shown).toContain("All 19 pacing tests pass.");
  });

  it("uses no italic tags, and no live labels once the turn is done", () => {
    const html = render("settled");
    expect(html).not.toContain("<em");
    expect(html).not.toContain("<i>");
    expect(html).not.toContain("o-shimmer");
    expect(html).not.toContain('data-mode="stop"');
  });
});

describe("the streaming turn", () => {
  it("thinks with a live line and no now-row while the thought streams", () => {
    const html = render("streaming", "dark", 3);
    expect(text(html)).toContain("Thinking · 1s");
    expect(html).toContain("o-shimmer");
    expect(html).not.toContain('class="o-now"');
    expect(html).toContain('data-mode="stop"');
  });

  it("runs the tests under the plan, with the now-row and the hint line", () => {
    const html = render("streaming", "light", 24);
    const shown = text(html);
    // The open command is one block whose header is its row: said once, and opening it moves
    // nothing. Its output unfolds under the row while it runs.
    expect(shown.match(/Running pnpm --filter @ghost\/ui test pacing/g)).toHaveLength(1);
    expect(shown).toContain("vitest run pacing");
    expect(html).toContain('class="o-call o-cmd" data-open=""');
    // The agent's live mark is in the command's gutter, not on a line of its own.
    expect(html).not.toContain("o-term-gutter");
    expect(html).not.toContain('class="o-now"');
    expect(shown).toContain("Plan 2/3 Add a regression test");
    // The header's diffstat shows the edits as soon as they land, before the turn ends.
    expect(html).toContain('class="o-diff-chip"');
    expect(shown).toContain("esc to stop · enter to queue · ⌘ enter to steer");
    // The command's output is open while it runs: the last lines, live.
    expect(html).toContain('class="o-term-body o-scroll"');
  });
});

describe("the permission card", () => {
  const html = render("permission");
  const shown = text(html);

  it("takes the composer's place with X Money's sentence and the facts", () => {
    expect(html).not.toContain('class="o-composer"');
    expect(shown).toContain("Codex needs you");
    expect(shown).toContain("Run cargo test -p aura-drivers in auracode");
    expect(shown).toContain("Auto-edit asks before running commands.");
    expect(shown).toContain("Approved 3 times in this project");
  });

  it("offers the real answers with their keys, and a way to say what instead", () => {
    // Allow once answers to 1 and to Enter, and says both.
    expect(shown).toContain("Allow once 1 ↵");
    expect(shown).toContain("Allow in this chat 2");
    expect(shown).toContain("Always allow in auracode 3");
    expect(shown).toContain("Deny esc");
    expect(html).toContain('placeholder="Tell Codex what to do instead"');
  });

  it("marks the wait in the transcript and the sidebar in orange", () => {
    // The state holds 9 s past the request, so the wait reads 0:09.
    expect(shown).toContain("Waiting on you · 0:09");
    // While the card is up, the call it asks about is said once: by the card.
    expect(shown).not.toContain("Waiting to run cargo test -p aura-drivers");
    expect(shown).toContain("Needs you 1");
  });

  it("takes focus on the card itself, not on Allow once, so no ring sits on a button at rest", () => {
    expect(html).toContain('role="alertdialog" tabindex="-1"');
  });
});

describe("the picker", () => {
  const shown = text(render("picker"));

  it("lists agents with versions and letters, missing ones last", () => {
    expect(shown).toContain("Claude Code 2.1.310 · signed in C");
    expect(shown).toContain("Codex 0.164.0 → 0.165.0 X");
    expect(render("picker")).toContain('title="Update to 0.165.0"');
    expect(shown).toContain("Not installed Cursor Not installed Install");
  });

  it("shows the models with one line each, and the effort ladder with keys", () => {
    expect(shown).toContain("Opus 5.5 Most capable 1M");
    expect(shown).toContain("Sonnet 5.5 Fast, everyday coding 1M");
    expect(shown).toContain("High default 3");
    expect(shown).toContain("High: thinks longer on hard steps; uses limits faster");
  });
});

describe("the empty state", () => {
  it("is one sentence, the composer and recent work", () => {
    const shown = text(render("empty", "light"));
    expect(shown).toMatch(/Claude Code\s+is ready in\s+auracode\s*\./);
    expect(shown).toContain("Recent in auracode");
    expect(shown).toContain("Wire up the ACP client Codex Needs you 2m");
    expect(render("empty")).not.toContain('class="o-user-block');
  });
});

describe("the Changes panel", () => {
  const html = render("changes");
  const shown = text(html);

  it("opens on this turn with the diffstat as a hero and the renderer's diff open", () => {
    expect(html).toContain('class="o-panel"');
    expect(shown).toContain("+59 −23");
    expect(shown).toContain("2 files · 48s · Claude Code");
    expect(html).toContain('data-open=""');
    expect(shown).toContain("@@ −55,6 +55,14 @@");
    expect(shown).toContain("Checkpoint saved before this turn");
  });

  it("lists each changed file once, latest edit, in the order first edited", () => {
    const model = reduceTranscript(GOLDEN.events);
    const files = changedFiles(model.turns[0] ?? null);
    expect(files.map((file) => [file.diff.path, file.diff.added, file.diff.removed])).toEqual([
      ["packages/ui/src/orb/pacing.ts", 19, 5],
      ["packages/ui/src/orb/pacing.test.ts", 40, 18],
    ]);
    expect(files.every((file) => file.newText !== null)).toBe(true);
    expect(changedFiles(null)).toEqual([]);
  });
});

describe("onboarding", () => {
  it("fills the roster as agents are found, with Hermes landing at 1.6 s", () => {
    const shown = text(render("onboarding", "dark", 1.6));
    expect(shown).toContain("Found 5 agents. Still looking for 2.");
    expect(shown).toContain("Antigravity /usr/local/bin/agy · 1.4.7 Sign in");
    // Every agent has its row from the first frame; the ones still being looked for say so.
    expect(shown).toContain("Looking for Cursor Looking for Grok Build");
    expect(shown).toContain("Agents 5/7 · Live");
    expect(shown).toContain("Agents run on this device. auracode never sees their keys.");
  });

  it("settles on what can be installed once the scan is done", () => {
    const shown = text(render("onboarding", "light", DISCOVERY.duration / 1000));
    expect(shown).toContain("Found 5 agents. 2 more can be installed.");
    expect(shown).toContain("Scanned PATH, ~/.local/bin and /opt/homebrew/bin in 2.6s");
    expect(shown).not.toContain("Looking for");
  });

  it("shows no sidebar and no composer before there is a chat", () => {
    const html = render("onboarding", "dark", 1.6);
    expect(html).not.toContain('class="o-sidebar"');
    expect(html).not.toContain('class="o-composer"');
  });
});

describe("the live end", () => {
  it("names what runs, what waits, and nothing once the turn is done", () => {
    const at = (seconds: number) =>
      reduceTranscript(GOLDEN.events.slice(0, eventsAt(GOLDEN.events, seconds * 1000)));
    // A running tool is its own live line.
    expect(modelNowRow(at(24), "claude-code")).toBeNull();
    expect(modelNowRow(at(10.5), "claude-code")).toMatchObject({
      state: "thinking",
      verb: "Thinking",
      since: 9900,
    });
    expect(modelNowRow(at(28.5), "claude-code")).toMatchObject({ state: "thinking" });
    // A streaming thought or message is its own live line.
    expect(modelNowRow(at(3), "claude-code")).toBeNull();
    expect(modelNowRow(at(60), "claude-code")).toBeNull();
    const waiting = reduceTranscript(APPROVAL.events);
    expect(modelNowRow(waiting, "codex")).toMatchObject({
      state: "waiting",
      verb: "Waiting on you",
      since: 10_000,
    });
  });

  it("says what a request is about, and sizes a handoff in tens of tokens", () => {
    const waiting = reduceTranscript(APPROVAL.events);
    const request = waiting.pendingPermissions[0];
    expect(request && requestTarget(request)).toBe("cargo test -p aura-drivers");
    const tokens = handoffTokens(reduceTranscript(GOLDEN.events));
    expect(tokens % 10).toBe(0);
    expect(tokens).toBeGreaterThan(400);
    expect(handoffTokens(reduceTranscript([]))).toBe(10);
  });
});

describe("the anti-slop rules, in the source", () => {
  const css = readdirSync(FOLDER)
    .filter((name) => name.endsWith(".css"))
    .map((name) => readFileSync(join(FOLDER, name), "utf8"))
    .join("\n");
  const tsx = readdirSync(FOLDER)
    .filter((name) => name.endsWith(".tsx") && !name.endsWith(".test.tsx"))
    .map((name) => readFileSync(join(FOLDER, name), "utf8"))
    .join("\n");

  it("has no serif, no italic, no glow and no frosted glass", () => {
    expect(css).not.toMatch(/font-style:\s*italic/);
    expect(css).not.toMatch(/Instrument Serif|--font-display|(?<!sans-)serif/);
    expect(css).not.toMatch(/radial-gradient/);
    expect(css).not.toMatch(/(?<!backdrop-)filter:\s*blur/);
    expect(css).not.toMatch(/text-shadow/);
    // Blur appears in one place: the toast (section 6), with the WebKit prefix beside it.
    const blurred = [...css.matchAll(/([^{}]+)\{[^{}]*backdrop-filter[^{}]*\}/g)].map((rule) =>
      (rule[1] ?? "").trim(),
    );
    expect(blurred).toEqual(['[data-direction="orbit"] .o-toast']);
    expect(css).toMatch(/-webkit-backdrop-filter: blur\(16px\) saturate\(1\.2\)/);
  });

  it("draws its own marks: no glossy orb and no aura field", () => {
    expect(tsx).not.toMatch(/<Orb\b|<AuraField\b|GlassPanel/);
    expect(tsx).not.toMatch(/Sparkles|WandSparkles|\bStar\b/);
  });

  it("spends hue only on signal, green, red and the code palette", () => {
    // Every hex colour outside the token file is black, white or the night panel's orange.
    const outside =
      readFileSync(join(FOLDER, "orbit.css"), "utf8").match(/#[0-9a-f]{3,8}\b/gi) ?? [];
    const allowed = new Set(["#000", "#fff", "#ff6308", "#ff7a2e", "#f0f0f0", "#252524"]);
    expect(outside.filter((hex) => !allowed.has(hex.toLowerCase()))).toEqual([]);
  });

  it("reads time from the player, never the wall clock", () => {
    expect(tsx).not.toMatch(/Date\.now|performance\.now|new Date\(/);
  });

  it("styles every class the components use: no element ships as bare HTML", () => {
    // Every o-* class literal in the components, against every class the stylesheets select.
    const used = new Set([...tsx.matchAll(/["`' ](o-[a-z0-9-]+)/g)].map((match) => match[1]));
    const styled = new Set([...css.matchAll(/\.(o-[a-z0-9-]+)/g)].map((match) => match[1]));
    expect([...used].filter((name) => !styled.has(name)).sort()).toEqual([]);
  });
});

describe("round 3: what the critics found missing", () => {
  it("renders the sticky prompt hidden and out of the tab order until the message scrolls away", () => {
    const html = render("settled");
    expect(html).toContain('class="o-sticky" aria-hidden="true"');
    expect(html).toContain('class="o-sticky-prompt o-t-ui" tabindex="-1"');
    expect(html).not.toContain('class="o-sticky" data-shown');
  });

  it("gives every chat row its hover tools, and the dock its two-letter codes", () => {
    const html = render("settled");
    expect(html.match(/class="o-chat-tools"/g)?.length).toBe(3);
    expect(text(html)).toContain("Agents 5/7 CC CX PI AG HM CU GB");
    expect(agentCode({ id: "someone-new", name: "Some Agent" })).toBe("SO");
  });

  it("counts a project's chats as listed, and says a folded project's failure in words", () => {
    const shown = text(render("settled"));
    // The waiting chat is listed (and counted) under Needs you, not again under its project.
    expect(shown).toContain("Projects 3 auracode 3");
    expect(shown).toContain("onghost.dev 1 failed");
  });

  it("maps project colours onto four muted tones, never violet and never near the orange", () => {
    expect(projectTone("#9db7c9")).toBe("slate");
    expect(projectTone("#8c7dff")).toBe("stone");
    expect(projectTone("#f0a63a")).toBe("sage");
    expect(projectTone("#4caf50")).toBe("clay");
    expect(projectTone("#808080")).toBe("stone");
    expect(projectTone("not a colour")).toBe("stone");
  });

  it("styles a still terminal header with the row grammar and keeps the cost's cents raised", () => {
    const html = render("changes");
    const shown = text(html);
    // X Money's built superscript cents: "$0" and a raised "21".
    expect(html).not.toContain('o-unit">¢');
    expect(shown).toContain("Checkpoint saved before this turn");
  });

  it("shows the session cost as a numeral with superscript cents", () => {
    // The Session tab is not a lab state; its numeral is MoneyNumeral, checked through the parts.
    const shown = text(render("settled"));
    expect(shown).toContain("Done in 48s · 4.8k in · 2.1k out · $0.21");
  });

  it("opens a failed chat on its failed turn, so the error card has a place to show", () => {
    const chat = {
      id: "chat-signin-email",
      title: "Sign-in email template",
      agent: "hermes" as const,
      status: "error",
    };
    const events = failedHistory(chat) ?? [];
    const model = reduceTranscript(events);
    expect(model.turns).toHaveLength(1);
    expect(model.turns[0]?.status).toBe("failed");
    expect(model.notices.at(-1)?.message).toMatch(/^Process exited with code 1\n/);
    // Every event is in the past when the chat opens.
    expect(events.every((entry) => entry.at < 0)).toBe(true);
    expect(failedHistory({ ...chat, status: "idle" })).toBeNull();
  });

  it("scripts a reply as work: one check run with its numbers, then two plain sentences", () => {
    for (const agent of ["claude-code", "codex", "pi"]) {
      const script = scriptFor(agent);
      expect(script.reply.split(". ").length).toBe(2);
      expect(script.reply).not.toMatch(/lab|scripted|no agent/i);
    }
    expect(SCRIPTED_NOTE).toBe("Scripted reply · no agent is connected in the lab");
    const model = reduceTranscript(GOLDEN.events);
    const events = localEvents(
      [
        {
          kind: "send",
          id: "m1",
          at: 60_000,
          text: "Run the tests again",
          agent: "codex",
          agentName: "Codex",
          model: "gpt-5.5-codex",
          effort: "medium",
        },
      ],
      model,
    );
    const after = reduceTranscript([...GOLDEN.events, ...events]);
    const turn = after.turns.at(-1);
    expect(turn?.status).toBe("completed");
    const tools = turn?.items.find((item) => item.kind === "tools");
    expect(tools?.kind === "tools" && tools.calls[0]?.terminal?.exitCode).toBe(0);
  });

  it("labels the globe like SpaceXAI labels a city: a name, a data line and a path", () => {
    const found = DISCOVERY.events.find((entry) => entry.event.type === "agent_found");
    expect(found).toBeDefined();
    expect(
      labelLines({
        id: "claude-code",
        name: "Claude Code",
        status: "found",
        path: "~/.local/bin/claude",
        version: "2.1.310",
        signedIn: true,
        installCommand: null,
        at: 500,
      }).map((line) => line.text),
    ).toEqual(["CLAUDE CODE", "2.1.310", "~/.local/bin/claude"]);
    expect(
      labelLines({
        id: "antigravity",
        name: "Antigravity",
        status: "found",
        path: "/usr/local/bin/agy",
        version: "1.4.7",
        signedIn: false,
        installCommand: null,
        at: 1300,
      }).map((line) => line.tone),
    ).toEqual(["name", "signal", "path"]);
  });

  it("waits 1.2 s before a gap's now-row shows, so it never flashes between two items", () => {
    expect(NOW_ROW_DELAY_MS).toBe(1200);
  });

  it("changes one lab setting in the URL and keeps the rest", () => {
    expect(withParam("#/orbit/onboarding?t=1.6&wall=satin", "theme", "light")).toBe(
      "#/orbit/onboarding?t=1.6&wall=satin&theme=light",
    );
    expect(withParam("#/orbit/onboarding?theme=light", "theme", null)).toBe("#/orbit/onboarding");
  });

  it("tints error and warning lines by their own words, never by a stray word", () => {
    const line = (text: string) => [
      {
        text,
        fg: null,
        bg: null,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
        strikethrough: false,
      },
    ];
    expect(lineTone(line("error[E0308]: mismatched types"))).toBe("error");
    expect(lineTone(line("Error: model hermes-4.5-405b is not available"))).toBe("error");
    expect(lineTone(line(" FAIL  src/orb/pacing.test.ts > waits"))).toBe("error");
    expect(lineTone(line("npm ERR! code ELIFECYCLE"))).toBe("error");
    expect(lineTone(line("warning: unused variable: `frame`"))).toBe("warn");
    expect(lineTone(line("npm WARN deprecated glob@7"))).toBe("warn");
    expect(lineTone(line("error handling tests passed"))).toBeNull();
    expect(lineTone(line("test result: ok. 12 passed; 0 failed"))).toBeNull();
    expect(lineTone([])).toBeNull();
  });
});
