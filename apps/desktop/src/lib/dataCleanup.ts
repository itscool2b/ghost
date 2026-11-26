import {
  commands,
  type DataCleanupChoice,
  type DataCleanupItem,
  type DataCleanupReview,
  type DataCleanupScope,
  type DataCleanupWorktree,
} from "../bindings";
import { settle } from "./ipc";

/**
 * Settings' "Delete all Ghost data": Rust reviews exactly what would go, then starts the cleanup
 * and quits. Everything the review lists is decided in Rust (ghost-core's uninstall module); this
 * file only words it.
 */
export const dataCleanupApi = {
  review: () => settle(commands.dataCleanupReview()),
  apply: (token: string, choice: DataCleanupChoice) =>
    settle(commands.dataCleanupApply(token, choice)),
};
export type DataCleanupApi = typeof dataCleanupApi;

/** The word typed to confirm. */
export const CONFIRM_WORD = "DELETE";

export function confirmed(text: string): boolean {
  return text.trim() === CONFIRM_WORD;
}

/** "1.2 GB", "340 MB", "12 KB"; under a kilobyte reads "1 KB". */
export function formatBytes(bytes: number): string {
  const units = ["KB", "MB", "GB", "TB"];
  let value = Math.max(bytes, 1) / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 && unit > 0 ? Math.round(value * 10) / 10 : Math.ceil(value);
  return `${rounded} ${units[unit]}`;
}

/** The row label for one thing the cleanup removes. */
export function itemLabel(item: DataCleanupItem, scope: DataCleanupScope): string {
  switch (item.kind) {
    case "data":
      return "Chats and settings";
    case "app":
      return "Window and web data";
    case "entry":
      return scope === "data_folder" ? "In this folder" : "In your data folder";
    case "temporary":
      return "Update downloads";
  }
}

/** Worktrees whose folder holds work that is not saved anywhere else. */
export function unsavedWorktrees(review: DataCleanupReview): DataCleanupWorktree[] {
  return review.worktrees.filter((worktree) => worktree.unsaved);
}

/** The person's choices, as Rust takes them. */
export function cleanupChoice(
  review: DataCleanupReview,
  deleteUnsaved: boolean,
  moveToTrash: boolean,
): DataCleanupChoice {
  return {
    remove_worktrees: deleteUnsaved
      ? unsavedWorktrees(review).map((worktree) => worktree.path)
      : [],
    move_app_to_trash: moveToTrash && review.app_bundle !== null,
  };
}

/** The red pill's words for the choices made. */
export function deleteLabel(moveToTrash: boolean): string {
  return moveToTrash ? "Delete data and move to Trash" : "Delete data and quit";
}

/** The worktree's name in the list: its branch, or its folder's last part. */
export function worktreeName(worktree: DataCleanupWorktree): string {
  if (worktree.branch) return worktree.branch;
  const parts = worktree.path.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? worktree.path;
}
