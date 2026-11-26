import { type AgentBrandId, Orb, springs } from "@ghost/ui";
import { BookOpen, Bug, GitPullRequestArrow } from "lucide-react";
import { motion } from "motion/react";
import { type CSSProperties, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Project } from "../../kit";
import { Composer, type ComposerProps } from "./Composer";

const SUGGESTIONS = [
  { icon: Bug, label: "Fix a failing test", text: "Find out why the tests fail and fix it." },
  { icon: BookOpen, label: "Explain this codebase", text: "Give me a tour of this codebase." },
  { icon: GitPullRequestArrow, label: "Review my changes", text: "Review my uncommitted changes." },
] as const;

/** Room kept under an open picker, to the sheet's bottom edge. */
const PICKER_MARGIN = 20;

/**
 * A new chat: the one hero moment. The agent's orb breathes over its own light, a serif headline
 * asks the question, and the composer waits in the middle of the sheet with three plain starters.
 * The picker opens below the composer here, lifting the stack just enough that it fits, so the
 * orb stays in view and shows whichever agent is under the cursor.
 *
 * When the first message goes, the window flies the orb into the conversation's gutter and the
 * composer down to its dock (see App); the hero hides those two at once and fades the rest.
 */
export function Hero({
  agent,
  agentName,
  project,
  composer,
  onSuggest,
  leaving,
  onLeft,
}: {
  /** The agent the orb shows: the one under the picker's cursor, else the selected one. */
  readonly agent: AgentBrandId;
  readonly agentName: string;
  readonly project: Project | null;
  readonly composer: Omit<ComposerProps, "hero">;
  /** A starter was picked: it goes into the box, ready to edit or send. */
  readonly onSuggest: (text: string) => void;
  /** The first message went: hide the orb and composer (they fly), fade the rest. */
  readonly leaving: boolean;
  readonly onLeft: () => void;
}) {
  const heroRef = useRef<HTMLDivElement>(null);
  const [lift, setLift] = useState(0);
  const pickerOpen = composer.pickerOpen;

  // Measured in layout px (offsets ignore the stack's own motion), once the picker has mounted.
  useLayoutEffect(() => {
    const hero = heroRef.current;
    if (!pickerOpen || !hero) {
      setLift(0);
      return;
    }
    const wrap = hero.querySelector<HTMLElement>(".au-composer-wrap");
    const picker = hero.querySelector<HTMLElement>(".au-picker-pop");
    if (!wrap || !picker) return;
    const below = wrap.offsetTop + wrap.offsetHeight + 10 + picker.offsetHeight + PICKER_MARGIN;
    setLift(Math.max(0, below - hero.clientHeight));
  }, [pickerOpen]);

  // A safety net: the hero leaves even if its fade never reports back (nothing to animate).
  useEffect(() => {
    if (!leaving) return;
    const timer = window.setTimeout(onLeft, 600);
    return () => window.clearTimeout(timer);
  }, [leaving, onLeft]);

  const hidden: CSSProperties | undefined = leaving ? { visibility: "hidden" } : undefined;
  return (
    <div ref={heroRef} className="au-hero" style={leaving ? { pointerEvents: "none" } : undefined}>
      <motion.div
        className="au-hero-stack"
        initial={{ opacity: 0, y: 12 }}
        animate={leaving ? { opacity: 0, y: -lift } : { opacity: 1, y: -lift }}
        transition={leaving ? { duration: 0.18, ease: "easeOut" } : springs.gentle}
        onAnimationComplete={() => {
          if (leaving) onLeft();
        }}
      >
        <div className="au-hero-orb" data-hero-orb="" style={hidden}>
          <span className="au-hero-glow" aria-hidden="true" />
          <Orb brand={agent} state="idle" size={104} label={`${agentName} is ready`} />
        </div>
        <h1 className="au-hero-title">
          What should we <em>build</em>?
        </h1>
        <p className="au-hero-sub">
          {agentName} is ready in
          {project ? (
            <span
              className="au-hero-project"
              style={{ "--au-dot": project.color } as CSSProperties}
            >
              {project.name}
            </span>
          ) : (
            " this project"
          )}
        </p>
        <div className="au-hero-composer" style={hidden}>
          <Composer {...composer} hero />
        </div>
        <div className="au-hero-chips">
          {SUGGESTIONS.map(({ icon: Icon, label, text }) => (
            <button
              key={label}
              type="button"
              className="au-chip-btn"
              onClick={() => onSuggest(text)}
            >
              <Icon size={14} strokeWidth={1.75} />
              {label}
            </button>
          ))}
        </div>
      </motion.div>
    </div>
  );
}
