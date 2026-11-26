import { AgentMark } from "@o7/ui/graphite";
import { useEffect, useState, useSyncExternalStore } from "react";
import type {
  AgentEntry,
  AppInfo,
  Autonomy,
  ConfigChoice,
  DetectedEditors,
  ProjectEditor,
} from "../../bindings";
import { getThemePreference } from "../../lib/appearance";
import { AUTONOMIES, autonomyLabel, levelName } from "../../lib/chat/present";
import { chatInterfaces } from "../../lib/chatInterface";
import { editorName } from "../../lib/projectTools";
import { DEFAULT_GENERAL, type GeneralApi } from "../../lib/setup";
import { playNotice } from "../../lib/sounds";
import { TERMINAL_SIZES, terminalFonts } from "../../lib/terminalFont";
import { Select } from "../Select";
import type { Preferences } from "./preferences";
import { Row, Switch } from "./SettingsRow";
import { ThemeGallery } from "./ThemeGallery";

/** One line under Autonomy for the level chosen. */
export function autonomyDetail(autonomy: Autonomy): string {
  switch (autonomy) {
    case "full_auto":
      return "Agents work without asking, within their own rules.";
    case "auto_edit":
      return "Each agent applies it within its own rules and asks for the rest.";
    case "ask":
      return "Agents check with you before they act.";
  }
}

/**
 * The agents Settings offers as the default: those that can run here, and the saved one even when
 * it is not found now, so the choice never silently changes.
 */
export function defaultAgentChoices(
  agents: readonly AgentEntry[],
  saved: string | null,
): { id: string; name: string }[] {
  const usable = agents
    .filter((agent) => agent.availability.state === "ready" || agent.id === saved)
    .map((agent) => ({
      id: agent.id,
      name: agent.availability.state === "ready" ? agent.name : `${agent.name} (not found)`,
    }));
  if (saved && !usable.some((agent) => agent.id === saved))
    usable.push({ id: saved, name: `${saved} (not found)` });
  return usable;
}

/** The models offered for the default agent: what it reported, and the saved one if it is not. */
export function defaultModelChoices(
  reported: readonly ConfigChoice[],
  saved: string | null,
): { value: string; name: string }[] {
  const choices = reported.map((choice) => ({ value: choice.value, name: choice.name }));
  if (saved && !choices.some((choice) => choice.value === saved))
    choices.push({ value: saved, name: saved });
  return choices;
}

/**
 * The thinking levels offered for the default model: what it offered (its own "default" is the
 * select's first entry already), and the saved one if it is not among them.
 */
export function defaultLevelChoices(
  reported: readonly ConfigChoice[],
  saved: string | null,
): { value: string; name: string }[] {
  const choices = reported
    .filter((choice) => choice.value !== "default")
    .map((choice) => ({ value: choice.value, name: levelName(choice) }));
  if (saved && !choices.some((choice) => choice.value === saved))
    choices.push({ value: saved, name: levelName({ value: saved, name: saved }) });
  return choices;
}

/**
 * Settings' General page: what new chats start with, what o7 does while agents work in the
 * background, and setup. One control per row; each change shows at once and saves in turn.
 */
export function GeneralPage({
  info,
  prefs,
  lists,
  onSetup,
  onTour,
}: {
  readonly info: AppInfo;
  readonly prefs: Preferences;
  readonly lists: GeneralApi;
  readonly onSetup: () => void;
  /** Takes the tour. Without it (a view with no chats behind it) the row is not offered. */
  readonly onTour?: () => void;
}) {
  const [agents, setAgents] = useState<readonly AgentEntry[]>([]);
  const [models, setModels] = useState<readonly ConfigChoice[]>([]);
  const [levels, setLevels] = useState<readonly ConfigChoice[]>([]);
  useEffect(() => {
    let live = true;
    lists.agents().then(
      (value) => {
        if (live) setAgents(value);
      },
      () => {
        // The agent list is a convenience here; without it the saved choice still shows.
      },
    );
    return () => {
      live = false;
    };
  }, [lists]);
  const general = prefs.shown?.general ?? DEFAULT_GENERAL;
  const agentId = general.default_agent;
  useEffect(() => {
    let live = true;
    setModels([]);
    if (agentId)
      lists.models(agentId).then(
        (value) => {
          if (live) setModels(value);
        },
        () => {
          // No running chat of this agent yet: it has not listed its models.
        },
      );
    return () => {
      live = false;
    };
  }, [lists, agentId]);
  const defaultModel = general.default_model;
  useEffect(() => {
    let live = true;
    setLevels([]);
    if (agentId)
      lists.thinking(agentId, defaultModel).then(
        (value) => {
          if (live) setLevels(value);
        },
        () => {
          // Not known until the agent has run that model.
        },
      );
    return () => {
      live = false;
    };
  }, [lists, agentId, defaultModel]);
  // Only loading disables a control: a save in flight never takes keyboard focus away.
  const disabled = !prefs.saved;
  const agentChoices = defaultAgentChoices(agents, agentId);
  const agentName = agentChoices.find((agent) => agent.id === agentId)?.name ?? agentId ?? "";
  const modelChoices = defaultModelChoices(models, general.default_model);
  const levelChoices = defaultLevelChoices(levels, general.default_effort);
  const tray = info.os === "macos" ? "menu bar" : "tray";
  // The window cannot make a sound here (Linux without GStreamer's plugins): Sounds says why once,
  // and its switch waits until it can.
  const silent = info.media === false;
  return (
    <>
      <section className="settings-section" aria-labelledby="settings-new-chats">
        <h2 id="settings-new-chats">New chats</h2>
        <div className="settings-group">
          <Row title="Agent" detail="New chat starts with this agent selected.">
            <span className="settings-control settings-agent-choice">
              {/* The chosen agent's mark; the select's own text names it. */}
              {agentId ? (
                <span className="settings-agent-mark" aria-hidden="true">
                  <AgentMark agent={agentId} size={16} />
                </span>
              ) : null}
              <Select
                className="g-select settings-select"
                aria-label="Default agent"
                data-testid="settings-default-agent"
                value={agentId ?? ""}
                disabled={disabled}
                onChange={(event) =>
                  prefs.change({
                    type: "default_agent",
                    agent_id: event.currentTarget.value || null,
                  })
                }
              >
                <option value="">Choose each time</option>
                {agentChoices.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </Select>
            </span>
          </Row>
          {agentId ? (
            <Row
              title="Model"
              detail={
                modelChoices.length > 0
                  ? `New ${agentName} chats start on this model.`
                  : `${agentName} lists its models once one of its chats starts.`
              }
            >
              {modelChoices.length > 0 ? (
                <Select
                  className="g-select settings-control settings-select"
                  aria-label="Default model"
                  data-testid="settings-default-model"
                  value={general.default_model ?? ""}
                  disabled={disabled}
                  onChange={(event) =>
                    prefs.change({
                      type: "default_model",
                      model: event.currentTarget.value || null,
                    })
                  }
                >
                  <option value="">The agent's default</option>
                  {modelChoices.map((model) => (
                    <option key={model.value} value={model.value}>
                      {model.name}
                    </option>
                  ))}
                </Select>
              ) : null}
            </Row>
          ) : null}
          {agentId && (levelChoices.length > 0 || modelChoices.length > 0) ? (
            <Row
              title="Thinking"
              detail={
                levelChoices.length > 0
                  ? `New ${agentName} chats start at this thinking level.`
                  : `${agentName} lists this model's thinking levels once it has run it.`
              }
            >
              {levelChoices.length > 0 ? (
                <Select
                  className="g-select settings-control settings-select"
                  aria-label="Default thinking level"
                  data-testid="settings-default-thinking"
                  value={general.default_effort ?? ""}
                  disabled={disabled}
                  onChange={(event) =>
                    prefs.change({
                      type: "default_effort",
                      effort: event.currentTarget.value || null,
                    })
                  }
                >
                  <option value="">The model's default</option>
                  {levelChoices.map((level) => (
                    <option key={level.value} value={level.value}>
                      {level.name}
                    </option>
                  ))}
                </Select>
              ) : null}
            </Row>
          ) : null}
          <Row title="Autonomy" detail={autonomyDetail(general.default_autonomy)}>
            <fieldset className="settings-segmented settings-control" disabled={disabled}>
              <legend className="sr-only">Autonomy</legend>
              {[...AUTONOMIES].reverse().map((value) => (
                <label key={value}>
                  <input
                    type="radio"
                    name="settings-autonomy"
                    value={value}
                    checked={general.default_autonomy === value}
                    onChange={() => prefs.change({ type: "default_autonomy", value })}
                  />
                  <span>{autonomyLabel(value)}</span>
                </label>
              ))}
            </fieldset>
          </Row>
        </div>
        <p className="settings-footnote">
          Applies to new chats. A project's own model comes first.
        </p>
      </section>
      <section className="settings-section" aria-labelledby="settings-background">
        <h2 id="settings-background">While agents work</h2>
        <div className="settings-group">
          <Row
            title="Notifications"
            detail="When a chat you are not looking at finishes, fails or needs you."
          >
            <Switch
              label="Notifications"
              testId="settings-notifications"
              checked={general.notifications}
              disabled={disabled}
              onChange={(enabled) => prefs.change({ type: "notifications", enabled })}
            />
          </Row>
          <Row
            title="Sounds"
            detail={
              silent
                ? (info.media_notice ?? "Sounds cannot play here.")
                : "A short sound when a chat finishes, fails or needs you."
            }
          >
            <Switch
              label="Sounds"
              testId="settings-sounds"
              checked={general.sounds}
              disabled={disabled || silent}
              onChange={(enabled) => {
                // Turning them on plays one, so the choice is heard as it is made.
                if (enabled) playNotice("finished");
                prefs.change({ type: "sounds", enabled });
              }}
            />
          </Row>
          <Row
            title="Keep awake"
            detail="No idle sleep while an agent works. The screen can still turn off."
          >
            <Switch
              label="Keep awake"
              testId="settings-keep-awake"
              checked={general.keep_awake}
              disabled={disabled}
              onChange={(enabled) => prefs.change({ type: "keep_awake", enabled })}
            />
          </Row>
          <Row
            title={`Keep running in the ${tray}`}
            setting="Keep running in the background"
            detail="Closing the window leaves agents working. Off, closing quits."
          >
            <Switch
              label={`Keep running in the ${tray}`}
              testId="settings-close-to-tray"
              checked={general.close_to_tray}
              disabled={disabled}
              onChange={(enabled) => prefs.change({ type: "close_to_tray", enabled })}
            />
          </Row>
          <Row
            title="Check for updates"
            setting="Check for updates automatically"
            detail="Looks for a newer o7 a few times a day and shows Update. Nothing installs until you choose."
          >
            <Switch
              label="Check for updates automatically"
              testId="settings-automatic-updates"
              checked={general.automatic_updates}
              disabled={disabled}
              onChange={(enabled) => prefs.change({ type: "automatic_updates", enabled })}
            />
          </Row>
        </div>
      </section>
      <section className="settings-section" aria-labelledby="settings-setup">
        <h2 id="settings-setup">Setup</h2>
        <div className="settings-group">
          <Row
            title="Welcome and setup"
            detail="Review your agents and pick a first project again."
          >
            <button
              className="g-button g-button-outline g-button-28"
              type="button"
              onClick={onSetup}
            >
              Run setup
            </button>
          </Row>
          {onTour ? (
            <Row title="Tour" detail="The wing shows you around o7. About a minute.">
              <button
                className="g-button g-button-outline g-button-28"
                type="button"
                data-testid="settings-tour"
                onClick={onTour}
              >
                Take the tour
              </button>
            </Row>
          ) : null}
        </div>
      </section>
      {prefs.saved?.notice ? <p className="settings-footnote">{prefs.saved.notice}</p> : null}
      <PreferencesError prefs={prefs} />
    </>
  );
}

/** Settings' Appearance page: light or dark, the theme, motion and text size. */
export function AppearancePage({ prefs }: { readonly prefs: Preferences }) {
  const shown = prefs.shown;
  const disabled = !prefs.saved;
  return (
    <>
      <div className="settings-group">
        <Row title="Appearance" detail="System follows your device's light or dark appearance.">
          <fieldset className="settings-segmented settings-control" disabled={disabled}>
            <legend className="sr-only">Appearance</legend>
            {(["system", "light", "dark"] as const).map((value) => (
              <label key={value}>
                <input
                  type="radio"
                  name="settings-appearance"
                  value={value}
                  checked={(shown?.appearance ?? "system") === value}
                  onChange={() => prefs.change({ type: "appearance", value })}
                />
                <span>{value === "system" ? "System" : value === "light" ? "Light" : "Dark"}</span>
              </label>
            ))}
          </fieldset>
        </Row>
        <Row
          title="Theme"
          detail="Main is o7's own. Each agent's theme carries its colours through the whole app, in light and dark."
        >
          <ThemeGallery
            value={shown?.theme ?? getThemePreference()}
            disabled={disabled}
            onChange={(value) => prefs.change({ type: "theme", value })}
          />
        </Row>
        <Row title="Motion" detail="Full still follows your system's reduced motion setting.">
          <fieldset className="settings-segmented settings-control" disabled={disabled}>
            <legend className="sr-only">Motion</legend>
            {(["full", "reduced", "off"] as const).map((value) => (
              <label key={value}>
                <input
                  type="radio"
                  name="settings-motion"
                  value={value}
                  checked={shown?.motion === value}
                  onChange={() => prefs.change({ type: "motion", value })}
                />
                <span>{value === "full" ? "Full" : value === "reduced" ? "Reduced" : "Off"}</span>
              </label>
            ))}
          </fieldset>
        </Row>
        <Row
          title="Text size"
          detail="Compact fits more of a chat on screen. Comfortable sets its text larger."
        >
          <fieldset
            className="settings-segmented settings-control"
            disabled={disabled}
            data-testid="settings-text-size"
          >
            <legend className="sr-only">Text size</legend>
            {(["compact", "comfortable"] as const).map((value) => (
              <label key={value}>
                <input
                  type="radio"
                  name="settings-text-size"
                  value={value}
                  checked={(shown?.text_size ?? "compact") === value}
                  onChange={() => prefs.change({ type: "text_size", value })}
                />
                <span>{value === "compact" ? "Compact" : "Comfortable"}</span>
              </label>
            ))}
          </fieldset>
        </Row>
        <TerminalFontRows />
      </div>
      <PreferencesError prefs={prefs} />
    </>
  );
}

/** "JetBrainsMono Nerd Font, 11 pt": what the person's terminal app uses, as its settings say it. */
function systemFontText(): string | null {
  const system = terminalFonts.system();
  if (!system) return null;
  const family = system.family === "ui-monospace" ? "SF Mono" : system.family;
  const size =
    system.size_points === null ? null : `${Math.round(system.size_points * 10) / 10} pt`;
  return [family, size].filter(Boolean).join(", ") || null;
}

/**
 * The terminals' font and size, this device's choice (lib/terminalFont.ts): the person's own
 * terminal app's by default, or o7's.
 */
function TerminalFontRows() {
  useSyncExternalStore(terminalFonts.subscribe, terminalFonts.getVersion, terminalFonts.getVersion);
  useEffect(() => {
    void terminalFonts.load();
  }, []);
  const preference = terminalFonts.preference();
  const system = terminalFonts.system();
  const systemText = systemFontText();
  const matching = system ? `Your terminal (${system.source})` : "Your terminal";
  return (
    <>
      <Row
        title="Terminal font"
        detail={
          preference.font === "system"
            ? systemText
              ? `Terminals and terminal chats draw like your own terminal app: ${systemText}.`
              : "Terminals draw like your own terminal app once o7 finds its font; until then they use o7's."
            : "Terminals use o7's Geist Mono."
        }
      >
        <Select
          className="g-select settings-control settings-select"
          aria-label="Terminal font"
          data-testid="settings-terminal-font"
          value={preference.font}
          onChange={(event) =>
            terminalFonts.set({
              ...preference,
              font: event.currentTarget.value === "auracode" ? "auracode" : "system",
            })
          }
        >
          <option value="system">{matching}</option>
          <option value="auracode">o7's</option>
        </Select>
      </Row>
      <Row title="Terminal size" detail="The size of text in terminals and terminal chats.">
        <Select
          className="g-select settings-control settings-select"
          aria-label="Terminal size"
          data-testid="settings-terminal-size"
          value={preference.size === null ? "auto" : String(preference.size)}
          onChange={(event) => {
            const value = Number(event.currentTarget.value);
            terminalFonts.set({
              ...preference,
              size: event.currentTarget.value === "auto" || !Number.isFinite(value) ? null : value,
            });
          }}
        >
          <option value="auto">
            {preference.font === "system" ? "Same as the font" : "Default"}
          </option>
          {TERMINAL_SIZES.map((size) => (
            <option key={size} value={String(size)}>
              {size}
            </option>
          ))}
        </Select>
      </Row>
    </>
  );
}

/**
 * Settings' Chats page: whether finished turns fold, the editor a chat's files open in, and
 * checkpoints before each message.
 */
export function ChatsPage({
  prefs,
  editors,
}: {
  readonly prefs: Preferences;
  /** The installed editors; without it the page has no editor row. */
  readonly editors?: () => Promise<DetectedEditors>;
}) {
  const general = prefs.shown?.general ?? DEFAULT_GENERAL;
  const disabled = !prefs.saved;
  const [installed, setInstalled] = useState<readonly ProjectEditor[] | null>(null);
  // New chats' interface is this device's choice (lib/chatInterface.ts), not a saved preference.
  useSyncExternalStore(
    chatInterfaces.subscribe,
    chatInterfaces.getVersion,
    chatInterfaces.getVersion,
  );
  const newChatInterface = chatInterfaces.defaultInterface();
  useEffect(() => {
    let live = true;
    editors?.().then(
      (found) => {
        if (live) setInstalled(found.editors);
      },
      () => {
        if (live) setInstalled([]);
      },
    );
    return () => {
      live = false;
    };
  }, [editors]);
  // A saved editor that is no longer installed shows as the one files now open in.
  const chosen =
    installed && general.editor !== null && installed.includes(general.editor)
      ? general.editor
      : (installed?.[0] ?? null);
  return (
    <>
      {/* Four choices, one list: a heading over each single row only made the page longer. */}
      <div className="settings-group">
        <Row
          title="Open new chats in"
          detail={
            newChatInterface === "terminal"
              ? "Agents open in their own interface, in o7's terminal. Any chat switches back from its header. This device only."
              : "o7's chat. Any chat can switch to its agent's own interface from the header."
          }
        >
          <Select
            className="g-select settings-control settings-select"
            aria-label="Open new chats in"
            data-testid="settings-new-chat-interface"
            value={newChatInterface}
            onChange={(event) =>
              chatInterfaces.setDefault(
                event.currentTarget.value === "terminal" ? "terminal" : "chat",
              )
            }
          >
            <option value="chat">Chat</option>
            <option value="terminal">Terminal</option>
          </Select>
        </Row>
        {installed ? (
          <Row
            title="Open files in"
            detail={
              chosen
                ? "File names in answers, steps and Changes open in this editor, at their line."
                : "No supported editor was found in its standard location (Cursor, Visual Studio Code, Zed or a JetBrains IDE). File names stay text until one is installed."
            }
          >
            {chosen ? (
              <Select
                className="g-select settings-control settings-select"
                aria-label="Open files in"
                data-testid="settings-editor"
                value={chosen}
                disabled={disabled}
                onChange={(event) =>
                  prefs.change({
                    type: "editor",
                    editor: event.currentTarget.value as ProjectEditor,
                  })
                }
              >
                {installed.map((editor) => (
                  <option key={editor} value={editor}>
                    {editorName[editor]}
                  </option>
                ))}
              </Select>
            ) : null}
          </Row>
        ) : null}
        <Row
          title="Keep steps open"
          detail={
            general.keep_steps_open
              ? "A finished turn shows its messages, tools and thinking above its answer."
              : "A finished turn folds its work under Worked for, above its answer."
          }
        >
          <Switch
            label="Keep steps open"
            testId="settings-keep-steps-open"
            checked={general.keep_steps_open}
            disabled={disabled}
            onChange={(enabled) => prefs.change({ type: "keep_steps_open", enabled })}
          />
        </Row>
        <Row
          title="Automatic checkpoints"
          detail={
            general.automatic_checkpoints
              ? "Save workspace files before each message for per-turn file undo in Changes. Saving delays sending."
              : "Send without saving a checkpoint first. Aura's per-turn file undo is unavailable for new messages."
          }
        >
          <Switch
            label="Automatic checkpoints"
            testId="settings-automatic-checkpoints"
            checked={general.automatic_checkpoints}
            disabled={disabled}
            onChange={(enabled) => prefs.change({ type: "automatic_checkpoints", enabled })}
          />
        </Row>
      </div>
      <p className="settings-footnote">
        Checkpoints apply from the next message in this profile. Saved checkpoints remain available
        in Changes.
      </p>
      <PreferencesError prefs={prefs} />
    </>
  );
}

/** A preference that would not load or save, with the way back to what is saved. */
export function PreferencesError({ prefs }: { readonly prefs: Preferences }) {
  if (!prefs.error) return null;
  return (
    <div className="settings-error" role="alert">
      <p>{prefs.error}</p>
      <button className="g-button g-button-28" type="button" onClick={prefs.reload}>
        Reload preferences
      </button>
    </div>
  );
}
