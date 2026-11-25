//! Owned, bounded Git helpers. Read-only commands do not refresh the user's index.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use o7_process::{
    EnvKey, EnvResolver, HelperEnv, HelperOutput, HelperSpec, LeftoverRegistry, ResolvedEnv,
    find_exe, run_helper, run_helper_input,
};

use super::{GitError, Result};

pub(super) struct Git {
    pub(super) program: PathBuf,
    pub(super) protected: Vec<PathBuf>,
    env: Arc<ResolvedEnv>,
    registry: Arc<LeftoverRegistry>,
    hooks: PathBuf,
}

impl Git {
    pub(super) async fn new(
        env: &EnvResolver,
        registry: Arc<LeftoverRegistry>,
        hooks: PathBuf,
    ) -> Result<Self> {
        let mut env = (*env.get().await).clone();
        let value = |name: &str| {
            env.vars
                .get(&EnvKey::from(name))
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = value("USERPROFILE").or_else(|| value("HOME"));
        let mut protected = Vec::new();
        for (variable, default) in [
            ("CLAUDE_CONFIG_DIR", ".claude"),
            ("CODEX_HOME", ".codex"),
            ("PI_CODING_AGENT_DIR", ".pi/agent"),
        ] {
            if let Some(path) =
                value(variable).or_else(|| home.as_ref().map(|home| home.join(default)))
            {
                protected.push(path);
            }
        }
        if let Some(home) = home {
            for suffix in [
                ".claude.json",
                ".pi",
                ".omp",
                ".grok",
                ".config",
                ".local/share",
            ] {
                protected.push(home.join(suffix));
            }
        }
        // An inherited alternate index, worktree, configuration injection or trace destination
        // must never redirect an operation away from the folder the user reviewed.
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        env.vars.remove(&EnvKey::from("SSH_ASKPASS"));
        let program = find_exe("git", &env.vars).ok_or_else(|| {
            GitError::Message(
                "Install Git and refresh the environment before managing worktrees.".into(),
            )
        })?;
        Ok(Self {
            program,
            protected,
            env: Arc::new(env),
            registry,
            hooks,
        })
    }

    pub(super) async fn output(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        maximum: usize,
    ) -> Result<HelperOutput> {
        self.output_with(cwd, args, maximum, Extra::default()).await
    }

    /// As `output`, with variables such as a private `GIT_INDEX_FILE`, text on standard input,
    /// a longer wait for a network operation, or output cut at `maximum`.
    pub(super) async fn output_with(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        maximum: usize,
        extra: Extra,
    ) -> Result<HelperOutput> {
        let Extra {
            set: extra_set,
            input,
            timeout,
            allow_cut,
        } = extra;
        let mut command = vec![
            "--no-pager".into(),
            "--literal-pathspecs".into(),
            "-c".into(),
            format!("core.hooksPath={}", self.hooks.display()).into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "-c".into(),
            "core.quotePath=false".into(),
            "-c".into(),
            "core.fsync=all".into(),
            "-c".into(),
            "diff.autoRefreshIndex=false".into(),
        ];
        command.extend(args);
        let mut set: Vec<(OsString, OsString)> = vec![
            ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_ASKPASS".into(), OsString::new()),
            ("GCM_INTERACTIVE".into(), "never".into()),
        ];
        set.extend(extra_set);
        let spec = HelperSpec {
            program: self.program.clone(),
            args: command,
            cwd: cwd.into(),
            env: HelperEnv::Resolved(Arc::clone(&self.env)),
            set,
            new_session: true,
            timeout: timeout.unwrap_or(Duration::from_secs(30)),
            max_stdout: maximum,
        };
        let output = match input {
            Some(input) => run_helper_input(&self.registry, spec, input).await,
            None => run_helper(&self.registry, spec).await,
        }
        .map_err(|_| GitError::Message("The owned Git helper could not start.".into()))?;
        if output.timed_out {
            return Err(GitError::Message(
                "Git timed out. Review the worktree state before trying again.".into(),
            ));
        }
        if output.stdout_truncated && !allow_cut {
            return Err(GitError::Message(
                "This repository exceeds the bounded Git operation limit.".into(),
            ));
        }
        Ok(output)
    }

    pub(super) async fn run(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        maximum: usize,
    ) -> Result<Vec<u8>> {
        let output = self.output(cwd, args, maximum).await?;
        if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            #[cfg(test)]
            eprintln!("Owned Git fixture error: {}", output.stderr_tail);
            // Git may echo repository data or remote credentials. Never expose its raw stderr.
            return Err(GitError::Message("Git could not complete this operation. The recorded workspace is retained for review.".into()));
        }
        Ok(output.stdout)
    }
}

/// What one Git command adds to the owned defaults.
#[derive(Default)]
pub(super) struct Extra {
    pub(super) set: Vec<(OsString, OsString)>,
    pub(super) input: Option<Vec<u8>>,
    pub(super) timeout: Option<Duration>,
    /// Output longer than the maximum comes back cut, with `stdout_truncated` set, instead of
    /// being refused. For a bounded read of the first bytes of something.
    pub(super) allow_cut: bool,
}

/// The commit a full reference names. A missing reference or a non-commit is an error.
pub(super) async fn resolve_commit(git: &Git, root: &Path, reference: &str) -> Result<String> {
    let output = git
        .output(
            root,
            vec![
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("{reference}^{{commit}}").into(),
            ],
            256,
        )
        .await?;
    let value = text(&output.stdout)?;
    if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) || !oid(&value) {
        return Err(GitError::Message(
            "The branch does not exist or has no commit.".into(),
        ));
    }
    Ok(value)
}

/// One `git worktree list --porcelain` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Registration {
    pub(super) path: String,
    /// The full checked-out reference, such as `refs/heads/main`.
    pub(super) branch: Option<String>,
    pub(super) locked: Option<String>,
}

/// Every worktree registered in the repository family, including missing ones.
pub(super) async fn registrations(git: &Git, root: &Path) -> Result<Vec<Registration>> {
    let output = git
        .run(
            root,
            args(&["worktree", "list", "--porcelain"]),
            1024 * 1024,
        )
        .await?;
    Ok(parse_registrations(&String::from_utf8_lossy(&output)))
}

pub(super) fn parse_registrations(output: &str) -> Vec<Registration> {
    let mut entries = Vec::new();
    let mut current: Option<Registration> = None;
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.extend(current.take());
            current = Some(Registration {
                path: path.into(),
                branch: None,
                locked: None,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch = Some(branch.into());
            } else if line == "locked" {
                entry.locked = Some(String::new());
            } else if let Some(reason) = line.strip_prefix("locked ") {
                entry.locked = Some(reason.into());
            }
        }
    }
    entries.extend(current);
    entries
}

/// Compares a Git-reported path with an app path without touching the filesystem.
pub(super) fn same_path_text(left: &str, right: &Path) -> bool {
    let normalize = |value: &str| {
        let value = value.replace('\\', "/");
        let value = value.trim_end_matches('/').to_owned();
        if cfg!(any(windows, target_os = "macos")) {
            value.to_lowercase()
        } else {
            value
        }
    };
    normalize(left) == normalize(&path_argument(right).to_string_lossy())
}

pub(super) fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

pub(super) fn text(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        GitError::Message("Git returned text that cannot be represented by this app.".into())
    })?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

pub(super) fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Git for Windows treats extended-length path arguments as UNC paths. The app bounds worktree
/// paths separately, so regular drive/UNC spellings preserve the same canonical location.
pub(super) fn path_argument(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut parts = path.components();
        let base = match parts.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => {
                    Some(PathBuf::from(format!("{}:\\", char::from(drive))))
                }
                Prefix::VerbatimUNC(server, share) => {
                    let mut base = OsString::from("\\\\");
                    base.push(server);
                    base.push("\\");
                    base.push(share);
                    Some(PathBuf::from(base))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(mut base) = base {
            for part in parts {
                if !matches!(part, Component::RootDir) {
                    base.push(part.as_os_str());
                }
            }
            return base;
        }
    }
    path.to_path_buf()
}
