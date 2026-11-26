import { CopyButton } from "@ghost/ui/graphite";
import { MessageSquarePlus, RotateCcw, UnfoldVertical } from "lucide-react";
import { Fragment, type KeyboardEvent, useEffect, useId, useMemo, useRef, useState } from "react";
import type { LineHunk } from "../../bindings";
import { type DiffComment, excerptOf } from "../../lib/chat/review";
import {
  changeParts,
  type DiffPart,
  diffLines,
  type NumberedLine,
  numberLines,
} from "../../lib/diff";
import { useDisclosure } from "./DisclosureMemory";
import { OpenFileButton } from "./FileLink";

const COLLAPSED_ROWS = 14;

/** Comments on this file's lines, for Changes' review (lib/chat/review.ts). */
export interface DiffReview {
  readonly comments: readonly DiffComment[];
  readonly onAdd: (comment: Omit<DiffComment, "id" | "path">) => void;
  readonly onRemove: (id: string) => void;
}

/** One change Changes can revert on its own, and where it starts in the current text. */
export interface HunkChange {
  readonly hunk: LineHunk;
  readonly added: number;
  readonly removed: number;
  readonly line: number;
}

/** What the table lists: a line, a fold of unchanged lines, or the bar above a change. */
type Item =
  | { readonly kind: "row"; readonly row: NumberedLine }
  | {
      readonly kind: "fold";
      readonly from: number;
      readonly to: number;
      /** The change right after the fold, whose Revert the fold's bar carries. */
      readonly revert?: HunkChange;
    }
  | { readonly kind: "head"; readonly revert: HunkChange };

/** The line a comment is on, as its buttons name it. */
function place(comment: Pick<DiffComment, "side" | "line">): string {
  return comment.side === "old" ? `removed line ${comment.line}` : `line ${comment.line}`;
}

function commentCount(count: number): string {
  return `${count} ${count === 1 ? "comment" : "comments"}`;
}

function hunkChange(part: Extract<DiffPart, { kind: "change" }>): HunkChange {
  const { hunk } = part;
  return {
    hunk,
    added: part.added,
    removed: part.removed,
    // Removed lines alone sit after the line before them.
    line: Math.max(1, hunk.new_lines > 0 ? hunk.new_start + 1 : hunk.new_start),
  };
}

/** A comment being written under a line: Enter adds it, Shift+Enter starts a new line. */
function CommentEditor({
  label,
  onAdd,
  onCancel,
}: {
  readonly label: string;
  readonly onAdd: (text: string) => void;
  readonly onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    // The editor opens because its line was chosen.
    field.current?.focus();
  }, []);
  const add = () => {
    if (text.trim()) onAdd(text.trim());
  };
  return (
    <div className="chat-diff-comment-editor">
      <textarea
        ref={field}
        className="g-textarea"
        aria-label={label}
        placeholder="What should change here?"
        maxLength={4000}
        value={text}
        onChange={(event) => setText(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Escape") {
            // Esc only closes the editor; it never reaches the panel, which it would close.
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          } else if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            add();
          }
        }}
      />
      <div className="chat-diff-comment-actions">
        <button type="button" className="g-button g-button-ghost g-button-24" onClick={onCancel}>
          Cancel
        </button>
        <button
          type="button"
          className="g-button g-button-24"
          disabled={!text.trim()}
          onClick={add}
        >
          Add comment
        </button>
      </div>
    </div>
  );
}

function RevertHunk({
  change,
  onRevert,
}: {
  readonly change: HunkChange;
  readonly onRevert: (change: HunkChange) => void;
}) {
  return (
    <button
      type="button"
      className="g-button g-button-ghost g-button-24 chat-diff-revert"
      aria-label={`Revert the change at line ${change.line}`}
      onClick={() => onRevert(change)}
    >
      <RotateCcw size={12} aria-hidden="true" />
      Revert
    </button>
  );
}

/** Line numbers refer to the supplied text, which may be a fragment of the named file. */
export function DiffBlock({
  path,
  oldText,
  newText,
  disclosureKey,
  wholeFile = false,
  copyPath = true,
  review,
  context,
  onRevertHunk,
}: {
  readonly path: string;
  readonly oldText: string | null;
  readonly newText: string;
  readonly disclosureKey: string;
  readonly wholeFile?: boolean;
  /** Off when `path` names a document rather than a file, such as project memory. */
  readonly copyPath?: boolean;
  /** Comments on lines, where line numbers are the file's own (Changes). */
  readonly review?: DiffReview;
  /** Shows each change with this many unchanged lines around it, folding the rest (Changes). */
  readonly context?: number;
  /** A Revert above each change, where `context` shows a whole file's changes. */
  readonly onRevertHunk?: (change: HunkChange) => void;
}) {
  const [expanded, setExpanded] = useDisclosure(disclosureKey, "diff", false);
  const bodyId = useId();
  const noteId = useId();
  const scroller = useRef<HTMLElement>(null);
  const diff = useMemo(() => diffLines(oldText, newText), [oldText, newText]);
  const rows = useMemo(() => numberLines(diff.lines), [diff]);
  // The new text's line where the first change starts: where Open puts the cursor.
  const firstChange = useMemo(() => {
    let line = 0;
    for (const row of rows) {
      if (row.op !== "same") return line + 1;
      line += 1;
    }
    return null;
  }, [rows]);
  // A shortened replacement view lists lines as removed and added that were not: no comments,
  // no folds and no reverts of single changes.
  const reviewing = diff.truncated ? undefined : review;
  const parts = useMemo(
    () => (context === undefined || diff.truncated ? null : changeParts(rows, context)),
    [rows, context, diff.truncated],
  );
  // Folds opened, by their first line, which moves with the text if a change above is reverted.
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set());
  // The line whose comment is being written, and the line whose comment button Tab reaches.
  const [writing, setWriting] = useState<string | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  // Each comment's row, or -1 when the diff no longer has its line.
  const commented = useMemo(() => {
    const at = new Map<string, number>();
    for (const comment of reviewing?.comments ?? []) {
      at.set(
        comment.id,
        rows.findIndex((row) =>
          comment.side === "old"
            ? row.op === "remove" && row.oldLine === comment.line
            : row.op !== "remove" && row.newLine === comment.line,
        ),
      );
    }
    return at;
  }, [reviewing, rows]);
  const reverting = onRevertHunk !== undefined && parts !== null;
  const items = useMemo(() => {
    const list: Item[] = [];
    if (!parts) {
      for (const row of expanded ? rows : rows.slice(0, COLLAPSED_ROWS))
        list.push({ kind: "row", row });
      return list;
    }
    parts.forEach((part, index) => {
      if (part.kind === "fold") {
        // A fold with a comment on one of its lines stays open, so the comment shows.
        const open =
          opened.has(rows[part.from]?.key ?? "") ||
          [...commented.values()].some((row) => row >= part.from && row < part.to);
        if (open) {
          for (const row of rows.slice(part.from, part.to)) list.push({ kind: "row", row });
          return;
        }
        const next = parts[index + 1];
        list.push({
          kind: "fold",
          from: part.from,
          to: part.to,
          revert: reverting && next?.kind === "change" ? hunkChange(next) : undefined,
        });
        return;
      }
      const previous = list[list.length - 1];
      if (reverting && !(previous?.kind === "fold" && previous.revert))
        list.push({ kind: "head", revert: hunkChange(part) });
      for (const row of rows.slice(part.from, part.to)) list.push({ kind: "row", row });
    });
    return list;
  }, [parts, rows, expanded, opened, commented, reverting]);
  const shown = useMemo(
    () => items.flatMap((item) => (item.kind === "row" ? [item.row] : [])),
    [items],
  );
  const hidden = parts ? 0 : rows.length - COLLAPSED_ROWS;
  // Each comment sits under its line. One on a folded line is counted on Show more lines; one
  // whose line has gone (the file changed since) is listed after the lines.
  const placement = useMemo(() => {
    const under = new Map<string, DiffComment[]>();
    const lost: DiffComment[] = [];
    let folded = 0;
    const listed = new Set(shown.map((row) => row.key));
    for (const comment of reviewing?.comments ?? []) {
      const row = rows[commented.get(comment.id) ?? -1];
      if (!row) lost.push(comment);
      else if (!listed.has(row.key)) folded += 1;
      else under.set(row.key, [...(under.get(row.key) ?? []), comment]);
    }
    return { under, lost, folded };
  }, [reviewing, rows, commented, shown]);
  // One comment button takes Tab, at first the first changed line's; arrows move between them.
  const tabStop =
    shown.find((row) => row.key === current)?.key ??
    (shown.find((row) => row.op !== "same") ?? shown[0])?.key;
  const commentButtons = () =>
    Array.from(
      scroller.current?.querySelectorAll<HTMLButtonElement>("button[data-comment-line]") ?? [],
    );
  const focusLine = (key: string) =>
    commentButtons()
      .find((button) => button.dataset.commentLine === key)
      ?.focus();
  const moveFocus = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    const buttons = commentButtons();
    const index = buttons.indexOf(event.currentTarget);
    const target =
      buttons[
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? buttons.length - 1
            : index + (event.key === "ArrowDown" ? 1 : -1)
      ];
    if (!target) return;
    event.preventDefault();
    target.focus();
  };
  const directoryEnd = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1;
  const omittedOld = diff.truncated
    ? diff.removed - rows.filter((line) => line.op === "remove").length
    : 0;
  const omittedNew = diff.truncated
    ? diff.added - rows.filter((line) => line.op === "add").length
    : 0;
  const changed = diff.added + diff.removed;
  const explain = !wholeFile || oldText === null || diff.truncated;
  const filledSquares = Math.min(5, changed);
  const addedSquares = changed === 0 ? 0 : Math.round((diff.added / changed) * filledSquares);
  const columns = reviewing ? 5 : 4;

  return (
    <figure
      className="chat-diff"
      data-testid="diff-block"
      data-test-truncated={String(diff.truncated)}
      data-folded={hidden > 0 && !expanded}
    >
      <figcaption className="chat-diff-header">
        <svg
          width="14"
          height="14"
          viewBox="0 0 14 14"
          fill="none"
          stroke="currentColor"
          aria-hidden="true"
        >
          <path d="M3 1.5h5l3 3v8H3zM8 1.5v3h3" />
        </svg>
        <span className="chat-diff-path" title={path}>
          <span className="chat-diff-directory">{path.slice(0, directoryEnd)}</span>
          <strong>{path.slice(directoryEnd) || "Changed text"}</strong>
        </span>
        <span
          className="chat-diff-totals"
          title={`${diff.added} added, ${diff.removed} removed${diff.truncated ? ", replacement totals" : ""}`}
        >
          <span data-op="add">+{diff.added}</span> <span data-op="remove">−{diff.removed}</span>
        </span>
        <span className="chat-diff-stat" aria-hidden="true">
          {[0, 1, 2, 3, 4].map((square) => (
            <span
              key={square}
              data-op={square < addedSquares ? "add" : square < filledSquares ? "remove" : "same"}
            />
          ))}
        </span>
        {copyPath ? (
          <>
            {/* At the first change, where the line numbers are the file's own. */}
            <OpenFileButton reference={path} line={wholeFile ? firstChange : null} />
            <CopyButton text={path} label="Copy path" caption="Copy path" />
          </>
        ) : null}
      </figcaption>
      {explain ? (
        <p className="chat-diff-note" id={noteId}>
          {!wholeFile ? "Line numbers are relative to the supplied text." : null}
          {oldText === null ? " No previous text was provided." : null}
          {diff.truncated
            ? " Large diff: counts describe a replacement, not individual edits."
            : null}
        </p>
      ) : null}
      <section
        ref={scroller}
        className="chat-diff-scroll"
        id={bodyId}
        data-pinned={reviewing || parts ? "" : undefined}
        aria-label={`Line changes for ${path || "supplied text"}`}
        aria-describedby={explain ? noteId : undefined}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: Keyboard users need to scroll long code lines horizontally.
        tabIndex={0}
      >
        {rows.length === 0 ? (
          <p className="chat-diff-empty">No text to compare.</p>
        ) : (
          <table
            className="chat-diff-table"
            aria-label={wholeFile ? "File line changes" : "Relative line changes"}
          >
            <thead>
              <tr>
                {reviewing ? <th scope="col">Comment</th> : null}
                <th scope="col">Old</th>
                <th scope="col">New</th>
                <th scope="col">Change</th>
                <th scope="col">Text</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => {
                if (item.kind === "fold") {
                  const count = item.to - item.from;
                  return (
                    <tr key={`fold:${item.from}`} className="chat-diff-band">
                      <td colSpan={columns} className="chat-diff-band-cell">
                        <div className="chat-diff-band-bar">
                          <button
                            type="button"
                            className="chat-diff-unfold"
                            onClick={() =>
                              setOpened((value) => new Set(value).add(rows[item.from]?.key ?? ""))
                            }
                          >
                            <UnfoldVertical size={13} aria-hidden="true" />
                            Show {count} unchanged {count === 1 ? "line" : "lines"}
                          </button>
                          {item.revert && onRevertHunk ? (
                            <RevertHunk change={item.revert} onRevert={onRevertHunk} />
                          ) : null}
                        </div>
                      </td>
                    </tr>
                  );
                }
                if (item.kind === "head") {
                  return (
                    <tr
                      key={`head:${item.revert.hunk.old_start}:${item.revert.hunk.new_start}`}
                      className="chat-diff-band"
                    >
                      <td colSpan={columns} className="chat-diff-band-cell">
                        <div className="chat-diff-band-bar">
                          {onRevertHunk ? (
                            <RevertHunk change={item.revert} onRevert={onRevertHunk} />
                          ) : null}
                        </div>
                      </td>
                    </tr>
                  );
                }
                const line = item.row;
                // A removed line exists only in the earlier text: its number is the old one.
                const side = line.op === "remove" ? "old" : "new";
                const number = (side === "old" ? line.oldLine : line.newLine) ?? 0;
                const where = place({ side, line: number });
                return (
                  <Fragment key={line.key}>
                    <tr className="chat-diff-line" data-op={line.op} data-test-op={line.op}>
                      {reviewing ? (
                        <td className="chat-diff-comment-cell">
                          <button
                            type="button"
                            className="chat-diff-comment-add"
                            data-comment-line={line.key}
                            tabIndex={line.key === tabStop ? 0 : -1}
                            aria-label={`Comment on ${where}`}
                            title={`Comment on ${where}`}
                            onFocus={() => setCurrent(line.key)}
                            onKeyDown={moveFocus}
                            onClick={() => setWriting(line.key)}
                          >
                            <MessageSquarePlus size={13} aria-hidden="true" />
                          </button>
                        </td>
                      ) : null}
                      <td className="chat-diff-number" data-side="old">
                        {line.oldLine}
                      </td>
                      <td className="chat-diff-number" data-side="new">
                        {line.newLine}
                      </td>
                      <td
                        className="chat-diff-marker"
                        aria-label={
                          line.op === "add"
                            ? "Added"
                            : line.op === "remove"
                              ? "Removed"
                              : "Unchanged"
                        }
                      >
                        {line.op === "add" ? "+" : line.op === "remove" ? "−" : " "}
                      </td>
                      <td className="chat-diff-code">
                        <code>{line.text || " "}</code>
                      </td>
                    </tr>
                    {reviewing
                      ? (placement.under.get(line.key) ?? []).map((comment) => (
                          <tr key={comment.id} className="chat-diff-review-row">
                            <td colSpan={columns}>
                              <div
                                className="chat-diff-comment chat-diff-pinned"
                                data-testid="diff-comment"
                              >
                                <p className="chat-diff-comment-text">{comment.text}</p>
                                <button
                                  type="button"
                                  className="g-button g-button-ghost g-button-24"
                                  aria-label={`Delete the comment on ${where}`}
                                  onClick={() => {
                                    reviewing.onRemove(comment.id);
                                    focusLine(line.key);
                                  }}
                                >
                                  Delete
                                </button>
                              </div>
                            </td>
                          </tr>
                        ))
                      : null}
                    {reviewing && writing === line.key ? (
                      <tr className="chat-diff-review-row">
                        <td colSpan={columns}>
                          <div className="chat-diff-pinned">
                            <CommentEditor
                              label={`Comment on ${where}`}
                              onCancel={() => {
                                setWriting(null);
                                focusLine(line.key);
                              }}
                              onAdd={(text) => {
                                reviewing.onAdd({
                                  line: number,
                                  side,
                                  excerpt: excerptOf(line.text),
                                  text,
                                });
                                setWriting(null);
                                focusLine(line.key);
                              }}
                            />
                          </div>
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        )}
      </section>
      {hidden > 0 ? (
        <button
          type="button"
          className="chat-diff-expand"
          aria-expanded={expanded}
          aria-controls={bodyId}
          onClick={() => setExpanded(!expanded)}
        >
          {expanded
            ? "Show fewer lines"
            : `Show ${hidden} more lines${placement.folded ? `, ${commentCount(placement.folded)}` : ""}`}
        </button>
      ) : null}
      {reviewing && placement.lost.length ? (
        <ul className="chat-diff-lost" aria-label="Comments on lines this diff no longer has">
          {placement.lost.map((comment) => (
            <li key={comment.id} className="chat-diff-comment" data-testid="diff-comment">
              <p className="chat-diff-comment-text">
                <span className="chat-diff-comment-place">On {place(comment)}:</span> {comment.text}
              </p>
              <button
                type="button"
                className="g-button g-button-ghost g-button-24"
                aria-label={`Delete the comment on ${place(comment)}`}
                onClick={() => {
                  reviewing.onRemove(comment.id);
                  scroller.current?.focus();
                }}
              >
                Delete
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {diff.truncated ? (
        <p className="chat-diff-truncated">
          The replacement preview omits {omittedOld} old and {omittedNew} new lines. Expanding shows
          only the retained preview.
        </p>
      ) : null}
    </figure>
  );
}
