/**
 * `?motion=reduce` for the whole lab. @ghost/ui (Orb, AuraField) and Motion read the OS setting
 * through `matchMedia`, which a URL cannot change, so the lab wraps `matchMedia`: queries for
 * prefers-reduced-motion get a stand-in list that also matches while the URL forces reduced motion,
 * and fires `change` when either flips. Other queries pass straight through.
 *
 * Imported first in main.tsx, before anything reads the preference. It also keeps
 * `<html data-motion="reduce|full">` current, so CSS can follow both sources with one selector.
 */

const REDUCE_QUERY = "(prefers-reduced-motion: reduce)";

let forced = false;
const lists = new Set<ReducedMotionList>();
let realReduce: MediaQueryList | null = null;

function systemReduces(): boolean {
  return realReduce?.matches ?? false;
}

/** Whether the lab renders with reduced motion right now (forced by the URL or set in the OS). */
export function reducedMotionActive(): boolean {
  return forced || systemReduces();
}

function syncAttribute(): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.motion = reducedMotionActive() ? "reduce" : "full";
}

/** A MediaQueryList for a reduced-motion query that also honours the URL override. */
class ReducedMotionList extends EventTarget {
  readonly media: string;
  /** True for queries that match when motion is reduced, false for `no-preference`. */
  readonly #matchesWhenReduced: boolean;
  #last: boolean;
  onchange: ((this: MediaQueryList, event: MediaQueryListEvent) => unknown) | null = null;

  constructor(media: string, matchesWhenReduced: boolean) {
    super();
    this.media = media;
    this.#matchesWhenReduced = matchesWhenReduced;
    this.#last = this.matches;
  }

  get matches(): boolean {
    return reducedMotionActive() === this.#matchesWhenReduced;
  }

  /** Fires `change` if `matches` flipped since the last call. */
  update(): void {
    const matches = this.matches;
    if (matches === this.#last) return;
    this.#last = matches;
    let event: Event;
    try {
      event = new MediaQueryListEvent("change", { matches, media: this.media });
    } catch {
      event = Object.assign(new Event("change"), { matches, media: this.media });
    }
    this.dispatchEvent(event);
    this.onchange?.call(this as unknown as MediaQueryList, event as MediaQueryListEvent);
  }

  /** The deprecated listener API, still used by some libraries. */
  addListener(listener: ((event: MediaQueryListEvent) => void) | null): void {
    if (listener) this.addEventListener("change", listener as EventListener);
  }

  removeListener(listener: ((event: MediaQueryListEvent) => void) | null): void {
    if (listener) this.removeEventListener("change", listener as EventListener);
  }
}

/**
 * Which reduced-motion queries the stand-in handles: the bare feature (true when reducing), and
 * `: reduce` / `: no-preference`. Anything more complex goes to the real matchMedia.
 */
function classify(query: string): boolean | null {
  const normalized = query.toLowerCase().replace(/\s+/g, "");
  if (
    normalized === "(prefers-reduced-motion)" ||
    normalized === "(prefers-reduced-motion:reduce)"
  ) {
    return true;
  }
  if (normalized === "(prefers-reduced-motion:no-preference)") return false;
  return null;
}

function install(): void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
  const original = window.matchMedia.bind(window);
  realReduce = original(REDUCE_QUERY);
  realReduce.addEventListener("change", () => {
    for (const list of lists) list.update();
    syncAttribute();
  });
  window.matchMedia = (query: string): MediaQueryList => {
    const kind = classify(query);
    if (kind === null) return original(query);
    const list = new ReducedMotionList(query, kind);
    lists.add(list);
    return list as unknown as MediaQueryList;
  };
}

/** Turns the URL override on or off; every reduced-motion listener hears about a change. */
export function setReducedMotionOverride(on: boolean): void {
  if (forced === on) return;
  forced = on;
  for (const list of lists) list.update();
  syncAttribute();
}

/** Whether a hash asks for reduced motion (`motion=reduce`), read before React starts. */
export function hashForcesReducedMotion(hash: string): boolean {
  const queryStart = hash.indexOf("?");
  if (queryStart === -1) return false;
  return (
    new URLSearchParams(hash.slice(queryStart + 1)).get("motion")?.trim().toLowerCase() === "reduce"
  );
}

install();
if (typeof window !== "undefined") forced = hashForcesReducedMotion(window.location.hash);
syncAttribute();
