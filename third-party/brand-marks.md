# Agent marks

o7 shows each coding agent's own logo beside its name, in the desktop app and on
o7.codes. The marks live in `packages/ui/src/graphite/agent-logos/` and are drawn by
`AgentMark` and `AgentLogo` (`packages/ui/src/graphite/`). Five follow terms their makers publish
for this kind of use; four (Claude Code, Grok Build, Antigravity, Hermes) are used at the owner's
direction while the owner handles permissions with those vendors (2026-10-28).

**These marks are trademarks of their respective owners.** They are not part of o7, are not
covered by o7's license (whichever the owner chooses), and are not licensed to anyone who
copies this repository: each vendor grants its own permission, to each user, on its own terms. They
are used only to say which agent a chat uses (nominative use). o7 is not affiliated with,
endorsed by or sponsored by any of these companies or projects.

Retrieved 2026-10-28. Hashes are SHA-256 of the file as downloaded.

## How the marks are used everywhere

- **Unaltered.** The vendor's own path data and colours, the variant it publishes for dark
  backgrounds. Optimising removed only the export wrapper: XML prolog, comments, CSS classes
  (replaced by the same fill on the path), and masks and clips that covered the whole artwork and
  so had no effect. The viewBox is cropped to the artwork's bounds; the stored files say so below.
- **Never recoloured, dimmed, filtered, stretched, skewed, rotated or cropped.** Optical balance
  between marks uses uniform scaling only (`SCALE` in `agentLogos.ts`). A "not installed" agent
  keeps its mark at full strength in a dashed tile; the dashes and the words say it is missing.
  One exception to "rotated": Nous Research's own site shows the Hermes wing turned 45 degrees, so
  o7 draws it the same way (`TURN` in `agentLogos.ts`); the stored file stays upright, as
  published. A mark published in `currentColor` (Grok's) takes o7's foreground colour, as
  it takes the foreground on its maker's site.
- **Clear space.** A mark sits in the app's agent tile with an even margin (2px at 16px and below,
  about a sixth of the tile above that) and never touches other artwork or text; the agent's name
  sits beside the tile.
- **Size and prominence.** Tiles are 14 to 56px in the app and 16 to 24px on the website, smaller
  than o7's own logo where both appear. No mark is used in o7's name, logo, app icon
  or headline, and none is combined with other elements into a new mark.
- **Website motion** moves a whole mark (a rise, a 1.5px drift, a 3px lift on hover) and changes
  nothing about the mark itself.
- Anything else (state loaders, the waiting dot, the done check) takes the mark's place; it is
  never drawn over a mark.

## Marks in use

### Claude Code: the Claude spark

- File: `claude-code.svg`, from `Claude logos/3 Claude Spark/SVG/Claude Spark - Clay.svg` in
  Anthropic's press kit, https://www.anthropic.com/press-kit (which serves
  https://www-cdn.anthropic.com/ae59ca4ca194dac9c9dc3bc78c5829468cb0e8af.zip). Zip
  `c68ac92df86c825f95177e24016fcc9a8863a3fd4ca344fe6f0700b2c1e07151`, SVG
  `6d53db4be375e899c937c26cf16684a80d6e869b1928d72b37748bef2560e219`. The spark is published in
  Clay (`#D97757`) only, so it is used in Clay. The kit's Claude Code logo is the spark beside the
  words "Claude Code"; the spark alone is used because the name sits beside it. viewBox cropped
  from `0 0 94 94` to `0.4 0.2 93.6 93.6`.
- Used at the owner's direction; the owner is handling permissions with the vendor. Anthropic's
  guidelines: https://www.anthropic.com/legal/trademark-guidelines (no changes of colour, font or
  proportion; reasonable space around the mark; no implied sponsorship).

### Grok Build: the Grok mark

- File: `grok.svg`, the Grok mark exactly as xAI's own Grok page draws it (the inline SVG beside
  "Grok" on https://x.ai/grok, read from the page as a browser loads it on 2026-10-28; two paths
  in `currentColor`, viewBox `0 0 34 32`). Captured markup
  `ba8b4ece6ce7f2019b89f3b1bab1d1a67fb5c94385363620ab91687d9e922ff4`, as SVG
  `add74f0bb9fc082dac934027023b85b1d380a60c37f01226d4a9680615059eb6`. viewBox cropped to
  `0.5 0 33.33 32`. xAI's logo download (https://data.x.ai/logos/SpaceXAI_Grok_Assets.zip) sits
  behind a bot check that refuses automated retrieval; it was not worked around. xAI publishes no
  separate Grok Build mark.
- Used at the owner's direction; the owner is handling permissions with the vendor. xAI's
  guidelines: https://x.ai/legal/brand-guidelines (accurate reference only; no endorsement; nothing
  near the mark that makes a new mark; no sublicensing).

### Antigravity: the Antigravity icon

- File: `antigravity.svg`, from Google's Antigravity press page, https://antigravity.google/press
  ("Icon - White"): https://antigravity.google/assets/image/brand/antigravity-icon__white.svg. SVG
  `5b8121138dcb017642ccf15d9f5cda738e61d8d70a12277c9cf5aeec3e575e81`. viewBox cropped from
  `0 0 180 180` to `22.5 28.125 134.33 123.75`.
- Used at the owner's direction; the owner is handling permissions with the vendor. Google's brand
  rules: https://partnermarketinghub.withgoogle.com/brands/google/branding-guidelines/how-to-show-googles-brand/.

### Hermes: the Hermes wing

- File: `hermes.svg`, the wing that the Hermes Agent site (https://hermes-agent.nousresearch.com/)
  shows as its header logo:
  https://web-assets.nousresearch.com/marketing/b45356e6366b289aa373bef2ad42d009711fc9e2/assets/hermes-landing/teams/hermes-wing.svg.
  SVG `6ee276e9bff5a166c41bc8e80c6717c525cb9b9e81a1111348559452a711e8cd`. Its two paths and
  colours (`#F2F2F2`, `#0000F2`) are kept; the group's even-odd rules moved onto the paths, and
  its clip, a rectangle the size of the viewBox, was dropped (it trims under 0.2% of the
  artwork's height). viewBox unchanged. The site turns it 45 degrees, and so does o7.
- This is Nous Research's logomark for Hermes Agent. The repository's other marks
  (https://github.com/NousResearch/hermes-agent/tree/main/assets, the figure on `nous-girl-*.svg`
  and `icon-master*.svg`) are detailed illustrations that do not read under 24px; the Hermes bot
  wears its app icon at 28px and up (Assistants linked as bots, below). The wing reads
  as a wing from 24px; at 14 and 16px it is a small blue stroke, the smallest it can be drawn.
  The file is about 51 KB because the engraving is traced to pixel steps.
- Used at the owner's direction; the owner is handling permissions with the vendor. Nous
  Research's terms: https://portal.nousresearch.com/terms (12.1).

### Codex: the OpenAI Blossom

- File: `codex.svg`, from `OAI_OpenAI-Blossom_White.svg` (white, for dark backgrounds) in
  https://cdn.openai.com/brand/openai-logos.zip, offered at https://openai.com/brand/ ("Download
  logos"). Zip `c54e85ab5884228f89f0230dd8effa8d588cad78166fe954135f4afa553222db`, SVG
  `01d158767c4eec0e47bd617e67759c33da0accd1438be1a8d29dfdb99ce87285`. The viewBox is cropped from
  `0 0 716 716` to the Blossom's bounds, `180.5 180.5 354.67 354.67`.
- OpenAI publishes no Codex-specific mark; the Blossom identifies OpenAI's services, and Codex is
  one.
- Guidelines: https://openai.com/brand/ (usage terms on the same page). Followed: used only where
  it directly relates to an OpenAI service; exactly as provided, with no added colour, effect or
  element; not the primary branding and never more prominent than o7's own marks; no implied
  partnership, endorsement or sponsorship; not on merchandise; not part of o7's branding.
- Terms: OpenAI owns the marks; its permission is non-exclusive, non-transferable (it cannot be
  passed on to others, hence the note above), may be reviewed or ended at any time, and follows
  updates to the guide. Requests: partnercomms@openai.com.

### Cursor

- File: `cursor.svg`, from `General Logos/Cube/SVG/CUBE_2D_DARK.svg` (the flat cube for dark
  backgrounds, `#edecec`) in
  https://ptht05hbb1ssoooe.public.blob.vercel-storage.com/assets/brand/cursor-brand-assets.zip,
  offered at https://cursor.com/brand. Zip
  `97488a7751914e60f9ff532bc33810cdeaebdddc017548abe6ca2bc29bbc3928`, SVG
  `cd0e3e5d8991a4cdd4577f8896cd063105207665165c73e25a1ff918dd367eb7`. viewBox unchanged.
- Guidelines: https://cursor.com/brand ("Resources to represent Cursor consistently and
  accurately"; "Refer to us as Cursor. Not Cursor AI or Cursor Code."). No clear-space,
  minimum-size or permission terms are published; the general rules above apply.

### OpenCode

- File: `opencode.svg`, from
  https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/console/app/src/asset/brand/opencode-logo-dark.svg
  ("Logo, dark" on https://opencode.ai/brand; the same artwork is in
  https://opencode.ai/opencode-brand-assets.zip). SVG
  `5dd4abb4de60204510edf140bbe74b3580d07ebdda79fdb0081cd45acb53d44b`. Its two colours
  (`#F1ECEC`, `#4B4646`) are kept; a full-artwork mask and clip with no effect were removed.
- Guidelines: https://opencode.ai/brand publishes the assets with no usage terms. The OpenCode
  README asks projects that use the name to make clear they are not affiliated; o7 states
  that above and on its notices page. Repository license MIT, which covers the code, not the
  trademark.

### Pi

- File: `pi.svg`, from https://pi.dev/favicon.svg, the "Badge: Square mark for favicons and
  compact badges" on https://pi.dev/press-kit. SVG
  `d266b2c1d2c7bf169ca30d438963b176fba16fea6263b9ac30c8302f34ad7ce7`. The file switches its fill
  by colour scheme (`#111111`, or `#f6f6f6` for dark); o7 is dark only, so the stored mark
  uses the vendor's dark-scheme fill `#f6f6f6` and drops the media query. Paths unchanged.
- Guidelines: the press kit publishes no usage rules. Copyright "Earendil Inc. & Contributors",
  MIT License, which covers the code, not the trademark.

### oh-my-pi

- File: `oh-my-pi.svg`, from the project's own `assets/icon.svg`,
  https://raw.githubusercontent.com/can1357/oh-my-pi/main/assets/icon.svg. SVG
  `727019727006f26fe3d1f187d3fe9830f26213733a51721096a2044ab059cf61`. The same rects and circles
  and colours (`#fafafa`, `#f97316`, `#0d0d0d`); comments removed; viewBox cropped from
  `0 0 120 90` to the artwork, `10 8 100 74`.
- No brand kit or usage terms. The project's site (https://omp.sh) uses a different favicon, a
  gradient pi on a dark tile; the repository icon is used because it is the project's logo
  artwork on a clear background. Ask the maintainers (Stencil Labs, Can Bölük) which mark is
  canonical before a public release. Repository license MIT, which covers the code, not the
  trademark.

## Model makers

While an agent works, the chat's working line shows the mark of the company whose model is
working (`ModelMark`, `packages/ui/src/graphite/modelMakers.ts`). Used at the owner's direction
(2026-11-03), only to say which company's model is running; no endorsement is implied.

- **Anthropic, OpenAI and xAI** reuse the marks recorded above: the Claude spark, the OpenAI
  Blossom and the Grok mark.
- **The others** are in `packages/ui/src/graphite/maker-logos/`. Their makers' brand files sit
  behind pages that do not serve the plain mark, so each is the exact copy from Simple Icons
  16.36.0 (CC0, https://simpleicons.org), retrieved 2026-11-03.
  - Simple Icons publishes one colour per brand; the files use that brand colour unchanged. Moonshot
    AI's mark is published in black, so it is drawn in `currentColor`, as Grok's is.
  - The hash is SHA-256 of the file as downloaded, before only the wrapper changed (the role, the
    aria-label, the title and the fill attribute).

| File | Company | Simple Icons slug | Colour | Vendor source named by Simple Icons | SHA-256 |
| --- | --- | --- | --- | --- | --- |
| `gemini.svg` | Google Gemini | googlegemini | #8E75B2 | https://gemini.google.com | a6228d8846040401e941a39ed17459785e8324dbb7b870ce84233c9b3e9863a0 |
| `deepseek.svg` | DeepSeek | deepseek | #5786FE | https://www.deepseek.com | 7a55a0a7391d116eba7d32807d6838478f9209f6034612941e74fbb14934e2ef |
| `mistral.svg` | Mistral AI | mistralai | #FA520F | https://chat.mistral.ai | 88f00cf2e82e4d36bb4fdd0e913df200c6e7d6162ec627cc58cfdd6a4eb17d18 |
| `meta.svg` | Meta | meta | #0467DF | https://www.meta.com (guidelines: https://www.facebook.com/brand/resources/meta/company-brand) | e3e9db62f59dc477e5917687349b8aa9685b841d40300c1afad60ad85075cd2e |
| `qwen.svg` | Qwen | qwen | #6950EF | https://qwen.ai | 36854c60b26bfa5a0cc0d4123727c0ef559efba6f129c0dfa623f3c443fe3e5b |
| `moonshot.svg` | Moonshot AI | moonshotai | currentColor | https://www.moonshot.cn | 2b9efa8dd053294bc133f1c57cb618ac75487363bce6425b79dd5c0c92f73569 |

Before a public release, check each against the company's current mark and brand guidelines.

## Assistants linked as bots

Settings' Remote access page, setup and the website name four assistants that can be linked to
o7 as bots (`BotMark`, `packages/ui/src/graphite/bots.ts`). Used at the owner's direction
(2026-11-21), only to say which assistant a link is for; no endorsement or partnership is implied.

Since 2026-11-23 each wears its own logo, at the owner's direction ("Replace all the logos with
the actual logos of the bots"; the owner is handling permissions with the vendors). Before, Grok
Bot wore the Grok mark, Hermes the wing and Muse Meta's mark. Grok Bot, Muse and Hermes are shown
as their app icons: the icon is the whole tile, its corners masked by the tile's as a launcher
masks them, with a hairline just outside it and nothing drawn over it. They are stored in
`packages/ui/src/graphite/bot-logos/` unaltered (bytes as retrieved) and loaded as files of their
own when shown. `BotMark` has its own entry (`@o7/ui/bot-mark`), so the website, which shows no
assistant's logo, does not carry them. Retrieved 2026-11-23; hashes are SHA-256 of the file as
stored.

- **Grok Bot** is `grok-bot.png` (256 by 256), the app icon the Grok Bot desktop app for Windows
  draws (version 1.2.3, by SpaceXAI): `dist/renderer/assets/app-icon-B8tEBssC.png` in its
  `resources/app.asar`, the same artwork as its program icon.
  `43dae095b901722c366e9c8d2680fcd63a3130723ff74fefec73c8c48a2854ce`. xAI's guidelines:
  https://x.ai/legal/brand-guidelines.
- **Muse** is `muse.png` (256 by 256), the icon of "Muse from Meta" (Meta Platforms, Inc.) on its
  App Store listing (https://apps.apple.com/us/app/muse-from-meta/id6760173601), at the size Apple's image service
  serves:
  https://is1-ssl.mzstatic.com/image/thumb/Purple221/v4/80/26/47/80264783-e41c-97e5-f1ea-bc92f321b0fa/HatchAppIconPublic-0-0-1x_U007epad-0-1-0-sRGB-0-85-220.png/256x256bb.png.
  `e6691f80d3e375672ef2d6f9f8026089fdda6f917e3d2538f77ec3c3c67c242f`. Meta's guidelines:
  https://www.facebook.com/brand/resources/meta/company-brand.
- **Hermes** is `hermes.svg`, the master of the Hermes Agent app icon, `assets/icon-master.svg` in
  https://github.com/NousResearch/hermes-agent at commit `4ea79443bc8e7349adb1f6516a3da3fae8225cd6`
  (https://raw.githubusercontent.com/NousResearch/hermes-agent/4ea79443bc8e7349adb1f6516a3da3fae8225cd6/assets/icon-master.svg).
  `6c4968c1a46f104559684826ed8061fad6052711ff0af9c6b36a6ce674536684`. Shown as an image, so its
  inline style and clip stay as published. Nous Research's terms:
  https://portal.nousresearch.com/terms (12.1). The Hermes coding agent keeps the wing above: the
  figure does not read at the agent tile's 14 and 16px.
- **OpenClaw** is `packages/ui/src/graphite/bot-logos/openclaw.svg`: the mark its own app draws,
  `dist/control-ui/favicon.svg` in the `openclaw` package 2026.9.2
  (https://unpkg.com/openclaw@2026.9.2/dist/control-ui/favicon.svg, retrieved 2026-11-21, SHA-256
  74e1fb7542675f68aafe502576f7c857c3f3620d07095e910515e3c05d62ab2e as downloaded). Its shapes and
  colours are unchanged, its gradient included, so it is drawn as an image and not through the
  flat-shape loader; only the published file's animation and comments were removed, and the role,
  aria-label and title added.

## GitHub

The website's header, phone menu and footer link the public repository; the header's link wears
GitHub's mark (`GitHubMark`, `apps/web/app/repo.tsx`), only to say where the source is. Used at the
owner's direction (2026-11-21); no endorsement is implied.

- The exact path of Octicons `mark-github-16` (https://github.com/primer/octicons, MIT, which
  covers the icon files, not the trademark), `icons/mark-github-16.svg` at commit
  `ea8e6bb79894cc7e85564ee9b53d86b418738d04`, retrieved 2026-11-21, SHA-256
  `7421820090b50ac79d7c2bf7c951a433d7098a8a9c474809207bdd45f62d9d46` as downloaded. It is drawn in
  `currentColor`, as GitHub's own pages draw it, at 16px. GitHub's logo guidelines:
  https://github.com/logos.

## Agents without a mark

Any other ACP or custom agent shows the first two letters of its id (`agentCode()`), and an
unknown one "?". Until 2026-10-28 Claude Code, Antigravity, Grok Build and Hermes showed codes
too, because their makers' published terms ask for approval first (Anthropic, Google, Nous
Research) or allow only a download that refused automated retrieval (xAI); the owner chose to use
their marks and is handling those permissions.

## Adding or changing a mark

1. Download the vendor's own published file (its brand kit, press page, or the mark its own site
   draws) into an empty folder. Only when that file sits behind a login or a bot check, use an
   exact copy from a reputable public source (Wikimedia Commons, Simple Icons) and check it against
   the vendor's current mark. Never bypass a login or bot check, and never redraw or trace a mark.
2. Keep its paths and colours; remove only export wrappers and no-op masks or clips, and crop the
   viewBox to the artwork. The file must be one `<svg>` with `role="img"`, an `aria-label` and a
   `<title>` naming the agent, and only flat `path`, `circle`, `rect`, `ellipse` or `polygon`
   elements (`agentLogos.test.ts` refuses anything else: raster, script, style, ids, links).
3. Record its source, hash, guideline and terms here, and add it to the agent list on the notices
   page (`.github/scripts/third-party-notices.mjs`).
