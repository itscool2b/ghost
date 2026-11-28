/**
 * Game fixture (`?preview=game`, docs/game.md): a chat with its Game pane open, over in-memory
 * games. There is no engine: a running game shows a picture the fixture draws. The checks against
 * a real Godot are `crates/ghost-game/tests/godot.rs` and the native run recorded in docs/game.md.
 *
 * `?state=` chooses what the pane opens on: `ready` (the default), `none`, `none-on`, `web`,
 * `unity`, `bevy`, `unsupported`, `looking`, `get`, `fetching`, `unpacking`, `fetch-failed`,
 * `too-old`, `starting`, `running`, `frozen`, `agent`, `errors`, `ended`, `no-tools`, `remote`
 * or `off`. `&closed` starts with the pane closed. `window.gameFixture` is the stand-in itself:
 * its `calls`, `inputs`, `game(...)`, `summon(...)` and `frame(...)`.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { GameStatus } from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import {
  attachResult,
  chatSummary,
  FakeTransport,
  historyPage,
  lists,
  sessionInfo,
} from "../lib/chat/testing";
import { GameStore, gamePanels } from "../lib/game";
import { chatGame, FakeGame, gameStatus, godotStatus, LOG, project, running } from "./gameFixture";
import { withPreviewContext } from "./previewContext";

declare global {
  interface Window {
    gameFixture?: FakeGame;
  }
}

export const GAME_CHAT = "game-fixture-chat";
const CHAT = GAME_CHAT;

const NO_GODOT = godotStatus({ fetched: false });
const MISSING = "Godot is not on this computer yet. Get it in the Game pane.";

function statusFor(state: string): GameStatus {
  switch (state) {
    case "none":
      return gameStatus({ chats: [chatGame(CHAT, { project: null, tools: false })] });
    case "none-on":
      return gameStatus({ chats: [chatGame(CHAT, { project: null, tools: true })] });
    case "web":
      return gameStatus({
        chats: [
          chatGame(CHAT, {
            project: project({
              engine: "web",
              name: "orbit-run",
              version: null,
              framework: "three",
            }),
          }),
        ],
      });
    case "unity":
      return gameStatus({
        chats: [
          chatGame(CHAT, {
            project: project({ engine: "unity", name: "Harbor", version: "6000.0.23f1" }),
          }),
        ],
      });
    case "bevy":
      return gameStatus({
        chats: [
          chatGame(CHAT, {
            project: project({ engine: "bevy", name: "asteroids", version: null }),
          }),
        ],
      });
    case "unsupported":
      return gameStatus({
        chats: [
          chatGame(CHAT, {
            project: project({
              unsupported:
                "This Godot project is written in C#. Ghost runs GDScript projects for now.",
            }),
          }),
        ],
      });
    case "looking":
      return gameStatus({
        godot: godotStatus({ looked: false, fetched: false }),
        chats: [chatGame(CHAT)],
      });
    case "get":
      return gameStatus({ godot: NO_GODOT, chats: [chatGame(CHAT, { godot_missing: MISSING })] });
    case "fetching":
      return gameStatus({
        godot: {
          ...NO_GODOT,
          fetch: { state: "running", received: 21_500_000, total: 86_013_866 },
        },
        chats: [chatGame(CHAT, { godot_missing: MISSING })],
      });
    case "unpacking":
      return gameStatus({
        godot: { ...NO_GODOT, fetch: { state: "unpacking" } },
        chats: [chatGame(CHAT, { godot_missing: MISSING })],
      });
    case "fetch-failed":
      return gameStatus({
        godot: {
          ...NO_GODOT,
          fetch: {
            state: "failed",
            message:
              "Godot's download did not match the hash Godot publishes, so it was thrown away.",
          },
        },
        chats: [chatGame(CHAT, { godot_missing: MISSING })],
      });
    case "too-old":
      return gameStatus({
        godot: godotStatus({ fetched: false, found: ["4.3"] }),
        chats: [
          chatGame(CHAT, {
            godot_missing:
              "This project was made with Godot 4.7, which is newer than the Godot here (4.3). Install that Godot and Ghost will use it.",
          }),
        ],
      });
    case "starting":
      return gameStatus({ chats: [chatGame(CHAT, { state: "starting" })] });
    case "running":
      return gameStatus({ chats: [running(CHAT)] });
    case "frozen":
      return gameStatus({ chats: [running(CHAT, { frozen: true, fps: 0 })] });
    case "agent":
      return gameStatus({
        chats: [running(CHAT, { frozen: true, fps: 0, agent: "Holding ui_right" })],
      });
    case "errors":
      return gameStatus({
        chats: [running(CHAT, { errors: 3, last_error: LOG.errors[0] ?? null })],
      });
    case "ended":
      return gameStatus({
        chats: [
          chatGame(CHAT, {
            state: "ended",
            ended: "The game stopped by itself (exit code 1).",
            errors: 3,
            last_error: LOG.errors[0] ?? null,
          }),
        ],
      });
    case "no-tools":
    case "remote":
      return gameStatus({ chats: [chatGame(CHAT, { tools: false })] });
    case "off":
      return gameStatus({ enabled: false, chats: [chatGame(CHAT)] });
    default:
      return gameStatus({ chats: [chatGame(CHAT)] });
  }
}

/** One stand-in for the page (StrictMode builds the state twice). */
let shared: FakeGame | null = null;

function Preview() {
  const [fixture] = useState(() => {
    const params = new URLSearchParams(location.search);
    const state = params.get("state") ?? "ready";
    shared ??= new FakeGame(statusFor(state));
    const game = shared;
    window.gameFixture = game;
    const chat = chatSummary(CHAT, {
      status: "ready",
      agent_id: "claude",
      agent_name: "Claude Code",
      ...(state === "remote" ? { agent_host: { type: "wsl" as const, distro: "Ubuntu" } } : {}),
    });
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async () =>
          attachResult(chat.id, 0, historyPage(chat.id, []), {
            chat,
            session: sessionInfo({ status: "ready" }),
          }),
        lists: async () =>
          lists(1, {
            capabilities: {
              images: true,
              steer: false,
              interrupt: true,
              resume: true,
              fork: false,
              rewind: false,
              set_model: true,
              set_effort: true,
              permissions: true,
              plans: false,
              usage: false,
              limits: false,
              slash_commands: false,
              add_dirs: false,
              plan_edits: false,
              autonomy_levels: ["full_auto"],
            },
          }),
      }),
    );
    gamePanels.set(CHAT, params.get("closed") === null);
    return { chat, transport: Object.assign(transport, { game: new GameStore(game) }) };
  });
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Game verification</h1>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          chatId={fixture.chat.id}
          listed={fixture.chat}
          transport={fixture.transport}
          agents={[]}
          storage={null}
        />
      </div>
    </main>
  );
}

export function mountGamePreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
