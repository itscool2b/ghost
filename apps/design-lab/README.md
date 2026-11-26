# auracode Design Lab

**Historical studies:** This lab is not the production design authority. The current proposed direction and pending visual checkpoint are in [the visual language](../../docs/design/visual-language.md). Orbit remains selected only inside this historical gallery.

The M1a Design Lab from [docs/plan.md](../../docs/plan.md): visual directions for the auracode UI
(Aurora, Noir and Halo, then Orbit after the owner turned those three down), each rendering the same
recorded-style chat, so the owner could pick one. The owner chose **Orbit**: the gallery opens with
it, marked as chosen, and it graduates into the real UI in M2a. The other three stay for comparison.
Every direction gets the same data (the transcript model, the workspace, a timeline player) and
differs only in how it presents it, inside a simulated 1440x900 macOS window floating over a
wallpaper (the stand-in for the native Liquid Glass behind the real, transparent window).

## The directions

The gallery lists Orbit, the chosen direction, first and then the others in the order they were
made. Below they are described in the order they were made, so Orbit comes last.

**Aurora** (`#/aurora`): immersive light behind heavy glass, the agent's colour glowing through every
layer. The agent's light sits as two lobes on the transcript sheet's edges, in a deeper version of
its glow colour, while a light window tint lets the wallpaper show in the gutters and the text panels
carry the density. Waiting for approval dims that light and fades in a breathing amber layer. Past
turns keep their own agent's colours, the permission card answers from the composer, and switching
agents shows the handoff inside the composer with a preview of what is handed over.

**Noir** (`#/noir`): smoked glass and serif headlines, with the orb as the only light in the room. A
near-black smoke and vignette cover the reading area; the live orb in the composer pill (and in a
reply's byline while the agent works) pools its light under the composer, steps with what the agent
is doing and turns amber when it waits. Everything else is grey or ink. Instrument Serif is kept for
display moments, prose holds an 80-character line on a 24px rhythm, and nothing jumps: streamed words
never change line, and anything that moves the text being read glides at most one line per frame.

**Halo** (`#/halo`): spatial glass, floating slabs at real depth, the agent's orb in a ring of its
light. Planes differ in brightness and edge crispness, so depth reads even in a still, and parallax
adds to it. Send is the agent's orb in a 3px ring that turns while the agent works and breathes amber
while it waits. The slab under the composer is one status line for the running turn, tool groups stay
single rows until opened, and Esc works through one keyboard layer, from the topmost menu down to
stopping the turn.

**Orbit** (`#/orbit`, chosen): mission control for your agents, from the design language in
[docs/design/visual-language.md](../../docs/design/visual-language.md). Black and white, sentences
and numbers, one orange that means look here. Paper panes laid on a warm grey tray (light) or black
(dark), separated by hairlines, not glass. Every agent is a line-drawn orbit with a square satellite
whose motion is its state. It renders every lab state, the two new ones included, in both themes.

Each direction's own README (`src/directions/<id>/README.md`) has the full design notes. To compare
them without running anything, open the shots: `shots/index.json` lists every screenshot and video.

## Run it

```sh
pnpm install                                         # once, at the repo root
pnpm --filter @ghost/design-lab dev               # http://localhost:5180
pnpm --filter @ghost/design-lab dev --port 5181   # any other port (never 1420, the app's)
```

Each dev server keeps its own Vite cache (`node_modules/.vite/port-<n>`), so several can run side by
side. Open `#/` for the gallery, or a direction directly.

## Build

```sh
pnpm --filter @ghost/design-lab build             # typecheck, then a static build in dist/
pnpm --filter @ghost/design-lab preview           # serve dist/ at http://localhost:5180
```

The build is portable. Asset URLs are relative (`base: "./"` in `vite.config.ts`) and routing lives
in the hash, so `apps/design-lab/dist` works from any folder of any static host with no rewrites:
copy it anywhere, or serve it with any static server, then open `#/`.

```sh
python3 -m http.server 5191 --bind 127.0.0.1 --directory apps/design-lab/dist   # http://127.0.0.1:5191/#/
```

Use a server rather than opening `index.html` from disk: browsers refuse the module scripts over
`file://`.

## Shoot

```sh
pnpm --filter @ghost/design-lab shoot --direction all --port 5190
pnpm --filter @ghost/design-lab shoot --direction aurora --port 5190 --engines webkit
pnpm --filter @ghost/design-lab shoot --direction aurora --server http://localhost:5181/ --only settled,picker --no-video
```

It builds the lab into a private folder, serves it with `vite preview` on the port, and writes
`shots/<direction>/<engine>-<shot>.png` at 1440x900 and 2x for `streaming-t7.5`, `streaming-t24`,
`settled`, `empty`, `permission`, `picker`, `changes`, `onboarding` (frozen at t=1.6) and
`settled-reduced-motion`, plus `<engine>-streaming.webm` (14 s from t=0 at 1x), in WebKit and
Chromium by default. A direction is shot only in the states its `meta.ts` lists, on its own wallpaper.
`--themes dark,light` shoots each theme the direction draws (`?theme=`, with the OS colour scheme
emulated to match) and names the files `<engine>-<theme>-<shot>.png` and
`<engine>-<theme>-streaming.webm`; without it, one dark pass keeps the names above. `--server` shoots
a running lab instead, and `--dev` serves with the dev server, so a direction that is broken mid-edit
only breaks its own pages. Run `node scripts/shoot.mjs --help` for every option.

```sh
node scripts/shoot.mjs --direction orbit --port 5190 --themes dark,light
```

`shots/index.json` indexes the final shots: `chosen` (the chosen direction's id), then for each
direction, chosen first, its id, name, tagline (and `chosen: true` on the chosen one), key ideas,
and, for a direction that lists them, its `states` and `themes`, then the engine, theme (when shot
with `--themes`), shot name, path (relative to the index) and lab URL of every screenshot, and the
same for every video plus its `playbackSeconds`. Orbit is shot with `--themes dark,light` (36
screenshots and 4 videos); Aurora, Noir and Halo with one dark pass (14 and 2 each). `shots/` is review
material and is not committed.

## URLs

```
#/                                   gallery: the chosen direction first, then the rest, with live
                                     thumbnails and links to each state
#/<direction>                        that direction, streaming
#/<direction>/<state>?<params>       that direction in one state
```

Directions: `orbit` (chosen), `aurora`, `noir`, `halo`.

| State | What it shows |
|---|---|
| `streaming` | The golden turn playing in real time, looping after a 4 s pause |
| `settled` | The golden turn finished: answer, plan, footer |
| `empty` | A new chat with no messages: the hero moment |
| `permission` | The Codex chat waiting on a permission card |
| `picker` | The composer's agent > model > effort picker open over the settled chat |
| `changes` | The settled chat with the right panel open on the turn's diffs (`workspace.rightPanel`) |
| `onboarding` | First-run agent discovery, played as a timeline and looping (`workspace.discovery`) |

The first five are every direction's. The last two are opt-in: a direction lists the states it renders
in its `meta.ts` (`states`), and the gallery, the controls and the shoot script skip the rest, so
`#/aurora/changes` says the state is missing instead of rendering half a window.

| Param | Effect |
|---|---|
| `t=<seconds>` | Freeze the playhead there: the same frame every time (7.5: the thought is finishing; 24: the tests run under the plan) |
| `speed=<n>` | Playback rate, 0.1 to 16 (default 1) |
| `motion=reduce` | Render as if the OS asked for reduced motion |
| `play=1`, `play=0` | Force playback on (any state plays and loops) or off |
| `hud=1`, `hud=0` | Pin the lab controls open, or hide them |
| `wall=dusk`, `dawn`, `graphite` | Wallpaper behind the window (dawn is the bright, hard case); the default is the direction's own (`meta.ts`), else dusk |
| `theme=dark`, `light` | The appearance (default dark). Passed to every direction as the `theme` prop and set as `data-theme` around it; directions that list only dark in `meta.ts` ignore it |
| `thumb=1` | Gallery thumbnail: tight margins, no controls |

Example: `#/noir/streaming?t=24&wall=dawn`, `#/orbit/onboarding?t=1.6&theme=light`. The lab controls
(direction, state, play, scrub, speed, reduced motion, and the theme for directions that draw both)
appear when the pointer nears the bottom edge. Keys: Space plays or pauses, the arrows step 2 s (10 s
with Shift), H pins them.

## Layout

```
src/
  main.tsx                 boot: reduced-motion override, styles, the lab
  kit.ts                   everything a direction imports: types, hooks, helpers (one stable path)
  styles.css               Tailwind, Aura fonts and tokens, base styles
  lab/                     the lab itself: routing, stage, gallery, controls, reduced motion
  fixture/                 the recordings and the workspace (types.ts holds every fixture type)
    golden.ts              Claude Code fixes the flaky orb pacing test (about 48 s)
    golden-files.ts        the files the golden turn edits, before and after
    approval.ts            Codex stops on "Run cargo test -p aura-drivers?"
    discovery.ts           first-run discovery: five agents found, two missing (about 3 s)
    timeline.ts            authored scripts to timed, seeded stream events
    workspace.ts           projects, chats, agent dock, models, composers, lookup helpers
    scenes.ts              which recording, chat and composer each state shows
  model/                   shared logic, pure and tested
    transcript.ts          the reducer: timed events in, TranscriptModel out
    discovery.ts           the discovery reducer: scan events in, DiscoveryModel out
    types.ts               TranscriptModel and everything in it
    player.ts              the playback clock: speed, loop, pause, seek
    usePlayer.ts           the player a direction receives, and usePlayhead
    markdown.ts            message text to blocks (parseMarkdown)
    ansi.ts, diff.ts       terminal colours, unified diffs
    tools.ts               tool names and groups ("Read 2 files, searched 1 pattern")
    share.ts               structural sharing, so finished blocks keep their identity
    format.ts              durations, counts, costs, paths
  shared/                  the simulated desktop: WindowChrome, Wallpaper, PlainTranscript
  directions/
    index.ts               the registry: id -> { id, name, tagline, states, themes, wallpaper, App },
                           in gallery order, and CHOSEN_DIRECTION_ID ("orbit")
    types.ts               DirectionProps, Direction
    orbit/                 the chosen direction
    aurora/ noir/ halo/    one folder per direction, each owned by one designer
scripts/shoot.mjs          screenshots and video in WebKit and Chromium
```

## Building a direction

Your folder (`src/directions/<id>/`) is yours alone: `App.tsx` exports `App`, which renders everything
inside the 1440x900 window, and `meta.ts` holds the name and tagline the gallery shows, plus, when
you need them, the states you render (`states`, default the core five), the themes you draw (`themes`,
default dark only) and the desktop you open on (`wallpaper`, default dusk). Import types and helpers
from `src/kit.ts` (`import { usePlayhead, type DirectionProps } from "../../kit"`), and orbs, the aura,
brands, glass and tokens from `@ghost/ui`.

```ts
interface DirectionProps {
  state: LabState;              // the core five, or "changes" | "onboarding" when meta.ts lists them
  model: TranscriptModel;       // the open chat at the playhead (no turns in "empty", "onboarding")
  workspace: Workspace;         // sidebar, dock, composer (pickerOpen in "picker"), rightPanel in
                                // "changes", discovery (the scan at the playhead) in "onboarding"
  player: Player;               // the timeline; read the moving playhead with usePlayhead(player)
  theme?: LabTheme;             // "dark" | "light", from ?theme=
}
```

- The model changes only when the playhead crosses an event, and finished blocks keep their object
  identity, so `React.memo` on a block skips everything but the one that streams.
- Live counters ("Thinking 4s") read the playhead in a small component:
  `const now = usePlayhead(player, 1000)`, then `formatElapsed(now - item.startedAt)`.
- Agent text is markdown: `parseMarkdown(item.text, item.streaming)` gives keyed blocks with inline
  nodes, never HTML. Terminal output comes parsed (`call.terminal.lines`, named colours you map to your
  palette), and so do diffs (`call.diffs`: counts and unified hunks with line numbers).
- Orbs and the aura come from `@ghost/ui`: `<Orb brand={agent} state={model.activity} />`,
  `<AuraField brand={agent} intensity={model.aura} />`, and `brandCssVariables(getBrand(agent))` for
  accent colours.
- Window geometry is in `src/shared/window.ts`: the title bar is 52px, and the traffic lights sit at
  x 18 to 70 inside it. The real window is transparent over native glass; `WindowChrome` simulates
  that glass, and your root lays the tint over it (`--aura-window-tint`, as the app does).

Rules:

- Edit only your folder. No new dependencies (lucide-react, Motion, Tailwind and `@ghost/ui` are
  installed). Shared changes go through the lab owner.
- Scope CSS to your direction: CSS Modules (`*.module.css`), or selectors under
  `[data-direction="<id>"]`, which the lab sets around your App. `lab-` class names belong to the lab.
- Keep overlays inside the window (no portals to `document.body`), so they scale with it.
- Time comes from the player, never the wall clock: `?t=` must render the same frame every time, and
  when `player.playing` is false, render the model as it is.
- Honour reduced motion: `usePrefersReducedMotion()` from `@ghost/ui`, Motion's own handling, or
  `:root[data-motion="reduce"]` in CSS. All three follow both the OS setting and `?motion=reduce`.
- WebKit first: the app runs in WKWebView. Write `-webkit-backdrop-filter` next to `backdrop-filter`,
  keep blur radii constant (never animate them), and check `--engines webkit` shots. One catch:
  Playwright's headless WebKit does not paint `backdrop-filter`, so a frosted overlay shows unblurred
  in `webkit-*` shots (Safari and the app's WKWebView do paint it). Judge frosted overlays in the
  Chromium shots or in Safari against your dev server. The window's own glass is drawn without it,
  so it looks the same in both engines.
- Product copy is plain and short. No em dashes anywhere (a test checks), and no personal data.

## Checks

```sh
pnpm --filter @ghost/design-lab test        # reducer, player, fixture, URLs, rules, directions
pnpm --filter @ghost/design-lab typecheck
pnpm --filter @ghost/design-lab build       # typecheck plus the static build
pnpm lint                                      # Biome, at the repo root
```
