import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { WorktreeActionPreview } from "../../bindings";
import { conflictPrompt, MergeConflicts } from "./MergeConflicts";

function preview(conflicts: string[]): WorktreeActionPreview {
  return {
    token: "merge",
    worktree: {
      id: "tree",
      chat_id: "chat",
      source: "C:/projects/orbit",
      path: "C:/auracode/worktrees/owned-one",
      branch: "ghost/release-readiness",
      base_commit: "5a4d647",
      state: "ready",
      detail: null,
      recovery: null,
      created_at: 1,
    },
    action: { type: "merge", message: "Merge release readiness" },
    target: {
      root: "C:/projects/orbit",
      branch: "main",
      head: "5a4d647",
      files: [],
      blockers: [],
      has_submodules: false,
      is_linked_worktree: false,
    },
    changes: "",
    conflicts,
    warnings: [],
    expires_at: 1,
  };
}

describe("a conflicted merge", () => {
  it("asks in plain words, naming both branches, the folder and every conflicted file", () => {
    const text = conflictPrompt(preview(["src/orbit.rs", "src/app.ts"]));
    expect(text).toBe(
      [
        "Merging ghost/release-readiness into main has conflicts in 2 files:",
        "- src/orbit.rs",
        "- src/app.ts",
        "",
        "Please resolve them on ghost/release-readiness in C:/auracode/worktrees/owned-one: merge main into ghost/release-readiness, fix the conflicts in the files above while keeping the intent of both sides, run the relevant tests, and commit the merge. Do not push and do not change main. I will review the merge in Ghost afterwards.",
      ].join("\n"),
    );
    expect(conflictPrompt(preview(["one.rs"]))).toContain("conflicts in 1 file:");
  });

  it("lists thirty paths and counts the rest", () => {
    const files = Array.from({ length: 34 }, (_, index) => `src/file-${index}.rs`);
    const text = conflictPrompt(preview(files));
    expect(text).toContain("- src/file-29.rs");
    expect(text).not.toContain("- src/file-30.rs");
    expect(text).toContain("- and 4 more");
    expect(text).toContain("conflicts in 34 files:");
  });

  it("shows the exact message and both ways forward when the chat works in the worktree", () => {
    const html = renderToStaticMarkup(
      <MergeConflicts
        chatId="chat"
        preview={preview(["src/orbit.rs"])}
        inWorktree
        agentName="Claude Code"
        onAskAgent={vi.fn(async () => null)}
      />,
    );
    expect(html).toContain("Conflicts to resolve first");
    expect(html).toContain("Message to Claude Code");
    expect(html).toContain("Merging ghost/release-readiness into main has conflicts in 1 file:");
    expect(html).toContain("Ask Claude Code to resolve");
    expect(html).toContain("Nothing has changed yet.");
  });

  it("sends nothing from a chat outside the worktree and says where the work happens", () => {
    const html = renderToStaticMarkup(
      <MergeConflicts
        chatId="chat"
        preview={preview(["src/orbit.rs"])}
        inWorktree={false}
        agentName="Claude Code"
        onAskAgent={vi.fn(async () => null)}
      />,
    );
    expect(html).toContain("src/orbit.rs");
    expect(html).not.toContain("to resolve</button>");
    expect(html).not.toContain("Message to");
    expect(html).toContain("Move this chat into it");
  });

  it("offers no agent action while the composer could not send", () => {
    const html = renderToStaticMarkup(
      <MergeConflicts
        chatId="chat"
        preview={preview(["src/orbit.rs"])}
        inWorktree
        agentName="Claude Code"
      />,
    );
    expect(html).not.toContain("Message to");
    expect(html).not.toContain("Ask Claude Code");
  });
});
