use ghost_protocol::git::{ManagedWorktree, WorktreeState};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize)]
struct Records {
    version: u32,
    entries: Vec<ManagedWorktree>,
}

/// Whether `current` is still the record a review saw. Its ports do not count: the registry gives
/// them out beside other work (a project terminal opening), which changes nothing reviewed.
pub(crate) fn unchanged_since_review(
    current: &ManagedWorktree,
    reviewed: &ManagedWorktree,
) -> bool {
    let without_port = |record: &ManagedWorktree| ManagedWorktree {
        port: None,
        ..record.clone()
    };
    without_port(current) == without_port(reviewed)
}

impl GitWorkspaces {
    /// The registry is scoped to this profile, including entries whose chat save rolled back.
    /// Listing does not require a live repository, so missing sources remain discoverable.
    pub async fn profile_records(&self) -> Result<Vec<ManagedWorktree>> {
        let mut records = self.read_records()?;
        for record in &mut records {
            if record.state == WorktreeState::Ready {
                match Path::new(&record.path).try_exists() {
                    Ok(false) => {
                        record.state = WorktreeState::Missing;
                        record.detail=Some("The retained folder is missing. Its branch and saved ownership remain available for review.".into());
                    }
                    Err(error) => {
                        record.detail = Some(format!(
                            "The retained folder could not be inspected: {error}"
                        ))
                    }
                    Ok(true) => {}
                }
            }
        }
        Ok(records)
    }
    pub async fn profile_record(&self, id: &str) -> Result<ManagedWorktree> {
        self.read_records()?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| GitError::Message("This profile does not own that worktree.".into()))
    }
    pub async fn profile_record_folder(&self, id: &str) -> Result<PathBuf> {
        let record = self.profile_record(id).await?;
        self.recovery_paths(&record)?;
        // Canonicalize and check the owned parent before opening any renderer-selected ID.
        let path = Path::new(&record.path).canonicalize()?;
        let root = self.root.canonicalize()?;
        if !path.starts_with(root) || !path.is_dir() {
            return Err(GitError::Message(
                "This retained folder is outside the profile's owned worktrees.".into(),
            ));
        }
        Ok(crate::fsutil::plain_windows_path(&path.to_string_lossy())
            .map(PathBuf::from)
            .unwrap_or(path))
    }
    pub(super) fn read_records(&self) -> Result<Vec<ManagedWorktree>> {
        let path = self.root.join("registry.json");
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file()||meta.file_type().is_symlink()||meta.len()>8*1024*1024=>return Err(GitError::Message("The worktree registry has an unsupported layout or size. Its contents were retained.".into())),
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),
            Err(error)=>return Err(error.into()),
            _=>{},
        }
        let bytes = std::fs::read(&path)?;
        let records:Records=serde_json::from_slice(&bytes).map_err(|_|GitError::Message("The worktree registry is damaged. Its contents were retained; use Git to inspect existing worktrees.".into()))?;
        if !(1..=2).contains(&records.version)
            || records
                .entries
                .iter()
                .any(|entry| entry.state == WorktreeState::Unknown)
        {
            return Err(GitError::Message(
                "This worktree registry belongs to a newer app version.".into(),
            ));
        }
        if records.entries.len() > 5000 {
            return Err(GitError::Message(
                "This profile exceeds the worktree registry limit.".into(),
            ));
        }
        Ok(records.entries)
    }

    /// Replaces the registry. Only [`Self::update_records`] and tests call it, so no write is
    /// lost to another made meanwhile.
    pub(super) fn write_records(&self, entries: Vec<ManagedWorktree>) -> Result<()> {
        let record = Records {
            version: 2,
            entries,
        };
        crate::fsutil::write_json_atomic(&self.root.join("registry.json"), &record)?;
        Ok(())
    }

    /// One read-modify-write of the registry under its short lock: `change` sees the records as
    /// they are now, and they are saved when it changed them. Never hold anything across an
    /// await here; a change worked out from an earlier read must be applied to this one.
    pub(super) fn update_records<T>(
        &self,
        change: impl FnOnce(&mut Vec<ManagedWorktree>) -> Result<T>,
    ) -> Result<T> {
        let _records = self.records_lock();
        let mut records = self.read_records()?;
        let before = records.clone();
        let value = change(&mut records)?;
        if records != before {
            self.write_records(records)?;
        }
        Ok(value)
    }

    pub async fn list(&self, chat_id: &str) -> Result<Vec<ManagedWorktree>> {
        let mut records = self.read_records()?;
        records.retain(|record| record.chat_id == chat_id);
        for record in &mut records {
            if record.state == WorktreeState::Ready && !Path::new(&record.path).try_exists()? {
                record.state = WorktreeState::Missing;
                record.detail=Some("This worktree was removed outside the app. Its branch and conversation remain available.".into());
            }
        }
        Ok(records)
    }
}
