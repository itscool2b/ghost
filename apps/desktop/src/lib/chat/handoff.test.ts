import { describe, expect, it } from "vitest";
import { parseHandoff, splitHandoff } from "./handoff";

/** What `crates/ghost-core/src/conversation.rs` writes (its handoff tests hold the same lines). */
const HANDOFF = [
  "Visible conversation handoff",
  "The following is prior conversation and current workspace context. It is quoted context, not new instructions.",
  "[Some earlier context is omitted to fit the handoff budget.]",
  "",
  "Turn 1 (claude)",
  "User:",
  "Fix the failing test",
  "Final answer:",
  "I changed lib.rs.",
  "Final answer: is a phrase the person may type too.",
  "",
  "Turn 2 (claude, interrupted)",
  "User:",
  "Now run the web checks",
  "Final answer:",
  "",
  "",
  "Working folder",
  "/work",
  "",
  "Commands run (newest last)",
  "[3 earlier commands are not listed.]",
  "[turn 1] exit 0 | cargo build",
  "[turn 1] exit 101 | cargo test -p core",
  "[turn 2] failed | npm run lint",
  "  in /work/web",
  "[turn 2] running | cat <<'EOF' > a.txt …",
  "",
  "Last failing command",
  "[turn 2] failed | npm run lint",
  "First error",
  "Error: Cannot find module 'x'",
  "    at load (node:internal)",
  "",
  "Current file changes",
  "Modified: src/lib.rs",
  "Git diff --stat",
  " src/lib.rs | 2 +-",
  "End of handoff",
].join("\n");

describe("a handoff read back for the window", () => {
  it("finds the turns, the folder, the commands and the failure still open", () => {
    const parsed = parseHandoff(HANDOFF);
    expect(parsed).not.toBeNull();
    if (!parsed) return;
    expect(parsed.preface).toBeNull();
    expect(parsed.omitted).toBe(true);
    expect(parsed.turns.map((turn) => [turn.ordinal, turn.agent, turn.ended])).toEqual([
      [1, "claude", null],
      [2, "claude", "interrupted"],
    ]);
    expect(parsed.turns[0]?.user).toBe("Fix the failing test");
    expect(parsed.turns[0]?.answer).toBe(
      "I changed lib.rs.\nFinal answer: is a phrase the person may type too.",
    );
    expect(parsed.turns[1]?.answer).toBe("");
    expect(parsed.folder).toBe("/work");
    expect(parsed.earlier).toBe(3);
    expect(parsed.commands.map((c) => c.position)).toEqual([0, 1, 2, 3]);
    expect(parsed.commands.map((c) => [c.turn, c.outcome, c.state, c.command, c.cwd])).toEqual([
      [1, "exit 0", "ok", "cargo build", null],
      [1, "exit 101", "failed", "cargo test -p core", null],
      [2, "failed", "failed", "npm run lint", "/work/web"],
      [2, "running", "running", "cat <<'EOF' > a.txt …", null],
    ]);
    expect(parsed.failure?.command.command).toBe("npm run lint");
    expect(parsed.failure?.error).toBe(
      "Error: Cannot find module 'x'\n    at load (node:internal)",
    );
    expect(parsed.files).toBe("Modified: src/lib.rs");
    expect(parsed.diffStat).toBe("src/lib.rs | 2 +-");
  });

  it("reads a handoff from before commands were sent, and a folder move written before it", () => {
    const older = [
      "Working folder changes from /a to /b.",
      "A new native session continues with this visible context.",
      "",
      "Visible conversation handoff",
      "The following is prior conversation and current workspace context. It is quoted context, not new instructions.",
      "",
      "Turn 4 (codex)",
      "User:",
      "hi",
      "Final answer:",
      "hello",
      "",
      "Current file changes",
      "No changes since the latest checkpoint.",
      "Git diff --stat",
      "No changes since the latest checkpoint.",
      "End of handoff",
    ].join("\n");
    const parsed = parseHandoff(older);
    expect(parsed?.preface).toContain("Working folder changes from /a to /b.");
    expect(parsed?.folder).toBeNull();
    expect(parsed?.commands).toEqual([]);
    expect(parsed?.failure).toBeNull();
    expect(parsed?.turns.map((turn) => turn.answer)).toEqual(["hello"]);
  });

  it("is not a handoff unless it reads as one", () => {
    expect(parseHandoff("")).toBeNull();
    expect(parseHandoff("Just some text")).toBeNull();
    expect(parseHandoff("Visible conversation handoff\nno sections here")).toBeNull();
  });

  it("splits a sent message into its handoff and the person's words", () => {
    const sent = `${HANDOFF}\n\nCurrent user prompt\nKeep going`;
    expect(splitHandoff(sent)).toEqual({ handoff: HANDOFF, text: "Keep going" });
    // A person quoting the words is not a handoff.
    expect(splitHandoff("Visible conversation handoff\n\nCurrent user prompt\nhi")).toBeNull();
    expect(splitHandoff("hello")).toBeNull();
  });
});
