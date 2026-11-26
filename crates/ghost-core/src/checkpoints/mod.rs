//! Byte-preserving checkpoints in private Git objects inside the profile.
//! User Git state is never written.

#[cfg(test)]
mod benchmarks;
mod cache;
mod coordination;
mod durability;
mod git;
mod plumbing;
mod quarantine;
mod recovery;
mod revert;
pub(crate) mod scan;
pub(crate) use revert::apply_file;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ghost_process::{EnvResolver, LeftoverRegistry};
use ghost_protocol::changes::{
    ChangeDiff, ChangeKind, ChangedFile, ChangesSnapshot, CheckpointSummary,
};
use serde::{Deserialize, Serialize};

use crate::profile::Profile;
use scan::{Fingerprint, SavedFile};

pub type Result<T> = std::result::Result<T, CheckpointError>;

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("{0}")]
    Message(String),
    #[error("Checkpoint file operation failed: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Checkpoints {
    root: PathBuf,
    /// The profile's own worktrees folder. A chat in one of its worktrees is user code inside the
    /// otherwise protected profile folder ([`Checkpoints::protected_paths`]).
    worktrees: PathBuf,
    env: Arc<EnvResolver>,
    registry: Arc<LeftoverRegistry>,
    /// Disposable hints. Workspace leases protect mutation; this lock only exchanges entries.
    cache: std::sync::Mutex<cache::Cache>,
    previews: std::sync::Mutex<std::collections::HashMap<String, revert::Prepared>>,
}

impl std::fmt::Debug for Checkpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoints")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    chat_id: String,
    folder_id: String,
    root: PathBuf,
    commit: String,
    summary: CheckpointSummary,
    files: BTreeMap<String, SavedFile>,
}

struct Operation {
    scratch: Arc<tempfile::TempDir>,
    context: coordination::Context,
    git: git::Git,
    repo: PathBuf,
    index: PathBuf,
}

struct CaptureTiming<'a> {
    chat_id: &'a str,
    turn_id: &'a str,
    started: Instant,
    outcome: &'static str,
}

impl Drop for CaptureTiming<'_> {
    fn drop(&mut self) {
        tracing::info!(
            chat_id = self.chat_id,
            turn_id = self.turn_id,
            stage = "total",
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            outcome = self.outcome,
            "Checkpoint capture finished"
        );
    }
}

impl Checkpoints {
    #[cfg(test)]
    pub(crate) async fn hold_for_test(
        &self,
        folder_id: &str,
        folder: &Path,
    ) -> coordination::Owner {
        let root = std::fs::canonicalize(folder).expect("checkpoint test workspace");
        coordination::acquire(&self.root, folder_id, &root)
            .await
            .expect("checkpoint test lease")
    }
    pub fn new(
        profile: &Profile,
        env: Arc<EnvResolver>,
        registry: Arc<LeftoverRegistry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root: profile.checkpoints_dir(),
            worktrees: profile.worktrees_dir(),
            env,
            registry,
            cache: std::sync::Mutex::new(cache::Cache::default()),
            previews: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Settle interrupted file restoration before a prompt even when snapshots are disabled.
    /// With no interrupted restore, only root directory entries are checked. No Git, recursive
    /// workspace scan, content hashing or new checkpoint is involved. Actual recovery retains
    /// its content verification and preservation of the user's files.
    /// The shared lease is still required: a cancelled recovery worker may still be restoring
    /// files, and retains that lease until it exits.
    pub async fn prepare_workspace(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
    ) -> Result<()> {
        valid_id(chat_id)?;
        valid_id(folder_id)?;
        let root = std::fs::canonicalize(folder)?;
        if !root.is_dir() {
            return Err(CheckpointError::Message(
                "The chat folder is unavailable.".into(),
            ));
        }
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let context = owner.context();
        if std::fs::canonicalize(folder)? != root {
            return Err(CheckpointError::Message(
                "The chat folder changed while preparing it. Try again.".into(),
            ));
        }
        self.sweep_reverts(&root, chat_id, folder_id, &context)
            .await
    }

    /// Saves exact bytes before the native prompt is sent. No user index, filters or hooks run.
    pub async fn capture(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        turn_id: &str,
    ) -> Result<CheckpointSummary> {
        let mut timing = CaptureTiming {
            chat_id,
            turn_id,
            started: Instant::now(),
            outcome: "cancelled",
        };
        let result = self
            .capture_inner(chat_id, folder_id, folder, turn_id)
            .await;
        timing.outcome = if result.is_ok() { "success" } else { "failure" };
        result
    }

    async fn capture_inner(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        turn_id: &str,
    ) -> Result<CheckpointSummary> {
        let started = Instant::now();
        valid_id(chat_id)?;
        valid_id(folder_id)?;
        valid_id(turn_id)?;
        let root = std::fs::canonicalize(folder)?;
        if !root.is_dir() {
            return Err(CheckpointError::Message(
                "The checkpoint folder is unavailable.".into(),
            ));
        }
        let waiting = Instant::now();
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let context = owner.context();
        tracing::info!(
            chat_id,
            turn_id,
            stage = "wait",
            elapsed_ms = waiting.elapsed().as_millis() as u64,
            "Checkpoint stage"
        );
        if std::fs::canonicalize(folder)? != root {
            return Err(CheckpointError::Message(
                "The checkpoint folder changed while waiting. Try again.".into(),
            ));
        }
        // Removing the hint before any fallible work invalidates it on failure or cancellation.
        let previous = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take(folder_id, &root);
        let setup = Instant::now();
        let operation = self
            .operation(folder_id, &root, true, context.clone())
            .await?;
        tracing::info!(
            chat_id,
            turn_id,
            stage = "setup",
            elapsed_ms = setup.elapsed().as_millis() as u64,
            "Checkpoint stage"
        );
        let recovery = Instant::now();
        self.sweep_reverts(&root, chat_id, folder_id, &context)
            .await?;
        tracing::info!(
            chat_id,
            turn_id,
            stage = "recovery",
            elapsed_ms = recovery.elapsed().as_millis() as u64,
            "Checkpoint stage"
        );
        let discovery = Instant::now();
        let paths = self.paths(&operation, &root).await?;
        tracing::info!(
            chat_id,
            turn_id,
            stage = "discovery",
            elapsed_ms = discovery.elapsed().as_millis() as u64,
            file_count = paths.len(),
            "Checkpoint stage"
        );
        let scan_started = Instant::now();
        let mut scanned =
            Self::capture_scan(&operation, &root, &paths, previous.clone(), "blobs").await?;
        tracing::info!(
            chat_id,
            turn_id,
            stage = "scan",
            elapsed_ms = scan_started.elapsed().as_millis() as u64,
            file_count = scanned.files.len(),
            copied_count = scanned.copies.len(),
            "Checkpoint stage"
        );
        let mut retained = previous
            .as_ref()
            .map(|entry| cache::retained(entry, &scanned))
            .unwrap_or_default();
        let verifying = Instant::now();
        let mut reused_tree = None;
        let mut tree_inventory = None;
        let mut cache_missing = false;
        if let Some(previous) = &previous {
            let blobs_present = cache::verify_blobs(&operation, &root, &retained).await?;
            let unchanged = previous.files == retained && scanned.files.len() == retained.len();
            // Check every tree even when a blob is missing. Otherwise a cache miss could hide
            // a damaged existing tree and turn corruption into an apparently successful retry.
            let trees_present =
                !unchanged || cache::verify_tree(&operation, &root, previous).await?;
            if blobs_present && trees_present {
                if unchanged {
                    reused_tree = Some(previous.tree.clone());
                    tree_inventory = Some(Arc::clone(&previous.tree_ids));
                }
            } else {
                cache_missing = true;
            }
        }
        tracing::info!(
            chat_id,
            turn_id,
            stage = "verify",
            elapsed_ms = verifying.elapsed().as_millis() as u64,
            reused_count = retained.len(),
            cache_missing,
            "Checkpoint stage"
        );
        if cache_missing {
            // Exactly one cold retry for absent objects. Corruption has already returned an error.
            let rescan = Instant::now();
            let paths = self.paths(&operation, &root).await?;
            scanned = Self::capture_scan(&operation, &root, &paths, None, "retry-blobs").await?;
            retained.clear();
            tracing::info!(
                chat_id,
                turn_id,
                stage = "rescan",
                elapsed_ms = rescan.elapsed().as_millis() as u64,
                file_count = scanned.files.len(),
                "Checkpoint stage"
            );
        }
        let writing = Instant::now();
        let (files, tree) = match reused_tree {
            Some(tree) => (retained, tree),
            None => {
                let (files, tree) =
                    plumbing::write_tree(&operation, &root, &scanned, Some(&retained)).await?;
                tree_inventory = cache::inventory(&operation, &root, &files, &tree).await?;
                (files, tree)
            }
        };
        tracing::info!(
            chat_id,
            turn_id,
            stage = "write_tree",
            elapsed_ms = writing.elapsed().as_millis() as u64,
            file_count = files.len(),
            "Checkpoint stage"
        );
        context.check()?;
        let publishing = Instant::now();
        let commit = git::oid(
            &operation
                .git
                .run(
                    &operation.repo,
                    &root,
                    &operation.index,
                    vec![
                        "commit-tree".into(),
                        tree.clone().into(),
                        "-m".into(),
                        format!("auracode before-turn checkpoint {turn_id}").into(),
                    ],
                    128,
                )
                .await?,
        )?;
        context.check()?;
        let summary = CheckpointSummary {
            id: crate::ids::new_id(),
            turn_id: turn_id.into(),
            created_at: crate::time::now_millis(),
            file_count: files.len() as u32,
            total_bytes: files.values().map(|file| file.fingerprint.bytes).sum(),
            warnings: scanned.warnings,
        };
        let reference = format!("refs/aura/{chat_id}/{}", summary.id);
        operation
            .git
            .run(
                &operation.repo,
                &root,
                &operation.index,
                vec!["update-ref".into(), reference.into(), commit.clone().into()],
                4096,
            )
            .await?;
        context.check()?;
        let manifest = Manifest {
            version: 1,
            chat_id: chat_id.into(),
            folder_id: folder_id.into(),
            root: root.clone(),
            commit,
            summary: summary.clone(),
            files,
        };
        let directory = operation.repo.join("auracode");
        std::fs::create_dir_all(&directory)?;
        crate::fsutil::write_json_atomic(
            &directory.join(format!("{}.json", summary.id)),
            &manifest,
        )
        .map_err(|error| CheckpointError::Message(error.to_string()))?;
        context.check()?;
        // A valid capture with an unusually deep layout may exceed the optional cache budget.
        if let Some(tree_inventory) = tree_inventory {
            self.cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .publish(folder_id, &root, manifest.files, tree, tree_inventory);
        }
        tracing::info!(
            chat_id,
            turn_id,
            stage = "publish",
            elapsed_ms = publishing.elapsed().as_millis() as u64,
            "Checkpoint stage"
        );
        tracing::info!(
            chat_id,
            turn_id,
            stage = "complete",
            storage = "local",
            elapsed_ms = started.elapsed().as_millis() as u64,
            file_count = summary.file_count,
            total_bytes = summary.total_bytes,
            helper_count = operation.git.helper_count(),
            "Checkpoint stage"
        );
        Ok(summary)
    }

    async fn capture_scan(
        operation: &Operation,
        root: &Path,
        paths: &[String],
        previous: Option<Arc<cache::Entry>>,
        directory: &str,
    ) -> Result<scan::Scan> {
        operation.context.check()?;
        let scratch = Arc::clone(&operation.scratch);
        let copies = scratch.path().join(directory);
        std::fs::create_dir(&copies)?;
        let root = root.to_owned();
        let paths = paths.to_owned();
        let context = operation.context.clone();
        tokio::task::spawn_blocking(move || {
            let _scratch = scratch;
            scan::collect_with(
                &root,
                &paths,
                Some(&copies),
                previous.as_ref().map(|entry| &entry.files),
                Some(&context),
            )
        })
        .await
        .map_err(|_| CheckpointError::Message("Checkpoint scanning failed.".into()))?
    }
    /// Git's stat against private trees. It reads no user index, filter or credential file.
    pub async fn diff_stat(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
    ) -> Result<String> {
        let (_owner, manifest, operation) = self
            .checkpoint_operation(chat_id, folder_id, folder, checkpoint_id)
            .await?;
        let copies = operation.scratch.path().join("stat-blobs");
        std::fs::create_dir(&copies)?;
        let current = self
            .current_scan(&operation, &manifest, Some(copies))
            .await?;
        let retained = manifest
            .files
            .iter()
            .filter(|(path, _)| current.excluded.contains(*path))
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();
        let (_, tree) =
            plumbing::write_tree(&operation, &manifest.root, &current, Some(&retained)).await?;
        let bytes = operation
            .git
            .run(
                &operation.repo,
                &manifest.root,
                &operation.index,
                vec![
                    "diff-tree".into(),
                    "--stat".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-renames".into(),
                    "--no-commit-id".into(),
                    "-r".into(),
                    manifest.commit.into(),
                    tree.into(),
                ],
                16 * 1024,
            )
            .await?;
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    }

    pub async fn changes(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
    ) -> Result<ChangesSnapshot> {
        let (_owner, manifest, operation) = self
            .checkpoint_operation(chat_id, folder_id, folder, checkpoint_id)
            .await?;
        self.sweep_reverts(&manifest.root, chat_id, folder_id, &operation.context)
            .await?;
        let current = self.current(&operation, &manifest).await?;
        let files = changes(&manifest.files, &current.files)
            .into_iter()
            .filter(|file| !current.excluded.contains(&file.path))
            .collect();
        let mut summary = manifest.summary;
        summary.warnings.extend(current.warnings);
        Ok(ChangesSnapshot {
            checkpoint: Some(summary),
            files,
            unavailable_reason: None,
        })
    }

    pub async fn diff(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
        path: &str,
    ) -> Result<ChangeDiff> {
        scan::validate(path)?;
        let (_owner, manifest, operation) = self
            .checkpoint_operation(chat_id, folder_id, folder, checkpoint_id)
            .await?;
        let current = self.current(&operation, &manifest).await?;
        if current.excluded.contains(path)
            || (!manifest.files.contains_key(path) && !current.files.contains_key(path))
        {
            return Err(CheckpointError::Message(
                "This file is outside the checkpoint's reviewable files.".into(),
            ));
        }
        let before = match manifest.files.get(path) {
            Some(file) => Some(self.blob(&operation, &manifest.root, file).await?),
            None => None,
        };
        let root = manifest.root.clone();
        let relative = path.to_owned();
        let context = operation.context.clone();
        let after = tokio::task::spawn_blocking(move || {
            context.check()?;
            scan::read(&root, &relative)
        })
        .await
        .map_err(|_| CheckpointError::Message("Reading the changed file failed.".into()))??;
        let after_sha256 = after
            .as_ref()
            .map(|(fingerprint, _)| fingerprint.sha256.clone());
        let after = after.map(|(_, bytes)| bytes);
        let binary = before
            .as_ref()
            .into_iter()
            .chain(after.as_ref())
            .any(|bytes| bytes.contains(&0) || std::str::from_utf8(bytes).is_err());
        let truncated = before
            .as_ref()
            .into_iter()
            .chain(after.as_ref())
            .any(|bytes| bytes.len() > DIFF_TEXT_LIMIT);
        let text = |bytes: Option<Vec<u8>>| -> Option<String> {
            if binary {
                return None;
            }
            bytes.map(|bytes| {
                let mut value = String::from_utf8_lossy(&bytes).into_owned();
                let mut end = value.len().min(DIFF_TEXT_LIMIT);
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                value.truncate(end);
                value
            })
        };
        Ok(ChangeDiff {
            path: path.into(),
            before: text(before),
            after: text(after),
            binary,
            truncated,
            after_sha256,
        })
    }

    async fn checkpoint_operation(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
    ) -> Result<(coordination::Owner, Manifest, Operation)> {
        let root = std::fs::canonicalize(folder)?;
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let manifest = self.load(chat_id, folder_id, folder, checkpoint_id)?;
        if manifest.root != root {
            return Err(CheckpointError::Message(
                "The checkpoint folder changed while waiting. Refresh Changes.".into(),
            ));
        }
        let operation = self
            .operation(folder_id, &root, false, owner.context())
            .await?;
        Ok((owner, manifest, operation))
    }

    async fn operation(
        &self,
        folder_id: &str,
        folder: &Path,
        create: bool,
        context: coordination::Context,
    ) -> Result<Operation> {
        context.check()?;
        valid_id(folder_id)?;
        std::fs::create_dir_all(&self.root)?;
        let scratch = Arc::new(
            tempfile::Builder::new()
                .prefix("operation-")
                .tempdir_in(&self.root)?,
        );
        let git = git::Git::new(&self.env, Arc::clone(&self.registry), scratch.path()).await?;
        let repo = self.root.join(format!("{folder_id}.git"));
        #[cfg(windows)]
        if beyond_git_dir_limit(&repo) {
            return Err(CheckpointError::Message(GIT_DIR_TOO_LONG.into()));
        }
        if !repo.exists() {
            if !create {
                return Err(CheckpointError::Message(
                    "This checkpoint repository is missing.".into(),
                ));
            }
            git.init(&repo, folder).await?;
        }
        let index = scratch.path().join("index");
        Ok(Operation {
            scratch,
            context,
            git,
            repo,
            index,
        })
    }

    async fn paths(&self, operation: &Operation, root: &Path) -> Result<Vec<String>> {
        operation.context.check()?;
        let mut args = vec![
            "ls-files".into(),
            "--others".into(),
            "--exclude-standard".into(),
            "--exclude=.auracode-revert-*".into(),
            "--exclude=.claude".into(),
            "--exclude=.codex".into(),
            "--exclude=.pi".into(),
            "--exclude=.omp".into(),
            "--exclude=.grok".into(),
            "--exclude=.hermes".into(),
            "--exclude=.ssh".into(),
            "--exclude=.gnupg".into(),
            "--exclude=.aws".into(),
            "--exclude=.azure".into(),
            "--exclude=keychains".into(),
            "--exclude=credentials".into(),
            "--exclude=credentials.json".into(),
            "--exclude=auth.json".into(),
            "--exclude=auth.toml".into(),
            "--exclude=.npmrc".into(),
            "--exclude=.netrc".into(),
            "--exclude=id_rsa".into(),
            "--exclude=id_ed25519".into(),
            "--exclude=.env".into(),
            "--exclude=.env.*".into(),
            "--exclude=*.key".into(),
            "--exclude=*.pem".into(),
            "--exclude=*.p12".into(),
            "--exclude=*.keychain-db".into(),
            "-z".into(),
        ];
        for protected in self.protected_paths(operation, root)? {
            if let Ok(relative) = protected.strip_prefix(root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                // Git exclude patterns quote metacharacters from the actual directory name.
                let escaped = relative
                    .replace('\\', "\\\\")
                    .replace('*', "\\*")
                    .replace('?', "\\?")
                    .replace('[', "\\[");
                args.push(format!("--exclude=/{escaped}").into());
            }
        }
        let bytes = operation
            .git
            .run(
                &operation.repo,
                root,
                &operation.index,
                args,
                8 * 1024 * 1024,
            )
            .await?;
        scan::paths(&bytes)
    }

    /// Folders a checkpoint never saves or restores. A workspace inside one of them is refused,
    /// except the profile's own worktrees: Ghost creates them inside the profile folder for
    /// worktree chats, and nothing else of the profile lies within one.
    fn protected_paths(&self, operation: &Operation, root: &Path) -> Result<Vec<PathBuf>> {
        let profile = self.root.parent().filter(|_| !self.is_owned_worktree(root));
        let mut paths = Vec::new();
        for protected in operation
            .git
            .protected
            .iter()
            .map(PathBuf::as_path)
            .chain(profile)
        {
            let protected = if protected.is_absolute() {
                protected.to_path_buf()
            } else {
                root.join(protected)
            };
            let protected = std::fs::canonicalize(&protected).unwrap_or(protected);
            if root.starts_with(&protected) {
                return Err(CheckpointError::Message(
                    "Agent configuration and Ghost profile folders cannot be checkpoint workspaces."
                        .into(),
                ));
            }
            paths.push(protected);
        }
        Ok(paths)
    }

    /// Whether `root` (canonical) is a folder inside the profile's worktrees folder, not that
    /// folder itself, which holds the worktree records.
    fn is_owned_worktree(&self, root: &Path) -> bool {
        let worktrees =
            std::fs::canonicalize(&self.worktrees).unwrap_or_else(|_| self.worktrees.clone());
        root != worktrees && root.starts_with(&worktrees)
    }

    async fn current(&self, operation: &Operation, manifest: &Manifest) -> Result<scan::Scan> {
        self.current_scan(operation, manifest, None).await
    }

    async fn current_scan(
        &self,
        operation: &Operation,
        manifest: &Manifest,
        copies: Option<PathBuf>,
    ) -> Result<scan::Scan> {
        let mut paths = self.paths(operation, &manifest.root).await?;
        // A newly added ignore rule must not make a previously saved file appear deleted.
        paths.extend(manifest.files.keys().cloned());
        paths.sort();
        paths.dedup();
        let protected = self.protected_paths(operation, &manifest.root)?;
        let mut excluded = std::collections::BTreeSet::new();
        paths.retain(|path| {
            if protected
                .iter()
                .any(|prefix| manifest.root.join(path).starts_with(prefix))
            {
                excluded.insert(path.clone());
                false
            } else {
                true
            }
        });
        let root = manifest.root.clone();
        let context = operation.context.clone();
        let scratch = Arc::clone(&operation.scratch);
        tokio::task::spawn_blocking(move || {
            let _scratch = scratch;
            let mut current =
                scan::collect_with(&root, &paths, copies.as_deref(), None, Some(&context))?;
            current.excluded.extend(excluded);
            Ok(current)
        })
        .await
        .map_err(|_| CheckpointError::Message("Reading changed files failed.".into()))?
    }

    fn load(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
    ) -> Result<Manifest> {
        valid_id(chat_id)?;
        valid_id(folder_id)?;
        valid_id(checkpoint_id)?;
        let path = self
            .root
            .join(format!("{folder_id}.git/auracode/{checkpoint_id}.json"));
        let file = std::fs::File::open(path)?;
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(CheckpointError::Message(
                "Checkpoint metadata exceeds its limit.".into(),
            ));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| {
            CheckpointError::Message("This checkpoint metadata is damaged or unsupported.".into())
        })?;
        if manifest.version != 1
            || manifest.chat_id != chat_id
            || manifest.folder_id != folder_id
            || manifest.summary.id != checkpoint_id
            || manifest.root != std::fs::canonicalize(folder)?
        {
            return Err(CheckpointError::Message(
                "This checkpoint belongs to a different chat or folder.".into(),
            ));
        }
        for (path, file) in &manifest.files {
            scan::validate(path)?;
            git::oid(file.oid.as_bytes())?;
        }
        Ok(manifest)
    }

    async fn blob(&self, operation: &Operation, root: &Path, file: &SavedFile) -> Result<Vec<u8>> {
        operation.context.check()?;
        let bytes = operation
            .git
            .run(
                &operation.repo,
                root,
                &operation.index,
                vec!["cat-file".into(), "blob".into(), file.oid.clone().into()],
                scan::FILE_LIMIT,
            )
            .await?;
        use sha2::{Digest, Sha256};
        if bytes.len() != file.fingerprint.bytes as usize
            || crate::chatstore::rows::hex(&Sha256::digest(&bytes)) != file.fingerprint.sha256
        {
            return Err(CheckpointError::Message(
                "The checkpoint file failed its integrity check.".into(),
            ));
        }
        Ok(bytes)
    }
}

/// The most of each side a diff gives as text; a longer file's diff is marked truncated, and its
/// hunks cannot be reverted one by one.
pub(super) const DIFF_TEXT_LIMIT: usize = 128 * 1024;

fn valid_id(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(CheckpointError::Message(
            "Invalid checkpoint identity.".into(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
const GIT_DIR_TOO_LONG: &str = "Checkpoints are unavailable because Ghost's data folder path is too long for Git on Windows. Set GHOST_DATA_DIR to a shorter folder, then restart Ghost.";

/// Git refuses a `GIT_DIR` whose UTF-8 length exceeds `PATH_MAX - 40`, and Git for Windows
/// defines `PATH_MAX` as 260. That also keeps `git init`'s change into the new repository within
/// the current-directory limit. Refs and objects inside the repository may still exceed 260.
#[cfg(windows)]
fn beyond_git_dir_limit(repo: &Path) -> bool {
    repo.to_string_lossy().len() > 220
}

fn changes(
    before: &BTreeMap<String, SavedFile>,
    after: &BTreeMap<String, Fingerprint>,
) -> Vec<ChangedFile> {
    let mut paths: Vec<_> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let old = before.get(path).map(|file| &file.fingerprint);
            let new = after.get(path);
            if old == new {
                return None;
            }
            Some(ChangedFile {
                path: path.clone(),
                kind: match (old, new) {
                    (None, _) => ChangeKind::Added,
                    (_, None) => ChangeKind::Deleted,
                    _ => ChangeKind::Modified,
                },
                before_bytes: old.map(|file| file.bytes),
                after_bytes: new.map(|file| file.bytes),
                binary: old.into_iter().chain(new).any(|file| file.binary),
            })
        })
        .collect()
}
