import { PixelLoader } from "@ghost/ui/graphite";
import { ChevronDown, Folder, GitBranch } from "lucide-react";
import { type CSSProperties, useEffect, useId, useRef, useState } from "react";
import {
  startingBranch,
  startsAtCurrent,
  type WorktreeChoiceView,
  type WorktreeField,
  type WorktreeOptions,
} from "../../lib/chat/worktreeChoice";
import { Select } from "../Select";
import { Popover } from "./Popover";

/** The words a chip shows, and its accessible name: the same words, then what it opens. */
export function chipText(view: WorktreeChoiceView): {
  readonly mode: string;
  readonly from: string | null;
  readonly branch: string | null;
  readonly name: string;
} {
  const local = view.options.mode === "local";
  const current = view.plan?.source.branch ?? null;
  const from = local ? current : startingBranch(view);
  const branch = local ? null : (view.options.branch ?? view.plan?.branch ?? null);
  const mode = local ? "Local" : "Worktree";
  const words = local
    ? [mode, from ? `on ${from}` : null]
    : [mode, from ? `from ${from}` : null, branch ? `on ${branch}` : null];
  return {
    mode,
    from,
    branch,
    name: `${words.filter(Boolean).join(" ")}. Where this chat works`,
  };
}

/** One line for the saved setup, from its recipe. */
export function setupLine(view: WorktreeChoiceView): string | null {
  if (!view.setupSaved) return null;
  if (!view.options.runSetup) return "Skipped for this chat. Run it later from the Git panel.";
  const setup = view.plan?.setup;
  if (!setup) return "Runs after the worktree is created.";
  const files = setup.files.length
    ? `Copies ${setup.files.length} ${setup.files.length === 1 ? "file" : "files"}`
    : null;
  const command = setup.command
    ? `runs ${[setup.command.program, ...setup.command.args].join(" ")}`
    : null;
  const line = [files, command].filter(Boolean).join(", then ");
  return `${line.charAt(0).toUpperCase()}${line.slice(1)}.`;
}

/** Where the popover opens: below the chip while there is room, otherwise above it. */
function placement(element: HTMLElement | null): {
  side: "top" | "bottom";
  room: number;
} {
  if (!element) return { side: "bottom", room: 480 };
  const chip = element.getBoundingClientRect();
  const frame = element.closest(".new-chat")?.getBoundingClientRect();
  const top = Math.max(0, frame?.top ?? 0);
  const bottom = Math.min(window.innerHeight, frame?.bottom ?? window.innerHeight);
  // 8px from the chip and 16px from the window's edge, as every popover keeps.
  const below = Math.floor(bottom - chip.bottom - 24);
  const above = Math.floor(chip.top - top - 24);
  return below >= 320 || below >= above
    ? { side: "bottom", room: Math.max(160, below) }
    : { side: "top", room: Math.max(160, above) };
}

/**
 * The one chip row under the New chat card (Codex's per-thread Local or Worktree, after Grok's
 * composer chips): where the chat will work, from which branch, on which new branch. The defaults
 * need no click. The chip opens a popover in the picker's shape to switch to Local, start from
 * another local branch, name the branch, and bring uncommitted changes or skip the saved setup.
 * Every change is reviewed by Rust; a refused one goes back, with its reason beside its control.
 */
export function WorktreeChip({
  view,
  onPropose,
  onClearError,
  busyChats = 0,
  onStopBusy,
  openRef,
}: {
  readonly view: WorktreeChoiceView;
  readonly onPropose: (change: Partial<WorktreeOptions>, field: WorktreeField) => void;
  readonly onClearError: () => void;
  /** Chats working in the project folder: copying its files needs them stopped first. */
  readonly busyChats?: number;
  /** Stops those chats, so the copy can go; nothing waits for them on its own. */
  readonly onStopBusy?: () => void;
  /** Lets New chat open the popover when Create stops on a refused change. */
  readonly openRef?: { current: (() => void) | null };
}) {
  const id = useId();
  const row = useRef<HTMLDivElement>(null);
  const [side, setSide] = useState<{ side: "top" | "bottom"; room: number }>({
    side: "bottom",
    room: 480,
  });
  const typed = view.options.branch ?? "";
  const [draft, setDraft] = useState(typed);
  // A name Rust accepted (or a new folder's default) replaces the field; a refused one stays to fix.
  useEffect(() => {
    setDraft(typed);
  }, [typed]);
  const refused = view.error?.field === "branch";
  useEffect(() => {
    if (!openRef) return;
    openRef.current = () => {
      const details = row.current?.querySelector("details");
      if (!details) return;
      setSide(placement(details));
      details.open = true;
    };
    return () => {
      openRef.current = null;
    };
  }, [openRef]);

  if (view.kind === "none" || view.kind === "plain") return null;
  if (view.kind === "checking")
    return (
      <div className="new-chat-worktree" data-testid="new-chat-worktree" data-test-mode="checking">
        <span className="new-chat-worktree-checking" role="status">
          <PixelLoader size={12} tempo="thinking" />
          Checking Git
        </span>
      </div>
    );

  const text = chipText(view);
  const local = view.options.mode === "local";
  const current = view.plan?.source.branch ?? null;
  const branches = view.plan?.local_branches ?? [];
  const start = startingBranch(view);
  const atCurrent = startsAtCurrent(view, view.options.startBranch);
  // Copying working files while an agent edits them could catch a half-made change.
  const copyBlocked = atCurrent && view.options.bringChanges && busyChats > 0;
  const setup = setupLine(view);
  const error = (field: WorktreeField) =>
    view.error?.field === field ? (
      <p className="new-chat-worktree-error" role="alert" id={`${id}-${field}-error`}>
        {view.error.message}
      </p>
    ) : null;
  const applyBranch = () => {
    const name = draft.trim();
    if ((name || null) === view.options.branch && view.error?.field !== "branch") return;
    onPropose({ branch: name || null }, "branch");
  };
  const disabled = view.creating;
  return (
    <div
      ref={row}
      className="new-chat-worktree"
      data-testid="new-chat-worktree"
      data-test-mode={view.options.mode}
      data-test-branch={text.branch ?? ""}
      data-test-start={text.from ?? ""}
      data-side={side.side}
      style={{ "--new-chat-worktree-room": `${side.room}px` } as CSSProperties}
    >
      <Popover
        label="Where this chat works"
        triggerLabel={text.name}
        tip={text.name.replace(/\. Where this chat works$/, "")}
        className="new-chat-worktree-popover"
        onOpenChange={(open) => {
          if (open) setSide(placement(row.current?.querySelector("details") ?? null));
          // A name typed but never applied is dropped on close; a refused one stays to be fixed.
          else if (!refused) setDraft(typed);
        }}
        trigger={
          // The spaces keep the chip's words apart in its text, which its name begins with; the
          // flex gap lays them out.
          <>
            {local ? (
              <Folder size={16} aria-hidden="true" />
            ) : (
              <GitBranch size={16} aria-hidden="true" />
            )}
            <strong className="new-chat-worktree-mode">{text.mode}</strong>
            {text.from ? (
              <>
                {" "}
                <span className="new-chat-worktree-word">{local ? "on" : "from"}</span>{" "}
                <code className="new-chat-branch">{text.from}</code>
              </>
            ) : null}
            {text.branch ? (
              // The new branch's name: in the chip's name and its tooltip, and drawn where the
              // line under the card has room for it (new-chat.css).
              <span className="new-chat-worktree-new">
                {" "}
                <span className="new-chat-worktree-word">on</span>{" "}
                <code className="new-chat-branch">{text.branch}</code>
              </span>
            ) : null}
            {view.error ? <span className="new-chat-worktree-flag" aria-hidden="true" /> : null}
            <ChevronDown size={12} aria-hidden="true" />
          </>
        }
      >
        <div className="new-chat-worktree-panel">
          <section aria-labelledby={`${id}-where`}>
            <h2 id={`${id}-where`} className="sr-only">
              Where it works
            </h2>
            <fieldset
              className="g-segmented new-chat-worktree-modes"
              aria-label="Where it works"
              disabled={disabled}
            >
              {(["worktree", "local"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  className="g-segment"
                  aria-pressed={view.options.mode === mode}
                  data-test-mode-choice={mode}
                  onClick={() => {
                    if (view.options.mode !== mode) onPropose({ mode }, "mode");
                  }}
                >
                  {mode === "worktree" ? (
                    <GitBranch size={14} aria-hidden="true" />
                  ) : (
                    <Folder size={14} aria-hidden="true" />
                  )}
                  {mode === "worktree" ? "Worktree" : "Local"}
                </button>
              ))}
            </fieldset>
            <p className="new-chat-worktree-hint">
              {local
                ? current
                  ? `Works in the project folder, on ${current}. Other chats there share its files.`
                  : "Works in the project folder. Other chats there share its files."
                : "A new branch in its own folder. The project folder stays as it is."}
            </p>
            {view.unavailable && local ? (
              <p className="new-chat-worktree-hint">{view.unavailable}</p>
            ) : null}
            {error("mode")}
          </section>
          {local ? null : (
            <>
              <section className="new-chat-worktree-fields" aria-labelledby={`${id}-branches`}>
                <h2 id={`${id}-branches`} className="sr-only">
                  Branches
                </h2>
                <div className="new-chat-worktree-field">
                  <label htmlFor={`${id}-start`}>Start from</label>
                  <Select
                    id={`${id}-start`}
                    className="g-select"
                    value={start ?? ""}
                    disabled={disabled || branches.length === 0}
                    aria-invalid={view.error?.field === "start" ? true : undefined}
                    aria-describedby={
                      view.error?.field === "start" ? `${id}-start-error` : undefined
                    }
                    onChange={(event) => {
                      const name = event.currentTarget.value;
                      onPropose({ startBranch: name === current ? null : name }, "start");
                    }}
                  >
                    {branches.map((name) => (
                      <option key={name} value={name}>
                        {name === current ? `${name} (current)` : name}
                      </option>
                    ))}
                  </Select>
                  {error("start")}
                </div>
                <div className="new-chat-worktree-field">
                  <label htmlFor={`${id}-name`}>New branch</label>
                  <div className="new-chat-worktree-name">
                    <input
                      id={`${id}-name`}
                      className="g-input"
                      value={draft}
                      placeholder={
                        view.plan?.branch_generated ? view.plan.branch : "ghost/new-branch"
                      }
                      maxLength={200}
                      spellCheck={false}
                      autoComplete="off"
                      disabled={disabled}
                      aria-invalid={view.error?.field === "branch" ? true : undefined}
                      aria-describedby={
                        view.error?.field === "branch" ? `${id}-branch-error` : `${id}-branch-hint`
                      }
                      onChange={(event) => {
                        setDraft(event.currentTarget.value);
                        if (view.error?.field === "branch") onClearError();
                      }}
                      onKeyDown={(event) => {
                        // Enter names the branch; it never submits New chat around it.
                        if (event.key === "Enter") {
                          event.preventDefault();
                          applyBranch();
                        }
                      }}
                    />
                    <button
                      type="button"
                      className="g-button"
                      disabled={
                        disabled ||
                        ((draft.trim() || null) === view.options.branch &&
                          view.error?.field !== "branch")
                      }
                      onClick={applyBranch}
                    >
                      Apply
                    </button>
                  </div>
                  {view.error?.field === "branch" ? (
                    error("branch")
                  ) : (
                    <p className="new-chat-worktree-hint" id={`${id}-branch-hint`}>
                      {view.options.branch === null
                        ? "Named for you. Type a name to use your own."
                        : "Your name. Clear it for a generated one."}
                    </p>
                  )}
                </div>
              </section>
              <section aria-labelledby={`${id}-options`}>
                <h2 id={`${id}-options`} className="sr-only">
                  Options
                </h2>
                <label className="new-chat-worktree-switch">
                  <span className="new-chat-worktree-switch-text">
                    Bring uncommitted changes
                    <small>
                      {!atCurrent
                        ? `Only when starting from ${current ?? "the current branch"}.`
                        : copyBlocked
                          ? `${busyChats === 1 ? "A chat is" : `${busyChats} chats are`} working in this folder, so its files can still change. Stop ${busyChats === 1 ? "it" : "them"} to copy them, or create without your changes.`
                          : "Copies your working files. The project folder keeps them too."}
                    </small>
                  </span>
                  <input
                    type="checkbox"
                    role="switch"
                    aria-checked={view.options.bringChanges}
                    className="g-switch"
                    checked={view.options.bringChanges}
                    disabled={disabled || !atCurrent}
                    onChange={(event) =>
                      onPropose({ bringChanges: event.currentTarget.checked }, "bring")
                    }
                  />
                </label>
                {copyBlocked ? (
                  <div className="new-chat-actions" data-testid="new-chat-worktree-busy">
                    {onStopBusy ? (
                      <button
                        type="button"
                        className="g-button g-button-outline g-button-28"
                        disabled={disabled}
                        onClick={onStopBusy}
                      >
                        {busyChats === 1 ? "Stop this chat" : "Stop these chats"}
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="g-button g-button-outline g-button-28"
                      disabled={disabled}
                      onClick={() => onPropose({ bringChanges: false }, "bring")}
                    >
                      Create without my changes
                    </button>
                  </div>
                ) : null}
                {error("bring")}
                {setup ? (
                  <label className="new-chat-worktree-switch">
                    <span className="new-chat-worktree-switch-text">
                      Run saved setup
                      <small>{setup}</small>
                    </span>
                    <input
                      type="checkbox"
                      role="switch"
                      aria-checked={view.options.runSetup}
                      className="g-switch"
                      checked={view.options.runSetup}
                      disabled={disabled}
                      onChange={(event) =>
                        onPropose({ runSetup: event.currentTarget.checked }, "setup")
                      }
                    />
                  </label>
                ) : null}
                {error("setup")}
              </section>
              {view.plan ? (
                <section className="new-chat-worktree-facts" aria-label="What Create does">
                  <p className="new-chat-worktree-path" title={view.plan.path}>
                    {view.plan.path}
                  </p>
                  {view.plan.warnings.map((warning) => (
                    <p key={warning}>{warning}</p>
                  ))}
                </section>
              ) : null}
            </>
          )}
        </div>
      </Popover>
      {view.pending ? (
        <span className="new-chat-worktree-pending" role="status" aria-label="Reviewing">
          <PixelLoader size={12} tempo="thinking" />
        </span>
      ) : null}
      {view.unavailable && local ? (
        <p className="new-chat-worktree-note" data-testid="new-chat-worktree-reason">
          {view.unavailable}
        </p>
      ) : null}
    </div>
  );
}
