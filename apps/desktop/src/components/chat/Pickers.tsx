import { AgentMark } from "@ghost/ui/graphite";
import { Check, ChevronDown, Search } from "lucide-react";
import { Fragment, type ReactNode, useEffect, useId, useRef, useState } from "react";
import type {
  AgentEntry,
  Autonomy,
  ConfigChoice,
  ConfigOption,
  ConfigValue,
  SessionControlChange,
  SessionLists,
  SessionStatus,
} from "../../bindings";
import { agentLineup } from "../../lib/chat/agentLineup";
import { levelName, levelShortName, orderedOptions, pendingLabel } from "../../lib/chat/present";
import { agentVersion } from "../../lib/format";
import { useMediaQuery } from "../../lib/hooks";
import { Select } from "../Select";
import { AccessPicker } from "./AccessPicker";
import { focusPickerChoice, movePickerFocus, PickerChoices } from "./PickerChoices";
import { closePopover, Popover } from "./Popover";

/**
 * The autonomy level as three square ticks, 4, 6 and 8px tall (6.11): Ask fills one, Auto-edit
 * two, Full auto three. Decorative; the words beside it, or the control's name, say the level.
 */
export function AutonomyGlyph({ autonomy }: { readonly autonomy: Autonomy }) {
  const level = autonomy === "full_auto" ? 3 : autonomy === "auto_edit" ? 2 : 1;
  return (
    <svg
      className="chat-autonomy-glyph"
      width="10"
      height="8"
      viewBox="0 0 10 8"
      fill="currentColor"
      shapeRendering="crispEdges"
      aria-hidden="true"
      focusable="false"
    >
      {[4, 6, 8].map((height, index) => (
        <rect
          key={height}
          x={index * 4}
          y={8 - height}
          width="2"
          height={height}
          data-on={index < level}
        />
      ))}
    </svg>
  );
}

const AGENT_KEYS: Readonly<Record<string, string>> = {
  claude: "c",
  codex: "x",
  "grok-build": "g",
  cursor: "u",
  pi: "p",
  "oh-my-pi": "m",
  opencode: "o",
  hermes: "h",
  antigravity: "a",
};

/**
 * Where option changes go: to the running agent, to its next start (it is stopped, failed or
 * signed out), or nowhere while it starts or stops.
 */
export function optionsTarget(status: SessionStatus): "agent" | "next-start" | "none" {
  switch (status) {
    case "ready":
    case "running":
    case "cancelling":
      return "agent";
    case "stopped":
    case "failed":
    case "auth_required":
      return "next-start";
    case "starting":
    case "stopping":
      return "none";
  }
}

/** The name of the choice an option shows: the one on its way, else its current one. */
function shownChoice(option: ConfigOption, saving: ConfigValue | null) {
  if (option.kind.type !== "select") return null;
  const value = saving?.type === "choice" ? saving.value : option.kind.current;
  return option.kind.choices.find((choice) => choice.value === value) ?? null;
}

/**
 * The model the pill names. An alias such as Claude Code's "Default (recommended)" says in its
 * description which model it stands for ("Opus 5.5 · Best for everyday, complex tasks"); when that
 * name is another model the agent lists, the pill shows it. Otherwise the choice's own name.
 */
export function shownModelName(option: ConfigOption, choice: ConfigChoice): string {
  if (option.kind.type !== "select") return choice.name;
  const named = /^(.+?) · /.exec(choice.description ?? "")?.[1]?.trim();
  if (!named || named === choice.name) return choice.name;
  return option.kind.choices.some((other) => other.value !== choice.value && other.name === named)
    ? named
    : choice.name;
}

/** Models listed before "More models": agents list their newest first. */
const MODEL_ROWS = 5;

/** The choices a long model list shows folded: the first few, and the chosen one wherever it is. */
export function foldedModels<T extends { readonly value: string }>(
  choices: readonly T[],
  chosen: string,
  rows = MODEL_ROWS,
): readonly T[] {
  // One more row than the fold is no fold: it would only trade a model for "More models".
  if (choices.length <= rows + 1) return choices;
  return choices.filter((choice, index) => index < rows || choice.value === chosen);
}

/**
 * The agent, model and thinking picker, beside the access and native workflow controls. Live
 * descriptors determine the choices; pending changes keep the requested value visible. While the
 * agent is not running, its last known models and levels stay choosable for its next start.
 *
 * The menu reads top down (owner, 4 November 2026: "model and thinking mode then harnesses"):
 * search, the model list (the first few and the chosen one, then "More models"; it scrolls on its
 * own once opened), the thinking level, then every agent in a grid that is always in view, More
 * options, and the chat's Plan and build. The pill names the model an alias stands for and a chosen thinking level.
 *
 * Keys: Ctrl or Cmd + . opens it on the current model; up and down move within a part, left and
 * right move between parts, Enter chooses, digits set the thinking level, letters jump to agents.
 */
export function Pickers({
  lists,
  autonomy,
  status,
  error,
  optionError = null,
  optionsSaving = new Map(),
  autonomySaving = null,
  controlSaving = null,
  transitionBlocked = false,
  onSetAutonomy,
  onSetOption,
  agent = null,
  agents,
  onChooseAgent,
  onSetControl,
  account = null,
  roles = null,
  compact = false,
}: {
  readonly lists: SessionLists | null;
  /** The chat's autonomy until the lists arrive. */
  readonly autonomy: Autonomy;
  readonly status: SessionStatus;
  /** Why the lists, access or workflow could not load or change. */
  readonly error: string | null;
  /** Why a model, thinking level or other option change was refused. */
  readonly optionError?: string | null;
  /** Option changes on their way, by option id. */
  readonly optionsSaving?: ReadonlyMap<string, ConfigValue>;
  readonly autonomySaving?: Autonomy | null;
  readonly controlSaving?: SessionControlChange | null;
  readonly transitionBlocked?: boolean;
  readonly onSetAutonomy: (autonomy: Autonomy) => void;
  readonly onSetOption: (optionId: string, value: ConfigValue) => void;
  readonly agent?: AgentEntry | null;
  readonly agents?: readonly AgentEntry[] | null;
  readonly onChooseAgent?: (id: string) => void;
  readonly onSetControl?: (change: SessionControlChange) => void;
  /** The account picker, for an agent that takes added accounts. */
  readonly account?: ReactNode;
  /** The chat's Plan and build part (RolePicker), the menu's last. */
  readonly roles?: ReactNode;
  /**
   * Only the agent, model and thinking picker, without its shortcut: a second picker for the same
   * chat (Annotate's note), beside the composer's own.
   */
  readonly compact?: boolean;
}) {
  const id = useId();
  const [query, setQuery] = useState("");
  // A long model list folds after its first few; "More models" opens the rest until it closes.
  const [allModels, setAllModels] = useState(false);
  const columns = useRef<HTMLDivElement>(null);
  useWheelChain(columns);
  const target = optionsTarget(status);
  const choicesEnabled = target !== "none";
  // Mode changes wait for an idle agent, as the session requires.
  const modeEnabled = status === "ready" && !transitionBlocked;
  const options = orderedOptions(lists?.options ?? []).filter(
    (option) => !["aura.access", "aura.workflow"].includes(option.id),
  );
  const model = options.find((option) => option.category === "model");
  const effort = options.find((option) => option.category === "effort");
  const saving = (option: ConfigOption | undefined) =>
    option ? (optionsSaving.get(option.id) ?? null) : null;
  const modelChoice = model ? shownChoice(model, saving(model)) : null;
  const modelName = model && modelChoice ? shownModelName(model, modelChoice) : null;
  const modelLabel = modelName ?? agent?.name ?? "Model";
  const levelChoice = effort ? shownChoice(effort, saving(effort)) : null;
  const level = levelChoice ? levelName(levelChoice) : null;
  // The pill names a level only when one was chosen: "Opus 5.5 · Max", never "· Default".
  const pillLevel = level && levelChoice?.value.toLowerCase() !== "default" ? level : null;
  // Narrow windows show the level shortened (chat.css); the name says what is shown, so speech
  // control finds the pill by its visible words.
  const narrow = useMediaQuery("(max-width: 1100px)");
  const namedLevel = pillLevel && narrow ? levelShortName(pillLevel) : level;
  const others = options.filter(
    (option) => option.category !== "model" && option.category !== "effort",
  );
  // The agents worth offering: one per product, plus the chat's own agent wherever it stands.
  const offered = agents?.length
    ? [
        ...(agent && !agentLineup(agents).products.some((entry) => entry.id === agent.id)
          ? [agent]
          : []),
        ...agentLineup(agents).products,
      ]
    : [];
  const matchingAgents = offered.filter((entry) =>
    entry.name.toLowerCase().includes(query.trim().toLowerCase()),
  );
  const note =
    target === "none"
      ? status === "starting"
        ? "Changes open once the agent has started."
        : "Changes open once the agent has stopped."
      : target === "next-start" && (model || effort || others.length)
        ? "Applies when the agent starts."
        : null;
  const thinkingEmpty =
    target === "agent"
      ? model
        ? "This model has no thinking levels."
        : "Not offered by this agent."
      : model
        ? "Shown once the agent runs this model."
        : "Shown once the agent runs.";
  const modelEmpty =
    target === "agent"
      ? "This agent does not offer a model selector."
      : "Models show once this agent has run.";
  const shortcutKey =
    typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent) ? "⌘" : "Ctrl";
  return (
    <div className={compact ? "chat-pickers chat-pickers-compact" : "chat-pickers"}>
      {compact ? null : (
        <AccessPicker
          lists={lists}
          autonomy={autonomy}
          saving={autonomySaving}
          controlSaving={controlSaving}
          transitionBlocked={transitionBlocked}
          status={status}
          error={error}
          onSetAutonomy={onSetAutonomy}
          onSetControl={onSetControl}
        />
      )}
      {/* Who answers, on which account and model: one group at the row's end, beside Send. */}
      <div className="chat-pickers-end">
        {compact ? null : account}
        <Popover
          label="Agent, model and thinking"
          triggerLabel={`${[agent?.name, modelLabel, namedLevel ? `${namedLevel} thinking` : null]
            .filter(Boolean)
            .join(", ")}. Agent, model and thinking`}
          tip={
            compact ? "Agent, model and thinking" : `Agent, model and thinking (${shortcutKey} .)`
          }
          shortcut={compact ? undefined : "."}
          className="chat-model-popover"
          onOpenChange={(open) => {
            if (!open) {
              setQuery("");
              setAllModels(false);
            }
          }}
          trigger={
            <>
              {agent ? (
                <span aria-hidden="true">
                  <AgentMark agent={agent.id} size={14} />
                </span>
              ) : null}
              <strong className="chat-pill-model" data-testid="picker-current-model">
                {modelLabel}
              </strong>
              {pillLevel ? (
                <span className="chat-pill-effort" data-testid="picker-pill-thinking">
                  <span aria-hidden="true">·</span>
                  <span className="chat-pill-level">{pillLevel}</span>
                  <span className="chat-pill-level-short">{levelShortName(pillLevel)}</span>
                </span>
              ) : null}
              <ChevronDown size={12} />
            </>
          }
        >
          {/* biome-ignore lint/a11y/useSemanticElements: a fieldset's anonymous content box gives its flex children no definite height, so the model list could not scroll on its own below the thinking level. */}
          <div
            ref={columns}
            role="group"
            className="chat-model-columns"
            aria-label="Agent, model and thinking"
            onKeyDown={(event) => {
              if (
                event.ctrlKey ||
                event.metaKey ||
                event.altKey ||
                event.defaultPrevented ||
                event.target instanceof HTMLSelectElement ||
                event.target instanceof HTMLInputElement ||
                (event.target instanceof HTMLElement &&
                  event.target.getAttribute("role") === "combobox")
              )
                return;
              const parts = [
                ...event.currentTarget.querySelectorAll<HTMLElement>("[data-picker-part]"),
              ].filter((part) => part.querySelector("button:not(:disabled)"));
              const part = (event.target as HTMLElement).closest<HTMLElement>("[data-picker-part]");
              const index = part ? parts.indexOf(part) : 0;
              if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
                if (!parts.length) return;
                event.preventDefault();
                const next =
                  parts[
                    (index + (event.key === "ArrowRight" ? 1 : -1) + parts.length) % parts.length
                  ];
                focusPickerChoice(next);
              } else if (
                /^[1-9]$/.test(event.key) &&
                effort?.kind.type === "select" &&
                choicesEnabled
              ) {
                const choice = effort.kind.choices[Number(event.key) - 1];
                if (choice) {
                  event.preventDefault();
                  onSetOption(effort.id, { type: "choice", value: choice.value });
                }
              } else if (/^[a-z]$/i.test(event.key)) {
                const row = event.currentTarget.querySelector<HTMLButtonElement>(
                  `button[data-agent-key="${event.key.toLowerCase()}"]:not(:disabled)`,
                );
                if (row) {
                  event.preventDefault();
                  row.focus();
                }
              }
            }}
          >
            {note || optionError ? (
              <div className="chat-model-notes">
                {optionError ? (
                  <p className="ui-error" role="alert" data-testid="picker-option-error">
                    {optionError}
                  </p>
                ) : null}
                {note ? (
                  <p data-testid="picker-note" data-test-target={target}>
                    {note}
                  </p>
                ) : null}
              </div>
            ) : null}
            <label className="chat-picker-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="search"
                className="g-input"
                aria-label="Search agents and models"
                placeholder="Search models and agents…"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
              />
            </label>
            <section
              className="chat-picker-models"
              data-picker-part="model"
              data-picker-initial=""
              aria-labelledby={`${id}-model`}
            >
              <h3 id={`${id}-model`}>Model</h3>
              {!model ? <p className="chat-picker-empty">{modelEmpty}</p> : null}
              {model ? (
                <div className="chat-model-scroll" data-testid="picker-lists">
                  <OptionPicker
                    option={model}
                    saving={saving(model)}
                    enabled={choicesEnabled}
                    onSet={onSetOption}
                    query={query}
                    folded={!allModels}
                    onUnfold={() => setAllModels(true)}
                  />
                </div>
              ) : null}
            </section>
            <section
              className="chat-picker-thinking"
              data-picker-part="thinking"
              aria-labelledby={`${id}-thinking`}
            >
              <h3 id={`${id}-thinking`}>Thinking</h3>
              {effort ? (
                <OptionPicker
                  option={effort}
                  saving={saving(effort)}
                  enabled={choicesEnabled}
                  onSet={onSetOption}
                />
              ) : (
                <p className="chat-picker-empty" data-testid="picker-thinking-empty">
                  {thinkingEmpty}
                </p>
              )}
            </section>
            <section
              className="chat-picker-agents"
              data-picker-part="agent"
              aria-labelledby={`${id}-agent`}
            >
              <h3 id={`${id}-agent`}>Agent</h3>
              {offered.length && onChooseAgent ? (
                <fieldset
                  className="chat-agent-choices"
                  aria-label="Agents"
                  onKeyDown={movePickerFocus}
                >
                  {matchingAgents.map((entry) => {
                    const availability = entry.availability;
                    const ready = availability.state === "ready";
                    const detail =
                      availability.state === "ready"
                        ? availability.version === null
                          ? "Installed"
                          : agentVersion(entry.name, availability.version)
                        : availability.state.replaceAll("_", " ");
                    return (
                      <button
                        key={entry.id}
                        type="button"
                        aria-pressed={entry.id === agent?.id}
                        data-agent-key={AGENT_KEYS[entry.id]}
                        disabled={!ready}
                        title={`${entry.name}: ${detail}`}
                        onClick={(event) => {
                          if (entry.id !== agent?.id) {
                            closePopover(event.currentTarget);
                            onChooseAgent(entry.id);
                          }
                        }}
                      >
                        <AgentMark agent={entry.id} size={16} />
                        <span className="picker-choice-copy">
                          <strong>{entry.name}</strong>
                        </span>
                        {/* The version or the reason it can't run, for readers; a tooltip shows it. */}
                        <span className="sr-only">{detail}</span>
                        {entry.id === agent?.id ? <Check size={14} aria-label="Selected" /> : null}
                      </button>
                    );
                  })}
                  {!matchingAgents.length ? (
                    <p className="chat-picker-empty">No matching agents.</p>
                  ) : null}
                </fieldset>
              ) : (
                <span className="chat-current-agent">
                  <AgentMark agent={agent?.id ?? "generic"} size={16} />
                  {agent?.name ?? "Agent"}
                </span>
              )}
            </section>
            {others.length ? (
              <details className="chat-other-options">
                <summary>More options</summary>
                {others.map((option) => (
                  <OptionPicker
                    key={option.id}
                    option={option}
                    saving={saving(option)}
                    enabled={option.category === "mode" ? modeEnabled : choicesEnabled}
                    onSet={onSetOption}
                  />
                ))}
              </details>
            ) : null}
            {compact ? null : roles}
          </div>
        </Popover>
      </div>
      {error !== null || optionError !== null ? (
        <span
          className="chat-picker-error-dot"
          role="img"
          aria-label={
            optionError !== null
              ? "A change was refused. Open the agent, model and thinking picker for details."
              : "Session settings error. Open the access picker for details."
          }
        />
      ) : null}
    </div>
  );
}

/**
 * A wheel over the model and agent lists scrolls them; once they reach their end, it scrolls the
 * menu itself (on a short window the menu scrolls too), instead of stopping or scrolling the chat.
 */
function useWheelChain(root: React.RefObject<HTMLElement | null>) {
  useEffect(() => {
    const element = root.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.deltaY === 0 || !(event.target instanceof Element)) return;
      const list = event.target.closest<HTMLElement>(".chat-model-scroll");
      const panel = element.closest<HTMLElement>(".chat-popover-panel");
      if (!list || !panel || panel.scrollHeight <= panel.clientHeight + 1) return;
      const down = event.deltaY > 0;
      const atEnd = down
        ? list.scrollTop + list.clientHeight >= list.scrollHeight - 1
        : list.scrollTop <= 0;
      if (!atEnd) return;
      event.preventDefault();
      const line = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 : 1;
      panel.scrollTop += event.deltaY * line;
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [root]);
}

function OptionPicker({
  option,
  saving,
  enabled,
  onSet,
  query = "",
  folded = false,
  onUnfold,
}: {
  readonly option: ConfigOption;
  /** The value a change on its way asked for. */
  readonly saving: ConfigValue | null;
  readonly enabled: boolean;
  readonly onSet: (optionId: string, value: ConfigValue) => void;
  readonly query?: string;
  /** A model list shows its first few and the chosen one, then "More models" (not while searching). */
  readonly folded?: boolean;
  readonly onUnfold?: () => void;
}) {
  const id = useId();
  const pending = option.pending === null ? null : pendingLabel(option, option.pending);
  // Whatever the agent calls its level, the menu calls it Thinking.
  const name = option.category === "effort" ? "Thinking" : option.name;
  const label = (
    <>
      {name}
      {saving !== null ? (
        <span className="ui-muted"> (saving…)</span>
      ) : pending !== null ? (
        <span className="ui-muted"> (pending: {pending})</span>
      ) : null}
    </>
  );
  if (option.kind.type === "toggle") {
    return (
      <div className="ui-field chat-picker-toggle">
        <label htmlFor={id} title={option.description ?? undefined}>
          <input
            id={id}
            type="checkbox"
            className="g-switch"
            data-testid="picker-option"
            data-test-option-id={option.id}
            data-test-saving={String(saving !== null)}
            checked={saving?.type === "toggle" ? saving.value : option.kind.current}
            disabled={!enabled}
            onChange={(event) =>
              onSet(option.id, { type: "toggle", value: event.currentTarget.checked })
            }
          />{" "}
          {label}
        </label>
      </div>
    );
  }
  const { choices, current } = option.kind;
  if (option.category === "model" || option.category === "effort") {
    const effort = option.category === "effort";
    const matching = effort
      ? choices.map((choice) => ({ ...choice, name: levelName(choice) }))
      : choices.filter((choice) =>
          `${choice.name} ${choice.group ?? ""}`.toLowerCase().includes(query.trim().toLowerCase()),
        );
    const value = saving?.type === "choice" ? saving.value : current;
    const shown = effort || !folded || query.trim() ? matching : foldedModels(matching, value);
    const hidden = matching.length - shown.length;
    return (
      <div
        className="ui-field"
        data-testid="picker-option"
        data-test-option-id={option.id}
        data-test-saving={String(saving !== null)}
        data-test-value={value}
        data-layout={effort ? "segments" : "rows"}
      >
        {/* The section's heading names the part; this says what is on its way, to readers. */}
        <span className={pending !== null && !effort ? "picker-option-label" : "sr-only"}>
          {label}
        </span>
        <PickerChoices
          label={effort ? "Thinking" : option.name}
          choices={shown}
          value={value}
          busy={saving?.type === "choice" ? saving.value : null}
          enabled={enabled}
          effort={effort}
          lines={!effort}
          onChoose={(choice) => onSet(option.id, { type: "choice", value: choice })}
          after={
            hidden > 0 && onUnfold ? (
              <button
                type="button"
                className="chat-picker-more"
                data-testid="picker-more-models"
                onClick={(event) => {
                  // The first model it opened takes focus, so the arrow keys carry on from there.
                  const list = event.currentTarget.parentElement;
                  const first = matching.find((choice) => !shown.includes(choice))?.value;
                  onUnfold();
                  if (first === undefined) return;
                  requestAnimationFrame(() => {
                    list
                      ?.querySelector<HTMLButtonElement>(
                        `button[data-picker-value="${CSS.escape(first)}"]`,
                      )
                      ?.focus();
                  });
                }}
              >
                <span>More models</span>
                <small>
                  {hidden}
                  <span className="sr-only"> more</span>
                </small>
                <ChevronDown size={12} aria-hidden="true" />
              </button>
            ) : null
          }
        />
        {!matching.length && query ? (
          <p className="chat-picker-empty">No matching models.</p>
        ) : null}
      </div>
    );
  }
  const groups = [...new Set(choices.map((choice) => choice.group))];
  const render = (group: string | null) =>
    choices
      .filter((choice) => choice.group === group)
      .map((choice) => (
        <option key={choice.value} value={choice.value} title={choice.description ?? undefined}>
          {choice.name}
        </option>
      ));
  return (
    <div className="ui-field chat-picker-field">
      <label
        htmlFor={id}
        className="chat-picker-field-label"
        title={option.description ?? undefined}
      >
        {label}
      </label>
      <Select
        id={id}
        className="g-select"
        data-testid="picker-option"
        data-test-option-id={option.id}
        data-test-saving={String(saving !== null)}
        value={saving?.type === "choice" ? saving.value : current}
        disabled={!enabled}
        onChange={(event) => onSet(option.id, { type: "choice", value: event.currentTarget.value })}
      >
        {groups.length <= 1
          ? render(groups[0] ?? null)
          : groups.map((group) =>
              group === null ? (
                <Fragment key="">{render(null)}</Fragment>
              ) : (
                <optgroup key={group} label={group}>
                  {render(group)}
                </optgroup>
              ),
            )}
      </Select>
    </div>
  );
}
