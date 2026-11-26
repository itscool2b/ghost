# onghost.dev

React Router 7 interface that follows the system appearance, prerendered to static HTML. Cloudflare serves `build/client`.
`entry.server.tsx` is build-time React rendering only; the server output is removed by `ssr:false`.
There is no application backend and no account: the app is local-only, so the site has no
sign-in, account or sync pages. The one page that talks to a service is `/remote`, which pairs a
browser with a computer through the relay in `apps/relay` ([remote-sessions.md](../../docs/remote-sessions.md)).

## Local checks

```powershell
corepack pnpm --filter @ghost/web typecheck
corepack pnpm --filter @ghost/web test
corepack pnpm --filter @ghost/web test:browser
corepack pnpm --filter @ghost/web dev
```

Browser checks use owned headless Chromium and WebKit and block external requests
(`pnpm --filter @ghost/web test:browser chromium` runs one engine; Playwright's WebKit does not
start on Arch Linux, which lacks the Ubuntu libraries it is built against). They cover
dark and light with the system, desktop/mobile overflow, keyboard focus, the agents' chips at every
width (complete, in view, still), as toggle buttons and with the chosen one's mark, the six feature
cards, the FAQ's first answer standing open, the header's Download waiting for the hero's on phones,
the phone menu, no canvas and no Pause button, and no-JavaScript downloads. Axe checks the home and changelog views. Chromium verifies native Tab
order; Windows WebKit lacks the OS full-keyboard-access setting, so that harness explicitly focuses
the skip link and verifies keyboard activation. Real Safari/macOS acceptance remains pending.

React Router 7.18.4 supports TypeScript through version 6 in its peer contract, so this package pins
TypeScript 6.0.3 locally. The desktop/workspace compiler remains unchanged. The website uses the
shared Geist font package and semantic tokens from `@ghost/ui`.

## Reference evidence

The website was rebuilt for Main on 26 November 2026, as [visual-language.md](../../docs/design/visual-language.md) describes under "Website composition": always dark, one ink and one grey, two-tone headings at the left edge, chapters on hairlines and the product on monochrome stages. [reference-audit.md](../../docs/design/reference-audit.md) records the pages it was measured against (polar.sh, linear.app, ghost.ai and x.ai/bot) and their limits. No third-party reference artwork or brand assets are shipped.

The page shows no screenshot of the app. The hero window and the views of what the app does are
drawn in HTML from tokens, after the app's own Main look and preview modes (a pending handoff, the
queue, the Changes panel, a terminal chat, the browser in Annotate and the Dashboard); update them
with the app. The stages' field is drawn by one function in `app/home/Stage.tsx`, as an SVG when
the page is built and on a canvas once the page can move. The favicons and social card are
generated at build time by `scripts/site-images.ts` from the logo's own paths. Without JavaScript
the page is whole: every row's words are in the markup, the first view stands, the field is its
still drawing and the FAQ opens natively. The site is always dark, with no appearance control and
no persisted theme; the standalone notices page is script-free and dark too. The changelog page is
built from the repository's `CHANGELOG.md` by `scripts/changelog.ts`. Reduced motion holds the
live window, the field and the list of views still.

## Deployment prerequisites

The assets-only `wrangler.toml` remains available. The separate [Rust website Worker](../web-worker/README.md)
embeds the same public build for ordinary code deployment without an Assets upload. It is deployed
at [onghost.dev](https://onghost.dev) and [preview.onghost.dev](https://preview.onghost.dev).
Downloads are not published. Production styling deploys automatically from main after merge through Cloudflare Workers Builds;
verify the deployment against the merged commit. Deploy only one website implementation per route
and rebuild from the final integrated source. The generated `_headers` supplies a hash-based CSP
for the exact prerendered framework scripts. Rebuild headers
whenever HTML changes. No third-party script or frame origin is permitted; the only other origin
the pages may connect to is the relay's WebSocket on the site's own hosts.

Public installer signing is deferred at the owner's request. Downloads remain disabled until
reviewed Windows/macOS installers, per-artifact SHA-256 checksums and release notes are published
under [docs/releases.md](../../docs/releases.md). Unsigned package compilation and native installation
checks remain separate acceptance gates, including the supported ARM64 builds. Final operator identity, legal
contacts and legal review are required before a public release.
The pre-release legal pages explicitly state their draft status.
