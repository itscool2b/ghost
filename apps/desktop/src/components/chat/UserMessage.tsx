import { ArrowRightLeft, BookOpen, ChevronRight, History, Pencil, Workflow } from "lucide-react";
import { useCallback, useId, useMemo, useState } from "react";
import type { ChatImageContext } from "../../bindings";
import { parseHandoff } from "../../lib/chat/handoff";
import type { MemoryContext } from "../../lib/chat/memoryContext";
import { type ProjectEvent, projectEvent } from "../../lib/chat/projectEvent";
import { steeringState, type UserItem } from "../../lib/chat/transcript";
import { MessageImages } from "./ChatImages";
import { CopyAction } from "./CopyAction";
import { useDisclosure } from "./DisclosureMemory";
import { HandoffView, handoffSummary } from "./HandoffView";

export function UserMessage({
  item,
  onEdit,
  onRewind,
  settled = false,
  images,
}: {
  readonly item: UserItem;
  readonly onEdit?: () => void;
  /** Opens Changes against the files as they were before this message (ChangesPanel.tsx). */
  readonly onRewind?: () => void;
  readonly settled?: boolean;
  readonly images?: ChatImageContext;
}) {
  const delivery = steeringState(item, settled);
  const event = projectEvent(item.text);
  const [expanded, setExpanded] = useDisclosure(item.key, "prompt", false);
  const [long, setLong] = useState(() => item.text.split("\n").length > 12);
  const measure = useCallback((node: HTMLDivElement | null) => {
    if (!node) return;
    const update = () => setLong(node.scrollHeight > 252);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  if (event)
    return (
      <div className="chat-user-wrap" data-testid="item" data-test-kind="project-event">
        <SentEvent itemKey={item.key} event={event} />
        {item.memory ? <AttachedMemory itemKey={item.key} memory={item.memory} /> : null}
      </div>
    );
  return (
    <div className="chat-user-wrap" data-testid="item" data-test-kind="user">
      {item.handoff ? <SentHandoff itemKey={item.key} handoff={item.handoff} /> : null}
      {/* The block and its actions keep to the words' width when memory opens wider below. */}
      {item.text || !item.memory ? (
        <div className="chat-user-row">
          <div className="chat-user" data-folded={long && !expanded}>
            <div className="chat-user-text" data-testid="prompt-text">
              <div ref={measure}>{item.text}</div>
            </div>
            {long ? (
              <button
                className="chat-user-expand"
                type="button"
                aria-expanded={expanded}
                onClick={() => setExpanded(!expanded)}
              >
                {expanded ? "Show less" : "Show all"}
              </button>
            ) : null}
          </div>
          <div className="chat-user-actions">
            {onEdit && !item.steering ? (
              <button
                type="button"
                className="chat-action"
                aria-label="Edit prompt"
                title="Edit and resend"
                onClick={onEdit}
              >
                <Pencil size={16} />
              </button>
            ) : null}
            {onRewind && !item.steering ? (
              <button
                type="button"
                className="chat-action"
                aria-label="Rewind to before this message"
                title="Rewind files to before this message"
                onClick={onRewind}
              >
                <History size={16} />
              </button>
            ) : null}
            <CopyAction text={item.text} label="Copy prompt" />
          </div>
        </div>
      ) : null}
      {images ? <MessageImages context={images} /> : null}
      {item.memory ? <AttachedMemory itemKey={item.key} memory={item.memory} /> : null}
      {delivery ? (
        <div className="chat-steering-status" role="status" data-steering-state={delivery}>
          <span>
            {
              {
                pending: "Sending to current turn…",
                accepted: "Delivered to current turn",
                rejected: "Not delivered",
                unconfirmed: "Delivery unconfirmed",
              }[delivery]
            }
          </span>
          {item.steering?.detail ? <span>{item.steering.detail}</span> : null}
        </div>
      ) : null}
    </div>
  );
}

/**
 * The handoff a new agent received with this message: one quiet line saying what it held, which
 * opens it laid out, with the exact text one row further. The person did not type it, so it is
 * no bubble.
 */
function SentHandoff({ itemKey, handoff }: { readonly itemKey: string; readonly handoff: string }) {
  const [open, setOpen] = useDisclosure(itemKey, "handoff", false);
  const body = useId();
  const parsed = useMemo(() => parseHandoff(handoff), [handoff]);
  const summary = parsed ? handoffSummary(parsed) : "";
  return (
    <div className="chat-sent-handoff" data-testid="sent-handoff">
      <button
        type="button"
        className="chat-project-event-toggle"
        aria-expanded={open}
        aria-controls={body}
        title="Context o7 sent to the new agent with this message"
        onClick={() => setOpen(!open)}
      >
        <ArrowRightLeft size={14} aria-hidden="true" />
        <span>Handoff sent with this message</span>
        {summary ? <span className="chat-sent-handoff-summary">{summary}</span> : null}
        <ChevronRight className="chat-disclosure" size={12} aria-hidden="true" />
      </button>
      {open ? (
        <div className="chat-sent-handoff-body" id={body}>
          <HandoffView text={handoff} />
        </div>
      ) : null}
    </div>
  );
}

/**
 * A prompt o7 sent for the chat's coordinated project (a worker's report, an assignment, a
 * routine): one quiet line naming it, which opens what the agent was sent. The person did not
 * write it, so it is no bubble and offers no edit or rewind.
 */
function SentEvent({ itemKey, event }: { readonly itemKey: string; readonly event: ProjectEvent }) {
  const [open, setOpen] = useDisclosure(itemKey, "project-event", false);
  const body = useId();
  return (
    <div className="chat-project-event" data-testid="project-event">
      <button
        type="button"
        className="chat-project-event-toggle"
        aria-expanded={event.body ? open : undefined}
        aria-controls={event.body ? body : undefined}
        title="Sent to the agent by o7 for this project"
        onClick={() => setOpen(!open)}
      >
        <Workflow size={14} aria-hidden="true" />
        <span>{event.title}</span>
        {event.body ? (
          <ChevronRight className="chat-disclosure" size={12} aria-hidden="true" />
        ) : null}
      </button>
      {open && event.body ? (
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a long event scrolls by keyboard.
        <pre className="chat-project-event-body" id={body} tabIndex={0}>
          {event.body}
        </pre>
      ) : null}
    </div>
  );
}

/**
 * The project memory Aura sent along with a message, folded to one quiet line under it. The
 * agent received all of it; the reader opens it only to check what that was.
 */
function AttachedMemory({
  itemKey,
  memory,
}: {
  readonly itemKey: string;
  readonly memory: MemoryContext;
}) {
  const [open, setOpen] = useDisclosure(itemKey, "memory", false);
  const body = useId();
  return (
    <div className="chat-user-memory" data-testid="prompt-memory">
      <button
        type="button"
        className="chat-user-memory-toggle"
        aria-expanded={open}
        aria-controls={body}
        title="Sent to the agent with this message as reference"
        onClick={() => setOpen(!open)}
      >
        <BookOpen size={14} aria-hidden="true" />
        <span>Project memory</span>
        <ChevronRight className="chat-disclosure" size={12} aria-hidden="true" />
      </button>
      {open ? (
        <div className="chat-user-memory-body" id={body}>
          <p className="chat-user-memory-note">
            Sent to the agent with this message as reference.
            {memory.editedBy ? ` ${memory.editedBy}` : ""}
          </p>
          {/* biome-ignore lint/a11y/noNoninteractiveTabindex: a long memory scrolls by keyboard. */}
          <pre className="chat-user-memory-text" tabIndex={0}>
            {memory.markdown}
          </pre>
        </div>
      ) : null}
    </div>
  );
}
