# Game: the game you are building, beside the chat

Ghost runs a chat's game beside the chat. The person plays it in the **Game pane** with the
keyboard and the mouse; the chat's agent runs, sees and plays the same game through the
`ghost_game` tools, with time frozen while it thinks. The plan, with the research behind it and
what is still to come (point and tell, replays, a controller, tuning, assets, export), is
[superpowers/plans/2026-11-27-game.md](superpowers/plans/2026-11-27-game.md). This page says how
what is built works.

## Which engines

| Engine | What Ghost does |
| --- | --- |
| Godot 4 (GDScript) | Runs it itself, in the pane. Everything on this page. |
| Web (Three.js, Phaser, Babylon, PlayCanvas, Pixi, plain canvas) | Runs in Ghost's browser beside the chat, and the agent plays it there with the same tools and the same frozen time. See "Web games". |
| Unity, Unreal Engine, Roblox | Connected. Their own editors run the game, their makers' agent tools drive them, and the pane says how to turn those on. See "Other engines". |
| Bevy, LÖVE, Pygame, GameMaker, Defold, MonoGame | Recognised and named. The agent runs them from its shell. |

A Godot project in C#, and a Godot 3 project, are named and declined in words
(`crates/ghost-game/src/detect.rs`).

## What a person sees

- **The game button** in a chat's header (Ctrl+Shift+G) opens the Game pane. It is in the header
  of chats that have a game: the folder holds one, one of its games runs, or the person turned
  Game on for the chat. In any other chat it is "Game" in the More menu. The pane shares the
  chat's side with the device, the browser, Changes and the task panel: opening one closes the
  rest. Chat view only.
- **No game in the folder**: the pane offers to **start one**: Godot 2D or 3D, a plain canvas
  page, or Three.js. Ghost writes a few small files of its own into the chat's folder (a player
  that moves and jumps on a floor), writes over nothing, and refuses a folder that already holds
  a game or a file of the same name. Game is turned on for the chat, and its idle session starts
  again with the game tools. **Only turn on Game for this chat** does that last part alone, for
  an agent that will make the game from nothing.
- **Godot is not on this computer**: the pane says what getting it means (Godot 4.7.2, about
  82 MB on Windows, from Godot's own releases on GitHub, checked against the hash Godot
  publishes, into Ghost's own folder) and fetches nothing until the person presses **Get Godot**.
  A Godot the person installed themselves is found and used; one with the project's own minor
  version is preferred.
- **Play** starts the game with no window of its own and shows it in the pane. Click the picture
  and the keyboard and the mouse are the game's. Every key still down comes up when the pane
  loses the keyboard, so a game is never left with a key stuck. Ctrl and Command chords stay
  Ghost's, which is how a keyboard leaves the game (Ctrl+Shift+G).
- **Under the picture**: Pause and Resume, Step (one frame, while paused), the speed (0.25x to
  2x), then Point, the last 15 seconds and Tune ("Telling the agent", below), sound (off until
  asked for), a picture saved to Pictures, a window of the game's own (the game starts again in
  it, at full speed), start again, and Stop.
- **A controller** plugged into the computer plays the game while the picture has the keyboard:
  click the game first. A small mark beside the frame rate says one is connected.
- **Errors** are counted in the pane's header. The count opens the log: each error and warning
  with its file and line (a click opens the file in the person's editor), then what the game
  printed.
- **Open in Godot** opens the project in Godot's own editor, as a program of the person's.
- **Settings > Game** has the switch (on by default), what agents get, and whether Godot is here.
  Off stops every game Ghost started, takes the pane out of the app and makes every tool answer
  that Game is off, in running chats too.

## Telling the agent

A person can judge what an agent cannot: whether it feels right. Three controls under the
picture turn that into something the agent can act on. Godot games only, for now.

- **Point.** The game freezes and the pointer chooses instead of playing: what is under it is
  outlined and named, and a click opens a note beside it, the same note the browser's Annotate
  writes. Send goes to the chat now (or queues after its turn), Add to chat puts it in the
  message box. The message carries the person's words, then the node (its path in the running
  game), the scene file and the line where it is defined, the script that drives it (its own,
  or the nearest above it), where it is in the picture, and a picture with it outlined. Esc
  gives the pointer back, and the game runs again if it ran before.
- **The last 15 seconds.** What the person just played, as a filmstrip of twelve small
  pictures, with or without words. The run is kept under a name (r1, r2, ...; the last eight of
  a chat) and the message tells the agent to play it again with `game_replay`: the game starts
  afresh with the same random seed and is given the same inputs at the same steps, as fast as it
  can go, and freezes where the person's run ended. The agent reads the game there, changes the
  code, and plays the same run again to see the fault gone.
- **Tune.** The numbers and switches a script exports (`@export`), as sliders that change the
  running game as they move. A range the script gives (`@export_range`) is the slider's; without
  one it runs from nothing to twice the value. The sheet opens on the running scene; the target
  button chooses another node by pointing at it. Turned values are in this run only: **Keep**
  sends the agent what changed and what it was, to write into the script or the scene, and
  Reset puts them back.

## The agent plays it

An agent gets the tools only in a chat whose folder holds a game, or one the person turned Game
on for. Other chats carry none of them. The session is asked once, as it starts
(`GameBridge::wanted`, from `prepare()` in `crates/ghost-core/src/session/actor.rs`), so an agent
that started before the folder was a game does not have them: the pane says so and offers **Give
it the tools**, which starts the idle session again.

| Tool | What it does |
| --- | --- |
| `game_status` | The project, its engine and version, where Godot is, whether the game runs |
| `game_check` | Loads every script and scene without running the game; what does not parse, by file and line |
| `game_run`, `game_stop` | Starts the game (again), stops it |
| `game_look` | A picture of the game as it is |
| `game_tree`, `game_inspect` | The running scene as an outline with refs and screen positions; one node's values |
| `game_set`, `game_eval` | Change a value, or run GDScript, in the running game (this run only) |
| `game_act` | Play input for stretches of game time; answers with a filmstrip, the last frame and new errors |
| `game_wait_for` | Run until a GDScript expression is true, or the time is up |
| `game_logs`, `game_perf` | Errors with file, line and stack, and printed lines; frame time, draw calls, memory |
| `game_time` | Freeze, resume, change the speed |
| `game_replay` | Play again a run the person kept: same seed, same inputs at the same steps, frozen where theirs ended (or at `until_s`) |

**A run can be played again exactly.** Everything done to a game from outside is written on the
run's tape with the step it happened at: every key, click and controller input, from the pane
or from an agent's `game_act`, and every `game_set` and `game_eval`. A replay gives the tape
back to a fresh run that began from the same seed. It is exact for a game that draws its random
numbers from `randi()` and its like and reads no clock: the real-engine test replays a run an
agent set up, an agent played and a person played live, and ends with the same frame count,
score and position. A game that calls `randomize()`, makes its own `RandomNumberGenerator` or
reads the system time may differ, and the tool's description says so. A tape keeps 20 000
events (a pointer that never rests fills it in about five minutes; keys alone take hours);
past that the run cannot be replayed and the message says so.

**Frozen time.** A game an agent runs is frozen between its calls, so nothing moves while it
thinks. `game_act` plays its steps in fixed steps of 1/60 s, as fast as the computer can, then
freezes again: half a second of game time takes about a tenth of a second, and the same steps
give the same result every time. One still picture hides motion, so the answer carries a
**filmstrip**: up to twelve frames of the action in one picture, then the frame it ended on.

Every tool but `game_status`, `game_stop`, `game_logs` and `game_check` is a summon: the pane
opens beside the chat, as the Device pane does for an agent's call, and stays closed where the
person closed it during that stretch of work. What the agent is doing ("Holding ui_right") takes
the game's name's place in the pane's header.

## Web games

A web game is a page, so it runs where pages do: the tab in front of the chat's browser, where
the person plays it with real keys and a real mouse. The game tools of a web project (and of a
chat whose folder holds no game Ghost knows, where the game may be a plain page) go to that
browser instead of to Godot (`WebGames` in `ghost-game`, `browser/game.rs` in the desktop).

- **`game_run` with a `url`** opens the game's address (the agent starts its dev server first),
  marks the tab's page as a game in the page's own session storage, and loads it again. That
  wakes **Ghost's game clock** in the page (`browser/game.js`, injected with the page agent
  before the page's own scripts): `requestAnimationFrame`, `performance.now`, `Date` and timers
  read a clock Ghost holds. Every other page keeps its own time, untouched.
- Live, the game's clock follows real time, a frame for each of the display's. Frozen, no frame
  and no timer runs. `game_act` steps it in fixed 1/60 s steps with keys by name (`ArrowLeft`,
  `Space`, `KeyW`; Godot's `ui_left` and the like are taken as the arrows) and clicks in the
  picture's pixels, then answers with the filmstrip and the last frame, read from the game's
  canvas in the frame's own task. A game with no canvas gets the browser's picture of the page.
- `game_wait_for` and `game_eval` take JavaScript. `game_tree` lists a Three.js scene (three
  announces its scenes to `__THREE_DEVTOOLS__`, which the clock defines first) or a Phaser game
  that keeps itself on `globalThis.__GHOST_GAME__`. `game_logs` reads the page's console.
  `game_check` and `game_set` are Godot's and say so.
- **A page's passwords never reach an agent**, as with the browser tools: every answer that
  carries the page's own text has the page's password values taken out, read before the call and
  after it, and is withheld when they cannot be read. The tools that read a page's insides or
  run code in it (`game_eval`, `game_tree`, `game_inspect`) work only in a page the agent
  marked as a game.
- What it does not reach: frames and workers of other origins keep real time, and input is the
  page's own events (`isTrusted` is false), which games do not check.

## Other engines

A Unity, Unreal or Roblox project gets a **Connect** card in the pane instead of a Play button
(`components/game/EngineConnect.tsx`). Ghost does not run these editors. Each card holds:

- **The maker's own steps**, as its guide gave them when the card was written (27 November 2026),
  with a link to that guide, since the steps are the maker's to change. Unity: its CLI
  (`unity pipeline install`), then its agent plugin for Claude Code or `unity mcp configure` for
  another agent. Unreal: the Unreal MCP plugin and All Toolsets, whose server starts with the
  editor at `http://127.0.0.1:8000/mcp`. Roblox: Enable Studio as MCP server, under Assistant.
- **One live line**: whether the tools are within reach now (`ghost-game`'s `probe.rs`). For
  Unreal one loopback connection to port 8000, closed at once; for Unity whether its CLI is in a
  folder of `PATH`; for Roblox whether Studio's launcher file is there. Nothing is run and
  nothing is sent.
- **Add to this project**, for Unreal and Roblox: the maker's MCP server becomes one of the MCP
  servers of the chat's project (Settings > Plugins & MCP shows it, and it can be removed
  there). The address or program is Ghost's own constant for that engine, the person presses
  it, and no tool does. Unity has no server for Ghost to add: its CLI configures each agent.
- What the maker says about reach: Unreal's server has no sign-in, and Roblox's lets a connected
  agent change what is in an open place.

## Who may do what

- **Only the person**: get Godot, start a game from one of Ghost's starters, turn Game on for a
  chat, add another engine's server to a project, give the game a window of its own, open
  Godot's editor, point at something, keep a run for the agent, turn a value from the pane, turn
  Game off. None of these is a tool, and `helper.rs` tests that no tool
  name fetches, installs, exports, publishes or uploads, and that none takes a chat or a folder.
- **An agent**: a native Chat session only (not Terminal chats, not agents on another computer),
  through a loopback listener with a per-session token that goes when the session does
  (`crates/ghost-core/src/game_service.rs`). The chat and the folder a call acts on are the
  grant's own: the folder the session started in. Game's switch is read on every call.
- `game_eval` and `game_set` run inside the person's own game. An agent can already do as much
  by editing the game's scripts, and Settings says so plainly.
- **A replay is the game's own run given back to it.** The tape comes from the game and goes
  only to a fresh run of the same project, so code on it (what `game_eval` ran) runs where it
  ran before and nowhere else. An agent can replay only a run the person kept, by its name, in
  its own chat. Kept runs live in memory and go when Ghost quits.
- **What pointing reads.** The node's name, text, scene and script are the game's own words:
  cut to size and sent as data. The scene's line is read by Ghost from the project's own file,
  and only a path inside the project is opened (`run::checked_scene`).
- A value turned from the pane is one of the node's own, named by a plain name, and a number or
  a switch: `set_value` refuses anything else.
- **The game is not trusted.** It connects to a loopback listener made for that run and must show
  the run's token before anything else. From then on it can only answer and send pictures: every
  message is bounded, and what it says (node names, text, printed lines, errors) is cut to size
  and is data for the agent, never an instruction (`bridge.rs`, `output.rs`, `tools.rs`).
- Godot is started with its arguments apart, never through a shell. One game a chat, two running
  at once, each stopped after ten minutes unwatched and unused, and all of them when Ghost quits.
- The pane's commands take a chat id and read its folder from the chat's own record
  (`apps/desktop/src-tauri/src/game.rs`): the page never names a folder.

## How it works

- **`crates/ghost-fetch`**: one HTTPS download checked against a pinned size and SHA-256 or
  SHA-512, redirects followed only to named hosts, and the unzip that keeps every entry inside
  its folder. `ghost-devices` unpacks Android's packages with the same code.
- **`crates/ghost-game`** (no Tauri): `detect.rs` (what a folder holds), `godot.rs` (find or
  fetch), `run.rs` (start it: a job object on Windows, a process group elsewhere, killed with
  Ghost), `bridge.rs` (the loopback protocol), `output.rs` (errors and printed lines, bounded),
  `check.rs`, `manager.rs` (every chat's game and the one status the pane shows), `tools.rs`.
- **Nothing is written into the project.** Godot is started as
  `godot --path <project> -s <Ghost's loop.gd> --fixed-fps 60 --disable-vsync`: Ghost's own
  `SceneTree` (`resources/godot/loop.gd`) is loaded from Ghost's folder, the project's autoloads
  load as usual, and the loop adds Ghost's bridge node and opens the project's main scene.
- **Out of sight.** On Windows the game's window is owned by Ghost's (`--wid`) and placed far
  off-screen, so it has no taskbar entry and still draws; a foreground lock held across the
  start keeps it from taking the keyboard. On macOS and Linux the game's own window shows.
- **The bridge** (`resources/godot/bridge.gd`) speaks frames of
  `[u32 length][u8 kind][payload]`: JSON requests and answers, the pane's pictures (JPEG, sent
  only while a pane watches, at most two in flight) and an answer's pictures. `--fixed-fps` makes
  every frame exactly 1/60 s of game time; the bridge itself keeps real time while a person
  plays, and lets go of it while an agent acts.
- **Whole steps.** The game is stopped and started only after a frame is drawn, never in the
  middle of one, so every step it takes is a whole one (its physics, then its nodes). The
  bridge counts those steps and stamps the tape with them. A run started for a replay
  (`GHOST_GAME_HOLD`) takes its first step and then stands still: that step is where the engine
  puts the first scene in the tree, and a game stopped before it would be one step ahead of the
  run it repeats. The seed is `GHOST_GAME_SEED`, set by `loop.gd` before anything loads.
- **Pointing** asks the bridge what is at a point: the smallest box that holds it among
  controls, sprites, collision shapes, polygons, tile layers and the projected bounds of 3D
  meshes, with whatever fills most of the picture counted only when nothing smaller is there.
- **A controller.** The pane reads the browser's standard gamepad and sends what changed in
  Godot's own numbers (the triggers are axes there), sticks in twentieths past a dead zone. A
  game that reads the computer's controllers itself is sent none, or it would hear every press
  twice; it says how many it sees. Ghost has no controller to test with: the pane's side is
  checked with a stand-in gamepad, and a real one is unverified.
- **Errors** come from Godot's own `Logger` (4.5 and later), with file, line and the project's
  stack frames; the same error again is counted, not listed again.
- **The pane** (`components/game/GamePanel.tsx`, `lib/game.ts`) draws the pictures with the
  player the Device pane uses (`lib/screen.ts`) and sends keys by their `code`, down and up, and
  the mouse in the game's own pixels.

## Checking it

- `cargo test -p ghost-fetch -p ghost-game`, `cargo test -p ghost-core game_service` (the grant:
  its own chat and folder, a wrong token, unknown fields, the switch, the listener gone with the
  session), `cargo test -p ghost-desktop game::`.
- A real engine: `GHOST_GODOT_E2E=1 xvfb-run -a cargo test -p ghost-game --test godot --
  --nocapture` fetches Godot 4.7.2 (checked against its SHA-512), then drives a small fixture
  game through every tool: half a second of `ui_right` moves the player exactly 100 pixels, twice.
  The **Godot game** workflow runs it on a Linux runner.
- `pnpm --filter @ghost/desktop exec node scripts/check-game.mjs` (`?preview=game`): every state
  of the pane in both appearances and both engines, its keys and mouse, its controls, the log,
  the agent's line and summons, the narrow window and Settings > Game, with axe.
- `node scripts/check-web-game.mjs`: the game clock itself in Chromium and WebKit, on a small
  canvas game: asleep until marked, live, frozen, half a second of a key as exactly thirty frames
  and 100 pixels, timers on the game's clock, clicks, the filmstrip, waits, the outline.
- `node --test scripts/game-starters.test.mjs`: the canvas starter's own little server answers
  this computer only (a request under another name is refused), and serves no hidden file and
  nothing outside its folder. The real-Godot test loads and plays both Godot starters.
- `vitest`: `lib/game.test.ts`, `lib/gameTell.test.ts` (the words of each note, slider ranges, a
  controller's input).
- The real-engine test also points at the fixture's player and its score (the node, its scene
  and line, its script), turns an exported value, keeps a run that an agent set up and played
  and a person played live, and replays it to the same frame, twice, the second time stopped
  early. `check-game.mjs` plays Point, the last 15 seconds, Tune and a stand-in controller.

## Still to come

An assets view, and export. They are in the plan, in that order. Pointing, replays and tuning
are for Godot games: a web game is played in the browser with the person's own input, which
Ghost does not record, and its page is annotated with the browser's own Annotate.
