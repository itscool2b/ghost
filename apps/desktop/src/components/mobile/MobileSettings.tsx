import { Smartphone } from "lucide-react";
import { useEffect, useSyncExternalStore } from "react";
import { type MobileStore, mobileStore as nativeMobileStore } from "../../lib/mobile";
import type { Preferences } from "../settings/preferences";
import { Switch } from "../settings/SettingsRow";
import "../library/plugin-library.css";

/** What an agent can do on the chat's device, in a person's words. The tool names are o7's. */
const AGENT_TOOLS: readonly (readonly [string, string])[] = [
  ["mobile_devices", "See the devices"],
  ["mobile_start", "Start a phone"],
  ["mobile_install", "Install an app"],
  ["mobile_launch", "Start an app"],
  ["mobile_stop_app", "Stop an app"],
  ["mobile_snapshot", "Read the screen"],
  ["mobile_tap", "Tap"],
  ["mobile_type", "Type"],
  ["mobile_swipe", "Swipe"],
  ["mobile_press", "Press buttons"],
  ["mobile_screenshot", "Take a screenshot"],
  ["mobile_logs", "Read the app's log"],
  ["mobile_open_link", "Open a link"],
  ["mobile_settings", "Change device settings"],
  ["mobile_wait_for", "Wait for something"],
  ["mobile_stop", "Stop the phone"],
];

/**
 * Mobile, the universal plugin (docs/superpowers/plans/2026-11-21-mobile.md): o7's own, for every
 * agent, like Browser use. It comes with o7 and cannot be added or removed, only turned off,
 * which stops every device o7 started and takes the Device pane out of the app.
 */
export function MobileSettings({
  prefs,
  mobile = nativeMobileStore,
}: {
  readonly prefs: Preferences;
  readonly mobile?: MobileStore;
}) {
  useSyncExternalStore(mobile.subscribe, mobile.getVersion, mobile.getVersion);
  // The page says what is installed now, not what was when the app started.
  useEffect(() => mobile.refresh(), [mobile]);
  const status = mobile.status();
  const general = prefs.shown?.general ?? null;
  const on = general?.mobile ?? true;
  const setup = status?.setup ?? null;
  const running = status?.devices.filter(
    (device) => device.kind === "emulator" && device.state !== "stopped",
  ).length;
  const iphones = status?.devices.filter(
    (device) => device.kind === "simulator" && device.state !== "stopped",
  ).length;
  return (
    <div className="plugins-agent plugins-universal" data-testid="mobile-settings">
      {prefs.error ? (
        <p className="library-feedback" role="alert">
          {prefs.error}
        </p>
      ) : null}
      <section className="plugins-universal-card" aria-labelledby="plugins-universal-mobile">
        <div className="plugins-universal-head" data-setting="Mobile">
          <span className="plugins-universal-mark" aria-hidden="true">
            <Smartphone size={18} />
          </span>
          <div className="plugins-universal-text">
            <h2 id="plugins-universal-mobile">Mobile</h2>
            <p>
              {on
                ? "A phone beside each chat, for building phone apps: an Android emulator of the chat's own, an iPhone simulator of its own on a Mac, or your phone on a cable. You and the agent use the same device."
                : "Off for every agent: chats show no device, agents' device tools stop working, and every phone o7 started is stopped."}
            </p>
          </div>
          <Switch
            label="Mobile"
            testId="plugins-mobile"
            checked={on}
            disabled={general === null}
            onChange={(enabled) => prefs.change({ type: "mobile", enabled })}
          />
        </div>
        {on && status?.unavailable ? (
          <p className="plugins-universal-note" role="status">
            {status.unavailable}
          </p>
        ) : null}
        {on ? (
          <>
            <div className="plugins-universal-tools" data-setting="Device tools for agents">
              <div>
                <strong>Device tools for agents</strong>
                <span>
                  Every agent you chat with gets these from o7, on its own chat's device only. An
                  agent cannot use your phone until you allow it in the Device pane, and it cannot
                  set Android up or accept Google's licence for you. Terminal chats and agents on
                  another computer do not get them.
                </span>
              </div>
              <ul aria-label="Device tools agents get">
                {AGENT_TOOLS.map(([tool, does]) => (
                  <li key={tool} title={tool}>
                    {does}
                  </li>
                ))}
              </ul>
            </div>
            <div className="plugins-universal-setting" data-setting="Android">
              <div>
                <strong>Android</strong>
                <span data-testid="mobile-android">
                  {setup === null
                    ? "Looking for Android's tools."
                    : setup.emulator
                      ? setup.acceleration && !setup.acceleration.usable
                        ? `The emulator is installed, but this computer cannot run it fast yet. ${setup.acceleration.fix ?? setup.acceleration.detail}`
                        : running
                          ? `Set up. ${running === 1 ? "One phone is" : `${running} phones are`} running.`
                          : "Set up. Open the Device pane in a chat to start a phone."
                      : setup.adb
                        ? "Phones on a cable work. The emulator is not downloaded yet: open the Device pane in a chat to set it up."
                        : "Not set up. Open the Device pane in a chat (the phone button in its header) and o7 downloads Android's emulator from Google."}
                </span>
              </div>
            </div>
            <div className="plugins-universal-setting" data-setting="iPhone">
              <div>
                <strong>iPhone</strong>
                <span data-testid="mobile-iphone">
                  {setup === null
                    ? "Looking for Xcode."
                    : !setup.ios.possible
                      ? "An iPhone simulator needs a Mac with Xcode."
                      : setup.ios.ready
                        ? iphones
                          ? `Ready: ${setup.ios.detail}. ${iphones === 1 ? "One iPhone is" : `${iphones} iPhones are`} running.`
                          : `Ready: ${setup.ios.detail}. Open the Device pane in a chat to start an iPhone.`
                        : (setup.ios.missing ?? "Looking for Xcode.")}
                </span>
              </div>
            </div>
          </>
        ) : null}
      </section>
    </div>
  );
}
