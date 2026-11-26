# Halo

Spatial glass: the window is a set of floating glass slabs at different depths, and the agent's
orb sits in a ring of its own light. Rounder, softer and more tactile than the other directions,
with an airier tint, still dark-first. Geist throughout (larger sizes, tight headings), Geist Mono
for code.

This folder belongs to the Halo designer. Only that designer edits files here.

## The idea

- **Planes.** Far to near: the agent's aura field, the window glass with Halo's tint, the sidebar
  (dimmest), the turn cards, the status slab tucked under the composer, the composer, and menus
  nearest. Nearer glass is brighter, has a crisper top edge (a 1px specular) and a longer shadow,
  so depth reads in a still frame. Parallax adds to it: the aura drifts against the pointer and the
  dock drifts with it, and everything holds still while the pointer is in the dock or a menu is
  open, so nothing you aim at moves (off under reduced motion).
- **Glass.** Every slab is a squircle (`squirclePath` from `@ghost/ui`) with three layers: a
  fill, a bevel (white light along the top inner edge, the agent's colour refracting along the
  bottom one), and a 1px rim drawn as an SVG stroke on the same path. Wells (code, diffs, terminal,
  the thought) are recessed.
- **The halo.** Send is the agent's live orb in a 3px ring of its colours with a blurred bloom, so
  it reads as light. The ring turns while the agent works (Send is Stop, or Queue once you type);
  waiting on you, it holds still and breathes amber, since rotation only ever means working. The
  turn card's orb wears the same ring. The empty chat shows the orb in a glass lens with a tilted
  halo orbiting it.
- **One status line.** The slab under the composer always says what is going on: while a turn runs
  it is the status line (a mini-orb, a verb and what it acts on, the plan's pips and count, the
  turn's one clock, and Escape to stop). A verb stays up at least a second and survives short
  thoughts between tool calls, so it never flickers. After a stop it says so; after you pick
  another agent it says the next message carries a handoff; otherwise it holds the plan, folded,
  with its checklist one click away. Messages typed while the agent works wait under it.
- **Rows, not walls.** Tool groups stay one row ("Edited pacing.ts, pacing.test.ts +59 −23",
  "Ran pnpm test · 19 passed") and open to diffs, output and results. A running command shows its
  last three lines, then folds to its result; a failure or a waiting call stays open. Opened, a run
  is one container: the header strip over one dark body. Long code lines wrap with a hanging
  indent from their own indentation instead of being cut off.
- **Motion.** Turn cards settle in with a spring; blocks open their space and rise in; streamed text
  inks in a delta at a time without moving earlier lines. The view follows the stream and lets go
  the moment you scroll up by any means; "Latest" brings it back.
- **Keys.** Escape closes the topmost layer first: the autonomy menu, the picker, a pending
  permission (rejected), the running turn (stopped; a second Escape forces it). Menus take focus
  when opened, keep Tab inside, and hand focus back to their trigger. The picker is three listboxes:
  arrows move, Enter chooses, since choosing an agent cross-fades the whole window.

## Type and colour

One scale: 12 (meta, keys, counts), 13 (secondary), 14 (rows, menus), 16 (titles, the input),
17/28 (the message body, held to a 36em measure), 20 (headings), 44 (the hero). Weights 400, 500,
600; 650 only for the hero and the wordmark. Text tokens `--h-text-1..3`; `--h-text-4` is for icons
and separators only. Every label under 18px clears WCAG AA on the dusk, dawn and graphite
wallpapers (measured against the real pixels behind it, in WebKit).

## Files

- `App.tsx`: the window: planes, parallax, selection, the keyboard layer, the lab's stop and
  queue, live announcements.
- `keys.ts` (+ test): what Escape and Enter do, topmost layer first.
- `status.ts` (+ test): the status line's verb, target, and hold.
- `primitives.tsx`: `Glass`, `Collapse`, entrances, parallax, focus helpers, small controls.
- `text.tsx`, `syntax.ts`: streaming markdown with ink, code blocks, a small highlighter.
- `transcript.tsx`: the scroll that follows the stream, its scrollbar, turns, the agent card, the
  footer.
- `blocks.tsx`: the thought pill, tool runs, search results, diffs, terminal output, the
  permission card.
- `composer.tsx`: the composer, the orb Send, the autonomy menu, the agent > model > effort picker.
- `chrome.tsx`: the header, the status slab, the empty-chat hero.
- `sidebar.tsx`: the floating sidebar, mini-orbs, the agent dock.
- `halo.css`: every style, scoped under `[data-direction="halo"]`.

## Notes for M2a

- Real Liquid Glass adapts to bright wallpapers by itself; the lab's stand-in does not, so
  `.halo-guard` caps how bright the glass behind Halo can get (a `darken` blend that leaves dark
  wallpapers alone). Over native glass it blends with nothing and does nothing.
- Geist ships no italic in `@ghost/ui/fonts.css`, so comments are marked by colour alone and
  `em` is a synthesized slant until the italic faces are imported there.
- Backdrop blur only on small overlays (menus, the picker, "Latest"); the blurred element itself
  fades, never a parent, so the blur never snaps in.
- Clipping containers use `overflow: clip`, not `hidden`: a hidden box can still be scrolled by
  focus, which would shift the whole window.
- Answers, Stop, the queue, the picker and the draft are local to the lab: a stop pauses the
  recording and the turn reads as stopped until it plays again; queued messages clear when the
  turn ends.
