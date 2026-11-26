import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { WorktreeChatPlan } from "../../bindings";
import { DEFAULT_WORKTREE_OPTIONS, type WorktreeChoiceView } from "../../lib/chat/worktreeChoice";
import { recreateLead } from "./RecreateWorktreeDialog";
import { chipText, setupLine, WorktreeChip } from "./WorktreeChip";

const noop = (): void => {};

const PLAN: WorktreeChatPlan = {
  token: "t1",
  source: {
    root: "/code/app",
    branch: "main",
    head: "abc",
    files: [],
    blockers: [],
    has_submodules: false,
    is_linked_worktree: false,
  },
  starting_branch: "main",
  starting_commit: "abc",
  branch: "ghost/7f3a9c2e",
  branch_generated: true,
  path: "/worktrees/app-7f3a",
  bring_changes: false,
  local_branches: ["main", "release/1.4"],
  setup: {
    files: [".env.local", "config/dev.json"],
    command: { program: "pnpm", args: ["install"] },
  },
  chip: {
    mode: "worktree",
    starting_branch: "main",
    branch: "ghost/7f3a9c2e",
    label: "Worktree from main on ghost/7f3a9c2e",
  },
  warnings: ["Uncommitted changes stay in the source folder."],
  expires_at: 1,
};

function view(overrides: Partial<WorktreeChoiceView> = {}): WorktreeChoiceView {
  return {
    kind: "git",
    options: DEFAULT_WORKTREE_OPTIONS,
    plan: PLAN,
    plain: null,
    pending: false,
    error: null,
    unavailable: null,
    setupSaved: true,
    creating: false,
    ...overrides,
  };
}

const chip = (value: WorktreeChoiceView, busyChats = 0) =>
  renderToStaticMarkup(
    <WorktreeChip view={value} onPropose={noop} onClearError={noop} busyChats={busyChats} />,
  );

describe("the worktree chip", () => {
  it("shows nothing for a plain folder or before a folder is chosen", () => {
    expect(chip(view({ kind: "plain", plan: null, plain: "Not a Git repository." }))).toBe("");
    expect(chip(view({ kind: "none", plan: null }))).toBe("");
  });

  it("says it is checking while the first review is on its way", () => {
    const html = chip(view({ kind: "checking", plan: null }));
    expect(html).toContain('data-test-mode="checking"');
    expect(html).toMatch(/role="status"[^>]*>.*Checking Git/);
  });

  it("names the worktree, its start and its new branch in the chip and its accessible name", () => {
    const html = chip(view());
    expect(chipText(view()).name).toBe(
      "Worktree from main on ghost/7f3a9c2e. Where this chat works",
    );
    expect(html).toContain(
      'aria-label="Worktree from main on ghost/7f3a9c2e. Where this chat works"',
    );
    expect(html).toContain('data-test-mode="worktree"');
    expect(html).toContain('data-test-branch="ghost/7f3a9c2e"');
    expect(html).toContain('data-test-start="main"');
    // The popover: both modes, the branches with the current one marked, the generated name.
    expect(html).toMatch(/<button[^>]*class="g-segment"[^>]*aria-pressed="true"[^>]*>.*Worktree/);
    // Start from: the app's dropdown shows the chosen branch and offers every branch.
    expect(html).toContain('<span class="ui-select-value">main (current)</span>');
    expect(html).toContain('<span data-value="main">main (current)</span>');
    expect(html).toContain('<span data-value="release/1.4">release/1.4</span>');
    expect(html).toContain('placeholder="ghost/7f3a9c2e"');
    expect(html).toContain("Named for you. Type a name to use your own.");
    expect(html).toContain("Bring uncommitted changes");
    expect(html).toContain("Run saved setup");
    expect(html).toContain("Copies 2 files, then runs pnpm install.");
    expect(html).toContain("/worktrees/app-7f3a");
    expect(html).toContain("Uncommitted changes stay in the source folder.");
  });

  it("keeps Local to one switch and says the folder is shared", () => {
    const local = view({ options: { ...DEFAULT_WORKTREE_OPTIONS, mode: "local" } });
    const html = chip(local);
    expect(chipText(local).name).toBe("Local on main. Where this chat works");
    expect(html).toContain('data-test-mode="local"');
    expect(html).not.toContain("Start from");
    expect(html).not.toContain("Bring uncommitted changes");
    expect(html).toContain(
      "Works in the project folder, on main. Other chats there share its files.",
    );
  });

  it("shows why a folder cannot have a worktree under the Local chip", () => {
    const html = chip(
      view({
        plan: null,
        options: { ...DEFAULT_WORKTREE_OPTIONS, mode: "local" },
        unavailable: "The starting branch has no commit yet.",
      }),
    );
    expect(html).toContain('data-testid="new-chat-worktree-reason"');
    expect(html).toContain("The starting branch has no commit yet.");
    expect(
      chipText(view({ plan: null, options: { ...DEFAULT_WORKTREE_OPTIONS, mode: "local" } })).name,
    ).toBe("Local. Where this chat works");
  });

  it("marks a refused branch name on the field, the line under it and the chip", () => {
    const html = chip(
      view({ error: { field: "branch", message: "A branch with this name already exists." } }),
    );
    expect(html).toMatch(
      /<input[^>]*aria-invalid="true"[^>]*aria-describedby="[^"]*-branch-error"/,
    );
    expect(html).toMatch(/role="alert"[^>]*>A branch with this name already exists\./);
    expect(html).toContain("new-chat-worktree-flag");
    expect(html).not.toContain("Named for you.");
  });

  it("offers bringing changes only from the current branch, and offers to stop the chats in the way", () => {
    const other = view({ options: { ...DEFAULT_WORKTREE_OPTIONS, startBranch: "release/1.4" } });
    const html = chip(other);
    expect(html).toContain("Only when starting from main.");
    expect(html).toMatch(
      /<input type="checkbox" role="switch" aria-checked="false" class="g-switch" disabled=""/,
    );
    const bringing = view({ options: { ...DEFAULT_WORKTREE_OPTIONS, bringChanges: true } });
    const busy = renderToStaticMarkup(
      <WorktreeChip
        view={bringing}
        onPropose={noop}
        onClearError={noop}
        busyChats={2}
        onStopBusy={noop}
      />,
    );
    // Nothing waits on its own any more: the chip says why, and offers the two ways on.
    expect(busy).not.toContain("once the chats in this folder stop");
    expect(busy).toContain(
      "2 chats are working in this folder, so its files can still change. Stop them to copy them, or create without your changes.",
    );
    expect(busy).toMatch(/<button[^>]*>Stop these chats<\/button>/);
    expect(busy).toMatch(/<button[^>]*>Create without my changes<\/button>/);
    const one = chip(bringing, 1);
    expect(one).toContain("A chat is working in this folder");
    expect(one).toMatch(/<button[^>]*>Create without my changes<\/button>/);
    // Without busy chats the copy simply goes.
    expect(chip(bringing)).toContain(
      "Copies your working files. The project folder keeps them too.",
    );
    expect(chip(bringing)).not.toContain("Create without my changes");
  });

  it("disables every control while Create applies the review", () => {
    const html = chip(view({ creating: true }));
    expect(html).toMatch(/<fieldset class="g-segmented new-chat-worktree-modes"[^>]*disabled=""/);
    expect(html).toMatch(/<button[^>]*role="combobox"[^>]*disabled=""/);
  });

  it("summarises the saved setup, or says it is skipped", () => {
    expect(setupLine(view())).toBe("Copies 2 files, then runs pnpm install.");
    expect(setupLine(view({ setupSaved: false }))).toBeNull();
    expect(
      setupLine(
        view({
          options: { ...DEFAULT_WORKTREE_OPTIONS, runSetup: false },
          plan: { ...PLAN, setup: null },
        }),
      ),
    ).toBe("Skipped for this chat. Run it later from the Git panel.");
    expect(
      setupLine(
        view({ plan: { ...PLAN, setup: { files: [], command: { program: "make", args: [] } } } }),
      ),
    ).toBe("Runs make.");
  });
});

describe("recreating a deleted worktree folder", () => {
  it("leads with what is lost and what is kept, in Rust's words", () => {
    const review = {
      token: "r",
      worktree: {
        id: "w",
        chat_id: "c",
        source: "/code/app",
        path: "/w",
        branch: "ghost/x",
        base_commit: "abc",
        state: "missing",
        detail: null,
        created_at: 1,
      },
      path: "/w",
      branch: "ghost/x",
      commit: "abc",
      warnings: [
        "The folder is recreated at its original app-owned path.",
        "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.",
      ],
      expires_at: 1,
    };
    expect(recreateLead(review)).toEqual({
      lead: "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.",
      rest: ["The folder is recreated at its original app-owned path."],
    });
    expect(recreateLead({ ...review, warnings: [] }).lead).toContain("Everything committed");
  });
});
