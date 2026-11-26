//! Ghost's worktrees, found on disk and checked with Git before anything is deleted.
//!
//! Ghost creates every worktree inside its data folder (`profiles/<id>/worktrees/<name>`, and
//! `worktrees/retired/<id>` once a chat is done), registered with the person's repository and
//! locked with the reason `Ghost <id>`. They are found by walking the data folder for a `.git`
//! file rather than through Ghost's records, so a profile whose records cannot be read (a
//! damaged one, or one left by a removed Ghost account) is covered too.
//!
//! A worktree has unsaved work when its folder holds changes Git has not committed (untracked
//! files included, ignored files not), or when its HEAD has commits no other branch, tag or remote
//! has. Deleting the folder loses the first for good; commits stay on the worktree's branch, which
//! is never deleted, but a detached HEAD's commits would be lost. A worktree Git cannot read counts
//! as unsaved, since nobody can say what is in it.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One Ghost worktree and what deleting its folder would lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The repository it belongs to: its main folder, or its Git folder when that has no
    /// working folder. `None` when Git could not tell (the repository is gone).
    pub repository: Option<PathBuf>,
    /// The branch checked out in it; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Changed, added, deleted and untracked files Git has not committed.
    pub changed_files: u32,
    /// Commits on HEAD that no other branch, tag or remote has.
    pub unique_commits: u32,
    /// Why Git could not check it.
    pub problem: Option<String>,
    /// The repository's Git folder, for the unlock and the prune.
    pub(super) common_dir: Option<PathBuf>,
}

impl Worktree {
    /// Whether deleting the folder could lose work, or nobody can tell.
    pub fn unsaved(&self) -> bool {
        self.changed_files > 0 || self.unique_commits > 0 || self.problem.is_some()
    }

    /// One plain line for the review: "3 changed files, 2 commits on no other branch".
    pub fn summary(&self) -> String {
        if let Some(problem) = &self.problem {
            return problem.clone();
        }
        let mut parts = Vec::new();
        if self.changed_files > 0 {
            parts.push(plural(
                self.changed_files,
                "uncommitted file",
                "uncommitted files",
            ));
        }
        if self.unique_commits > 0 {
            let commits = plural(self.unique_commits, "commit", "commits");
            parts.push(match &self.branch {
                Some(branch) => format!("{commits} only on {branch}"),
                None => format!("{commits} on no branch"),
            });
        }
        if parts.is_empty() {
            "Nothing unsaved".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

fn plural(count: u32, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Folders under a profile that never hold worktrees, skipped while looking.
const NOT_WORKTREES: &[&str] = &[
    "checkpoints",
    "attachments",
    "logs",
    "backups",
    "metadata",
    "config-apply",
    "config-managed",
    "config-transition",
    "config-transition-journals",
    "node_modules",
];

/// At most this deep below `profiles/`: `.account-removals/<token>/data/worktrees/retired/<id>`,
/// where an earlier version staged an account's removal.
const MAX_DEPTH: usize = 6;
/// A bound on the folders visited, so a strange tree cannot stall the plan.
const MAX_VISITS: usize = 20_000;

/// Ghost worktrees in the data folder `root`: folders under `profiles/` whose `.git` is a file
/// (a linked worktree), not followed through links.
pub(super) fn discover(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut visits = 0;
    let mut stack = vec![(root.join("profiles"), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        visits += 1;
        if visits > MAX_VISITS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() || is_link(&meta) {
                continue;
            }
            let name = entry.file_name();
            if NOT_WORKTREES.iter().any(|skip| OsStr::new(skip) == name) {
                continue;
            }
            if is_linked_worktree(&path) {
                found.push(path);
            } else if depth < MAX_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

fn is_linked_worktree(path: &Path) -> bool {
    let git = path.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&git) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let mut start = [0u8; 8];
    std::fs::File::open(&git)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok_and(|()| &start == b"gitdir: ")
}

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    meta.file_type().is_symlink() || reparse
}

/// Git for the cleanup: read-only questions about Ghost's worktrees, and the unlock and prune
/// that drop their registration. No hook, filesystem monitor or inherited `GIT_*` variable runs
/// or redirects anything, and nothing touches the person's index or their main checkout.
#[derive(Debug, Clone)]
pub struct GitTool {
    program: Option<PathBuf>,
    timeout: Duration,
}

impl GitTool {
    /// Git found on `PATH`; without it every worktree counts as unchecked.
    pub fn from_path() -> GitTool {
        let program = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path).find_map(|dir| {
                let names: &[&str] = if cfg!(windows) {
                    &["git.exe"]
                } else {
                    &["git"]
                };
                names
                    .iter()
                    .map(|name| dir.join(name))
                    .find(|candidate| candidate.is_file())
            })
        });
        GitTool {
            program,
            timeout: Duration::from_secs(30),
        }
    }

    /// No Git at all (tests of the no-Git path).
    pub fn missing() -> GitTool {
        GitTool {
            program: None,
            timeout: Duration::from_secs(30),
        }
    }

    pub fn available(&self) -> bool {
        self.program.is_some()
    }

    fn run(&self, args: &[&OsStr]) -> Result<String, String> {
        let Some(program) = &self.program else {
            return Err("Git is not installed, so Ghost could not check this folder.".into());
        };
        let mut command = Command::new(program);
        command
            .args([
                "--no-pager",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.quotePath=false",
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0");
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
                && key != "GIT_OPTIONAL_LOCKS"
                && key != "GIT_TERMINAL_PROMPT"
            {
                command.env_remove(key);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window flashes up behind the uninstaller or the app.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = ghost_process::spawn_command(&mut command)
            .map_err(|error| format!("Git did not start: {error}"))?;
        // Both pipes drain on their own threads, so neither can fill up and stall Git.
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut bytes);
                }
                bytes
            })
        };
        let out = drain(
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        );
        let err = drain(
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        );
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Git did not answer in time.".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => return Err(format!("Git did not finish: {error}")),
            }
        };
        let out = out.join().unwrap_or_default();
        let err = err.join().unwrap_or_default();
        if status.success() {
            Ok(String::from_utf8_lossy(&out).into_owned())
        } else {
            let message = String::from_utf8_lossy(&err);
            let line = message
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            Err(line.trim().trim_start_matches("fatal: ").to_owned())
        }
    }

    fn at<'a>(folder: &'a Path, rest: &[&'a OsStr]) -> Vec<&'a OsStr> {
        let mut args = vec![OsStr::new("-C"), folder.as_os_str()];
        args.extend_from_slice(rest);
        args
    }

    /// Checks one worktree. Read-only: no index refresh, no lock taken.
    pub fn inspect(&self, path: &Path) -> Worktree {
        let mut worktree = Worktree {
            path: path.to_path_buf(),
            repository: None,
            branch: None,
            changed_files: 0,
            unique_commits: 0,
            problem: None,
            common_dir: None,
        };
        let common = self.run(&Self::at(
            path,
            &[
                OsStr::new("rev-parse"),
                OsStr::new("--path-format=absolute"),
                OsStr::new("--git-common-dir"),
            ],
        ));
        let common = match common {
            Ok(text) => PathBuf::from(text.trim()),
            Err(error) => {
                worktree.problem = Some(if self.available() {
                    format!("Git could not read this worktree ({error}).")
                } else {
                    error
                });
                return worktree;
            }
        };
        worktree.repository = Some(if common.file_name() == Some(OsStr::new(".git")) {
            common
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(common.clone())
        } else {
            common.clone()
        });
        worktree.common_dir = Some(common);
        match self.run(&Self::at(
            path,
            &[
                OsStr::new("status"),
                OsStr::new("--porcelain=v1"),
                OsStr::new("-z"),
                OsStr::new("--untracked-files=all"),
            ],
        )) {
            Ok(text) => worktree.changed_files = count_status(&text),
            Err(error) => {
                worktree.problem = Some(format!("Git could not list its changes ({error})."));
                return worktree;
            }
        }
        worktree.branch = self
            .run(&Self::at(
                path,
                &[
                    OsStr::new("symbolic-ref"),
                    OsStr::new("-q"),
                    OsStr::new("--short"),
                    OsStr::new("HEAD"),
                ],
            ))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|branch| !branch.is_empty());
        let has_head = self
            .run(&Self::at(
                path,
                &[
                    OsStr::new("rev-parse"),
                    OsStr::new("-q"),
                    OsStr::new("--verify"),
                    OsStr::new("HEAD^{commit}"),
                ],
            ))
            .is_ok();
        if has_head {
            let exclude = worktree
                .branch
                .as_deref()
                .map(|branch| format!("--exclude={}", escape_glob(branch)));
            let mut rest = vec![
                OsStr::new("rev-list"),
                OsStr::new("--count"),
                OsStr::new("HEAD"),
                OsStr::new("--not"),
            ];
            if let Some(exclude) = &exclude {
                rest.push(OsStr::new(exclude));
            }
            rest.extend([
                OsStr::new("--branches"),
                OsStr::new("--tags"),
                OsStr::new("--remotes"),
            ]);
            match self.run(&Self::at(path, &rest)) {
                Ok(text) => worktree.unique_commits = text.trim().parse().unwrap_or(0),
                Err(error) => {
                    worktree.problem =
                        Some(format!("Git could not compare its commits ({error})."));
                }
            }
        }
        worktree
    }

    /// Lifts Ghost's lock, so Git can prune the registration or the person can manage the
    /// worktree like any other. Failures are harmless: an unlocked worktree stays unlocked.
    pub(super) fn unlock(&self, worktree: &Worktree) {
        if let Some(common) = &worktree.common_dir {
            let _ = self.run(&[
                &git_dir(common),
                OsStr::new("worktree"),
                OsStr::new("unlock"),
                worktree.path.as_os_str(),
            ]);
        }
    }

    /// Drops the registrations of worktree folders that are gone (Git's own prune, which only
    /// ever touches registrations whose folder is missing and that are not locked). Returns a
    /// note when the registration could not be dropped.
    pub(super) fn prune(&self, worktree: &Worktree) -> Option<String> {
        let common = worktree.common_dir.as_ref()?;
        if !common.exists() {
            return None;
        }
        self.run(&[
            &git_dir(common),
            OsStr::new("worktree"),
            OsStr::new("prune"),
        ])
        .err()
        .map(|error| {
            format!(
                "{} still lists the deleted worktree {} ({error}). Run git worktree prune there.",
                worktree.repository.as_deref().unwrap_or(common).display(),
                worktree.path.display()
            )
        })
    }
}

fn git_dir(common: &Path) -> std::ffi::OsString {
    let mut argument = std::ffi::OsString::from("--git-dir=");
    argument.push(common.as_os_str());
    argument
}

/// Entries in `git status --porcelain=v1 -z`: a rename or copy carries its old path as one more
/// NUL-separated field.
fn count_status(text: &str) -> u32 {
    let mut count = 0;
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        count += 1;
        let code = field.get(..2).unwrap_or("");
        if code.contains('R') || code.contains('C') {
            fields.next();
        }
    }
    count
}

/// A branch name as a literal `--exclude` pattern.
fn escape_glob(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '*' | '?' | '[' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn status_counts_renames_once() {
        assert_eq!(count_status(""), 0);
        assert_eq!(count_status(" M a.txt\0?? b.txt\0"), 2);
        assert_eq!(count_status("R  new.txt\0old.txt\0 D gone.txt\0"), 2);
    }

    #[test]
    fn branch_names_are_literal_patterns() {
        assert_eq!(escape_glob("ghost/fix-login"), "ghost/fix-login");
        assert_eq!(escape_glob("a*b?[c]\\"), "a\\*b\\?\\[c]\\\\");
    }

    #[test]
    fn summaries_say_what_would_be_lost() {
        let mut worktree = Worktree {
            path: PathBuf::from("w"),
            repository: None,
            branch: Some("ghost/fix".into()),
            changed_files: 3,
            unique_commits: 1,
            problem: None,
            common_dir: None,
        };
        assert_eq!(
            worktree.summary(),
            "3 uncommitted files, 1 commit only on ghost/fix"
        );
        worktree.branch = None;
        worktree.changed_files = 1;
        worktree.unique_commits = 2;
        assert_eq!(
            worktree.summary(),
            "1 uncommitted file, 2 commits on no branch"
        );
        worktree.changed_files = 0;
        worktree.unique_commits = 0;
        assert!(!worktree.unsaved());
    }
}
