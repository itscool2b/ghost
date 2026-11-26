import { Globe } from "lucide-react";
import { useState, useSyncExternalStore } from "react";
import { type BrowserStore, browserStore as nativeBrowserStore } from "../../lib/browser";
import { ClearDataDialog } from "../browser/ClearDataDialog";
import { HistoryView } from "../browser/HistoryView";
import { Modal } from "../chat/Modal";
import type { Preferences } from "../settings/preferences";
import { Switch } from "../settings/SettingsRow";
import "../browser/browser.css";

/** What the Universal plugins section needs from Settings. */
export interface UniversalSettings {
  readonly prefs: Preferences;
  /** Saved passwords are WebView2's own, so they are offered on Windows only. */
  readonly os: string;
  readonly browser?: BrowserStore;
}

/** What an agent can do in the browser, in a person's words. The tool names are Ghost's. */
const AGENT_TOOLS: readonly (readonly [string, string])[] = [
  ["browser_navigate", "Open a page"],
  ["browser_snapshot", "Read a page"],
  ["browser_click", "Click"],
  ["browser_type", "Type"],
  ["browser_press_key", "Press keys"],
  ["browser_hover", "Hover"],
  ["browser_select", "Choose in a list"],
  ["browser_scroll", "Scroll"],
  ["browser_screenshot", "Take a screenshot"],
  ["browser_console", "Read the console"],
  ["browser_network", "See network requests"],
  ["browser_evaluate", "Run JavaScript"],
  ["browser_wait_for", "Wait for something"],
  ["browser_tabs", "Use tabs"],
];

/**
 * Browser use, the universal plugin: Ghost's own, for every agent (owner, 9 November 2026). It
 * comes with Ghost and cannot be added or removed, only turned off, which turns it off for
 * every agent and takes it out of the app. It is Settings > Browser (owner, 10 November 2026),
 * which says outright that its tools are exposed to every agent, and which ones.
 */
export function UniversalPlugins({ prefs, os, browser = nativeBrowserStore }: UniversalSettings) {
  useSyncExternalStore(browser.subscribe, browser.getVersion, browser.getVersion);
  const status = browser.status();
  const general = prefs.shown?.general ?? null;
  const on = general?.browser ?? true;
  const disabled = general === null;
  const [history, setHistory] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState(false);
  const windows = os === "windows";
  return (
    <div className="plugins-agent plugins-universal" data-testid="plugins-universal">
      {prefs.error ? (
        <p className="library-feedback" role="alert">
          {prefs.error}
        </p>
      ) : null}
      <section className="plugins-universal-card" aria-labelledby="plugins-universal-browser">
        <div className="plugins-universal-head" data-setting="Browser use">
          <span className="plugins-universal-mark" aria-hidden="true">
            <Globe size={18} />
          </span>
          <div className="plugins-universal-text">
            <h2 id="plugins-universal-browser">Browser use</h2>
            <p>
              {on
                ? "A browser beside each chat, Annotate to point at what you mean on a page, and tools agents use to open, read, click through and test pages."
                : "Off for every agent: chats show no browser, and agents' browser tools stop working. History and sign-ins stay for when it is back on."}
            </p>
          </div>
          <Switch
            label="Browser use"
            testId="plugins-browser"
            checked={on}
            disabled={disabled}
            onChange={(enabled) => prefs.change({ type: "browser", enabled })}
          />
        </div>
        {on && status?.unavailable ? (
          <p className="plugins-universal-note" role="status">
            {status.unavailable}
          </p>
        ) : null}
        {on ? (
          <>
            <div className="plugins-universal-tools" data-setting="Tools for agents">
              <div>
                <strong>Exposed to every AI agent</strong>
                <span>
                  Every agent you chat with gets these tools from Ghost, including agents you add
                  later. Nothing to install. Terminal chats and agents on another computer do not
                  get them. Turn Browser use off and they are gone for every agent.
                </span>
              </div>
              <ul aria-label="Browser tools agents get">
                {AGENT_TOOLS.map(([tool, does]) => (
                  <li key={tool} title={tool}>
                    {does}
                  </li>
                ))}
              </ul>
            </div>
            {/* Only Windows' engine keeps saved passwords; elsewhere the row says so outright, so
                Settings search for passwords still lands on it. */}
            <div className="plugins-universal-setting" data-setting="Save passwords">
              <div>
                <strong>Save passwords</strong>
                <span>
                  {windows
                    ? "The browser offers to save sign-ins and fills them in. Agents never see saved passwords."
                    : "Windows only. Here the browser does not offer to save sign-ins."}
                </span>
              </div>
              {windows ? (
                <Switch
                  label="Save passwords"
                  testId="plugins-browser-passwords"
                  checked={general?.browser_passwords ?? true}
                  disabled={disabled}
                  onChange={(enabled) => prefs.change({ type: "browser_passwords", enabled })}
                />
              ) : null}
            </div>
            <div className="plugins-universal-setting" data-setting="Browsing history">
              <div>
                <strong>Browsing history</strong>
                <span>
                  The pages the address bar suggests, for every chat. Review or remove them.
                </span>
              </div>
              <button
                type="button"
                className="g-button g-button-outline g-button-28"
                aria-label="Open browsing history"
                onClick={() => setHistory(true)}
              >
                Open
              </button>
            </div>
            <div className="plugins-universal-setting" data-setting="Clear browsing data">
              <div>
                <strong>Clear browsing data</strong>
                <span>
                  {cleared
                    ? "Cleared. Your own browser was not touched."
                    : "History, cookies, cached files and saved passwords. Your own browser is not touched."}
                </span>
              </div>
              <button
                type="button"
                className="g-button g-button-outline g-button-28"
                aria-label="Clear browsing data"
                onClick={() => {
                  setCleared(false);
                  setClearing(true);
                }}
              >
                Clear…
              </button>
            </div>
          </>
        ) : null}
      </section>
      {history ? (
        <Modal
          title="Browsing history"
          size="lg"
          className="plugins-history-dialog"
          onClose={() => setHistory(false)}
        >
          <div className="plugins-history">
            <HistoryView api={browser.api} level={3} />
          </div>
        </Modal>
      ) : null}
      {clearing ? (
        <ClearDataDialog
          api={browser.api}
          passwords={windows}
          onClose={() => setClearing(false)}
          onCleared={() => setCleared(true)}
        />
      ) : null}
    </div>
  );
}
