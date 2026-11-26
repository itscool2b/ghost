import { type AgentBrandId, Orb, springs } from "@ghost/ui";
import { ArrowUp, Check } from "lucide-react";
import { motion } from "motion/react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { type AgentInfo, type AgentModel, findAgent, findModel, type Workspace } from "../../kit";
import { Glass } from "./Glass";
import { cx, Kbd } from "./ui";

/** What the composer sends with: the picker reads and writes this. */
export interface Selection {
  readonly agent: AgentBrandId;
  readonly model: string;
  readonly effort: string | null;
}

/** The model and effort last used with each agent, so coming back to an agent restores them. */
export type SelectionMemory = ReadonlyMap<
  AgentBrandId,
  { readonly model: string; readonly effort: string | null }
>;

function defaultModel(agent: AgentInfo | null): AgentModel | null {
  if (!agent) return null;
  return findModel(agent, agent.defaultModel) ?? agent.models[0] ?? null;
}

/** A model's effort, kept when the model offers it, else the model's default. */
export function effortFor(model: AgentModel | null, wanted: string | null): string | null {
  if (!model || model.efforts.length === 0) return null;
  if (wanted !== null && model.efforts.some((effort) => effort.id === wanted)) return wanted;
  return model.defaultEffort ?? model.efforts[0]?.id ?? null;
}

/** What choosing `agent` selects: its last-used model and effort, else its defaults. */
export function recall(
  workspace: Workspace,
  memory: SelectionMemory,
  agentId: AgentBrandId,
): Selection | null {
  const agent = findAgent(workspace, agentId);
  if (!agent?.installed) return null;
  const remembered = memory.get(agentId);
  const model = findModel(agent, remembered?.model ?? null) ?? defaultModel(agent);
  if (!model) return null;
  return {
    agent: agentId,
    model: model.id,
    effort: effortFor(model, remembered ? remembered.effort : model.defaultEffort),
  };
}

type Column = 0 | 1 | 2;

/** How long the pointer rests on an agent before the light behind the window previews it. */
const HOVER_PREVIEW_MS = 250;

/**
 * Agent > model > effort, three columns in one floating glass sheet beside the composer.
 *
 * Clicks commit: an agent click selects it (with its last-used model and effort) and moves on to
 * its models, a model click selects it and moves on to its efforts, an effort click selects it and
 * closes. Escape or a click outside only closes; nothing already chosen is undone. The keyboard
 * browses without committing (the light previews the agent under the cursor) until Enter commits
 * all three. Up and down move, left and right switch columns.
 */
export function Picker({
  workspace,
  selection,
  memory,
  onCommit,
  onClose,
  onPreview,
  placement = "above",
}: {
  readonly workspace: Workspace;
  readonly selection: Selection;
  readonly memory: SelectionMemory;
  /** Selects, and leaves the picker open. */
  readonly onCommit: (next: Selection) => void;
  readonly onClose: () => void;
  readonly onPreview: (agent: AgentBrandId | null) => void;
  /** Above the composer in a chat; below it in a new chat, so the hero stays in view. */
  readonly placement?: "above" | "below";
}) {
  const [cursor, setCursor] = useState<Selection>(selection);
  const [column, setColumn] = useState<Column>(0);
  const [hoverAgent, setHoverAgent] = useState<AgentBrandId | null>(null);
  const hoverTimer = useRef(0);
  const agent = findAgent(workspace, cursor.agent);
  const model = findModel(agent, cursor.model) ?? defaultModel(agent);
  const effort = effortFor(model, cursor.effort);
  const installed = useMemo(
    () => workspace.agents.filter((candidate) => candidate.installed),
    [workspace.agents],
  );

  // The light follows the agent under the cursor, or the one the pointer rests on.
  useEffect(() => {
    const preview = hoverAgent ?? cursor.agent;
    onPreview(preview === selection.agent ? null : preview);
  }, [hoverAgent, cursor.agent, selection.agent, onPreview]);
  useEffect(
    () => () => {
      window.clearTimeout(hoverTimer.current);
      onPreview(null);
    },
    [onPreview],
  );

  const commit = useCallback(
    (next: Selection, then: Column | "close") => {
      setCursor(next);
      onCommit(next);
      if (then === "close") onClose();
      else setColumn(then);
    },
    [onCommit, onClose],
  );

  // Keyboard focus follows the cursor, so screen readers hear each option as it is reached.
  const listRefs = useRef<(HTMLDivElement | null)[]>([]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus moves whenever the cursor does.
  useLayoutEffect(() => {
    const current = listRefs.current[column]?.querySelector<HTMLElement>("[data-current]");
    current?.focus({ preventScroll: true });
  }, [column, cursor]);

  // Keys while open. Capture phase, so the lab's own arrow and space keys stay out of the way.
  const stateRef = useRef({ column, cursor, model, effort, installed, commit });
  stateRef.current = { column, cursor, model, effort, installed, commit };
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const current = stateRef.current;
      const move = (delta: number): void => {
        if (current.column === 0) {
          const index = current.installed.findIndex((item) => item.id === current.cursor.agent);
          const next =
            current.installed[
              (index + delta + current.installed.length) % current.installed.length
            ];
          const browsed = next ? recall(workspace, memory, next.id) : null;
          if (browsed) setCursor(browsed);
        } else if (current.column === 1) {
          const list = findAgent(workspace, current.cursor.agent)?.models ?? [];
          const index = list.findIndex((item) => item.id === current.model?.id);
          const next = list[(index + delta + list.length) % list.length];
          if (next) {
            setCursor({
              agent: current.cursor.agent,
              model: next.id,
              effort: effortFor(next, current.effort),
            });
          }
        } else {
          const list = current.model?.efforts ?? [];
          const index = list.findIndex((item) => item.id === current.effort);
          const next = list[(index + delta + list.length) % list.length];
          if (next) setCursor({ ...current.cursor, effort: next.id });
        }
      };
      let handled = true;
      switch (event.key) {
        case "Escape":
          onClose();
          break;
        case "ArrowDown":
          move(1);
          break;
        case "ArrowUp":
          move(-1);
          break;
        case "ArrowRight": {
          const hasEfforts = (current.model?.efforts.length ?? 0) > 0;
          setColumn((value) => (value === 0 ? 1 : value === 1 && hasEfforts ? 2 : value));
          break;
        }
        case "ArrowLeft":
          setColumn((value) => (value === 2 ? 1 : 0));
          break;
        case "Enter":
          if (current.model) {
            current.commit(
              { agent: current.cursor.agent, model: current.model.id, effort: current.effort },
              "close",
            );
          }
          break;
        case "Tab":
          // Focus stays in the picker: Tab walks the columns.
          setColumn((value) =>
            event.shiftKey
              ? ((value === 0 ? 2 : value - 1) as Column)
              : ((value === 2 ? 0 : value + 1) as Column),
          );
          break;
        default:
          handled = false;
      }
      if (handled) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, workspace, memory]);

  const below = placement === "below";
  return (
    <motion.div
      className="au-picker-pop"
      data-placement={placement}
      initial={{ opacity: 0, y: below ? -10 : 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: below ? -6 : 6, scale: 0.98, transition: { duration: 0.14 } }}
      transition={springs.snappy}
    >
      <Glass
        level={3}
        radius={16}
        frost
        className="au-picker"
        role="dialog"
        aria-label="Choose the agent, model and effort"
      >
        <div className="au-picker-cols">
          <PickerColumn
            title="Agent"
            active={column === 0}
            listRef={(node) => {
              listRefs.current[0] = node;
            }}
          >
            {workspace.agents.map((item) => {
              const current = item.id === cursor.agent;
              const chosen = item.id === selection.agent;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="option"
                  aria-selected={chosen}
                  disabled={!item.installed}
                  tabIndex={current && column === 0 ? 0 : -1}
                  className="au-option"
                  data-current={current ? "" : undefined}
                  data-cursor={current && column === 0 ? "" : undefined}
                  data-missing={item.installed ? undefined : ""}
                  onClick={() => {
                    const next = recall(workspace, memory, item.id);
                    if (next) commit(next, 1);
                  }}
                  onMouseEnter={() => {
                    window.clearTimeout(hoverTimer.current);
                    if (!item.installed) return;
                    hoverTimer.current = window.setTimeout(
                      () => setHoverAgent(item.id),
                      HOVER_PREVIEW_MS,
                    );
                  }}
                  onMouseLeave={() => {
                    window.clearTimeout(hoverTimer.current);
                    setHoverAgent(null);
                  }}
                >
                  {item.installed ? (
                    <Orb brand={item.id} state="idle" size={22} label={item.name} />
                  ) : (
                    <span className="au-ghost-orb" aria-hidden="true" />
                  )}
                  <span className="au-option-label au-option-grow">{item.name}</span>
                  {/* Only what needs doing; versions live in the agent dock's tooltips. */}
                  {item.update ? (
                    <span
                      className="au-option-trail au-update-chip"
                      title={`Update to ${item.update}`}
                    >
                      <ArrowUp size={10} strokeWidth={2.6} />
                      Update
                    </span>
                  ) : item.installed && item.signedIn === false ? (
                    <span className="au-option-trail au-install-chip">Sign in</span>
                  ) : item.installed ? null : (
                    <span className="au-option-trail au-install-chip">Install</span>
                  )}
                  <OptionCheck on={chosen} />
                </button>
              );
            })}
          </PickerColumn>
          <PickerColumn
            title="Model"
            active={column === 1}
            listRef={(node) => {
              listRefs.current[1] = node;
            }}
          >
            {(agent?.models ?? []).map((item) => {
              const current = item.id === model?.id;
              const chosen = selection.agent === cursor.agent && selection.model === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="option"
                  aria-selected={chosen}
                  tabIndex={current && column === 1 ? 0 : -1}
                  className="au-option au-option-tall"
                  data-current={current ? "" : undefined}
                  data-cursor={current && column === 1 ? "" : undefined}
                  onClick={() =>
                    commit(
                      { agent: cursor.agent, model: item.id, effort: effortFor(item, effort) },
                      item.efforts.length > 0 ? 2 : "close",
                    )
                  }
                >
                  <span className="au-option-text">
                    <span className="au-option-label">{item.label}</span>
                    <span className="au-option-sub">{item.description}</span>
                  </span>
                  <OptionCheck on={chosen} />
                </button>
              );
            })}
          </PickerColumn>
          <PickerColumn
            title="Effort"
            active={column === 2}
            listRef={(node) => {
              listRefs.current[2] = node;
            }}
            note={model && model.efforts.length > 0 ? null : "This model has no effort setting."}
          >
            {model && model.efforts.length > 0
              ? model.efforts.map((item, index) => {
                  const current = item.id === effort;
                  const chosen =
                    selection.agent === cursor.agent &&
                    selection.model === model.id &&
                    selection.effort === item.id;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="option"
                      aria-selected={chosen}
                      tabIndex={current && column === 2 ? 0 : -1}
                      className="au-option"
                      data-current={current ? "" : undefined}
                      data-cursor={current && column === 2 ? "" : undefined}
                      onClick={() =>
                        commit({ agent: cursor.agent, model: model.id, effort: item.id }, "close")
                      }
                    >
                      <span className="au-option-label au-option-grow">
                        {item.label}
                        {item.id === model.defaultEffort ? (
                          <span className="au-option-default"> · Default</span>
                        ) : null}
                      </span>
                      <span className="au-option-trail au-meter" aria-hidden="true">
                        {model.efforts.map((level, levelIndex) => (
                          <span
                            key={level.id}
                            className={cx("au-meter-bar", levelIndex <= index && "au-meter-on")}
                          />
                        ))}
                      </span>
                      <OptionCheck on={chosen} />
                    </button>
                  );
                })
              : null}
          </PickerColumn>
        </div>
        <div className="au-picker-foot">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> Move
          </span>
          <span>
            <Kbd>←</Kbd>
            <Kbd>→</Kbd> Switch
          </span>
          <span>
            <Kbd>↵</Kbd> Select
          </span>
          <span>
            <Kbd>esc</Kbd> Close
          </span>
        </div>
      </Glass>
    </motion.div>
  );
}

/** The check keeps its slot when off, so versions, chips and meters end on one edge. */
function OptionCheck({ on }: { readonly on: boolean }) {
  return (
    <Check
      className="au-option-check"
      data-off={on ? undefined : ""}
      size={14}
      strokeWidth={2.4}
      aria-hidden="true"
    />
  );
}

function PickerColumn({
  title,
  active,
  listRef,
  note = null,
  children,
}: {
  readonly title: string;
  readonly active: boolean;
  readonly listRef: (node: HTMLDivElement | null) => void;
  /** Shown instead of the list when the column has nothing to choose. */
  readonly note?: string | null;
  readonly children: ReactNode;
}) {
  return (
    <div className="au-picker-col" data-active={active ? "" : undefined}>
      <p className="au-picker-title" aria-hidden="true">
        {title}
      </p>
      {note ? (
        <p className="au-picker-empty">{note}</p>
      ) : (
        <div ref={listRef} className="au-picker-list" role="listbox" aria-label={title}>
          {children}
        </div>
      )}
    </div>
  );
}
