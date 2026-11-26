# The name

The product is **Ghost ADE**, "Ghost" for short, at [onghost.dev](https://onghost.dev), built
from the [itscool2b/ghost](https://github.com/itscool2b/ghost) repository. It has had two names
before:

| When | Name | Address | Repository |
|---|---|---|---|
| until 19 November 2026 | auracode ("Aura" for short) | auracode.si | itscool2b/auracode |
| 19 to 26 November 2026 | o7 | o7.codes | itscool2b/o7 |
| since 26 November 2026 | Ghost ADE | onghost.dev | itscool2b/ghost |

Both times the owner asked for the old name to go everywhere. This page says what the name is
now, how a copy from before finds its way, and the old names that stay on purpose.

## How to write it

Write **Ghost ADE** where the product is introduced or named formally: a window's or a page's
title, the first heading of a page, the installer, About, the legal pages. Write **Ghost** after
that and in running text: "Ghost keeps your chats on your computer." Write `ghost`, lowercase,
for anything a program reads.

## What is Ghost now

- **What people read**: the website, the app's text, its window title and tray, the installer and
  uninstaller, what agents and bots are told about the app, the docs.
- **The program**: `ghost.exe` (`ghost` on macOS and Linux), installed in `%LOCALAPPDATA%\Ghost`
  on Windows and as `Ghost.app` on macOS, under the bundle identifier `dev.onghost.desktop` and
  the publisher `Ghost`. Release files are `Ghost_<version>_...`.
- **The code**: crates `ghost-*` (`ghost_core` in Rust), packages `@ghost/*`, the desktop crate
  `ghost-desktop`, settings `GHOST_*` (`GHOST_DATA_DIR` and the rest), the helper modes
  `--ghost-memory-helper`, `--ghost-browser-helper`, `--ghost-mobile-helper`,
  `--ghost-bot-helper` and `--ghost-project-helper`, the tool servers agents see
  (`ghost_browser`, `ghost_mobile`, `ghost_project_memory`, `ghost_project`) and their tools
  (`ghost_memory_read` and the rest), the protocol's `GhostEvent`.
- **What other programs are told**: an agent the app starts, and a tool server a person tests from
  Settings, hear the client's name as `ghost` ("Ghost ADE"); a terminal inside the app says
  `TERM_PROGRAM=ghost`; a debug bundle starts "Ghost debug bundle v1". These still said
  `auracode` until this rename.
- **The phone app**: Ghost for iPhone, `dev.onghost.mobile`, with the link scheme `ghost://`.
- **The site**: onghost.dev, served by the same four Workers as before. They keep their o7
  names for now (below).

## A copy from before a rename

An update from o7, or straight from auracode, moves to Ghost without losing anything:

- **The data stays where it is.** Ghost uses `dev.onghost.desktop` when that holds data, else
  `codes.o7.desktop` when that does, else `si.auracode.desktop` when that does, else the new
  folder (`ghost-core`'s `paths.rs`). The folder is never moved, because every Git worktree
  inside it records its full path, and so does its repository, and agents keep their own history
  of those folders by path. On Windows and Linux the app's page settings and its browser's
  sign-ins come along (the WebView is given the old folder, as it was then), and the window opens
  where it was. On macOS the WebView's store is kept per bundle identifier, so those start over.
- **The program is replaced.** The update installs Ghost beside the earlier program (a product's
  name is its install folder), and the installer then retires the earlier one: Ghost's shortcut
  takes its place in the Start menu and on the desktop, a taskbar pin is pointed at Ghost, and
  the earlier entry in the list of apps and its installer records go. Its program is deleted
  only from its default folder (`%LOCALAPPDATA%\o7` or `%LOCALAPPDATA%\auracode`), and never
  when Ghost was installed in or under it; an earlier copy installed elsewhere keeps its files
  but is no longer listed, so its uninstaller, which would delete the data Ghost now uses, is not
  offered. The data is never touched (`windows/hooks.nsh`, `GHOST_RETIRE`, once for each earlier
  name). On Linux the `ghost` package replaces and conflicts with `o7` and `auracode`.
- **On macOS and Linux the file keeps its name.** An update there replaces the app where it is,
  so an `o7.app` in Applications is Ghost inside (its program, its identifier, its window) and is
  still a file called `o7.app`, which is the name Finder shows, and an AppImage keeps its file
  name too. It runs the same. Dragging `Ghost.app` from a fresh download into Applications gives
  the file the name as well. This path was read in the updater's code and not run on a Mac or a
  Linux machine.
- **A start at sign-in carries over.** It was added while the app was o7, so the system's record
  of it names o7 and starts o7's program. The first time Ghost starts it takes that record over
  under its own name, unless the person had switched it off in the system's own list
  (`login_start.rs`, `carry_earlier`).
- **Updates still reach it.** A copy from the auracode days accepts update packages only under
  `itscool2b/auracode`'s release path, and a copy from the o7 days under that path or
  `itscool2b/o7`'s. So the release feed keeps naming `itscool2b/auracode`, which GitHub
  redirects to `itscool2b/ghost`, and Ghost accepts all three (`ghost-release`'s `RELEASES`, the
  updater's `RELEASES`). Switch the feed to `itscool2b/ghost` once no copy from before is in
  use, and never let anyone else take the names `itscool2b/auracode` or `itscool2b/o7`
  meanwhile (only the owner can).
- **Old settings still work.** `O7_DATA_DIR` and `AURACODE_DATA_DIR` are still read when
  `GHOST_DATA_DIR` is not set, worktree commands still get `O7_PORT` and `AURACODE_PORT` beside
  `GHOST_PORT` (saved Setup and Run commands name them), the app still answers the old
  `--o7-*-helper` and `--aura-*-helper` modes, and every variable family is kept from agents. An
  agent's own allow list that named the old tools (`mcp__o7_browser__...`) asks once more for
  the new ones. The upgrade scripts find the old install, data and kept upgrades the same way.
- **Remote access starts over once.** Credentials belong to the site they were issued under, so
  the first time Ghost starts it removes every phone pairing and bot link made at o7.codes
  (docs/remote-sessions.md). Pair the phone and link the bot once more, at onghost.dev. Until a
  copy has updated, o7.codes still serves its remote page and its relay (infra/README.md).

## Old names that stay, on purpose

These are written by one version and read back by a later one, or are someone else's to keep.
Renaming one would make an updated app stop recognising what it wrote before.

| Name | Where | What breaks if it changes |
|---|---|---|
| `codes.o7.desktop`, `si.auracode.desktop` | the data folder of a copy updated from o7 or from auracode | its chats, worktrees and agents' history of them (above) |
| `aura.db` | the profile database's file name | every profile opens empty |
| `auracode <id>`, `auracode retired <id>` | a Git worktree's lock reason | the app no longer knows its own worktrees |
| `auracode checkpoint`, `auracode before-turn checkpoint` | a checkpoint commit's author and message | earlier checkpoints are not found |
| `[auracode project event]` | the start of a message a coordinator is sent | events already in a chat read as plain messages |
| `Project memory (Aura reference context, revision ` | the start of the memory attached to a message | old messages show the whole memory instead of folding it |
| `aura-image:` | a stored image's reference in a message | images in old chats stop showing |
| `auracode remote v1 ...` | the labels the remote channel's keys are made from | a paired phone and a computer on different versions cannot talk |
| `o7 push v1`, `o7 push collapse v1` | the labels a notification's sealing key and a chat's collapse id are made from | a phone and a computer on different versions cannot read each other's notifications, and the vectors both are tested against (`push-vectors.json`) no longer hold |
| `auracode-remote` | the phone browser's key store | every phone is unpaired |
| `auracode.agents.v1` | the secure-store entry agent sign-ins are kept under | every agent sign-in added in the app is lost |
| `auracode.*` and `o7.*` page settings | keys the app's page keeps in its own storage (`o7.legal.accepted`, `o7.device-panel.*`) | the font, appearance and similar choices reset, and the terms are asked again |
| `o7_phone_<n>`, `o7_iphone_<n>` | the ids of the emulators and simulators the app made | a new emulator is made and the old one stays on disk, unused |
| `o7-counts`, `o7-counts-preview` | the site's counter databases | a D1 database cannot be renamed; the counts would start from zero in a new one |
| `auracode://` | a link scheme earlier versions registered | the uninstaller would not find and remove it |
| auracode.si, o7.codes | the addresses before onghost.dev | copies from before a move lose remote access (infra/README.md) |

Three more stay only until Cloudflare is changed with them, which takes access the rename did not
have (infra/README.md, "Everything is Ghost"):

| Name | Where | Until |
|---|---|---|
| `o7-web`, `o7-web-preview`, `o7-relay`, `o7-relay-preview` | the Workers' names, in the `wrangler.toml` files | the site's build triggers, which belong to a Worker by its name, are moved with them |
| `@o7/web` | `apps/web-builds-alias`, a package whose only script builds `@ghost/web` | those triggers' build command, which is kept in Cloudflare and names the site's package, is changed |
| "page analytics are turned off for o7.codes" | one sentence of the privacy page | the new zone has the rule that turns them off there too; until then the page's own security policy keeps the script from running |

Also kept: dated history (older changelog entries, `docs/progress.md`, design notes, spikes and
benchmarks), recorded agent sessions in test fixtures (all but the client's name in the line the
app itself sends first, which the tests hold it to), and the sample project the previews and the
Design Lab show, which is still called "auracode" (the README's pictures show it as "ghost"). Names the app uses only inside itself and that nobody reads, such as the
window events `auracode-*`, were left as they were the first time and are left again. The legacy
design system's glow and its `--aura-*` tokens name a look, not the product.
