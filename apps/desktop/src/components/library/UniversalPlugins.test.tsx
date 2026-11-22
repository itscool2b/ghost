import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { BrowserStatus, SetupPreferences } from "../../bindings";
import { type BrowserApi, BrowserStore } from "../../lib/browser";
import { DEFAULT_GENERAL } from "../../lib/setup";
import type { Preferences } from "../settings/preferences";
import { UniversalPlugins } from "./UniversalPlugins";

const refuse = () => Promise.reject(new Error("no pages here"));

/** A store whose status is known before the first render. */
function store(status: Partial<BrowserStatus> = {}): BrowserStore {
  const full: BrowserStatus = {
    enabled: true,
    unavailable: null,
    passwords: true,
    media_notice: null,
    ...status,
  };
  const api = {
    status: async () => full,
    listen: (listener) => {
      listener({ type: "status", status: full });
      return () => {};
    },
    history: async () => [],
    removeHistory: async () => 0,
    clear: async () => {},
    suggest: async () => [],
    chat: refuse,
  } as Partial<BrowserApi> as BrowserApi;
  const browser = new BrowserStore(api);
  browser.onEvent(() => {});
  return browser;
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

const render = (props: Parameters<typeof UniversalPlugins>[0]) =>
  renderToStaticMarkup(<UniversalPlugins {...props} />);

describe("Universal plugins", () => {
  it("offers Browser use on, with its settings, and no way to add or remove it", () => {
    const html = render({ prefs: prefs(), os: "windows", browser: store() });
    expect(html).toContain("Browser use");
    // It says outright that agents get its tools, and which.
    expect(html).toContain("Exposed to every AI agent");
    expect(html).toContain("including agents you");
    for (const tool of ["Open a page", "Click", "Take a screenshot", "Run JavaScript"])
      expect(html).toContain(tool);
    expect(html).toContain('title="browser_navigate"');
    expect(html).toMatch(/aria-label="Browser use"[^>]*checked=""/);
    expect(html).toContain("Save passwords");
    expect(html).toContain("Browsing history");
    expect(html).toContain("Clear browsing data");
    expect(html).not.toMatch(/>(Add|Remove|Install|Uninstall)/);
  });

  it("says saved passwords are Windows only, where the engine keeps them, with no switch elsewhere", () => {
    for (const os of ["macos", "linux"] as const) {
      const html = render({ prefs: prefs(), os, browser: store({ passwords: false }) });
      // The row stays, so Settings search for passwords lands on it.
      expect(html).toContain("Save passwords");
      expect(html).toContain("Windows only. Here the browser does not offer to save sign-ins.");
      expect(html).not.toContain('aria-label="Save passwords"');
      expect(html).toContain("Browsing history");
    }
  });

  it("says what turning it off does, and hides its settings while off", () => {
    const html = render({
      prefs: prefs({ browser: false }),
      os: "windows",
      browser: store({ enabled: false }),
    });
    expect(html).toMatch(/aria-label="Browser use"(?![^>]*\schecked="")/);
    expect(html).toContain("Off for every agent");
    expect(html).not.toContain("Browsing history");
  });

  it("shows the saved-passwords switch as saved", () => {
    const html = render({
      prefs: prefs({ browser_passwords: false }),
      os: "windows",
      browser: store(),
    });
    expect(html).toMatch(/aria-label="Save passwords"(?![^>]*\schecked="")/);
    // And the same assertion catches a checked switch.
    expect(render({ prefs: prefs(), os: "windows", browser: store() })).not.toMatch(
      /aria-label="Save passwords"(?![^>]*\schecked="")/,
    );
  });

  it("explains a system that cannot run the browser", () => {
    expect(
      render({
        prefs: prefs(),
        os: "windows",
        browser: store({ unavailable: "The WebView2 runtime is missing." }),
      }),
    ).toContain("The WebView2 runtime is missing.");
  });
});
