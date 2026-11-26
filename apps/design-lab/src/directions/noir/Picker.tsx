import { GlassPanel, Orb } from "@ghost/ui";
import { Check } from "lucide-react";
import { motion } from "motion/react";
import { type KeyboardEvent, useEffect, useId, useRef } from "react";
import { type AgentInfo, findAgent, findModel, type Workspace } from "../../kit";
import type { ComposerState, ComposerUpdate } from "./state";
import { AgentAction, Kbd, MiniOrb, popIn, useDismiss, useReduced } from "./ui";

/**
 * Agent > model > effort, as three columns over the composer. Choosing an agent or a model
 * applies it at once (with its defaults below), so the columns always show the live choice;
 * choosing an effort finishes and closes. Each column is a radio group: Tab moves between the
 * groups (and the Update and Install buttons), arrows move through and across them, and Enter or
 * Space chooses.
 */
export function Picker({
  workspace,
  state,
  update,
  onClose,
  fromPill,
}: {
  readonly workspace: Workspace;
  readonly state: ComposerState;
  readonly update: ComposerUpdate;
  readonly onClose: () => void;
  /** Opened from the agent pill (not by the lab's picker state): focus moves into it. */
  readonly fromPill: boolean;
}) {
  const reduced = useReduced();
  const ref = useRef<HTMLDivElement>(null);
  const ids = useId();
  const focusOnOpen = useRef(fromPill);
  useDismiss(ref, onClose);

  // Opened from the pill, focus moves to the chosen agent. Closed after the reader used it, focus
  // goes to the composer's field, where the next thing typed belongs (unless it has already moved
  // somewhere else on its own).
  useEffect(() => {
    const picker = ref.current;
    if (!picker) return;
    const field = picker
      .closest(".n-composer-wrap")
      ?.querySelector<HTMLTextAreaElement>("[data-composer-field]");
    let used = false;
    const markUsed = (): void => {
      used = true;
    };
    picker.addEventListener("pointerdown", markUsed);
    picker.addEventListener("keydown", markUsed);
    // Safari never focuses a clicked button, so the pill says it opened the picker.
    if (focusOnOpen.current) {
      used = true;
      picker.querySelector<HTMLElement>('[data-column="0"][aria-checked="true"]')?.focus();
    }
    return () => {
      picker.removeEventListener("pointerdown", markUsed);
      picker.removeEventListener("keydown", markUsed);
      const now = document.activeElement;
      if (used && (!now || now === document.body || picker.contains(now))) {
        field?.focus({ preventScroll: true });
      }
    };
  }, []);
  const agent = findAgent(workspace, state.agent);
  const model = findModel(agent, state.model);
  const motionProps = popIn(reduced);

  const chooseAgent = (next: AgentInfo): void => {
    if (!next.installed || next.id === state.agent) return;
    const nextModel = findModel(next, next.defaultModel) ?? next.models[0] ?? null;
    update({
      agent: next.id,
      model: nextModel?.id ?? "",
      effort: nextModel?.defaultEffort ?? null,
      autonomy: next.autonomy.includes(state.autonomy)
        ? state.autonomy
        : (next.autonomy[0] ?? "full_auto"),
    });
  };

  // Roving focus: arrows move within a column, left and right between columns.
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const current = document.activeElement;
    if (!(current instanceof HTMLElement) || !ref.current?.contains(current)) return;
    const column = Number(current.dataset.column ?? -1);
    const row = Number(current.dataset.row ?? -1);
    if (column < 0) return;
    const at = (c: number, r: number): HTMLElement | null =>
      ref.current?.querySelector<HTMLElement>(`[data-column="${c}"][data-row="${r}"]`) ?? null;
    const rows = (c: number): number =>
      ref.current?.querySelectorAll(`[data-column="${c}"]`).length ?? 0;
    const chosenIn = (c: number): HTMLElement | null =>
      ref.current?.querySelector<HTMLElement>(`[data-column="${c}"][aria-checked="true"]`) ??
      at(c, 0);
    let target: HTMLElement | null = null;
    if (event.key === "ArrowDown") target = at(column, Math.min(row + 1, rows(column) - 1));
    else if (event.key === "ArrowUp") target = at(column, Math.max(row - 1, 0));
    else if (event.key === "Home") target = at(column, 0);
    else if (event.key === "End") target = at(column, rows(column) - 1);
    else if (event.key === "ArrowRight") target = chosenIn(column + 1);
    else if (event.key === "ArrowLeft") target = chosenIn(column - 1);
    if (target) {
      event.preventDefault();
      event.stopPropagation();
      target.focus();
    }
  };

  return (
    <motion.div
      ref={ref}
      className="n-picker"
      role="dialog"
      aria-label="Choose agent, model and effort"
      onKeyDown={onKeyDown}
      initial={motionProps.initial}
      animate={motionProps.animate}
      exit={motionProps.exit}
      transition={motionProps.transition}
    >
      <GlassPanel elevation={3} blur squircle radius={16} className="n-overlay-panel">
        <div className="n-picker-cols">
          <div className="n-picker-col" data-col="agent">
            <p className="n-picker-label" id={`${ids}-agent`}>
              Agent
            </p>
            <div className="n-picker-group" role="radiogroup" aria-labelledby={`${ids}-agent`}>
              {workspace.agents.map((item, index) => {
                const checked = item.id === state.agent;
                const stop = checked || (!agent && index === 0);
                const action = item.installed ? (item.update ? "update" : null) : "install";
                return (
                  <div key={item.id} className="n-pick-row" data-action={action ?? undefined}>
                    {/* biome-ignore lint/a11y/useSemanticElements: manual activation: arrows only move focus here, where a native radio would choose on every press (and close the picker from the effort column). */}
                    <button
                      type="button"
                      role="radio"
                      aria-checked={checked}
                      aria-disabled={!item.installed}
                      tabIndex={stop ? 0 : -1}
                      data-column={0}
                      data-row={index}
                      className="n-pick"
                      data-missing={item.installed ? undefined : ""}
                      onClick={() => chooseAgent(item)}
                    >
                      <span className="n-pick-orb">
                        {item.installed ? (
                          <Orb brand={item.id} state="idle" size={18} />
                        ) : (
                          <MiniOrb agent={item.id} size={16} ghost />
                        )}
                      </span>
                      <span className="n-pick-text">
                        <span className="n-pick-label">{item.name}</span>
                        <span className="n-pick-sub n-num">
                          {item.installed ? item.version : "Not installed"}
                        </span>
                      </span>
                      {checked ? (
                        <Check
                          className="n-pick-check"
                          size={14}
                          strokeWidth={1.8}
                          aria-hidden="true"
                        />
                      ) : null}
                    </button>
                    {action ? <AgentAction kind={action} agent={item} /> : null}
                  </div>
                );
              })}
            </div>
          </div>
          <div className="n-picker-col" data-col="model">
            <p className="n-picker-label" id={`${ids}-model`}>
              Model
            </p>
            <div className="n-picker-group" role="radiogroup" aria-labelledby={`${ids}-model`}>
              {(agent?.models ?? []).map((item, index) => {
                const checked = item.id === state.model;
                const stop = checked || (!model && index === 0);
                return (
                  // biome-ignore lint/a11y/useSemanticElements: manual activation: arrows only move focus here, where a native radio would choose on every press (and close the picker from the effort column).
                  <button
                    key={item.id}
                    type="button"
                    role="radio"
                    aria-checked={checked}
                    tabIndex={stop ? 0 : -1}
                    data-column={1}
                    data-row={index}
                    className="n-pick"
                    onClick={() => {
                      const keeps = item.efforts.some((option) => option.id === state.effort);
                      update({ model: item.id, effort: keeps ? state.effort : item.defaultEffort });
                    }}
                  >
                    <span className="n-pick-text">
                      <span className="n-pick-label">{item.label}</span>
                      <span className="n-pick-sub">{item.description}</span>
                    </span>
                    {checked ? (
                      <Check
                        className="n-pick-check"
                        size={14}
                        strokeWidth={1.8}
                        aria-hidden="true"
                      />
                    ) : null}
                  </button>
                );
              })}
            </div>
          </div>
          <div className="n-picker-col" data-col="effort">
            <p className="n-picker-label" id={`${ids}-effort`}>
              Effort
            </p>
            {model && model.efforts.length > 0 ? (
              <div className="n-picker-group" role="radiogroup" aria-labelledby={`${ids}-effort`}>
                {model.efforts.map((item, index) => {
                  const checked = item.id === state.effort;
                  const stop =
                    checked ||
                    (index === 0 && !model.efforts.some((option) => option.id === state.effort));
                  return (
                    // biome-ignore lint/a11y/useSemanticElements: manual activation: arrows only move focus here, where a native radio would choose on every press (and close the picker from the effort column).
                    <button
                      key={item.id}
                      type="button"
                      role="radio"
                      aria-checked={checked}
                      tabIndex={stop ? 0 : -1}
                      data-column={2}
                      data-row={index}
                      className="n-pick"
                      onClick={() => update({ effort: item.id, pickerOpen: false })}
                    >
                      <span className="n-pick-text">
                        <span className="n-pick-label">{item.label}</span>
                      </span>
                      <EffortMeter level={index} of={model.efforts.length} />
                      {checked ? (
                        <Check
                          className="n-pick-check"
                          size={14}
                          strokeWidth={1.8}
                          aria-hidden="true"
                        />
                      ) : null}
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="n-picker-empty">This model has no effort setting.</p>
            )}
          </div>
        </div>
        <div className="n-picker-foot">
          <span className="n-picker-hint">
            <Kbd>{"↑"}</Kbd>
            <Kbd>{"↓"}</Kbd> Move
          </span>
          <span className="n-picker-hint">
            <Kbd>{"←"}</Kbd>
            <Kbd>{"→"}</Kbd> Column
          </span>
          <span className="n-picker-hint">
            <Kbd>{"⏎"}</Kbd> Choose
          </span>
          <span className="n-picker-hint">
            <Kbd>Esc</Kbd> Close
          </span>
        </div>
      </GlassPanel>
    </motion.div>
  );
}

/** A small stepped bar: how much effort a level spends, relative to the model's range. */
function EffortMeter({ level, of }: { readonly level: number; readonly of: number }) {
  return (
    <span className="n-effort" aria-hidden="true">
      {Array.from({ length: of }, (_, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: the meter's steps are positional.
        <span key={index} className="n-effort-step" data-on={index <= level ? "" : undefined} />
      ))}
    </span>
  );
}
