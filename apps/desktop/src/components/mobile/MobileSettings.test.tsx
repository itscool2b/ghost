import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { MobileStatus, SetupPreferences } from "../../bindings";
import { emulator, FakeMobile, mobileStatus } from "../../dev/mobileFixture";
import { MobileStore } from "../../lib/mobile";
import { DEFAULT_GENERAL } from "../../lib/setup";
import type { Preferences } from "../settings/preferences";
import { MobileSettings } from "./MobileSettings";

/** A store whose status is known before the first render. */
function store(status: MobileStatus): MobileStore {
  const fake = new FakeMobile(status);
  fake.listen = (listener) => {
    listener(status);
    return () => {};
  };
  const mobile = new MobileStore(fake);
  mobile.subscribe(() => {});
  return mobile;
}

function prefs(general: Partial<SetupPreferences["general"]> = {}): Preferences {
  return {
    saved: null,
    shown: {
      revision: 1,
      step: "project",
      motion: "full",
      appearance: "system",
      theme: "claude",
      text_size: "compact",
      completed: true,
      first_project_id: null,
      tour_seen: [],
      general: { ...DEFAULT_GENERAL, ...general },
    },
    error: null,
    change: () => {},
    reload: () => {},
  };
}

const render = (status: MobileStatus, general: Partial<SetupPreferences["general"]> = {}) =>
  renderToStaticMarkup(<MobileSettings prefs={prefs(general)} mobile={store(status)} />);

describe("Settings > Mobile", () => {
  it("is on by default, lists what agents get, and offers no way to add or remove it", () => {
    const html = render(mobileStatus());
    expect(html).toMatch(/aria-label="Mobile"[^>]*checked=""/);
    expect(html).toContain("Device tools for agents");
    expect(html.match(/<li /g)).toHaveLength(16);
    // What an agent may not do is said outright.
    expect(html).toContain("cannot use your phone until you allow it");
    expect(html).toContain("accept Google&#x27;s licence for you");
    expect(html).not.toMatch(/<button[^>]*>(Add|Remove|Install|Uninstall)/);
  });

  it("says what is installed in plain words", () => {
    const android = (status: MobileStatus) =>
      />([^<]*)<\/span><\/div><\/div><\/section>/.exec(render(status))?.[1] ?? "";
    expect(android(mobileStatus())).toMatch(/^Set up\. Open the Device pane/);
    expect(
      android(
        mobileStatus({
          devices: [
            emulator({ state: "ready" }),
            emulator({ id: "o7_phone_2", state: "starting" }),
          ],
        }),
      ),
    ).toBe("Set up. 2 phones are running.");
    expect(android(mobileStatus({ devices: [emulator({ state: "ready" })] }))).toBe(
      "Set up. One phone is running.",
    );
    const none = mobileStatus();
    expect(
      android({
        ...none,
        setup: { ...none.setup, adb: false, emulator: false, acceleration: null },
      }),
    ).toMatch(/^Not set up\./);
    expect(
      android({ ...none, setup: { ...none.setup, emulator: false, acceleration: null } }),
    ).toMatch(/^Phones on a cable work\./);
    expect(
      android({
        ...none,
        setup: {
          ...none.setup,
          acceleration: { usable: false, detail: "WHPX is missing.", fix: "Turn it on." },
        },
      }),
    ).toBe("The emulator is installed, but this computer cannot run it fast yet. Turn it on.");
  });

  it("turned off, says what stops and shows nothing else", () => {
    const html = render(mobileStatus(), { mobile: false });
    expect(html).not.toMatch(/aria-label="Mobile"[^>]*checked=""/);
    expect(html).toContain("every phone o7 started is stopped");
    expect(html).not.toContain("Device tools for agents");
    expect(html).not.toContain('data-setting="Android"');
  });

  it("says why when this computer cannot run Android's tools", () => {
    const html = render(
      mobileStatus({ unavailable: "Android's tools are not published for this kind of computer." }),
    );
    expect(html).toContain("not published for this kind of computer");
  });
});
