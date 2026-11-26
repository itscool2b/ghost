import { useState } from "react";
import type {
  ManagedWorktree,
  WorktreeRecoveryPreview,
  WorktreeRecreatePreview,
} from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { withoutVerbatimPrefix } from "../../lib/gitWorkspace";
import type { WorktreeRecoveryApi } from "../../lib/worktreeRecovery";
import "./git-dialog.css";

/** A folder's last name: a worktree's app-owned folder, or the project it came from. */
function folderName(path: string): string {
  const plain = withoutVerbatimPrefix(path);
  return plain.split(/[\\/]/).filter(Boolean).pop() ?? plain;
}

/**
 * States a person should look at now: an operation stopped half way. A worktree kept from a chat
 * that is gone (ready), one retired for recovery and one whose folder is missing can wait.
 */
export function needsRecovery(record: ManagedWorktree): boolean {
  return !["ready", "retired", "missing"].includes(record.state);
}

/**
 * The worktrees to offer for review: those no saved chat holds, and any whose operation stopped
 * half way, from `inProject` when a project is chosen. Which ones no chat holds can only be told
 * once the chats have loaded (`savedChats` null until then): before, every chat's worktree looked
 * kept, and each start of Ghost listed them all.
 */
export function worktreesToReview(
  records: readonly ManagedWorktree[],
  savedChats: ReadonlySet<string> | null,
  inProject: (record: ManagedWorktree) => boolean = () => true,
): ManagedWorktree[] {
  if (savedChats === null) return [];
  return records.filter(
    (record) =>
      inProject(record) &&
      (!savedChats.has(record.chat_id) || !["ready", "retired"].includes(record.state)),
  );
}

export function WorktreeRecoveryList({
  records,
  api,
  onChanged,
  heading = true,
}: {
  readonly records: readonly ManagedWorktree[];
  readonly api: WorktreeRecoveryApi;
  readonly onChanged: () => void;
  /** Its own heading and introduction; off where a disclosure already names the list. */
  readonly heading?: boolean;
}) {
  const [review, setReview] = useState<{
    value: WorktreeRecoveryPreview | WorktreeRecreatePreview;
    recreate: boolean;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (failure) {
      setError(errorMessage(failure));
      setReview(null);
    } finally {
      setBusy(false);
    }
  };
  if (!records.length) return null;
  return (
    <section className={heading ? "new-chat-more" : undefined} aria-label="Retained worktrees">
      {heading ? (
        <>
          <h2>Retained worktrees</h2>
          <p>Review worktrees whose chat could not be saved or whose operation needs recovery.</p>
        </>
      ) : null}
      {error ? (
        <p role="alert" className="ui-error">
          {error}
        </p>
      ) : null}
      <ul className="git-worktrees git-worktrees-inline">
        {records.map((record) => (
          <li key={record.id}>
            <strong>{record.branch}</strong>
            {/* Names, not paths: the full paths are a hover away, and Open folder shows it. */}
            <p
              className="git-path"
              title={`Source: ${withoutVerbatimPrefix(record.source)}\nFolder: ${withoutVerbatimPrefix(record.path)}`}
            >
              {folderName(record.source)} · {folderName(record.path)}
            </p>
            <p>
              {record.detail ??
                (record.state === "ready"
                  ? "The worktree is reconciled and retained without a saved chat."
                  : `State: ${record.state.replaceAll("_", " ")}`)}
            </p>
            <div className="git-actions">
              {record.state !== "ready" ? (
                <button
                  type="button"
                  className="g-button g-button-24"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const recreate = record.state === "missing";
                      const value = recreate
                        ? await api.recreate(record.id)
                        : await api.preview(
                            record.id,
                            record.state === "retired" ? "restore" : "reconcile",
                          );
                      setReview({ value, recreate });
                    })
                  }
                >
                  {record.state === "missing"
                    ? "Review recreation"
                    : record.state === "retired"
                      ? "Review restore"
                      : "Review recovery"}
                </button>
              ) : null}
              <button
                type="button"
                className="g-button g-button-ghost g-button-24"
                disabled={busy}
                onClick={() => void run(() => api.open(record.id))}
              >
                Open folder
              </button>
            </div>
          </li>
        ))}
      </ul>
      {review ? (
        <section className="git-review" aria-label="Review retained worktree recovery">
          <h3>{review.value.worktree.branch}</h3>
          <p>{"destination" in review.value ? review.value.destination : review.value.path}</p>
          {review.value.warnings.map((warning) => (
            <p key={warning}>{warning}</p>
          ))}
          <button
            type="button"
            className="g-button"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                const result = await api.apply(
                  review.value.worktree.id,
                  review.value.token,
                  review.recreate,
                );
                setReview(null);
                onChanged();
                if (result.error) setError(result.error);
              })
            }
          >
            Apply reviewed recovery
          </button>
          <button
            type="button"
            className="g-button g-button-ghost"
            disabled={busy}
            onClick={() => setReview(null)}
          >
            Cancel
          </button>
        </section>
      ) : null}
    </section>
  );
}
