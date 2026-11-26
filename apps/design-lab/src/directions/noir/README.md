# Noir

Smoked glass, editorial calm. A near-black window where the agent's orb is the only light; the
agent's colour shows as a thin accent (the streaming caret, the plan's progress rule, the selected
row, the focus ring) and colour elsewhere means something (a diff, an approval, an error).

This folder belongs to the Noir designer. Only that designer edits files here, and the Noir
direction lives entirely inside it.

## Ideas

- **The room.** Plain tints over the native glass, never a blend mode (in the app the glass sits
  behind a transparent webview, where no page blend can reach it): near-black smoke with a soft
  vignette and film grain over the reading area, a light neutral tint under the sidebar, so the
  sidebar reads as a lighter pane of smoked glass with the wallpaper's light behind it.
- **One light.** The live orb sits in the composer pill, and in the byline of a reply while the
  agent works on it. Its light pools under the composer and catches the composer's top edge as a
  thin rule, stepping with what the agent does: dim at rest, warmer thinking, brighter writing,
  brightest running a tool, amber waiting for approval. Everything else is grey or ink: the dock
  keeps colour only for the composer's agent, Stop and Update are glass, not light.
- **Editorial type.** Instrument Serif for display moments only: the chat's headline (the title
  bar takes the title over once it scrolls away, decided before the first paint), the new-chat
  line, headings and bold lead-ins in answers (set as italic run-in heads). Geist on a 24 px
  rhythm and an 80-character measure; code, diffs and terminals span the full column like figures.
  Geist Mono for code, with monochrome syntax (weight and brightness, no hue).
- **Steps, not boxes.** Thoughts and tool runs are quiet one-line steps on a hairline guide. A
  thought shows its newest lines in a three-line window, then folds into "Thought for 6s". A live
  run keeps its present tense ("Reading pacing.ts") until the next step starts. Nothing that
  opened itself folds while the turn is live: a passed command shrinks to its last four lines, and
  folds only in history. The plan sits in the dock while the agent works through it, then joins
  its turn as a step where it last changed.
- **Motion.** Crisp and short: 160 ms fades and a 6 px rise for new blocks. What moves the text the
  reader is reading (the transcript following new output, the dock giving back the plan's room)
  glides on a soft curve (`GLIDE`), never more than about one line per frame. Streamed text wraps
  plainly, so no word ever jumps a line (text that was whole on arrival wraps "pretty"). Reduced
  motion keeps every state and drops the movement.
- **Answers and switches.** A permission card takes Cmd+Enter (allow once) and Esc (reject), only
  while it can be seen; "Always allow" takes a click. Typing in the composer while it waits turns
  the request down and sends the text instead. Switching agent in a chat shows who takes over and
  the exact handoff block that will be sent, nothing else.

## Layout

| | |
|---|---|
| Sidebar | 264 px glass slab inset 6 px, corners concentric with the window (radius 12) |
| Reading column | 720 px, centred; prose 600 px (about 80 characters) |
| Composer dock | 752 px (the column plus 16 px a side, so input text aligns with the column) |
| Edges | nothing in the title bar's 52 px or the 16 px above the dock; fades over 44 and 40 px |
| Radii | 6 chips, 8 rows, 12 wells, 16 cards and overlays, 22 composer (squircles via `GlassPanel`) |
| Type | 11 / 12 / 13 / 15 px Geist on 16 / 20 / 24 px lines, 12 / 12.5 px Geist Mono, 18 / 19 / 21 / 26 / 36 / 44 px Instrument Serif |
| Rails | glyphs and prose at 0, step labels and the byline name at 26 px |
| Ink | `--n-ink-1` to `--n-ink-4` all keep 4.5:1 or more on the smoke, dawn wallpaper included |

## Files

- `App.tsx`: the window (backdrop, sidebar, aura, title bar, transcript or new chat, dock), local
  composer state, permission answers, the handoff, Esc to stop, the status line for screen readers.
- `Sidebar.tsx`: wordmark, new chat, search, projects with live mini-orbs and status words, agent
  dock, account.
- `Transcript.tsx`: following and gliding, headline, turns, live bylines, plan placement, the
  starting placeholder, notices, shrink-wrapped user bubbles, the turn footer.
- `Thought.tsx`, `ToolRun.tsx`, `Permission.tsx`, `Markdown.tsx`, `Plan.tsx`: the blocks of a turn.
- `Composer.tsx`, `Picker.tsx`, `state.ts`, `handoff.ts`: composer, autonomy menu, handoff row and
  preview, agent picker, the handoff block.
- `ui.tsx`: motion presets, `Expand`, transcript geometry, mini-orbs, small controls. `text.ts`,
  `syntax.ts`: pure helpers. `noir.css`: every style, scoped under `[data-direction="noir"]`.
  `noir.test.tsx`: tests.

## Run

```sh
pnpm --filter @ghost/design-lab dev --port 5182        # then #/noir/streaming
pnpm --filter @ghost/design-lab shoot --direction noir --port 5182
```

Rules for this folder: scope CSS under `[data-direction="noir"]`, keep overlays inside the window,
take time from the player, honour reduced motion, WebKit first, no new dependencies, no edits
outside this folder, plain short copy, no em dashes.
