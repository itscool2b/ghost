//! Reverts use a one-use content-bound preview and retain original files for recovery.
//!
//! Selected originals and a recovery manifest reach private durable storage before mutation.
//! The transient same-volume swap verifies the actual renamed file before publishing replacement
//! bytes. Finalization copies and verifies any surviving swap objects before removing them.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use ghost_protocol::changes::{HunkRevert, LineHunk, RevertPreview, RevertResult};
use sha2::{Digest, Sha256};

use super::{
    CheckpointError, Checkpoints, DIFF_TEXT_LIMIT, Manifest, Result, changes, coordination, scan,
};
use scan::Fingerprint;

/// The name a revert's working folder takes at the root of the folder it reverts.
pub(super) const STAGING_PREFIX: &str = ".auracode-revert-";

pub(super) struct Prepared {
    manifest: Manifest,
    current: BTreeMap<String, Option<Fingerprint>>,
    /// A hunk revert's bytes, fixed by its preview against the content in `current`. The
    /// recovery journal still binds the path to the checkpoint's own fingerprint, so if the
    /// revert cannot finish, these bytes stay in the recovery copy rather than being discarded.
    partial: BTreeMap<String, (Fingerprint, Vec<u8>)>,
    expires: Instant,
}

impl Checkpoints {
    /// Empty selection is rejected. A caller must explicitly select the files it shows the user.
    pub async fn preview_revert(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
        mut paths: Vec<String>,
    ) -> Result<RevertPreview> {
        if paths.is_empty() || paths.len() > 20_000 {
            return Err(CheckpointError::Message(
                "Select at least one changed file to revert.".into(),
            ));
        }
        for path in &paths {
            scan::validate(path)?;
        }
        paths.sort();
        paths.dedup();
        let root = std::fs::canonicalize(folder)?;
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let context = owner.context();
        let manifest = self.load(chat_id, folder_id, folder, checkpoint_id)?;
        if manifest.root != root {
            return Err(CheckpointError::Message(
                "The checkpoint folder changed while waiting. Refresh Changes.".into(),
            ));
        }
        let operation = self
            .operation(folder_id, &manifest.root, false, context.clone())
            .await?;
        self.sweep_reverts(&manifest.root, chat_id, folder_id, &context)
            .await?;
        let current = self.current(&operation, &manifest).await?;
        let changed = changes(&manifest.files, &current.files);
        let mut files = Vec::new();
        let mut fingerprints = BTreeMap::new();
        for path in &paths {
            if current.excluded.contains(path) {
                return Err(CheckpointError::Message(format!(
                    "{path} cannot be safely inspected and cannot be reverted."
                )));
            }
            let change = changed
                .iter()
                .find(|file| &file.path == path)
                .ok_or_else(|| {
                    CheckpointError::Message(format!(
                        "{path} is no longer changed. Refresh Changes."
                    ))
                })?;
            files.push(change.clone());
            fingerprints.insert(path.clone(), current.files.get(path).cloned());
        }
        let token = crate::ids::new_id();
        let mut warnings = manifest.summary.warnings.clone();
        warnings.push("This restores the selected checkpoint bytes, including edits made outside the agent. Review each selected file before reverting.".into());
        self.hold(
            token.clone(),
            Prepared {
                manifest,
                current: fingerprints,
                partial: BTreeMap::new(),
                expires: Instant::now() + PREVIEW_LIFETIME,
            },
        )?;
        Ok(RevertPreview {
            token,
            checkpoint_id: checkpoint_id.into(),
            files,
            expires_at: crate::time::now_millis().saturating_add(10 * 60 * 1000),
            warnings,
        })
    }

    /// One modified text file's hunks go back to the checkpoint's lines; the rest of the file
    /// stays as it is. The hunks count lines of the text whose SHA-256 the selection names, which
    /// must still be the file's; the preview fixes the resulting bytes, so an edit after it makes
    /// the revert stale instead of mixing versions. The original is kept as for any revert.
    pub async fn preview_hunk_revert(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        checkpoint_id: &str,
        selection: HunkRevert,
    ) -> Result<RevertPreview> {
        let HunkRevert {
            path,
            after_sha256,
            hunks,
        } = selection;
        scan::validate(&path)?;
        if hunks.is_empty() || hunks.len() > 10_000 {
            return Err(CheckpointError::Message(
                "Select a change to revert.".into(),
            ));
        }
        let root = std::fs::canonicalize(folder)?;
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let context = owner.context();
        let manifest = self.load(chat_id, folder_id, folder, checkpoint_id)?;
        if manifest.root != root {
            return Err(CheckpointError::Message(
                "The checkpoint folder changed while waiting. Refresh Changes.".into(),
            ));
        }
        let operation = self
            .operation(folder_id, &manifest.root, false, context.clone())
            .await?;
        self.sweep_reverts(&manifest.root, chat_id, folder_id, &context)
            .await?;
        let current = self.current(&operation, &manifest).await?;
        let refresh = || {
            CheckpointError::Message(format!(
                "{path} changed since Changes showed it. Refresh Changes."
            ))
        };
        if current.excluded.contains(&path) {
            return Err(CheckpointError::Message(format!(
                "{path} cannot be safely inspected and cannot be reverted."
            )));
        }
        let (Some(saved), Some(scanned)) = (manifest.files.get(&path), current.files.get(&path))
        else {
            return Err(CheckpointError::Message(format!(
                "Only a file that exists before and after can be reverted a change at a time. Revert {path} as a whole."
            )));
        };
        if saved.fingerprint.binary || scanned.binary {
            return Err(CheckpointError::Message(format!(
                "{path} is not text. Revert it as a whole."
            )));
        }
        if scanned.sha256 != after_sha256 || saved.fingerprint == *scanned {
            return Err(refresh());
        }
        let before = self.blob(&operation, &manifest.root, saved).await?;
        let read_root = manifest.root.clone();
        let relative = path.clone();
        let read_context = operation.context.clone();
        let (fingerprint, after) = tokio::task::spawn_blocking(move || {
            read_context.check()?;
            scan::read(&read_root, &relative)
        })
        .await
        .map_err(|_| CheckpointError::Message("Reading the changed file failed.".into()))??
        .ok_or_else(refresh)?;
        if fingerprint != *scanned {
            return Err(refresh());
        }
        // Changes counts lines only in texts it gives whole.
        if before.len() > DIFF_TEXT_LIMIT || after.len() > DIFF_TEXT_LIMIT {
            return Err(CheckpointError::Message(format!(
                "{path} is too long to revert a change at a time. Revert it as a whole."
            )));
        }
        let bytes = revert_hunks(&after, &before, &hunks)?;
        if bytes == after {
            return Err(refresh());
        }
        let replacement = Fingerprint {
            sha256: crate::chatstore::rows::hex(&Sha256::digest(&bytes)),
            bytes: u32::try_from(bytes.len())
                .map_err(|_| CheckpointError::Message(format!("{path} is too large to revert.")))?,
            mode: fingerprint.mode,
            binary: false,
        };
        let change = changes(&manifest.files, &current.files)
            .into_iter()
            .find(|file| file.path == path)
            .ok_or_else(refresh)?;
        let token = crate::ids::new_id();
        self.hold(
            token.clone(),
            Prepared {
                manifest,
                current: BTreeMap::from([(path.clone(), Some(fingerprint))]),
                partial: BTreeMap::from([(path, (replacement, bytes))]),
                expires: Instant::now() + PREVIEW_LIFETIME,
            },
        )?;
        Ok(RevertPreview {
            token,
            checkpoint_id: checkpoint_id.into(),
            files: vec![change],
            expires_at: crate::time::now_millis().saturating_add(10 * 60 * 1000),
            warnings: Vec::new(),
        })
    }

    /// Keeps a preview for its revert; a bounded number wait at once.
    fn hold(&self, token: String, prepared: Prepared) -> Result<()> {
        let mut previews = self
            .previews
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        previews.retain(|_, preview| preview.expires > Instant::now());
        if previews.len() >= 32 {
            return Err(CheckpointError::Message(
                "Too many revert previews are open. Close an older preview or try again later."
                    .into(),
            ));
        }
        previews.insert(token, prepared);
        Ok(())
    }

    /// A token is consumed once, including on failure. The UI must refresh after any failure.
    pub async fn revert(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        token: &str,
    ) -> Result<RevertResult> {
        self.revert_guarded(chat_id, folder_id, folder, token, ())
            .await
    }

    pub(crate) async fn revert_guarded<G: Send + 'static>(
        &self,
        chat_id: &str,
        folder_id: &str,
        folder: &Path,
        token: &str,
        guard: G,
    ) -> Result<RevertResult> {
        super::valid_id(token)?;
        let root = std::fs::canonicalize(folder)?;
        let owner = coordination::acquire(&self.root, folder_id, &root).await?;
        let context = owner.context();
        let prepared = self
            .previews
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(token)
            .ok_or_else(|| {
                CheckpointError::Message(
                    "This revert preview expired or was already used. Refresh Changes.".into(),
                )
            })?;
        if prepared.expires <= Instant::now()
            || prepared.manifest.chat_id != chat_id
            || prepared.manifest.folder_id != folder_id
            || prepared.manifest.root != root
            || prepared.manifest.root != std::fs::canonicalize(folder)?
        {
            return Err(CheckpointError::Message(
                "This revert preview no longer matches the chat and folder.".into(),
            ));
        }
        let operation = self
            .operation(folder_id, &prepared.manifest.root, false, context.clone())
            .await?;
        let mut replacements = BTreeMap::new();
        for path in prepared.current.keys() {
            let bytes = match (
                prepared.partial.get(path),
                prepared.manifest.files.get(path),
            ) {
                // A hunk revert's bytes were fixed by its preview, against this very content.
                (Some(partial), _) => Some(partial.clone()),
                (None, Some(file)) => Some((
                    file.fingerprint.clone(),
                    self.blob(&operation, &prepared.manifest.root, file).await?,
                )),
                (None, None) => None,
            };
            replacements.insert(path.clone(), bytes);
        }
        let token = token.to_owned();
        let keep = self.recovery_store();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            // Keep both session admission and checkpoint ownership until apply/recovery ends,
            // even if the caller stops awaiting this blocking task.
            let _context = context;
            _context.check()?;
            apply(&prepared, &token, replacements, &keep)
        })
        .await
        .map_err(|_| {
            CheckpointError::Message(
                "The revert operation stopped unexpectedly. Check the folder's recovery directory."
                    .into(),
            )
        })?
    }
}

type Replacement = Option<(Fingerprint, Vec<u8>)>;

/// How long a revert preview's token stays usable.
const PREVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Lines of `bytes`, each with its own ending; the last may have none. They split as the app's
/// diff splits text, so line counts agree with the hunks Changes shows.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&bytes[start..=index]);
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

/// The current text with each hunk's lines replaced by the checkpoint's, every line keeping its
/// own ending. A hunk that reaches the end of both texts also ends the result as the checkpoint
/// ends, so reverting every hunk gives back the checkpoint's bytes exactly.
pub(super) fn revert_hunks(
    current: &[u8],
    checkpoint: &[u8],
    hunks: &[LineHunk],
) -> Result<Vec<u8>> {
    let new = lines(current);
    let old = lines(checkpoint);
    let invalid = || {
        CheckpointError::Message("The change no longer matches the file. Refresh Changes.".into())
    };
    let mut result = Vec::with_capacity(current.len().max(checkpoint.len()));
    let (mut next_old, mut next_new) = (0usize, 0usize);
    let mut at_end = false;
    for hunk in hunks {
        let (old_start, new_start) = (hunk.old_start as usize, hunk.new_start as usize);
        let old_end = old_start
            .checked_add(hunk.old_lines as usize)
            .ok_or_else(invalid)?;
        let new_end = new_start
            .checked_add(hunk.new_lines as usize)
            .ok_or_else(invalid)?;
        if (old_end == old_start && new_end == new_start)
            || old_start < next_old
            || new_start < next_new
            || old_end > old.len()
            || new_end > new.len()
        {
            return Err(invalid());
        }
        for line in &new[next_new..new_start] {
            result.extend_from_slice(line);
        }
        for line in &old[old_start..old_end] {
            result.extend_from_slice(line);
        }
        (next_old, next_new) = (old_end, new_end);
        at_end = old_end == old.len() && new_end == new.len();
    }
    for line in &new[next_new..] {
        result.extend_from_slice(line);
    }
    if at_end && !result.is_empty() {
        // The checkpoint's last line decides how the text ends.
        let ending: &[u8] = match old.last() {
            Some(line) if line.ends_with(b"\r\n") => b"\r\n",
            Some(line) if line.ends_with(b"\n") => b"\n",
            _ => b"",
        };
        if result.ends_with(b"\n") {
            result.pop();
            if result.ends_with(b"\r") {
                result.pop();
            }
        }
        result.extend_from_slice(ending);
    }
    Ok(result)
}

fn apply(
    prepared: &Prepared,
    token: &str,
    replacements: BTreeMap<String, Replacement>,
    keep: &super::recovery::Store,
) -> Result<RevertResult> {
    apply_with(prepared, token, replacements, keep, || {})
}

fn apply_with(
    prepared: &Prepared,
    token: &str,
    replacements: BTreeMap<String, Replacement>,
    keep: &super::recovery::Store,
    before_swap: impl FnOnce(),
) -> Result<RevertResult> {
    let root = &prepared.manifest.root;
    // Validate and save every selected original before the first working-folder mutation.
    let mut originals = BTreeMap::new();
    for (path, expected) in &prepared.current {
        let current = scan::read(root, path)?;
        if &current.as_ref().map(|(fingerprint, _)| fingerprint.clone()) != expected {
            return Err(stale(path));
        }
        originals.insert(path.clone(), current);
    }
    let mut result = RevertResult {
        applied: Vec::new(),
        error: None,
        recovery_paths: Vec::new(),
    };
    let operation = (|| -> Result<()> {
        let group = keep.group(token)?;
        result
            .recovery_paths
            .push(keep.path(token).to_string_lossy().into_owned());
        let journal = super::recovery::Journal::new(&prepared.manifest, token, &prepared.current);
        for (index, (_, original)) in originals.iter().enumerate() {
            if let Some((_, bytes)) = original {
                group.write(&format!("original-{index}"), bytes)?;
            }
        }
        let encoded = serde_json::to_vec(&journal).map_err(std::io::Error::other)?;
        group.write("recovery.json", &encoded)?;
        before_swap();
        let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        let backup_name = format!("{STAGING_PREFIX}{token}");
        let builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use cap_std::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        directory.create_dir_with(&backup_name, &builder)?;
        // Every error after creation, including journal writes and opening the swap, is finalized.
        let mutation = (|| -> Result<()> {
            let backup = directory.open_dir_nofollow(&backup_name)?;
            super::recovery::write_new(&backup, "recovery.json", &encoded)?;
            for (index, (path, expected)) in prepared.current.iter().enumerate() {
                apply_file(
                    &directory,
                    &backup,
                    root,
                    path,
                    index,
                    expected.as_ref(),
                    replacements.get(path).and_then(Option::as_ref),
                )?;
                result.applied.push(path.clone());
            }
            Ok(())
        })();
        if let Err(error) = mutation {
            super::recovery::error(&mut result, error);
        }
        super::recovery::settle(&directory, &backup_name, &journal, &group, &mut result);
        Ok(())
    })();
    if let Err(error) = operation {
        super::recovery::error(&mut result, error);
    }
    Ok(result)
}

pub(crate) fn apply_file(
    root_dir: &Dir,
    backup: &Dir,
    root: &Path,
    path: &str,
    index: usize,
    expected: Option<&Fingerprint>,
    replacement: Option<&(Fingerprint, Vec<u8>)>,
) -> Result<()> {
    scan::checked_path(root, path)?;
    let mut parent = root_dir.try_clone()?;
    if let Some(ancestors) = Path::new(path).parent() {
        for component in ancestors.components() {
            let name = component.as_os_str();
            if replacement.is_some() {
                match parent.create_dir(name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
            parent = parent.open_dir_nofollow(name)?;
        }
    }
    let name = Path::new(path).file_name().ok_or_else(|| stale(path))?;
    let original = format!("original-{index}");
    let restored = format!("replacement-{index}");
    if let Some((fingerprint, bytes)) = replacement {
        let mut file =
            backup.open_with(&restored, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(bytes)?;
        #[cfg(unix)]
        {
            use cap_std::fs::PermissionsExt;
            file.set_permissions(cap_std::fs::Permissions::from_mode(
                fingerprint.mode & 0o777,
            ))?;
        }
        #[cfg(not(unix))]
        let _ = fingerprint;
        file.sync_all()?;
    }
    if let Some(expected) = expected {
        // Move aside first, then inspect the object actually removed from the name. Newer work is
        // retained even if another editor changed the file between preview validation and rename.
        parent.rename(name, backup, &original)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let saved = backup.open_with(&original, &options)?.into_std();
        let meta = saved.metadata()?;
        let mut bytes = Vec::new();
        saved
            .take(scan::FILE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        let hash = crate::chatstore::rows::hex(&Sha256::digest(&bytes));
        #[cfg(unix)]
        let mode_changed = {
            use std::os::unix::fs::PermissionsExt;
            (meta.permissions().mode() & 0o111 != 0) != (expected.mode & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let mode_changed = false;
        if !meta.is_file()
            || hash != expected.sha256
            || bytes.len() != expected.bytes as usize
            || mode_changed
        {
            let _ = backup.hard_link(&original, &parent, name);
            return Err(stale(path));
        }
    }
    if replacement.is_some() {
        // Hard-link publication is atomic and refuses to replace a path another writer created.
        // The complete original and replacement remain in the recovery directory on any failure.
        if let Err(error) = backup.hard_link(&restored, &parent, name) {
            if expected.is_some() {
                let _ = backup.hard_link(&original, &parent, name);
            }
            return Err(error.into());
        }
    }
    Ok(())
}

fn stale(path: &str) -> CheckpointError {
    CheckpointError::Message(format!(
        "{path} changed after the preview. No newer file was overwritten; refresh Changes before retrying."
    ))
}

#[cfg(test)]
mod tests {
    use super::super::recovery::Store;
    use super::*;
    use ghost_protocol::changes::CheckpointSummary;

    fn prepared(root: &Path) -> Prepared {
        let current = ["a", "b"]
            .into_iter()
            .map(|path| {
                (
                    path.into(),
                    scan::read(root, path).unwrap().map(|(fp, _)| fp),
                )
            })
            .collect();
        Prepared {
            manifest: Manifest {
                version: 1,
                chat_id: crate::ids::new_id(),
                folder_id: crate::ids::new_id(),
                root: root.canonicalize().unwrap(),
                commit: String::new(),
                files: BTreeMap::new(),
                summary: CheckpointSummary {
                    id: crate::ids::new_id(),
                    turn_id: crate::ids::new_id(),
                    created_at: 0,
                    file_count: 0,
                    total_bytes: 0,
                    warnings: vec![],
                },
            },
            current,
            partial: BTreeMap::new(),
            expires: Instant::now() + Duration::from_secs(60),
        }
    }
    #[test]
    fn durable_originals_precede_mutation_for_success_and_partial_failure() {
        for partial in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("work");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("a"), "private original a").unwrap();
            std::fs::write(root.join("b"), "private original b").unwrap();
            let prepared = prepared(&root);
            let token = crate::ids::new_id();
            let store = Store::fixture(temp.path().join("durable"));
            // Removal uses the same verified swap as replacement publication.
            let result = apply_with(&prepared, &token, BTreeMap::new(), &store, || {
                let group = store.group(&token).unwrap();
                assert_eq!(group.read("original-0"), b"private original a");
                assert_eq!(group.read("original-1"), b"private original b");
                assert!(!group.read("recovery.json").is_empty());
                assert!(root.join("a").exists());
                if partial {
                    std::fs::write(root.join("b"), "private newer b").unwrap();
                }
            })
            .unwrap();
            assert_eq!(result.error.is_some(), partial, "{:?}", result.error);
            assert_eq!(result.applied.len(), if partial { 1 } else { 2 });
            assert!(!root.join(format!("{STAGING_PREFIX}{token}")).exists());
            if partial {
                assert_eq!(std::fs::read(root.join("b")).unwrap(), b"private newer b");
            }
            assert_eq!(
                result.recovery_paths,
                vec![store.path(&token).to_string_lossy().into_owned()]
            );
            let group = store.group(&token).unwrap();
            let catalog: serde_json::Value =
                serde_json::from_slice(&group.read("saved-recovery.json")).unwrap();
            let original = catalog
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["source"] == "original-1")
                .unwrap();
            assert_eq!(
                group.read(original["object"].as_str().unwrap()),
                if partial {
                    b"private newer b".as_slice()
                } else {
                    b"private original b".as_slice()
                }
            );
        }
    }

    #[test]
    fn failed_durable_write_stops_before_any_working_file_mutation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a"), "original a").unwrap();
        std::fs::write(temp.path().join("b"), "original b").unwrap();
        let prepared = prepared(temp.path());
        let token = crate::ids::new_id();
        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, "not a storage folder").unwrap();
        let store = Store::fixture(blocked);
        let result = apply(&prepared, &token, BTreeMap::new(), &store).unwrap();
        assert!(result.error.is_some());
        assert!(result.applied.is_empty());
        assert_eq!(std::fs::read(temp.path().join("a")).unwrap(), b"original a");
        assert_eq!(std::fs::read(temp.path().join("b")).unwrap(), b"original b");
        assert!(
            !temp
                .path()
                .join(format!("{STAGING_PREFIX}{token}"))
                .exists()
        );
    }

    #[test]
    fn existing_recovery_object_write_failure_never_mutates_work() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a"), "original a").unwrap();
        std::fs::write(root.join("b"), "original b").unwrap();
        let prepared = prepared(&root);
        let token = crate::ids::new_id();
        let store = Store::fixture(temp.path().join("durable"));
        let group = store.group(&token).unwrap();
        group
            .write("original-0", b"immutable different original")
            .unwrap();
        let outcome = apply(&prepared, &token, BTreeMap::new(), &store).unwrap();
        assert!(outcome.error.is_some());
        assert!(outcome.applied.is_empty());
        assert_eq!(group.read("original-0"), b"immutable different original");
        assert_eq!(std::fs::read(root.join("a")).unwrap(), b"original a");
        assert_eq!(std::fs::read(root.join("b")).unwrap(), b"original b");
        assert!(!root.join(format!("{STAGING_PREFIX}{token}")).exists());
    }

    #[test]
    fn concurrent_new_path_prevents_replacement_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a"), "original").unwrap();
        std::fs::write(root.join("b"), "replacement").unwrap();
        let replacement = scan::read(root, "b").unwrap().unwrap();
        std::fs::remove_file(root.join("b")).unwrap();
        let prepared = prepared(root);
        let token = crate::ids::new_id();
        let store = Store::fixture(root.join("durable"));
        let result = apply_with(
            &prepared,
            &token,
            BTreeMap::from([("b".into(), Some(replacement))]),
            &store,
            || {
                std::fs::write(root.join("b"), "concurrent writer").unwrap();
            },
        )
        .unwrap();
        assert!(result.error.is_some());
        assert_eq!(std::fs::read(root.join("b")).unwrap(), b"concurrent writer");
        assert!(!root.join(format!("{STAGING_PREFIX}{token}")).exists());
    }

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> LineHunk {
        LineHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        }
    }

    fn reverted(current: &str, checkpoint: &str, hunks: &[LineHunk]) -> String {
        String::from_utf8(revert_hunks(current.as_bytes(), checkpoint.as_bytes(), hunks).unwrap())
            .unwrap()
    }

    #[test]
    fn a_hunk_puts_back_the_checkpoint_lines_and_leaves_the_rest() {
        let checkpoint = "1\n2\n3\n4\n5\n";
        let current = "1\nX\n3\n4\nY\n";
        assert_eq!(
            reverted(current, checkpoint, &[hunk(4, 1, 4, 1)]),
            "1\nX\n3\n4\n5\n"
        );
        assert_eq!(
            reverted(current, checkpoint, &[hunk(1, 1, 1, 1)]),
            "1\n2\n3\n4\nY\n"
        );
        assert_eq!(
            reverted(current, checkpoint, &[hunk(1, 1, 1, 1), hunk(4, 1, 4, 1)]),
            checkpoint
        );
        // Added lines go, removed lines come back.
        assert_eq!(
            reverted("a\nb\nc\n", "a\nc\n", &[hunk(1, 0, 1, 1)]),
            "a\nc\n"
        );
        assert_eq!(
            reverted("a\nc\n", "a\nb\nc\n", &[hunk(1, 1, 1, 0)]),
            "a\nb\nc\n"
        );
        // A block with an unchanged line inside reverts as one.
        assert_eq!(
            reverted("a\nX\nm\nY\nz\n", "a\nb\nm\nc\nz\n", &[hunk(1, 3, 1, 3)]),
            "a\nb\nm\nc\nz\n"
        );
    }

    #[test]
    fn line_endings_stay_and_the_end_follows_the_checkpoint() {
        // Each line keeps its own ending.
        assert_eq!(
            reverted("a\r\nB\r\n", "a\r\nb\r\n", &[hunk(1, 1, 1, 1)]),
            "a\r\nb\r\n"
        );
        // A hunk at the end of both texts ends the result as the checkpoint ends.
        assert_eq!(reverted("a\nb\nc\n", "a\nb", &[hunk(2, 0, 2, 1)]), "a\nb");
        assert_eq!(reverted("a\nb\nc", "a\nb\n", &[hunk(2, 0, 2, 1)]), "a\nb\n");
        assert_eq!(
            reverted("a\nb\n", "a\nb\nc", &[hunk(2, 1, 2, 0)]),
            "a\nb\nc"
        );
        assert_eq!(
            reverted("a\r\nb\r\nc\r\n", "a\r\nb", &[hunk(2, 0, 2, 1)]),
            "a\r\nb"
        );
        // Every line removed, or every line new.
        assert_eq!(reverted("x\ny\n", "", &[hunk(0, 0, 0, 2)]), "");
        assert_eq!(reverted("", "x\ny", &[hunk(0, 2, 0, 0)]), "x\ny");
        // A hunk short of the end leaves the end alone.
        assert_eq!(reverted("A\nb", "a\nb", &[hunk(0, 1, 0, 1)]), "a\nb");
    }

    #[test]
    fn hunks_out_of_order_overlapping_empty_or_past_the_end_are_refused() {
        let refused = |hunks: &[LineHunk]| {
            revert_hunks(b"1\n2\n3\n", b"1\n2\n3\n4\n", hunks).expect_err("invalid hunks")
        };
        refused(&[hunk(1, 0, 1, 0)]);
        refused(&[hunk(3, 2, 3, 0)]);
        refused(&[hunk(1, 0, 3, 1)]);
        refused(&[hunk(2, 1, 2, 1), hunk(1, 1, 1, 1)]);
        refused(&[hunk(1, 2, 1, 1), hunk(2, 1, 2, 1)]);
        refused(&[hunk(u32::MAX, 1, 0, 1)]);
    }
}
