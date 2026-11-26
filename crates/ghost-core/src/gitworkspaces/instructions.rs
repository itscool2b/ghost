//! Git facts for instruction files, and committing one new instruction file on request.
//! Reads never refresh or write the index; the commit stages and commits exactly one path.

use std::{collections::HashMap, ffi::OsString, path::Path};

use ghost_config::instructions::{GitFacts, GitState};

use super::{GitError, GitWorkspaces, Result, git};

/// Parses `git status --porcelain=v1 -z` entries for paths under `prefix` (repository relative).
fn parse_status(output: &[u8], prefix: &str) -> HashMap<String, GitState> {
    let mut states = HashMap::new();
    let mut entries = output.split(|byte| *byte == 0).peekable();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        // A rename or copy carries its source path in the next entry.
        if matches!(code[0], b'R' | b'C') {
            entries.next();
        }
        let Some(relative) = path.strip_prefix(prefix) else {
            continue;
        };
        let state = match code {
            b"??" => GitState::Untracked,
            b"!!" => GitState::Ignored,
            _ => GitState::Modified,
        };
        states.insert(relative.to_owned(), state);
    }
    states
}

impl GitWorkspaces {
    /// Tracked, modified, untracked or ignored, for each of `paths` that exists in `folder`.
    /// A folder outside any repository gives the default facts.
    pub async fn instruction_facts(&self, folder: &Path, paths: &[&str]) -> Result<GitFacts> {
        let git = self.git().await?;
        let prefix = git
            .output(folder, git::args(&["rev-parse", "--show-prefix"]), 4096)
            .await?;
        if prefix.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            return Ok(GitFacts::default());
        }
        let prefix = git::text(&prefix.stdout)?;
        let mut args: Vec<OsString> = git::args(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored=matching",
            "--untracked-files=all",
            "--no-renames",
            "--",
        ]);
        args.extend(paths.iter().map(OsString::from));
        let status = git.run(folder, args, 1024 * 1024).await?;
        let mut files = parse_status(&status, &prefix);
        // A folder that is ignored whole is listed once ("!! .claude/"), not file by file.
        let ignored_folders = files
            .iter()
            .filter(|(path, state)| path.ends_with('/') && **state == GitState::Ignored)
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        files.retain(|path, _| !path.ends_with('/'));
        for path in paths {
            if files.contains_key(*path) || !folder.join(path).is_file() {
                continue;
            }
            let state = if ignored_folders
                .iter()
                .any(|ignored| path.starts_with(ignored.as_str()))
            {
                GitState::Ignored
            } else {
                GitState::Tracked
            };
            files.insert((*path).to_owned(), state);
        }
        let inspection = super::inspect::inspect(&git, folder).await.ok();
        let worktree_branch = inspection
            .filter(|inspection| inspection.view.is_linked_worktree)
            .and_then(|inspection| inspection.view.branch);
        Ok(GitFacts {
            repository: true,
            files,
            worktree_branch,
        })
    }

    /// Stages and commits exactly one untracked instruction file. Other staged changes stay
    /// staged and out of the commit; a failed commit unstages the file again.
    pub async fn commit_instruction_file(&self, folder: &Path, path: &str) -> Result<String> {
        if !ghost_config::instructions::project_paths().contains(&path) {
            return Err(GitError::Message(
                "Only an instruction file can be committed here.".into(),
            ));
        }
        let git = self.git().await?;
        let _repository = self.hold_repository(&git, folder).await;
        let facts = self.instruction_facts(folder, &[path]).await?;
        if facts.files.get(path) != Some(&GitState::Untracked) {
            return Err(GitError::Message(format!(
                "{path} is not a new, untracked file, so Ghost leaves committing it to you."
            )));
        }
        git.run(folder, git::args(&["add", "--", path]), 4096)
            .await?;
        let message = format!("Add {path}");
        let committed = git
            .run(
                folder,
                git::args(&["commit", "--only", "--quiet", "-m", &message, "--", path]),
                64 * 1024,
            )
            .await;
        if committed.is_err() {
            // Put the index back as it was: the file was untracked before.
            let _ = git
                .run(
                    folder,
                    git::args(&["rm", "--cached", "--quiet", "--", path]),
                    4096,
                )
                .await;
            return Err(GitError::Message(format!(
                "Git could not commit {path}. Its author settings or signing may need your terminal. Nothing else was changed."
            )));
        }
        let head = git
            .run(folder, git::args(&["rev-parse", "--short", "HEAD"]), 256)
            .await?;
        git::text(&head)
    }

    /// Copies `source`'s instruction files that Git ignores into the new worktree at `worktree`,
    /// so its agents read what they read in the source folder. Files that are only untracked stay
    /// out: in a worktree they would be new changes an agent could commit on the chat's branch.
    /// Create-only, so a file already in the worktree (a setup copy, say) is kept. Returns the
    /// copied paths and, for each file that could not be copied, why.
    pub async fn copy_ignored_instructions(
        &self,
        source: &Path,
        worktree: &Path,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let paths = ghost_config::instructions::project_paths();
        let facts = self.instruction_facts(source, &paths).await?;
        let ignored = paths
            .into_iter()
            .filter(|path| facts.files.get(*path) == Some(&GitState::Ignored))
            .collect::<Vec<_>>();
        if ignored.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let ignored = ignored.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let (from, to) = (source.to_path_buf(), worktree.to_path_buf());
        let (copied, problems) = tokio::task::spawn_blocking(move || {
            super::setup_files::copy_known(&from, &to, &ignored)
        })
        .await
        .map_err(|_| GitError::Message("The instruction copy stopped unexpectedly.".into()))??;
        if copied.is_empty() {
            return Ok((copied, problems));
        }
        // A copy the worktree does not ignore (its rule is in a .gitignore change that is not
        // committed) would be a change on the chat's branch, so it comes back out. This copy
        // created it moments ago: create-only, nothing was there before.
        let paths = copied.iter().map(String::as_str).collect::<Vec<_>>();
        let facts = self.instruction_facts(worktree, &paths).await?;
        let (kept, stray): (Vec<_>, Vec<_>) = copied
            .into_iter()
            .partition(|path| facts.files.get(path) == Some(&GitState::Ignored));
        for path in stray {
            std::fs::remove_file(worktree.join(&path))?;
        }
        Ok((kept, problems))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn names(rig: &super::super::tests::Rig, args: &[&str]) -> Vec<String> {
        let output = rig
            .git
            .run(&rig.work, git::args(args), 64 * 1024)
            .await
            .unwrap();
        String::from_utf8_lossy(&output)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn facts_are_read_only_and_a_commit_takes_exactly_one_new_file() {
        let rig = super::super::tests::rig().await;
        let paths = ghost_config::instructions::project_paths();
        std::fs::write(rig.work.join("GEMINI.md"), b"Committed.\n").unwrap();
        rig.git
            .run(&rig.work, git::args(&["add", "--", "GEMINI.md"]), 4096)
            .await
            .unwrap();
        rig.git
            .run(
                &rig.work,
                git::args(&["commit", "-q", "-m", "gemini"]),
                4096,
            )
            .await
            .unwrap();
        std::fs::write(rig.work.join("GEMINI.md"), b"Changed.\n").unwrap();
        std::fs::write(rig.work.join(".gitignore"), b"CLAUDE.md\n").unwrap();
        std::fs::write(rig.work.join("CLAUDE.md"), b"Personal.\n").unwrap();
        std::fs::write(rig.work.join("AGENTS.md"), b"Shared.\n").unwrap();
        // Unrelated work the person staged must stay staged and out of the commit.
        std::fs::write(rig.work.join("other.txt"), b"Staged work.\n").unwrap();
        rig.git
            .run(&rig.work, git::args(&["add", "--", "other.txt"]), 4096)
            .await
            .unwrap();
        let facts = rig
            .service
            .instruction_facts(&rig.work, &paths)
            .await
            .unwrap();
        assert!(facts.repository);
        assert_eq!(facts.files.get("AGENTS.md"), Some(&GitState::Untracked));
        assert_eq!(facts.files.get("CLAUDE.md"), Some(&GitState::Ignored));
        assert_eq!(facts.files.get("GEMINI.md"), Some(&GitState::Modified));
        assert_eq!(facts.worktree_branch, None);
        assert!(
            rig.service
                .commit_instruction_file(&rig.work, "CLAUDE.md")
                .await
                .is_err()
        );
        assert!(
            rig.service
                .commit_instruction_file(&rig.work, "GEMINI.md")
                .await
                .is_err()
        );
        assert!(
            rig.service
                .commit_instruction_file(&rig.work, "other.txt")
                .await
                .is_err()
        );
        let head = rig
            .service
            .commit_instruction_file(&rig.work, "AGENTS.md")
            .await
            .unwrap();
        assert!(!head.is_empty());
        assert_eq!(
            names(&rig, &["show", "--name-only", "--format=", "HEAD"]).await,
            vec!["AGENTS.md"]
        );
        assert_eq!(
            names(&rig, &["diff", "--cached", "--name-only"]).await,
            vec!["other.txt"]
        );
        let after = rig
            .service
            .instruction_facts(&rig.work, &paths)
            .await
            .unwrap();
        assert_eq!(after.files.get("AGENTS.md"), Some(&GitState::Tracked));
        assert_eq!(after.files.get("GEMINI.md"), Some(&GitState::Modified));
    }

    #[tokio::test]
    async fn ignored_instruction_files_reach_a_new_worktree_without_changing_it() {
        let rig = super::super::tests::rig().await;
        std::fs::write(
            rig.work.join(".gitignore"),
            b"CLAUDE.md\nHERMES.md\n.claude/\n",
        )
        .unwrap();
        rig.git
            .run(&rig.work, git::args(&["add", "--", ".gitignore"]), 4096)
            .await
            .unwrap();
        rig.git
            .run(
                &rig.work,
                git::args(&["-c", "commit.gpgSign=false", "commit", "-q", "-m", "ignore"]),
                4096,
            )
            .await
            .unwrap();
        // GEMINI.md is ignored only by a rule that is not committed yet.
        std::fs::write(
            rig.work.join(".gitignore"),
            b"CLAUDE.md\nHERMES.md\n.claude/\nGEMINI.md\n",
        )
        .unwrap();
        std::fs::write(rig.work.join("CLAUDE.md"), b"Personal.\n").unwrap();
        std::fs::write(rig.work.join("HERMES.md"), b"Local.\n").unwrap();
        std::fs::create_dir(rig.work.join(".claude")).unwrap();
        std::fs::write(rig.work.join(".claude").join("CLAUDE.md"), b"Nested.\n").unwrap();
        std::fs::write(rig.work.join("GEMINI.md"), b"Rule not committed.\n").unwrap();
        std::fs::write(rig.work.join("AGENTS.md"), b"Untracked.\n").unwrap();
        let facts = rig
            .service
            .instruction_facts(&rig.work, &ghost_config::instructions::project_paths())
            .await
            .unwrap();
        assert_eq!(
            facts.files.get(".claude/CLAUDE.md"),
            Some(&GitState::Ignored),
            "a file in a folder ignored whole is ignored"
        );
        assert!(facts.files.keys().all(|path| !path.ends_with('/')));
        let worktree = super::super::tests::isolated(&rig).await;
        let worktree = Path::new(&worktree.path);
        std::fs::write(worktree.join("HERMES.md"), b"Already here.\n").unwrap();
        let (mut copied, problems) = rig
            .service
            .copy_ignored_instructions(&rig.work, worktree)
            .await
            .unwrap();
        copied.sort();
        assert_eq!(copied, [".claude/CLAUDE.md", "CLAUDE.md"]);
        assert!(problems.is_empty());
        assert_eq!(
            std::fs::read(worktree.join(".claude").join("CLAUDE.md")).unwrap(),
            b"Nested.\n"
        );
        assert_eq!(
            std::fs::read(worktree.join("HERMES.md")).unwrap(),
            b"Already here.\n",
            "an existing file is never overwritten"
        );
        for skipped in ["AGENTS.md", "GEMINI.md"] {
            assert!(
                !worktree.join(skipped).exists(),
                "{skipped} would become a change on the chat's branch"
            );
        }
        let view = super::super::inspect::inspect(&rig.git, worktree)
            .await
            .unwrap()
            .view;
        assert!(view.files.is_empty(), "the worktree shows no new changes");
    }

    #[tokio::test]
    async fn a_folder_outside_any_repository_has_no_git_facts() {
        let rig = super::super::tests::rig().await;
        let plain = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("AGENTS.md"), b"x").unwrap();
        let facts = rig
            .service
            .instruction_facts(plain.path(), &ghost_config::instructions::project_paths())
            .await
            .unwrap();
        assert_eq!(facts, GitFacts::default());
    }

    #[test]
    fn status_entries_map_to_instruction_states_under_the_folder() {
        let output = b"?? app/AGENTS.md\0!! app/CLAUDE.md\0 M app/GEMINI.md\0?? other/AGENTS.md\0R  app/new.md\0app/old.md\0";
        let states = parse_status(output, "app/");
        assert_eq!(states.get("AGENTS.md"), Some(&GitState::Untracked));
        assert_eq!(states.get("CLAUDE.md"), Some(&GitState::Ignored));
        assert_eq!(states.get("GEMINI.md"), Some(&GitState::Modified));
        assert!(!states.contains_key("other/AGENTS.md"));
        assert!(!states.contains_key("old.md"));
        assert_eq!(parse_status(b"", "").len(), 0);
    }
}
