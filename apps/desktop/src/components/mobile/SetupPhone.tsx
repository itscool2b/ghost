import { Check } from "lucide-react";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { MobileStatus } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import type { MobileStore } from "../../lib/mobile";
import { FetchView } from "./DevicePanel";

const noSubscribe = () => () => {};
const noVersion = () => 0;

/** Whether a phone can start here: Android set up and fast enough, or an iPhone on a Mac. */
export function phoneReady(status: MobileStatus | null): boolean {
  if (status === null || !status.enabled || status.unavailable !== null) return false;
  const android =
    status.setup.emulator &&
    (status.setup.acceleration === null || status.setup.acceleration.usable);
  return android || status.setup.ios.ready;
}

/**
 * Setup's phone page (owner, 24 November 2026: make the phone "more intuitive to the user. maybe
 * add it to onboarding"): Android's setup, with Google's offer and licence in full exactly as the
 * Device pane shows them, and on a Mac the iPhone's state. Nothing downloads before the person
 * accepts; a download keeps going after setup moves on; Continue never needs any of it.
 */
export function SetupPhone({ store }: { readonly store: MobileStore | null }) {
  useSyncExternalStore(
    store?.subscribe ?? noSubscribe,
    store?.getVersion ?? noVersion,
    store?.getVersion ?? noVersion,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const fail = useCallback((error: unknown) => setProblem(errorMessage(error)), []);
  // Looks at what this computer has, as opening the pane does.
  useEffect(() => store?.refresh(), [store]);
  const status = store?.status() ?? null;

  if (store === null || status === null)
    return (
      <p className="setup-footnote" role="status">
        Looking at this computer.
      </p>
    );
  if (!status.enabled)
    return (
      <p className="setup-footnote" data-testid="setup-phone-off">
        Mobile is off. Settings turns it on under Plugins &amp; MCP.
      </p>
    );
  if (status.unavailable !== null)
    return (
      <p className="setup-footnote" data-testid="setup-phone-unavailable">
        {status.unavailable}
      </p>
    );

  const { setup } = status;
  if (setup.fetch.state !== "idle")
    return (
      <div className="setup-phone-fetch" data-testid="setup-phone-fetch">
        <FetchView fetch={setup.fetch} api={store.api} onError={fail} />
      </div>
    );

  const slow = setup.acceleration !== null && !setup.acceleration.usable;
  const android = !setup.emulator
    ? "About 2.5 GB from Google, 8 GB on disk."
    : slow
      ? (setup.acceleration?.fix ?? setup.acceleration?.detail ?? "It cannot run fast here yet.")
      : "Ready.";
  return (
    <>
      <ul className="device-list setup-phone" aria-label="Phones" data-testid="setup-phone">
        <li className="device-row">
          <span className="device-row-text">
            <span className="device-row-title">Android phone</span>
            <span className="device-row-state" data-testid="setup-phone-android">
              {android}
            </span>
          </span>
          {setup.emulator ? (
            slow ? null : (
              <Check className="setup-phone-ready" size={16} aria-label="Ready" />
            )
          ) : (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              data-testid="setup-phone-setup"
              onClick={() => {
                setProblem(null);
                store.api.fetchOffer().catch(fail);
              }}
            >
              Set up
            </button>
          )}
        </li>
        {setup.ios.possible ? (
          <li className="device-row">
            <span className="device-row-text">
              <span className="device-row-title">iPhone</span>
              <span className="device-row-state" data-testid="setup-phone-iphone">
                {setup.ios.ready ? setup.ios.detail : (setup.ios.missing ?? "Looking for Xcode.")}
              </span>
            </span>
            {setup.ios.ready ? (
              <Check className="setup-phone-ready" size={16} aria-label="Ready" />
            ) : null}
          </li>
        ) : null}
      </ul>
      {problem ? (
        <p className="ui-error" role="alert">
          {problem}
        </p>
      ) : null}
    </>
  );
}
