/**
 * A browser for Settings' previews: Browser use's status, a history to review and remove from, and
 * Clear browsing data. It opens no page; everything else refuses.
 */
import type { BrowserStatus, ClearData, HistoryEntry } from "../bindings";
import { type BrowserApi, BrowserStore } from "../lib/browser";

const MINUTE = 60_000;

export function createBrowserSettingsFixture(status: Partial<BrowserStatus> = {}) {
  const now = Date.now();
  let history: HistoryEntry[] = [
    {
      url: "http://localhost:3104/pricing",
      title: "Acme · Pricing",
      visits: 4,
      last_visit: now - 20 * MINUTE,
    },
    {
      url: "https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_grid_layout",
      title: "CSS grid layout - CSS | MDN",
      visits: 2,
      last_visit: now - 3 * 60 * MINUTE,
    },
    {
      url: "https://github.com/itscool2b/ghost",
      title: "itscool2b/ghost",
      visits: 7,
      last_visit: now - 26 * 60 * MINUTE,
    },
  ];
  const cleared: ClearData[] = [];
  const refuse = () => Promise.reject(new Error("This preview opens no pages."));
  const api: BrowserApi = {
    status: async () => ({
      enabled: true,
      unavailable: null,
      passwords: true,
      media_notice: null,
      ...status,
    }),
    chat: refuse,
    open: refuse,
    close: refuse,
    activate: refuse,
    move: refuse,
    navigate: refuse,
    go: refuse,
    place: refuse,
    capture: refuse,
    zoom: refuse,
    devtools: refuse,
    focus: refuse,
    annotate: refuse,
    pickAt: refuse,
    suggest: async () => [],
    history: async (query, limit, before) =>
      history
        .filter((entry) => before === null || (entry.last_visit ?? 0) < before)
        .filter((entry) =>
          `${entry.title} ${entry.url}`.toLowerCase().includes(query.toLowerCase()),
        )
        .slice(0, limit),
    removeHistory: async (urls) => {
      const count = history.length;
      history = history.filter((entry) => !urls.includes(entry.url));
      return count - history.length;
    },
    clear: async (data) => {
      cleared.push(data);
      if (data.history) history = [];
    },
    openOutside: refuse,
    listen: () => () => {},
  };
  return { store: new BrowserStore(api), cleared };
}
