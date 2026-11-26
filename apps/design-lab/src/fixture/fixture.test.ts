import { AGENT_BRAND_IDS } from "@ghost/ui";
import { describe, expect, it } from "vitest";
import { stripAnsi } from "../model/ansi";
import { diffFile } from "../model/diff";
import {
  APPROVAL,
  APPROVAL_MESSAGE,
  APPROVAL_PROMPT,
  APPROVAL_REQUEST_ID,
  APPROVAL_THOUGHT,
} from "./approval";
import {
  GOLDEN,
  GOLDEN_ANSWER,
  GOLDEN_CAUSE,
  GOLDEN_PROMPT,
  GOLDEN_TEST_OUTPUT,
  GOLDEN_THOUGHT,
  GOLDEN_TURN_ID,
} from "./golden";
import { PACING_TEST_BEFORE } from "./golden-files";
import { chunkText, seededRandom, TimelineBuilder } from "./timeline";
import type { ChatRecording, LabEvent, TimedEvent } from "./types";
import {
  AGENTS,
  BASE_WORKSPACE,
  CHAT_ACP,
  CHAT_PACING,
  COMPOSERS,
  composerLabel,
  effortLabel,
  findAgent,
  findChat,
  modelLabel,
  NEW_CHAT_COMPOSER,
  PROJECTS,
} from "./workspace";

type EventOf<T extends LabEvent["type"]> = Extract<LabEvent, { type: T }>;

function eventsOf<T extends LabEvent["type"]>(
  recording: ChatRecording,
  type: T,
): (TimedEvent & { event: EventOf<T> })[] {
  return recording.events.filter(
    (entry): entry is TimedEvent & { event: EventOf<T> } => entry.event.type === type,
  );
}

function joined(recording: ChatRecording, type: "thought_delta" | "message_delta", id: string) {
  return recording.events
    .map((entry) => entry.event)
    .filter((event) =>
      event.type === "thought_delta"
        ? type === "thought_delta" && event.thought_id === id
        : event.type === "message_delta" && type === "message_delta" && event.message_id === id,
    )
    .map((event) =>
      event.type === "thought_delta" || event.type === "message_delta" ? event.text : "",
    )
    .join("");
}

function first<T extends LabEvent["type"]>(
  recording: ChatRecording,
  type: T,
  where: (event: EventOf<T>) => boolean,
) {
  return eventsOf(recording, type).find((entry) => where(entry.event));
}

describe("timeline helpers", () => {
  it("seeds a repeatable sequence", () => {
    const a = seededRandom("x");
    const b = seededRandom("x");
    const c = seededRandom("y");
    const sequence = [a(), a(), a()];
    expect([b(), b(), b()]).toEqual(sequence);
    expect(c()).not.toBe(sequence[0]);
    for (const value of sequence) expect(value).toBeGreaterThanOrEqual(0);
    for (const value of sequence) expect(value).toBeLessThan(1);
  });

  it("chunks text losslessly, at word boundaries", () => {
    for (const text of [GOLDEN_THOUGHT, GOLDEN_ANSWER, "  leading space", "one", ""]) {
      const chunks = chunkText(text, "seed");
      expect(chunks.join("")).toBe(text);
      for (const chunk of chunks.slice(0, -1)) expect(chunk).toMatch(/\s$/);
    }
    expect(chunkText("", "seed")).toEqual([]);
  });

  it("streams between its start and end, in order", () => {
    const events = new TimelineBuilder()
      .stream(2, 4, GOLDEN_CAUSE, "s", (text) => ({ type: "message_delta", message_id: "m", text }))
      .at(1, { type: "turn_started", turn_id: "t" })
      .build();
    expect(events[0]?.event.type).toBe("turn_started");
    const deltas = events.slice(1);
    expect(deltas[0]?.at).toBe(2000);
    expect(deltas.at(-1)?.at).toBe(4000);
    for (let index = 1; index < events.length; index += 1) {
      expect(events[index]?.at).toBeGreaterThanOrEqual(events[index - 1]?.at ?? 0);
    }
  });

  it("puts a single-chunk stream at its start", () => {
    const events = new TimelineBuilder()
      .stream(1, 3, "short", "s", (text) => ({ type: "message_delta", message_id: "m", text }))
      .build();
    expect(events.map((entry) => entry.at)).toEqual([1000]);
  });
});

describe("recordings", () => {
  for (const recording of [GOLDEN, APPROVAL]) {
    it(`${recording.chatId} is in time order and ends at its duration`, () => {
      for (let index = 1; index < recording.events.length; index += 1) {
        expect(recording.events[index]?.at).toBeGreaterThanOrEqual(
          recording.events[index - 1]?.at ?? 0,
        );
      }
      expect(recording.events[0]).toMatchObject({ at: 0, event: { type: "user_message" } });
      expect(recording.duration).toBe(recording.events.at(-1)?.at);
    });

    it(`${recording.chatId} announces every tool call before updating it`, () => {
      const announced = new Set<string>();
      for (const { event } of recording.events) {
        if (event.type === "tool_call") announced.add(event.tool_call_id);
        if (event.type === "tool_update") expect(announced.has(event.tool_call_id)).toBe(true);
      }
    });
  }
});

describe("the golden turn", () => {
  it("is Claude Code fixing the flaky pacing test, in about 48 s", () => {
    expect(GOLDEN.chatId).toBe(CHAT_PACING);
    const [prompt] = eventsOf(GOLDEN, "user_message");
    expect(prompt?.event).toMatchObject({ text: GOLDEN_PROMPT, agent: "claude-code" });
    expect(GOLDEN_PROMPT).toBe(
      "The orb pacing test fails about 1 in 20 runs on CI. Can you find out why and fix it?",
    );
    const start = first(GOLDEN, "turn_started", () => true);
    const end = first(GOLDEN, "turn_ended", () => true);
    expect(start?.event.turn_id).toBe(GOLDEN_TURN_ID);
    expect((end?.at ?? 0) - (start?.at ?? 0)).toBe(48_000);
    expect(GOLDEN.duration).toBeGreaterThanOrEqual(44_000);
    expect(GOLDEN.duration).toBeLessThanOrEqual(50_000);
  });

  it("streams a thought for about 6 s, then a cause, then an answer", () => {
    expect(joined(GOLDEN, "thought_delta", "thought-pacing")).toBe(GOLDEN_THOUGHT);
    expect(joined(GOLDEN, "message_delta", "msg-cause")).toBe(GOLDEN_CAUSE);
    expect(joined(GOLDEN, "message_delta", "msg-answer")).toBe(GOLDEN_ANSWER);
    const thought = eventsOf(GOLDEN, "thought_delta");
    const span = (thought.at(-1)?.at ?? 0) - (thought[0]?.at ?? 0);
    expect(span).toBeGreaterThanOrEqual(5_500);
    expect(span).toBeLessThanOrEqual(6_500);
    expect(first(GOLDEN, "thought_completed", () => true)?.event.duration_ms).toBe(6000);
    expect(GOLDEN_CAUSE).toContain("16.7 ms");
  });

  it("calls tools in the story's order", () => {
    const calls = eventsOf(GOLDEN, "tool_call").map(({ event }) => [event.kind, event.title]);
    expect(calls).toEqual([
      ["read", "Read packages/ui/src/orb/pacing.ts"],
      ["read", "Read packages/ui/src/orb/pacing.test.ts"],
      ["search", 'grep "performance.now" packages/ui/src'],
      ["edit", "Edit packages/ui/src/orb/pacing.ts"],
      ["edit", "Edit packages/ui/src/orb/pacing.test.ts"],
      ["execute", "pnpm --filter @ghost/ui test pacing"],
    ]);
  });

  it("finds three uses of performance.now, at the lines the old test really had", () => {
    const update = first(GOLDEN, "tool_update", (event) => event.tool_call_id === "toolu-grep-now");
    const text = update?.event.content[0]?.type === "text" ? update.event.content[0].text : "";
    const results = text.split("\n");
    expect(results).toHaveLength(3);
    const testLines = PACING_TEST_BEFORE.split("\n");
    for (const result of results.filter((line) => line.includes("pacing.test.ts"))) {
      const [, line, code] = /:(\d+):(.*)$/.exec(result) ?? [];
      expect(testLines[Number(line) - 1]).toBe(code);
    }
  });

  it("edits with diffs whose locations point at the first change", () => {
    for (const id of ["toolu-edit-pacing", "toolu-edit-pacing-test"]) {
      const call = first(GOLDEN, "tool_call", (event) => event.tool_call_id === id);
      const update = first(
        GOLDEN,
        "tool_update",
        (event) => event.tool_call_id === id && event.content.length > 0,
      );
      const content = update?.event.content[0];
      expect(content?.type).toBe("diff");
      if (content?.type !== "diff") continue;
      const diff = diffFile(content.path, content.old_text, content.new_text);
      const firstChange = diff.hunks[0]?.lines.find((line) => line.op !== "context");
      expect(call?.event.locations[0]?.line).toBe(firstChange?.newLine ?? firstChange?.oldLine);
      // The agent's own edit input names text that really is in the file.
      const input = call?.event.raw_input as { old_string: string; new_string: string };
      expect(content.old_text).toContain(input.old_string);
      expect(content.new_text).toContain(input.new_string);
    }
  });

  it("runs the pacing suite: streamed, coloured, passing", () => {
    const updates = eventsOf(GOLDEN, "tool_update").filter(
      ({ event }) => event.tool_call_id === "toolu-run-tests",
    );
    const outputs = updates.map(({ event }) =>
      event.content[0]?.type === "terminal" ? event.content[0].output : "",
    );
    for (let index = 1; index < outputs.length; index += 1) {
      expect(outputs[index]?.startsWith(outputs[index - 1] ?? "")).toBe(true);
    }
    const last = updates.at(-1)?.event;
    expect(last?.status).toBe("completed");
    expect(last?.content[0]).toMatchObject({ type: "terminal", exit_code: 0, truncated: false });
    expect(GOLDEN_TEST_OUTPUT).toContain("\u001b[32m");
    expect(stripAnsi(GOLDEN_TEST_OUTPUT)).toMatch(/Tests {2}19 passed \(19\)/);
  });

  it("moves the plan forward to done", () => {
    const plans = eventsOf(GOLDEN, "plan_updated").map(({ event }) =>
      event.entries.map((entry) => entry.status),
    );
    expect(
      eventsOf(GOLDEN, "plan_updated")[0]?.event.entries.map((entry) => entry.content),
    ).toEqual([
      "Reproduce the flake",
      "Make pacing tolerant to frame jitter",
      "Add a regression test",
    ]);
    expect(plans).toEqual([
      ["completed", "in_progress", "pending"],
      ["completed", "completed", "in_progress"],
      ["completed", "completed", "completed"],
    ]);
  });

  it("reports usage, cost and the stop reason", () => {
    expect(first(GOLDEN, "usage_updated", () => true)?.event.cost).toEqual({
      amount: 0.21,
      currency: "USD",
    });
    const end = first(GOLDEN, "turn_ended", () => true)?.event;
    expect(end?.stop_reason).toBe("end_turn");
    expect(end?.usage).toMatchObject({
      input_tokens: 4812,
      output_tokens: 2147,
      cached_input_tokens: 96_384,
    });
  });
});

describe("the approval turn", () => {
  it("is Codex stopping to ask before it runs the tests", () => {
    expect(APPROVAL.chatId).toBe(CHAT_ACP);
    expect(eventsOf(APPROVAL, "user_message")[0]?.event).toMatchObject({
      text: APPROVAL_PROMPT,
      agent: "codex",
    });
    expect(joined(APPROVAL, "thought_delta", "thought-acp")).toBe(APPROVAL_THOUGHT);
    expect(joined(APPROVAL, "message_delta", "msg-acp")).toBe(APPROVAL_MESSAGE);
    const last = APPROVAL.events.at(-1)?.event;
    expect(last).toMatchObject({
      type: "permission_requested",
      request_id: APPROVAL_REQUEST_ID,
      tool_call_id: "call-cargo-test",
      title: "Run cargo test -p aura-drivers?",
    });
    expect(eventsOf(APPROVAL, "turn_ended")).toEqual([]);
    expect(APPROVAL.duration).toBeLessThanOrEqual(15_000);
  });
});

describe("the workspace", () => {
  it("has the three projects, auracode open with its four chats", () => {
    expect(
      PROJECTS.map((project) => [project.name, project.expanded, project.chats.length]),
    ).toEqual([
      ["auracode", true, 4],
      ["onghost.dev", false, 2],
      ["dotfiles", false, 1],
    ]);
    expect(PROJECTS[0]?.chats.map((chat) => [chat.title, chat.agent, chat.status])).toEqual([
      ["Fix the flaky orb pacing test", "claude-code", "running"],
      ["Wire up the ACP client", "codex", "needs_approval"],
      ["Landing page hero", "antigravity", "done_unseen"],
      ["Tray icon states", "pi", "idle"],
    ]);
  });

  it("uses unique chat ids, real projects and designed brands", () => {
    const chats = PROJECTS.flatMap((project) => project.chats);
    expect(new Set(chats.map((chat) => chat.id)).size).toBe(chats.length);
    for (const project of PROJECTS) {
      for (const chat of project.chats) expect(chat.projectId).toBe(project.id);
    }
    for (const chat of chats) expect(AGENT_BRAND_IDS).toContain(chat.agent);
    for (const agent of AGENTS) expect(AGENT_BRAND_IDS).toContain(agent.id);
  });

  it("docks five installed agents and two to install", () => {
    expect(AGENTS.map((agent) => [agent.name, agent.installed])).toEqual([
      ["Claude Code", true],
      ["Codex", true],
      ["Pi", true],
      ["Antigravity", true],
      ["Hermes", true],
      ["Cursor", false],
      ["Grok Build", false],
    ]);
    expect(AGENTS.filter((agent) => agent.update !== null).map((agent) => agent.name)).toEqual([
      "Codex",
    ]);
    for (const agent of AGENTS) {
      if (!agent.installed) {
        expect(agent).toMatchObject({
          version: null,
          signedIn: null,
          models: [],
          defaultModel: null,
        });
        continue;
      }
      expect(agent.models.map((model) => model.id)).toContain(agent.defaultModel);
      for (const model of agent.models) {
        if (model.defaultEffort === null) expect(model.efforts).toEqual([]);
        else expect(model.efforts.map((effort) => effort.id)).toContain(model.defaultEffort);
      }
    }
  });

  it("points every composer at a real agent, model and effort", () => {
    for (const composer of [...Object.values(COMPOSERS), NEW_CHAT_COMPOSER]) {
      const agent = findAgent(BASE_WORKSPACE, composer.agent);
      expect(agent?.installed).toBe(true);
      const model = agent?.models.find((candidate) => candidate.id === composer.model);
      expect(model).toBeDefined();
      if (composer.effort !== null)
        expect(model?.efforts.map((effort) => effort.id)).toContain(composer.effort);
      expect(agent?.autonomy).toContain(composer.autonomy);
    }
  });

  it("labels models and the composer pill", () => {
    expect(composerLabel(BASE_WORKSPACE)).toBe("Claude Code · Opus 5.5 · High");
    expect(
      composerLabel(BASE_WORKSPACE, {
        ...NEW_CHAT_COMPOSER,
        agent: "hermes",
        model: "hermes-4.5-405b",
        effort: null,
      }),
    ).toBe("Hermes · Hermes 4.5 405B");
    expect(modelLabel(BASE_WORKSPACE, "codex", "gpt-5.5-codex")).toBe("GPT-5.5 Codex");
    expect(modelLabel(BASE_WORKSPACE, "codex", "unknown-model")).toBe("unknown-model");
    expect(effortLabel(BASE_WORKSPACE, "claude-code", "claude-opus-5-5", "xhigh")).toBe(
      "Extra high",
    );
    expect(findChat(BASE_WORKSPACE, CHAT_ACP)?.title).toBe("Wire up the ACP client");
    expect(findChat(BASE_WORKSPACE, null)).toBeNull();
  });
});

describe("writing rules", () => {
  const everything = JSON.stringify([GOLDEN, APPROVAL, BASE_WORKSPACE]);

  it("has no em dashes", () => {
    expect(everything).not.toContain("\u2014");
  });

  it("has no personal data: no home paths, no email addresses", () => {
    expect(everything).not.toMatch(/\/Users\/|\/home\/|C:\\\\Users/);
    expect(everything).not.toMatch(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/);
  });
});
