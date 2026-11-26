/**
 * `?preview=missing-worktree`: a chat whose app-owned worktree folder was deleted by hand, so its
 * start failed. Only the Git calls the start error needs are imitated: listing the chat's worktrees
 * and Rust's recreate review and apply. `&stale` refuses the first apply as an expired review.
 * `window.missingWorktreeFixture.calls()` lists what was asked, in order. Nothing touches Git.
 */
import type { ManagedWorktree } from "../bindings";
import type { GitWorkspaceApi } from "../lib/gitWorkspace";

export const MISSING_FOLDER = "C:\\auracode\\worktrees\\sidebar-scan";
const COMMIT = "9c41e07d2b8a6f13e5c0d4b7a2f98e16c3d5b0a4";

export function missingWorktreeGit(): GitWorkspaceApi {
  const calls: string[] = [];
  const params = new URLSearchParams(location.search);
  let stale = params.has("stale");
  let sequence = 0;
  let tree: ManagedWorktree = {
    id: "worktree-sidebar",
    chat_id: "preview",
    source: "C:\\workspace\\auracode",
    path: MISSING_FOLDER,
    branch: "ghost/sidebar-scan",
    base_commit: COMMIT,
    state: "missing",
    detail:
      "This worktree was removed outside the app. Its branch and conversation remain available.",
    recovery: null,
    created_at: 1,
  };
  Object.assign(window, { missingWorktreeFixture: { calls: () => [...calls] } });
  const unused = async (): Promise<never> => {
    throw new Error("This preview only imitates recreating the deleted folder.");
  };
  return {
    inspect: unused,
    initializePreview: unused,
    initializeApply: unused,
    list: async () => {
      calls.push("list");
      return [structuredClone(tree)];
    },
    isolationPreview: unused,
    isolationApply: unused,
    actionPreview: unused,
    actionApply: unused,
    recoveryPreview: unused,
    recoveryApply: unused,
    relocationPreview: unused,
    relocationApply: unused,
    pullRequestPreview: unused,
    pullRequestApply: unused,
    commitPreview: unused,
    commitApply: unused,
    pushPreview: unused,
    pushApply: unused,
    recreatePreview: async () => {
      calls.push("review-recreate");
      return {
        token: `recreate-${++sequence}`,
        worktree: structuredClone(tree),
        path: tree.path,
        branch: tree.branch,
        commit: COMMIT,
        warnings: [
          "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.",
          `The folder is recreated at its original app-owned path from the last commit on ${tree.branch} (${COMMIT.slice(0, 12)}).`,
        ],
        expires_at: Date.now() + 600_000,
      };
    },
    recreateApply: async (_chat, _id, token) => {
      calls.push(`apply-recreate:${token}`);
      if (stale) {
        stale = false;
        throw new Error("This recreate preview expired or was already used.");
      }
      tree = { ...tree, state: "ready", detail: null };
      return { worktree: structuredClone(tree), recovery: tree.path, commit: COMMIT, error: null };
    },
  };
}
