# @ghost/ui

The Ghost design system, shared by the desktop app and onghost.dev. It ships as TypeScript source (no build step), with React 19 as a peer dependency.

**Graphite** (`@ghost/ui/graphite`) is the current visual language: the Grok Bot app's flat look (owner, 17 November 2026), four solid layers and one ink at set strengths, with no blur and no shadows in dark, cut into a palette for every supported agent. The spec is `docs/design/visual-language.md` and the measured reference is `docs/design/reference-audit.md`. The older **Aura** (`@ghost/ui`: Orb, AuraField, GlassPanel) and **Orbit** (`@ghost/ui/orbit`) modules stay for the Design Lab; new work uses Graphite.

```tsx
import "@ghost/ui/fonts.css"; // Geist and Geist Mono, bundled for offline use
import "@ghost/ui/graphite.css"; // tokens, the opt-in .g-root base layer and the g-* primitives
import { AgentMark, Keycap, Wordmark } from "@ghost/ui/graphite";

// With class="g-root" on <html>:
<main>
  <Wordmark size={16} />
  <AgentMark agent="claude" state="thinking" size={16} />
  <button type="button" className="g-button g-button-primary">
    Allow once <Keycap>↵</Keycap>
  </button>
</main>;
```

- `@ghost/ui/graphite-tokens.css` declares only the `--g-*` variables on `:root`; `@ghost/ui/graphite.css` adds the base layer (opt in with `.g-root`) and the primitives. Every primitive is one class with its states inside `:where()`, so one app class loaded later always wins.
- **Tokens.** `graphite/tokens.css` holds Aura's own palette, the type roles, radii, motion and the per-platform weight grades (`data-os` on the root). The layers are `--g-window`, `--g-glass-layer` and `--g-glass-pop` (solid, despite the historical name); fills and edges are the ink at set strengths (`--g-glass-control*`, `--g-glass-rim*`, `--g-fill-primary-disabled`); shadows are `--g-shadow-control`, `-inline`, `-layer` and `-pop`, empty in dark where the reference draws none. `graphite/tokens.ts` mirrors the colours for code, and `tokens.test.ts` keeps the two in step, checks contrast on every layer and rejects any blur.
- **Themes.** `graphite/themes.ts` is the source of every agent's palette; `themes.css` is generated from it with `node packages/ui/scripts/write-themes.mts`, and `themes.test.ts` fails on drift or on contrast.
- `Keycap` sets one key in Geist Mono and draws the key symbols Geist Mono lacks (⌘ ⇧ ↵ ⌥ ⌃ ← → ↑ ↓ ⌫) as strokes in its own ink, named for screen readers.
- `AgentMark` replaces `OrbitMark` with the same props. Idle, it shows the agent's own published logo (`agent-logos/`, sources and terms in `third-party/brand-marks.md`), or its two-letter code where the maker allows no logo use; `AgentLogo` draws a mark alone. The marks are their owners' trademarks and are not covered by Ghost's license. Idle, hidden, offscreen and reduced-motion marks own no animation frame.
- `LOGO_PATH`, `LOGO_SMALL_PATH` and `logoSvg()` are the source of the logo, the app icons and the favicons. `LOGO_OUTLINE_PATH` and `LOGO_EYE_PATHS` are the same wing in parts, for a picture whose eyes move.
- The water, its swimmers and the dither field are gone (no animal or water motifs; the logo has been a flying wing since 19 November 2026).
- Checks: `pnpm --filter @ghost/ui test`, `pnpm --filter @ghost/ui typecheck`, `pnpm exec biome check packages/ui`.
