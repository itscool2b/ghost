import { AgentMark } from "@o7/ui/graphite";
import { ArrowDown, ArrowUp, Pencil, X } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";
import type { QueuedPrompt } from "../../bindings";
import type { QueueWaitView } from "../../lib/chat/queueWait";
import { fitTextarea } from "../../lib/fitTextarea";

/** The row action that keeps focus after a move or an edit re-renders the queue. */
type RowAction = "edit" | "up" | "down";

/** Who runs the queue (`QueueList`'s head). */
export interface QueueOwner {
  readonly agentId: string;
  readonly name: string;
  readonly model?: string | null;
}

/** "Codex · GPT-6 Astra", or the agent alone when the model is unknown or is the agent's name. */
export function ownerLabel(owner: QueueOwner): string {
  return owner.model && owner.model !== owner.name ? `${owner.name} · ${owner.model}` : owner.name;
}

/**
 * The prompts waiting behind the running turn (or the start), as one segment over the composer:
 * why they wait (or how many wait), then each message on its own row with its controls. Held
 * ones (queued before a stop, an error or a restart) never go on their own, and every message
 * after a held one waits behind it.
 *
 * The head says in plain words why the next message waits, with what helps: Send now while the
 * agent works or finishes its turn, Start after a failed start, Recreate folder for a missing
 * worktree, and for a held message Send it now or Discard (with Send all when several are held;
 * each later held row has its own Send). "Send queued" shows whenever the session says some are
 * held while something else is the reason.
 *
 * A row's words can be edited in place (Enter saves, Esc cancels), and Move up and Move down
 * reorder it from the keyboard as well as the pointer: the controls show under the hand and
 * whenever focus is in the row.
 *
 * The head also names who runs the queue: the chat's agent and model when its next message
 * starts. A chat can change agent while messages wait (the agent picker, Accept and build with),
 * and they then go to the new one, so the name follows the chat, never the agent that was
 * running when they were typed.
 */
export function QueueList({
  queue,
  held,
  wait = null,
  onRemove,
  onSendQueued,
  onSendNow,
  onStart,
  onRecreate,
  onSetup,
  onLoadText,
  onEdit,
  onMove,
  owner = null,
}: {
  readonly queue: readonly QueuedPrompt[];
  /** Some queued prompts are held (`SessionInfo.queue_held`). */
  readonly held: boolean;
  /** Why the next message waits (`SessionInfo.queue_wait` in words). */
  readonly wait?: QueueWaitView | null;
  readonly onRemove: (promptId: string) => void;
  readonly onSendQueued: () => void;
  /** "Send now" for the next message, and Send for one held message. */
  readonly onSendNow?: (promptId: string) => void;
  /** Starts the agent (after a failed start, or a stopped one). */
  readonly onStart?: () => void;
  /** Reviews recreating this chat's missing worktree folder. */
  readonly onRecreate?: () => void;
  readonly onSetup?: () => void;
  /** The prompt's words as typed; `null` when it is gone or could not be read. */
  readonly onLoadText?: (promptId: string) => Promise<string | null>;
  /** Saves new words; answers why they were not saved, or `null`. */
  readonly onEdit?: (promptId: string, text: string) => Promise<string | null>;
  /** Moves the prompt to `index` (0 goes next). */
  readonly onMove?: (promptId: string, index: number) => Promise<void>;
  /** Who runs the queued messages: the chat's agent, and its model when the agent lists one. */
  readonly owner?: QueueOwner | null;
}) {
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const list = useRef<HTMLUListElement>(null);
  const refocus = useRef<{ id: string; action: RowAction } | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Focus returns after the queue or the editor changes.
  useLayoutEffect(() => {
    const wanted = refocus.current;
    if (!wanted || !list.current) return;
    const row = [...list.current.querySelectorAll<HTMLElement>("[data-test-prompt-id]")].find(
      (item) => item.dataset.testPromptId === wanted.id,
    );
    if (!row) return;
    refocus.current = null;
    const button = (action: RowAction) =>
      row.querySelector<HTMLButtonElement>(`[data-queue-action="${action}"]`);
    const target = button(wanted.action);
    // A row moved to an end has that direction disabled; the other one keeps the keyboard there.
    const usable =
      target && !target.disabled ? target : button(wanted.action === "up" ? "down" : "up");
    (usable && !usable.disabled ? usable : button("edit"))?.focus();
  }, [queue, editing]);
  if (queue.length === 0 && !held) return null;
  const startEditing = async (promptId: string) => {
    const text = await onLoadText?.(promptId);
    if (text !== null && text !== undefined) setEditing({ id: promptId, text });
  };
  const move = (promptId: string, index: number, action: RowAction) => {
    refocus.current = { id: promptId, action };
    void onMove?.(promptId, index);
  };
  // A reason that already says the messages are held does not say it twice.
  const heldNote = held && wait?.reason !== "held" ? " · held" : "";
  const count = queue.length === 0 ? "Nothing queued" : `${queue.length} queued${heldNote}`;
  const next = queue.find((row) => !row.held)?.prompt_id ?? null;
  const action = wait?.action ?? null;
  // Held at the head: everything waits behind that message until the person sends or discards it.
  const heldHead =
    action === "send_held" ? (queue.find((row) => row.held) ?? queue[0] ?? null) : null;
  const heldCount = Math.max(1, queue.filter((row) => row.held).length);
  return (
    <section className="chat-queue-list" aria-label="Queued messages" data-testid="queue">
      <div className="chat-queue-head">
        {wait ? (
          <span
            className="chat-queue-reason"
            data-testid="queue-reason"
            data-test-reason={wait.reason}
            data-failed={wait.failed ? "" : undefined}
            title={wait.detail ?? undefined}
            role="status"
          >
            <span className="chat-queue-dot" aria-hidden="true" />
            <span className="chat-queue-why">
              {wait.text}
              {queue.length > 0 ? <span className="chat-queue-count"> · {count}</span> : null}
            </span>
          </span>
        ) : (
          <span>{count}</span>
        )}
        {owner && queue.length > 0 ? (
          <span
            className="chat-queue-owner"
            data-testid="queue-owner"
            data-test-agent={owner.agentId}
            title={`Queued messages run on ${ownerLabel(owner)}`}
          >
            <span className="chat-queue-owner-word">Runs on</span>
            <AgentMark agent={owner.agentId} size={14} />
            <span className="chat-queue-owner-name">{ownerLabel(owner)}</span>
          </span>
        ) : null}
        <span className="chat-queue-head-actions">
          {action === "send_now" && next !== null && onSendNow ? (
            <button
              type="button"
              className="g-button g-button-24"
              data-testid="queue-send-now"
              title={wait?.detail ?? "Send the next message now"}
              onClick={() => onSendNow(next)}
            >
              Send now
            </button>
          ) : null}
          {action === "start" && onStart ? (
            <button
              type="button"
              className="g-button g-button-24"
              data-testid="queue-start"
              onClick={onStart}
            >
              {wait?.reason === "start_failed" ? "Retry" : "Start"}
            </button>
          ) : null}
          {action === "recreate" && onRecreate ? (
            <button
              type="button"
              className="g-button g-button-24"
              data-testid="queue-recreate"
              onClick={onRecreate}
            >
              Recreate folder
            </button>
          ) : null}
          {action === "setup" && onSetup ? (
            <button type="button" className="g-button g-button-24" onClick={onSetup}>
              Retry setup
            </button>
          ) : null}
          {heldHead !== null ? (
            <>
              {onSendNow ? (
                <button
                  type="button"
                  className="g-button g-button-24"
                  data-testid="queue-send-held"
                  title="Send the held message now; the messages after it follow"
                  onClick={() => onSendNow(heldHead.prompt_id)}
                >
                  Send it now
                </button>
              ) : null}
              <button
                type="button"
                className="g-button g-button-ghost g-button-24"
                data-testid="queue-discard-held"
                title="Remove the held message; the next one goes"
                onClick={() => onRemove(heldHead.prompt_id)}
              >
                Discard
              </button>
              {heldCount > 1 ? (
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-24"
                  data-testid="send-queued"
                  title="Send every held message, in order"
                  onClick={onSendQueued}
                >
                  Send all
                </button>
              ) : null}
            </>
          ) : held ? (
            <button
              type="button"
              className="g-button g-button-24"
              data-testid="send-queued"
              title="Held messages wait for you after a stop, an error or a restart."
              onClick={onSendQueued}
            >
              Send queued
            </button>
          ) : null}
        </span>
      </div>
      {queue.length > 0 ? (
        <ul className="chat-queue" ref={list}>
          {queue.map((row, index) => (
            <li
              key={row.prompt_id}
              data-testid="queued-prompt"
              data-test-prompt-id={row.prompt_id}
              data-test-held={String(row.held)}
              data-editing={editing?.id === row.prompt_id ? "" : undefined}
            >
              {editing?.id === row.prompt_id && onEdit ? (
                <QueueEditor
                  initial={editing.text}
                  onCancel={() => {
                    refocus.current = { id: row.prompt_id, action: "edit" };
                    setEditing(null);
                  }}
                  onSave={async (text) => {
                    const error = await onEdit(row.prompt_id, text);
                    if (error === null) {
                      refocus.current = { id: row.prompt_id, action: "edit" };
                      setEditing(null);
                    }
                    return error;
                  }}
                />
              ) : (
                <>
                  <span title={row.preview} data-testid="queued-preview">
                    {row.preview || "(empty)"}
                  </span>
                  {row.held ? <span className="chat-held">held</span> : null}
                  {onEdit || onMove ? (
                    <span className="chat-queue-actions">
                      {onEdit && onLoadText ? (
                        <button
                          type="button"
                          className="g-button g-button-ghost g-button-icon g-button-24"
                          data-testid="queued-edit"
                          data-queue-action="edit"
                          aria-label="Edit queued message"
                          title="Edit"
                          onClick={() => void startEditing(row.prompt_id)}
                        >
                          <Pencil size={14} />
                        </button>
                      ) : null}
                      {onMove ? (
                        <>
                          <button
                            type="button"
                            className="g-button g-button-ghost g-button-icon g-button-24"
                            data-testid="queued-up"
                            data-queue-action="up"
                            aria-label="Move up in the queue"
                            title="Move up"
                            disabled={index === 0}
                            onClick={() => move(row.prompt_id, index - 1, "up")}
                          >
                            <ArrowUp size={14} />
                          </button>
                          <button
                            type="button"
                            className="g-button g-button-ghost g-button-icon g-button-24"
                            data-testid="queued-down"
                            data-queue-action="down"
                            aria-label="Move down in the queue"
                            title="Move down"
                            disabled={index === queue.length - 1}
                            onClick={() => move(row.prompt_id, index + 1, "down")}
                          >
                            <ArrowDown size={14} />
                          </button>
                        </>
                      ) : null}
                    </span>
                  ) : null}
                  {row.held && onSendNow && row !== heldHead ? (
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-24 chat-queue-send"
                      data-testid="queued-send"
                      aria-label="Send this message now"
                      title="Send this message now; the others stay held"
                      onClick={() => onSendNow(row.prompt_id)}
                    >
                      Send
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="g-button g-button-ghost g-button-icon g-button-24"
                    data-testid="queued-remove"
                    aria-label="Remove from the queue"
                    title="Remove from the queue"
                    onClick={() => onRemove(row.prompt_id)}
                  >
                    <X size={14} />
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      ) : null}
      {held && wait?.reason !== "held" ? (
        <p className="chat-queue-note">
          Held messages wait for you after a stop, an error or a restart.
        </p>
      ) : null}
    </section>
  );
}

/** A queued message's words in a field: Enter saves, Shift+Enter adds a line, Esc cancels. */
export function QueueEditor({
  initial,
  onSave,
  onCancel,
}: {
  readonly initial: string;
  /** Answers why the words were not saved, or `null`. */
  readonly onSave: (text: string) => Promise<string | null>;
  readonly onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const errorId = useId();
  // biome-ignore lint/correctness/useExhaustiveDependencies: The value changes scrollHeight after React commits the textarea.
  useLayoutEffect(() => {
    if (!field.current) return;
    fitTextarea(field.current, 36, 160);
  }, [text]);
  useLayoutEffect(() => {
    const node = field.current;
    if (!node) return;
    node.focus();
    node.setSelectionRange(node.value.length, node.value.length);
  }, []);
  const save = async () => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      setError(await onSave(text));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="chat-queue-editor">
      <textarea
        ref={field}
        className="g-textarea"
        data-testid="queued-edit-input"
        aria-label="Queued message"
        aria-invalid={error !== null}
        aria-describedby={error !== null ? errorId : undefined}
        value={text}
        readOnly={saving}
        onChange={(event) => setText(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Escape") {
            // Esc here only closes the editor; it never reaches the chat, where it stops a turn.
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          } else if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            void save();
          }
        }}
      />
      {error !== null ? (
        <p id={errorId} className="chat-queue-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="chat-queue-editor-actions">
        <button
          type="button"
          className="g-button g-button-ghost g-button-24"
          disabled={saving}
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          type="button"
          className="g-button g-button-24"
          data-testid="queued-edit-save"
          disabled={saving}
          onClick={() => void save()}
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
