import { GlassPanel } from "@ghost/ui";
import { Check, ChevronDown, ChevronRight, ListChecks } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import type { PlanEntry, PlanModel } from "../../kit";
import { Expand, GLIDE, reveal, useReduced } from "./ui";

/** How far the plan sheet tucks under the composer, like paper slid behind glass. */
const TUCK = 18;

/** The checklist itself, in the dock and in the transcript alike. */
function PlanList({ entries }: { readonly entries: readonly PlanEntry[] }) {
  return (
    <ol className="n-plan-list">
      {entries.map((entry, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: plan entries are positional.
        <li key={index} className="n-plan-entry" data-status={entry.status}>
          <span className="n-plan-mark" aria-hidden="true">
            {entry.status === "completed" ? <Check size={12} strokeWidth={2} /> : null}
          </span>
          <span className="n-plan-text">{entry.content}</span>
          <span className="n-sr">
            {entry.status === "completed"
              ? "done"
              : entry.status === "in_progress"
                ? "in progress"
                : "to do"}
          </span>
        </li>
      ))}
    </ol>
  );
}

/**
 * The plan while the agent works through it, docked above the composer: a hairline in the
 * agent's colour along its top rim fills as steps finish. Once every step is done it leaves the
 * dock and becomes a step of its turn (PlanStep), so the answer that follows has the room.
 * It arrives and leaves by opening and closing its height, so the transcript above glides.
 */
export function PlanCard({ plan }: { readonly plan: PlanModel }) {
  const reduced = useReduced();
  const [open, setOpen] = useState(true);
  const progress = plan.total === 0 ? 0 : plan.completed / plan.total;
  const gone = { height: 0, opacity: 0, marginBottom: 0 };
  // The transcript above follows the dock's height, so the sheet comes and goes on the glide.
  const sheet = { ...GLIDE, duration: 0.4 };
  const transition = reduced
    ? { duration: 0 }
    : { height: sheet, marginBottom: sheet, opacity: { duration: 0.2 } };
  return (
    <motion.div
      className="n-plan-wrap"
      initial={gone}
      animate={{ height: "auto", opacity: 1, marginBottom: -TUCK }}
      exit={gone}
      transition={transition}
    >
      <GlassPanel
        elevation={1}
        squircle
        radius={16}
        className="n-plan"
        data-done={plan.done ? "" : undefined}
      >
        <button
          type="button"
          className="n-plan-head"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          <ListChecks size={14} strokeWidth={1.6} aria-hidden="true" className="n-plan-icon" />
          <span className="n-plan-title">Plan</span>
          <span className="n-plan-current">
            {plan.done ? "Complete" : (plan.current?.content ?? "")}
          </span>
          <span className="n-plan-count n-num">
            {plan.completed} of {plan.total}
          </span>
          <ChevronDown className="n-plan-chev" size={14} strokeWidth={1.6} aria-hidden="true" />
          <span className="n-plan-progress" aria-hidden="true">
            <span className="n-plan-progress-fill" style={{ transform: `scaleX(${progress})` }} />
          </span>
        </button>
        <AnimatePresence initial={false}>
          {open ? (
            <motion.div key="entries" className="n-plan-body" {...reveal(reduced)}>
              <PlanList entries={plan.entries} />
            </motion.div>
          ) : null}
        </AnimatePresence>
      </GlassPanel>
    </motion.div>
  );
}

/**
 * A plan its turn is done with, as one quiet step where the plan last changed: "Plan, 3 of 3 done",
 * opening to the checklist.
 */
export function PlanStep({ plan }: { readonly plan: PlanModel }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="n-step-block" data-kind="plan" data-done={plan.done ? "" : undefined}>
      <button
        type="button"
        className="n-step"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="n-step-glyph" aria-hidden="true">
          <ListChecks size={14} strokeWidth={1.6} />
        </span>
        <span className="n-step-label">Plan</span>
        <span className="n-step-meta n-num">
          {plan.completed} of {plan.total} done
        </span>
        <ChevronRight className="n-step-chev" size={14} strokeWidth={1.6} aria-hidden="true" />
      </button>
      <Expand open={open} follow>
        <div className="n-step-inner">
          <PlanList entries={plan.entries} />
        </div>
      </Expand>
    </div>
  );
}
