# Explicit vendor integrations

The Plugin library's actions (Add plugin, Add package, Add marketplace, and each row's Enable,
Disable, Update and Remove) prepare a personal, native CLI command. Review does not start a process. Start setup consumes a page/profile-bound token, rechecks executable bytes,
arguments, canonical home/configuration folders and the relevant public declarations, then opens
the owned terminal. Reviews expire after ten minutes. At most four can be preparing or pending.
Project rows stay separate. Unsupported commands, private sources, arbitrary URLs, local paths and
cloud-managed `synced`/`claudeai` identifiers are refused rather than translated into file writes.

The terminal retains profile admission, the Library mutation lock and an owned maintenance guard
until cleanup is confirmed. Close, page reload and the ten-minute command deadline stop only the
owned process tree. Unconfirmed cleanup retains these guards until recovery/restart. Quit and app
update wait for this work; no agent slot is consumed. Vendor changes can be partial and cannot be
rolled back as a Library copy. Refresh reads the result instead of treating exit status as proof
that an integration is installed. No terminal output or credential value is logged or synchronized.

Commands preserve interactive vendor approval. No `--yes`, `-y`, install-command acceptance or
authentication configuration is supplied. The native CLI uses its own existing authentication.
Ghost reads only approved public declarations to bind the review. Protected or malformed
declarations make the review unavailable. A missing personal configuration folder is permitted
for first installation and is distinguished from an unreadable folder.

## Reviewed command sources

Claude Code 2.1.310 uses the [official plugin CLI reference](https://code.claude.com/docs/en/plugins/cli-reference):
`plugin install`, `update`, `enable`, `disable`, and `uninstall NAME@MARKETPLACE --scope user`.
Uninstall also passes `--keep-data`. Marketplace add accepts a credential-free GitHub owner/repo;
remove passes `--scope user`; update always names one marketplace. Marketplace removal can remove
dependent plugins and their persistent data, so both the form and command review describe that
consequence. Plugin identifiers are always qualified to avoid marketplace ambiguity.

Codex 0.162.1 uses pinned
[plugin_cmd.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/cli/src/plugin_cmd.rs)
and [marketplace_cmd.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/cli/src/marketplace_cmd.rs):
`plugin add NAME@MARKETPLACE`, `plugin remove NAME@MARKETPLACE`, and
`plugin marketplace add|upgrade|remove TARGET`. No plugin enable, disable or update command exists
in this pinned parser. Those operations are unavailable. Its configuration fields are `plugins`
and `marketplaces`; synchronization alone must never activate either declaration.

Pi 1.0.2 uses pinned
[package-manager-cli.ts](https://github.com/badlogic/pi-mono/blob/v1.0.2/packages/coding-agent/src/package-manager-cli.ts)
and [main.ts](https://github.com/badlogic/pi-mono/blob/v1.0.2/packages/coding-agent/src/main.ts):
`install SOURCE --no-approve`, `remove SOURCE --no-approve`, and
`update --extension SOURCE --no-approve`. The package branch returns before agent startup.
`--no-approve` ignores project configuration, and no `--local` is supplied. New installs accept
only exact npm versions or GitHub sources with an explicit tag/commit. Unversioned existing
sources can be removed or updated explicitly. An empty target can never become Pi's self-update
or a batch package update.

Windows npm shims are resolved through reviewed package manifests and Node, never interpreted as
shell text. Unsupported shims are refused. Executable and Node entry bytes are bounded and hashed.
Only home and provider configuration-path overlays are fixed; credential environments are neither
read for review nor copied into its metadata. These safeguards do not claim remote package content
is immutable or trusted. The user reviews the source and vendor prompts before executing it.

## Verification limits

Synthetic planner tests cover exact arguments, source/flag rejection, executable replacement and
configuration-root changes. Core fixtures cover declaration drift, private-field preservation and
owned maintenance admission. React tests and the Library browser fixture cover the separate review
and Start steps without vendor processes. Native execution and platform acceptance remain part of
the final integrated Windows/macOS verification. No real vendor installation was run in development.
