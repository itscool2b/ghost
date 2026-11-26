import { useEffect, useRef, useState } from "react";
import type { ProjectEditor, WorktreeActionPreview } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { editorName, type ProjectToolsApi } from "../../lib/projectTools";

/** At most this many paths go into the message; the rest are counted. */
const LISTED = 30;

/**
 * The message the chat's agent receives when asked to resolve a previewed merge's conflicts: the
 * branches, the folder and every conflicted path, in plain words the person reads before sending.
 * The merge happens on the worktree's branch, so the target branch and its files stay untouched.
 */
export function conflictPrompt(preview: WorktreeActionPreview): string {
  const branch = preview.worktree.branch;
  const target = preview.target?.branch ?? "the target branch";
  const files = preview.conflicts;
  const listed = files.slice(0, LISTED).map((path) => `- ${path}`);
  if (files.length > LISTED) listed.push(`- and ${files.length - LISTED} more`);
  return [
    `Merging ${branch} into ${target} has conflicts in ${files.length === 1 ? "1 file" : `${files.length} files`}:`,
    ...listed,
    "",
    `Please resolve them on ${branch} in ${preview.worktree.path}: merge ${target} into ${branch}, fix the conflicts in the files above while keeping the intent of both sides, run the relevant tests, and commit the merge. Do not push and do not change ${target}. I will review the merge in Ghost afterwards.`,
  ].join("\n");
}

/**
 * A previewed merge that conflicts: the files, then two ways forward that never merge anything by
 * themselves. "Ask the agent" sends the message shown here to this chat's agent as an ordinary,
 * visible message; "Open in" hands the chat's folder to a detected editor. Merge stays disabled in
 * the dialog until a new review finds no conflicts.
 */
export function MergeConflicts({
  chatId,
  preview,
  inWorktree,
  agentName,
  onAskAgent,
  projectTools,
}: {
  readonly chatId: string;
  readonly preview: WorktreeActionPreview;
  /** The chat's own folder is this worktree, so its agent and its editor work where Git resolves. */
  readonly inWorktree: boolean;
  readonly agentName: string;
  /** Sends a visible message; resolves to null once sent, or to why it was not. */
  readonly onAskAgent?: (text: string) => Promise<string | null>;
  readonly projectTools?: Pick<ProjectToolsApi, "review" | "openEditor">;
}) {
  const prompt = conflictPrompt(preview);
  const [editors, setEditors] = useState<{ token: string; list: ProjectEditor[] } | null>(null);
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  // A read-only review of the chat's folder says which editors are installed; each open uses it up.
  useEffect(() => {
    void revision;
    if (!inWorktree || !projectTools) return;
    let current = true;
    projectTools.review(chatId).then(
      (review) => {
        if (current) setEditors({ token: review.token, list: review.editors });
      },
      (failure) => {
        if (current) setError(errorMessage(failure));
      },
    );
    return () => {
      current = false;
    };
  }, [chatId, inWorktree, projectTools, revision]);

  const ask = async () => {
    if (!onAskAgent || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    const refused = await onAskAgent(prompt);
    if (!alive.current) return;
    setBusy(false);
    if (refused) setError(refused);
    else setNotice(`Sent to ${agentName}. Review the merge again once it commits.`);
  };
  const open = async (editor: ProjectEditor) => {
    if (!editors || !projectTools || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    const token = editors.token;
    setEditors(null);
    try {
      await projectTools.openEditor(chatId, token, editor);
      if (alive.current)
        setNotice(
          `The operating system accepted the request for ${editorName[editor]}. Merge ${preview.target?.branch ?? "the target branch"} into ${preview.worktree.branch} there, resolve and commit, then review the merge again.`,
        );
    } catch (failure) {
      if (alive.current) setError(errorMessage(failure));
    } finally {
      if (alive.current) {
        setBusy(false);
        setRevision((value) => value + 1);
      }
    }
  };

  return (
    <div className="git-conflicts" data-testid="git-conflicts">
      <h4 className="git-conflict-title">Conflicts to resolve first</h4>
      <ul>
        {preview.conflicts.map((path) => (
          <li key={path}>
            <code>{path}</code>
          </li>
        ))}
      </ul>
      {inWorktree ? (
        <>
          <p className="git-note">
            Nothing has changed yet. Resolve them on{" "}
            <code className="git-inline">{preview.worktree.branch}</code>, then review the merge
            again.
          </p>
          {onAskAgent ? (
            <figure className="git-conflict-message">
              <figcaption className="git-conflict-label">Message to {agentName}</figcaption>
              <pre data-testid="git-conflict-prompt">{prompt}</pre>
            </figure>
          ) : null}
          <div className="git-actions git-conflict-actions">
            {editors?.list.map((editor) => (
              <button
                key={editor}
                type="button"
                className="g-button g-button-28"
                disabled={busy}
                onClick={() => void open(editor)}
              >
                Open in {editorName[editor]}
              </button>
            ))}
            {onAskAgent ? (
              <button
                type="button"
                className="g-button g-button-28"
                data-testid="git-ask-agent"
                disabled={busy}
                onClick={() => void ask()}
              >
                Ask {agentName} to resolve
              </button>
            ) : null}
          </div>
          {editors && editors.list.length === 0 ? (
            <p className="git-help git-conflict-help">
              No supported editor was found. Open {preview.worktree.path} in your editor.
            </p>
          ) : null}
          {notice ? (
            <p className="git-note" role="status">
              {notice}
            </p>
          ) : null}
          {error ? (
            <p className="git-conflict-error" role="alert">
              {error}
            </p>
          ) : null}
        </>
      ) : (
        <p className="git-note">
          They are resolved in the worktree at{" "}
          <code className="git-inline">{preview.worktree.path}</code>. Move this chat into it to ask
          its agent or open it in an editor from here.
        </p>
      )}
    </div>
  );
}
