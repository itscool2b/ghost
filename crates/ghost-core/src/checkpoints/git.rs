//! Bounded Git plumbing against an application-owned repository and index only.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ghost_process::{
    EnvResolver, HelperEnv, HelperSpec, LeftoverRegistry, ResolvedEnv, find_exe, run_helper,
    run_helper_input,
};

use super::{CheckpointError, Result};

/// Keep checkpoint batches within `ghost_process::run_helper_input`'s unlogged input limit.
pub(super) const INPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy)]
struct ObjectDirectory<'a> {
    path: &'a Path,
    batch_fsync: bool,
}

pub(super) struct Git {
    pub(super) program: PathBuf,
    env: Arc<ResolvedEnv>,
    registry: Arc<LeftoverRegistry>,
    empty: PathBuf,
    hooks: PathBuf,
    pub(super) protected: Vec<PathBuf>,
    helpers: AtomicU64,
}

impl Git {
    pub(super) async fn new(
        env: &EnvResolver,
        registry: Arc<LeftoverRegistry>,
        scratch: &Path,
    ) -> Result<Self> {
        let mut env = (*env.get().await).clone();
        let value = |name: &str| {
            env.vars
                .get(&ghost_process::EnvKey::from(name))
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = value("USERPROFILE").or_else(|| value("HOME"));
        let mut protected = Vec::new();
        for (variable, default) in [("CLAUDE_CONFIG_DIR", ".claude"), ("CODEX_HOME", ".codex")] {
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
        // Do not inherit alternate object directories, worktrees, indexes, config injections,
        // tracing paths, external diffs, or a credential askpass from Ghost's launch context.
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        let program = find_exe("git", &env.vars).ok_or_else(|| {
            CheckpointError::Message(
                "Git is required to save checkpoints. Install Git and refresh the environment."
                    .into(),
            )
        })?;
        let empty = scratch.join("empty-config");
        std::fs::write(&empty, [])?;
        let hooks = scratch.join("empty-template");
        std::fs::create_dir(&hooks)?;
        Ok(Self {
            program,
            env: Arc::new(env),
            registry,
            empty,
            hooks,
            protected,
            helpers: AtomicU64::new(0),
        })
    }

    pub(super) async fn init(&self, repo: &Path, cwd: &Path) -> Result<()> {
        self.run(
            repo,
            cwd,
            &repo.join("unused-index"),
            vec![
                "init".into(),
                "--bare".into(),
                format!("--template={}", self.hooks.display()).into(),
                "--".into(),
                repo.as_os_str().to_owned(),
            ],
            4096,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn run(
        &self,
        repo: &Path,
        cwd: &Path,
        index: &Path,
        args: Vec<OsString>,
        max_stdout: usize,
    ) -> Result<Vec<u8>> {
        self.run_inner(repo, cwd, index, cwd, args, None, max_stdout, None)
            .await
    }

    pub(super) fn helper_count(&self) -> u64 {
        self.helpers.load(Ordering::Relaxed)
    }

    pub(super) async fn run_input(
        &self,
        repo: &Path,
        worktree: &Path,
        index: &Path,
        args: Vec<OsString>,
        input: Vec<u8>,
        max_stdout: usize,
    ) -> Result<Vec<u8>> {
        self.run_input_at(repo, worktree, index, worktree, args, input, max_stdout)
            .await
    }

    /// Relative input names resolve in `cwd`; Git still uses only the explicit private repo,
    /// index and workspace. Numbered scratch files need no platform-dependent path quoting.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_input_at(
        &self,
        repo: &Path,
        worktree: &Path,
        index: &Path,
        cwd: &Path,
        args: Vec<OsString>,
        input: Vec<u8>,
        max_stdout: usize,
    ) -> Result<Vec<u8>> {
        if input.len() > INPUT_LIMIT {
            return Err(CheckpointError::Message(
                "Checkpoint Git input exceeded its safety limit.".into(),
            ));
        }
        self.run_inner(
            repo,
            worktree,
            index,
            cwd,
            args,
            Some(input),
            max_stdout,
            None,
        )
        .await
    }

    /// `add` opens an upstream object-database transaction. On supported volumes, Git fully
    /// flushes the batch before migrating its loose objects into the durable object directory.
    /// The staging repo, worktree and index are fresh private scratch paths, so no user or
    /// storage-repository attributes, filters, hooks or indexes participate in this operation.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn write_objects(
        &self,
        staging_repo: &Path,
        copies: &Path,
        batch_index: &Path,
        objects: &Path,
        input: Vec<u8>,
        max_stdout: usize,
        batch_fsync: bool,
    ) -> Result<Vec<u8>> {
        if input.len() > INPUT_LIMIT {
            return Err(CheckpointError::Message(
                "Checkpoint Git input exceeded its safety limit.".into(),
            ));
        }
        let directory = Some(ObjectDirectory {
            path: objects,
            batch_fsync,
        });
        self.run_inner(
            staging_repo,
            copies,
            batch_index,
            copies,
            vec![
                "add".into(),
                "--force".into(),
                "--pathspec-from-file=-".into(),
                "--pathspec-file-nul".into(),
            ],
            Some(input),
            4096,
            directory,
        )
        .await?;
        self.run_inner(
            staging_repo,
            copies,
            batch_index,
            copies,
            vec!["ls-files".into(), "--stage".into(), "-z".into()],
            None,
            max_stdout,
            directory,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_inner(
        &self,
        repo: &Path,
        worktree: &Path,
        index: &Path,
        cwd: &Path,
        args: Vec<OsString>,
        input: Option<Vec<u8>>,
        max_stdout: usize,
        object_directory: Option<ObjectDirectory<'_>>,
    ) -> Result<Vec<u8>> {
        let operation = args
            .first()
            .map(|arg| arg.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut timing = HelperTiming::new(&operation);
        self.helpers.fetch_add(1, Ordering::Relaxed);
        let mut command = vec![
            "--no-pager".into(),
            "--literal-pathspecs".into(),
            "-c".into(),
            format!("core.hooksPath={}", self.hooks.display()).into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "-c".into(),
            "core.autocrlf=false".into(),
            "-c".into(),
            "core.safecrlf=false".into(),
            "-c".into(),
            // Profile paths plus checkpoint refs can exceed Windows' legacy path limit.
            "core.longpaths=true".into(),
            "-c".into(),
            // Durable objects and refs outlive the operation; its scratch index does not.
            // Exclude index flushes even if platform defaults include them.
            "core.fsync=objects,reference,-index".into(),
            "-c".into(),
            if object_directory.is_some_and(|directory| directory.batch_fsync) {
                "core.fsyncMethod=batch".into()
            } else {
                "core.fsyncMethod=fsync".into()
            },
            "-c".into(),
            "core.quotePath=false".into(),
            "-c".into(),
            format!("core.excludesFile={}", self.empty.display()).into(),
            "-c".into(),
            format!("core.attributesFile={}", self.empty.display()).into(),
        ];
        if object_directory.is_some() {
            command.extend(["-c".into(), "core.bare=false".into()]);
        }
        command.extend(args);
        let mut set = vec![
            ("GIT_DIR".into(), repo.as_os_str().to_owned()),
            ("GIT_INDEX_FILE".into(), index.as_os_str().to_owned()),
            (
                "GIT_CONFIG_GLOBAL".into(),
                self.empty.as_os_str().to_owned(),
            ),
            (
                "GIT_CONFIG_SYSTEM".into(),
                self.empty.as_os_str().to_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_ATTR_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
            ("GIT_AUTHOR_NAME".into(), "auracode checkpoint".into()),
            ("GIT_AUTHOR_EMAIL".into(), "checkpoint@localhost".into()),
            ("GIT_COMMITTER_NAME".into(), "auracode checkpoint".into()),
            ("GIT_COMMITTER_EMAIL".into(), "checkpoint@localhost".into()),
        ];
        if operation != "init" {
            set.push(("GIT_WORK_TREE".into(), worktree.as_os_str().to_owned()));
        }
        if let Some(directory) = object_directory {
            set.push((
                "GIT_OBJECT_DIRECTORY".into(),
                directory.path.as_os_str().to_owned(),
            ));
        }
        let spec = HelperSpec {
            program: self.program.clone(),
            args: command,
            cwd: cwd.to_path_buf(),
            env: HelperEnv::Resolved(Arc::clone(&self.env)),
            set,
            new_session: true,
            timeout: Duration::from_secs(20),
            max_stdout,
        };
        let output = match input {
            Some(input) => run_helper_input(&self.registry, spec, input).await,
            None => run_helper(&self.registry, spec).await,
        };
        timing.outcome = "failed";
        let output = output.map_err(|error| {
            CheckpointError::Message(format!("Checkpoint Git helper could not start: {error}"))
        })?;
        if output.timed_out {
            return Err(CheckpointError::Message(
                "Checkpoint Git helper timed out.".into(),
            ));
        }
        if output.stdout_truncated {
            return Err(CheckpointError::Message(
                "Checkpoint output exceeded its safety limit.".into(),
            ));
        }
        if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            #[cfg(test)]
            eprintln!("fixture Git {operation}: {}", output.stderr_tail);
            // Git can echo arbitrary repository content. Do not put its unredacted stderr in UI.
            return Err(CheckpointError::Message(format!(
                "Checkpoint Git {operation} failed. The working folder and user index were left unchanged."
            )));
        }
        // `cat-file --batch` can exit successfully and print "<oid> missing" after a loose
        // object fails to inflate, while reporting the corruption only on stderr. A genuine
        // missing-object response is silent. Never turn diagnostics into a recoverable cache
        // miss, and never expose Git's potentially content-bearing stderr to the UI or logs.
        if operation == "cat-file" && !output.stderr_tail.is_empty() {
            return Err(CheckpointError::Message(
                "Checkpoint Git could not verify stored objects.".into(),
            ));
        }
        timing.outcome = "success";
        Ok(output.stdout)
    }
}

/// Dropping an interrupted helper records cancellation too, without logging paths or content.
struct HelperTiming<'a> {
    operation: &'a str,
    started: Instant,
    outcome: &'static str,
}

impl<'a> HelperTiming<'a> {
    fn new(operation: &'a str) -> Self {
        Self {
            operation,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
}

impl Drop for HelperTiming<'_> {
    fn drop(&mut self) {
        tracing::debug!(
            phase = "git_helper",
            operation = self.operation,
            elapsed_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            helper_count = 1,
            outcome = self.outcome,
            "checkpoint phase finished"
        );
    }
}

pub(super) fn oid(bytes: &[u8]) -> Result<String> {
    let value = std::str::from_utf8(bytes).unwrap_or("").trim();
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CheckpointError::Message(
            "Git returned an invalid checkpoint object ID.".into(),
        ));
    }
    Ok(value.to_owned())
}
