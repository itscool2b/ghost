import { useLayoutEffect, useRef, useSyncExternalStore } from "react";
import { type TerminalChats, terminalChats } from "./terminalChatRuntime";
import "./terminal-chat.css";

/**
 * A terminal chat: its agent's own interface in Ghost's terminal, in place of the transcript
 * and composer. The terminal keeps running while another chat is open; leaving the view only
 * detaches it. Once the agent exits, its output stays readable above Start again and Back to Chat.
 */
export function TerminalChat({
  chatId,
  agentName,
  runtimes = terminalChats,
  setupPending = false,
  onChat,
}: {
  readonly chatId: string;
  readonly agentName: string;
  readonly runtimes?: TerminalChats;
  /** The chat's new worktree is still setting up: the agent starts once setup succeeds. */
  readonly setupPending?: boolean;
  /** Back to Ghost's chat for this chat. */
  readonly onChat: () => void;
}) {
  const runtime = runtimes.get(chatId, agentName);
  const snapshot = useSyncExternalStore(
    runtime.subscribe,
    runtime.getSnapshot,
    runtime.getSnapshot,
  );
  const frame = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = frame.current;
    if (!element) return;
    runtime.attach(element);
    runtime.focus();
    return () => runtime.detach();
  }, [runtime]);
  const { state } = snapshot;
  const ended = state.state === "finished" || state.state === "failed";
  const endedText =
    state.state === "failed"
      ? state.message
      : state.state === "finished"
        ? `${agentName} exited${
            state.exitCode !== null && state.exitCode !== 0 ? ` with code ${state.exitCode}` : ""
          }.`
        : null;
  return (
    <section
      className="terminal-chat"
      aria-label={`${agentName} in the terminal`}
      data-testid="terminal-chat"
      data-state={state.state}
    >
      {snapshot.omitted.length > 0 ? (
        <p className="chat-banner terminal-chat-note" role="status">
          <span>
            Left out of this terminal: {snapshot.omitted.join("; ")}. Chat still uses them.
          </span>
        </p>
      ) : null}
      <div className="terminal-chat-frame">
        <div className="terminal-chat-screen" ref={frame} />
        {state.state === "idle" || state.state === "starting" ? (
          <p className="terminal-chat-starting" role="status">
            <span className="ui-pulse" aria-hidden="true" />
            {setupPending
              ? `Waiting for this worktree's setup to finish, then starting ${agentName} in the terminal…`
              : `Starting ${agentName} in the terminal…`}
          </p>
        ) : null}
      </div>
      {snapshot.link ? (
        // Opens in the browser, never here: the opener plugin takes target=_blank clicks for http
        // and https only (Markdown.tsx). The full address shows first, as in the sign-in sheet.
        <p className="chat-banner terminal-chat-link" data-testid="terminal-chat-link">
          <a
            href={snapshot.link}
            target="_blank"
            rel="noreferrer noopener"
            onClick={() => runtime.dismissLink()}
          >
            Open link: {snapshot.link}
          </a>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            onClick={() => runtime.dismissLink()}
          >
            Dismiss
          </button>
        </p>
      ) : null}
      {ended ? (
        <div className="terminal-chat-ended" data-testid="terminal-chat-ended">
          <p role={state.state === "failed" ? "alert" : "status"}>{endedText}</p>
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            onClick={() => void runtime.restart()}
          >
            Start again
          </button>
          <button type="button" className="g-button g-button-ghost g-button-28" onClick={onChat}>
            Back to Chat
          </button>
        </div>
      ) : null}
    </section>
  );
}
