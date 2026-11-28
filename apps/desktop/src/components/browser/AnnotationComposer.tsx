import { ArrowUp, MessageSquarePlus, X } from "lucide-react";
import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { type Annotation, elementLabel } from "../../lib/annotation";
import { fitTextarea } from "../../lib/fitTextarea";

/** Where the note sits: below what was picked, or above it when there is no room, inside the page. */
export function notePosition(
  target: { x: number; y: number; width: number; height: number },
  page: { width: number; height: number },
  note: { width: number; height: number },
  gap = 10,
): { left: number; top: number } {
  const margin = 8;
  const left = Math.min(
    Math.max(margin, target.x),
    Math.max(margin, page.width - note.width - margin),
  );
  const below = target.y + target.height + gap;
  const above = target.y - gap - note.height;
  const top =
    below + note.height <= page.height - margin
      ? below
      : above >= margin
        ? above
        : Math.max(margin, Math.min(page.height - note.height - margin, target.y + gap));
  return { left, top };
}

/**
 * Annotate's small composer, beside what was picked on the frozen page (Cursor's and Codex's
 * point-and-prompt): what the note is about, the note, the chat's own model picker, and Send (to
 * this chat now, or queued after its turn) or Add to chat (into the message box, for one message
 * about several annotations). Enter sends; Esc closes it and goes back to picking.
 */
export function AnnotationComposer({
  annotation,
  subject: givenSubject,
  placeholder,
  noteOptional = false,
  testId = "browser-note",
  anchor,
  page,
  controls,
  agentName,
  images,
  sending,
  error,
  onSend,
  onAdd,
  onCancel,
  onRemove,
}: {
  /** What was picked on a page. Without one, `subject` says what the note is about. */
  readonly annotation?: Annotation;
  /** What the note is about, for a note that is not about a page's elements (the game's). */
  readonly subject?: string;
  readonly placeholder?: string;
  /** The note may go with no words of the person's: what it carries says enough. */
  readonly noteOptional?: boolean;
  readonly testId?: string;
  /** What it points at, in the page area's CSS pixels. */
  readonly anchor: { x: number; y: number; width: number; height: number };
  readonly page: { width: number; height: number };
  /** The chat's agent, model and thinking picker. */
  readonly controls?: ReactNode;
  readonly agentName: string;
  /** Whether the agent sees images: `false` sends the note without its screenshot. */
  readonly images: boolean | null;
  readonly sending: boolean;
  readonly error: string | null;
  readonly onSend: (note: string) => void;
  readonly onAdd: (note: string) => void;
  readonly onCancel: () => void;
  /** Takes one element off a note about several. */
  readonly onRemove?: (index: number) => void;
}) {
  const [note, setNote] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 380, height: 168 });

  useEffect(() => {
    field.current?.focus();
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the note's text changes its height after React commits it.
  useLayoutEffect(() => {
    if (field.current) fitTextarea(field.current, 44, 160, () => 0);
    const node = box.current;
    if (node && (node.offsetWidth !== size.width || node.offsetHeight !== size.height))
      setSize({ width: node.offsetWidth, height: node.offsetHeight });
  }, [note]);

  const at = notePosition(anchor, page, size);
  const elements = annotation?.elements ?? [];
  const many = elements.length > 1;
  const area = annotation?.pick.mode === "area";
  const subject =
    givenSubject ??
    (area
      ? `The area marked${elements[0] ? ` around ${elementLabel(elements[0])}` : ""}`
      : many
        ? `${elements.length} elements`
        : elements[0]
          ? elementLabel(elements[0])
          : "");
  const empty = !noteOptional && !note.trim();
  return (
    <div
      ref={box}
      className="browser-note"
      role="dialog"
      aria-label="Annotation"
      data-testid={testId}
      style={{ left: at.left, top: at.top } as CSSProperties}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          event.preventDefault();
          event.stopPropagation();
          onCancel();
        }
      }}
    >
      <div className="browser-note-subject">
        <span className="browser-note-subject-text" title={subject}>
          {subject}
        </span>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-24"
          aria-label="Close the note"
          title="Close the note (Esc)"
          onClick={onCancel}
        >
          <X size={14} />
        </button>
      </div>
      {many ? (
        <ol className="browser-note-elements">
          {elements.map((element, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: the same element may be picked twice; its place tells them apart
            <li key={`${element.selector}-${index}`}>
              <span className="browser-note-number" aria-hidden="true">
                {index + 1}
              </span>
              <span className="browser-note-element" title={element.selector}>
                {elementLabel(element)}
              </span>
              <button
                type="button"
                className="g-button g-button-ghost g-button-icon g-button-24"
                aria-label={`Take ${elementLabel(element)} off the note`}
                onClick={() => onRemove?.(index)}
              >
                <X size={12} />
              </button>
            </li>
          ))}
        </ol>
      ) : null}
      <textarea
        ref={field}
        className="browser-note-field"
        aria-label={`Note to ${agentName}`}
        placeholder={
          placeholder ?? (area ? "What should change here?" : "What should change about this?")
        }
        value={note}
        disabled={sending}
        onChange={(event) => setNote(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (!empty && !sending) onSend(note);
          }
        }}
      />
      {error ? (
        <p className="browser-note-error" role="alert">
          {error}
        </p>
      ) : images === false ? (
        <p className="browser-note-hint">
          {agentName} cannot see images, so the note goes without the screenshot.
        </p>
      ) : null}
      <div className="browser-note-actions">
        {controls ? <div className="browser-note-pickers">{controls}</div> : null}
        <button
          type="button"
          className="g-button g-button-ghost g-button-28 browser-note-add"
          disabled={sending}
          title="Put it in the message box instead, to send with other notes"
          onClick={() => onAdd(note)}
        >
          <MessageSquarePlus size={14} />
          Add to chat
        </button>
        <button
          type="button"
          className="g-button g-button-primary g-button-icon g-button-28 browser-note-send"
          aria-label={`Send to ${agentName}`}
          title={`Send to ${agentName} (Enter)`}
          disabled={empty || sending}
          onClick={() => onSend(note)}
        >
          <ArrowUp size={16} />
        </button>
      </div>
    </div>
  );
}
