import { ChevronDown, Plus, Trash2, X } from "lucide-react";
import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { rememberedNames } from "../../lib/terminalMemory";
import {
  CLOSED_PANEL,
  clampHeight,
  PANEL_HEIGHT_MAX,
  PANEL_HEIGHT_MIN,
  PANEL_ROOM_ABOVE,
  type PanelTab,
  type TerminalPanels,
} from "../../lib/terminalPanel";
import { commandKey } from "./Keys";
import { closePopover, Popover } from "./Popover";
import "@xterm/xterm/css/xterm.css";
import "./terminal-panel.css";

/** One line for a tab that is not simply running: where it stands, never a filler caption. */
export function tabStatusLine(tab: PanelTab): string | null {
  switch (tab.status.state) {
    case "reviewing":
      return "Checking this chat's folder…";
    case "starting":
      return `Starting ${tab.title}…`;
    case "running":
      return null;
    case "failed":
      return tab.status.message;
    case "finished":
      return `${tab.title} exited${
        tab.status.exitCode === null || tab.status.exitCode === 0
          ? ""
          : ` with code ${tab.status.exitCode}`
      }. Output is not saved.`;
  }
}

/**
 * The docked terminal panel under the conversation (visual-language 6.22): quiet pill tabs on a
 * hairline, New terminal and the shell choice, the reviewed folder in mono, and the terminal flush
 * on the pane. Ctrl+` (Cmd+` on macOS) toggles it; the top edge resizes it on the 4px grid.
 */
export function TerminalPanel({
  chatId,
  panels,
  onHidden,
  review,
  onRun,
}: {
  readonly chatId: string;
  readonly panels: TerminalPanels;
  /** Starts Run again, for a Run tab that failed. */
  readonly onRun?: () => void;
  /** Focus goes back to the conversation when the panel hides from inside. */
  readonly onHidden?: () => void;
  readonly review?: ReactNode;
}) {
  const snapshot = useSyncExternalStore(panels.subscribe, panels.getSnapshot, panels.getSnapshot);
  const panel = snapshot.chats.get(chatId) ?? CLOSED_PANEL;
  const section = useRef<HTMLElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState(PANEL_HEIGHT_MAX + PANEL_ROOM_ABOVE);
  const [renaming, setRenaming] = useState<string | null>(null);
  const base = useId();
  const active = panel.active ? snapshot.tabs.get(panel.active) : undefined;
  // Run comes back only where the chat can still run it.
  const offer = panel.resume?.filter((tab) => tab.kind !== "run" || onRun);
  const runtime = panel.active && panel.open ? panels.runtime(panel.active) : undefined;
  const max = clampHeight(PANEL_HEIGHT_MAX, room - PANEL_ROOM_ABOVE);
  const height = clampHeight(panel.height, room - PANEL_ROOM_ABOVE);

  // The panel never takes the room the transcript and the composer need.
  useEffect(() => {
    const parent = section.current?.parentElement;
    if (!panel.open || !parent) return;
    const measure = () => setRoom(parent.clientHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(parent);
    return () => observer.disconnect();
  }, [panel.open]);

  // The active tab's terminal moves into the panel, and out again when another tab or chat shows.
  useEffect(() => {
    const element = viewport.current;
    if (!element || !runtime) return;
    runtime.attach(element);
    return () => runtime.detach();
  }, [runtime]);

  // The open tab stays in view when more tabs than fit scroll sideways.
  useEffect(() => {
    if (!panel.open || !panel.active) return;
    document
      .getElementById(`${base}-${panel.active}`)
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [base, panel.open, panel.active]);

  // Focus follows the person's own request (show, a new tab, a tab chosen), never a chat switch.
  const focused = useRef(panel.focus);
  useEffect(() => {
    if (!runtime || focused.current === panel.focus) return;
    focused.current = panel.focus;
    runtime.focus();
  }, [runtime, panel.focus]);

  if (!panel.open) return null;
  const hide = () => {
    panels.hide(chatId);
    onHidden?.();
  };
  const tabs = panel.tabs.flatMap((id) => {
    const tab = snapshot.tabs.get(id);
    return tab ? [tab] : [];
  });
  const status = active ? tabStatusLine(active) : null;
  const outcome =
    active?.status.state === "failed"
      ? "failed"
      : active?.status.state === "finished"
        ? "finished"
        : undefined;
  return (
    <section
      ref={section}
      className="terminal-panel"
      aria-label="Terminal"
      data-testid="terminal-panel"
      data-test-tabs={tabs.length}
      style={{ "--terminal-panel-height": `${height}px` } as CSSProperties}
      onKeyDown={(event) => {
        // xterm keeps Escape for the shell; on the panel's own controls it hides the panel, and
        // never reaches the chat's Escape (which would interrupt the agent).
        if (event.key === "Escape" && !event.defaultPrevented && renaming === null) {
          event.preventDefault();
          event.stopPropagation();
          hide();
        }
      }}
    >
      <hr
        className="terminal-panel-resizer"
        tabIndex={0}
        aria-label="Resize terminal panel"
        aria-orientation="horizontal"
        aria-valuemin={PANEL_HEIGHT_MIN}
        aria-valuemax={max}
        aria-valuenow={height}
        onKeyDown={(event) => {
          const room = section.current?.parentElement?.clientHeight ?? 0;
          const step = event.key === "ArrowUp" ? 20 : event.key === "ArrowDown" ? -20 : 0;
          if (step !== 0) {
            event.preventDefault();
            panels.setHeight(chatId, height + step, room - PANEL_ROOM_ABOVE);
          } else if (event.key === "Home" || event.key === "End") {
            event.preventDefault();
            panels.setHeight(
              chatId,
              event.key === "Home" ? PANEL_HEIGHT_MIN : PANEL_HEIGHT_MAX,
              room - PANEL_ROOM_ABOVE,
            );
          }
        }}
        onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
        onPointerMove={(event) => {
          if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
          const box = section.current?.getBoundingClientRect();
          const room = section.current?.parentElement?.clientHeight ?? 0;
          if (box) panels.setHeight(chatId, box.bottom - event.clientY, room - PANEL_ROOM_ABOVE);
        }}
        onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)}
      />
      <header className="terminal-panel-header">
        <div
          className="terminal-panel-tabs"
          role="tablist"
          aria-label="Terminals"
          onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
            if (renaming !== null || tabs.length === 0) return;
            const current = tabs.findIndex((tab) => tab.id === panel.active);
            const index =
              event.key === "Home"
                ? 0
                : event.key === "End"
                  ? tabs.length - 1
                  : (current + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
            const next = tabs[index];
            if (!next) return;
            event.preventDefault();
            panels.select(chatId, next.id, false);
            event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]')[index]?.focus();
          }}
        >
          {tabs.map((tab) =>
            renaming === tab.id ? (
              <input
                key={tab.id}
                className="g-input terminal-panel-rename"
                aria-label={`Rename ${tab.title}`}
                defaultValue={tab.title}
                maxLength={40}
                // biome-ignore lint/a11y/noAutofocus: The rename field replaces the tab the person just chose to rename.
                autoFocus
                onFocus={(event) => event.currentTarget.select()}
                onBlur={(event) => {
                  panels.rename(tab.id, event.currentTarget.value);
                  setRenaming(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    event.currentTarget.blur();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    event.currentTarget.value = tab.title;
                    event.currentTarget.blur();
                  }
                }}
              />
            ) : (
              <button
                key={tab.id}
                type="button"
                role="tab"
                id={`${base}-${tab.id}`}
                className="terminal-panel-tab"
                aria-selected={tab.id === panel.active}
                aria-controls={`${base}-panel`}
                tabIndex={tab.id === panel.active ? 0 : -1}
                data-state={tab.status.state}
                data-testid="terminal-panel-tab"
                title={tab.folder ? `${tab.title} in ${tab.folder}` : tab.title}
                onClick={(event) => {
                  if ((event.target as Element).closest("[data-close]")) {
                    void panels.closeTab(tab.id);
                    return;
                  }
                  panels.select(chatId, tab.id);
                }}
                onAuxClick={(event) => {
                  if (event.button === 1) void panels.closeTab(tab.id);
                }}
                onDoubleClick={() => setRenaming(tab.id)}
                onKeyDown={(event) => {
                  if (event.key === "Delete") {
                    event.preventDefault();
                    void panels.closeTab(tab.id);
                  } else if (event.key === "F2") {
                    event.preventDefault();
                    setRenaming(tab.id);
                  }
                }}
              >
                <span className="terminal-panel-tab-title">{tab.title}</span>
                {tab.status.state === "failed" ? <span className="sr-only"> (failed)</span> : null}
                {tab.status.state === "finished" ? (
                  <span className="sr-only"> (exited)</span>
                ) : null}
                <span className="terminal-panel-tab-close" data-close="" aria-hidden="true">
                  <X size={12} />
                </span>
              </button>
            ),
          )}
        </div>
        {active?.url ? (
          // Run's address opens in the browser, as links in answers do (the opener plugin takes
          // every http link with target=_blank).
          <a
            className="terminal-panel-url"
            href={active.url}
            target="_blank"
            rel="noreferrer noopener"
            title={`Open ${active.url} in the browser`}
          >
            {active.url.replace(/^http:\/\//, "")}
          </a>
        ) : null}
        <span className="terminal-panel-folder" title={active?.folder ?? undefined}>
          {active?.folder ?? ""}
        </span>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="New terminal"
          title="New terminal"
          data-testid="terminal-panel-new"
          onClick={() => void panels.newTab(chatId, active?.shell?.id)}
        >
          <Plus size={16} />
        </button>
        {panel.shells.length > 1 ? (
          <Popover
            label="New terminal with"
            tip="Choose a shell"
            className="terminal-panel-shells"
            trigger={<ChevronDown size={16} />}
          >
            {panel.shells.map((shell) => (
              <button
                key={shell.id}
                type="button"
                className="g-menu-item"
                onClick={(event) => {
                  closePopover(event.currentTarget);
                  void panels.newTab(chatId, shell.id);
                }}
              >
                {shell.label}
              </button>
            ))}
          </Popover>
        ) : null}
        {active ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label={`Close ${active.title}`}
            title={`Close ${active.title} and stop its shell`}
            onClick={() => void panels.closeTab(active.id)}
          >
            <Trash2 size={16} />
          </button>
        ) : null}
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Hide terminal"
          title={`Hide terminal (${commandKey()} \`)`}
          aria-keyshortcuts="Control+` Meta+`"
          onClick={hide}
        >
          <X size={16} />
        </button>
      </header>
      <div
        className="terminal-panel-body"
        role="tabpanel"
        id={`${base}-panel`}
        aria-labelledby={active ? `${base}-${active.id}` : undefined}
        aria-label={active ? undefined : "Terminal"}
      >
        {!active && review ? (
          <div style={{ overflowY: "auto", minHeight: 0, flex: 1 }}>{review}</div>
        ) : null}
        {panel.notice ? (
          <p className="terminal-panel-status" role="alert" data-outcome="failed">
            {panel.notice}
          </p>
        ) : null}
        {offer?.length ? (
          <div className="terminal-panel-status" role="status" data-testid="terminal-resume">
            <span>
              {rememberedNames(offer)} {offer.length === 1 ? "was" : "were"} open when Ghost last
              closed. Output from then was not kept.
            </span>
            <button
              type="button"
              className="g-button g-button-24"
              onClick={() => void panels.resume(chatId, onRun)}
            >
              Resume
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-24"
              onClick={() => panels.dismissResume(chatId)}
            >
              Not now
            </button>
          </div>
        ) : null}
        {status !== null && active ? (
          <div
            className="terminal-panel-status"
            role={outcome === "failed" ? "alert" : "status"}
            data-outcome={outcome}
          >
            <span>{status}</span>
            {outcome === "failed" ? (
              <button
                type="button"
                className="g-button g-button-24"
                onClick={() => {
                  if (active.purpose === "setup") {
                    panels.showReview(chatId);
                    return;
                  }
                  if (active.purpose === "run") {
                    void panels.closeTab(active.id);
                    onRun?.();
                    return;
                  }
                  const shell = active.shell?.id;
                  void panels.closeTab(active.id);
                  void panels.newTab(chatId, shell);
                }}
              >
                {active.purpose === "setup" ? "Retry setup" : "Try again"}
              </button>
            ) : null}
          </div>
        ) : null}
        <div
          ref={viewport}
          className="terminal-panel-viewport"
          data-testid="terminal-panel-view"
          hidden={!active && Boolean(review)}
        />
      </div>
    </section>
  );
}
