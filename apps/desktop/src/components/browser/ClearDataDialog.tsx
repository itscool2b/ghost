import { useState } from "react";
import type { ClearData } from "../../bindings";
import type { BrowserApi } from "../../lib/browser";
import { errorMessage } from "../../lib/errors";
import { Modal } from "../chat/Modal";

const CHOICES: readonly { key: keyof ClearData; label: string; detail: string }[] = [
  {
    key: "history",
    label: "Browsing history",
    detail: "The pages the address bar and History offer.",
  },
  {
    key: "cookies",
    label: "Cookies and site data",
    detail: "Signs you out of sites in Ghost's browser. Your own browser is not touched.",
  },
  { key: "cache", label: "Cached images and files", detail: "Pages load again from the network." },
  { key: "passwords", label: "Saved passwords", detail: "Sign-ins the browser offered to save." },
];

/** Clears the browser's data, as chosen. Its own data only: never the person's own browser. */
export function ClearDataDialog({
  api,
  passwords,
  onClose,
  onCleared,
}: {
  readonly api: BrowserApi;
  /** The engine keeps saved passwords here (Windows). */
  readonly passwords: boolean;
  readonly onClose: () => void;
  readonly onCleared?: () => void;
}) {
  const [choice, setChoice] = useState<ClearData>({
    history: true,
    cookies: false,
    cache: true,
    passwords: false,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shown = CHOICES.filter((item) => item.key !== "passwords" || passwords);
  const nothing = !shown.some((item) => choice[item.key]);
  return (
    <Modal title="Clear browsing data" onClose={onClose} closeDisabled={busy}>
      <form
        className="browser-clear"
        onSubmit={(event) => {
          event.preventDefault();
          if (nothing || busy) return;
          setBusy(true);
          setError(null);
          api.clear({ ...choice, passwords: passwords && choice.passwords }).then(
            () => {
              setBusy(false);
              onCleared?.();
              onClose();
            },
            (failure) => {
              setBusy(false);
              setError(errorMessage(failure));
            },
          );
        }}
      >
        <p className="browser-clear-lead">
          Everything Ghost's browser keeps, for every chat. Your own browser is not touched.
        </p>
        <fieldset className="browser-clear-choices">
          <legend className="sr-only">What to clear</legend>
          {shown.map((item) => (
            <label key={item.key} className="browser-clear-choice">
              <input
                type="checkbox"
                checked={choice[item.key]}
                onChange={(event) => setChoice({ ...choice, [item.key]: event.target.checked })}
              />
              <span className="browser-clear-text">
                <span>{item.label}</span>
                <span className="browser-clear-detail">{item.detail}</span>
              </span>
            </label>
          ))}
        </fieldset>
        {error ? (
          <p className="ui-error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="g-dialog-actions">
          <button type="button" className="g-button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="g-button g-button-primary" disabled={nothing || busy}>
            {busy ? "Clearing…" : "Clear data"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
