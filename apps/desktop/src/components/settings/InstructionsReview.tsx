import { useEffect, useId, useRef, useState } from "react";
import type {
  InstructionsAction,
  InstructionsChange,
  InstructionsReview,
  InstructionsSnapshot,
  InstructionsTarget,
} from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { fileByKey, type InstructionsApi } from "../../lib/instructions";
import { DiffBlock } from "../chat/DiffBlock";
import { Modal } from "../chat/Modal";

export interface ReviewChoice {
  readonly id: string;
  readonly label: string;
  readonly detail?: string;
  readonly change: InstructionsChange;
}

const ACTION: Record<InstructionsAction, string> = {
  create: "Create",
  replace: "Replace",
  remove: "Remove",
  rename: "Rename",
  keep: "Keep",
};

/**
 * The review of one change (visual-language 6.19, Vercel's transfer dialog): the title, Rust's
 * sentence, an explicit choice where one is needed, every file with what happens to it and its
 * diff, then Cancel and Apply. Nothing is written until Apply; a file changed meanwhile is refused
 * and reviewed again.
 */
export function ReviewDialog({
  api,
  target,
  snapshot,
  change,
  choose,
  onClose,
  onApplied,
}: {
  readonly api: InstructionsApi;
  readonly target: InstructionsTarget;
  readonly snapshot: InstructionsSnapshot | null;
  readonly change: InstructionsChange | null;
  readonly choose: {
    readonly title: string;
    readonly sentence: string;
    readonly choices: readonly ReviewChoice[];
  } | null;
  readonly onClose: () => void;
  readonly onApplied: (message: string, savedKey?: string) => void;
}) {
  const [choice, setChoice] = useState<string | null>(null);
  const [remove, setRemove] = useState<readonly string[]>([]);
  const [review, setReview] = useState<InstructionsReview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [busy, setBusy] = useState(false);
  const [applying, setApplying] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const reviewId = useRef<string | null>(null);
  const choiceName = useId();
  const base = choose?.choices.find((option) => option.id === choice)?.change ?? change;
  const current: InstructionsChange | null =
    base && base.type === "universal" ? { ...base, remove: [...remove] } : base;
  const requestKey = JSON.stringify(current);

  // biome-ignore lint/correctness/useExhaustiveDependencies: The serialized change is the request's identity; `attempt` reviews again on request.
  useEffect(() => {
    if (!current) return;
    let live = true;
    setBusy(true);
    setError(null);
    setStale(false);
    const previous = reviewId.current;
    if (previous) void api.cancel(previous);
    reviewId.current = null;
    api.review(target, current).then(
      (next) => {
        if (!live) {
          void api.cancel(next.id);
          return;
        }
        reviewId.current = next.id;
        setReview(next);
        setBusy(false);
      },
      (failure: unknown) => {
        if (!live) return;
        setReview(null);
        setError(errorMessage(failure));
        setBusy(false);
      },
    );
    return () => {
      live = false;
    };
  }, [api, target, requestKey, attempt]);

  const close = () => {
    if (reviewId.current) void api.cancel(reviewId.current);
    reviewId.current = null;
    onClose();
  };

  const apply = async () => {
    if (!review) return;
    setApplying(true);
    setError(null);
    const id = review.id;
    reviewId.current = null;
    try {
      const result = await api.apply(id);
      if (result.rolled_back || result.recovery_required) {
        setReview(null);
        const message =
          result.message ??
          (result.rolled_back
            ? "The change stopped. Every file was put back."
            : "The change stopped. Some files may have changed.");
        setError(
          result.backup
            ? `${message} Backups and recovery details are kept at ${result.backup}.`
            : message,
        );
        setApplying(false);
        return;
      }
      const saved = current?.type === "save" ? current.file : undefined;
      onApplied(
        result.message ??
          (review.files.length === 1 && current?.type === "save"
            ? `Saved ${review.files[0]?.path}. New sessions read it.`
            : "Saved. New agent sessions read the new files."),
        saved,
      );
    } catch (failure) {
      const message = errorMessage(failure);
      setReview(null);
      setStale(/changed after it was displayed|changed while/i.test(message));
      setError(message);
      setApplying(false);
    }
  };

  const title = review?.title ?? choose?.title ?? "Review changes";
  const writes = review?.files.filter((file) => file.action !== "keep").length ?? 0;
  return (
    <Modal title={title} size="lg" onClose={close} closeDisabled={applying}>
      <div className="instructions-review" data-testid="instructions-review">
        {choose ? (
          <fieldset className="instructions-choices">
            <legend className="instructions-lead">{choose.sentence}</legend>
            {choose.choices.map((option) => (
              <label key={option.id} className="instructions-choice">
                <input
                  type="radio"
                  name={choiceName}
                  value={option.id}
                  checked={choice === option.id}
                  disabled={busy}
                  onChange={() => setChoice(option.id)}
                />
                <span>
                  <strong>{option.label}</strong>
                  {option.detail ? <small>{option.detail}</small> : null}
                </span>
              </label>
            ))}
          </fieldset>
        ) : null}
        {review ? (
          <>
            <p className="instructions-lead">{review.summary}</p>
            {review.files.length > 0 ? (
              <ul className="instructions-review-files">
                {review.files.map((file, index) => (
                  <li key={file.key} data-action={file.action}>
                    <div className="instructions-review-file">
                      <span className="instructions-review-action">{ACTION[file.action]}</span>
                      <span className="settings-mono instructions-review-path">{file.path}</span>
                      {file.how ? (
                        <span className="instructions-review-how">{file.how}</span>
                      ) : null}
                    </div>
                    {file.action === "create" || file.action === "replace" ? (
                      <DiffBlock
                        path={file.path}
                        oldText={file.before}
                        newText={file.after ?? ""}
                        disclosureKey={`instructions:${review.id}:${index}`}
                        wholeFile
                      />
                    ) : null}
                    {[
                      ...file.notes,
                      ...(file.backup && file.action !== "remove"
                        ? ["The replaced text is backed up in Ghost's data folder."]
                        : []),
                    ].map((note) => (
                      <p className="instructions-review-note" key={note}>
                        {note}
                      </p>
                    ))}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="instructions-review-note">
                Nothing changes unless you remove a file below.
              </p>
            )}
            {review.optional_removals.length > 0 ? (
              <fieldset className="instructions-removals">
                <legend>Remove, with a backup</legend>
                {review.optional_removals.map((key) => (
                  <label key={key}>
                    <input
                      type="checkbox"
                      checked={remove.includes(key)}
                      disabled={busy}
                      onChange={(event) => {
                        const on = event.currentTarget.checked;
                        setRemove((list) =>
                          on ? [...list, key] : list.filter((item) => item !== key),
                        );
                      }}
                    />
                    <span className="settings-mono">
                      {(snapshot && fileByKey(snapshot, key)?.path) ?? key}
                    </span>
                  </label>
                ))}
              </fieldset>
            ) : null}
            {review.notes.map((note) => (
              <p className="instructions-review-note" key={note}>
                {note}
              </p>
            ))}
          </>
        ) : busy ? (
          <p className="instructions-review-note" role="status">
            Preparing the review…
          </p>
        ) : null}
        {error ? (
          <div className="instructions-review-error" role="alert">
            <p>{error}</p>
            {stale || current ? (
              <button
                className="g-button g-button-outline g-button-28"
                type="button"
                onClick={() => setAttempt((value) => value + 1)}
              >
                Review again
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
      <div className="g-dialog-actions">
        <button className="g-button" type="button" onClick={close} disabled={applying}>
          Cancel
        </button>
        <button
          className="g-button g-button-primary"
          type="button"
          disabled={!review || busy || applying || (writes === 0 && review.files.length === 0)}
          onClick={() => void apply()}
        >
          {applying ? "Applying…" : "Apply"}
        </button>
      </div>
    </Modal>
  );
}
