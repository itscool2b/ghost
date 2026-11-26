import { describe, expect, it } from "vitest";
import type { ConfigOption, PermissionOption } from "../../bindings";
import {
  answerLabel,
  authCommandText,
  contextLabel,
  costLabel,
  errorActions,
  folderStateLabel,
  isStartFailure,
  levelName,
  levelShortName,
  matchingCommands,
  minutesLabel,
  orderedOptions,
  pendingLabel,
  permissionResolutionLabel,
  questionResolutionLabel,
  shortAgo,
  slashQuery,
  statusWord,
  tokensLabel,
} from "./present";

const OPTIONS: PermissionOption[] = [
  { option_id: "allow", kind: "allow_once", label: "Allow once" },
  { option_id: "always", kind: "allow_always", label: "Always allow" },
  { option_id: "reject", kind: "reject_once", label: "Reject" },
];

describe("resolved cards", () => {
  it("say who closed them, or the choice the user made", () => {
    const label = (
      by: Parameters<typeof permissionResolutionLabel>[0]["by"],
      optionId: string | null,
    ) =>
      permissionResolutionLabel(
        {
          decision:
            optionId === null ? { type: "cancelled" } : { type: "selected", option_id: optionId },
          by,
        },
        OPTIONS,
      );
    expect(label("user", "reject")).toBe("You chose: Reject");
    expect(label("user", null)).toBe("Dismissed");
    expect(label("auto", "allow")).toBe("Auto-approved: Allow once");
    expect(label("agent", null)).toBe("Withdrawn by the agent");
    expect(label("turn", null)).toBe("Closed when the turn ended");
    expect(label("system", null)).toBe("Closed when the agent stopped");
    expect(label("other", null)).toBe("Closed");
    expect(label("user", "unknown-option")).toBe("You chose: unknown-option");
  });

  it("show a question's answer, or who closed it", () => {
    const select = { type: "select" as const, message: "Which?", options: ["Red", "Green"] };
    expect(
      questionResolutionLabel({ answer: { type: "select", index: 1 }, by: "user" }, select),
    ).toBe("You answered: Green");
    expect(questionResolutionLabel({ answer: null, by: "user" }, select)).toBe("Dismissed");
    expect(questionResolutionLabel({ answer: null, by: "agent" }, select)).toBe(
      "Withdrawn by the agent",
    );
    expect(questionResolutionLabel({ answer: null, by: "system" }, select)).toBe(
      "Closed when the agent stopped",
    );
    const confirm = { type: "confirm" as const, message: "Sure?" };
    expect(answerLabel({ type: "confirm", yes: true }, confirm)).toBe("Yes");
    expect(answerLabel({ type: "confirm", yes: false }, confirm)).toBe("No");
    expect(answerLabel({ type: "input", text: "blue" }, confirm)).toBe("blue");
    expect(answerLabel({ type: "select", index: 5 }, select)).toBe("Choice 6");
  });
});

describe("error card actions", () => {
  it("offer Retry for start failures only, on the newest card, while nothing runs", () => {
    for (const kind of [
      "agent_unavailable",
      "folder_missing",
      "folder_not_allowed",
      "spawn_failed",
      "start_timeout",
      "protocol_mismatch",
      "agent_exited",
    ] as const) {
      expect(isStartFailure(kind), kind).toBe(true);
      expect(errorActions(kind, { newest: true, live: false, resumable: false })).toEqual({
        retry: true,
        startFresh: false,
        recreate: false,
      });
    }
    for (const kind of [
      "agent_error",
      "protocol_error",
      "storage",
      "internal",
      "auth_required",
      "other",
    ] as const) {
      expect(errorActions(kind, { newest: true, live: false, resumable: true }), kind).toEqual({
        retry: false,
        startFresh: false,
        recreate: false,
      });
    }
    expect(
      errorActions("spawn_failed", { newest: false, live: false, resumable: true }).retry,
    ).toBe(false);
    expect(errorActions("spawn_failed", { newest: true, live: true, resumable: true }).retry).toBe(
      false,
    );
  });

  it("offer Start fresh when an earlier session may be what failed to continue", () => {
    expect(errorActions("start_timeout", { newest: true, live: false, resumable: true })).toEqual({
      retry: true,
      startFresh: true,
      recreate: false,
    });
  });

  it("offer Recreate folder only for the newest missing-folder start of a deleted worktree", () => {
    const context = { newest: true, live: false, resumable: false, recreatable: true };
    expect(errorActions("folder_missing", context)).toEqual({
      retry: true,
      startFresh: false,
      recreate: true,
    });
    // A plain folder that went missing, another start failure, an older card or a live session.
    expect(errorActions("folder_missing", { ...context, recreatable: false }).recreate).toBe(false);
    expect(errorActions("spawn_failed", context).recreate).toBe(false);
    expect(errorActions("folder_missing", { ...context, newest: false }).recreate).toBe(false);
    expect(errorActions("folder_missing", { ...context, live: true }).recreate).toBe(false);
  });
});

describe("words for protocol values", () => {
  it("names every status and folder state", () => {
    expect(statusWord("running")).toBe("Working");
    expect(statusWord("auth_required")).toBe("Sign-in needed");
    expect(statusWord("cancelling")).toBe("Stopping the turn");
    expect(folderStateLabel("ok")).toBeNull();
    expect(folderStateLabel("missing")).toBe("folder missing");
    expect(folderStateLabel("not_allowed")).toBe("folder not allowed");
  });

  it("writes sign-in commands as the user types them, variables first", () => {
    expect(authCommandText({ label: "Sign in", argv: ["codex", "login"], env: [] })).toBe(
      "codex login",
    );
    expect(
      authCommandText({
        label: "Sign in",
        argv: ["claude", "/login", "--name", "my machine"],
        env: [
          ["CODEX_HOME", "/tmp/codex home"],
          ["MODE", "it's"],
        ],
      }),
    ).toBe("CODEX_HOME='/tmp/codex home' MODE='it'\\''s' claude /login --name 'my machine'");
  });

  it("labels tokens with their scope, and context and cost when known", () => {
    expect(
      tokensLabel({
        input_tokens: 1200,
        output_tokens: 300,
        cache_read_tokens: 800,
        cache_write_tokens: null,
        reasoning_tokens: 0,
        scope: "turn",
      }),
    ).toBe("1.2k in · 300 out · 800 cached (this turn)");
    expect(
      tokensLabel({
        input_tokens: 5,
        output_tokens: 6,
        cache_read_tokens: null,
        cache_write_tokens: null,
        reasoning_tokens: 40,
        scope: "session",
      }),
    ).toBe("5 in · 6 out · 40 reasoning (this session)");
    expect(contextLabel(null)).toBeNull();
    expect(contextLabel({ context_used: 12_000, context_size: 200_000, cost: null })).toBe(
      "Context 12k of 200k",
    );
    expect(contextLabel({ context_used: 900, context_size: null, cost: null })).toBe("Context 900");
    expect(
      costLabel({
        context_used: null,
        context_size: null,
        cost: { amount: 0.0201, currency: "USD" },
      }),
    ).toBe("$0.0201");
    expect(costLabel({ context_used: null, context_size: null, cost: null })).toBeNull();
    expect(minutesLabel(1)).toBe("1 minute");
    expect(minutesLabel(3)).toBe("3 minutes");
  });

  it("orders options by category and names pending values", () => {
    const option = (id: string, category: ConfigOption["category"]): ConfigOption => ({
      id,
      name: id,
      description: null,
      category,
      kind: {
        type: "select",
        current: "a",
        choices: [
          { value: "a", name: "Alpha", description: null, group: null },
          { value: "b", name: "Beta", description: null, group: null },
        ],
      },
      pending: null,
    });
    const ordered = orderedOptions([
      option("other", "other"),
      option("mode", "mode"),
      option("model", "model"),
      option("effort", "effort"),
      option("model2", "model"),
    ]);
    expect(ordered.map((entry) => entry.id)).toEqual([
      "model",
      "model2",
      "effort",
      "mode",
      "other",
    ]);
    expect(pendingLabel(option("m", "mode"), { type: "choice", value: "b" })).toBe("Beta");
    expect(pendingLabel(option("m", "mode"), { type: "toggle", value: false })).toBe("off");
  });

  it("name thinking levels plainly, keeping a name the agent chose", () => {
    const level = (value: string, name = value) => levelName({ value, name });
    expect([level("default", "Default"), level("low"), level("xhigh"), level("max")]).toEqual([
      "Default",
      "Low",
      "Extra high",
      "Max",
    ]);
    expect(level("high", "High effort")).toBe("High effort");
    expect(level("turbo")).toBe("Turbo");
    expect(level("XHIGH")).toBe("Extra high");
    expect(levelShortName("Extra high")).toBe("X-high");
    expect(levelShortName("Medium")).toBe("Med");
    expect(levelShortName("High")).toBe("High");
  });
});

describe("the slash menu", () => {
  it("opens on a leading slash and filters by what follows it", () => {
    expect(slashQuery("/")).toBe("");
    expect(slashQuery("/mo")).toBe("mo");
    expect(slashQuery("/model x")).toBeNull();
    expect(slashQuery("hello /x")).toBeNull();
    expect(slashQuery("")).toBeNull();
    const commands = [
      { name: "compact", description: "", input_hint: null },
      { name: "model", description: "", input_hint: null },
      { name: "remodel", description: "", input_hint: null },
    ];
    expect(matchingCommands(commands, "mo").map((c) => c.name)).toEqual(["model", "remodel"]);
    expect(matchingCommands(commands, "").map((c) => c.name)).toEqual([
      "compact",
      "model",
      "remodel",
    ]);
    expect(matchingCommands(commands, "zz")).toEqual([]);
  });
});

describe("a chat row's time", () => {
  const now = Date.UTC(2026, 10, 25, 12, 0, 0);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;

  it("is as short as the row can hold", () => {
    expect(shortAgo(now - 20_000, now)).toBe("now");
    expect(shortAgo(now - 4 * minute, now)).toBe("4m");
    expect(shortAgo(now - 59 * minute, now)).toBe("59m");
    expect(shortAgo(now - 2 * hour, now)).toBe("2h");
    expect(shortAgo(now - 3 * day, now)).toBe("3d");
    expect(shortAgo(now - 15 * day, now)).toBe("2w");
    expect(shortAgo(now - 90 * day, now)).toBe("Aug 27");
  });

  it("says nothing for a time that is missing, and never runs backwards", () => {
    expect(shortAgo(0, now)).toBe("");
    expect(shortAgo(1, now)).toBe("");
    expect(shortAgo(Number.NaN, now)).toBe("");
    expect(shortAgo(now + 5 * minute, now)).toBe("now");
  });
});
