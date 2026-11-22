import { setMotionPreference } from "@o7/ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SetupAction, SetupPreferences, SetupSnapshot } from "../../bindings";
import {
  setAppearancePreference,
  setTextSizePreference,
  setThemePreference,
} from "../../lib/appearance";
import { setKeepStepsOpen } from "../../lib/chat/stepsPreference";
import { errorMessage } from "../../lib/errors";
import type { SetupApi } from "../../lib/setup";

/**
 * The profile's preferences for the Settings pages, saved one change at a time. A change shows at
 * once, and the appearance, theme, motion and text size apply at once too; no control is disabled
 * while a save runs, so keyboard focus never drops from a radio, a switch or the theme gallery
 * (owner's plan, 4 November 2026). The latest choice for a setting replaces one still waiting. A
 * failed save returns every waiting choice to what is saved, beside the error.
 */
export interface Preferences {
  readonly saved: SetupSnapshot | null;
  /** What the pages show: the saved preferences with the choices still saving applied. */
  readonly shown: SetupPreferences | null;
  readonly error: string | null;
  change(action: SetupAction): void;
  reload(): void;
}

/** One waiting change per setting: a later choice for it replaces the earlier one. */
function slot(action: SetupAction): string {
  return action.type;
}

/** The preferences as they will be once `action` saves, as the backend applies it. */
export function withAction(preferences: SetupPreferences, action: SetupAction): SetupPreferences {
  const general = preferences.general;
  switch (action.type) {
    case "appearance":
      return { ...preferences, appearance: action.value };
    case "theme":
      return { ...preferences, theme: action.value };
    case "motion":
      return { ...preferences, motion: action.value };
    case "text_size":
      return { ...preferences, text_size: action.value };
    case "default_agent":
      // A new agent clears the model and thinking level, which belonged to the previous one.
      return general.default_agent === action.agent_id
        ? preferences
        : {
            ...preferences,
            general: {
              ...general,
              default_agent: action.agent_id,
              default_model: null,
              default_effort: null,
            },
          };
    case "default_model":
      return {
        ...preferences,
        general: {
          ...general,
          default_model: action.model,
          default_effort: general.default_model === action.model ? general.default_effort : null,
        },
      };
    case "default_effort":
      return { ...preferences, general: { ...general, default_effort: action.effort } };
    case "default_autonomy":
      return { ...preferences, general: { ...general, default_autonomy: action.value } };
    case "automatic_checkpoints":
      return { ...preferences, general: { ...general, automatic_checkpoints: action.enabled } };
    case "notifications":
      return { ...preferences, general: { ...general, notifications: action.enabled } };
    case "sounds":
      return { ...preferences, general: { ...general, sounds: action.enabled } };
    case "editor":
      return { ...preferences, general: { ...general, editor: action.editor } };
    case "keep_awake":
      return { ...preferences, general: { ...general, keep_awake: action.enabled } };
    case "close_to_tray":
      return { ...preferences, general: { ...general, close_to_tray: action.enabled } };
    case "keep_steps_open":
      return { ...preferences, general: { ...general, keep_steps_open: action.enabled } };
    case "copy_instructions_to_worktrees":
      return {
        ...preferences,
        general: { ...general, copy_instructions_to_worktrees: action.enabled },
      };
    case "agent_memory_writes":
      return { ...preferences, general: { ...general, agent_memory_writes: action.enabled } };
    case "browser":
      return { ...preferences, general: { ...general, browser: action.enabled } };
    case "browser_passwords":
      return { ...preferences, general: { ...general, browser_passwords: action.enabled } };
    case "mobile":
      return { ...preferences, general: { ...general, mobile: action.enabled } };
    case "automatic_updates":
      return { ...preferences, general: { ...general, automatic_updates: action.enabled } };
    case "coordinated_projects":
      return { ...preferences, general: { ...general, coordinated_projects: action.enabled } };
    case "speech_voice":
      return { ...preferences, general: { ...general, speech_voice: action.voice } };
    default:
      return preferences;
  }
}

/** Paints what the pages show, so a choice applies before its save returns. */
function paint(preferences: SetupPreferences): void {
  setAppearancePreference(preferences.appearance);
  setThemePreference(preferences.theme);
  setTextSizePreference(preferences.text_size);
  setMotionPreference(preferences.motion);
  setKeepStepsOpen(preferences.general.keep_steps_open);
}

export function usePreferences(api: SetupApi): Preferences {
  const [saved, setSaved] = useState<SetupSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloads, setReloads] = useState(0);
  // The change saving now and those waiting, in the order they were made.
  const [overlay, setOverlay] = useState<readonly SetupAction[]>([]);
  const savedRef = useRef<SetupSnapshot | null>(null);
  const inFlight = useRef<SetupAction | null>(null);
  const waiting = useRef(new Map<string, SetupAction>());
  const running = useRef(false);
  const apiRef = useRef(api);
  apiRef.current = api;

  // After a change or a save: what the pages show, painted at once.
  const refresh = useCallback(() => {
    const actions = [...(inFlight.current ? [inFlight.current] : []), ...waiting.current.values()];
    setOverlay(actions);
    const base = savedRef.current?.preferences;
    if (base) paint(actions.reduce(withAction, base));
  }, []);

  useEffect(() => {
    void reloads;
    let live = true;
    api.snapshot().then(
      (next) => {
        if (!live) return;
        savedRef.current = next;
        setSaved(next);
        setError(null);
      },
      (failure) => {
        if (live) setError(errorMessage(failure));
      },
    );
    return () => {
      live = false;
    };
  }, [api, reloads]);

  const drain = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    try {
      for (;;) {
        const next = waiting.current.entries().next();
        if (next.done || !savedRef.current) break;
        const [key, action] = next.value;
        waiting.current.delete(key);
        inFlight.current = action;
        try {
          const snapshot = await apiRef.current.apply(
            savedRef.current.preferences.revision,
            action,
          );
          savedRef.current = snapshot;
          inFlight.current = null;
          setSaved(snapshot);
          setError(null);
        } catch (failure) {
          // Everything still waiting returns to what is saved, beside the error.
          inFlight.current = null;
          waiting.current.clear();
          setError(errorMessage(failure));
        }
        refresh();
      }
    } finally {
      running.current = false;
    }
  }, [refresh]);

  const change = useCallback(
    (action: SetupAction) => {
      if (!savedRef.current) return;
      const key = slot(action);
      waiting.current.delete(key);
      waiting.current.set(key, action);
      refresh();
      void drain();
    },
    [drain, refresh],
  );

  const shown = useMemo(
    () => (saved ? overlay.reduce(withAction, saved.preferences) : null),
    [saved, overlay],
  );
  return {
    saved,
    shown,
    error,
    change,
    reload: () => setReloads((value) => value + 1),
  };
}
