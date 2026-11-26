import type { Terminal } from "@xterm/xterm";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { findSetupToken } from "../lib/setupToken";
import {
  TerminalSession,
  type TerminalState,
  type TerminalTransport,
  terminalTransport,
} from "../lib/terminal";
import { DialogClose, dialogClassName } from "./chat/Modal";
import { openXterm } from "./xterm";
import "@xterm/xterm/css/xterm.css";
import "./TerminalSheet.css";

/** `claude setup-token` for Add account: it signs in, prints the token and saves nothing. */
function tokenStatus(state: TerminalState | { state: "idle" }): string {
  switch (state.state) {
    case "idle":
      return "Ready to get a token.";
    case "starting":
      return "Opening the vendor sign-in flow…";
    case "running":
      return "Approve it in your browser. If it shows a code, paste it below and choose Continue.";
    case "failed":
      return state.message;
    case "finished":
      return state.exitCode === 0
        ? "The token did not show up. Copy it from above and paste it into Add account."
        : state.exitCode === null
          ? "The terminal closed without an exit status."
          : `The vendor command exited with code ${state.exitCode}.`;
  }
}

/** The caller opens this sheet; only its Start button launches a vendor command. No theme or
 * command picker is exposed. Keep it mounted until onClose to retain the terminal's local view. */
export function TerminalSheet({
  agentId,
  agentName,
  onClose,
  onFinished,
  transport = terminalTransport,
  purpose = "sign-in",
  review,
  autoStart = false,
  onToken,
}: {
  readonly agentId: string;
  readonly agentName: string;
  readonly onClose: () => void;
  readonly onFinished?: (exitCode: number | null) => void;
  readonly transport?: TerminalTransport;
  readonly purpose?: "sign-in" | "installation" | "setup" | "project" | "token" | "update";
  readonly review?: ReactNode;
  /** Starts the command once the terminal is ready: the person already asked for it. */
  readonly autoStart?: boolean;
  /** `purpose="token"`: the token `claude setup-token` printed, once, as soon as it is whole. */
  readonly onToken?: (token: string) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const container = useRef<HTMLElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const session = useRef<TerminalSession | null>(null);
  const callbacks = useRef({ onClose, onFinished, onToken });
  callbacks.current = { onClose, onFinished, onToken };
  const started = useRef(false);
  const [ready, setReady] = useState(false);
  const [state, setState] = useState<TerminalState | { state: "idle" }>({ state: "idle" });
  const [link, setLink] = useState<string | null>(null);
  const [copyMessage, setCopyMessage] = useState("");
  const title = useId();
  const description = useId();

  useEffect(() => {
    const sheet = dialog.current;
    sheet?.showModal();
    return () => sheet?.close();
  }, []);

  useEffect(() => {
    let disposed = false;
    let cleanup = () => {};
    setReady(false);
    setState({ state: "idle" });
    void (async () => {
      const element = container.current;
      if (disposed || !element) return;
      // Vendor output cannot set the OS clipboard. Copying requires the explicit button below.
      const view = await openXterm(element, {
        inputLabel: `${agentId} ${purpose} terminal input`,
        onLink: setLink,
      });
      if (disposed) {
        view.dispose();
        return;
      }
      const term = view.term;
      let token: string | null = null;
      // The token is read off the rendered screen, so wrapping and redraws are already resolved.
      const scan = (finished: boolean) => {
        if (purpose !== "token" || token !== null || !callbacks.current.onToken) return;
        token = findSetupToken(screenLines(term), finished);
        if (token !== null) callbacks.current.onToken(token);
      };
      const connection = new TerminalSession(
        transport,
        (bytes) =>
          new Promise<void>((resolve) =>
            term.write(bytes, () => {
              scan(false);
              resolve();
            }),
          ),
        (next) => {
          if (disposed) return;
          term.options.disableStdin = next.state !== "running";
          setState(next);
          if (next.state === "running") term.focus();
          if (next.state === "finished") {
            scan(true);
            callbacks.current.onFinished?.(next.exitCode);
          }
        },
      );
      terminal.current = term;
      session.current = connection;
      const encoder = new TextEncoder();
      const textInput = term.onData((text) => connection.write(encoder.encode(text)));
      const binaryInput = term.onBinary((text) =>
        connection.write(Uint8Array.from(text, (char) => char.charCodeAt(0) & 255)),
      );
      const resize = term.onResize(({ rows, cols }) => {
        void connection.resize({ rows, cols });
      });
      const observer = new ResizeObserver(view.fit);
      observer.observe(element);
      view.fit();
      setReady(true);
      cleanup = () => {
        void connection.close();
        observer.disconnect();
        textInput.dispose();
        binaryInput.dispose();
        resize.dispose();
        view.dispose();
        if (session.current === connection) session.current = null;
        if (terminal.current === term) terminal.current = null;
      };
    })().catch(() => {
      if (!disposed) setState({ state: "failed", message: "The terminal view could not load." });
    });
    return () => {
      disposed = true;
      cleanup();
    };
  }, [agentId, purpose, transport]);

  useEffect(() => {
    if (!autoStart || !ready || started.current || state.state !== "idle") return;
    const term = terminal.current;
    if (!term) return;
    started.current = true;
    void session.current?.start(agentId, { rows: term.rows, cols: term.cols });
  }, [autoStart, ready, state.state, agentId]);

  const close = () => {
    void session.current?.close();
    callbacks.current.onClose();
  };
  const status =
    purpose === "token"
      ? tokenStatus(state)
      : state.state === "idle"
        ? purpose === "installation"
          ? "Review the installation before starting."
          : purpose === "setup"
            ? "Review the command before starting."
            : purpose === "project"
              ? "Review the folder and shell before starting."
              : purpose === "update"
                ? "Ready to run the update."
                : "Ready to sign in."
        : state.state === "starting"
          ? purpose === "installation"
            ? "Checking the reviewed installer…"
            : purpose === "setup"
              ? "Checking the reviewed setup…"
              : purpose === "project"
                ? "Checking the reviewed project folder…"
                : purpose === "update"
                  ? "Starting the agent's update…"
                  : "Opening the vendor sign-in flow…"
          : state.state === "running"
            ? purpose === "setup"
              ? "The setup command is running."
              : purpose === "project"
                ? "The project shell is running."
                : purpose === "update"
                  ? "The update is running. Answer any prompt it asks below."
                  : "Follow the vendor's prompts below."
            : state.state === "failed"
              ? state.message
              : state.exitCode === 0
                ? purpose === "installation"
                  ? "The installer finished. Refresh agents to verify the installation."
                  : purpose === "setup"
                    ? "The setup command finished."
                    : purpose === "project"
                      ? "The project shell finished."
                      : purpose === "update"
                        ? "The update finished. Ghost checks the new version when you close this."
                        : "The vendor command finished. Check sign-in status to confirm."
                : state.exitCode === null
                  ? "The terminal closed without an exit status."
                  : purpose === "update"
                    ? `The update exited with code ${state.exitCode}. Its output below says why.`
                    : `The ${purpose === "project" ? "project shell" : "vendor command"} exited with code ${state.exitCode}.`;

  const outcome =
    state.state === "failed" || (state.state === "finished" && state.exitCode !== 0)
      ? "failed"
      : state.state === "finished"
        ? "passed"
        : undefined;
  const content = (
    <dialog
      ref={dialog}
      className={dialogClassName("lg", "terminal-sheet")}
      data-purpose={purpose}
      data-state={state.state}
      data-outcome={outcome}
      aria-labelledby={title}
      aria-describedby={description}
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
    >
      <header className="g-dialog-header terminal-sheet__header">
        <div>
          <h2 id={title} className="g-dialog-title">
            {purpose === "token"
              ? `Sign in to another ${agentName} account`
              : purpose === "installation"
                ? `Install ${agentName}`
                : purpose === "setup"
                  ? `Setup ${agentName}`
                  : purpose === "project"
                    ? `${agentName} terminal`
                    : purpose === "update"
                      ? `Update ${agentName}`
                      : `Sign in to ${agentName}`}
          </h2>
          <p id={description}>
            {purpose === "token"
              ? "Runs claude setup-token, the agent's own sign-in. It prints a token and saves nothing; Ghost keeps the token in this computer's secure storage. Closing stops it."
              : purpose === "installation"
                ? "Runs the package manager below. Closing stops it."
                : purpose === "setup"
                  ? "Runs the reviewed command. Closing stops it."
                  : purpose === "project"
                    ? "Opens a shell in the reviewed project folder. Closing stops it."
                    : purpose === "update"
                      ? "Runs the agent's own update command, the one Update runs. Closing stops it."
                      : "Runs the agent's own sign-in. Closing stops it."}{" "}
            Terminal output is not saved.
          </p>
        </div>
        <DialogClose label={`Close ${purpose} terminal`} onClick={close} />
      </header>
      {review}
      <div className="terminal-sheet__status" role="status">
        {status}
      </div>
      {state.state === "finished" && state.truncated ? (
        <p role="status" className="terminal-sheet__note">
          Some output could not be drained before the terminal closed.
        </p>
      ) : null}
      <section
        ref={container}
        className="terminal-sheet__viewport"
        aria-label={`${agentName} ${purpose} terminal`}
      />
      {link ? (
        <p className="terminal-sheet__link">
          <a href={link} target="_blank" rel="noreferrer noopener">
            Open vendor link: {link}
          </a>
        </p>
      ) : null}
      <footer className="g-dialog-actions terminal-sheet__footer">
        <span className="terminal-sheet__copied" role="status">
          {copyMessage}
        </span>
        <button
          type="button"
          className="g-button"
          disabled={!ready}
          onClick={() => {
            const selected = terminal.current?.getSelection();
            if (!selected) {
              setCopyMessage("Select terminal text to copy.");
              return;
            }
            void navigator.clipboard.writeText(selected).then(
              () => setCopyMessage("Copied selection."),
              () => setCopyMessage("Could not copy the selection."),
            );
          }}
        >
          Copy selection
        </button>
        {purpose === "token" && state.state === "running" ? (
          // The browser's code is pasted into the terminal; Continue is its Enter.
          <button
            type="button"
            className="g-button g-button-primary terminal-sheet__button--primary"
            onClick={() => {
              void session.current?.write(new TextEncoder().encode("\r"));
              terminal.current?.focus();
            }}
          >
            Continue
          </button>
        ) : (
          <button
            type="button"
            className="g-button g-button-primary terminal-sheet__button--primary"
            disabled={!ready || state.state !== "idle"}
            onClick={() => {
              const term = terminal.current;
              started.current = true;
              if (term) void session.current?.start(agentId, { rows: term.rows, cols: term.cols });
            }}
          >
            {purpose === "token"
              ? "Start sign-in"
              : purpose === "installation"
                ? "Start installation"
                : purpose === "setup"
                  ? "Start setup"
                  : purpose === "project"
                    ? "Start project terminal"
                    : purpose === "update"
                      ? "Start update"
                      : "Start sign-in"}
          </button>
        )}
      </footer>
    </dialog>
  );
  return typeof document === "undefined" ? content : createPortal(content, document.body);
}

/** The terminal's lines, screen and scrollback, with lines it wrapped itself joined again. */
function screenLines(term: Terminal): string[] {
  const buffer = term.buffer.active;
  const lines: string[] = [];
  for (let index = 0; index < buffer.length; index += 1) {
    const line = buffer.getLine(index);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && lines.length) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  return lines;
}
