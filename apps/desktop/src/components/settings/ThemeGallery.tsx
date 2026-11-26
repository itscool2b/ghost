import { AgentMark, GRAPHITE_THEMES, Logo, THEMES, themeTokens } from "@o7/ui/graphite";
import { type CSSProperties, useSyncExternalStore } from "react";
import type { ThemePreference } from "../../bindings";
import { getResolvedAppearance, subscribeAppearance } from "../../lib/appearance";

/** What a swatch paints, from a theme's own tokens in the appearance in effect. */
function swatch(id: ThemePreference, appearance: "dark" | "light"): CSSProperties {
  const theme = THEMES.find((candidate) => candidate.id === id);
  const tokens: Readonly<Record<string, string>> = theme
    ? themeTokens(theme[appearance], appearance)
    : GRAPHITE_THEMES[appearance];
  const value = (name: string) => tokens[name] ?? "";
  return {
    "--swatch-bg": value("bg"),
    "--swatch-card": value("surface-2"),
    "--swatch-fg": value("fg-1"),
    "--swatch-muted": value("fg-3"),
    "--swatch-accent": value("primary"),
    "--swatch-mark": value("volt"),
  } as CSSProperties;
}

/**
 * The theme picker: Main and one theme per supported agent, each a small window painted in its
 * own colours for the appearance in effect, with the agent's mark. A radio group: arrow keys move
 * between themes, and the choice applies at once.
 */
export function ThemeGallery({
  value,
  disabled,
  onChange,
}: {
  readonly value: ThemePreference;
  readonly disabled: boolean;
  readonly onChange: (value: ThemePreference) => void;
}) {
  const appearance = useSyncExternalStore(
    subscribeAppearance,
    getResolvedAppearance,
    getResolvedAppearance,
  );
  const choices: readonly { id: ThemePreference; name: string; agent: string | null }[] = [
    { id: "aura", name: "Main", agent: null },
    ...THEMES.map((theme) => ({ id: theme.id, name: theme.name, agent: theme.agent })),
  ];
  return (
    <fieldset className="settings-themes" disabled={disabled} data-testid="settings-themes">
      <legend className="sr-only">Theme</legend>
      {choices.map((choice) => (
        <label key={choice.id} className="settings-theme" data-testid={`theme-${choice.id}`}>
          <input
            type="radio"
            className="sr-only"
            name="settings-theme"
            value={choice.id}
            checked={value === choice.id}
            onChange={() => onChange(choice.id)}
          />
          <span
            className="settings-theme-swatch"
            style={swatch(choice.id, appearance)}
            aria-hidden="true"
          >
            <span className="settings-theme-card">
              <span className="settings-theme-line" />
              <span className="settings-theme-line" data-short="" />
            </span>
            <span className="settings-theme-pill" />
            <span className="settings-theme-dot" />
          </span>
          <span className="settings-theme-name">
            {/* The name alone labels the choice; the mark would read "Claude Code, Idle". */}
            <span className="settings-theme-mark" aria-hidden="true">
              {choice.agent ? <AgentMark agent={choice.agent} size={16} /> : <Logo size={20} />}
            </span>
            <span className="settings-theme-label">{choice.name}</span>
          </span>
        </label>
      ))}
    </fieldset>
  );
}
