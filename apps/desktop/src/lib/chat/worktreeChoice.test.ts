import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorktreeChatInput,
  WorktreeChatPlan,
  WorktreeChatPreview,
  WorktreeChatResult,
} from "../../bindings";
import type { WorktreeChatApi } from "../worktreeChat";
import {
  EXPIRY_MARGIN_MS,
  REVIEW_DELAY_MS,
  startingBranch,
  WorktreeChoice,
} from "./worktreeChoice";

const BRANCHES = ["main", "release/1.4"];

function plan(input: WorktreeChatInput, token: string, overrides: Partial<WorktreeChatPlan> = {}) {
  const start = input.start_branch ?? "main";
  const branch = input.branch ?? `ghost/generated-${token}`;
  const value: WorktreeChatPlan = {
    token,
    source: {
      root: "/code/app",
      branch: "main",
      head: "abc",
      files: [],
      blockers: [],
      has_submodules: false,
      is_linked_worktree: false,
    },
    starting_branch: start,
    starting_commit: "abc",
    branch,
    branch_generated: input.branch === null,
    path: `/worktrees/app-${token}`,
    bring_changes: input.bring_changes,
    local_branches: BRANCHES,
    setup: input.run_setup ? { files: [".env"], command: null } : null,
    chip: { mode: "worktree", starting_branch: start, branch, label: "" },
    warnings: [],
    expires_at: Date.now() + 600_000,
    ...overrides,
  };
  return value;
}

/** Rust's review, imitated: typed names that exist are refused; tokens count up. */
function fakeApi(answer?: (input: WorktreeChatInput) => WorktreeChatPreview | Error) {
  const inputs: WorktreeChatInput[] = [];
  const applied: string[] = [];
  let count = 0;
  const api: WorktreeChatApi = {
    preview: vi.fn(async (input: WorktreeChatInput) => {
      inputs.push(input);
      const custom = answer?.(input);
      if (custom instanceof Error) throw custom;
      if (custom) return custom;
      if (input.branch && BRANCHES.includes(input.branch))
        throw new Error("A branch with this name, or one that overlaps it, already exists.");
      return { type: "worktree" as const, plan: plan(input, `t${++count}`) };
    }),
    apply: vi.fn(async (token: string) => {
      applied.push(token);
      return { token } as unknown as WorktreeChatResult;
    }),
    start: vi.fn(async () => {}),
    setupTerminal: vi.fn(),
  };
  return { api, inputs, applied };
}

async function settle() {
  await vi.advanceTimersByTimeAsync(REVIEW_DELAY_MS + 10);
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-27T12:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("the new chat's worktree choice", () => {
  it("defaults to a reviewed worktree from the current branch with nothing to click", async () => {
    const { api, inputs } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    expect(choice.getSnapshot().kind).toBe("checking");
    await settle();
    const view = choice.getSnapshot();
    expect(view.kind).toBe("git");
    expect(view.options.mode).toBe("worktree");
    expect(view.plan?.token).toBe("t1");
    expect(startingBranch(view)).toBe("main");
    expect(inputs).toEqual([
      {
        folder_id: "folder",
        agent_id: "claude",
        start_branch: null,
        branch: null,
        title: null,
        bring_changes: false,
        run_setup: true,
      },
    ]);
    expect(view.setupSaved).toBe(true);
    await expect(choice.resolve()).resolves.toEqual({ type: "worktree", plan: view.plan });
  });

  it("keeps the generated branch name while other options change", async () => {
    const { api, inputs } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    const generated = choice.getSnapshot().plan?.branch;
    choice.propose({ startBranch: "release/1.4" }, "start");
    expect(choice.getSnapshot().pending).toBe(true);
    await settle();
    expect(inputs[1]).toMatchObject({ start_branch: "release/1.4", branch: generated });
    expect(choice.getSnapshot().plan?.branch).toBe(generated);
    expect(startingBranch(choice.getSnapshot())).toBe("release/1.4");
  });

  it("reuses a fresh review for options it has already seen", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    choice.propose({ bringChanges: true }, "bring");
    await settle();
    choice.propose({ bringChanges: false }, "bring");
    await settle();
    choice.propose({ mode: "local" }, "mode");
    choice.propose({ mode: "worktree" }, "mode");
    await settle();
    // The first review, then bring changes; the rest came from the cache.
    expect(api.preview).toHaveBeenCalledTimes(2);
    expect(choice.getSnapshot().plan?.token).toBe("t1");
  });

  it("puts a refused branch name back and shows why beside the field", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    const reviewed = choice.getSnapshot().plan;
    choice.propose({ branch: "main" }, "branch");
    await settle();
    const view = choice.getSnapshot();
    expect(view.error).toEqual({
      field: "branch",
      message: "A branch with this name, or one that overlaps it, already exists.",
    });
    expect(view.options.branch).toBeNull();
    expect(view.plan).toBe(reviewed);
    // Create never applies other options while a refusal shows.
    await expect(choice.resolve()).resolves.toEqual({ type: "stop" });
    choice.propose({ branch: "ghost/my-name" }, "branch");
    await settle();
    expect(choice.getSnapshot().error).toBeNull();
    expect(choice.getSnapshot().plan?.branch).toBe("ghost/my-name");
  });

  it("names a branch at once, without waiting for the review delay", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    choice.propose({ branch: "ghost/typed" }, "branch");
    await vi.advanceTimersByTimeAsync(0);
    expect(api.preview).toHaveBeenCalledTimes(2);
  });

  it("works directly in a plain folder", async () => {
    const { api } = fakeApi(() => ({
      type: "not_git_repository",
      folder: "/code/notes",
      message: "This folder is not a Git repository.",
      chip: { mode: "direct", starting_branch: null, branch: null, label: "Direct in notes" },
    }));
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    expect(choice.getSnapshot()).toMatchObject({ kind: "plain", plan: null });
    await expect(choice.resolve()).resolves.toEqual({ type: "local" });
  });

  it("falls back to Local with the reason when the folder cannot have a worktree", async () => {
    const { api } = fakeApi(() => new Error("The starting branch has no commit yet."));
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    // Create during the first review stops, so the reason is seen before a local chat is made.
    await expect(choice.resolve()).resolves.toEqual({ type: "stop" });
    expect(choice.getSnapshot()).toMatchObject({
      kind: "git",
      options: { mode: "local" },
      unavailable: "The starting branch has no commit yet.",
    });
    await expect(choice.resolve()).resolves.toEqual({ type: "local" });
    // Trying Worktree again keeps Local and says why beside the switch.
    choice.propose({ mode: "worktree" }, "mode");
    await settle();
    expect(choice.getSnapshot()).toMatchObject({
      options: { mode: "local" },
      error: { field: "mode", message: "The starting branch has no commit yet." },
    });
  });

  it("drops answers for a folder that is no longer chosen", async () => {
    const resolvers: ((value: WorktreeChatPreview) => void)[] = [];
    const api = fakeApi().api;
    api.preview = vi.fn(
      (input: WorktreeChatInput) =>
        new Promise<WorktreeChatPreview>((resolve) => {
          resolvers.push((value) => resolve(value));
          void input;
        }),
    );
    const choice = new WorktreeChoice(api);
    choice.target("first", "claude");
    await settle();
    choice.target("second", "claude");
    await settle();
    const answer = (token: string, folder: string): WorktreeChatPreview => ({
      type: "worktree",
      plan: plan(
        {
          folder_id: folder,
          agent_id: "claude",
          start_branch: null,
          branch: null,
          title: null,
          bring_changes: false,
          run_setup: true,
        },
        token,
      ),
    });
    resolvers[1]?.(answer("second", "second"));
    await vi.advanceTimersByTimeAsync(0);
    resolvers[0]?.(answer("first", "first"));
    await vi.advanceTimersByTimeAsync(0);
    expect(choice.getSnapshot().plan?.token).toBe("second");
  });

  it("reviews a plan again before Create when it is about to expire", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    vi.setSystemTime(Date.now() + 600_000 - EXPIRY_MARGIN_MS + 1);
    const decision = choice.resolve();
    await vi.advanceTimersByTimeAsync(0);
    await expect(decision).resolves.toMatchObject({ type: "worktree", plan: { token: "t2" } });
  });

  it("creates from the reviewed token, and reviews again after a refusal", async () => {
    const { api, applied } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    const first = choice.getSnapshot().plan as WorktreeChatPlan;
    api.apply = vi.fn(async (token: string) => {
      applied.push(token);
      throw new Error("This worktree chat preview expired or was already used. Review it again.");
    });
    await expect(choice.create(first)).rejects.toThrow("expired");
    expect(choice.getSnapshot().creating).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    const fresh = choice.getSnapshot().plan as WorktreeChatPlan;
    expect(fresh.token).toBe("t2");
    expect(applied).toEqual(["t1"]);
  });

  it("chooses Local at once, and turns off bringing changes from another branch", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    choice.propose({ bringChanges: true }, "bring");
    await settle();
    choice.propose({ startBranch: "release/1.4" }, "start");
    expect(choice.getSnapshot().options).toMatchObject({
      startBranch: "release/1.4",
      bringChanges: false,
    });
    choice.propose({ mode: "local" }, "mode");
    expect(choice.getSnapshot()).toMatchObject({ pending: false, options: { mode: "local" } });
    await expect(choice.resolve()).resolves.toEqual({ type: "local" });
  });

  it("starts over for another folder and forgets everything when left", async () => {
    const { api } = fakeApi();
    const choice = new WorktreeChoice(api);
    choice.target("folder", "claude");
    await settle();
    choice.propose({ mode: "local" }, "mode");
    choice.target("other", "claude");
    expect(choice.getSnapshot()).toMatchObject({ kind: "checking", options: { mode: "worktree" } });
    choice.target(null, null);
    expect(choice.getSnapshot().kind).toBe("none");
    await settle();
    expect(choice.getSnapshot().kind).toBe("none");
  });
});
