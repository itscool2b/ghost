# The name

The product is **o7**, at [o7.codes](https://o7.codes), built from the
[itscool2b/o7](https://github.com/itscool2b/o7) repository. Until 19 November 2026 it was
called auracode, "Aura" for short, at auracode.si. The owner asked for the old name to go
everywhere. This page says what changed, how a copy from before finds its way, and the few old
names that stay on purpose.

Write "o7", lowercase, wherever a person reads the name.

## What is o7 now

- **What people read**: the website, the app's text, its window title and tray, the installer and
  uninstaller, what agents and bots are told about the app, the docs.
- **The program**: `o7.exe` (`o7` on macOS and Linux), installed in `%LOCALAPPDATA%\o7` on
  Windows, under the bundle identifier `codes.o7.desktop` and the publisher `o7`. Release files
  are `o7_<version>_...`.
- **The code**: crates `o7-*` (`o7_core` in Rust), packages `@o7/*`, the desktop crate `o7-desktop`,
  settings `O7_*` (`O7_DATA_DIR` and the rest), the helper modes `--o7-memory-helper`,
  `--o7-browser-helper`, `--o7-mobile-helper` and `--o7-project-helper`, the tool servers agents
  see (`o7_browser`, `o7_mobile`, `o7_project_memory`, `o7_project`) and their tools
  (`o7_memory_read` and the rest), the protocol's `O7Event`.
- **The services**: the Workers `o7-web`, `o7-web-preview`, `o7-relay` and `o7-relay-preview`.

## A copy from before the rename

An update from auracode moves to o7 without losing anything:

- **The data stays where it is.** A copy updated from auracode keeps using
  `si.auracode.desktop`: o7 uses `codes.o7.desktop` when that holds data, else the old folder when
  that does, else the new one (`o7-core`'s `paths.rs`). The folder is not moved, because every Git
  worktree inside it records its full path, and so does its repository, and agents keep their own
  history of those folders by path. On Windows and Linux the app's page settings and its browser's
  sign-ins come along (the WebView is given the old folder, as it was then), and the window opens
  where it was. On macOS the WebView's store is kept per bundle identifier, so those start over.
- **The program is replaced.** The update installs o7 beside auracode (a product's name is its
  install folder), and the installer then retires auracode: o7's shortcut takes the place of
  auracode's in the Start menu and on the desktop, a taskbar pin is pointed at o7, and auracode's
  entry in the list of apps and its installer records go. Its program is deleted only from its
  default folder, `%LOCALAPPDATA%\auracode`, and never when o7 was installed in or under it; an
  auracode installed elsewhere keeps its files but is no longer listed, so its uninstaller, which
  would delete the data o7 now uses, is not offered. The data is never touched
  (`windows/hooks.nsh`, `O7_RETIRE_AURACODE`). On Linux the `o7` package replaces and conflicts
  with `auracode`.
- **Updates still reach it.** A copy from before accepts update packages only under
  `itscool2b/auracode`'s release path. So the release feed keeps naming that path, which GitHub
  redirects to `itscool2b/o7`, and copies from after accept both (`o7-release`'s `RELEASES`, the
  updater's `RELEASES`). Switch the feed to `itscool2b/o7` once no copy from before is in use, and
  never let anyone else take the name `itscool2b/auracode` meanwhile (only the owner can).
- **Old settings still work.** `AURACODE_DATA_DIR` is still read when `O7_DATA_DIR` is not set,
  worktree commands still get `AURACODE_PORT` beside `O7_PORT` (saved Setup and Run commands name
  it), the app still answers the old `--aura-*-helper` modes, and both variable families are kept
  from agents. An agent's own allow list that named the old tools (`mcp__aura_browser__...`) asks
  once more for the new ones. The upgrade scripts find the old install, data and kept upgrades the same way.
- **Remote access**: pairings and bot links made at auracode.si were already retired by the move
  to o7.codes (docs/remote-sessions.md).

## Old names that stay, on purpose

These are written by one version and read back by a later one, or are someone else's to keep.
Renaming one would make an updated app stop recognising what it wrote before.

| Name | Where | What breaks if it changes |
|---|---|---|
| `si.auracode.desktop` | the data folder of a copy updated from auracode | its chats, worktrees and agents' history of them (above) |
| `aura.db` | the profile database's file name | every profile opens empty |
| `auracode <id>`, `auracode retired <id>` | a Git worktree's lock reason | the app no longer knows its own worktrees |
| `auracode checkpoint`, `auracode before-turn checkpoint` | a checkpoint commit's author and message | earlier checkpoints are not found |
| `[auracode project event]` | the start of a message a coordinator is sent | events already in a chat read as plain messages |
| `Project memory (Aura reference context, revision ` | the start of the memory attached to a message | old messages show the whole memory instead of folding it |
| `aura-image:` | a stored image's reference in a message | images in old chats stop showing |
| `auracode remote v1 ...` | the labels the remote channel's keys are made from | a paired phone and a computer on different versions cannot talk |
| `auracode-remote` | the phone browser's key store | every phone is unpaired |
| `auracode.agents.v1` | the secure-store entry agent sign-ins are kept under | every agent sign-in added in the app is lost |
| `auracode.*` page settings | keys the app's page keeps in its own storage | the font, appearance and similar choices reset |
| `auracode://` | a link scheme earlier versions registered | the uninstaller would not find and remove it |
| auracode.si | the address before o7.codes | copies from before the move lose remote access (infra/README.md) |

Also kept: dated history (older changelog entries, `docs/progress.md`, design notes, spikes and
benchmarks), recorded agent sessions in test fixtures, and the Design Lab's studies, which keep
the names of their day. The default theme is still called Aura, and so are the legacy design
system's glow and its `--aura-*` tokens: those name a look, not the product.
