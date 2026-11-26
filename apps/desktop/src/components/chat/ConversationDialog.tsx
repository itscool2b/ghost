import { AgentMark } from "@ghost/ui/graphite";
import { useEffect, useRef, useState } from "react";
import type {
  AgentEntry,
  ContinuationAction,
  ContinuationPreview,
  ConversationTurns,
  EditablePrompt,
  SessionStatus,
} from "../../bindings";
import { hasProcess } from "../../lib/chat/transcript";
import type { ConversationTransport } from "../../lib/conversation";
import { errorMessage } from "../../lib/errors";
import { HandoffView } from "./HandoffView";
import { Modal } from "./Modal";

export type ConversationIntent =
  | { type: "history" }
  | Exclude<ContinuationAction, { type: "edit" }>
  | { type: "edit"; turn_id: string };

export function ConversationDialog({
  chatId,
  api,
  initial,
  status,
  queued,
  agents,
  onStop,
  onApplied,
  onClose,
}: {
  readonly chatId: string;
  readonly api: ConversationTransport;
  readonly initial: ConversationIntent;
  readonly status: SessionStatus;
  readonly queued: number;
  readonly agents: readonly AgentEntry[] | null;
  readonly onStop: () => void;
  readonly onApplied: (id: string) => Promise<void>;
  readonly onClose: () => void;
}) {
  const [intent, setIntent] = useState(initial);
  const [turns, setTurns] = useState<ConversationTurns | null>(null);
  const [editable, setEditable] = useState<EditablePrompt | null>(null);
  const [text, setText] = useState("");
  const [preview, setPreview] = useState<ContinuationPreview | null>(null);
  const [busy, setBusy] = useState(true);
  const [applying, setApplying] = useState(false);
  const [completed, setCompleted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const live = hasProcess(status);
  const wasLive = useRef(live);
  const request = useRef(0);
  useEffect(() => {
    const stopped = wasLive.current && !live;
    wasLive.current = live;
    if (!stopped || !preview || intent.type === "history") return;
    // Stopping can append a final event. Rebuild its content-bound preview before confirmation.
    const action: ContinuationAction = intent.type === "edit" ? { ...intent, text } : intent;
    const current = ++request.current;
    setBusy(true);
    void api
      .preview(chatId, action)
      .then((value) => {
        if (request.current === current) setPreview(value);
      })
      .catch((error) => {
        if (request.current !== current) return;
        setError(errorMessage(error));
        setPreview(null);
      })
      .finally(() => {
        if (request.current === current) setBusy(false);
      });
  }, [api, chatId, intent, live, preview, text]);
  useEffect(() => {
    void revision;
    let stale = false;
    ++request.current;
    setBusy(true);
    setError(null);
    setPreview(null);
    setEditable(null);
    const work =
      intent.type === "history"
        ? api.turns(chatId, null).then((value) => {
            if (!stale) setTurns(value);
          })
        : intent.type === "edit"
          ? api.editable(chatId, intent.turn_id).then((value) => {
              if (!stale) {
                setEditable(value);
                setText(value.text);
              }
            })
          : api.preview(chatId, intent).then((value) => {
              if (!stale) setPreview(value);
            });
    void work
      .catch((error) => {
        if (!stale) setError(errorMessage(error));
      })
      .finally(() => {
        if (!stale) setBusy(false);
      });
    return () => {
      stale = true;
      ++request.current;
    };
  }, [api, chatId, intent, revision]);
  const editPreview = async () => {
    if (!editable) return;
    const current = ++request.current;
    setBusy(true);
    setError(null);
    try {
      const value = await api.preview(chatId, { type: "edit", turn_id: editable.turn_id, text });
      if (request.current === current) setPreview(value);
    } catch (error) {
      if (request.current === current) setError(errorMessage(error));
    } finally {
      if (request.current === current) setBusy(false);
    }
  };
  const earlier = async () => {
    if (!turns || busy) return;
    const current = ++request.current;
    setBusy(true);
    setError(null);
    try {
      const page = await api.turns(chatId, turns.before_ordinal);
      if (request.current === current)
        setTurns({ ...page, turns: [...turns.turns, ...page.turns] });
    } catch (error) {
      if (request.current === current) setError(errorMessage(error));
    } finally {
      if (request.current === current) setBusy(false);
    }
  };
  const apply = async () => {
    if (!preview || applying || busy || live || queued) return;
    setApplying(true);
    setError(null);
    let destination: string;
    try {
      destination = (await api.apply(chatId, preview.token)).chat_id;
    } catch (error) {
      setError(errorMessage(error));
      setPreview(null);
      setApplying(false);
      return;
    }
    // The mutation is acknowledged. A later refresh failure must never offer to send it again.
    setCompleted(true);
    setPreview(null);
    try {
      await onApplied(destination);
      onClose();
    } catch (error) {
      setError(
        `Continuation saved. Close this dialog and reopen the chat to refresh. ${errorMessage(error)}`,
      );
    } finally {
      setApplying(false);
    }
  };
  const agent = agents?.find((agent) => agent.id === preview?.agent_id);
  const label =
    intent.type === "continue_here"
      ? "Continue here with this handoff"
      : intent.type === "fresh_session"
        ? "Use handoff in a fresh session"
        : intent.type === "switch_agent"
          ? `Switch to ${agent?.name ?? preview?.agent_id ?? "agent"}`
          : intent.type === "fork"
            ? "Create fork"
            : intent.type === "edit"
              ? "Send edited prompt"
              : "Retry prompt";
  return (
    <Modal
      title={
        intent.type === "history"
          ? "Conversation history"
          : intent.type === "edit" && !preview
            ? "Edit prompt"
            : "Review continuation"
      }
      size="lg"
      className="conversation-dialog"
      onClose={() => {
        if (!applying) onClose();
      }}
    >
      {busy ? (
        <p className="conversation-notice" role="status">
          Preparing…
        </p>
      ) : null}
      {error ? (
        <div className="conversation-notice">
          <p className="ui-error" role="alert">
            {error}
          </p>
          {!preview && !editable && !completed ? (
            <button
              type="button"
              className="g-button"
              disabled={busy}
              onClick={() => setRevision((value) => value + 1)}
            >
              Review again
            </button>
          ) : null}
        </div>
      ) : null}
      {intent.type === "history" && turns ? (
        <>
          {turns.can_continue_here ? (
            <div className="conversation-notice">
              <p>This conversation has new history from another device.</p>
              <button
                type="button"
                className="g-button"
                disabled={busy}
                onClick={() => setIntent({ type: "continue_here" })}
              >
                Review and continue here
              </button>
            </div>
          ) : null}
          <p className="conversation-notice">
            Choose a message to retry, edit, or continue in a new chat.
          </p>
          <ol className="conversation-turns">
            {turns.turns.map((turn) => (
              <li key={turn.turn_id}>
                <div>
                  <AgentMark agent={turn.agent_id} size={16} />
                  <span>{turn.prompt_preview || "Agent activity"}</span>
                  <small>{turn.status}</small>
                </div>
                <div className="conversation-turn-actions">
                  {(["retry", "edit", "fork"] as const).map((type) => (
                    <button
                      key={type}
                      type="button"
                      className="g-button g-button-28"
                      disabled={busy || turn.status === "running"}
                      onClick={() => setIntent({ type, turn_id: turn.turn_id })}
                    >
                      {type === "retry" ? "Retry" : type === "edit" ? "Edit" : "Fork here"}
                    </button>
                  ))}
                </div>
              </li>
            ))}
          </ol>
          {!turns.turns.length ? (
            <p className="conversation-notice">This conversation has no saved turns yet.</p>
          ) : null}
          {turns.has_more ? (
            <button
              type="button"
              className="g-button"
              disabled={busy}
              onClick={() => void earlier()}
            >
              Load earlier messages
            </button>
          ) : null}
        </>
      ) : null}
      {editable && !preview && !completed ? (
        <div className="conversation-edit">
          <label htmlFor="continuation-edit-text">Your prompt</label>
          <textarea
            id="continuation-edit-text"
            className="g-textarea"
            value={text}
            onChange={(event) => setText(event.currentTarget.value)}
            rows={8}
          />
          {editable.has_context ? (
            <p>Existing attachments and file mentions stay with this prompt.</p>
          ) : null}
          <button
            type="button"
            className="g-button g-button-primary"
            disabled={busy}
            onClick={() => void editPreview()}
          >
            Review edited prompt
          </button>
        </div>
      ) : null}
      {preview ? (
        <>
          <div className="conversation-target">
            <AgentMark agent={preview.agent_id} size={24} />
            <div>
              <strong>{agent?.name ?? preview.agent_id}</strong>
              <span>
                {preview.uses_native_fork
                  ? "Forks the saved native session"
                  : preview.resumes_native
                    ? "Resumes this agent's saved session"
                    : "Starts a new native session"}
              </span>
            </div>
          </div>
          <section className="conversation-context" aria-label="Exact continuation context">
            <h3>Handoff</h3>
            <div className="conversation-handoff">
              <HandoffView
                text={preview.handoff}
                emptyText={
                  preview.uses_native_fork || preview.resumes_native
                    ? "No extra context is needed."
                    : "No earlier context is included."
                }
              />
            </div>
            {preview.prompt !== null ? (
              <>
                <h3>Prompt to send</h3>
                <pre>{preview.prompt}</pre>
              </>
            ) : (
              <p>Your next message will include this handoff. Confirming does not send a prompt.</p>
            )}
          </section>
          {preview.warnings.map((warning) => (
            <p className="conversation-notice" key={warning}>
              {warning}
            </p>
          ))}
          {queued ? (
            <p className="conversation-notice">
              Clear the {queued} queued {queued === 1 ? "message" : "messages"} before continuing.
            </p>
          ) : null}
          <div className="g-dialog-actions">
            {live ? (
              <button
                className="g-button"
                type="button"
                disabled={status === "stopping" || applying}
                onClick={onStop}
              >
                {status === "stopping" ? "Stopping…" : "Stop session to continue"}
              </button>
            ) : null}
            <button
              type="button"
              className="g-button g-button-primary"
              disabled={live || queued > 0 || applying || busy}
              onClick={() => void apply()}
            >
              {applying ? "Applying…" : label}
            </button>
          </div>
        </>
      ) : null}
    </Modal>
  );
}
