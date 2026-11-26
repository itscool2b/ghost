# Instruction files: one file for every agent, or a file per agent

The Instructions page in Settings manages the markdown files coding agents read for their standing
instructions (`AGENTS.md`, `CLAUDE.md` and the rest). It replaces the Library's instruction items:
the Library's user-level instruction copy (Library copies of `CLAUDE.md`, `AGENTS.md` and
`AGENTS.override.md`) is superseded by this page, and the planned per-agent Library rework can drop
its instruction items. The Rust logic (`crates/ghost-config/src/instructions/`) has no dependency on
the Library interface.

Implementation: `ghost-config::instructions` (reading, classification, plans, the transaction),
`ghost-core` (`session/instructions.rs`, `gitworkspaces/instructions.rs`: targets, Git facts,
worktree setup, worktree copies, the single-file commit, owned applies), `apps/desktop/src-tauri/src/instructions_commands.rs`
(typed commands), and `apps/desktop/src/components/settings/InstructionsPanel.tsx` with its review
dialog.

## Scopes

- **Your user files**: each agent's user-level file (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`,
  ...). These are the person's real agent configuration, so they are only read on opening the page
  and only written by an explicit, reviewed Apply.
- **A project folder**: the folder's root. A project with several folders lists each folder
  separately; each is its own scope with its own files and record.
- **A chat's folder**: for a worktree chat, its worktree. The page says that changes stay on the
  chat's branch until it is merged.

The page opens from Settings (Instructions), from a folder's Instructions button in project
settings, and from the palette ("Edit instructions", and "Edit this chat's instructions" with a chat
open). The interface sends a target and a file key, never a path; Rust resolves every folder from the
same environment agents run in (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR`,
`GROK_HOME`, `XDG_CONFIG_HOME`).

## Modes

The mode is read from the files, not stored: if every agent that reads something reads the same text,
the scope is **one file**; if agents read different text it is **per agent**; with no text anywhere it
is **none**. A local record (`<data>/instructions/managed.json`) remembers which files Ghost keeps
as managed copies, so a hand edit can be told from a stale copy. It lives in Ghost's data folder,
never in a project or an agent folder.

### One file for every agent

In a project the shared file is always `AGENTS.md`, because every catalog agent reads it natively
(a `CLAUDE.md` source would need a copy that Cursor, Grok Build and Antigravity load twice). Each
agent is reached by the least invasive official mechanism:

1. **Native read** where the agent reads the shared file itself. Claude Code reads `AGENTS.md`
   natively (2.1.277 and later; the catalog requires 2.1.310) when no `CLAUDE.md`,
   `.claude/CLAUDE.md` or `CLAUDE.local.md` is in the folder or a parent folder.
2. **Official import** otherwise: `CLAUDE.md` containing `@AGENTS.md`. A `CLAUDE.md` that holds only
   imports of the shared file, or is a link to it, is detected as one-file already.
3. **Managed copy** only where neither works: a byte-identical copy Ghost keeps in sync and
   labels as such. In practice this happens only at user scope, where agents' files live in
   different folders.

At user scope the shared file is one agent's user file (by default Codex's `AGENTS.md` when Codex
is installed, else the first file with text). Claude Code imports it with a home-relative path
(`@~/.codex/AGENTS.md`); Pi, oh-my-pi, OpenCode, Antigravity and Grok Build get managed copies.
oh-my-pi and OpenCode get their own copies because they would otherwise fall back to
`~/.claude/CLAUDE.md`, which they read as plain text (OpenCode does not parse imports, and
oh-my-pi's support for `~` in imports is not documented). Hermes and Cursor have no user-level
instructions file and are listed as such.

Symlinks are never created (Windows needs privileges for them). Existing links are detected and
respected.

### A file per agent

Each agent keeps the text it reads now as a file of its own: an import file becomes a full copy of
the text it imported, a link becomes a regular file with the same text, a hard link becomes a separate
file, and managed copies stop being managed (their text stays). With only `AGENTS.md`, Claude Code is
given a `CLAUDE.md` of its own starting from the shared text. Nothing is deleted.

Several agents share `AGENTS.md` in a project, so a separate file is not always possible. The page
says who reads each file, and lists the agent-specific files where they exist: `CLAUDE.md` (Claude
Code; Cursor and Grok Build also read it), `AGENTS.override.md` (Codex, Pi and Hermes all read it
instead of `AGENTS.md`), `.omp/AGENTS.md` (oh-my-pi), `.hermes.md` (Hermes) and `GEMINI.md`
(Antigravity, read together with `AGENTS.md`). OpenCode, Cursor and Grok Build have no project file
of their own besides `AGENTS.md`/`CLAUDE.md` (their rule folders and `opencode.json` instructions are
out of scope).

## Reviews and writes

Every write is a review first: switching modes, saving text, restoring or adopting an edited copy,
and renaming a case variant. The review names every file that is created, replaced, renamed, removed
or kept, with a diff for each written file, how the file serves its agents afterwards ("Claude Code
imports AGENTS.md", "Managed copy of ~/.codex/AGENTS.md"), and notes. Reviews wait ten minutes,
then expire.

Apply checks every reviewed file (and every instruction file the plan did not touch) against the
exact bytes reviewed and refuses before the first write if anything changed; the person reviews
again. It writes the replaced bytes to `<data>/instructions/backups/<id>/` (the newest 40 settled changes are
kept; unresolved journals are never pruned), stages new bytes next to each file, moves the original aside and publishes with a
no-overwrite hard link (a file created meanwhile is never overwritten; file systems without hard
links fall back to a checked rename). If any file fails, every file already changed is put back; if
one cannot be put back, its backup is kept and the result says so. Writes share the configuration
mutation gate with Library copies, run inside the maintenance owner so quit and update wait for
them, and only touch the instruction files themselves.

Agents read instructions when a session starts; every review and the page say that running chats
keep what they started with and new sessions read the new files.

## Worktree chats

A worktree checks out only what Git tracks, so a project's instruction files that Git ignores (a
local `AGENTS.md` or `CLAUDE.md`, say) would be missing from every new worktree chat. By default
Ghost copies them in, from the source folder into the new worktree, before the chat's agent
starts. This happens for a new worktree chat (after its worktree setup, so a setup copy of the same
file is kept), for a chat isolated into a worktree, and for a recreated worktree.

- Only the app's own list of project instruction files is copied (`project_paths()`), never a path
  someone typed, so `.claude/CLAUDE.md` and `.omp/AGENTS.md` qualify while setup copies refuse
  agent folders. Links are never followed and files over 1 MiB are reported, not copied.
- Create-only: a file already in the worktree is kept.
- Only files Git ignores. A file that is only untracked stays out, because in the worktree it would
  be a new change an agent could commit on the chat's branch; commit it instead.
- A copy the worktree does not ignore (its rule is in a `.gitignore` change that is not committed
  yet) is taken back out, so the new worktree never shows changes of its own.
- A folder ignored whole (`.claude/`) counts for the files inside it.

The switch is Instructions' **Worktree chats** section (Settings > Instructions), stored as the
profile preference `copy_instructions_to_worktrees` and on unless turned off; profiles from before
it read it as on. A problem copying never fails the chat: a new worktree chat or an isolation shows
it as a warning, and a recreate logs it.

## Per-agent table

What each catalog agent reads, as implemented in `instructions/agents.rs`. Unconfirmed vendor
behaviour is not modelled.

| Agent (catalog ids) | Project files, per folder | User file | Imports | Limits |
|---|---|---|---|---|
| Claude Code (`claude`, `claude-acp`) | `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` all load; `AGENTS.md` only when none of those is in the folder or a parent [C1] | `$CLAUDE_CONFIG_DIR/CLAUDE.md`, default `~/.claude/CLAUDE.md` [C2] | `@path`, relative to the importing file, four hops, not in code spans or fences [C1] | Skips a file over 4 MiB; suggests under 200 lines [C1] |
| Codex (`codex`, `codex-acp`) | First non-empty of `AGENTS.override.md`, `AGENTS.md` (fallback names from config are not modelled) [X1] | `$CODEX_HOME/AGENTS.override.md`, else `AGENTS.md` [X1] | None documented | 32 KiB across all loaded files (`project_doc_max_bytes`, read from `config.toml`) [X1][X2] |
| Pi (`pi`) | First of `AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md` [P1][P2] | Same names in `$PI_CODING_AGENT_DIR`, default `~/.pi/agent` [P1] | None | None |
| oh-my-pi (`omp`) | First of `.omp/AGENTS.md`, `.claude/CLAUDE.md`, `.agents/AGENTS.md`, `.agent/AGENTS.md`, `.gemini/GEMINI.md`, `.github/copilot-instructions.md`, `AGENTS.md`, `CLAUDE.md` [O1][O2] | First of `~/.omp/agent/AGENTS.md`, `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`, `~/.gemini/GEMINI.md`, `~/.config/opencode/AGENTS.md` [O1][O3] | `@path`, five hops [O1] | None |
| OpenCode (`opencode`, `opencode-v2`) | First of `AGENTS.md`, `CLAUDE.md` [N1] | `~/.config/opencode/AGENTS.md`, else `~/.claude/CLAUDE.md` [N1] | None ("doesn't automatically parse file references") [N1] | None |
| Hermes (`hermes`) | First of `.hermes.md`, `HERMES.md`, `AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules` [H1][H2] | None (`SOUL.md` sets identity) [H1] | None | Head and tail cut over the character cap, at least 20,000 [H1] |
| Cursor (`cursor`) | `AGENTS.md` and `CLAUDE.md` both apply [U1][U2] | None (user rules live in settings and `~/.cursor/rules`) [U3] | None | Suggests under 500 lines [U2] |
| Grok Build (`grok`, `grok-stream`) | Every one of `AGENTS.md`, `AGENT.md`, `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, `.claude/CLAUDE.local.md`; files Git ignores are skipped [G1][G2] | Every `*.md` in `$GROK_HOME/rules`, default `~/.grok/rules`; Ghost uses `rules/AGENTS.md` [G1][G2][G3] | None documented | None ("loaded in full") [G1] |
| Antigravity (`antigravity`, `agy-stream`) | `AGENTS.md`, `GEMINI.md`, `.agents/AGENTS.md`, `.agents/GEMINI.md`, cumulative [A1] | `~/.gemini/AGENTS.md`, `~/.gemini/GEMINI.md` [A1] | `@[label](path)` (not used by Ghost) [A1] | Cuts a file at 24,000 bytes [A1] |

All of them walk up to parent folders in some form; the page lists instruction files in parent folders
and in subfolders read-only (with which agents read them and when), and edits only the scope's own
files. Gemini CLI is not in the catalog; it reads `GEMINI.md` by default and `AGENTS.md` only when
configured [M1].

Sources (checked 2026-10-28):

- [C1] https://code.claude.com/docs/en/memory
- [C2] https://code.claude.com/docs/en/claude-directory
- [X1] https://learn.chatgpt.com/docs/agent-configuration/agents-md (developers.openai.com/codex/guides/agents-md redirects there)
- [X2] https://learn.chatgpt.com/docs/config-file/config-advanced and https://github.com/openai/codex/blob/main/codex-rs/core/src/agents_md.rs
- [P1] https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/configuration.md
- [P2] https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/resource-loader.ts
- [O1] https://omp.sh/docs/context-files
- [O2] https://github.com/can1357/oh-my-pi/tree/main/packages/coding-agent/src/discovery
- [O3] https://omp.sh/docs/env
- [N1] https://opencode.ai/docs/rules/
- [H1] https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/context-files.md
- [H2] https://github.com/NousResearch/hermes-agent/blob/main/agent/prompt_builder.py
- [U1] https://cursor.com/docs/cli/using
- [U2] https://cursor.com/docs/rules
- [U3] https://cursor.com/help/customization/rules
- [G1] https://docs.x.ai/build/features/project-rules
- [G2] https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/12-project-rules.md
- [G3] https://docs.x.ai/build/settings/reference
- [A1] https://antigravity.google/docs/rules
- [M1] https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/gemini-md.md

Known gaps, not modelled: Claude Code's `instructionFiles` setting (`claude-md-and-agents-md`
loads both), Codex's `project_doc_fallback_filenames` and its rule that untrusted projects skip
project files, OpenCode's `instructions` config field, Cursor, Grok and Antigravity rule folders,
and whether Grok Build also reads home-level `~/.claude` sources (its guide mentions scanning them;
the page then shows only the import line there, which is harmless).

## Edge cases

Each case has defined behaviour and a test (`ghost-config` unless named otherwise).

| Case | Behaviour | Test |
|---|---|---|
| `CLAUDE.md` and `AGENTS.md` with different text | Per agent; switching to one file first asks which text to keep or to combine them; the replaced text is backed up; never a silent overwrite | `different_claude_and_agents_text_is_a_conflict_resolved_only_by_choice`; browser: conflict flow |
| `CLAUDE.md` that imports `AGENTS.md`, or links to it | Detected as one file; saving through a link writes its target | `a_claude_md_that_imports_or_links_to_agents_md_is_already_universal` (link part needs symlink privileges) |
| Case variants (`agents.md`) on Windows and macOS | Found, read, saved under the existing spelling; renamed only on request | `a_case_variant_is_read_kept_on_save_and_renamed_only_on_request` |
| Two names differing only by case side by side (case-sensitive file systems) | Read-only with a note | Not tested: Windows and macOS (the release targets) cannot hold both names in one folder |
| Edit outside Ghost between review and apply | Refused before any write; review again | `an_edit_outside_auracode_after_the_review_is_refused_before_any_write`, `apply::tests::*refuses*` |
| A managed copy edited by hand | "Edited copy" row; restore the source's text or adopt the edit everywhere; saving the source leaves it alone | `a_hand_edited_managed_copy_shows_drift_and_can_be_restored_or_adopted`; browser: drift flow |
| Read-only files | Shown with their text, not writable, with the reason | `read_only_files_are_shown_and_never_written` |
| Locked files and permission errors | The failing write stops, every changed file is put back, the message says the file may be locked or read-only | `a_failure_part_way_puts_back_every_file`, `apply::tests::a_failure_part_way_puts_every_changed_file_back` |
| Not UTF-8, BOM, CRLF, mixed endings | Non-UTF-8 is read-only (copies stay byte-identical); BOM and line endings are kept; mixed endings use the majority and say so | `bom_crlf_non_utf8_and_empty_files_keep_their_bytes`, `text::tests::*` |
| Empty files | Shown as empty; agents that skip empty files read nothing; saving empty text is allowed and says agents skip it | same |
| Large files | Per-agent warnings beside the text (Codex 32 KiB or the configured limit, Antigravity 24,000 bytes, Hermes 20,000 characters, Claude 200 and Cursor 500 lines); over 1 MiB is read-only | `large_files_warn_per_agent_and_oversized_ones_are_not_editable`, `a_configured_codex_limit_is_read_and_a_bad_config_falls_back_to_the_default` |
| Ignored files | Copied into new worktree chats by default (see [Worktree chats](#worktree-chats)) and noted so ("New worktree chats get a copy"); with the switch off, noted ("new worktree chats won't have it") with the offer to copy it through the folder's worktree setup; Grok Build skips ignored files | `untracked_and_ignored_files_are_flagged_for_worktrees_and_grok_skips_ignored`; ghost-core `ignored_instruction_files_reach_a_new_worktree_without_changing_it`, `new_worktree_chats_get_ignored_instruction_files_unless_turned_off`; desktop `copies ignored files into new worktree chats while the switch is on` |
| Untracked files | Noted ("new worktree chats won't have it"); offer to copy it through the folder's worktree setup or to commit it alone. Never copied automatically: in a worktree it would be a new change an agent could commit on the chat's branch | `untracked_and_ignored_files_are_flagged_for_worktrees_and_grok_skips_ignored`; ghost-core `facts_are_read_only_and_a_commit_takes_exactly_one_new_file`, `setup_settings_are_shared_by_folder...` |
| Nested files in subfolders, files in parent folders | Listed read-only with their readers; a parent `CLAUDE.md` changes what Claude Code reads and one-file mode adds the import it needs | `nested_and_parent_files_are_listed_and_a_parent_claude_md_changes_what_claude_reads` |
| Path traversal, unknown keys | Only known file keys are accepted | `unknown_keys_traversal_and_links_outside_the_folder_are_refused` |
| Links outside the folder, linked folders, dangling links | Classified by the OS's own resolution against the canonical folder; outside and dangling links are never followed or written | `link_classification_follows_the_resolved_target_never_the_link_text`, `a_relative_link_through_a_linked_folder_is_classified_by_where_the_os_resolves_it`, `a_dangling_link_reads_as_nothing_and_is_never_written`, `a_linked_agent_folder_pointing_outside_is_never_followed` |
| Hard links between instruction files | Detected; a save writes both as separate files with the same text | `hard_linked_files_are_detected_and_saved_as_separate_files` |
| An override hiding the shared file | Shown as "Override"; one-file mode offers its removal (backed up) and removes it only when ticked | `an_override_hides_the_shared_file_and_is_removed_only_by_explicit_choice` |
| A project with several folders | Each folder is a separate scope and record | `several_project_folders_are_separate_scopes_with_separate_records` |
| Agents not installed | Listed apart; no files are created for them | `global_universal_imports_for_claude_and_copies_for_agents_without_imports` |
| Chats already running | Every review and the page say changes apply to new sessions | review notes (`plan.rs`); browser checks |
| User-level files are real configuration | Read on opening the page, written only by a reviewed Apply; nothing else lands in agent folders | `global_universal_imports_for_claude_and_copies_for_agents_without_imports` |
| The app stops part way through an apply | The replaced bytes are already in the backup folder with a journal; staged `.NAME.auracode-*.tmp` and `.old` files may remain next to the file. Opening Instructions or applying a later change automatically recovers exact known states under the configuration gate. Before file commit it rolls back; after file commit it completes managed metadata only if the expected record still matches. External changes stay untouched, and unresolved overlapping writes are blocked. Startup and Instructions show the retained path | `apply::recovery::tests::*` (temporary homes only) |


Recovery journal version 2 records root resolution, file hashes, link text, rename spelling, and the
managed-record entry before and after the change. Durable markers distinguish backups ready, files
committed, managed metadata committed, and settled. Restart cleanup only removes the named staged
siblings when their bytes or link target still match. Missing roots, changed paths, damaged backups,
and malformed journals retain their evidence. Legacy version 1 journals roll back only when the
remaining evidence is exact; a legacy complete marker cannot prove managed metadata was saved and
therefore requires attention. Unrelated file writes remain available. A completely unreadable journal
has no trustworthy target map, so it is reported and retained without guessing which paths to block.
