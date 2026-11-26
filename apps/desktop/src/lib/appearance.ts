import { THEME_IDS } from "@o7/ui/graphite";
import type { AppearancePreference, TextSizePreference, ThemePreference } from "../bindings";

export type ResolvedAppearance = "light" | "dark";
const PREVIEW_KEY = "auracode.preview.appearance";
const PALETTE_PREVIEW_KEY = "auracode.preview.palette";

export function isTheme(value: unknown): value is ThemePreference {
  return typeof value === "string" && (THEME_IDS as readonly string[]).includes(value);
}

/**
 * The theme a profile has until it chooses one, and what a missing or unknown saved theme reads
 * as: Main (owner, 25 November 2026), which is saved as "aura", the name its palette had before.
 * A saved choice is kept.
 */
export const DEFAULT_THEME: ThemePreference = "aura";

let theme: ThemePreference = DEFAULT_THEME;
const themeListeners = new Set<() => void>();

/** Main is the base palette itself and sets no attribute; every other theme names its own. */
function paintPalette(next: ThemePreference): void {
  if (typeof document === "undefined") return;
  if (next === "aura") delete document.documentElement.dataset.palette;
  else document.documentElement.dataset.palette = next;
}

/**
 * The desktop theme: a palette layer over the tokens for whichever appearance is resolved
 * (themes.css).
 */
export function setThemePreference(next: ThemePreference): void {
  theme = next;
  paintPalette(next);
  for (const listener of themeListeners) listener();
}

/**
 * The chat's text size (Settings > General > Text size). Compact, the default, is the tokens
 * themselves and sets no attribute; Comfortable sets the chat at the prose size (tokens.css).
 */
export function setTextSizePreference(next: TextSizePreference): void {
  textSize = next;
  if (typeof document === "undefined") return;
  if (next === "comfortable") document.documentElement.dataset.textSize = "comfortable";
  else delete document.documentElement.dataset.textSize;
}

let textSize: TextSizePreference = "compact";

/** The text size in effect; previews seed their saved preferences with it, as with the theme. */
export function getTextSizePreference(): TextSizePreference {
  initializeAppearance();
  return textSize;
}

export function getThemePreference(): ThemePreference {
  initializeAppearance();
  return theme;
}

export function subscribeTheme(listener: () => void): () => void {
  themeListeners.add(listener);
  return () => {
    themeListeners.delete(listener);
  };
}

export function isAppearance(value: unknown): value is AppearancePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function resolveAppearance(
  preference: AppearancePreference,
  systemDark: boolean,
): ResolvedAppearance {
  return preference === "system" ? (systemDark ? "dark" : "light") : preference;
}

type Media = Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener">;

/** The OS subscription stays live, but an explicit preference never follows OS changes. */
export function createAppearanceController(
  media: Media,
  initial: AppearancePreference,
  apply: (resolved: ResolvedAppearance) => void,
  persist: (preference: AppearancePreference) => void = () => {},
) {
  let preference = initial;
  let resolved = resolveAppearance(preference, media.matches);
  const listeners = new Set<() => void>();
  apply(resolved);
  const update = () => {
    const next = resolveAppearance(preference, media.matches);
    if (next === resolved) return;
    resolved = next;
    apply(resolved);
    for (const listener of listeners) listener();
  };
  media.addEventListener("change", update);
  return {
    preference: () => preference,
    resolved: () => resolved,
    set: (next: AppearancePreference) => {
      preference = next;
      persist(next);
      update();
    },
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    dispose: () => {
      media.removeEventListener("change", update);
      listeners.clear();
    },
  };
}

let controller: ReturnType<typeof createAppearanceController> | undefined;
let appliedRevision = -1;

/** Call before mounting any view. Production preferences only live in the active Rust profile. */
export function initializeAppearance(): void {
  if (controller || typeof window === "undefined") return;
  const query = new URLSearchParams(window.location.search);
  const preview = import.meta.env.DEV && query.has("preview");
  let initial: AppearancePreference = "system";
  if (preview) {
    let saved: string | null = null;
    try {
      saved = window.localStorage.getItem(PREVIEW_KEY);
    } catch {
      /* Storage may be disabled. */
    }
    const override = query.get("theme");
    initial = isAppearance(override) ? override : isAppearance(saved) ? saved : "system";
    // `&palette=claude` previews a theme; the choice is kept like the appearance's.
    let savedPalette: string | null = null;
    try {
      savedPalette = window.localStorage.getItem(PALETTE_PREVIEW_KEY);
    } catch {
      /* Storage may be disabled. */
    }
    const palette = query.get("palette");
    setThemePreference(
      isTheme(palette) ? palette : isTheme(savedPalette) ? savedPalette : DEFAULT_THEME,
    );
    // `&textSize=comfortable` previews the larger chat text.
    if (query.get("textSize") === "comfortable") setTextSizePreference("comfortable");
  } else {
    // The first frame already wears the default theme; the profile's choice follows it.
    paintPalette(theme);
  }
  controller = createAppearanceController(
    window.matchMedia("(prefers-color-scheme: dark)"),
    initial,
    (resolved) => {
      document.documentElement.dataset.theme = resolved;
      document.documentElement.style.colorScheme = resolved;
    },
    (preference) => {
      if (!preview) return;
      try {
        window.localStorage.setItem(PREVIEW_KEY, preference);
      } catch {
        /* Keep the in-memory choice. */
      }
    },
  );
}

/** The appearance in effect now: System resolved against the device. */
export function getResolvedAppearance(): ResolvedAppearance {
  initializeAppearance();
  return controller?.resolved() ?? "dark";
}

export function getAppearancePreference(): AppearancePreference {
  initializeAppearance();
  return controller?.preference() ?? "system";
}

export function setAppearancePreference(preference: AppearancePreference): void {
  initializeAppearance();
  controller?.set(preference);
}

/** Ignore a slower snapshot response from before the user's latest saved preference. */
export function applySetupAppearance(preferences: {
  readonly appearance: AppearancePreference;
  readonly theme?: ThemePreference;
  readonly text_size?: TextSizePreference;
  readonly revision: number;
}): void {
  if (preferences.revision < appliedRevision) return;
  appliedRevision = preferences.revision;
  setAppearancePreference(preferences.appearance);
  setThemePreference(isTheme(preferences.theme) ? preferences.theme : DEFAULT_THEME);
  setTextSizePreference(preferences.text_size === "comfortable" ? "comfortable" : "compact");
}

/** Previews keep the chosen theme across reloads, as they keep the appearance. */
export function rememberPreviewTheme(next: ThemePreference): void {
  try {
    window.localStorage.setItem(PALETTE_PREVIEW_KEY, next);
  } catch {
    /* Keep the in-memory choice. */
  }
}

export function subscribeAppearance(listener: () => void): () => void {
  initializeAppearance();
  return controller?.subscribe(listener) ?? (() => {});
}
