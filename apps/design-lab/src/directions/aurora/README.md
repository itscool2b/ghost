# Aurora

Immersive light behind heavy glass. The most literal take on the plan's "Aura" language: the
active agent's colour is a slow light field behind the whole window, and the UI is three layers of
glass over it. This folder belongs to the Aurora designer; the direction lives entirely inside it.

## The idea

- **Light, then glass.** Under everything: a light tint over the native window glass, the shared
  `AuraField`, and two backlight lobes centred on the transcript sheet's left and right edges, so
  their bright halves fall in the gutters. The lobes use the agent's glow colour made a step
  deeper and richer (`oklch(from ...)`), never the palette's dark tones, so it reads as coloured
  light, not haze. Thinking and tool runs brighten it; agent switches cross-fade it, along with
  every accent, through the registered `--aura-*` properties.
- **Waiting is amber, and it breathes.** A pending permission dims the agent's light to 40% and
  fades in an amber layer of the same shape over 1.4 s, which breathes slowly while it waits. The
  sheet's top rim turns amber too (that stays, without the breathing, under reduced motion).
- **Glass carries the density.** The global tint is light, so the wallpaper and the light show in
  the gutters and at the window's edges. The surfaces that carry text are denser: level 1 is the
  frosted sidebar, level 2 the floating transcript sheet (plus the plan, queued messages and the
  user's bubbles), level 3 the composer, the permission card and every popover. Each is a
  squircle (`squirclePath` from `@ghost/ui`) with a gradient rim stroked inside the clip, a
  soft specular top, and a shadow on an unclipped layer. Opened tool output sits in a well
  (level 0). Only small overlays use `backdrop-filter`.
- **One content box.** Header, transcript column, plan, queue and composer share a symmetric 32px
  inset in the sheet. Radii come in one family: 18 for the panels, 16 for floating cards and
  popovers, 12 for wells, 10 and 8 for controls and rows.
- **The orb leads the turn, in its own colours.** Each agent turn has its orb in a gutter of its
  own, sticky while the turn scrolls past (just below the top fade, so it never dims). A turn keeps
  its own agent's brand and the autonomy its session reported, whatever the composer selects now.
  A glow behind the live orb breathes with the stream, read from the playhead, so `?t=` frames are
  exact.
- **Text inks in.** Each streamed delta fades up from the agent's glow colour to plain text in its
  own keyed span, so earlier words never re-render or move. Half-streamed `**bold**` and `` `code` ``
  render styled at once, and an ember marks where text is still arriving.
- **Calm by default.** A tool group at rest is one quiet row with no box; opening it draws a well
  around what it opened. A command opens itself while it runs, stays open if it fails, and folds
  to its command line 0.8 s after it passes. The thought folds into "Thought for 6s". The live plan
  docks above the composer as one line and leaves 1.5 s after its turn ends; the turn's footer
  keeps it ("Plan 3 of 3").

## Interactions

- **Picker** (the pill, or Command-/ from anywhere): agent > model > effort. Clicks commit: an agent
  click selects it with its last-used model and effort and moves on to the models, a model click
  moves on to the efforts, an effort click closes. Escape or a click outside only closes. The
  keyboard browses without committing (arrows, Tab between columns; the light previews the agent
  under the cursor) until Enter commits. The pointer previews an agent's light after resting 250 ms
  on it. Focus follows the cursor and goes back to the pill on close. In a new chat the picker
  opens below the composer and the hero lifts to make room, so its orb shows the agent under the
  cursor.
- **Switching agents:** when the next message goes to another agent than the last turn's, the
  composer says so ("Next message goes to Pi with a handoff") and previews the handoff: what was
  asked, the other agents' answers, the files changed. When the new agent cannot keep the chosen
  autonomy (Pi is Full auto only), an amber line says so and the chip pulses once; switching back
  restores the level you chose. The agent dock in the sidebar switches agents too.
- **Permission card:** it stands in for the run it holds. Enter allows once, Option-Enter always
  allows, Escape rejects, from the composer too (with text in the box, Enter sends it and allowing
  takes Command; the card's hints follow). Once answered, the card folds away and the run comes
  back with the answer as a chip ("Allowed once", "Rejected"); the light leaves amber and the
  sidebar row follows.
- **Composer:** grows to eight lines. Send becomes Stop while the agent works; typing then queues,
  and each queued message waits in a row above the composer (Edit puts it back, the cross drops
  it) until the turn ends, when it rises into the transcript. A new chat's first message flies the
  hero orb into the conversation's gutter and the composer down to its dock. Messages sent here
  show their agent starting (the recording cannot answer them) until Stop ends them. Page Up and
  Page Down, and Command-arrows, scroll the transcript while the box is empty.
- **Disclosures:** the thought pill, tool groups and each call (diff with line numbers, ANSI
  terminal, search matches), the footer's files and plan chips, the docked plan. Opening one near
  the end of the chat reveals it; new output glides into view only while you are at the end, and
  "Jump to latest" appears when you are not.
- **Screen readers:** the transcript is a named log; one polite live region announces turn ends and
  new requests, not every streamed word. The picker's columns are listboxes.

## Files

- `App.tsx`: the window: backdrop, sidebar, sheet, dock; local state (selection and memory, chosen
  vs actual autonomy, menus, the answer, queued and sent messages, the hero flight).
- `Glass.tsx`: the squircle glass surface. `ui.tsx`: small shared pieces (Collapse, Spinner, ...).
- `Sidebar.tsx`: wordmark, projects and chats with mini-orbs, agent dock, profile.
- `Transcript.tsx`: turns, user bubbles, the orb gutter and stream glow, cards and the runs they
  hold, footers, sent messages.
- `Thought.tsx`, `ToolRun.tsx` (diffs, terminal, search), `Permission.tsx`, `Plan.tsx`.
- `Composer.tsx` (pill, autonomy menu, notes, Send/Stop/Queue), `Picker.tsx`, `Hero.tsx`.
- `Handoff.tsx`: the handoff block (built from the chat, shown in the composer and on sent messages).
- `Markdown.tsx` + `ink.ts`: streaming markdown; `highlight.ts`: a small code tinter.
- `useStickToBottom.ts`: the transcript's scroll behaviour. `aurora.css`: every style, scoped.

## Rules for this folder

- Scope CSS under `[data-direction="aurora"]`; Aurora's classes start with `au-`. Class names
  starting with `lab-` belong to the lab.
- Keep overlays inside the window: no portals to `document.body`.
- Time comes from the player (`usePlayhead(player)`), never the wall clock.
- WebKit first: `-webkit-backdrop-filter` next to `backdrop-filter`. Popovers are 84% dense with a
  30px frost; headless WebKit does not paint the frost, so its shots show a little of what is
  behind (Safari and WKWebView do blur it).
- Reduced motion (`usePrefersReducedMotion()`, `:root[data-motion="reduce"]`): nothing moves or
  loops; state still reads through colour and short fades.
- No new dependencies, no edits outside this folder. Product copy is plain and short, without em
  dashes.
