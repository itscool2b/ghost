import "./delete-data.css";
import { useEffect, useId, useRef, useState } from "react";
import type { DataCleanupReview } from "../../bindings";
import {
  CONFIRM_WORD,
  cleanupChoice,
  confirmed,
  type DataCleanupApi,
  dataCleanupApi,
  deleteLabel,
  formatBytes,
  itemLabel,
  unsavedWorktrees,
  worktreeName,
} from "../../lib/dataCleanup";
import { errorMessage } from "../../lib/errors";
import { Modal } from "../chat/Modal";

/** Where the dialog stands. */
export type DeleteDataPhase = "loading" | "ready" | "applying" | "quitting";

/**
 * "Delete all Ghost data" (Settings, About, This device): Rust's review of exactly what goes,
 * a plain warning for worktrees with unsaved work (kept unless the person ticks to delete them),
 * what stays, and DELETE typed before the red pill. Applying starts the cleanup and quits; nothing
 * is deleted while Ghost runs.
 */
export function DeleteDataDialog({
  api = dataCleanupApi,
  onClose,
}: {
  readonly api?: DataCleanupApi;
  readonly onClose: () => void;
}) {
  const [review, setReview] = useState<DataCleanupReview | null>(null);
  const [phase, setPhase] = useState<DeleteDataPhase>("loading");
  const [error, setError] = useState<string | null>(null);
  const [deleteUnsaved, setDeleteUnsaved] = useState(false);
  const [moveToTrash, setMoveToTrash] = useState(false);
  const [typed, setTyped] = useState("");
  const [generation, setGeneration] = useState(0);
  const live = useRef(true);
  useEffect(() => {
    void generation;
    live.current = true;
    setPhase("loading");
    setReview(null);
    setTyped("");
    api.review().then(
      (next) => {
        if (!live.current) return;
        setReview(next);
        setError(null);
        setPhase("ready");
      },
      (failure) => {
        if (!live.current) return;
        setError(errorMessage(failure));
        setPhase("ready");
      },
    );
    return () => {
      live.current = false;
    };
  }, [api, generation]);
  const apply = async () => {
    if (!review || phase !== "ready" || !confirmed(typed)) return;
    setPhase("applying");
    setError(null);
    try {
      await api.apply(review.token, cleanupChoice(review, deleteUnsaved, moveToTrash));
      if (live.current) setPhase("quitting");
    } catch (failure) {
      if (!live.current) return;
      // The review is used up either way: read it again before another try.
      setError(`${errorMessage(failure)} Review again to try once more.`);
      setReview(null);
      setPhase("ready");
    }
  };
  const busy = phase === "applying" || phase === "quitting";
  return (
    <Modal
      title={review?.scope === "data_folder" ? "Delete this data folder" : "Delete all Ghost data"}
      size="lg"
      className="delete-data-dialog"
      closeDisabled={busy}
      onClose={onClose}
    >
      <DeleteDataBody
        review={review}
        phase={phase}
        error={error}
        deleteUnsaved={deleteUnsaved}
        moveToTrash={moveToTrash}
        typed={typed}
        onDeleteUnsaved={setDeleteUnsaved}
        onMoveToTrash={setMoveToTrash}
        onTyped={setTyped}
        onRetry={() => {
          setError(null);
          setGeneration((value) => value + 1);
        }}
        onCancel={onClose}
        onDelete={() => void apply()}
      />
    </Modal>
  );
}

/** The dialog's contents for one state; rendered on its own in tests. */
export function DeleteDataBody({
  review,
  phase,
  error,
  deleteUnsaved,
  moveToTrash,
  typed,
  onDeleteUnsaved,
  onMoveToTrash,
  onTyped,
  onRetry,
  onCancel,
  onDelete,
}: {
  readonly review: DataCleanupReview | null;
  readonly phase: DeleteDataPhase;
  readonly error: string | null;
  readonly deleteUnsaved: boolean;
  readonly moveToTrash: boolean;
  readonly typed: string;
  readonly onDeleteUnsaved: (value: boolean) => void;
  readonly onMoveToTrash: (value: boolean) => void;
  readonly onTyped: (value: string) => void;
  readonly onRetry: () => void;
  readonly onCancel: () => void;
  readonly onDelete: () => void;
}) {
  const id = useId();
  const busy = phase === "applying" || phase === "quitting";
  if (!review) {
    return (
      <div className="delete-data" data-testid="delete-data">
        {phase === "loading" ? (
          <p className="delete-data-lead" role="status">
            Reading what Ghost keeps on this device…
          </p>
        ) : null}
        {error ? (
          <p className="settings-alert" role="alert">
            {error}
          </p>
        ) : null}
        <div className="g-dialog-actions">
          <button type="button" className="g-button" onClick={onCancel}>
            Cancel
          </button>
          {phase === "ready" ? (
            <button type="button" className="g-button g-button-primary" onClick={onRetry}>
              Review again
            </button>
          ) : null}
        </div>
      </div>
    );
  }
  const folder = review.scope === "data_folder";
  const unsaved = unsavedWorktrees(review);
  const clean = review.worktrees.length - unsaved.length;
  return (
    <div className="delete-data" data-testid="delete-data">
      <p className="delete-data-lead">
        {folder
          ? "Ghost quits, then deletes what it keeps in this data folder. Your own files in it, the installed app's data, your agents and your repositories are not touched."
          : "Ghost quits, then deletes everything below, so the next start is a fresh install. Your agents, their settings and your repositories are not touched."}
      </p>
      <dl className="delete-data-rows">
        {review.items.map((item) => (
          <div key={item.path}>
            <dt>{itemLabel(item, review.scope)}</dt>
            <dd>
              <span className="delete-data-path">{item.path}</span>
              {item.bytes !== null ? (
                <span className="delete-data-size">{formatBytes(item.bytes)}</span>
              ) : null}
            </dd>
          </div>
        ))}
        {review.secure_store ? (
          <div>
            <dt>Agent tokens</dt>
            <dd>
              {`Sign-in tokens of agent accounts you added in Ghost, kept in ${review.secure_store}.`}
            </dd>
          </div>
        ) : null}
        {review.items.length === 0 && !review.secure_store ? (
          <div>
            <dt>Files</dt>
            <dd>Nothing of Ghost's is left here.</dd>
          </div>
        ) : null}
      </dl>
      {unsaved.length > 0 ? (
        <section
          className="delete-data-worktrees"
          aria-labelledby={`${id}-worktrees`}
          data-testid="delete-data-worktrees"
        >
          <h3 id={`${id}-worktrees`}>
            {unsaved.length === 1
              ? "1 worktree has work saved nowhere else"
              : `${unsaved.length} worktrees have work saved nowhere else`}
          </h3>
          <ul>
            {unsaved.map((worktree) => (
              <li key={worktree.path}>
                <span className="delete-data-branch">{worktreeName(worktree)}</span>
                <span className="delete-data-loss">{worktree.summary}</span>
                <span className="delete-data-path">{worktree.repository ?? worktree.path}</span>
              </li>
            ))}
          </ul>
          <p className="delete-data-note">
            Kept, they stay where they are with their branches; git worktree list in each repository
            shows them.
          </p>
          <label className="settings-check">
            <input
              type="checkbox"
              data-testid="delete-data-unsaved"
              checked={deleteUnsaved}
              disabled={busy}
              onChange={(event) => onDeleteUnsaved(event.currentTarget.checked)}
            />
            Delete these worktrees too. Their uncommitted files are lost; commits stay on their
            branches.
          </label>
        </section>
      ) : null}
      <p className="delete-data-note">
        {clean > 0
          ? `${clean === 1 ? "1 worktree" : `${clean} worktrees`} with nothing unsaved ${clean === 1 ? "is" : "are"} deleted too; branches stay in your repositories. `
          : ""}
        Stays: your agents and their folders, files Ghost wrote into them at your request, and your
        repositories and branches.
      </p>
      {review.app_bundle ? (
        <label className="settings-check">
          <input
            type="checkbox"
            data-testid="delete-data-trash"
            checked={moveToTrash}
            disabled={busy}
            onChange={(event) => onMoveToTrash(event.currentTarget.checked)}
          />
          Also move Ghost to the Trash
        </label>
      ) : null}
      <div className="delete-data-confirm">
        <label className="settings-field" htmlFor={`${id}-confirm`}>
          Type {CONFIRM_WORD} to confirm
        </label>
        <input
          id={`${id}-confirm`}
          className="g-input"
          data-testid="delete-data-confirm"
          value={typed}
          autoComplete="off"
          spellCheck={false}
          disabled={busy}
          onChange={(event) => onTyped(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && confirmed(typed)) onDelete();
          }}
        />
      </div>
      {error ? (
        <p className="settings-alert" role="alert">
          {error}
        </p>
      ) : null}
      {phase === "quitting" ? (
        <p className="delete-data-lead" role="status">
          Ghost is quitting. Your data is deleted as soon as it has closed.
        </p>
      ) : null}
      <div className="g-dialog-actions">
        <button type="button" className="g-button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
        <button
          type="button"
          className="g-button g-button-danger"
          data-testid="delete-data-apply"
          disabled={busy || !confirmed(typed)}
          onClick={onDelete}
        >
          {deleteLabel(moveToTrash && review.app_bundle !== null)}
        </button>
      </div>
    </div>
  );
}
