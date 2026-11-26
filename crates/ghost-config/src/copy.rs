//! Explicit, one-use, content-bound copies. Journals retain replaced files, and publication uses
//! no-overwrite hard links so an intervening external writer cannot be silently overwritten.

use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};

use crate::{
    CopyFile, CopyPreview, CopyReceipt, CopyResult, CopyState, Error, ItemRef, Kind, Library,
    Provider, Result, Scope, Source, inventory, mcp, mcp_document,
    safe_fs::{self, FileData},
};

const JOURNALS: &str = ".auracode-library";

pub struct PreparedCopy {
    preview: CopyPreview,
    source_revision: String,
    source_root: PathBuf,
    target_root: PathBuf,
    target: Dir,
    expires: Instant,
    files: Vec<Mutation>,
    target_kind: Kind,
    target_prefix: String,
    mcp: Option<EntryPatch>,
}

impl PreparedCopy {
    pub fn preview(&self) -> &CopyPreview {
        &self.preview
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() > self.expires
    }
}

struct Mutation {
    path: String,
    before: Option<FileData>,
    after: FileData,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    source_id: String,
    kind: Kind,
    prefix: String,
    files: Vec<JournalFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp: Option<EntryPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restores: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct EntryPatch {
    key: String,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
}

struct Operation {
    id: String,
    kind: Kind,
    prefix: String,
    files: Vec<Mutation>,
    mcp: Option<EntryPatch>,
    restores: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct JournalFile {
    path: String,
    before: Option<String>,
    after: String,
}

fn fingerprint(file: &FileData) -> String {
    // The relative path belongs to the journal separately; the mode is part of its revision.
    let mut bytes = vec![u8::from(file.executable)];
    bytes.extend_from_slice(&file.bytes);
    safe_fs::hash(&bytes)
}

impl Library {
    /// Read only bounded, validated journal metadata. Never exposes retained file contents.
    pub fn copy_history(&self) -> Result<Vec<CopyReceipt>> {
        let mut receipts = Vec::new();
        for source in &self.sources {
            let Ok(root) = inventory::open(source) else {
                continue;
            };
            match root.symlink_metadata(JOURNALS) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(Error::Io),
                Ok(_) => {}
            }
            for id in safe_fs::children(&root, JOURNALS)? {
                if uuid::Uuid::parse_str(&id)
                    .ok()
                    .is_none_or(|uuid| uuid.to_string() != id)
                {
                    continue;
                }
                let backup = safe_fs::directory(&root, &format!("{JOURNALS}/{id}"))?;
                let Some(bytes) = safe_fs::read(&backup, "journal.json")? else {
                    continue;
                };
                let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
                // Several project providers share a folder. Only its original source owns a receipt.
                if journal.source_id != source.id {
                    continue;
                }
                validate_journal(source, &journal)?;
                let mixed_pending = journal.mcp.is_some() && mixed_payloads_remain(&backup)?;
                if journal.restores.is_some() && has_marker(&backup, "complete")? && !mixed_pending
                {
                    continue;
                }
                let state = if has_marker(&backup, "rolled-back")? {
                    CopyState::RolledBack
                } else if has_marker(&backup, "complete")?
                    && !has_marker(&backup, "rollback-started")?
                    && !mixed_pending
                {
                    CopyState::Complete
                } else {
                    CopyState::RecoveryRequired
                };
                receipts.push(CopyReceipt {
                    id,
                    source_id: source.id.clone(),
                    kind: journal.kind,
                    files: journal.files.into_iter().map(|file| file.path).collect(),
                    state,
                });
                if receipts.len() > safe_fs::ITEM_COUNT {
                    return Err(Error::Limit);
                }
            }
        }
        receipts.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(receipts)
    }

    pub fn prepare_copy(
        &self,
        from: &ItemRef,
        revision: &str,
        to_source: &str,
    ) -> Result<PreparedCopy> {
        let source = self
            .sources
            .iter()
            .find(|source| source.id == from.source_id)
            .ok_or(Error::Unavailable)?;
        let target = self
            .sources
            .iter()
            .find(|source| source.id == to_source)
            .ok_or(Error::Unavailable)?;
        let entry = self.entry(from)?;
        if entry.item.revision != revision {
            return Err(Error::Stale);
        }
        if let Some(reason) = entry.item.copy_blocked {
            return Err(Error::Unsupported(reason));
        }
        let prefix = destination(target, from.kind, &from.key, source.provider)?;
        let source_root = std::fs::canonicalize(&source.root).map_err(|_| Error::Unavailable)?;
        let target_root = std::fs::canonicalize(&target.root).map_err(|_| Error::Unavailable)?;
        if source_root == target_root && entry.item.relative_path == prefix {
            return Err(Error::Unsupported(
                "This item is already in that location.".into(),
            ));
        }
        let root = inventory::open(target)?;
        let mut files = Vec::new();
        let mut before_bytes = 0;
        let mut before_text = None;
        let mut after_text = entry.text;
        let mut patch = None;
        if from.kind == Kind::McpServer {
            // Reparse the source strictly so duplicate JSON keys cannot disguise the reviewed item.
            let source_bytes = safe_fs::read(&inventory::open(source)?, &entry.item.relative_path)?
                .ok_or(Error::Stale)?;
            let value = mcp_document::selected(&source_bytes, source.provider, &from.key)?
                .ok_or(Error::Stale)?;
            if safe_fs::hash(&serde_json::to_vec(&value).map_err(|_| Error::Format)?) != revision {
                return Err(Error::Stale);
            }
            let after = mcp::portable(&value, source.provider, target.provider)?;
            let before = safe_fs::read_file(&root, &prefix)?;
            let bytes = before
                .as_ref()
                .map_or(&[][..], |file| file.bytes.as_slice());
            let old = mcp_document::selected(bytes, target.provider, &from.key)?;
            if let Some(old) = &old {
                mcp::portable(old, target.provider, target.provider)?;
            }
            before_text = old
                .as_ref()
                .map(serde_json::to_string_pretty)
                .transpose()
                .map_err(|_| Error::Format)?;
            after_text = serde_json::to_string_pretty(&after).map_err(|_| Error::Format)?;
            let bytes = mcp_document::replace(bytes, target.provider, &from.key, Some(&after))?;
            let executable = before.as_ref().is_some_and(|file| file.executable);
            files.push(Mutation {
                path: prefix.clone(),
                before,
                after: FileData {
                    path: prefix.clone(),
                    bytes,
                    executable,
                },
            });
            patch = Some(EntryPatch {
                key: from.key.clone(),
                before: old,
                after: Some(after),
            });
        }
        for file in entry.files {
            let path = if from.kind == Kind::Skill {
                format!("{prefix}/{}", file.path)
            } else {
                prefix.clone()
            };
            let before = safe_fs::read_file(&root, &path)?;
            before_bytes += before.as_ref().map_or(0, |file| file.bytes.len());
            if before_bytes > safe_fs::TREE_LIMIT {
                return Err(Error::Limit);
            }
            if path.ends_with("/SKILL.md") || from.kind == Kind::Instructions {
                before_text = before
                    .as_ref()
                    .map(|file| inventory::text(&file.bytes).map(str::to_owned))
                    .transpose()?;
            }
            files.push(Mutation {
                path,
                before,
                after: file,
            });
        }
        if files.is_empty() {
            return Err(Error::Unsupported(
                "This item needs a vendor-specific installation workflow.".into(),
            ));
        }
        let mut preview = CopyPreview {
            id: uuid::Uuid::now_v7().to_string(), from: from.clone(), to_source: to_source.into(),
            files: files.iter().map(|file| CopyFile { path: file.path.clone(), before_revision: file.before.as_ref().map(fingerprint),
                after_revision: fingerprint(&file.after), bytes: file.after.bytes.len() as u32 }).collect(),
            before_text, after_text,
            notices: vec!["Copying changes the selected agent's configuration. Review instructions and scripts before the next agent session.".into(),
                "Existing unrelated files stay in place. Replaced files are retained for rollback; the Library does not run copied scripts.".into()],
        };
        if patch.is_some() {
            preview.notices = vec!["Only this server entry changes. Other servers, credentials and project settings stay in place.".into(),
                "The destination applies its own enable and approval rules. Source approvals are not copied.".into(),
                "Restore reverses this entry without replacing unrelated configuration changes.".into()];
        }
        Ok(PreparedCopy {
            preview,
            source_revision: revision.into(),
            source_root,
            target_root,
            target: root,
            expires: Instant::now() + Duration::from_secs(600),
            files,
            target_kind: from.kind,
            target_prefix: prefix,
            mcp: patch,
        })
    }

    /// Consumes the reviewed plan even if validation fails. The host must serialize apply/rollback
    /// and run this in an owned blocking task so an IPC cancellation cannot interrupt the journal.
    pub fn apply_copy(&self, prepared: PreparedCopy) -> Result<CopyResult> {
        let source = self
            .sources
            .iter()
            .find(|source| source.id == prepared.preview.from.source_id)
            .ok_or(Error::Unavailable)?;
        let target = self
            .sources
            .iter()
            .find(|source| source.id == prepared.preview.to_source)
            .ok_or(Error::Unavailable)?;
        if Instant::now() > prepared.expires
            || std::fs::canonicalize(&source.root).ok().as_ref() != Some(&prepared.source_root)
            || std::fs::canonicalize(&target.root).ok().as_ref() != Some(&prepared.target_root)
            || self.entry(&prepared.preview.from)?.item.revision != prepared.source_revision
        {
            return Err(Error::Stale);
        }
        Self::write_copy(
            target,
            prepared.target,
            Operation {
                id: prepared.preview.id,
                kind: prepared.target_kind,
                prefix: prepared.target_prefix,
                files: prepared.files,
                mcp: prepared.mcp,
                restores: None,
            },
        )
    }

    fn write_copy(target: &Source, root: Dir, operation: Operation) -> Result<CopyResult> {
        // Keep the handle opened for the preview. A path replacement after canonical validation
        // cannot redirect any write into a different root.
        for file in &operation.files {
            if safe_fs::read_file(&root, &file.path)?
                .as_ref()
                .map(fingerprint)
                != file.before.as_ref().map(fingerprint)
            {
                return Err(Error::Stale);
            }
        }
        let journals = create_directory(&root, JOURNALS)?;
        journals.create_dir(&operation.id)?;
        let backup = journals.open_dir_nofollow(&operation.id)?;
        let journal = Journal {
            version: if operation.mcp.is_some() { 2 } else { 1 },
            source_id: target.id.clone(),
            kind: operation.kind,
            prefix: operation.prefix,
            mcp: operation.mcp,
            restores: operation.restores,
            files: operation
                .files
                .iter()
                .map(|file| JournalFile {
                    path: file.path.clone(),
                    before: file.before.as_ref().map(fingerprint),
                    after: fingerprint(&file.after),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&journal).map_err(|_| Error::Format)?;
        stage(
            &backup,
            "journal.json",
            &FileData {
                path: String::new(),
                bytes,
                executable: false,
            },
        )?;
        sync_directory(&backup)?;
        sync_directory(&journals)?;
        // Publish recovery metadata before staging full documents, so interruption cannot strand
        // mixed configuration bytes in an undiscoverable folder. No live file changes yet.
        for (index, file) in operation.files.iter().enumerate() {
            stage(&backup, &format!("after-{index}"), &file.after)?;
            if let Some(before) = &file.before {
                // Keep a separate snapshot too: an editor holding an old handle can still change
                // the moved original. The reviewed bytes must remain recoverable in that case.
                stage(&backup, &format!("snapshot-{index}"), before)?;
            }
        }
        sync_directory(&backup)?;
        let mut result = CopyResult {
            id: operation.id,
            source_id: target.id.clone(),
            applied: Vec::new(),
            recovery_required: false,
            message: None,
        };
        for (index, file) in operation.files.iter().enumerate() {
            if let Err(error) = apply_file(&root, &backup, index, file) {
                result.recovery_required = true;
                result.message = Some(format!(
                    "Copy stopped: {error} Original and staged files remain in the Library recovery folder."
                ));
                break;
            }
            result.applied.push(file.path.clone());
        }
        if !result.recovery_required && marker(&backup, "complete").is_err() {
            result.recovery_required = true;
            result.message = Some("The files were copied, but the final recovery marker could not be saved. The retained originals remain available.".into());
        }
        if journal.mcp.is_some()
            && !result.recovery_required
            && cleanup_mixed_payloads(&backup, &journal).is_err()
        {
            result.recovery_required = true;
            result.message = Some("The server entry was saved, but temporary transaction files could not be removed. Copy history can finish recovery.".into());
        }
        Ok(result)
    }

    /// Rollback only accepts a UUID journal beneath a known source. It rechecks every affected file
    /// before the first mutation, preserving subsequent user edits and partial-copy originals.
    pub fn rollback_copy(&self, source_id: &str, id: &str) -> Result<CopyResult> {
        let target = self
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .ok_or(Error::Unavailable)?;
        if uuid::Uuid::parse_str(id)
            .ok()
            .is_none_or(|uuid| uuid.to_string() != id)
        {
            return Err(Error::UnsafePath);
        }
        let root = inventory::open(target)?;
        let backup = safe_fs::directory(&root, &format!("{JOURNALS}/{id}"))?;
        let journal: Journal =
            serde_json::from_slice(&safe_fs::read(&backup, "journal.json")?.ok_or(Error::Format)?)
                .map_err(|_| Error::Format)?;
        validate_journal(target, &journal)?;
        if journal.mcp.is_some() {
            return Self::rollback_mcp(target, root, backup, id, journal);
        }
        let complete = has_marker(&backup, "complete")?;
        let rollback_started = has_marker(&backup, "rollback-started")?;
        // A copy can stop before moving a file, after moving its old value, or after publication.
        // Inspect actual retained objects rather than trusting a last-written progress counter.
        for (index, file) in journal.files.iter().enumerate() {
            let current = safe_fs::read_file(&root, &file.path)?
                .as_ref()
                .map(fingerprint);
            if current == file.before {
                continue;
            }
            if current.is_none() && complete && !rollback_started {
                return Err(Error::Stale);
            }
            if current.as_ref().is_some_and(|hash| hash != &file.after) {
                return Err(Error::Stale);
            }
            if file.before.is_some()
                && safe_fs::read_file(&backup, &format!("before-{index}"))?
                    .as_ref()
                    .map(fingerprint)
                    != file.before
            {
                return Err(Error::Stale);
            }
        }
        let mut result = CopyResult {
            id: id.into(),
            source_id: source_id.into(),
            applied: Vec::new(),
            recovery_required: false,
            message: None,
        };
        marker(&backup, "rollback-started")?;
        for (index, file) in journal.files.iter().enumerate() {
            match rollback_file(&root, &backup, index, file) {
                Ok(()) => result.applied.push(file.path.clone()),
                Err(error) => {
                    result.recovery_required = true;
                    result.message = Some(format!(
                        "Rollback stopped: {error} All retained files remain available for recovery."
                    ));
                    break;
                }
            }
        }
        if !result.recovery_required && marker(&backup, "rolled-back").is_err() {
            result.recovery_required = true;
            result.message = Some("Files were restored, but the recovery status could not be saved. Retained files remain available.".into());
        }
        Ok(result)
    }

    fn rollback_mcp(
        target: &Source,
        root: Dir,
        backup: Dir,
        id: &str,
        journal: Journal,
    ) -> Result<CopyResult> {
        let patch = journal.mcp.as_ref().ok_or(Error::Format)?;
        let mut current = safe_fs::read_file(&root, &journal.prefix)?;
        if current.is_none() {
            // Recover the only missing-file window using the verified transient original. Never
            // interpret a later deletion of a completed copy as an interrupted publication.
            if has_marker(&backup, "complete")? {
                return Err(Error::Stale);
            }
            if journal.files[0].before.is_some() {
                let retained = safe_fs::read_file(&backup, "before-0")?.ok_or(Error::Stale)?;
                if Some(fingerprint(&retained)) != journal.files[0].before {
                    return Err(Error::Stale);
                }
                let (parent, name) = destination_parent(&root, &journal.prefix)?;
                backup.hard_link("before-0", &parent, &name)?;
                sync_directory(&parent)?;
                current = Some(retained);
            }
        }
        let current_bytes = current
            .as_ref()
            .map_or(&[][..], |file| file.bytes.as_slice());
        let value = mcp_document::selected(current_bytes, target.provider, &patch.key)?;
        if value == patch.before {
            cleanup_mixed_payloads(&backup, &journal)?;
            marker(&backup, "rolled-back")?;
            return Ok(CopyResult {
                id: id.into(),
                source_id: target.id.clone(),
                applied: vec![journal.prefix],
                recovery_required: false,
                message: None,
            });
        }
        if value != patch.after {
            return Err(Error::Stale);
        }
        let bytes = mcp_document::replace(
            current_bytes,
            target.provider,
            &patch.key,
            patch.before.as_ref(),
        )?;
        let executable = current.as_ref().is_some_and(|file| file.executable);
        marker(&backup, "rollback-started")?;
        let inverse = EntryPatch {
            key: patch.key.clone(),
            before: patch.after.clone(),
            after: patch.before.clone(),
        };
        let operation = Operation {
            id: uuid::Uuid::now_v7().to_string(),
            kind: Kind::McpServer,
            prefix: journal.prefix.clone(),
            mcp: Some(inverse),
            restores: Some(id.into()),
            files: vec![Mutation {
                path: journal.prefix.clone(),
                before: current,
                after: FileData {
                    path: journal.prefix.clone(),
                    bytes,
                    executable,
                },
            }],
        };
        let mut result = Self::write_copy(target, root, operation)?;
        if !result.recovery_required {
            // The inverse operation has its own discoverable journal if the process stops before
            // this parent marker. Completed inverse receipts are hidden to avoid duplicate history.
            if cleanup_mixed_payloads(&backup, &journal)
                .and_then(|()| marker(&backup, "rolled-back"))
                .is_err()
            {
                result.recovery_required = true;
                result.message = Some("The entry was restored, but recovery metadata needs another review. Temporary files were retained.".into());
            } else {
                result.id = id.into();
            }
        }
        Ok(result)
    }
}

fn destination(target: &Source, kind: Kind, key: &str, from: Provider) -> Result<String> {
    if kind == Kind::McpServer {
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(Error::Format);
        }
        return mcp_document::path(target).map(str::to_owned);
    }
    if target.claude_state_only {
        return Err(Error::Unsupported(
            "Choose an agent configuration folder for this item.".into(),
        ));
    }
    safe_fs::validate(key)?;
    match kind {
        Kind::Skill => {
            // The source key may identify a nested Pi/shared collection. Install the reviewed
            // bundle at the target's standard direct skill location, with normal overwrite review.
            let key = key.rsplit('/').next().ok_or(Error::UnsafePath)?;
            let base = match (target.provider, target.scope) {
                (Provider::Shared, _) => "",
                (_, Scope::User) => "skills/",
                (Provider::Claude, Scope::Project) => ".claude/skills/",
                (Provider::Codex, Scope::Project) => ".codex/skills/",
                (Provider::Pi, Scope::Project) => ".pi/skills/",
            };
            Ok(format!("{base}{key}"))
        }
        Kind::Instructions if target.provider != Provider::Shared => {
            if key == "AGENTS.override.md" && from != target.provider {
                return Err(Error::Unsupported("Override instructions have different precedence in another agent. Copy them manually after reviewing that behavior.".into()));
            }
            if !matches!(key, "CLAUDE.md" | "AGENTS.md" | "AGENTS.override.md") {
                return Err(Error::Unsupported(
                    "This instruction type needs a manual conversion.".into(),
                ));
            }
            Ok(if target.provider == Provider::Claude {
                "CLAUDE.md".into()
            } else if key == "AGENTS.override.md" {
                key.into()
            } else {
                "AGENTS.md".into()
            })
        }
        _ => Err(Error::Unsupported(
            "This item needs a vendor-specific installation workflow. No files were changed."
                .into(),
        )),
    }
}

fn validate_journal(target: &Source, journal: &Journal) -> Result<()> {
    if journal.version != if journal.mcp.is_some() { 2 } else { 1 }
        || journal.source_id != target.id
        || journal.files.is_empty()
        || journal.files.len() > safe_fs::FILE_COUNT
    {
        return Err(Error::Format);
    }
    if journal.restores.as_ref().is_some_and(|id| {
        uuid::Uuid::parse_str(id)
            .ok()
            .is_none_or(|parsed| parsed.to_string() != *id)
    }) {
        return Err(Error::Format);
    }
    if let Some(patch) = &journal.mcp {
        if journal.kind != Kind::McpServer
            || journal.files.len() != 1
            || destination(target, Kind::McpServer, &patch.key, target.provider)? != journal.prefix
            || journal.files[0].path != journal.prefix
        {
            return Err(Error::UnsafePath);
        }
        for value in [&patch.before, &patch.after].into_iter().flatten() {
            mcp::portable(value, target.provider, target.provider)?;
        }
    }
    let mut paths = std::collections::HashSet::new();
    for file in &journal.files {
        safe_fs::validate(&file.path)?;
        if !paths.insert(&file.path)
            || file.after.len() != 64
            || file.before.as_ref().is_some_and(|hash| hash.len() != 64)
        {
            return Err(Error::Format);
        }
        match journal.kind {
            Kind::McpServer if journal.mcp.is_some() => {}
            Kind::Instructions => {
                if journal.files.len() != 1
                    || file.path != journal.prefix
                    || !matches!(
                        file.path.as_str(),
                        "CLAUDE.md" | "AGENTS.md" | "AGENTS.override.md"
                    )
                {
                    return Err(Error::UnsafePath);
                }
            }
            Kind::Skill => {
                let key = journal.prefix.rsplit('/').next().ok_or(Error::UnsafePath)?;
                if destination(target, Kind::Skill, key, target.provider)? != journal.prefix
                    || !file.path.starts_with(&format!("{}/", journal.prefix))
                {
                    return Err(Error::UnsafePath);
                }
            }
            _ => return Err(Error::UnsafePath),
        }
    }
    Ok(())
}

fn mixed_payloads_remain(backup: &Dir) -> Result<bool> {
    for name in ["after-0", "before-0", "snapshot-0"] {
        match backup.symlink_metadata(name) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Io),
        }
    }
    Ok(false)
}

fn cleanup_mixed_payloads(backup: &Dir, journal: &Journal) -> Result<()> {
    // A writer may retain the old OS handle after our rename. Preserve any newer bytes it wrote.
    if let Some(original) = safe_fs::read_file(backup, "before-0")?
        && Some(fingerprint(&original)) != journal.files[0].before
    {
        return Err(Error::Stale);
    }
    for name in ["after-0", "before-0", "snapshot-0"] {
        match backup.remove_file(name) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    sync_directory(backup)
}

fn create_directory(root: &Dir, path: &str) -> Result<Dir> {
    let mut directory = root.try_clone()?;
    safe_fs::validate(path)?;
    for part in path.split('/') {
        match directory.create_dir(part) {
            Ok(()) => sync_directory(&directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        directory = directory
            .open_dir_nofollow(part)
            .map_err(|_| Error::UnsafePath)?;
    }
    Ok(directory)
}

fn destination_parent(root: &Dir, path: &str) -> Result<(Dir, String)> {
    safe_fs::validate(path)?;
    match path.rsplit_once('/') {
        Some((parent, name)) => Ok((create_directory(root, parent)?, name.into())),
        None => Ok((root.try_clone()?, path.into())),
    }
}

fn stage(root: &Dir, name: &str, data: &FileData) -> Result<()> {
    let mut file = root.open_with(name, OpenOptions::new().write(true).create_new(true))?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        file.set_permissions(cap_std::fs::Permissions::from_mode(if data.executable {
            0o700
        } else {
            0o600
        }))?;
    }
    file.write_all(&data.bytes)?;
    file.sync_all()?;
    Ok(())
}

fn apply_file(root: &Dir, backup: &Dir, index: usize, file: &Mutation) -> Result<()> {
    let (parent, name) = destination_parent(root, &file.path)?;
    let original = format!("before-{index}");
    if let Some(expected) = &file.before {
        parent.rename(&name, backup, &original)?;
        if safe_fs::read_file(backup, &original)?
            .as_ref()
            .map(fingerprint)
            != Some(fingerprint(expected))
        {
            let _ = backup.hard_link(&original, &parent, &name);
            return Err(Error::Stale);
        }
        sync_directory(backup)?;
    }
    if let Err(error) = backup.hard_link(format!("after-{index}"), &parent, &name) {
        if file.before.is_some() {
            let _ = backup.hard_link(&original, &parent, &name);
        }
        return Err(error.into());
    }
    sync_directory(&parent)?;
    Ok(())
}

fn rollback_file(root: &Dir, backup: &Dir, index: usize, file: &JournalFile) -> Result<()> {
    let current = safe_fs::read_file(root, &file.path)?
        .as_ref()
        .map(fingerprint);
    if current == file.before {
        return Ok(());
    }
    if current.as_ref().is_some_and(|hash| hash != &file.after) {
        return Err(Error::Stale);
    }
    let (parent, name) = destination_parent(root, &file.path)?;
    if current.is_some() {
        let removed = format!("rollback-{index}-{}", uuid::Uuid::now_v7());
        parent.rename(&name, backup, &removed)?;
        if safe_fs::read_file(backup, &removed)?
            .as_ref()
            .map(fingerprint)
            != Some(file.after.clone())
        {
            let _ = backup.hard_link(&removed, &parent, &name);
            return Err(Error::Stale);
        }
        sync_directory(backup)?;
    }
    if file.before.is_some() {
        backup.hard_link(format!("before-{index}"), &parent, &name)?;
    }
    sync_directory(&parent)?;
    Ok(())
}

fn sync_directory(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    crate::safe_fs::fsync_dir(directory)?;
    #[cfg(not(unix))]
    let _ = directory; // Windows syncs staged file handles; directory handles do not support FlushFileBuffers.
    Ok(())
}

fn has_marker(directory: &Dir, name: &str) -> Result<bool> {
    match safe_fs::read(directory, name)? {
        None => Ok(false),
        Some(bytes) if bytes == b"1" => Ok(true),
        Some(_) => Err(Error::Format),
    }
}

fn marker(directory: &Dir, name: &str) -> Result<()> {
    if !has_marker(directory, name)? {
        stage(
            directory,
            name,
            &FileData {
                path: String::new(),
                bytes: b"1".to_vec(),
                executable: false,
            },
        )?;
        sync_directory(directory)?;
    }
    Ok(())
}

impl Library {
    /// An agent's MCP server as one native `mcpServers` value for Ghost's shared MCP, which
    /// gives it to every agent. Reparsed strictly and checked against the revision the person
    /// reviewed, as a copy is; credentials in `env` and `headers` travel only to this device's
    /// local credential store (`mcp::shareable`).
    pub fn mcp_share(&self, reference: &ItemRef, revision: &str) -> Result<serde_json::Value> {
        if reference.kind != Kind::McpServer {
            return Err(Error::Unsupported(
                "Only MCP servers are shared with every agent.".into(),
            ));
        }
        let source = self
            .sources
            .iter()
            .find(|source| source.id == reference.source_id)
            .ok_or(Error::Unavailable)?;
        let entry = self.entry(reference)?;
        if entry.item.revision != revision {
            return Err(Error::Stale);
        }
        let bytes = safe_fs::read(&inventory::open(source)?, &entry.item.relative_path)?
            .ok_or(Error::Stale)?;
        let value =
            mcp_document::selected(&bytes, source.provider, &reference.key)?.ok_or(Error::Stale)?;
        if safe_fs::hash(&serde_json::to_vec(&value).map_err(|_| Error::Format)?) != revision {
            return Err(Error::Stale);
        }
        mcp::shareable(&value, source.provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_interruption_before_and_after_publication_recovers_without_retaining_private_settings() {
        for phase in 0..3 {
            let fixture = tempfile::tempdir().unwrap();
            let source = Source {
                id: "pi".into(),
                provider: Provider::Pi,
                scope: Scope::User,
                root: fixture.path().into(),
                claude_state_only: false,
            };
            let root = inventory::open(&source).unwrap();
            let before = FileData {
                path: "mcp.json".into(),
                bytes:
                    br#"{"private":"SYNTHETIC_PRIVATE","mcpServers":{"docs":{"command":"old"}}}"#
                        .to_vec(),
                executable: false,
            };
            let after = FileData {
                path: "mcp.json".into(),
                bytes:
                    br#"{"private":"SYNTHETIC_PRIVATE","mcpServers":{"docs":{"command":"new"}}}"#
                        .to_vec(),
                executable: false,
            };
            root.write("mcp.json", &before.bytes).unwrap();
            let id = uuid::Uuid::now_v7().to_string();
            let backup = create_directory(&root, &format!("{JOURNALS}/{id}")).unwrap();
            let journal = Journal {
                version: 2,
                source_id: "pi".into(),
                kind: Kind::McpServer,
                prefix: "mcp.json".into(),
                restores: None,
                mcp: Some(EntryPatch {
                    key: "docs".into(),
                    before: Some(serde_json::json!({"command":"old"})),
                    after: Some(serde_json::json!({"command":"new"})),
                }),
                files: vec![JournalFile {
                    path: "mcp.json".into(),
                    before: Some(fingerprint(&before)),
                    after: fingerprint(&after),
                }],
            };
            stage(
                &backup,
                "journal.json",
                &FileData {
                    path: String::new(),
                    bytes: serde_json::to_vec(&journal).unwrap(),
                    executable: false,
                },
            )
            .unwrap();
            stage(&backup, "after-0", &after).unwrap();
            stage(&backup, "snapshot-0", &before).unwrap();
            if phase >= 1 {
                root.rename("mcp.json", &backup, "before-0").unwrap();
            }
            if phase == 2 {
                backup.hard_link("after-0", &root, "mcp.json").unwrap();
            }
            let library = Library::new(vec![source]).unwrap();
            let restored = library.rollback_copy("pi", &id).unwrap();
            assert!(
                !restored.recovery_required,
                "phase {phase}: {:?}",
                restored.message
            );
            assert_eq!(
                mcp_document::selected(&root.read("mcp.json").unwrap(), Provider::Pi, "docs")
                    .unwrap(),
                Some(serde_json::json!({"command":"old"}))
            );
            assert!(!mixed_payloads_remain(&backup).unwrap());
            assert_eq!(
                library.copy_history().unwrap()[0].state,
                CopyState::RolledBack
            );
        }
    }

    #[test]
    fn changed_file_between_validation_and_move_is_retained_without_overwriting_it() {
        let fixture = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(fixture.path(), cap_std::ambient_authority()).unwrap();
        root.create_dir("backup").unwrap();
        let backup = root.open_dir_nofollow("backup").unwrap();
        root.write("AGENTS.md", b"original").unwrap();
        let before = safe_fs::read_file(&root, "AGENTS.md").unwrap();
        let after = FileData {
            path: "AGENTS.md".into(),
            bytes: b"replacement".to_vec(),
            executable: false,
        };
        stage(&backup, "after-0", &after).unwrap();
        root.write("AGENTS.md", b"concurrent user edit").unwrap();
        let file = Mutation {
            path: "AGENTS.md".into(),
            before,
            after,
        };
        assert!(matches!(
            apply_file(&root, &backup, 0, &file),
            Err(Error::Stale)
        ));
        assert_eq!(root.read("AGENTS.md").unwrap(), b"concurrent user edit");
        assert_eq!(backup.read("before-0").unwrap(), b"concurrent user edit");
    }

    #[test]
    fn publication_never_overwrites_a_new_file_created_after_validation() {
        let fixture = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(fixture.path(), cap_std::ambient_authority()).unwrap();
        root.create_dir("backup").unwrap();
        let backup = root.open_dir_nofollow("backup").unwrap();
        let after = FileData {
            path: "AGENTS.md".into(),
            bytes: b"replacement".to_vec(),
            executable: false,
        };
        stage(&backup, "after-0", &after).unwrap();
        root.write("AGENTS.md", b"concurrent creation").unwrap();
        assert!(
            apply_file(
                &root,
                &backup,
                0,
                &Mutation {
                    path: "AGENTS.md".into(),
                    before: None,
                    after
                }
            )
            .is_err()
        );
        assert_eq!(root.read("AGENTS.md").unwrap(), b"concurrent creation");
    }

    #[test]
    fn rollback_recovers_a_crash_after_moving_the_original_before_publication() {
        let fixture = tempfile::tempdir().unwrap();
        let source = Source {
            id: "pi".into(),
            provider: Provider::Pi,
            scope: Scope::User,
            root: fixture.path().into(),
            claude_state_only: false,
        };
        let library = Library::new(vec![source.clone()]).unwrap();
        let root = inventory::open(&source).unwrap();
        root.write("AGENTS.md", b"original").unwrap();
        let before = safe_fs::read_file(&root, "AGENTS.md").unwrap().unwrap();
        let after = FileData {
            path: "AGENTS.md".into(),
            bytes: b"replacement".to_vec(),
            executable: false,
        };
        let id = uuid::Uuid::now_v7().to_string();
        let backup = create_directory(&root, &format!("{JOURNALS}/{id}")).unwrap();
        stage(&backup, "after-0", &after).unwrap();
        let journal = Journal {
            version: 1,
            mcp: None,
            restores: None,
            source_id: "pi".into(),
            kind: Kind::Instructions,
            prefix: "AGENTS.md".into(),
            files: vec![JournalFile {
                path: "AGENTS.md".into(),
                before: Some(fingerprint(&before)),
                after: fingerprint(&after),
            }],
        };
        stage(
            &backup,
            "journal.json",
            &FileData {
                path: String::new(),
                bytes: serde_json::to_vec(&journal).unwrap(),
                executable: false,
            },
        )
        .unwrap();
        root.rename("AGENTS.md", &backup, "before-0").unwrap();
        let result = library.rollback_copy("pi", &id).unwrap();
        assert!(!result.recovery_required);
        assert_eq!(root.read("AGENTS.md").unwrap(), b"original");
    }
}
