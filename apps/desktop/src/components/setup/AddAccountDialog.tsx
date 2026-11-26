import { useId, useRef, useState } from "react";
import type { AgentAccounts } from "../../bindings";
import { type AgentAccountsApi, nextAccountName } from "../../lib/agentAccounts";
import { errorMessage } from "../../lib/errors";
import { Modal } from "../chat/Modal";
import { TerminalSheet } from "../TerminalSheet";
import "./accounts.css";

/**
 * Adds a Claude Code account in one go: Sign in runs `claude setup-token` in the terminal sheet,
 * the person approves it in the browser, and the token it prints is picked up off the terminal
 * and goes straight to Rust and secure storage. Pasting a token stays possible for when the
 * terminal shows it but it is not picked up. Nothing here shows a token again.
 */
export function AddAccountDialog({
  agentId,
  agentName,
  api,
  accounts = null,
  after = "listed",
  onAdded,
  onClose,
}: {
  readonly agentId: string;
  readonly agentName: string;
  readonly api: AgentAccountsApi;
  /** The accounts already added, for the suggested name. */
  readonly accounts?: AgentAccounts | null;
  /** What adding leads to, for the dialog's words: this chat moves to it, or it is listed. */
  readonly after?: "switch" | "listed";
  /** The account is added. A failure here (moving the chat to it) shows in the dialog. */
  readonly onAdded: (accounts: AgentAccounts, id: string, name: string) => void | Promise<void>;
  readonly onClose: () => void;
}) {
  const intro = useId();
  const hint = useId();
  const [name, setName] = useState(() => nextAccountName(accounts));
  const [token, setToken] = useState("");
  const [terminal, setTerminal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [manual, setManual] = useState(false);
  const captured = useRef(false);
  const label = name.trim().replace(/\s+/g, " ");
  const save = async (value: string) => {
    setBusy(true);
    setError(null);
    let added: { id: string; name: string } | undefined;
    try {
      const next = await api.add(agentId, label, value);
      setToken("");
      added = next.accounts.find((account) => account.name.toLowerCase() === label.toLowerCase());
      if (!added) throw new Error("The account was added, but it is not listed. Reopen accounts.");
      await onAdded(next, added.id, added.name);
    } catch (reason) {
      if (added) {
        setDone(true);
        setError(`${added.name} is added. ${errorMessage(reason)}`);
      } else {
        setManual(true);
        setError(errorMessage(reason));
      }
    } finally {
      setBusy(false);
    }
  };
  const ready = !busy && !done && label.length > 0;
  return (
    <Modal
      title={`Add a ${agentName} account`}
      onClose={onClose}
      size="md"
      className="add-account"
      closeDisabled={busy}
    >
      <form
        className="add-account-form"
        aria-describedby={intro}
        onSubmit={(event) => {
          event.preventDefault();
          if (!ready) return;
          captured.current = false;
          setNote(null);
          setError(null);
          setTerminal(true);
        }}
      >
        <p id={intro} className="add-account-intro">
          Sign in with the account you want to add. Your browser approves whichever Claude account
          it is signed in to on claude.ai, so switch accounts there first if you need to.
          {after === "switch"
            ? " Once it is added, this chat continues on it."
            : " Once it is added, choose it from any chat."}
        </p>
        <label className="ui-field">
          Name
          <input
            className="g-input"
            value={name}
            maxLength={40}
            autoComplete="off"
            required
            disabled={busy || done}
            onChange={(event) => setName(event.currentTarget.value)}
          />
        </label>
        {note ? (
          <p className="ui-muted add-account-note" role="status">
            {note}
          </p>
        ) : null}
        <details
          className="add-account-manual"
          open={manual}
          onToggle={(event) => setManual(event.currentTarget.open)}
        >
          <summary>Paste a token instead</summary>
          <div className="add-account-field">
            <label className="ui-field">
              Token
              <input
                className="g-input add-account-token"
                type="password"
                value={token}
                placeholder="sk-ant-oat01-…"
                autoComplete="off"
                spellCheck={false}
                disabled={busy || done}
                aria-describedby={hint}
                onChange={(event) => setToken(event.currentTarget.value)}
              />
            </label>
            <small id={hint} className="ui-muted">
              From <code>claude setup-token</code>. Kept in this computer's secure storage; Ghost
              never shows it again.
            </small>
            <div className="ui-row">
              <button
                type="button"
                className="g-button"
                disabled={!ready || !token.trim()}
                onClick={() => void save(token)}
              >
                Add with this token
              </button>
            </div>
          </div>
        </details>
        {error ? (
          <p role="alert" className="ui-error">
            {error}
          </p>
        ) : null}
        <div className="g-dialog-actions">
          <button type="button" className="g-button" disabled={busy} onClick={onClose}>
            {done ? "Close" : "Cancel"}
          </button>
          {done ? null : (
            <button type="submit" className="g-button g-button-primary" disabled={!ready}>
              {busy ? "Adding…" : "Sign in…"}
            </button>
          )}
        </div>
      </form>
      {terminal ? (
        <TerminalSheet
          agentId={agentId}
          agentName={agentName}
          purpose="token"
          transport={api.tokenTerminal}
          autoStart
          onClose={() => setTerminal(false)}
          onToken={(value) => {
            captured.current = true;
            setTerminal(false);
            void save(value);
          }}
          onFinished={(exitCode) => {
            if (captured.current) return;
            setManual(true);
            setNote(
              exitCode === 0
                ? "The sign-in finished, but its token was not picked up. Copy it from the terminal and paste it below."
                : "The sign-in did not finish. Choose Sign in to try again.",
            );
          }}
        />
      ) : null}
    </Modal>
  );
}
