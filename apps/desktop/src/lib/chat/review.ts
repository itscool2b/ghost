/**
 * Reviewing what an agent changed: comments on lines of Changes' diffs, sent to the agent as one
 * message, and the request for the agent to review its own changes.
 */
import { useCallback, useSyncExternalStore } from "react";
import type { LineHunk } from "../../bindings";

/** A comment on one line of a changed file, by its line number in the new or the old text. */
export interface DiffComment {
  readonly id: string;
  readonly path: string;
  readonly line: number;
  /** `old` for a removed line, which exists only in the earlier text. */
  readonly side: "new" | "old";
  /** The line as it reads, quoted so the agent finds it even if the numbers have moved. */
  readonly excerpt: string;
  readonly text: string;
}

/** The longest quoted line; a comment needs the place, not a whole minified file. */
const EXCERPT_CHARS = 200;
/** How much of a message a review request quotes to name it. */
const QUOTE_CHARS = 80;
/** The files a review request lists by name; the rest are counted. */
const LISTED_FILES = 20;

export function excerptOf(line: string): string {
  const trimmed = line.trim();
  return trimmed.length > EXCERPT_CHARS ? `${trimmed.slice(0, EXCERPT_CHARS)}…` : trimmed;
}

/** The start of a message, enough to recognize it: its first line, cut at a word. */
export function quoteStart(text: string): string {
  const line = (text.trim().split("\n")[0] ?? "").trim();
  if (line.length <= QUOTE_CHARS) return line;
  const cut = line.slice(0, QUOTE_CHARS);
  const space = cut.lastIndexOf(" ");
  return `${(space > QUOTE_CHARS / 2 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

/** One message with every comment, file by file and line by line, each with the line it is on. */
export function reviewMessage(comments: readonly DiffComment[]): string {
  const sorted = [...comments].sort(
    (a, b) => a.path.localeCompare(b.path) || a.line - b.line || a.side.localeCompare(b.side),
  );
  const parts = sorted.map((comment) => {
    const place =
      comment.side === "old"
        ? `${comment.path}, a removed line (line ${comment.line} before the change)`
        : `${comment.path}:${comment.line}`;
    const quote = comment.excerpt ? `\n> ${comment.excerpt}` : "";
    return `${place}${quote}\n${comment.text.trim()}`;
  });
  const count = comments.length === 1 ? "this comment" : "these comments";
  return `Please address ${count} on your changes:\n\n${parts.join("\n\n")}`;
}

/**
 * Asks the agent to review its own changes and report before changing anything. `since` is the
 * message Changes compares from (empty when it had no words), or `null` for the latest turn;
 * `files` are the files Changes lists.
 */
export function reviewRequest(since: string | null, files: readonly string[]): string {
  const quoted = since === null ? "" : quoteStart(since);
  const scope =
    since === null
      ? "the changes from your last turn"
      : quoted
        ? `the changes made since my message "${quoted}"`
        : "the changes made since an earlier message of mine";
  const listed = files.slice(0, LISTED_FILES).map((file) => `- ${file}`);
  if (files.length > LISTED_FILES) listed.push(`- and ${files.length - LISTED_FILES} more`);
  const where = listed.length ? `\n\nThe files that changed:\n${listed.join("\n")}` : "";
  return `Review ${scope}.${where}\n\nLook for bugs, regressions, missed cases and anything left unfinished, and list the problems by file and line, the most serious first. Don't change any files yet.`;
}

/**
 * Comments once one change of `path` is reverted: those on its lines go, since the change they
 * were about is gone, and those on later lines move with them. A removed line keeps its number,
 * which counts the checkpoint's text.
 */
export function afterHunkRevert(
  comments: readonly DiffComment[],
  path: string,
  hunk: LineHunk,
): DiffComment[] {
  const newEnd = hunk.new_start + hunk.new_lines;
  const oldEnd = hunk.old_start + hunk.old_lines;
  return comments.flatMap((comment) => {
    if (comment.path !== path) return [comment];
    if (comment.side === "old")
      return comment.line > hunk.old_start && comment.line <= oldEnd ? [] : [comment];
    if (comment.line > hunk.new_start && comment.line <= newEnd) return [];
    return comment.line > newEnd
      ? [{ ...comment, line: comment.line + hunk.old_lines - hunk.new_lines }]
      : [comment];
  });
}

/** Unsent comments by chat: they outlast the Changes panel and the chat's view, not Ghost. */
const drafts = new Map<string, readonly DiffComment[]>();
const listeners = new Set<() => void>();
const NONE: readonly DiffComment[] = [];

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export type CommentsChange = (current: readonly DiffComment[]) => readonly DiffComment[];

/** A chat's unsent comments, until they are sent or discarded. */
export function useReviewComments(
  chatId: string,
): readonly [readonly DiffComment[], (change: CommentsChange) => void] {
  const read = () => drafts.get(chatId) ?? NONE;
  const comments = useSyncExternalStore(subscribe, read, read);
  const change = useCallback(
    (update: CommentsChange) => {
      const next = update(drafts.get(chatId) ?? NONE);
      if (next.length) drafts.set(chatId, next);
      else drafts.delete(chatId);
      for (const listener of listeners) listener();
    },
    [chatId],
  );
  return [comments, change] as const;
}
