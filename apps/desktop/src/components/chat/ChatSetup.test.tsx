import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ManagedWorktree, WorktreeSetupAttempt } from "../../bindings";
import { worktreeRecoveryApi } from "../../lib/worktreeRecovery";
import { worktreeSetupApi } from "../../lib/worktreeSetup";
import { ChatSetup } from "./ChatSetup";
import { needsRecovery, WorktreeRecoveryList, worktreesToReview } from "./WorktreeRecoveryList";

describe("durable setup and retained worktree discovery", () => {
  const attempt: WorktreeSetupAttempt = {
    worktree_id: "tree",
    generation: 1,
    state: "running",
    detail: null,
  };
  it("keeps setup separate from the normal composer and offers concrete retry", () => {
    const running = renderToStaticMarkup(
      <ChatSetup chatId="chat" attempt={attempt} api={worktreeSetupApi} terminals={null} />,
    );
    expect(running).toContain("Setup is running. You can queue messages below.");
    expect(running).not.toContain("dialog");
    const failed = renderToStaticMarkup(
      <ChatSetup
        chatId="chat"
        attempt={{
          ...attempt,
          state: "interrupted",
          detail: "Setup was interrupted by a restart.",
        }}
        api={worktreeSetupApi}
        terminals={null}
      />,
    );
    expect(failed).toContain("Setup was interrupted by a restart.");
    expect(failed).toContain("Retry setup");
  });
  it("shows branch, source, retained path and recovery for a record without any chat", () => {
    const record: ManagedWorktree = {
      id: "tree",
      chat_id: "chat-never-saved",
      source: "/project/source",
      path: "/profile/worktrees/retained",
      branch: "ghost/kept-work",
      base_commit: "abc",
      state: "recovery_required",
      detail: "The chat could not be saved. Rollback retained changed files.",
      recovery: { type: "create" },
      created_at: 1,
    };
    const html = renderToStaticMarkup(
      <WorktreeRecoveryList records={[record]} api={worktreeRecoveryApi} onChanged={vi.fn()} />,
    );
    for (const text of [
      record.source,
      record.path,
      record.branch,
      record.detail,
      "Review recovery",
      "Open folder",
    ])
      expect(html).toContain(text);
    const reconciled = renderToStaticMarkup(
      <WorktreeRecoveryList
        records={[{ ...record, state: "ready", detail: null }]}
        api={worktreeRecoveryApi}
        onChanged={vi.fn()}
      />,
    );
    expect(reconciled).toContain("retained without a saved chat");
    expect(reconciled).toContain("Open folder");
  });

  it("names the folders instead of spelling out their paths, which stay on hover", () => {
    const html = renderToStaticMarkup(
      <WorktreeRecoveryList
        records={[
          {
            id: "tree",
            chat_id: "gone",
            source: "\\\\?\\C:\\Users\\me\\code\\auracode",
            path: "C:\\Users\\me\\AppData\\Local\\si.auracode.desktop\\profiles\\local\\worktrees\\420476c6724f",
            branch: "ghost/ebf0f061",
            base_commit: "abc",
            state: "ready",
            detail: null,
            recovery: null,
            created_at: 1,
          },
        ]}
        api={worktreeRecoveryApi}
        onChanged={vi.fn()}
        heading={false}
      />,
    );
    expect(html).toContain("auracode · 420476c6724f");
    expect(html).toContain('title="Source: C:\\Users\\me\\code\\auracode');
    expect(html).not.toContain("<code>");
    expect(html).not.toContain("<h2>");
  });
});

describe("which retained worktrees New chat offers", () => {
  const tree = (id: string, chat: string, state: ManagedWorktree["state"]): ManagedWorktree => ({
    id,
    chat_id: chat,
    source: "/code/app",
    path: `/worktrees/${id}`,
    branch: `ghost/${id}`,
    base_commit: "abc",
    state,
    detail: null,
    recovery: null,
    created_at: 1,
  });
  const records = [
    tree("live", "chat-live", "ready"),
    tree("gone", "chat-gone", "ready"),
    tree("broken", "chat-live", "recovery_required"),
    tree("retired", "chat-live", "retired"),
  ];

  it("offers none until the chats have loaded, when every live chat's worktree looks kept", () => {
    expect(worktreesToReview(records, null)).toEqual([]);
  });

  it("then offers those no chat holds and those an operation left half way", () => {
    expect(worktreesToReview(records, new Set(["chat-live"])).map((record) => record.id)).toEqual([
      "gone",
      "broken",
    ]);
    expect(needsRecovery(records[2] as ManagedWorktree)).toBe(true);
    expect(needsRecovery(records[1] as ManagedWorktree)).toBe(false);
  });
});
