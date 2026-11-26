# Developing o7 inside o7

Chats in o7 often change o7 itself. The installed app runs those chats, its data folder
holds them and the worktrees being edited, and every agent process runs inside the app's Windows
job object, or on Linux and macOS is stopped with the agent's process tree when the app quits. So
anything that stops, replaces, migrates or uninstalls the installed app ends the session doing
the work. This page covers what the code guarantees, how to build and test a change
next to the running app, how to get the change into the installed app, and how to recover when
the new build is broken.

## Heavy work runs on GitHub

Builds and tests do not run on the owner's PC (owner, 24 November 2026). Compiling o7 takes every
core and many gigabytes of memory, and other programs crashed beside it. They run on GitHub's
machines instead:

- **Check** (`.github/workflows/check.yml`) runs what one change needs on one system. Push the
  branch, then start it with `node .github/scripts/remote-check.mjs`: it waits, prints each step,
  the failed steps' logs and the month's GitHub bill, and exits 0 only on success.
  - No options: Linux, rustfmt, Clippy, `cargo test --workspace`, then pnpm lint, typecheck and
    tests.
  - `--tests "-p o7-core --lib chatstore" --no-frontend`: one crate's tests; `--tests none` skips
    them, `--no-clippy` skips rustfmt and Clippy.
  - `--os windows --smoke stream`: the real app built with its frontend and smoke-run on Windows
    (`--os macos` likewise; scenarios are those of `run-smoke.sh`).
  - `--browser "check:chat --engines chromium; check:settings"`: browser checks, `;` between them.
  - `--no-wait` returns once it starts; `--watch <run id>` waits for a run already going.

  A branch's run starts from main's build cache. The first run on a system with none builds cold
  (CI once took 27 minutes of Clippy on Windows), so after a merge that changes dependencies, run
  Check on `main` too: `--ref main` (a worktree cannot check main out).
- **CI** is the full run on every system, before a release.
- **Publish** runs only with the owner's approval for that build.
- **Each time you start a build or a CI run, tell the owner the GitHub bill so far.** The helper
  prints it. It needs `gh`'s `user` scope once: `gh auth refresh -h github.com -s user`.

What stays local is what cannot go elsewhere: editing, quick scripts, and checks against a copy of
someone's real data, which never goes to a public repository's machines. Before anything local
that compiles, check that no game is running; WSL is capped at 8 GB and 6 cores (`.wslconfig`).

## What a development build cannot do

A development build is one with debug assertions: `cargo run`, `cargo test`, `pnpm dev`
(`tauri dev`) and `tauri build --debug`. Release builds (`pnpm bundle`, `cargo build --release`)
behave exactly like the installed app.

- **Its data folder is its own.** Without `O7_DATA_DIR` it opens `codes.o7.desktop.dev`
  next to the installed app's `codes.o7.desktop` (`DEVELOPMENT_FOLDER` in
  `crates/o7-core/src/paths.rs`). The installed app's chats are never migrated to a newer
  schema, which the installed app would then refuse, and never cleaned up. The CLI follows the same
  rule. `O7_DATA_DIR` still points a development build anywhere, the installed app's folder
  included, so using real data is a deliberate choice.
- **It is always its own instance** (`LaunchOptions::single_instance` in
  `apps/desktop/src-tauri/src/launch.rs`). It never hands over to the installed app. Its
  WebView2 profile is inside its data folder, and it does not share the
  window position.
- **Its cleanup stays in its folder.** Settings' "Delete all o7 data" and
  `--uninstall-cleanup` clean only the development build's own data folder. They never take the
  machine scope, which would delete the installed app's folder and ask the installed app to quit.
- **It cannot update.** An update installs over the installed app and closes it first.
- **Its window is titled `o7 dev`,** so neither a person nor automation that finds windows
  by title mistakes it for the installed app.

Every smoke run, in any build, needs `O7_DATA_DIR`. Without it the run reports a failure and
opens nothing, because a smoke run skips the single-instance hand-over and would otherwise open the
default folder next to a running app.

## Building on Windows with Smart App Control

When Smart App Control is on, Windows checks every new unsigned executable, including the build
scripts and procedural-macro DLLs cargo compiles. It blocks some of them ("An Application Control
policy has blocked this file", os error 4551) and remembers the verdict for that file, so retrying
the same build fails the same way. The location makes no difference, and a copied file loses its
verdict and is checked again. A cold target folder is therefore a lottery that can get stuck.

What works is building against a warm target folder whose build scripts and macros have already
run, such as the main checkout's: set `CARGO_TARGET_DIR` to it. Cargo names workspace artifacts by
their path inside the workspace, so a worktree reuses them, and only the final executable and the
test binaries are new. A workspace crate's build script can still be compiled anew (clippy
compiles its own copy), and that copy can be blocked. Delete just that unit,
`target\debug\build\<crate>-<hash>` and `target\debug\.fingerprint\<crate>-<hash>` as the error
names it, and build again: the rebuilt file is a new file and is checked again.

Sharing has one cost. Cargo decides freshness by comparing source modification times, so after a
worktree build the shared folder's workspace crates hold the worktree's code while the main
checkout's older sources look unchanged. When done, delete the workspace crates' compile
fingerprints, keeping the build scripts' own, so the next main build compiles its own sources:

```powershell
$target = "<main checkout>\target"
$names = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages.name
Get-ChildItem $target -Recurse -Depth 2 -Directory -Filter .fingerprint |
  Get-ChildItem -Directory | Where-Object { $names -contains ($_.Name -replace '-[0-9a-f]{16}$', '') } |
  Get-ChildItem -File | Where-Object { $_.Name -match '^(dep-|output-)?(lib|bin|test|run-build-script)-' } |
  Remove-Item
```

`tauri build` calls bare `pnpm`. Where pnpm only runs through corepack, put a `pnpm.cmd` that runs
`corepack pnpm %*` first on `PATH`.

## Testing a change next to the running app

On the owner's machine, Check's smoke runs and browser checks come first (see "Heavy work runs on
GitHub"). Build here only for what needs real data.

- **The UI:** the browser fixtures first (`check:*` scripts, `?preview=` modes). They need no
  native window and touch no data.
- **The native app:** `tauri build --debug --no-bundle`, then `target\debug\o7.exe`. It
  opens the development folder. For a throwaway first launch, set `O7_DATA_DIR` to a new
  folder. `pnpm dev` works too, but its Vite port (1420) is fixed, so only one worktree can run it
  at a time.
- **Stop what you started by its process id.** Never stop `o7.exe` by name: the installed
  app has the same name.
- **Drive windows by process id or handle,** and check the window is in the foreground before
  sending input. Otherwise keystrokes land in the installed app's composer.
- **Launch from a clean environment.** An agent's shell carries `CLAUDECODE` and the installed
  app's `O7_MCP_*` values, which include the project-memory helper's address and token. The app
  gives its agents a resolved environment anyway, but nothing it starts needs those values.
- **Smoke checks:** `.github/scripts/run-smoke.sh` (bash and jq), with a data folder of its own
  per run. `chat-reopen` covers what an upgrade does to running chats: the second launch finds the
  stored history, the interrupted turn and Resume.

## Opening the app

Every first read of the app waits behind the writer's first op, startup recovery
(`crates/o7-core/src/chatstore/recover.rs`). Nothing in it may read every event of a profile, or
of a chat: one turn can hold hundreds of megabytes. Each of its searches reads through an index
that holds only the rows it wants, and its tests name that index in the query plan.

What takes long runs after startup instead, as ordinary queued work that reads and live chats go
between (`ChatStore::leftovers`): two of those indexes are built once, outside the migrations
(a migration copies the whole database first, before any window exists), and turns above 1 MiB
are compacted. A turn too large to merge is read on a connection of its own, off the writer.
While the writer recovers or builds an index, a caller waits for its answer instead of failing
as busy.

The window's first frame shows the sidebar the last launch kept (`o7_core::launch_cache`): two
files in the profile's own folder, read back only by the build that wrote them, and never taken
as proof that a chat exists. `node apps/desktop/scripts/check-opening.mjs` checks what the sidebar
shows while it waits for the database (`?preview=opening`).

To measure a change to any of this:

- `cargo test -p o7-core --lib startup_on_a_large_profile -- --ignored --nocapture` builds a
  profile of `O7_LARGE_EVENTS` events (300,000 when unset), prints what startup took, and holds
  the first list to one second on a first start and on the next.
- Diagnostics shows what the running app's own launch took: opening the database, recovery, and
  the indexes when that launch built them. A release build keeps no log, so that card is where a
  slow start says what was slow.

Measured on 22 November 2026 in WSL, 700,000 events of about 1 KiB each with three chats left
mid-turn: the one search startup used to run over every event took 490 ms on data already in
memory (far longer from a cold disk); the first list now answers 86 ms after the writer starts
on a first launch and 8 ms on the next, and the work moved after startup takes about a second.

## The tour

The app has a tour that o7's wing flies: it lifts off from the logo at the sidebar's foot,
settles beside one control after another, and a card says what each is. The rule (owner,
22 November 2026): **every feature with a UI adds its stop to the tour in the same pull
request.** The tour is how a person finds out what the app can do, so a feature that is not in it
has not been introduced.

To add a stop:

1. Put `data-tour="<id>"` on the real control. The id is lowercase letters, digits and dashes,
   40 characters at most, and never changes afterwards: a profile saves the stops it has seen by
   id, so a renamed stop is offered to everybody again.
2. Add its entry to `TOUR_STOPS` in `apps/desktop/src/lib/tour/stops.ts`, where the tour should
   visit it: its `place` (`window` when the control is always there, `new-chat` when it is on the
   New chat page, `chat` when it needs an open chat), the `side` the wing settles on, a short
   title and one or two sentences in plain words.

Nothing else is needed. A profile that took the tour is offered a short flight of only the new
stop ("One more thing to see"); a profile that never took it gets it with the rest. A control
that a chat may lack (Run, the browser while its plugin is off) is passed, not waited for.

A control that should not have a stop gets a line in `NOT_TOURED` in the same file, saying why.
`node apps/desktop/scripts/check-tour.mjs` (part of `check:fixtures`) flies every stop against
`?preview=tour` in both engines and both appearances, and fails when a stop's control is missing
or when a control of the sidebar or of the chat's header has neither a stop nor a reason. A new
surface that should be held to the same rule is added to that check's list of controls.

How it is built: `components/tour/Tour.tsx` draws the wing, the ring and the card;
`lib/tour/flight.ts` is the geometry (where the wing settles, the arc it flies), tested without a
window; the seen stops are `tour_seen` in the profile's setup preferences
(`crates/o7-core/src/onboarding.rs`), which accepts only stop names and only ever grows. The wing
is the shared `Wing` (`packages/ui`), which the website flies too. On the desktop it flies only in
setup and the tour ([design/visual-language.md](design/visual-language.md)).

## Looks

The rule (owner, 25 November 2026, after ordering the Main styling pass): **every feature, and
every addition to the website, ships looking as good as the rest of the product, in the same
pull request.** "Every future feature needs to look as breathtaking as the product." Nothing
ships that only works. It holds for every surface: the desktop app, the website, the remote
page, the iPhone app, and the parts that are easy to skip (dialogs, menus, empty states, loading
states, errors).

What the look is: [design/visual-language.md](design/visual-language.md), "Main". What it was
measured against: [design/reference-audit.md](design/reference-audit.md).

Before a change that people can see is merged:

1. **Build it from what exists.** Colours, sizes, radii, type and durations come from the tokens
   (`packages/ui/src/graphite/tokens.css`, `apps/desktop/src/ui-tokens.css`) and controls from the
   shared primitives (`graphite.css`). A one-off value is a second look. `pnpm lint` refuses raw
   colours, spacing, radii, type sizes, weights and durations in the desktop's stylesheets.
2. **Design every state**: empty, loading, error, long text, a narrow window (960 by 640 for the
   desktop, 320 wide for the website).
3. **Both appearances and a second theme.** Light is not dark inverted, and a theme changes every
   colour: look at both, and at one agent's theme.
4. **Keyboard and focus.** Everything is reachable, and focus shows.
5. **Motion has a still form.** Anything that moves stays whole and readable under reduced
   motion.
6. **Look at it.** Open the state in a `?preview=` fixture (or the website's dev server) in o7's
   own browser and compare it with the references. A change that was never seen drawn is not
   finished, whatever its tests say.
7. **On the website**: dark, complete without JavaScript, still under reduced motion, no sideways
   scroll.

The tour stop and the changelog line below are part of the same habit: a feature is introduced
and announced when it lands, and it looks finished when it lands.

## Version

`VERSION` (three numbers, `major.minor.patch`) is the version o7 is published as. A pull request
that changes the app (the same paths as a changelog line: `apps/desktop`, `crates`,
`packages/ui`, the lockfiles and toolchain files) raises it in the same pull request, by how big
the change is (owner, 23 November 2026):

- **A big feature** raises the minor number and sets the patch to 0: `0.2.4` to `0.3.0`.
  Something a person would notice and that is worth telling people about: a new capability, a
  new surface, a change to how a whole flow works.
- **A small change or a fix** raises the patch: `0.2.4` to `0.2.5`.
- The major number moves only when the owner says so (`1.0.0`).

A pull request that only touches the website, the docs, the tests or the release tooling leaves
it alone. Builds are made only when the owner approves one, so pull requests gather into one
release: once main's `VERSION` is above the last published version, a pull request may leave it
as it is, and raises it further only when it is a bigger change than what was already raised (a
big feature after only small ones). The published version is what main holds when the build is
made. `pnpm lint` runs `.github/scripts/check-version.mjs`, which refuses a branch that changes
the app without a raise where one is due (it reads the published versions from the `v*` tags),
and Publish refuses one at build time.

## Changelog

The changelog is for the app only (owner, 21 November 2026: "change log is just for the app").
A pull request that changes the app, meaning what ships in it (`apps/desktop`, `crates`,
`packages/ui`, or what it is built with), adds its line to [CHANGELOG.md](../CHANGELOG.md) in
the same pull request. A pull request that only changes the website, the docs, the tests, the
release tooling or the repository's own setup adds no line. A change to both describes only the
app's part. The line goes under `## YYYY-MM-DD` for the day it merges in US Eastern time (the owner's
clock; 18 November 2026, after evening merges showed under the next day because the days were
UTC), newest first, and ending with its link,
`([#N](https://github.com/itscool2b/o7/pull/N))`. Open the pull request first to learn its
number, then commit the line. If it merges on a later day than planned, move the line. Check the
day with `node .github/scripts/check-changelog.mjs --dates` right before and right after the
merge: it compares every line's day with the time GitHub recorded, and wants a line whose pull
request is still open under today's Eastern day, so a line written before midnight and merged
after it is caught.
`pnpm lint` runs `.github/scripts/check-changelog.mjs`, which checks the dates, the order, the
links, that each pull request appears once, and that no line runs past 600 characters before its
link.

Write the line in simple English about what changed for the person using o7 (owner, 16
November 2026: the changelog is to "speak in English, simple English, about functionality changes
in o7"). The reader uses the app and may not be a programmer:

- **Headline first.** The words up to the line's first colon or full stop are the headline: a plain
  statement of the change, about twelve words at most. Then one to three short sentences of detail,
  and no more. Most lines stay well under 400 characters.
- **Short everyday words and short sentences.** Say what the app does differently now: "message
  box", not "composer"; "the chat shows", not "the transcript renders".
- **No internal names.** No crates, files, functions, style names, protocol names, test names or
  build tools. Names the person sees are fine: Claude Code, Codex, Settings, MCP servers, Windows,
  Mac.
- **Internal work on the app says so plainly.** A change to how the app is built or packaged
  opens with "Behind the scenes:" and says what it was for, such as "Behind the scenes: the Mac app
  builds again." Never invent an effect the person would not see.
- **Plain punctuation.** No em dashes, no emoji and no marketing words.

The website's changelog page is built from this file (`apps/web/scripts/changelog.ts`), and every
push to `main` redeploys the site, so a merged line is public within minutes (owner, 10 November
2026: the changelog "updates on the website as well"). The page shows a
line's words and its pull request number, not the link (the page was built while the
repository was private; it has been public since 19 November 2026). The headline
is set in ink and the rest of the line follows it as the body.

## Terms and Privacy

BASH.AI LLC publishes o7's Terms of Service and Privacy Policy (owner, 25 November 2026). Their
text is `apps/web/app/legal/terms.md` and `privacy.md`, shown at o7.codes/terms and /privacy;
change them only with the owner's approved wording. In the app, `apps/desktop/src/lib/legal.ts`
holds their effective date (`LEGAL_VERSION`). Setup's Welcome page says that continuing accepts
them, and a copy that has not accepted the current version shows one notice with both links until
OK (both documents promise an in-app notice for material changes). For a material change, update
the documents' date and `LEGAL_VERSION` together in the same pull request. Settings > About links
both and carries the copyright line, as the website's footer does. `LICENSE`, the workspace's
`authors` and the bundle's copyright name BASH.AI LLC; third-party notices keep their own.
Previews count as having accepted; `&legal=new` shows the notice.

## Getting a change into the installed app

On Windows, `apps/desktop/scripts/host-upgrade.mjs` replaces the installed app with a local build
and keeps the previous one. It runs from the repository root as
`pnpm --filter @o7/desktop host-upgrade <command>`.

1. Build the installer with `pnpm bundle`. It lands in
   `target\<target triple>\release\bundle\nsis\` or `target\release\bundle\nsis\`.
2. Commit and push the branch, so nothing depends on the worktree surviving.
3. Arm the install from the chat: `host-upgrade install --after-exit --installer <setup.exe>`.
   It returns at once. The waiter runs outside the app's job (started through WMI), from copies of
   the script and the installer in `%LOCALAPPDATA%\auracode-upgrades`. It waits up to 30 minutes
   (`--wait`) for o7 to quit.
4. Quit o7. The waiter moves the installed files in `%LOCALAPPDATA%\auracode` into a
   snapshot and leaves the folder empty for the installer. Smart App Control checks a copied
   executable again and may block it, so the snapshot keeps the very files it already allowed, and
   a restore moves them back. The waiter runs the installer in update mode (`/P /UPDATE`, as the
   updater does), which keeps the data folder (`src-tauri/windows/hooks.nsh`); the registration
   stays in the registry, so the installer treats the empty folder as an update. The exit code is
   not taken as proof: the waiter checks that `o7.exe` was written and logs its size and
   hash next to the previous build's. Then it starts o7 again. If the binary is missing,
   Windows refuses to start it, or it is not running 20 seconds later, the waiter restores the
   snapshot and starts that. A launch error is logged separately from a build that started and
   exited. If the restored app also cannot start, its files remain restored and that error is logged.
5. In the reopened app, chats that were running show their interrupted turn and Resume. An agent
   that keeps its own sessions, such as Claude Code, continues the same conversation, because its
   driver resumes by session id. Then check whatever only the installed app can show, such as the
   project-memory helper, worktree creation and the instruction files a new chat copies.

`host-upgrade status` shows the installed build, whether it runs, any armed install, the snapshots
and the last log lines. `host-upgrade cancel` disarms. The waiter keeps the three newest snapshots
(`--keep`). Without an agent, quit o7 and run `host-upgrade install --installer <setup.exe>`
from a terminal.

## When the new build is broken

- **It does not start, or exits at once:** the waiter has already restored the snapshot.
- **It starts but misbehaves:** quit it and run `host-upgrade rollback` from a terminal outside
  o7. That restores the newest snapshot, or the one `--snapshot <name>` names.
- **The restored app says the database is from a newer o7:** the new build migrated it. Run
  `host-upgrade rollback --restore-database`. o7-core backs a database up before every migration
  (`profiles\<id>\backups\aura.db.v<from>-<millis>.bak`). This puts back each database as it was
  before its first migration since the snapshot, and keeps the migrated one next to the backups as
  `<database>.rolled-back-<millis>`. Chats changed after the upgrade are only in that kept copy.
- **No working o7 at all:** run Claude Code or Codex in a terminal in the main checkout, fix
  forward, build an installer, and run `host-upgrade install` from that terminal.

The snapshots and the log (`%LOCALAPPDATA%\auracode-upgrades\upgrade.log`) are outside both the
install and the data folder. Neither an upgrade nor an uninstall removes them, and a later session
can read the log to see what happened.

## Linux

The installed app is an AppImage: `~/Applications/auracode.AppImage`, a link to a versioned file
beside it (`o7_0.1.0_amd64.AppImage`), which the desktop entry starts. The development
build's folder is `~/.local/share/codes.o7.desktop.dev`, by the same rule as elsewhere.

**Toolchain.** Rust through rustup (`rust-toolchain.toml` picks stable with rustfmt and clippy),
pnpm 12 (`mise use -g pnpm@12`, or corepack), Node from the supported range, CMake (dictation's
`transcribe-cpp-sys` builds with it; `mise use -g cmake` without root) and Tauri's system
packages; on Arch: `webkit2gtk-4.1 base-devel curl wget file openssl librsvg`. An AppImage build
also needs GStreamer's base and good plugins and `patchelf` (see "Sound and video" below); on
Arch: `gst-plugins-base gst-plugins-good patchelf`.

**What an agent's shell sees.** The AppImage's launcher points `LD_LIBRARY_PATH`, `PATH`,
`PYTHONHOME`, `PERLLIB`, `XDG_DATA_DIRS`, `GIO_MODULE_DIR` and others into its mount
(`/tmp/.mount_auraco…`). o7 strips them before it resolves the agents' environment
(`strip_launch_context` in `crates/o7-process/src/env/mod.rs`), so an agent's `node`, `git`,
`python` and `cargo` load the system's libraries, not the image's. o7's own helpers that
agents start (project memory, browser, mobile) run through the image's `AppRun`, which sets the image's
library path again for them alone (`helper_program` in `crates/o7-core/src/paths.rs`). An
installed build from before that fix leaks the launcher's variables: there, `node` fails with
"version `LIBFFI_CALL_PLAN_8.4' not found", `python3` with "No module named 'encodings'" and `git`
warns about `libpcre2-8.so.0`. Until such a build is replaced, start each command line with
`env -u LD_LIBRARY_PATH -u PYTHONHOME -u PYTHONPATH -u PERLLIB` and put the system's folders
first on `PATH`.

**Testing next to the running app.** `pnpm tauri build --debug --no-bundle` from `apps/desktop`,
then `target/debug/o7`. It opens the development folder, is titled `o7 dev` and runs
beside the installed app. Never stop `auracode` by name (`pkill o7`): the installed app's
process has the same name. Stop the PID you started. A plain `cargo build` or `cargo test` writes
`target/debug/o7` again without the page built in (it loads the Vite dev server's address
and shows an empty window when none runs), so build with the Tauri command again before starting
it.

On NVIDIA's driver under Wayland the window used to close as soon as it opened ("Error 71
(Protocol error) dispatching to Wayland display"): the driver puts the window's surface under
explicit sync once WebKitGTK's GPU renderer draws to it, and GTK 3 then commits a frame without an
acquire point ("Missing acquire timeline"). The app runs itself again with
`__NV_DISABLE_EXPLICIT_SYNC=1` there, unless that variable or `WEBKIT_DISABLE_DMABUF_RENDERER` is
already set (`keep_gpu_rendering_on_nvidia_wayland` in `apps/desktop/src-tauri/src/main.rs`). Do
not reach for `WEBKIT_DISABLE_DMABUF_RENDERER=1` instead: it also stops the error, but WebKitGTK
then paints on the CPU and copies every frame, and a chat scrolls at 21 frames a second where the
GPU renderer holds 60.

**The browser beside the chat on Wayland.** wry's child webviews are X11 child windows, so the
browser used to be unavailable on Wayland. There its pages are now GTK widgets: at startup, while
the window is still hidden, the app's own page moves into a `gtk::Overlay`, and each live tab's
WebKitGTK view joins it as an overlay child in a holder of exactly the panel's rectangle
(`apps/desktop/src-tauri/src/browser/gtk_layer.rs`). X11 keeps the child windows. The holder is
the page's size on purpose: GDK draws the app's page with OpenGL and uploads, every frame,
whatever GTK draws over it, so one layer across the whole window cost the main process 75 to 95 %
of a core for a small animation at 60 frames a second. The empty overlay, and a page in its
holder, cost nothing over the baseline (about 47 % with and without, three quiet rounds each;
that baseline is GTK 3 presenting a 4752x2600 frame, and forcing GDK's software path,
`GDK_GL=software-draw-gl`, takes 100 % and halves the frame rate, so the fast path is in use).

**Sound and video (GStreamer).** WebKitGTK plays media through GStreamer, and a page process that
needs a GStreamer element it cannot find aborts. On 17 November 2026 every page with sound or video
took its process down in the installed AppImage ("GStreamer element autoaudiosink not found", then
SIGABRT inside `WebKitWebProcess`): the image carried GStreamer's libraries but no plugins, and its
launcher (`AppRun.wrapped`) set `GST_PLUGIN_SYSTEM_PATH_1_0=$APPDIR/usr/lib/gstreamer-1.0:`, a
folder the image did not have, which also hid the system's plugins. The window's own page was at
the same risk, since Settings' Sounds play through Web Audio.

- **At startup** (`apps/desktop/src-tauri/src/browser/media.rs`) o7 looks for the plugin
  files WebKit's media cannot do without (`coreelements`, `app`, `audioconvert`, `audioresample`,
  `autodetect`, `playback`, `typefindfunctions`) in the folders GStreamer will search, by its own
  rules: `GST_PLUGIN_PATH_1_0` (else `GST_PLUGIN_PATH`), then `GST_PLUGIN_SYSTEM_PATH_1_0` (else
  `GST_PLUGIN_SYSTEM_PATH`), else `~/.local/share/gstreamer-1.0/plugins` and the system's folder
  (Debian's multiarch folder, `lib64`, then `lib`). It never starts GStreamer in its own process
  for this. The log says what it found: `GStreamer's plugins are here` or `GStreamer's plugins are
  not usable here ... missing=[...]`.
- **Where they are missing**, every browser tab and the window's own page start with Web Audio,
  media, Media Source, media stream, WebRTC, encrypted media and media capabilities off, set before
  their first load (tabs load only after their settings; the window's page is reached in setup,
  before its first document). Where they are there, WebKit's defaults stay. `app_info` carries
  `media: false` and a sentence naming the packages for the distribution (`os-release`); Sounds
  stay silent, Settings > Sounds says why and waits, and the browser panel says it once per device.
- **A page process that ends** (a crash, or out of memory) shows "This page stopped working." with
  Reload, on Linux (`web-process-terminated`, handled on the main loop's next turn) and on macOS
  (wry's content-process-terminate hook), as on Windows. A Linux tab whose page ends twice within
  30 seconds reloads with media off, and the panel says so. The window's own page reloads with media
  off after its process ends (Settings says sounds are off until a restart); if it ends again within
  30 seconds it is left alone rather than reloaded in a loop.
- **Packages**: Arch `gst-plugins-base gst-plugins-good`; Debian and Ubuntu
  `gstreamer1.0-plugins-base gstreamer1.0-plugins-good` (`gstreamer1.0-gl` for the GL video path,
  `gstreamer1.0-pulseaudio` for sound); Fedora `gstreamer1-plugins-base gstreamer1-plugins-good`;
  openSUSE `gstreamer-plugins-base gstreamer-plugins-good`. The deb and the rpm depend on them.

**What an AppImage build needs.** `pnpm bundle --bundles appimage` (and any bundle that includes the
AppImage) stages GStreamer's plugins from the build machine for Tauri's AppImage bundler
(`bundleMediaFramework: true`), whose linuxdeploy GStreamer plugin copies every file of
`GSTREAMER_PLUGINS_DIR` into the image with the libraries they need, copies the plugin scanner from
`GSTREAMER_HELPERS_DIR`, and adds a launch hook that points GStreamer at them
(`apps/desktop/scripts/appimage-media.mjs`). Only an allowlist is staged: GStreamer's core
elements and base and good plugins (conversion, playback, the GL video path, Ogg, Opus, Vorbis,
Theora, ALSA and PulseAudio output, MP4, WebM, VP8 and VP9, FLAC, MP3, WAV), never
gst-plugins-ugly, gst-plugins-bad or gst-libav, so there is no H.264 or AAC. The script stops
before the build, naming the packages for Arch, Debian and Ubuntu, and Fedora, when a required
plugin or `patchelf` (which the bundler's GStreamer step runs) is missing. After the build it
unpacks the image's GStreamer folder and launch hooks with the image's own runtime
(`--appimage-extract`) and checks the required plugins and the hook are there.

`--without-media` builds without plugins, on purpose and with a warning, and the check then makes
sure the image has no plugin folder. Such an image uses the system's plugins: at the very start of
`main`, when `APPDIR` is set and `$APPDIR/usr/lib/gstreamer-1.0` is missing, o7 runs itself
again (the same single re-run as the NVIDIA switch below) with `GST_PLUGIN_SYSTEM_PATH_1_0` on the
system's folders in place of the launcher's missing one (a value the person set after it is kept).
Every AppImage also gets a GStreamer registry of its own under
`~/.cache/codes.o7.desktop/gstreamer-1.0/` unless the person chose one: the image's plugins sit
at a mount that moves with every launch, and in the shared registry they would make the person's
other GStreamer apps scan everything again. Agents inherit none of these (`O7_LAUNCH_SET`,
and `strip_launch_context` drops the GStreamer hook's variables). An image without plugins loads
the system's plugins with its own GStreamer, which refuses plugins built for a newer GStreamer: when
the system's is newer than the image's, media stays off and Settings says so.

**Pictures of a page on Linux.** The panel's stand-in while a menu opens over the page, and the
agents' `browser_screenshot`, come from WebKitGTK's own snapshot (`browser/snapshot.rs`): what the
tab shows as a JPEG, the whole page or one element as a PNG, in the screen's pixels, encoded on a
worker thread. A whole page larger than 48 million pixels is refused with a sentence that says to
picture what shows or one element instead. macOS has no pictures yet.

**Pages parked for an agent on Wayland.** A page an agent works in while the panel is closed is
parked at -24000 inside the overlay. Measured on 17 November 2026 (Hyprland, NVIDIA, the window on
an output that draws at 60 frames a second): an animated page parked used 0.2 % of a core in the
main process and 0.2 % in its own, drew no frames (`requestAnimationFrame` pauses) and kept its
timers at full rate; hidden, it also drew nothing and WebKit slowed its timers to once a second;
shown over the panel it drew 63 frames a second for 7.8 % and 2.2 %. Parking costs nothing and keeps
the page's scripts running, and agents' snapshots and pictures do not need its frames, so it stays.
A page that waits on `requestAnimationFrame` waits while parked on Wayland.

**Measuring frame rate without a window on your screen.** `hyprctl output create headless NAME`
adds a virtual monitor (give it a position far from the real one, such as `-30000x0`, or the
pointer can slide onto it, and check the real monitor stayed at `0x0`); set its mode and scale to match the real one
(`hyprctl eval 'hl.monitor({ output = "NAME", mode = "3840x2160@60", scale = 1.6 })'`) and move the
test window to the workspace it shows. A window on a workspace no monitor shows gets no frames, so
`requestAnimationFrame` stalls and every measurement hangs. Start the build with
`O7_DATA_DIR` on a scratch folder and `WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:<port>`, then
drive the page over the inspector's WebSocket (`Runtime.evaluate`). `O7_STRESS=orbs=24,seconds=0`
opens the stress view; the scripted Demo agent (`mock`) gives a real chat to type, stream and
scroll in without spending anything. Remove the monitor with `hyprctl output remove NAME`. Measure
at the window's real size: at half width the same build drew several times the frames. Other
work on the machine moves the numbers a great deal (a compile in another worktree doubled the
main process's CPU and halved frame rates): lower its priority with `renice`, run each pair of
variants interleaved, three rounds, and distrust a single reading.

**Getting a change into the installed app.** `pnpm --filter @o7/desktop host-upgrade`
hands over to `apps/desktop/scripts/appimage-upgrade.mjs` on Linux.

1. Build the AppImage: `pnpm bundle --bundles appimage`. It lands in
   `target/release/bundle/appimage/`. It needs GStreamer's base and good plugins and `patchelf` on
   this machine (see "Sound and video" above).
2. Commit and push the branch.
3. Arm the install from the chat: `host-upgrade install --after-exit` (the newest AppImage under
   `target`, or `--appimage <file>`). It copies the AppImage beside the installed one at once,
   so the worktree may go away, and returns. Its waiter runs under the user's systemd
   (`systemd-run --user`, unit `auracode-upgrade-<millis>`), which shares no process with the
   app, so it outlives the agent that armed it. Without user systemd it starts in a new session
   that is reparented to init.
4. Quit o7. The waiter moves the link to the new build in one rename and starts it as its
   own systemd unit, with the environment the desktop launcher gives. If no process runs the new
   file 20 seconds later (`--health-seconds`), it moves the link back and starts the previous
   build. The previous build stays on disk either way; the three newest older builds are kept
   (`--keep`), and never the one running or the one a rollback would use.

`host-upgrade status` shows the link, what runs, any armed install, the kept builds and the last
log lines; `host-upgrade cancel` disarms. The log and the install history are in
`~/.local/state/auracode-upgrades` (`$XDG_STATE_HOME`), outside the data folder. Without an
agent, quit o7 and run `host-upgrade install` from a terminal: it installs and starts at
once.

**When the new build is broken.** Quit it and run `host-upgrade rollback` from a terminal: the
link goes back to the build the last install replaced (or `--to <file>`), and that build starts.
`--restore-database` also puts back each database the newer build migrated, from o7-core's
pre-migration backups, as on Windows. With no working o7 at all, run Claude Code or Codex
in a terminal in the main checkout, fix forward, build an AppImage and run `host-upgrade install`.

## macOS

A development build's folder is `~/Library/Application Support/codes.o7.desktop.dev`, by the
same rule. WebKit keeps one website-data store per bundle identifier, so a development build shares
it with the installed app (`LaunchOptions::webview_data_dir` has no effect there). The app keeps
nothing in web storage except the stress view's frame log, so this is harmless today. Anything
that starts keeping state in web storage changes that. `host-upgrade` covers Windows and Linux.
On macOS, copy `/Applications/o7.app` aside before replacing it.

## Verification

Run on 2026-11-01 on Windows 11 with Smart App Control on, from a chat inside the installed app.
The development build was built from this branch against the main checkout's warm target.

| Check | Result |
|---|---|
| A development build launched beside the running installed app stays up as its own instance, opens `codes.o7.desktop.dev` with its WebView2 profile inside, is titled `o7 dev`, and leaves the installed app's processes alone | pass |
| A smoke run without `O7_DATA_DIR` prints a failing result naming the reason, exits 1 and opens no store | pass |
| `--uninstall-cleanup --dry-run` from a development build plans only the development folder | pass |
| Smoke checks with the development build in scratch folders: stream, chat (30 of 30), chat-reopen (8 of 8) | pass |
| An agent's own processes are in the app's job; a process started through WMI is not (its parent is `WmiPrvSE.exe`) | pass |
| `host-upgrade` against the real install while it runs: `status` reports it, `install` refuses, `rollback` finds no snapshot, nothing changes | pass |
| `host-upgrade install --after-exit` against a stand-in install: the WMI waiter polls, proceeds when the stand-in quits, snapshots it, runs the installer, restores the snapshot when the installer fails, starts the app and removes its record; `cancel` stops an armed waiter | pass |
| The snapshot holds, and a restore puts back, the installed files themselves (same file ids), not copies | pass |
| Running a real installer over the installed app and reopening it | pass, on the second run. The first run failed: the installer exited 0, wrote its uninstaller and registry entries, and left the fresh copy of the previous `o7.exe` in place (the installer's payload held the new binary; no Code Integrity event; no file lock could be reproduced), and the waiter reported success from the exit code alone. With the emptied folder and the binary check, the second run logged `Installed o7.exe: 61048320 bytes, sha256 9640fbef… (previous cce195a7…)`, the installed file matches and carries the new build's smoke message (`SMOKE_NEEDS_DATA_DIR`, absent from the previous build; the development-only strings are compiled out of a release build, as expected), and the app reopened with its chats, the project-memory helper and Resume working |
| The installed binary is the one the bundle carried, not the file left in `target` | pass, with a caveat: `tauri build` rewrote `target\…\release\o7.exe` once more after `makensis` had read it (the link at 13:40:22 went into the installer; the file on disk is from 13:40:43, same size, different hash). Compare an install against the hash in the upgrade log, not against that file |

A later install attempt on 2 November 2026 at 00:22 UTC returned `-1` and restored the snapshot.
Windows Code Integrity events 3033 and 3077 at that time identify the staged installer as blocked
by the machine's signing policy. The previous installed binary remained in place, and no install
was still armed when checked. The helper now records a process-start error before the exit code
and rollback message, so a blocked installer is distinguishable from an installer that ran and
failed. This observation does not establish whether a different future build will be accepted.

Linux, run on 16 November 2026 on Omarchy (Hyprland, Wayland, NVIDIA RTX 5080, WebKitGTK 2.54)
from a chat inside the installed AppImage:

| Check | Result |
|---|---|
| An agent's shell in the installed build (before the fix) | fail, as described above: `node`, `python3` and `pacman` do not start, `git` warns |
| `strip_launch_context` against the variables that AppImage's launcher set | pass (unit tests), including entries outside the mount kept in order and an `APPDIR` that is not a mount |
| The memory helper started through the mount's `AppRun` from an env without the image's variables | pass: MCP `initialize` answered in 39 ms |
| `tauri build --debug --no-bundle` in a worktree, then `target/debug/auracode` beside the installed app | pass: titled `o7 dev`, its own data folder, same PID after the restart with the NVIDIA switch, the installed app untouched; without the switch the window closes with Wayland error 71 |
| Frame rate at 2376x1300 on a virtual 4K monitor at 1.6 (GDK scale 2), NVIDIA RTX 5080, same build, GPU renderer against CPU painting | stress view 60 against 12 to 25; a chat with the scripted agent: streaming 56 to 63 against 42, scrolling 60 (worst frame 28 ms) against 21 (153 ms); idle 0 % CPU on both; typing reaches the screen in about 23 ms on both |
| The browser panel on Wayland (Hyprland), on the virtual monitor | pass: a local page draws in the panel at its rectangle (1040x1202 at 2x), follows a move and resize, hides with the panel and returns; an agent's snapshot reads the page while it is hidden; the chat holds 60 frames a second beside it. Not tested: real pointer and keyboard input into the page (no second seat to drive it) |
| `host-upgrade status` against the real install | pass: names the link, the running build and its PID |
| `host-upgrade install`, `--after-exit` and its waiter, health rollback, `rollback --restore-database`, `cancel`, prune | pass against a stand-in install (`appimage-upgrade.test.mjs`); not yet run against the real install |

Sound and video, page processes and pictures, run on 17 November 2026 on the same machine (GStreamer
1.28.8 with `gst-plugins-base-libs` but neither `gst-plugins-base` nor `gst-plugins-good`, WebKitGTK
2.54.1, the screen asleep and locked, the test windows on Hyprland's fallback output, which draws at
60 frames a second), with a development build and a development AppImage built from this branch,
each with a data folder of its own, driven through WebKit's remote inspector and the development
`browser_agent_call` command:

| Check | Result |
|---|---|
| Detection | pass: the log names `autodetect` missing in `~/.local/share/gstreamer-1.0/plugins` and `/usr/lib64/gstreamer-1.0`; `app_info` and the browser status carry the Arch packages |
| The window's page and a tab where media is off | pass: no `AudioContext`, `RTCPeerConnection` or `navigator.mediaDevices`, `<video>` is no media element, `MediaSource.isTypeSupported` answers false; a page with video, audio and Web Audio loads and its process stays up, in the AppImage too |
| Where media is believed available (a stand-in `autodetect` file) | pass: WebKit's defaults stay (`AudioContext` in the window's page and in tabs) |
| A tab's page process killed (SIGKILL, no core dump) | pass: the tab reads crashed (the panel's Reload card), Reload builds it again; killed again within 30 seconds it reads `media: false`, and after Reload its page has no `AudioContext` and no media elements |
| The window's page process killed | pass: it loads again with media off and `app_info` says so; killed again 15 seconds later it is left alone and the log says so |
| Pictures | pass: the panel's stand-in is a 1280x800 JPEG with the test page's colours and box where they belong; the agents' viewport JPEG, full page PNG and element PNG come back |
| Clear browsing data with no page open, on Wayland | pass (it failed with `UnsupportedWindowHandle` before) |
| `pnpm bundle --bundles appimage` here | pass: stops before building, naming the missing plugin and `patchelf` and the packages for Arch, Debian and Ubuntu, and Fedora |
| A development AppImage without plugins | pass: the image check passes for an image without media and refuses it as one with media; launched, one re-run sets the system's plugin folders, a registry of o7's own and the NVIDIA switch, and `O7_LAUNCH_SET` names all three |
| X11 through XWayland (`GDK_BACKEND=x11`, pages as child windows) | pass: media off in a tab, its picture, a page end read as crashed, Clear browsing data with no page open. The first three X11 launches that afternoon left the window's page on `about:blank` (with either renderer); nine later launches of the same code, the very same binary among them, loaded normally, and the cause was not found |
| An AppImage that carries the plugins; sound and video actually playing | not run: this machine has no `gst-plugins-good` and no `patchelf`. CI's Ubuntu 22.04 job installs the plugin packages |
