import { AgentMark } from "@ghost/ui/graphite";
import {
  Ellipsis,
  FileDiff,
  Gamepad2,
  Globe,
  MessageSquare,
  Play,
  Smartphone,
  Square,
  SquareTerminal,
} from "lucide-react";
import type { AgentEntry, ChatSummary } from "../../bindings";
import { chatActivity } from "../../lib/chat/activity";
import type { ChatState } from "../../lib/chat/chatStore";
import { contextLabel, costLabel, folderStateLabel, statusWord } from "../../lib/chat/present";
import { hasProcess } from "../../lib/chat/transcript";
import type { ChatInterface } from "../../lib/chatInterface";
import { agentVersion, builtInPath } from "../../lib/format";
import { inTauri } from "../../lib/ipc";
import { projectName } from "./ChatList";
import { FolderContext } from "./FolderContext";
import { commandKey, Keys } from "./Keys";
import { closePopover, Popover } from "./Popover";

/** Statuses the header says out loud; Ready and Stopped stay quiet (screen readers still hear them). */
const QUIET = new Set(["ready", "stopped"]);

/**
 * The chat's header (Grok): the title and project at the left, then the status with its live mark
 * only while it says something, and the controls at the right: Chat or Terminal, the browser beside
 * the chat, Run, the Changes panel and the chat menu. The menu holds the session facts
 * (the agent and the version that runs, the context and cost when reported) and every other action;
 * the command palette offers the same actions by name.
 */
export function Header({
  state,
  listed = null,
  project = null,
  folderLabel = null,
  agent,
  resumable,
  onStart,
  onStop,
  onShowLog,
  onChanges,
  onHistory,
  onCompact,
  onGit,
  onTools,
  onTerminal,
  terminalOpen = false,
  onExport,
  onDebug,
  changesOpen = false,
  run,
  view,
  browser,
  device,
  game,
}: {
  readonly state: ChatState;
  /**
   * The chat as the chat list has it. Its folder state is the newer one: the list reloads after
   * a start fails or gets past the folder check, while the snapshot is from the join.
   */
  readonly listed?: ChatSummary | null;
  /**
   * The project the chat belongs to. A worktree chat's folder is named after its worktree id, so
   * the folder name stands in only when the workspace does not know the project.
   */
  readonly project?: string | null;
  /** Rust's selected folder label retains the managed worktree's branch and source. */
  readonly folderLabel?: string | null;
  /** The chat's agent as detection sees it, for the version that will run. */
  readonly agent: AgentEntry | null;
  /** There is an earlier session a start would continue. */
  readonly resumable: boolean;
  readonly onStart: (fresh: boolean) => void;
  readonly onStop: () => void;
  readonly onShowLog: () => void;
  readonly onChanges?: () => void;
  /**
   * Chat or Terminal: Ghost's interface, or the agent's own in Ghost's terminal. Absent for
   * agents without a terminal; `unavailable` says why Terminal cannot be chosen now.
   */
  readonly view?: {
    readonly current: ChatInterface;
    readonly unavailable: string | null;
    readonly onChange: (next: ChatInterface) => void;
  };
  readonly onHistory?: () => void;
  /**
   * Asks the agent to summarize the conversation so far to free context (its `/compact`); offered
   * when the agent lists that command.
   */
  readonly onCompact?: () => void;
  readonly onGit?: () => void;
  readonly onTools?: () => void;
  /** Shows or hides the docked terminal panel (Ctrl+`, Cmd+` on macOS). */
  readonly onTerminal?: () => void;
  readonly terminalOpen?: boolean;
  readonly onExport?: () => void;
  readonly onDebug?: () => void;
  readonly changesOpen?: boolean;
  /**
   * The browser beside the chat (the Browser plugin), in Chat view while the plugin is on. The
   * header's opener for it, where the panel button was (owner, 9 November 2026).
   */
  readonly browser?: { readonly open: boolean; readonly onToggle: () => void };
  /** The game beside the chat (the Game plugin), in Chat view, in chats that have a game. */
  readonly game?: {
    readonly open: boolean;
    /** The agent playing the game while its pane is closed, by name. */
    readonly agent: string | null;
    readonly onToggle: () => void;
  };
  /** The device beside the chat (the Mobile plugin), in Chat view while the plugin is on. */
  readonly device?: {
    readonly open: boolean;
    /** The agent working on the device while its pane is closed, by name. */
    readonly agent?: string | null;
    readonly onToggle: () => void;
  };
  /** Run, in a worktree chat whose folder setup saved a Run command: start it, or stop it. */
  readonly run?: {
    /** "pnpm dev". */
    readonly label: string;
    readonly port: number;
    readonly running: boolean;
    readonly onRun: () => void;
    readonly onStop: () => void;
  };
}) {
  const { session } = state;
  const chat = listed ?? state.chat;
  const status = session?.status ?? chat?.status ?? "stopped";
  const live = hasProcess(status);
  const name = session?.agent_name ?? chat?.agent_name ?? agent?.name ?? "Agent";
  // Detection names the binary that actually runs (the user's own CLI behind an adapter); the
  // agent's own report is the fallback.
  const ready = agent?.availability.state === "ready" ? agent.availability : null;
  const version = ready?.version ?? session?.agent_version ?? null;
  const folderState = chat ? folderStateLabel(chat.folder_state) : null;
  const projectLabel = project ?? (chat ? projectName(chat.folder) : null);
  const location =
    folderLabel && folderLabel !== projectLabel ? `${projectLabel} · ${folderLabel}` : projectLabel;
  const context = contextLabel(state.usage);
  const cost = costLabel(state.usage);
  const activity = chatActivity(state);
  // A chat shown as its agent's terminal: the session reads as stopped while the terminal has it.
  const inTerminal = view?.current === "terminal";
  const statusLabel = inTerminal
    ? "In the terminal"
    : activity === "waiting"
      ? "Waiting on you"
      : statusWord(status);
  const quiet = activity !== "waiting" && QUIET.has(status) && !session?.detail;
  const tools = chat && inTauri() ? onTools : undefined;
  const files = chat && inTauri();
  const compact = live ? onCompact : undefined;
  const item = (run: () => void) => (event: { currentTarget: Element }) => {
    closePopover(event.currentTarget);
    run();
  };
  return (
    <header className="chat-header" data-testid="chat-header">
      <div className="chat-header-who">
        <span className="chat-header-title" title={chat?.title ?? "New chat"}>
          {chat?.title ?? "New chat"}
        </span>
        {chat ? (
          <span className="chat-header-folder" title={chat.folder}>
            {location}
            {folderState !== null ? <span className="chat-row-flag"> · {folderState}</span> : null}
          </span>
        ) : null}
        <span
          className="chat-status"
          data-status={status}
          data-activity={activity}
          data-quiet={quiet ? "" : undefined}
          data-testid="chat-status"
          data-test-status={status}
          aria-live="polite"
        >
          {/* The live mark rides with the status it shows, so the title never moves (Grok). */}
          {activity !== "idle" ? (
            <span className="chat-status-mark" aria-hidden="true">
              <AgentMark
                agent={chat?.agent_id ?? agent?.id ?? "generic"}
                state={activity}
                size={16}
                label={`${name}, ${session?.detail ?? statusLabel}`}
              />
            </span>
          ) : null}
          {statusLabel}
          {session?.detail && !inTerminal ? (
            <span className="chat-status-detail"> · {session.detail}</span>
          ) : null}
        </span>
        {state.actionError !== null ? (
          <span
            role="alert"
            className="chat-header-error"
            title={state.actionError}
            data-testid="chat-action-error"
          >
            {state.actionError}
          </span>
        ) : null}
      </div>
      <div className="chat-header-controls">
        {view ? (
          <fieldset
            className="g-segmented chat-view-switch"
            aria-label="Show this chat as"
            data-tour="chat-view"
          >
            {(["chat", "terminal"] as const).map((choice) => {
              const blocked = choice === "terminal" && view.unavailable !== null;
              return (
                <button
                  key={choice}
                  type="button"
                  className="g-segment"
                  aria-pressed={view.current === choice}
                  aria-disabled={blocked && view.current !== choice ? true : undefined}
                  data-testid={`chat-view-${choice}`}
                  title={
                    blocked && view.current !== choice
                      ? (view.unavailable ?? undefined)
                      : choice === "chat"
                        ? "Ghost's chat"
                        : "The agent's own interface, in Ghost's terminal"
                  }
                  onClick={() => {
                    if (view.current === choice || (blocked && view.current !== choice)) return;
                    view.onChange(choice);
                  }}
                >
                  {choice === "chat" ? (
                    <MessageSquare size={14} aria-hidden="true" />
                  ) : (
                    <SquareTerminal size={14} aria-hidden="true" />
                  )}
                  {choice === "chat" ? "Chat" : "Terminal"}
                </button>
              );
            })}
          </fieldset>
        ) : null}
        {browser ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28 chat-browser-toggle"
            aria-label="Browser"
            title={`Browser (${commandKey()} Shift B)`}
            aria-keyshortcuts="Control+Shift+B Meta+Shift+B"
            aria-expanded={browser.open}
            data-testid="chat-browser"
            data-tour="browser"
            onClick={(event) => {
              event.currentTarget.focus();
              browser.onToggle();
            }}
          >
            <Globe size={16} />
          </button>
        ) : null}
        {device ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28 chat-device-toggle"
            aria-label={device.agent ? `Device: ${device.agent} is using it` : "Device"}
            title={
              device.agent
                ? `${device.agent} is using the device (${commandKey()} Shift D)`
                : `Device (${commandKey()} Shift D)`
            }
            aria-keyshortcuts="Control+Shift+D Meta+Shift+D"
            aria-expanded={device.open}
            data-testid="chat-device"
            data-tour="device"
            data-agent={device.agent ? "" : undefined}
            onClick={(event) => {
              event.currentTarget.focus();
              device.onToggle();
            }}
          >
            <Smartphone size={16} />
          </button>
        ) : null}
        {game ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28 chat-game-toggle"
            aria-label={game.agent ? `Game: ${game.agent} is playing it` : "Game"}
            title={
              game.agent
                ? `${game.agent} is playing the game (${commandKey()} Shift G)`
                : `Game (${commandKey()} Shift G)`
            }
            aria-keyshortcuts="Control+Shift+G Meta+Shift+G"
            aria-expanded={game.open}
            data-testid="chat-game"
            data-tour="game"
            data-agent={game.agent ? "" : undefined}
            onClick={(event) => {
              event.currentTarget.focus();
              game.onToggle();
            }}
          >
            <Gamepad2 size={16} />
          </button>
        ) : null}
        {run ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28 chat-run"
            data-testid="chat-run"
            data-tour="run"
            data-running={run.running ? "" : undefined}
            title={
              run.running
                ? `Stop ${run.label}`
                : `Run ${run.label} in this worktree, on localhost:${run.port}`
            }
            onClick={run.running ? run.onStop : run.onRun}
          >
            {run.running ? (
              <Square size={14} aria-hidden="true" />
            ) : (
              <Play size={14} aria-hidden="true" />
            )}
            {run.running ? "Stop" : "Run"}
          </button>
        ) : null}
        {onChanges ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Changes"
            data-tour="changes"
            title={`Changes (${commandKey()} J)`}
            aria-keyshortcuts="Control+J Meta+J"
            aria-expanded={changesOpen}
            onClick={(event) => {
              event.currentTarget.focus();
              onChanges();
            }}
          >
            <FileDiff size={16} />
          </button>
        ) : null}
        <Popover
          label="Chat details and controls"
          tip="More"
          className="chat-header-menu"
          trigger={<Ellipsis size={16} data-tour="chat-more" />}
        >
          <div className="chat-session-info">
            <strong>{name}</strong>
            <span>{version === null ? "Version not reported" : agentVersion(name, version)}</span>
            {ready?.path ? <span className="ui-mono">{builtInPath(ready.path)}</span> : null}
            {context !== null || cost !== null ? (
              <span className="chat-session-usage" data-testid="chat-usage">
                {[context, cost].filter((part) => part !== null).join(" · ")}
              </span>
            ) : null}
          </div>
          {onGit || onTerminal || tools || onHistory || compact ? (
            <div className="chat-header-actions">
              {onGit ? (
                <button type="button" className="g-menu-item" onClick={item(onGit)}>
                  Git workspace
                </button>
              ) : null}
              {onTerminal ? (
                <button
                  type="button"
                  className="g-menu-item"
                  data-testid="chat-terminal-toggle"
                  aria-keyshortcuts="Control+` Meta+`"
                  onClick={item(onTerminal)}
                >
                  {terminalOpen ? "Hide terminal" : "Show terminal"}
                  <Keys keys={[commandKey(), "`"]} />
                </button>
              ) : null}
              {tools ? (
                <button type="button" className="g-menu-item" onClick={item(tools)}>
                  Open in editor
                </button>
              ) : null}
              {onHistory ? (
                <button type="button" className="g-menu-item" onClick={item(onHistory)}>
                  Conversation history
                </button>
              ) : null}
              {compact ? (
                <button
                  type="button"
                  className="g-menu-item"
                  data-testid="chat-compact"
                  title="Summarize the conversation so far to free context (/compact)"
                  onClick={item(compact)}
                >
                  Compact conversation
                </button>
              ) : null}
            </div>
          ) : null}
          <div className="chat-header-actions">
            {files && onExport ? (
              <button type="button" className="g-menu-item" onClick={item(onExport)}>
                Export chat
              </button>
            ) : null}
            {files && onDebug ? (
              <button type="button" className="g-menu-item" onClick={item(onDebug)}>
                Copy debug bundle
              </button>
            ) : null}
            <button
              type="button"
              className="g-menu-item"
              data-testid="chat-show-log"
              onClick={item(onShowLog)}
            >
              Show log
            </button>
          </div>
          <div className="chat-header-actions">
            {live ? (
              <button
                type="button"
                className="g-menu-item"
                data-testid="chat-stop-session"
                onClick={item(onStop)}
                disabled={status === "stopping"}
              >
                Stop session
              </button>
            ) : (
              <>
                <button
                  type="button"
                  className="g-menu-item"
                  data-testid="chat-start"
                  data-test-resume={String(resumable)}
                  onClick={item(() => onStart(false))}
                >
                  {resumable ? "Resume session" : "Start session"}
                </button>
                {resumable ? (
                  <button
                    type="button"
                    className="g-menu-item"
                    data-testid="chat-start-fresh"
                    onClick={item(() => onStart(true))}
                  >
                    Start fresh
                  </button>
                ) : null}
              </>
            )}
          </div>
          {chat && inTauri() ? (
            <FolderContext key={chat.id} chatId={chat.id} status={status} />
          ) : null}
        </Popover>
      </div>
    </header>
  );
}
