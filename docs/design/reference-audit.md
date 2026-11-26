# Reference audit

Status: the current references are those of Main, the styling pass of 25 November 2026, in the first section. Everything after it supported earlier passes (Halo on 1 November, the website rebuilds of 10 and 17 November) and is kept as the record of where today's details came from. The observations do not establish visual approval or complete product parity.

## Main, 25 November 2026 (current)

The owner asked for "the greatest styling pass of all time": a default look for the desktop called Main, a new website, and the remote page to match. For the desktop the models are the Grok Bot desktop app, audited below on 17 November, and the Synara ADE. For the website: polar.sh ("look how clean and sleek"), linear.app, ghost.ai ("the attention to detail is awesome") and x.ai/bot. Every page was walked from top to bottom in Ghost's own browser, section by section, at the owner's word ("make sure u actually scroll through the whole landing pages"), with computed styles read from the page. Nothing of any of them ships: no code, font, icon, artwork or copy.

### Synara (the desktop's second model)

Synara is a desktop workspace for coding agents (trysynara.com). The owner believes the program is unsafe and asked that it never be installed or run, so its look was read from its public pictures and from the stylesheet in its public repository, as text.

- **Palette.** Dark is one near-black (`#0e0e0e`) with `#f5f5f5` ink; a card is that black plus 1% white, and fills and borders are white at 4 to 5%. Light is `#fcfcfc` with borders at 5% black. Colour appears only where it means something: diff counts, a status dot, an agent's mark.
- **Shape.** A 10px base radius (6, 8, 10, 14, 18, 22); the sent message 16px; the message box 19px, with a tray behind its top edge a few pixels larger; the content block inset 3px with a half-pixel hairline.
- **Type and controls.** A system grotesque at about 13px with a mono face for code. Buttons 24, 28 and 32px, flat, no shadows. Icons 14 to 16px in a thin stroke, at 80%.
- **Rows.** The thread list is dense: the agent's mark, the title, then project, pull request and branch in grey on a second line.
- **Transcript.** "Worked for 25s" folded on a hairline, answers unframed, a row of tiny ghost icons and a time under each answer, a soft right-aligned bubble for what was sent, and a card listing the files a turn edited with their counts, Undo and Review.
- **Message box.** A tall soft rectangle: the words on top, one row of bare controls under them (a plus, the access level, a context ring, the model with its effort, a microphone) and a round Send. Notices sit in a tray tucked behind its top edge. The command menu is a panel above it, the box's own width: icon, name, grey description, the command at the right.
- **Settings.** A sidebar with a search field, a page title with one line under it, and one bordered card of hairline rows with the control at the right of each.
- **Its own website** is weaker than its app, as the owner said, with one idea worth keeping: each feature is shown as one crisp fragment of the real interface over the rest of the app drawn as grey skeleton bars.

Taken for Main: the inkier dark layers, the thin icon stroke, three control heights and a 30px row, the agent's mark on every chat row, the soft sent message, the message box's bare controls and its tray, quiet menus with grey descriptions on the row, and hairline cards of rows. Not taken: its icon rail and thread tabs (a change of structure, not of look), its second line of Git details on a row (fewer chats would fit), its rectangular buttons (Ghost keeps the Grok Bot app's pills), its orange accent.

### polar.sh

- **Page.** Near-black (`#0a0a0a`), one ink and one grey, hairlines, and no colour at all except meaning (one red figure in a table).
- **Type.** One grotesque at weight 400 even in headings, and small: the first heading 36/45, section headings 30/41, the closing line 48/60, grey leads about 26px. Every heading is two-tone: its first line in ink, the continuation in grey.
- **Hero.** The two-tone heading at the left edge, one pill, then a stage: a monochrome, grainy field of slow dunes on a canvas with a product card standing on it, and beside it a statement and a list of features that changes what the card shows.
- **Chapters.** Each is a hairline, a name at left with the heading and a grey lead at right (the right column starts at half the width), a full-width stage with the product on it, then smaller cards. The stages are one texture in different tones.
- **Details.** Cells divided by hairlines with thin line diagrams; square cards of generative line bursts; a code panel with line numbers; a five by two grid of tiles whose corners are ticks, not borders; pricing as nested panels; a finale of letters moving on a canvas; a footer with the logo large at left and four groups of links.
- **Motion.** Canvases and a few CSS animations. Nothing takes over the scroll.

### linear.app

- **Page.** `#08090a` with a hairline under the header. The first heading is at the left edge, 64/64 at weight 510 with -0.022em tracking, over a grey 17px lead; the product window is drawn in HTML and runs off the right and bottom edges.
- **Statement.** One 40px paragraph whose first sentence is ink and the rest grey, then three isometric line drawings in cells split by hairlines.
- **Chapters.** A 48px two-line title at left, a 26px lead at right; an overlapping composition of real interface that fades into the page at its edges (a thread over a board, a chart over a timeline, agent windows side by side, an issue list over a split diff); a row of feature names that open; a full-width hairline. Colour lives only inside the product.
- **Changelog** as a horizontal timeline of dots on a hairline, the newest marked. Motion is quiet: a typing caret, hover states.

### ghost.ai

- **Cinema.** Full-viewport scenes alternating dark and light, fine grain over everything, one rendered object as the subject.
- **Scroll sequences.** Long pinned passages carry the story: the object opens into a pale blueprint and one part at a time comes into focus with a callout while every other label goes soft; a three-step story whose three tabs sit at the foot with only the current one sharp; a grid-paper stage where a segmented thumb slides between groups as the page moves.
- **Finish.** Hairline tables, counters, an exact grid visible as lines, a wordmark pressed into a brushed sheet at the foot.

Taken for the website: polar.sh's skeleton, restraint, two-tone headings and stages; Linear's product compositions that fade into the page; one scroll sequence in ghost.ai's manner, with focus as its motion; x.ai/bot's live app window, kept from the page below. Not taken: numbered kickers and uppercase labels (no eyebrows), square corners (Ghost is pills), video (the site's Worker serves none), any colour that does not mean something.

## dot.com, 17 November 2026 (superseded by Main for the website's look)

The owner found the 10 November copy wanting ("The website looks trash too ... use the browser for .com, and essentially copy that as well ... keep using this logo ... everything needs to just be on point"). dot.com still redirects to x.ai/bot ("AI teammates that finish the work | Grok Bot"), but the page has changed since 10 November: a statement with a row of bots and a film now follow the hero, the features are six equal cards, and guides sit where the news was. It loaded in plain headless Playwright Chromium (no stealth or evasion; it served the page with no challenge), so this time it was captured and measured in both appearances (`colorScheme`) at 1440, 1024, 768 and 390 wide: full-page screenshots, computed styles of every text and box read with `page.evaluate`, the hero's entrance sampled every 40ms, the header and every section watched while scrolling, and the phone menu opened. Its cookie banner was never clicked; it was removed from the local copy of the page for the screenshots. The captures stay outside the repository.

Measured, both appearances (dark / light):

- **Palette.** Page `#0a0a0a` / `#ffffff`; ink white / `#0a0a0a`; cards `#1a1a1a` / `#f9f8f6`; the small quiet pill (header, a section's) `#202020` / `#f2f1ef` (ink at 3% over a card), the large one `#2c2c2c` / `#edecea` (8%), its words `#d5d9e2` / `#0a0a0a`; hairlines `#1f2228` / `#d5d9e2`; secondary text `#7d8187` in both, which is 3.9:1 on white and 4.4:1 on the dark card; the primary pill is the ink, hovering to `#f9f8f6` / `#1f2228`. The header is the page at 85% under a 12px blur, with no rule.
- **Type** (its own X VF face). Hero name 60/60 at weight 500 and -0.01em at 1440 and 1024, 48/48 at 768, 36/37.8 at 390. Statement title 48/48 (36/40 at 768, 30/36 at 390). Section titles 36/40 (30/36 at 390). Leads 18/29.25 (16/26 at 390), grey under the hero and the download title, ink under the centred titles. Card titles 20/28 at 500 over 16/26 ink. FAQ questions 16/24 at 500, answers 16/28. Navigation 13/19.5 at 500; footer headings 13/19.5 at 500 over 13/21 links. Chips 14/20 at 500.
- **Columns.** A 1280px container with 24px padding (1232px of content) for the header, statement, film, feature cards and footer; a 1024px one (976px) for the hero window and the other sections; 16px padding on phones. Centred titles and their lines keep to 672px.
- **Rhythm.** 112px above and below every section at 1440, 1024 and 768, 80 at 390; the statement's title sits right at the hero's foot, its row of bots 80px under the paragraphs, the film 112px under the row and the features' title 176px under the film. Features: the title, its line 16px under it, the cards 56px under that.
- **Hero.** A badge above the name (36px pill, 13px), the name with the mascot (a disc 0.92em across) set into the line, the lead 20px under it, the pills 28px under that (44px, 16px words, 24px padding, 12px apart; 36px with 14px words at 390), and the app window 64px under the pills: 976 by 660 (720 at 768, 358 at 390, 660 tall throughout) with 24px corners and a hairline of the ink at 10%; at 390 its sidebar is an 84px rail of avatars.
- **Cards.** Features: six cards two to a row, 596 by 414 at 1440, 40px apart (32 at 768, 24 at 390), 24px corners, 32px padding (24 at 390), the title and text over a picture that starts 32px under the text and runs off the card's foot. Download rows: 456 by 80, 16px corners, 16 by 20 padding. Guides: three cards 315 by 334, a 166px picture over a 14px grey category and 15px text, a 44px quiet pill 32px under them.
- **Chips.** 32px pills with a hairline ring (`#1f2228` / `#d5d9e2`) and grey words at rest; the chosen one shows its bot's avatar and takes its bot's colour: a faint tint, a ring at 28% and its words in the colour (`#4ad8ec` / `#0093a3`, the latter 3.7:1 on white).
- **Header.** 64px at 1440 with the links in 32px pills 40px after the logo, a quiet 40px "Contact Sales" and a 36px split Download at right. Under its link breakpoint (at 1024 and below) it is 80px (84 at 390): the logo, a 40px Download and a quiet 48px disc (52 at 390) for the menu; that Download stays hidden while the hero's own is in view and fades in once it has scrolled away. The menu is a sheet over the whole page: 18/28 medium rows 52px tall between hairlines, Download at its foot.
- **Footer.** A hairline across the 1280px container, 40px over the logo and a 10px copyright line, a dotted rule, and five columns of links; an appearance switch and a "Built with Grok" pill at its foot.
- **Motion.** Nothing rises as it scrolls in and the header never moves. The opening: the name's words flip up 70ms apart (from 45% down and -40 degrees under a 1200px perspective, about 600ms each), then the badge and lead settle out of a 12px blur from 10px away, the pills 240ms later, and the window from 20px above, about a second each. Colours change over 150 to 200ms on `cubic-bezier(0.4, 0, 0.2, 1)`; download rows lift to the 8% grey under the hand; cards do not move. The pictures play (messages land, a bot types).

| dot.com section | Observed | auracode's copy |
| --- | --- | --- |
| Header | Logo, six menus, "Contact Sales" and a split Download | The wordmark, five section links, one Download; below 1024px the same 80/84px bar, quiet disc, full-page menu sheet and the Download that waits for the hero's to leave. The links stay in the bar at 1024, where five fit |
| Hero | Badge, "Meet [mascot] Grok Bot", two grey lines, two pills, the app window | "Meet [logo on a disc] auracode", the same entrance and measures, the live window in the same frame drawn in the desktop's flat look. No badge (no eyebrows), so the name sits 36px higher |
| "Work with Bots like teammates" | A 48px title at left, two ink paragraphs at right, a row of nine bots across the column | The same two columns, then the nine agents across the column, each mark at the site's 24px with its name: the marks may not be enlarged, rotated or recoloured (`third-party/brand-marks.md`) |
| Film | A 1232px video frame with Play | The real desktop screenshot in a 1232px frame, with its line under it |
| "Everything you'd want in a teammate" | Six cards two to a row | Six cards in the same measures (the handoff, the queue, Changes, the terminal, the browser, the Dashboard), then four smaller cards for the rest of what ships |
| "Give each Bot a job" | Chips beside a phone | The agents as chips beside the New chat view; the chosen chip in its agent's own colour, darkened in light to hold 4.5:1 |
| Pricing | Plans | Not copied: auracode is free |
| "Download Grok Bot" | Title and grey line, two 80px rows | The same, three rows for Windows, macOS and Linux with generic glyphs |
| "Grok Bot Guides" | Three picture cards and a quiet pill | The changelog's three newest days as cards (the day on a square of ink beside its month), then "Read the changelog" |
| "FAQs" | Sticky title, the first answer open, hairline rows closed at the foot | The same |
| "Meet your first Bot" | One line, one sentence, two pills, the mascot drawn in a hairline | The same, over the logo on its disc drawn in a hairline |
| Footer | Logo and copyright, dotted rule, five link columns, appearance switch | The wordmark and its line, the dotted rule, three link columns; no switch |

Kept apart on purpose: the secondary grey is `#80848a` in dark and `#6e7278` in light, not `#7d8187`, so every grey line holds 4.5:1 on the page and on a card (the site's axe checks require it); the face is Geist, not X VF; the hero pill reads "Download" until installers are published. Nothing of dot.com's ships: no logo, mascot, artwork, illustration, copy, font, icon or image.

## dot.com, 10 November 2026 (superseded)

After the blended rebuild below went live, the owner narrowed the reference to one site: "pretty much just copy dot.com". dot.com redirects to x.ai/bot, the Grok Bot landing page (titled "AI teammates that finish the work"). On 10 November it refused headless browsers, so it was studied in a real browser beside the chat, section by section, with its computed styles read from the page at a 1132px viewport, in dark only. It sets its appearance from the system through a stored theme that defaults to "system"; the owner chose to follow the system too, without the switch.

Measured there, in dark: page `rgb(10, 10, 10)`, text white, secondary text `rgb(125, 129, 135)`, cards `rgb(26, 26, 26)` with a 24px radius and no border or shadow (16px for download rows), a 24px page gutter. Headline 60px/60px at weight 500 with -0.01em tracking; section titles 36px/40px at 500; leads 18px/29.25px; card titles and text 16px/26px; navigation 13px at 500; footer 13px. The header is 64px over the page at 85%. Pills: 44px with 16px text in statements, 40px with 14px text at weight 500 elsewhere, 36px in the header; the primary is white with near-black text and the quiet one a dark grey.

| dot.com section | Observed | auracode's copy |
| --- | --- | --- |
| Header | Logo, sections beside it, a quiet "Contact Sales" pill and a white "Download" pill with a menu | The same bar with Sign in and Download; no menu on the pill |
| Hero | A badge, "Meet", the mascot on a white disc and the name on one centred line, two grey lines, "Download for Windows" and a quiet pill, then the app window at full width | "Meet auracode" with the logo on a disc, two grey lines, the same two pills and the live window. The badge is not copied (no eyebrows) |
| "Message Bots like teammates" | One wide card: a title and grey paragraph at left, the mascot very large at right, cropped by the card | The intro card with the logo set large on a disc |
| "Work with many Bots at once" | A centred title and one line over a two-by-two grid of cards, each a title and text above a product visual | The four feature cards with their product views, and six smaller cards for the rest of the workspace |
| "Give each Bot a job" | A title, chips for each job, one sentence for the chosen chip and a quiet pill at left; a phone showing it at right | The agents as chips beside the New chat view |
| "The team runs on Grok Bot" | One centred line over one large rounded film frame | The real desktop screenshot in one large rounded frame |
| Pricing | Plans | Not copied: auracode is free |
| "Download Grok Bot" | A title and grey line at left; a row per platform at right | The same, for Windows, macOS and Linux, with generic glyphs |
| "Grok Bot Guides" | A centred title, one line and a quiet pill | The pointer to the changelog |
| "FAQs" | The title at left; hairline-divided questions with a plus at right | The same |
| "Meet your first Bot" | A centred line, a sentence, two pills, and the mascot drawn very large in a hairline | The closing call over the logo drawn in a hairline |
| Footer | A hairline, the logo and a copyright line at left, a dotted rule, columns of 13px links, and an appearance switch | The same without the switch; the left line is auracode's own |

Nothing of dot.com's ships: no logo, mascot, artwork, copy, font or asset. The mascot's places are taken by auracode's own logo.

## Website rebuild references, 10 November 2026 (earlier the same day, superseded)

The owner asked for the website to look like SpaceXAI, Vercel, Aside, Linear and Harvey: "pretty much just copy them, but for our thing". Each was studied again on Mobbin, and the live linear.app, vercel.com, aside.com and harvey.ai home pages were captured at 1440px with Playwright, with their computed type read from the page. x.ai refused automated browsers, so SpaceXAI rests on Mobbin alone. The rebuild takes their type scale, structure and restraint. It ships none of their logos, artwork, copy or assets, and the labels and badges several of them set above headings are still not adopted (see the no-eyebrow rule in [visual-language.md](visual-language.md)).

Measured on the live pages: Linear sets its headline at 64px, weight 510; Vercel at 64px, weight 400, with -0.06em tracking, on `#fafafa` in Geist; Aside at 48px, weight 400, in a Geist-like face; Harvey at 72px in a serif, with 14px sans text. The site before the rebuild set its headline at 104px, weight 560, beside a serif italic.

| Reference | Observed | Adaptation |
| --- | --- | --- |
| [Linear hero](https://mobbin.com/sites/sections/40a46191-a57a-4bab-9a61-94fc14cad817) and linear.app | A left-set two-line headline at a calm size, one grey line of lead under it, the app window at the column's full width | The hero: headline, lead and actions on one row, then the live window flat and full width |
| linear.app, three figures | Three line drawings in cells divided by hairlines, each over a short caption | The statement's three cells and their drawings. Its "FIG" labels are not adopted |
| [Vercel grid](https://mobbin.com/sites/sections/ea59dfe9-3cc5-4536-972a-82cb838897f3) and [feature cells](https://mobbin.com/sites/sections/01de94b2-ba4b-4b69-9a08-49a96aef9fca) | Content on a frame of vertical rails, cells of a small glyph over an ink sentence and grey text | The page's rails and the six cells that name the rest of the workspace |
| vercel.com, feature list | A title above a short list of features at left and one framed product view at right; a quiet row of customer logos under the hero | The capability rows beside their stage, and the agents' still row |
| [Vercel cards](https://mobbin.com/sites/sections/78c2180a-54ba-4e20-91af-a5f5a8d55eab) | Two cards side by side, each a small product visual over a title and text | The terminal and canvas cards |
| [Aside hero](https://mobbin.com/sites/sections/bdad14df-7d77-4d9d-8c30-dfd826d134d1) | A plain bar (logo, sections, one Download), a statement, the real app window as the stage | The header and the window as the hero's proof |
| [Aside introduction](https://mobbin.com/sites/sections/ccd08031-9f3a-43f3-96df-5ab5e053f70e) | Paragraphs that open in ink and step back to grey | The statement, and every caption that starts in ink |
| [Aside scene cards](https://mobbin.com/sites/sections/4e1b4bd7-267e-4718-a76e-fd69b2a75603) and [footer](https://mobbin.com/sites/sections/e0e0e6b0-4b96-4403-94f9-f8daa23364f1) | Product scenes on tinted stages over ink-then-grey captions; a footer of small link columns | The tinted stages and the footer |
| [SpaceXAI hero](https://mobbin.com/sites/sections/00ed6985-4338-4daa-9061-607f173448cc) | A two-tone headline (ink, then grey), small pill actions, a live chat panel as proof | Two-tone titles and the ink and outlined pills |
| [SpaceXAI media](https://mobbin.com/sites/sections/dfea7d6f-670a-4d55-8289-ff0267c89efb) | One plain statement above one large rounded media frame | The real screenshot in a plain bezel |
| [Harvey hero](https://mobbin.com/sites/sections/2e23b7a5-3438-48b3-ae2c-0b8e468e15a5) and [statement](https://mobbin.com/sites/sections/599949fd-6c3a-4f84-84cf-e74a9082141d) | A headline at left with the lead and one action at right; a statement that opens in ink and continues in grey over a product frame | The split section headers (title left, lead right) |

## Halo website references (1 November, superseded for the website)

The owner named SpaceXAI, Harvey, Vercel and Aside for cleanliness and organization, and typesafe.ai as the bar for uniqueness. Each was studied on Mobbin. Halo adapts patterns; it copies no layout, copy, artwork or asset. The eyebrow-like labels and badges several of these pages use are deliberately not adopted (see the no-eyebrow rule in [visual-language.md](visual-language.md)).

| Reference | Observed | Halo adaptation |
| --- | --- | --- |
| [SpaceXAI hero](https://mobbin.com/sites/sections/00ed6985-4338-4daa-9061-607f173448cc) | Two-tone headline, a live chat panel as the hero's proof, black and outline pill actions | The live hero window, two-tone page titles, pill actions. Its label above the title and its compliance ticks are not adopted |
| [SpaceXAI media](https://mobbin.com/sites/sections/dfea7d6f-670a-4d55-8289-ff0267c89efb) | One plain statement above one large rounded media frame | The product reveal: one real screenshot in one large glass bezel |
| [Harvey hero](https://mobbin.com/sites/sections/2e23b7a5-3438-48b3-ae2c-0b8e468e15a5) | Large serif statement, short lead, one action, product interface on a textured band | Serif accents (Fraunces Italic since 3 November 2026) and the product shown on its own stage |
| [Harvey word wheel](https://mobbin.com/sites/sections/1ea04536-af11-4dd1-8806-28b61f033a33) | A sentence completed by a scrolling wheel of phrases | The "With auracode," wheel of true local-first facts |
| [Harvey product tabs](https://mobbin.com/sites/sections/0222603b-9f41-4d7e-af9e-40ccfc3c0ca3) | Product tabs on a dark band | The capabilities night band, its six tabs and gliding indicator |
| [Harvey benefits](https://mobbin.com/sites/sections/13ddabd9-7090-4949-88af-9d1cd651ccb5) | A clear list with supporting visuals | Capability notes and the FAQ's organization |
| [Vercel hero](https://mobbin.com/sites/sections/918111cc-0e15-4a62-942b-57715e278a2a) | Restrained centered headline, registration grid, soft spectral glow | The grid in the night bands. Its glow is not adopted: the owner retired every glow on 2 November 2026 |
| [Vercel toolbar](https://mobbin.com/sites/sections/872f144e-a120-4866-9781-6a9604ac9597) | A floating dark capsule toolbar with labels around it | The floating glass header capsule and the product callouts |
| [Vercel performance](https://mobbin.com/sites/sections/bee45d3f-2707-4946-9e34-a7e88ad3be44) | Live data lines with mono annotations, three short fact columns | Restraint in the numbers row. Its icon label above the title is not adopted |
| [Aside hero](https://mobbin.com/sites/sections/bdad14df-7d77-4d9d-8c30-dfd826d134d1) | Centered statement over a soft sky, the real app window as the stage, one download action | The window as the hero stage and one primary Download. Its badge above the title is not adopted |
| [Aside capabilities](https://mobbin.com/sites/sections/4e1b4bd7-267e-4718-a76e-fd69b2a75603) | A cascade of app logos and captioned scene cards | The agent marquee drawn from one logo sprite and the captioned capability scenes |
| [Typesafe proof](https://mobbin.com/sites/sections/0c9f4512-f64c-4f8e-b8d2-ed0cff5c19de), [hero](https://mobbin.com/sites/sections/35a3304a-d6ad-4b5a-9fdc-414e7ee17428) and [footer](https://mobbin.com/sites/sections/5e931165-f2a3-407f-b336-e75e418fc052) | Distinctive staged product proof and an unusual footer composition | The bar for uniqueness: Halo's own handoff loop, halo color and giant footer wordmark, not a copy |

The Mobbin evidence supports the named sections only. No live-site audit, exact clone or full-page parity is claimed. Never ship reference logos, artwork, copy or proprietary assets.

## Website polish, 3 November 2026

The owner asked for a sexier accent face than Instrument Serif Italic, a marquee that loops fully, and a polish of the whole site. These sections were studied on Mobbin for type scale, rhythm, details and mobile. Adaptations follow the no-glow and no-eyebrow rules; the labels several references put above their headings or rows are not adopted.

| Reference | Observed | Adaptation |
| --- | --- | --- |
| [Notion logo row](https://mobbin.com/sites/sections/590675e3-3657-40b8-86ae-7898f1422af5), [Glide integrations](https://mobbin.com/sites/sections/0fe6def3-c81b-4a09-8fbe-7ba356a99b33) and [Vizcom marquee](https://mobbin.com/sites/sections/3b190d10-f512-479f-ae72-c734bb2876ce) | Even, airy marks in one row; a row of tiles fading out at both ends; logos running under a product stage | The agent marquee: equal pills, lanes that fade at the slab's edges by a fixed width, still under the hero window. Their "trusted by" labels are not adopted |
| [Linear, roadmap to release](https://mobbin.com/sites/sections/f8b93019-f62b-4e58-8f0d-e577baa204f3) | Stacked planes held by hairline rules and annotations | Hairline rails above and below the marquee's two lanes; depth from tone and scale, a quieter lane behind. Its mono annotations are not adopted |
| [Harvey stats](https://mobbin.com/sites/sections/9b0bc6de-e1c7-4a3d-b51f-4663ff9be33e), [Legora stats](https://mobbin.com/sites/sections/2cf16054-0dd8-4dcc-8358-a8fbb6dad125) and [Cursor stats](https://mobbin.com/sites/sections/49475026-9a62-4dc4-b12d-98a280bee047) | Hairline-topped figures with short captions; serif figures; figures on tonal tiles | The numbers keep their hairline tops; each caption leads with its unit in ink, and phones set figure and line side by side. Figures stay in Geist (the accent italic is for phrases); tiles are not adopted. Harvey's label above its title is not adopted |
| [Retool FAQ](https://mobbin.com/sites/sections/525668ea-697f-4e30-84dd-04cacfc29545), [Speakeasy FAQ](https://mobbin.com/sites/sections/c6393d9b-962e-4fed-90fd-f494bdc57983) and [Ditto FAQ](https://mobbin.com/sites/sections/a751a0e3-4cb2-497f-b8b9-b8e83d6fecb4) | Questions separated by hairlines beside a heading; a light serif heading; large question type | The FAQ's questions on one white sheet with inset hairlines, the open row a step darker, at 18px |
| [Harvey hero](https://mobbin.com/sites/sections/599949fd-6c3a-4f84-84cf-e74a9082141d) and [Tana hero](https://mobbin.com/screens/829cb8c9-f0b0-45e6-8184-77897339cffa) | A two-tone statement; a serif headline with an italic accent and one download action | Every two-tone title sets its grey clause on its own line; the accent stays an italic |
| [Dropbox Dash desktop download](https://mobbin.com/screens/b59243e8-9c21-4120-9e4f-eee85c834d5c) | The app icon on a raised tile as the page's focal point | The download page's raised logo tile beside its title. Its centred single-column layout is not adopted |
| [v0 sign-up](https://mobbin.com/screens/3344e4be-edc0-472f-a6de-e546605ce8a2) and [Copilot sign-in](https://mobbin.com/screens/02dfc86d-2c9e-4e7a-95b8-238df49e024b) | One action per step on a hairline card | Sign-in keeps one action per step; a verification that cannot start reads as the site's other alerts |
| [Structured footer](https://mobbin.com/sites/sections/cf954a98-4f2e-41d3-b2a1-168e4ba0108d) and [Harvey footer](https://mobbin.com/sites/sections/3479c6e6-96b2-428f-9142-f47d07c94134) | An edge-to-edge wordmark; a statement over link columns | The footer keeps its giant Geist wordmark and stacks its line over the links below 1024px |
| [Typesafe proof](https://mobbin.com/sites/sections/0c9f4512-f64c-4f8e-b8d2-ed0cff5c19de) | Crop marks and a dotted field framing a giant statement | Still the bar for uniqueness, and not copied: no crop marks; the pixel texture stays the hero floor's own |

The accent face was chosen from real renders of the site's hero, titles, wheel and finale in each candidate, all SIL OFL on Fontsource: Fraunces, Playfair 2, Bodoni Moda, Newsreader, Source Serif 4, Noto Serif Display, Redaction (and its pixel-degraded cut), DM Serif Display, Spectral, Piazzolla, Petrona, Cormorant Garamond, Literata, Arapey, and the grotesk italics of Mona Sans, Instrument Sans, Schibsted Grotesk, Familjen Grotesk and Geist. Fraunces Italic at its largest optical size read as the most striking and the least templated against Geist's tight headlines, at 82 KB for its one italic file.

## Home page revamp, 9 November 2026

The owner asked for the landing page to look like linear.app, SpaceXAI and aside.com. Their sections were studied on Mobbin, and the live linear.app and aside.com home pages were captured with Playwright at 1440 by 900. The page stays light (the standing rule); Linear's night is kept for the two night bands. No layout, copy, artwork or asset is copied, and the labels and badges these pages put above their headings are not adopted.

| Reference | Observed | Adaptation |
| --- | --- | --- |
| [Linear hero](https://mobbin.com/sites/sections/f1148b2c-af7e-451c-a2a4-3f8740f58596), live linear.app | A large left-aligned headline over the product window; a statement paragraph in white and grey; line-art figures between hairlines; split headers (title left, text and link right) over large product panels | The left-aligned hero with its column beside it; the statement; three line-art drawings between hairlines; split headers for the product and the features |
| [Linear, roadmap to release](https://mobbin.com/sites/sections/40a46191-a57a-4bab-9a92-94fc14cad817) | Isometric planes drawn in hairlines | The drawings: slabs, agents wired to a plate, columns stepping up. Its mono annotations are not adopted |
| [SpaceXAI for Business](https://mobbin.com/sites/sections/00ed6985-4338-4daa-9061-607f173448cc) | A two-tone headline in one face, ink then grey; black and outline pills | Two-tone titles in Geist on the home page; the existing pills |
| [Aside hero](https://mobbin.com/sites/sections/bdad14df-7d77-4d9d-8c30-dfd826d134d1), live aside.com | A framed page with hairline rails; the app window as the stage; feature cards on soft tinted grounds with the product inside; captions that start in ink | The rails and hairlines across the page; the hero window on a grey stage; the feature cards and their captions. Its sky photograph is not adopted |

## Desktop references

### The Grok Bot app itself, 17 November 2026 (current)

The owner asked for the real app, not its page: "you need to open up Grokbot on my computer ... Go through everything ... and essentially just copy it and adapt it to each of the styles that we offer". Grok Bot 1.2.0 is installed on the owner's machine as an Electron AppImage. Its values were read from its own bundled stylesheet and scripts, read-only, without signing in; nothing of it ships (no code, font, icon or image). The monitor was asleep that night, so the app was not compared on screen; that comparison waits for the owner's screen.

- **Layers.** Flat and solid. Light: raised `#ffffff`, pane `#ffffff`, window `#fcfcfc`, sidebar `#f5f5f5`, ink `#1c1c1c`. Dark: raised `#1f1f1f`, pane `#161616`, window `#121212`, sidebar `#0e0e0e`, ink `#f5f5f5`. Accent and focus blue (`#1680f3`, `#1e86fc` light; `#116dd2`, `#1884fc` dark).
- **One ink at set strengths** (light / dark). Text 100, 68, 56, 40 / 100, 66, 54, 38; icons 92, 66, 52, 32 / 90, 64, 50, 30; borders 24, 12, 8, 6 / 22, 10, 6, 4; hover 6, active 8; secondary fills 6 to 18 (dark 4 to 20); a disabled primary 15 / 13; the agent's bubble 6 / 4, the person's solid ink in light and ink at 18 in dark; status tints 9 / 7; scrim 50 / 70.
- **Depth.** Light: a control `0 1px 3px` at 12%, an inline surface `0 4px 12px -1px` at 8% with `0 2px 4px -2px` at 6%, a popover `0 10px 20px -3px` and `0 4px 6px -4px` at 10%, a modal `0 20px 32px -5px` and `0 8px 12px -6px` at 10%. Dark: no shadows; a lit top edge and the border ladder.
- **Type.** X VF by Basement.studio, Geist's makers, with Geist's proportions (x-height 0.526em, cap 0.72em; weights 200 to 800). Roles: caption 11/16, label 12/16, body 13/18 and 14/20, chat 15/22, headings 17/24, 22/28 and 26/32 (tracking -0.008em to -0.015em); body tracking 0. Weights 400, 500 and 600, graded at runtime per platform (regular 420 to 425 on macOS). Features "case" and "calt".
- **Shape and motion.** Radii 6, 8, 10, 14, 16, 18 and pill. Menus 16 with 32px rows at 10, 6px apart; bubbles 18; tooltips 6 with a 0.5px edge. Durations 50 to 300ms, drawers 500; easing `cubic-bezier(.2, 0, 0, 1)`, arrivals `cubic-bezier(.22, 1, .36, 1)`, drawers `cubic-bezier(.32, .72, 0, 1)`.
- **Composer.** Send is solid ink with the page's colour on it; attach is the secondary fill with a 0.5px inset edge; the toolbar is the window's colour fading out over content. New chat is a full stage around the message box.

Taken, in every theme and both appearances (each theme keeps its own colours and cuts its layers from these seeds): the four solid layers and the one-ink ladders; no shadows in dark and the reference's soft ones in light; the type roles and graded weights (Geist stays, being the same studio's face with the same proportions; Linux takes heavier grades, since it draws text at twice its size and lets the compositor scale it); the radii; the flat open row; the message box at 18 with the faint disabled Send; the header with no line over a fade; the sent message as a bubble at 18. Not taken: the accent blue (each theme keeps its own mark), its typeface (not licensed to us), bubbles around agents' replies (answers run the column), avatars in the chat list.

### The Grok Bot app's page, 10 November 2026 (superseded by the app itself)

The owner asked for the Grok Bot app's styling across the desktop. Its page (x.ai/bot, which dot.com redirects to) draws the app live in HTML, so these are its computed styles, read in Aura's browser under a dark system, at the page's scale:

- **Window.** `rgba(28, 28, 26, 0.62)` with a 24px radius; the sidebar a darker `rgba(20, 20, 20, 0.28)` over it and 280px wide; the chat `rgba(38, 38, 38, 0.84)`. Text `rgb(242, 242, 242)`, then white at 0.55 to 0.58 and at 0.32 to 0.36.
- **Search.** 32px tall, 10px radius, white at 0.07 with a hairline of white at 0.12, placeholder at 0.32.
- **Rows.** 10px radius, 8px inside, the open one white at 0.075 with no rim. Name 14/20 at -0.15px tracking, time 12/16 at 0.32, preview 12/16 at 0.55.
- **Messages.** The person's is `rgb(236, 236, 232)` with `rgb(20, 20, 20)` on it; the agent's is white at 0.06. Both 16px radius, 8px by 12px inside, 14/20 at -0.15px. Notes between them are centred, 12 to 13px at 0.58.
- **Cards.** White at 0.06, 16px radius, 10px by 12px inside, no rim; the title 14px at weight 600. A badge is a 19px pill in a 0.14 tint of its colour with a 5px dot and 12px words.
- **Composer.** A full pill 45px tall with a hairline of white at 0.12; a 28px disc at white 0.06 for attaching and a 28px `rgb(240, 240, 240)` disc with dark ink for the microphone or Send.
- **Buttons.** Pills: the first in paper with ink, the second dark with a hairline.
- **Header and footer.** Title 13/18 at -0.08px; the account a 28px disc at white 0.075 with 10px initials.

Taken: the paper message, both trackings, the search field, the attach disc, the badge, flat cards, and its dark colours as the Grok Build theme's dark palette (adjusted where the desktop's contrast rules asked: raised surfaces a step darker, secondary text a step lighter). Already the desktop's own before this pass: the darker sidebar beside a lighter chat, 10px rows, the capsule composer with its white Send disc, pill buttons and solid menus. Not taken: bubbles around agents' replies (answers run the column), avatars in the chat list, and its palette as the default (each theme keeps its own, Claude Code's stays the default, and the paper derives from the theme). Only the dark appearance was seen, so the Grok Build theme's light palette is unchanged.

The desktop keeps the audited Devin organization: compact work surfaces, navigation and settings structure in light and dark. Since 17 November 2026 its layers are the Grok Bot app's flat ones; the native material (Acrylic or Mica on Windows, vibrancy or Liquid Glass on macOS) shows only behind the sidebar. Accent, focus and selection stay neutral. No fresh live Devin inspection is claimed for Halo.

The earlier inspection covered Devin Preferences in light and dark, a short greeting conversation with its side panel in light mode, and all six Customize sections in light mode: Plugins, Skills, MCPs, Hooks, Rules and Memory. The prior inspection covered 18 organization settings sections plus overview, five personal sections, six Customize sections, three Security sections, the main interface and nested screens in both themes. A terminal and a tool-heavy task were not inspected. These observations do not prove how those missing states look or behave.

## Earlier website references

The 30 October proposal used a [Vercel hero](https://mobbin.com/sites/sections/42b13454-50c5-40ec-bd46-c7c1ea521873), a [Vercel agent section](https://mobbin.com/sites/sections/c7c01a46-9a30-4842-8098-affe8ead6b9f), Typesafe/Jev's [automation collage](https://mobbin.com/sites/sections/970c5aa2-ff4c-4b87-8631-6a61e019e20f) and [news/Jev](https://mobbin.com/sites/sections/075e6b4c-364e-4707-8c2f-5fc0956bd527), and Harvey's [paired panels](https://mobbin.com/sites/sections/889f579b-4c96-4cf5-b8f5-411f4e7ac478) and [footer](https://mobbin.com/sites/sections/3479c6e6-96b2-428f-9142-f47d07c94134). Its coral dither field and offset handoff planes are retired. The canonical product decisions and verification gates are in [visual-language.md](visual-language.md).
