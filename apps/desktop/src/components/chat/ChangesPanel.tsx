import { FileDiff, RefreshCw, RotateCcw, ScanSearch, Send, X } from "lucide-react";
import { type CSSProperties, useEffect, useId, useRef, useState } from "react";
import type {
  AgentEntry,
  ChangeDiff,
  ChangesSnapshot,
  NativeRewind,
  RevertPreview,
  RevertResult,
  SessionStatus,
} from "../../bindings";
import type { ChangesTransport } from "../../lib/changes";
import type { ChatState } from "../../lib/chat/chatStore";
import {
  afterHunkRevert,
  type CommentsChange,
  type DiffComment,
  reviewMessage,
  reviewRequest,
} from "../../lib/chat/review";
import { hasProcess, type Transcript } from "../../lib/chat/transcript";
import type { ComposerTransport } from "../../lib/composer";
import { errorMessage } from "../../lib/errors";
import { Select } from "../Select";
import { ContextContents, type ContextTab } from "./ContextContents";
import { DiffBlock, type HunkChange } from "./DiffBlock";
import { Modal } from "./Modal";

/** Goes on to a sent message's Edit: a rewind's way to continue the conversation from there. */
function EditThisMessage({ onEdit }: { readonly onEdit: () => void }) {
  return (
    <>
      {" "}
      <button type="button" className="g-link changes-inline-action" onClick={onEdit}>
        Edit this message
      </button>
    </>
  );
}

const fileCount = (count: number) => `${count} ${count === 1 ? "file" : "files"}`;

/** A Rewind from a sent message. `nonce` changes with each request, so asking again re-applies it. */
export interface RewindRequest {
  readonly turnId: string;
  readonly nonce: number;
}

export function ChangesPanel({
  chatId,
  api,
  status,
  transcript,
  onClose,
  onStop,
  state,
  composer,
  agent,
  rewind = null,
  onEditFrom,
  comments = [],
  onCommentsChange,
  onSendReview,
  reviewBlocked = null,
}: {
  readonly chatId: string;
  readonly api: ChangesTransport;
  readonly status: SessionStatus;
  readonly transcript: Transcript;
  readonly onClose: () => void;
  readonly onStop: () => void;
  readonly state: ChatState;
  readonly composer?: ComposerTransport;
  readonly agent: AgentEntry | null;
  /** Compares with the files before this turn and selects every change, for one Review revert. */
  readonly rewind?: RewindRequest | null;
  /** Continues the conversation from a turn: its Edit, as a rewind's last step. */
  readonly onEditFrom?: (turnId: string) => void;
  /** Comments on the diffs' lines, kept by the chat until they are sent or discarded. */
  readonly comments?: readonly DiffComment[];
  readonly onCommentsChange?: (change: CommentsChange) => void;
  /** Sends a message as the person's own; answers why it was not sent, or null. */
  readonly onSendReview?: (text: string) => Promise<string | null>;
  /** Why no message can go now (the comments wait), or null. */
  readonly reviewBlocked?: string | null;
}) {
  const title = useId();
  const closeButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const previous = document.activeElement;
    closeButton.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);
  // A rewind opens on the files before its turn; otherwise Changes compares with the latest turn.
  const [scope, setScope] = useState(rewind?.turnId ?? "");
  const [revision, setRevision] = useState(0);
  const [snapshot, setSnapshot] = useState<ChangesSnapshot | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [path, setPath] = useState<string | null>(null);
  const [diff, setDiff] = useState<ChangeDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [acting, setActing] = useState(false);
  const [preview, setPreview] = useState<RevertPreview | null>(null);
  const [result, setResult] = useState<RevertResult | null>(null);
  // A single change's revert: what the confirmation describes, then the file it was in.
  const [hunk, setHunk] = useState<(HunkChange & { readonly path: string }) | null>(null);
  const [revertedIn, setRevertedIn] = useState<string | null>(null);
  const [width, setWidth] = useState(380);
  const [tab, setTab] = useState<ContextTab>("Changes");
  // A rewind selects every file its own comparison lists (never an earlier scope's), and offers
  // its message's Edit once they are back.
  const selectAllFor = useRef<string | null>(rewind?.turnId ?? null);
  const [editAfter, setEditAfter] = useState(rewind !== null && onEditFrom !== undefined);
  const applied = useRef(rewind?.nonce ?? 0);
  // The agent's own checkpoints (Claude's file rewind), for a turn Ghost saved none for.
  const [nativePreview, setNativePreview] = useState<NativeRewind | null>(null);
  const [nativeError, setNativeError] = useState<string | null>(null);
  const [nativeBusy, setNativeBusy] = useState(false);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [restored, setRestored] = useState<NativeRewind | null>(null);
  const live = hasProcess(status);

  const rewindTurn = rewind?.turnId;
  const rewindNonce = rewind?.nonce;
  const canEdit = onEditFrom !== undefined;
  useEffect(() => {
    // The request the panel opened with is already its state; a later one, even for the same
    // turn, compares again from scratch.
    if (!rewindTurn || rewindNonce === undefined || rewindNonce === applied.current) return;
    applied.current = rewindNonce;
    selectAllFor.current = rewindTurn;
    setTab("Changes");
    setScope(rewindTurn);
    setSelected(new Set());
    setSnapshot(null);
    setPath(null);
    setDiff(null);
    setResult(null);
    setRestored(null);
    setEditAfter(canEdit);
    setRevision((value) => value + 1);
  }, [rewindTurn, rewindNonce, canEdit]);

  useEffect(() => {
    // A status transition or explicit refresh scans the current files once.
    void status;
    void revision;
    let stale = false;
    setBusy(true);
    setError(null);
    void api
      .snapshot(chatId, scope || null)
      .then((value) => {
        if (stale) return;
        setSnapshot(value);
        if (selectAllFor.current !== null && selectAllFor.current === scope) {
          selectAllFor.current = null;
          setSelected(new Set(value.files.map((file) => file.path)));
        } else
          setSelected(
            (current) =>
              new Set(
                [...current].filter((path) => value.files.some((file) => file.path === path)),
              ),
          );
        setPath((current) =>
          value.files.some((file) => file.path === current)
            ? current
            : (value.files[0]?.path ?? null),
        );
      })
      .catch((error) => {
        if (!stale) {
          setSnapshot(null);
          setPath(null);
          setSelected(new Set());
          setError(errorMessage(error));
        }
      })
      .finally(() => {
        if (!stale) setBusy(false);
      });
    return () => {
      stale = true;
    };
  }, [api, chatId, scope, status, revision]);

  const checkpoint = snapshot?.checkpoint;
  useEffect(() => {
    let stale = false;
    setDiff(null);
    if (!checkpoint || !path) return;
    void api
      .diff(chatId, checkpoint.id, path)
      .then((value) => {
        if (!stale) setDiff(value);
      })
      .catch((error) => {
        if (!stale) setError(errorMessage(error));
      });
    return () => {
      stale = true;
    };
  }, [api, chatId, checkpoint, path]);

  const review = async () => {
    if (!checkpoint || !selected.size || acting) return;
    setActing(true);
    setError(null);
    setHunk(null);
    try {
      setPreview(await api.preview(chatId, checkpoint.id, [...selected]));
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setActing(false);
    }
  };
  // Comments on files put back are about changes that no longer exist.
  const dropComments = (paths: readonly string[]) => {
    const gone = new Set(paths.map((path) => path.replaceAll("\\", "/")));
    onCommentsChange?.((current) =>
      current.filter((comment) => !gone.has(comment.path.replaceAll("\\", "/"))),
    );
  };
  const revert = async () => {
    if (!preview || acting) return;
    setActing(true);
    try {
      const outcome = await api.revert(chatId, preview.token);
      setResult(outcome);
      setRevertedIn(hunk?.path ?? null);
      if (hunk) {
        const { path, hunk: range } = hunk;
        if (outcome.applied.includes(path))
          onCommentsChange?.((current) => afterHunkRevert(current, path, range));
      } else {
        dropComments(outcome.applied);
        setSelected(new Set());
      }
      setRevision((value) => value + 1);
      // A rewind goes on to the message's Edit only when every file came back.
      if (!hunk && editAfter && scope && onEditFrom && !outcome.error) onEditFrom(scope);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setPreview(null);
      setHunk(null);
      setActing(false);
    }
  };
  // One change goes back on its own, through the same preview, revert and recovery as files.
  const revertHunk = async (change: HunkChange) => {
    if (!checkpoint || !diff?.after_sha256 || acting) return;
    setActing(true);
    setError(null);
    try {
      const value = await api.previewHunks(chatId, checkpoint.id, {
        path: diff.path,
        after_sha256: diff.after_sha256,
        hunks: [change.hunk],
      });
      setHunk({ ...change, path: diff.path });
      setPreview(value);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setActing(false);
    }
  };
  const closePreview = () => {
    setPreview(null);
    setHunk(null);
  };
  // Without Ghost's checkpoint for the turn, an agent that keeps its own restores what its
  // edit tools changed. Its preview changes nothing; the session must be live and idle.
  const agentName = agent?.name ?? state.chat?.agent_name ?? "The agent";
  const native =
    scope !== "" &&
    snapshot !== null &&
    !snapshot.checkpoint &&
    live &&
    state.lists?.capabilities?.rewind === true;
  useEffect(() => {
    void revision;
    let stale = false;
    setNativePreview(null);
    setNativeError(null);
    if (!native) return;
    setNativeBusy(true);
    void api
      .rewindFiles(chatId, scope, true)
      .then((value) => {
        if (!stale) setNativePreview(value);
      })
      .catch((error) => {
        if (!stale) setNativeError(errorMessage(error));
      })
      .finally(() => {
        if (!stale) setNativeBusy(false);
      });
    return () => {
      stale = true;
    };
  }, [api, chatId, scope, native, revision]);
  const restore = async () => {
    if (!nativePreview?.files.length || acting) return;
    setActing(true);
    setNativeError(null);
    try {
      const outcome = await api.rewindFiles(chatId, scope, false);
      // Claude confirms a restore without naming the files again: they are those its preview
      // listed, from the same checkpoint moments before.
      setRestored(
        outcome.files.length
          ? outcome
          : {
              ...outcome,
              files: nativePreview.files,
              insertions: outcome.insertions ?? nativePreview.insertions,
              deletions: outcome.deletions ?? nativePreview.deletions,
            },
      );
      dropComments(outcome.files.length ? outcome.files : nativePreview.files);
      setRevision((value) => value + 1);
      if (editAfter && onEditFrom) onEditFrom(scope);
    } catch (error) {
      setNativeError(errorMessage(error));
    } finally {
      setConfirmRestore(false);
      setActing(false);
    }
  };
  // Review: the comments go to the agent as one message, or the agent is asked to review.
  const [reviewNote, setReviewNote] = useState<{
    readonly text: string;
    readonly queued: boolean;
  } | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const working = status === "running" || status === "cancelling";
  useEffect(() => {
    // A queued message has gone once the turn before it ends; the note would no longer be true.
    if (!working) setReviewNote((note) => (note?.queued ? null : note));
  }, [working]);
  const sendReview = async (text: string, sent: string, clear: boolean) => {
    if (!onSendReview || sending) return;
    const queued = working;
    setSending(true);
    setReviewError(null);
    try {
      const failure = await onSendReview(text);
      if (failure !== null) setReviewError(failure);
      else {
        setReviewNote({ text: queued ? "Queued for after this turn." : sent, queued });
        if (clear) onCommentsChange?.(() => []);
      }
    } finally {
      setSending(false);
    }
  };
  const commentedFiles = new Set(comments.map((comment) => comment.path)).size;
  const changedFiles = snapshot?.files.length
    ? snapshot.files.map((file) => file.path)
    : native
      ? (nativePreview?.files ?? [])
      : [];
  const turns = transcript.segments
    .flatMap((segment) => {
      if (!segment.turnId) return [];
      const prompt = segment.itemKeys
        .map((key) => transcript.items.get(key))
        .find((item) => item?.kind === "user");
      return [
        {
          id: segment.turnId,
          label: prompt?.kind === "user" ? prompt.text.slice(0, 70) : "Earlier turn",
          text: prompt?.kind === "user" ? prompt.text : "",
        },
      ];
    })
    .reverse();

  return (
    <aside
      className="changes-panel"
      style={{ "--changes-width": `${width}px` } as CSSProperties}
      aria-labelledby={title}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <hr
        className="changes-resizer"
        tabIndex={0}
        aria-label="Resize Changes panel"
        aria-orientation="vertical"
        aria-valuemin={320}
        aria-valuemax={560}
        aria-valuenow={width}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
            event.preventDefault();
            setWidth((value) =>
              Math.max(320, Math.min(560, value + (event.key === "ArrowLeft" ? 20 : -20))),
            );
          }
        }}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            const edge = event.currentTarget.parentElement?.getBoundingClientRect().right;
            if (edge) setWidth(Math.max(320, Math.min(560, edge - event.clientX)));
          }
        }}
        onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)}
      />
      <header className="changes-panel-header">
        <h2 id={title} className="sr-only">
          Chat context
        </h2>
        <div
          className="context-panel-tabs"
          role="tablist"
          aria-label="Chat context"
          onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
            const tabs: ContextTab[] = ["Changes", "Files", "Plan", "Session"];
            const index =
              event.key === "Home"
                ? 0
                : event.key === "End"
                  ? tabs.length - 1
                  : (tabs.indexOf(tab) + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) %
                    tabs.length;
            event.preventDefault();
            setTab(tabs[index] ?? "Changes");
            event.currentTarget.querySelectorAll<HTMLButtonElement>("button")[index]?.focus();
          }}
        >
          {(["Changes", "Files", "Plan", "Session"] as const).map((item) => (
            <button
              key={item}
              type="button"
              role="tab"
              id={`${title}-${item}`}
              aria-controls={`${title}-content`}
              aria-selected={tab === item}
              tabIndex={tab === item ? 0 : -1}
              onClick={() => setTab(item)}
            >
              {item}
              {item === "Changes" && snapshot?.files.length ? (
                <small>{snapshot.files.length}</small>
              ) : null}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          title="Refresh changes"
          aria-label="Refresh changes"
          disabled={busy || acting}
          onClick={() => setRevision((value) => value + 1)}
        >
          <RefreshCw size={16} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Close Changes"
          ref={closeButton}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>
      <div
        className="context-tab-content"
        role="tabpanel"
        id={`${title}-content`}
        aria-labelledby={`${title}-${tab}`}
      >
        {tab !== "Changes" ? (
          <ContextContents tab={tab} state={state} api={composer} agent={agent} />
        ) : (
          <>
            <label className="changes-scope">
              {/* The choice reads on its own ("Before the latest turn"); its name is for screen readers. */}
              <span className="sr-only">Compare current files with</span>
              <Select
                className="g-select"
                value={scope}
                onChange={(event) => {
                  setScope(event.currentTarget.value);
                  setEditAfter(false);
                  setRestored(null);
                  setSelected(new Set());
                  setSnapshot(null);
                  setPath(null);
                  setDiff(null);
                }}
              >
                <option value="">Before the latest turn</option>
                {turns.map((turn) => (
                  <option key={turn.id} value={turn.id}>
                    Before: {turn.label}
                  </option>
                ))}
              </Select>
            </label>
            <div className="changes-panel-scroll">
              {busy ? (
                <p className="changes-empty" role="status">
                  Reading changes…
                </p>
              ) : null}
              {error ? (
                <p className="ui-error" role="alert">
                  {error}
                </p>
              ) : null}
              {snapshot?.unavailable_reason && !native ? (
                <p className="changes-empty">
                  {snapshot.unavailable_reason}
                  {scope
                    ? " Automatic checkpoints in Settings > General save one before each message."
                    : null}
                  {scope && onEditFrom ? (
                    <EditThisMessage onEdit={() => onEditFrom(scope)} />
                  ) : null}
                </p>
              ) : null}
              {native ? (
                <section className="changes-native" aria-label={`Restore with ${agentName}`}>
                  <p className="changes-native-copy">
                    {agentName} kept a copy of each file its edit tools changed. Restoring puts them
                    back as they were before this message; changes made by commands it ran stay as
                    they are.
                  </p>
                  {nativeBusy ? (
                    <p className="changes-empty" role="status">
                      Asking {agentName} which files it changed…
                    </p>
                  ) : null}
                  {nativeError ? (
                    <p className="ui-error" role="alert">
                      {nativeError}
                    </p>
                  ) : null}
                  {nativePreview && nativePreview.files.length === 0 ? (
                    <p className="changes-empty">
                      {agentName} has changed no files since this message.
                      {onEditFrom ? <EditThisMessage onEdit={() => onEditFrom(scope)} /> : null}
                    </p>
                  ) : null}
                  {nativePreview?.files.length ? (
                    <ul className="changes-confirm-files" aria-label="Files to restore">
                      {nativePreview.files.map((file) => (
                        <li key={file}>
                          <code>{file}</code>
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </section>
              ) : null}
              {!busy && checkpoint && snapshot?.files.length === 0 ? (
                <p className="changes-empty">
                  No file changes since this checkpoint.
                  {scope && onEditFrom ? (
                    <EditThisMessage onEdit={() => onEditFrom(scope)} />
                  ) : null}
                </p>
              ) : null}
              {checkpoint?.warnings.length ? (
                <details className="changes-warnings">
                  <summary>Checkpoint exclusions</summary>
                  {checkpoint.warnings.map((warning) => (
                    <p key={warning}>{warning}</p>
                  ))}
                </details>
              ) : null}
              <div className="changes-file-list">
                {snapshot?.files.map((file) => (
                  <div key={file.path} data-selected={file.path === path}>
                    <input
                      type="checkbox"
                      aria-label={`Select ${file.path} for revert`}
                      checked={selected.has(file.path)}
                      onChange={(event) =>
                        setSelected((current) => {
                          const next = new Set(current);
                          if (event.target.checked) next.add(file.path);
                          else next.delete(file.path);
                          return next;
                        })
                      }
                    />
                    <button type="button" onClick={() => setPath(file.path)} title={file.path}>
                      <FileDiff size={14} />
                      <span>{file.path}</span>
                      <small data-kind={file.kind}>
                        {file.kind === "added" ? "A" : file.kind === "deleted" ? "D" : "M"}
                      </small>
                    </button>
                  </div>
                ))}
              </div>
              {diff ? (
                diff.binary ? (
                  <p className="changes-empty">
                    Binary file.{" "}
                    {snapshot?.files.find((file) => file.path === diff.path)?.after_bytes ?? 0}{" "}
                    bytes after this change.
                  </p>
                ) : (
                  <>
                    {diff.truncated ? (
                      <p className="changes-warning">
                        This preview is shortened. Reverting restores the complete checkpoint file.
                      </p>
                    ) : null}
                    <DiffBlock
                      wholeFile={!diff.truncated}
                      context={3}
                      // A change reverts alone in a file that is there before and after, as for
                      // whole files only once no agent works in the folder.
                      onRevertHunk={
                        !live &&
                        checkpoint &&
                        diff.after_sha256 &&
                        snapshot?.files.find((file) => file.path === diff.path)?.kind === "modified"
                          ? (change) => void revertHunk(change)
                          : undefined
                      }
                      review={
                        // Comments need the file's own line numbers: a shortened diff has none.
                        onCommentsChange && !diff.truncated
                          ? {
                              comments: comments.filter((comment) => comment.path === diff.path),
                              onAdd: (comment) => {
                                setReviewNote(null);
                                onCommentsChange((current) => [
                                  ...current,
                                  { ...comment, id: crypto.randomUUID(), path: diff.path },
                                ]);
                              },
                              onRemove: (id) =>
                                onCommentsChange((current) =>
                                  current.filter((comment) => comment.id !== id),
                                ),
                            }
                          : undefined
                      }
                      key={`${checkpoint?.id}:${diff.path}`}
                      path={diff.path}
                      oldText={diff.before ?? ""}
                      newText={diff.after ?? ""}
                      disclosureKey={`changes:${chatId}:${checkpoint?.id}:${diff.path}`}
                    />
                  </>
                )
              ) : path && !error ? (
                <p className="changes-empty" role="status">
                  Opening file diff…
                </p>
              ) : null}
              {result ? (
                <section className="changes-result" aria-label="Revert result">
                  <p role="status">
                    {revertedIn
                      ? result.applied.length
                        ? `Reverted the change in ${revertedIn}.`
                        : `The change in ${revertedIn} was not reverted.`
                      : `Reverted ${result.applied.length} ${result.applied.length === 1 ? "file" : "files"}.`}
                  </p>
                  {result.error ? (
                    <p role="alert" className="ui-error">
                      {result.error}
                    </p>
                  ) : null}
                  {result.recovery_paths.length ? (
                    <details>
                      <summary>Saved originals</summary>
                      {result.recovery_paths.map((path) => (
                        <p className="ui-mono" key={path}>
                          {path}
                        </p>
                      ))}
                    </details>
                  ) : null}
                </section>
              ) : null}
              {restored ? (
                <section className="changes-result" aria-label="Restore result">
                  <p role="status">Restored {fileCount(restored.files.length)}.</p>
                  {restored.skipped_links ? (
                    <p className="changes-warning">
                      {agentName} left {restored.skipped_links} linked{" "}
                      {restored.skipped_links === 1 ? "path" : "paths"} as they are, for safety.
                    </p>
                  ) : null}
                </section>
              ) : null}
            </div>
            {onCommentsChange && onSendReview && (comments.length || changedFiles.length) ? (
              <div className="changes-panel-footer changes-review" data-testid="changes-review">
                <span role="status">
                  {reviewBlocked ??
                    (comments.length
                      ? `${comments.length} ${comments.length === 1 ? "comment" : "comments"}${commentedFiles > 1 ? ` on ${commentedFiles} files` : ""}`
                      : (reviewNote?.text ??
                        (snapshot?.files.length ? "Comment on any line" : null)))}
                </span>
                <div className="ui-row changes-review-actions">
                  {comments.length ? (
                    <>
                      <button
                        type="button"
                        className="g-button g-button-ghost g-button-28"
                        disabled={sending}
                        onClick={() => {
                          setReviewError(null);
                          onCommentsChange(() => []);
                        }}
                      >
                        Discard
                      </button>
                      <button
                        type="button"
                        className="g-button g-button-28"
                        disabled={sending || reviewBlocked !== null}
                        onClick={() =>
                          void sendReview(
                            reviewMessage(comments),
                            comments.length === 1 ? "Comment sent." : "Comments sent.",
                            true,
                          )
                        }
                      >
                        <Send size={13} />
                        {comments.length === 1
                          ? "Send comment"
                          : `Send ${comments.length} comments`}
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      className="g-button g-button-28"
                      disabled={sending || reviewBlocked !== null}
                      onClick={() =>
                        void sendReview(
                          reviewRequest(
                            scope ? (turns.find((turn) => turn.id === scope)?.text ?? "") : null,
                            changedFiles,
                          ),
                          "Review requested.",
                          false,
                        )
                      }
                    >
                      <ScanSearch size={13} />
                      Ask {agentName} to review
                    </button>
                  )}
                </div>
                {reviewError ? (
                  <p className="ui-error changes-review-error" role="alert">
                    {reviewError}
                  </p>
                ) : null}
              </div>
            ) : null}
            {native ? (
              <footer className="changes-panel-footer">
                <span>
                  {nativePreview?.files.length
                    ? `${fileCount(nativePreview.files.length)} to restore`
                    : "Nothing to restore"}
                </span>
                <button
                  type="button"
                  className="g-button g-button-28"
                  disabled={!nativePreview?.files.length || nativeBusy || acting}
                  onClick={() => setConfirmRestore(true)}
                >
                  <RotateCcw size={13} />
                  Review restore
                </button>
              </footer>
            ) : (
              <footer className="changes-panel-footer">
                <span>
                  {selected.size ? `${selected.size} selected` : "Select files to revert"}
                </span>
                {live ? (
                  <button type="button" className="g-button g-button-28" onClick={onStop}>
                    Stop session to revert
                  </button>
                ) : (
                  <button
                    type="button"
                    className="g-button g-button-28"
                    disabled={!selected.size || busy || acting || error !== null}
                    onClick={() => void review()}
                  >
                    <RotateCcw size={13} />
                    Review revert
                  </button>
                )}
              </footer>
            )}
          </>
        )}
      </div>
      {confirmRestore && nativePreview ? (
        <Modal
          title={`Restore files with ${agentName}?`}
          onClose={() => {
            if (!acting) setConfirmRestore(false);
          }}
        >
          <p className="changes-confirm-copy">
            Put these files back as they were before the selected message. Changes made by commands{" "}
            {agentName} ran stay as they are.
          </p>
          <ul className="changes-confirm-files">
            {nativePreview.files.map((file) => (
              <li key={file}>
                <code>{file}</code>
              </li>
            ))}
          </ul>
          {nativePreview.insertions !== null || nativePreview.deletions !== null ? (
            <p className="changes-confirm-copy">
              <span className="chat-diff-totals">
                <span data-op="add">+{nativePreview.insertions ?? 0}</span>{" "}
                <span data-op="remove">−{nativePreview.deletions ?? 0}</span>
              </span>{" "}
              lines change back.
            </p>
          ) : null}
          {onEditFrom ? (
            <label className="changes-edit-after">
              <input
                type="checkbox"
                checked={editAfter}
                disabled={acting}
                onChange={(event) => setEditAfter(event.currentTarget.checked)}
              />
              Then edit this message to continue from there
            </label>
          ) : null}
          <div className="g-dialog-actions">
            <button
              type="button"
              className="g-button"
              disabled={acting}
              onClick={() => setConfirmRestore(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-danger"
              disabled={acting}
              onClick={() => void restore()}
            >
              {acting ? "Restoring…" : `Restore ${fileCount(nativePreview.files.length)}`}
            </button>
          </div>
        </Modal>
      ) : null}
      {preview && hunk ? (
        <Modal
          title="Revert this change?"
          onClose={() => {
            if (!acting) closePreview();
          }}
        >
          <p className="changes-confirm-copy">
            Put back the lines of <code>{hunk.path}</code> at line {hunk.line} as they were before
            the selected turn. The rest of the file stays as it is.
          </p>
          <p className="changes-confirm-copy">
            <span className="chat-diff-totals">
              <span data-op="add">+{hunk.added}</span> <span data-op="remove">−{hunk.removed}</span>
            </span>{" "}
            lines change back. Ghost keeps the file as it is now in its recovery folder.
          </p>
          <div className="g-dialog-actions">
            <button type="button" className="g-button" disabled={acting} onClick={closePreview}>
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-danger"
              disabled={acting}
              onClick={() => void revert()}
            >
              {acting ? "Reverting…" : "Revert change"}
            </button>
          </div>
        </Modal>
      ) : preview ? (
        <Modal
          title="Revert selected files?"
          onClose={() => {
            if (!acting) closePreview();
          }}
        >
          <p className="changes-confirm-copy">
            Restore these files to their state before the selected turn.
          </p>
          <ul className="changes-confirm-files">
            {preview.files.map((file) => (
              <li key={file.path}>
                <code>{file.path}</code>
                <span>{file.kind}</span>
              </li>
            ))}
          </ul>
          {preview.warnings.map((warning) => (
            <p className="changes-warning" key={warning}>
              {warning}
            </p>
          ))}
          {scope && onEditFrom ? (
            <label className="changes-edit-after">
              <input
                type="checkbox"
                checked={editAfter}
                disabled={acting}
                onChange={(event) => setEditAfter(event.currentTarget.checked)}
              />
              Then edit this message to continue from there
            </label>
          ) : null}
          <div className="g-dialog-actions">
            <button type="button" className="g-button" disabled={acting} onClick={closePreview}>
              Cancel
            </button>
            <button
              type="button"
              className="g-button g-button-danger"
              disabled={acting}
              onClick={() => void revert()}
            >
              {acting
                ? "Reverting…"
                : `Revert ${preview.files.length} ${preview.files.length === 1 ? "file" : "files"}`}
            </button>
          </div>
        </Modal>
      ) : null}
    </aside>
  );
}
