# Uninstalling Ghost and deleting its data

Uninstalling Ghost deletes its local files and the sign-in tokens of the agent accounts you
added in it. Your coding agents, their settings and your repositories are not touched. Settings
has the same deletion without uninstalling, on Windows, macOS and Linux (macOS and Linux have no
uninstaller of Ghost's own).

One piece of Rust decides what goes everywhere: ghost-core's `uninstall` module, run by the app
binary's `--uninstall-cleanup` mode (`apps/desktop/src-tauri/src/uninstall.rs`). The Windows
uninstaller, Settings and a person in a terminal all run that mode.

## What is removed

On a computer updated from auracode (the app's name until 19 November 2026), every folder below
is also removed under the earlier identifier, `si.auracode.desktop`, which is where such a copy
keeps its data ([docs/name.md](name.md)).

### Windows

- `%LOCALAPPDATA%\dev.onghost.desktop`, the data folder. It holds:
  - the device identity;
  - every profile, with its chats, attachments, checkpoints, logs and backups, and with them
    anything an earlier version kept there for a Ghost account (see
    [Secure storage](#secure-storage));
  - Ghost's worktrees (see [Worktrees](#worktrees));
  - the ACP adapters Ghost downloaded for itself;
  - Instructions backups;
  - clone recovery journals, including those in a custom data root;
  - the WebView2 profile (`EBWebView`).
- `%APPDATA%\dev.onghost.desktop`: the window position.
- In `%TEMP%`: the updater's downloads (`auracode-<version>-updater-*`) and the CLI's probe
  folders (`ghost-cli-*`).
- In Windows Credential Manager, the entries named `auracode.agents.v1:` that hold the tokens of
  agent accounts added in the deleted profiles. Other apps' credentials are never read. See
  [Secure storage](#secure-storage).
- The uninstaller's own records:
  - the shortcuts, the jump list and the uninstall entry;
  - an `auracode://` link registration an earlier version made, when it points at this install;
  - `HKCU\Software\auracode\auracode`;
  - Windows' notification settings for Ghost
    (`HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\dev.onghost.desktop`).
- A data folder chosen with `GHOST_DATA_DIR`, when the variable is set for the uninstaller.
  Only what Ghost made in it goes (see [Edge cases](#edge-cases)).

### macOS

- `~/Library/Application Support/dev.onghost.desktop`: the data folder and the window position.
- These folders and files:
  - `~/Library/Caches/dev.onghost.desktop`;
  - `~/Library/WebKit/dev.onghost.desktop`;
  - `~/Library/HTTPStorages/dev.onghost.desktop` and its `.binarycookies` file;
  - `~/Library/Saved Application State/dev.onghost.desktop.savedState`;
  - `~/Library/Logs/dev.onghost.desktop`;
  - `~/Library/Preferences/dev.onghost.desktop.plist`;
  - a leftover single-instance socket, `/tmp/codes_ghost_desktop_si.sock`.
- In `$TMPDIR`: the updater's downloads and the CLI's probe folders.
- Keychain items with the service `auracode.agents.v1`, for the agent accounts added in the
  deleted profiles.
- With "Also move Ghost to the Trash": the app bundle. Launch Services then forgets an
  `auracode://` link an earlier version registered.

### Linux

- `~/.local/share/dev.onghost.desktop`: the data folder, which also holds WebKitGTK's storage and
  cache.
- `~/.config/dev.onghost.desktop`: the window position.
- `~/.cache/dev.onghost.desktop`.
- In `/tmp` (or `$TMPDIR`): the CLI's probe folders.
- Secret Service items with the service `auracode.agents.v1`, for the agent accounts added in the
  deleted profiles.

Removing the package (`sudo apt remove Ghost`, `sudo dnf remove Ghost`, or deleting the
AppImage) removes the app and leaves all of this, so delete the data from Settings first.

Ghost adds no login item, Run key, launch agent or autostart entry, so there is none to remove.

## What stays, and why

- **Your agents.** Claude Code, Codex, Pi, OpenCode and the others stay installed, including any
  installed through Ghost's one-click install. Those are global npm installs, such as
  `@anthropic-ai/claude-code`, `@openai/codex`, `@earendil-works/pi-coding-agent` and
  `opencode-ai`, and they work without Ghost. Remove one with `npm uninstall -g <package>`.
- **Your agents' homes.** The cleanup never reads or changes `~/.claude`, `~/.claude.json`,
  `~/.codex`, `~/.pi`, `~/.agents` or OpenCode's folders.
  - Their logins stay, since Ghost never stored them.
  - Files Ghost wrote there at your request stay, because they are valid files of yours:
    instruction files, Plugin library items and MCP servers. The agents keep using them.
  - Sessions you ran through Ghost stay in each agent's own history, so `claude --resume` and
    `codex resume` still find them.
- **Plugin library undo folders.** `.auracode-library` sits inside agent folders, in `~` (for
  Claude MCP copies) and in projects. It keeps the files a Library copy replaced, so it may be the
  only copy of your earlier version. It is in your folders, so it stays. Delete it by hand once you
  no longer need to undo a copy.
- **Your repositories.** Their files, index, commits and remotes are never touched, and that
  includes a remote added by Publish to GitHub. Ghost's `ghost/...` branches stay with the
  commits on them; delete one with `git branch -d ghost/<name>`.
- **Worktrees with unsaved work.** They stay unless you choose otherwise (see
  [Worktrees](#worktrees)).
- **Keys of a removed Ghost account.** Ghost has no accounts any more. If you signed in
  with an earlier version, its keys are still in secure storage under `auracode.sync.v1`; this
  version neither reads nor removes them (see [Secure storage](#secure-storage)).
- **Shared system components.** The WebView2 runtime, Git, Node.js and npm stay, because other
  apps use them.
- **Developer files.** A development build's data folder (`dev.onghost.desktop.dev` next to the
  data folder) and `host-upgrade`'s snapshots and log (`%LOCALAPPDATA%\auracode-upgrades`) were
  made by tools for working on Ghost, not by the installed app. Delete them by hand; see
  [developing-in-ghost.md](developing-in-ghost.md).

## Secure storage

Ghost keeps one kind of secret: the sign-in token of each agent account added in it
([agent accounts](agent-accounts.md)), one entry per account in Windows Credential Manager, the
macOS login keychain or the Linux Secret Service, under the service `auracode.agents.v1`. An
entry's name is a digest of the account's id and names no account, email or folder. Before the
profiles go, the cleanup reads the ids they list and removes those entries; it never lists or
reads another entry. When the store is locked or missing, the files still go and the cleanup says
how many tokens stayed.

Earlier versions had Ghost accounts, removed on 16 November 2026. What one left on this
computer is treated like this:

- Its profile folder (`profiles/<id>`, with an encrypted database) is never opened by this
  version. It goes with the data folder, like every profile.
- Its keys in secure storage (`auracode.sync.v1:` in Credential Manager, the service
  `auracode.sync.v1` in the keychain or the Secret Service) are not read and not removed. Delete
  them by hand once you no longer want that data: without them, a copy of the account's profile
  folder cannot be decrypted.
- The empty file `si.auracode.sync.v1.lock` in the platform's local application data folder stays.
  It holds no credentials or user data, and you can delete it.

## Worktrees

Ghost creates its worktrees inside its data folder and registers them with your repository,
locked by Ghost. Before deleting anything, the cleanup asks Git about each one. A worktree has
unsaved work when:

- it has uncommitted changes, untracked files included (ignored files do not count); or
- its HEAD has commits that no other branch, tag or remote has.

A worktree with nothing unsaved is deleted, and Git drops its registration (`git worktree prune`).
Its branch stays.

A worktree with unsaved work is listed by branch, repository and what deleting it would lose, for
example "3 uncommitted files, 1 commit only on ghost/fix-login". It is kept unless you choose to
delete it:

- Kept, its folder stays where it is, still registered with its repository, and Ghost's lock is
  lifted. `git worktree list` in the repository shows it, and `git worktree remove <path>` deletes
  it later.
- Deleted, its uncommitted files are lost. Commits on its branch stay with the branch; commits on a
  detached HEAD are lost.

A worktree Git cannot check (Git is missing, or the repository is gone) counts as unsaved. A quiet
uninstall never deletes unsaved work.

## How it runs

### The Windows uninstaller

Open Settings, then Apps, find Ghost and choose Uninstall. The confirm page has one checkbox,
"Keep my Ghost data", which is unticked by default, so uninstalling deletes the data.

Before any file is removed, the uninstaller's hook (`apps/desktop/src-tauri/windows/hooks.nsh`)
runs `ghost.exe --uninstall-cleanup --interactive`:

1. If Ghost is running, it is asked to quit. It stops only its own agents and terminals and
   asks no further question.
2. If it has not quit after 45 seconds, Tauri's own check offers to close it. The cleanup then
   runs once the app's files are removed, from a copy of the program.
3. A dialog lists the worktrees with unsaved work. Yes keeps them; No deletes them.
4. If something could not be deleted, a second dialog lists it.

Quiet uninstalls show no dialogs:

- `uninstall.exe /S` and `/P` delete the data and keep worktrees with unsaved work.
- Adding `/KEEPDATA` keeps everything.

Updates (`/UPDATE`) keep everything. So does the uninstall a newer installer runs before installing
("Uninstall before installing"): that installer runs the uninstaller in place, and the hook treats
it as an upgrade.

The bundle builds the NSIS installer only. An MSI would uninstall without the cleanup.

### Settings, on Windows, macOS and Linux

In Settings, open About, then Delete all Ghost data under This device. The review lists:

- every folder and file that would go, with its size;
- the secure store whose agent account tokens go too;
- the worktrees with unsaved work, which are kept unless you tick to delete them too;
- what stays.

On macOS it also offers "Also move Ghost to the Trash".

After you type DELETE, Ghost starts the cleanup and quits, without a second question about
running agents. The cleanup waits up to two minutes for Ghost to exit, then deletes. Its only
dialog says what could not be deleted, if anything. If Ghost does not exit, nothing is deleted
and the cleanup says so. The next launch is a fresh install.

A copy started with its own `GHOST_DATA_DIR` reviews and deletes only that folder, and leaves
the installed app's data alone.

### A terminal

The same cleanup runs from a terminal:

```sh
# macOS
/Applications/Ghost.app/Contents/MacOS/ghost --uninstall-cleanup --dry-run
# Windows (the default per-user install folder)
"%LOCALAPPDATA%\Ghost\ghost.exe" --uninstall-cleanup --dry-run
# Linux (the deb or rpm package; an AppImage takes the same options)
ghost --uninstall-cleanup --dry-run
```

| Option | Effect |
|---|---|
| `--dry-run` | Prints what would go and what would stay; never requests quit, waits for exit, deletes files or tokens, or moves the app to Trash |
| `--interactive` | Asks about worktrees with unsaved work, and says what could not be deleted |
| `--report-failures` | Only says what could not be deleted |
| `--remove-unsaved-worktrees` | Deletes worktrees with unsaved work too |
| `--remove-worktree <folder>` | Deletes this worktree with unsaved work too; repeat as needed |
| `--keep-unsaved-worktrees` | The default |
| `--data-dir <folder>` | Only this `GHOST_DATA_DIR` folder, nothing of the installed app's |
| `--after-pid <pid>` | Waits for that process to exit first |
| `--trash-app <Ghost.app>` | macOS: moves the bundle to the Trash afterwards |

Exit codes:

- 0: everything was deleted; kept worktrees do not count as a failure.
- 2: an argument was not understood, and nothing was deleted.
- 3: Ghost is still running, and nothing was deleted.
- 4: something could not be deleted, and it was listed.

## Manual steps

Use these steps when the app is already gone.

1. **Save the worktrees you want.** In each repository you used with Ghost, run
   `git worktree list`. Commit or copy anything in a worktree under the data folder. Then run
   `git worktree remove --force <path>` (after `git worktree unlock <path>`) for each one you are
   done with.
2. **Delete the folders.**
   - **Windows:** quit Ghost, then delete the folders listed above:
     ```powershell
     Remove-Item -Recurse -Force "$env:LOCALAPPDATA\dev.onghost.desktop", "$env:APPDATA\dev.onghost.desktop"
     ```
   - **macOS:** quit Ghost, then delete the paths listed above:
     ```sh
     rm -rf ~/Library/Application\ Support/dev.onghost.desktop ~/Library/Caches/dev.onghost.desktop ~/Library/WebKit/dev.onghost.desktop ~/Library/HTTPStorages/dev.onghost.desktop* ~/Library/Saved\ Application\ State/dev.onghost.desktop.savedState ~/Library/Logs/dev.onghost.desktop
     defaults delete dev.onghost.desktop
     ```
   - **Linux:** quit Ghost, then delete the folders listed above:
     ```sh
     rm -rf ~/.local/share/dev.onghost.desktop ~/.config/dev.onghost.desktop ~/.cache/dev.onghost.desktop
     ```
3. **Delete the agent account tokens.** In Credential Manager, Keychain Access or your keyring,
   delete the entries named `auracode.agents.v1`. They belong to Ghost alone; your agents' own
   sign-ins are not kept there.

## Edge cases

| Case | What happens |
|---|---|
| Ghost is running | It is asked to quit and stops only its own agents and terminals. If it is still running, nothing is deleted (exit 3). The Windows uninstaller then offers to close it and runs the cleanup after removing the app, from a copy of the program. |
| A file is in use (a webview helper, an editor) | Retried for about ten seconds, then listed. Everything else goes. Running the cleanup again finishes once the file is free. |
| The secure store is locked or unavailable | The files go, and the cleanup says how many agent account tokens it could not remove and where to delete them by hand. |
| Git is missing, or a repository is gone | Its worktrees count as unsaved and stay unless chosen. A registration Git could not prune is reported, with the command to run. |
| A link or junction inside the data folder | Removed as a link; what it points to is never touched. |
| Read-only files (Git objects in checkpoints) | The read-only attribute is cleared, then the file is removed. |
| A data folder from `GHOST_DATA_DIR` | Only what Ghost made there goes, and the folder goes only if it ends up empty. |
| A second run | A completed cleanup removes nothing further. |
| An upgrade through the installer | Keeps everything (see the Windows uninstaller). |
| macOS app run from a disk image or a translocated copy | "Move to the Trash" is not offered; drag the app to the Trash yourself. |
| The cleanup cannot start (in Settings) | Ghost stays open, says why, and asks for a fresh review. |

Covered by tests:

- `cargo test -p ghost-core --lib uninstall`, with a temporary data folder and real Git;
- `cargo test -p ghost-desktop --lib`;
- `apps/desktop/scripts/uninstaller-hooks.test.mjs`, which checks the hooks against the pinned
  Tauri CLI's embedded installer template;
- the dialog's Vitest suite;
- `apps/desktop/scripts/check-delete-data.mjs`, in Chromium and WebKit.

Covered only by hand, on real machines: the uninstaller itself, the macOS Trash move and the
removal of agent account tokens from the real secure store. Automated cleanup tests use temporary
folders only.
