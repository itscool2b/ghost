import { prefersReducedMotion } from "@ghost/ui";
import { ChevronRight } from "lucide-react";
import { useContext, useEffect, useState } from "react";
import type { ThoughtItem } from "../../lib/chat/transcript";
import { Answer } from "../Markdown";
import { useDisclosure } from "./DisclosureMemory";
import { RevealContext } from "./VirtualRows";

/** Each a little longer than its glide, as a call's well (--g-dur-glide 240ms, --g-dur-panel 180ms). */
const OPEN_MS = 300;
const CLOSE_MS = 240;

/** What the live preview keeps of the reasoning: its last few lines are all it shows. */
const PREVIEW_CHARS = 640;

/**
 * The agent's reasoning, as one row of its turn (owner, 19 November 2026: make "it thinking, tool
 * calling, etc." sexier). While the agent thinks, the row reads "Thinking" with a sheen passing
 * over it, and the reasoning's newest lines stream in under it, fading out at the top. Once done
 * it folds to "Thought for 4 s"; it opens to the whole reasoning, set as prose a step quieter than
 * the agent's answer. A thought without words (an agent that keeps its reasoning to itself) says
 * how long it took and does not open.
 */
export function ThoughtRow({
  thought,
  thinking,
}: {
  readonly thought: ThoughtItem;
  readonly thinking: boolean;
}) {
  const words = thought.text.trim();
  const reveal = useContext(RevealContext);
  const [expanded, setExpanded] = useDisclosure(thought.key, "thought", false);
  const [moving, setMoving] = useState<"open" | "close" | null>(null);
  useEffect(() => {
    if (moving === null) return;
    const timer = setTimeout(() => setMoving(null), moving === "open" ? OPEN_MS : CLOSE_MS);
    return () => clearTimeout(timer);
  }, [moving]);
  const open = words !== "" && (expanded || moving === "close");
  const label = thinking
    ? "Thinking"
    : thought.durationMs !== null && thought.durationMs >= 1000
      ? `Thought for ${thoughtTime(thought.durationMs)}`
      : "Thought";
  const toggle = () => {
    setExpanded(!expanded);
    if (prefersReducedMotion()) return;
    setMoving(expanded ? "close" : "open");
    reveal?.hold(expanded ? CLOSE_MS : OPEN_MS);
  };
  const head = (
    <>
      <span className="chat-gutter">
        <ThoughtMark thinking={thinking} />
      </span>
      <span className="chat-thought-label" data-sheen={thinking ? "" : undefined}>
        {label}
      </span>
      {words ? <ChevronRight className="chat-tool-chevron" size={12} aria-hidden="true" /> : null}
    </>
  );
  return (
    <div
      className="chat-thought"
      data-testid="item"
      data-test-kind="thought"
      data-test-completed={String(thought.completed)}
      data-thinking={thinking ? "" : undefined}
    >
      {words ? (
        <details
          data-reveal={moving ?? undefined}
          open={open}
          onToggle={(event) => {
            // The page itself may open a closed thought (find in page); follow it.
            if (moving === null && event.currentTarget.open !== open)
              setExpanded(event.currentTarget.open);
          }}
        >
          {/* biome-ignore lint/a11y/noStaticElementInteractions: a summary is its details element's own button (Enter and Space click it); the click is taken over only so the reasoning can glide shut before the details close. */}
          <summary
            className="chat-tool-head chat-thought-head"
            data-testid="thought-toggle"
            onClick={(event) => {
              event.preventDefault();
              toggle();
            }}
          >
            {head}
          </summary>
          <div
            className="chat-reveal"
            onAnimationEnd={(event) => {
              if (event.target === event.currentTarget) setMoving(null);
            }}
          >
            <div className="chat-reveal-clip">
              <div className="chat-thought-body" data-testid="thought-body">
                <Answer text={words} />
              </div>
            </div>
          </div>
        </details>
      ) : (
        <div className="chat-tool-head chat-thought-head" data-static="">
          {head}
        </div>
      )}
      {thinking && words && !open ? (
        // The newest lines as they come; screen readers hear the row's word, not the stream.
        <div className="chat-thought-preview" aria-hidden="true" data-testid="thought-preview">
          <p>{words.length > PREVIEW_CHARS ? words.slice(-PREVIEW_CHARS) : words}</p>
        </div>
      ) : null}
    </div>
  );
}

/** How long a thought took, in whole seconds: "4 s", "1 min 12 s". */
export function thoughtTime(ms: number): string {
  const seconds = Math.max(1, Math.round(ms / 1000));
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/**
 * The reasoning's mark in the gutter: three dots in a row, the middle one raised, that ripple in
 * turn while the agent thinks and hold still once it is done. Drawn in the row's own colour.
 */
function ThoughtMark({ thinking }: { readonly thinking: boolean }) {
  return (
    <svg
      className="chat-thought-mark"
      data-thinking={thinking ? "" : undefined}
      width="14"
      height="14"
      viewBox="0 0 14 14"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx="3" cy="8" r="1.5" />
      <circle cx="7" cy="6" r="1.5" />
      <circle cx="11" cy="8" r="1.5" />
    </svg>
  );
}
