# Local configuration Library

The app shows this inventory through the per-agent [Plugin library](plugin-library.md). Instruction
files are still inventoried and copied by this crate for its other callers, but the Plugin library no
longer lists them: they belong to the separate Instructions surface.

`ghost-config` is the Rust adapter boundary. The host supplies approved user or project roots; callers
select logical item IDs, never arbitrary paths. Inspection is explicit and local. It does not run an
agent, load a skill, resolve an environment reference, connect to MCP, or install a package.

The initial inventory covers Claude, Codex and Pi instruction files, portable skill directories,
MCP declarations, Claude/Codex plugin declarations and Pi package declarations. Shared Agent Skills
have one separate source, so the same installation is not counted once for every provider. Missing
roots are reported independently. A malformed source reports a sanitized error and remains unchanged.

Credential files, session stores, caches, `.env` files, key files, vendor `synced` directories and Git
internals are excluded. Config parsing selects only the relevant section. Claude's `.claude.json`
never becomes an item as a whole; projects, trust state and account metadata are not exposed. Plugin
previews include only the declaration name and enabled state. A declaration does not establish that
the corresponding plugin/package is installed or active.

Reads use directory capabilities, no-follow traversal, regular-file checks and bounded reads. Limits
are 1 MiB per file, 256 files and eight directory levels per skill, 16 MiB per source and 2,048 items.
A skill with an excluded or unavailable companion file stays visible but cannot be copied as an
apparently complete bundle. Revisions cover all companion bytes, including binary assets. No file
contents or parser excerpts are logged. Tests use temporary synthetic roots only.

MCP conversion is a closed subset: stdio command/arguments or HTTP URL, with supported enabled state.
Unknown vendor options, environment/header/auth fields, URL userinfo/query/fragment and suspicious
argument interpolation require manual setup. Disabled servers cannot become enabled during a copy.
This is not a general secret detector: arbitrary instructions and scripts remain user-authored content
that requires review. Unsupported fields are never silently discarded to make a conversion succeed.

Source evidence checked on 2026-10-26:

- Claude's official [configuration scopes](https://code.claude.com/docs/en/settings),
  [MCP configuration](https://code.claude.com/docs/en/mcp),
  [skill layout](https://code.claude.com/docs/en/skills), and
  [plugin manifest](https://code.claude.com/docs/en/plugins-reference). These are live documentation;
  the current native driver is pinned separately to Claude 2.1.310.
- Codex [configuration schema at rust-v0.162.1](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/config.schema.json),
  matching the committed native protocol schema. Its `mcp_servers` and `plugins` tables are parsed
  independently of `projects`, provider keys, profiles and trust state.
- Pi 1.0.2 [configuration](https://github.com/earendil-works/pi/blob/v1.0.2/packages/coding-agent/docs/configuration.md),
  [MCP](https://github.com/earendil-works/pi/blob/v1.0.2/packages/coding-agent/docs/mcp.md), and
  [skills](https://github.com/earendil-works/pi/blob/v1.0.2/packages/coding-agent/docs/skills.md).
  Pi packages and executable extensions are not interchangeable with another provider's plugins.

Instruction files and complete skill bundles can now be copied through a ten-minute, one-use Rust
preview. The source revision covers supporting bytes and executable bits. Apply checks all target
revisions before touching any item and keeps the original target directory handle from preview.
Replaced files and independent snapshots remain in an excluded `.auracode-library/<UUID>` directory.
Publication refuses to overwrite a path another writer created. A partial failure returns its recovery
ID and retains originals and staged content. This needs a filesystem with hard-link support; a failure
is reported instead of falling back to an unsafe overwrite. The host must serialize mutations and
keep the blocking operation owned after an IPC caller disconnects.

Rollback validates the retained journal, selected paths and current revisions before any write. It
works after restarting the host and recovers interruption between moving an original and publishing
its replacement. Completed-copy markers distinguish interruption from a later user deletion. Newer
edits are retained, and unrelated target files are neither copied into the journal nor removed.
Windows file contents are flushed before mutation; macOS also flushes containing directories.

The desktop root resolver uses only named configuration path variables and registered local project
folders. It never exports the login environment. Relative overrides need a selected project; a custom
Claude home omits user MCP inventory until the corresponding state-file mapping is verified. Rust
commands serialize changes and retain maintenance admission until the blocking journal operation
finishes, including after its webview caller leaves. Copy history reads validated metadata after
restart and distinguishes completed, interrupted and rolled-back operations.

Library DTOs also have an independent pure-crate Specta exporter, checked for drift in CI. The
combined Windows desktop command export passed at d000c29. Later native verification remains
subject to the final integrated checks and the recorded Application Control limitations.

The dark Library screen supports source/agent/category filters, literal previews, a before/after
copy review, explicit apply and restart recovery. Unsupported MCP fields and plugin/package copies explain
the limitation instead of displaying a working action. A preparation that completes after its
item is closed is canceled; stale apply errors close the consumed review and remain visible.
Synthetic Chromium/WebKit tests cover these states and accessibility. Its standalone fixture
is `library.html`, served only by the development entry, and reads no actual configuration.

MCP copies change one reviewed stdio/HTTP entry, including the required Claude HTTP discriminator.
They preserve unselected JSON spans and TOML tables/comments, and reject unknown, credential-bearing
or incompatible selected entries. Persistent recovery metadata contains only the selected safe
before/after values. Full documents are staged privately in the same approved configuration realm
for checked publication, then removed after success. An interrupted or uncertain transaction retains
them locally for recovery and never includes them in inventory or previews. Metadata is saved
before staging, so an interrupted transaction is discoverable. Restoring a server applies its inverse
entry change to the current document, preserving unrelated edits made after the original copy. An
edited selected entry or a later whole-file deletion is refused. Agent enable/approval state remains
vendor-owned and is not copied between providers.

Pi and shared portable skill collections are discovered recursively with eight-level and 2,048-entry
bounds. Nested keys retain their collection path so same-named directories stay distinct. Copying
uses the destination's direct skill folder, previews any existing target and retains the normal
rollback journal. Private/excluded folders and links are never traversed. A selected duplicate shows
the other relative locations and the relevant vendor precedence rule; the Library never picks an
active runtime winner. The current [Claude skill rules](https://code.claude.com/docs/en/skills) and
[pinned Pi skill rules](https://github.com/earendil-works/pi/blob/v1.0.2/packages/coding-agent/docs/skills.md)
were checked on 2026-10-26. Claude monorepo subdirectories are inspected through the explicitly
selected project folder, rather than an unrestricted repository crawl.

Reviewed official install/remove/update commands are available through Manage integrations where
the installed vendor supports them. Executable packages cannot be blindly copied between providers.
macOS native execution and the final integrated acceptance remain pending. M8 and M9 are not marked
complete. No real agent configuration was read or changed during these checks.
