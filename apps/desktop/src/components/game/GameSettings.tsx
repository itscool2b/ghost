import { Gamepad2 } from "lucide-react";
import { useSyncExternalStore } from "react";
import { type GameStore, megabytes, gameStore as nativeGameStore } from "../../lib/game";
import type { Preferences } from "../settings/preferences";
import { Switch } from "../settings/SettingsRow";
import "../library/plugin-library.css";

/** What an agent can do with the chat's game, in a person's words. The tool names are Ghost's. */
const AGENT_TOOLS: readonly (readonly [string, string])[] = [
  ["game_status", "See what the project is"],
  ["game_check", "Check the scripts"],
  ["game_run", "Start the game"],
  ["game_stop", "Stop the game"],
  ["game_look", "Look at the game"],
  ["game_tree", "Read the scene"],
  ["game_inspect", "Inspect an object"],
  ["game_set", "Try a value"],
  ["game_act", "Play with keys and mouse"],
  ["game_wait_for", "Wait for something"],
  ["game_logs", "Read errors and output"],
  ["game_eval", "Run code in the game"],
  ["game_perf", "Measure speed"],
  ["game_time", "Freeze and resume"],
];

/**
 * Game, the universal plugin (docs/game.md): Ghost's own, for every agent, like Browser use and
 * Mobile. It comes with Ghost and cannot be added or removed, only turned off, which stops every
 * game Ghost started and takes the Game pane out of the app.
 */
export function GameSettings({
  prefs,
  game = nativeGameStore,
}: {
  readonly prefs: Preferences;
  readonly game?: GameStore;
}) {
  useSyncExternalStore(game.subscribe, game.getVersion, game.getVersion);
  const status = game.status();
  const general = prefs.shown?.general ?? null;
  const on = general?.game ?? true;
  const godot = status?.godot ?? null;
  const running = status?.chats.filter((chat) => chat.state === "running").length ?? 0;
  return (
    <div className="plugins-agent plugins-universal" data-testid="game-settings">
      {prefs.error ? (
        <p className="library-feedback" role="alert">
          {prefs.error}
        </p>
      ) : null}
      <section className="plugins-universal-card" aria-labelledby="plugins-universal-game">
        <div className="plugins-universal-head" data-setting="Game">
          <span className="plugins-universal-mark" aria-hidden="true">
            <Gamepad2 size={18} />
          </span>
          <div className="plugins-universal-text">
            <h2 id="plugins-universal-game">Game</h2>
            <p>
              {on
                ? "The game you are building, running beside the chat. You play it with your keyboard and mouse; the agent runs it, sees it and plays it too, with time frozen while it thinks."
                : "Off for every agent: chats show no game, agents' game tools stop working, and every game Ghost started is stopped."}
            </p>
          </div>
          <Switch
            label="Game"
            testId="plugins-game"
            checked={on}
            disabled={general === null}
            onChange={(enabled) => prefs.change({ type: "game", enabled })}
          />
        </div>
        {on ? (
          <>
            <div className="plugins-universal-tools" data-setting="Game tools for agents">
              <div>
                <strong>Game tools for agents</strong>
                <span>
                  An agent gets these from Ghost only in a chat whose folder holds a game, or one
                  you turned Game on for in its pane, and only for that chat's own game. One of them
                  runs code inside your game, which an agent can already do by editing the game's
                  scripts. An agent cannot get Godot for you, open a window of the game's own, or
                  export or upload a game. Terminal chats and agents on another computer do not get
                  the tools.
                </span>
              </div>
              <ul aria-label="Game tools agents get">
                {AGENT_TOOLS.map(([tool, does]) => (
                  <li key={tool} title={tool}>
                    {does}
                  </li>
                ))}
              </ul>
            </div>
            <div className="plugins-universal-setting" data-setting="Godot">
              <div>
                <strong>Godot</strong>
                <span data-testid="game-godot">
                  {godot === null || !godot.looked
                    ? "Ghost runs Godot projects itself. Open the Game pane in a game's chat and it looks for Godot on this computer."
                    : godot.fetched
                      ? `Ghost has its own Godot ${godot.pinned}.${
                          godot.found.length > 0
                            ? ` Yours (${godot.found.join(", ")}) is used for projects made with it.`
                            : ""
                        }${running > 0 ? ` ${running === 1 ? "One game is" : `${running} games are`} running.` : ""}`
                      : godot.found.length > 0
                        ? `Using the Godot you installed (${godot.found.join(", ")}).${running > 0 ? ` ${running === 1 ? "One game is" : `${running} games are`} running.` : ""}`
                        : godot.size > 0
                          ? `Not on this computer yet. The Game pane offers to get Godot ${godot.pinned} (${megabytes(godot.size)}) from Godot's own releases, when a game needs it.`
                          : "Not on this computer, and Godot publishes no build Ghost can fetch for it. Install Godot yourself and Ghost uses it."}
                </span>
              </div>
            </div>
            <div className="plugins-universal-setting" data-setting="Other engines">
              <div>
                <strong>Other engines</strong>
                <span>
                  A web game (Three.js, Phaser and the like) runs in Ghost's browser beside the
                  chat. Unity, Unreal Engine and Roblox projects are recognised, and are worked on
                  in their own editors with their makers' agent tools.
                </span>
              </div>
            </div>
          </>
        ) : null}
      </section>
    </div>
  );
}
