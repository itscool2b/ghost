//! Explicit, bounded discovery and device-local folder mapping. Never fetches or changes Git.
use super::*;
use crate::{
    chatstore::{
        folder_resolution::{Command, Reply},
        organization::{Organization, OrganizationCommand},
    },
    gitworkspaces::FolderIdentity,
};
use ghost_protocol::{
    folder_resolution::{FolderLocationReview, FolderResolution},
    workspace::{FolderKind, ProjectFolder, WorkspaceFilter},
};
use sha2::{Digest, Sha256};

const MAX_ROOTS: usize = 16;
const MAX_ENTRIES: usize = 4096;
const MAX_REPOSITORIES: usize = 128;
const MAX_DEPTH: usize = 3;

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}
fn revision(folder: &ProjectFolder) -> Result<String, SessionError> {
    let value = serde_json::to_vec(&(
        &folder.id,
        &folder.project_id,
        &folder.kind,
        &folder.git_remote_url,
        &folder.path,
    ))
    .map_err(|_| invalid("The folder identity could not be checked."))?;
    Ok(format!("{:x}", Sha256::digest(value)))
}
fn folder_from(organization: &Organization, id: &str) -> Result<ProjectFolder, SessionError> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(invalid("The folder ID is invalid."));
    }
    organization
        .projects
        .iter()
        .flat_map(|project| &project.folders)
        .find(|folder| folder.id == id)
        .cloned()
        .ok_or_else(|| invalid("This project folder no longer exists."))
}

impl SessionManager {
    async fn resolution_organization(&self) -> Result<Organization, SessionError> {
        self.deps()
            .store
            .organize(OrganizationCommand::Snapshot(WorkspaceFilter {
                include_archived: true,
                project_id: None,
            }))
            .await
            .map_err(|error| storage_error(&error))
    }
    pub async fn folder_code_roots(&self) -> Result<Vec<String>, SessionError> {
        match self
            .deps()
            .store
            .folder_resolution(Command::Roots(None))
            .await
            .map_err(|error| storage_error(&error))?
        {
            Reply::Roots(roots) => Ok(roots),
            _ => Err(invalid("Code roots could not be read.")),
        }
    }
    pub async fn folder_set_code_roots(
        &self,
        roots: Vec<String>,
    ) -> Result<Vec<String>, SessionError> {
        if roots.len() > MAX_ROOTS {
            return Err(invalid("Choose at most 16 code roots."));
        }
        let mut checked = Vec::new();
        for root in roots {
            let path = checked_directory(&root)
                .await?
                .to_string_lossy()
                .into_owned();
            if !checked.contains(&path) {
                checked.push(path);
            }
        }
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        match self
            .deps()
            .store
            .folder_resolution(Command::Roots(Some(checked)))
            .await
            .map_err(|error| storage_error(&error))?
        {
            Reply::Roots(roots) => Ok(roots),
            _ => Err(invalid("Code roots could not be saved.")),
        }
    }
    pub async fn folder_resolution(
        &self,
        folder_id: &str,
        search: bool,
    ) -> Result<FolderResolution, SessionError> {
        let folder = folder_from(&self.resolution_organization().await?, folder_id)?;
        let roots = self.folder_code_roots().await?;
        let mut result = FolderResolution {
            folder,
            roots,
            matches: Vec::new(),
            truncated: false,
            notices: Vec::new(),
        };
        if result.folder.kind == FolderKind::Local {
            result.notices.push("This is a local-only folder from its original device. Copy its files here and Locate the copy, or publish the original repository before using Clone.".into());
            return Ok(result);
        }
        if !search {
            return Ok(result);
        }
        let remote=result.folder.git_remote_url.as_deref().ok_or_else(||invalid("The Git folder has no usable origin. Locate it after restoring its portable identity."))?;
        let git = self
            .deps()
            .worktrees
            .as_ref()
            .ok_or_else(|| invalid("Git inspection is unavailable on this device."))?;
        let roots = result.roots.iter().map(PathBuf::from).collect();
        let (candidates, truncated) = tokio::task::spawn_blocking(move || candidates(roots))
            .await
            .map_err(|_| invalid("Code root inspection could not finish."))?;
        result.truncated = truncated;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        for path in candidates {
            match tokio::time::timeout_at(deadline, git.folder_identity(&path)).await {
                Ok(Ok(FolderIdentity::Git { remote: found, .. })) if found == remote => {
                    result.matches.push(path.to_string_lossy().into_owned())
                }
                Ok(Err(_)) => result.truncated = true,
                Err(_) => {
                    result.truncated = true;
                    break;
                }
                _ => {}
            }
        }
        result.matches.sort();
        result.matches.dedup();
        result.notices.push("Search checks selected roots and three directory levels below them. It skips linked and hidden directories and never contacts a remote.".into());
        if result.truncated {
            result.notices.push("Some folders could not be checked or the search limit was reached. Add a more specific code root or Locate the repository directly.".into());
        }
        Ok(result)
    }
    async fn location_review(
        &self,
        folder: &ProjectFolder,
        path: &str,
    ) -> Result<FolderLocationReview, SessionError> {
        let path = checked_directory(path).await?;
        let mut notices=vec!["This changes only this device's folder mapping. It does not move files or start an agent.".into()];
        if folder.kind == FolderKind::Git {
            let git = self
                .deps()
                .worktrees
                .as_ref()
                .ok_or_else(|| invalid("Git inspection is unavailable on this device."))?;
            match git
                .folder_identity(&path)
                .await
                .map_err(|_| invalid("The selected repository's origin could not be verified."))?
            {
                FolderIdentity::Git { remote, .. }
                    if Some(&remote) == folder.git_remote_url.as_ref() => {}
                _ => {
                    return Err(invalid(
                        "The selected folder is not a repository root with this project's origin.",
                    ));
                }
            }
            notices.push("The repository origin matches. Local branches and uncommitted files may differ from the original device.".into());
        } else {
            notices.push("A local-only folder has no portable origin to verify. Confirm that this is the copy you want these chats to use.".into());
        }
        Ok(FolderLocationReview {
            folder_id: folder.id.clone(),
            path: path.to_string_lossy().into_owned(),
            revision: revision(folder)?,
            notices,
        })
    }
    pub async fn folder_locate_review(
        &self,
        folder_id: &str,
        path: &str,
    ) -> Result<FolderLocationReview, SessionError> {
        let folder = folder_from(&self.resolution_organization().await?, folder_id)?;
        self.location_review(&folder, path).await
    }
    pub async fn folder_locate_apply(
        &self,
        review: FolderLocationReview,
    ) -> Result<ProjectFolder, SessionError> {
        let manager = self.clone();
        self.deps()
            .rt
            .spawn(async move { manager.apply_location(review).await })
            .await
            .map_err(|_| {
                invalid(
                    "Folder mapping could not finish. Reopen the app to read its saved location.",
                )
            })?
    }

    async fn apply_location(
        &self,
        review: FolderLocationReview,
    ) -> Result<ProjectFolder, SessionError> {
        // Locating a folder can move any chat's folder, so it waits for every change in flight.
        let _organization = self.shared().organization.exclusive().await;
        let _idle = self.try_quiesce_when_idle().ok_or_else(|| {
            invalid(
                "Stop running chats and wait for owned tasks to finish before locating a folder.",
            )
        })?;
        let organization = self.resolution_organization().await?;
        let folder = folder_from(&organization, &review.folder_id)?;
        if revision(&folder)? != review.revision {
            return Err(invalid("The folder changed. Review its location again."));
        }
        let chats: Vec<_> = organization
            .chats
            .iter()
            .filter(|chat| chat.folder_id.as_deref() == Some(&folder.id))
            .map(|chat| chat.chat.id.clone())
            .collect();
        if chats.len() > 4096 {
            return Err(invalid(
                "This folder has too many chats to relocate safely in one operation.",
            ));
        }
        let mut reservations = ActorReservations(Vec::new());
        for chat in chats {
            let mailbox = self.actor(&chat);
            let id = crate::ids::new_id();
            let (reply, answer) = oneshot::channel();
            mailbox
                .send(ChatMsg::ReserveGit {
                    id: id.clone(),
                    reply,
                })
                .map_err(|_| stopped())?;
            reservations.0.push((chat, mailbox, id));
            answer.await.map_err(|_| stopped())??;
        }
        // Inspect after admission, immediately before the writer's identity compare-and-swap.
        let checked = self.location_review(&folder, &review.path).await?;
        if checked.path != review.path {
            return Err(invalid(
                "The selected folder changed. Review its location again.",
            ));
        }
        let Reply::Located(updated) = self
            .deps()
            .store
            .folder_resolution(Command::Locate {
                folder_id: folder.id.clone(),
                path: checked.path,
                previous: folder.path,
                kind: folder.kind,
                remote: folder.git_remote_url,
            })
            .await
            .map_err(|error| storage_error(&error))?
        else {
            return Err(invalid("Folder mapping returned an unexpected result."));
        };
        // The committed writer response contains these records. There is no fallible reread
        // between persistence and refreshing the parked actors before releasing reservations.
        for (chat, mailbox, id) in &reservations.0 {
            if let Some(record) = updated.chats.iter().find(|record| record.chat.id == *chat) {
                let (reply, answer) = oneshot::channel();
                if mailbox
                    .send(ChatMsg::FolderLocated {
                        reservation: id.clone(),
                        record: record.chat.clone(),
                        reply,
                    })
                    .is_ok()
                {
                    answer.await.map_err(|_| invalid(
                        "The location was saved. Reopen the app before starting the affected chats."
                    ))??;
                }
            }
        }
        folder_from(&updated, &review.folder_id)
    }
}

struct ActorReservations(Vec<(String, mpsc::UnboundedSender<ChatMsg>, String)>);
impl Drop for ActorReservations {
    fn drop(&mut self) {
        for (_, mailbox, id) in &self.0 {
            let _ = mailbox.send(ChatMsg::ReleaseGit { id: id.clone() });
        }
    }
}

async fn checked_directory(value: &str) -> Result<PathBuf, SessionError> {
    if value.is_empty()
        || value.len() > 32768
        || value.chars().any(char::is_control)
        || !Path::new(value).is_absolute()
    {
        return Err(invalid("Choose an absolute folder path."));
    }
    let path = PathBuf::from(value);
    tokio::task::spawn_blocking(move || {
        if !plain_directory(&path) {
            return Err(invalid(
                "Choose an accessible folder, not a linked directory.",
            ));
        }
        check_new_folder(&path)?;
        std::fs::canonicalize(path)
            .map(|path| crate::fsutil::plain_path(&path))
            .map_err(|_| invalid("The folder could not be resolved."))
    })
    .await
    .map_err(|_| invalid("The folder could not be checked."))?
}
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    metadata.file_type().is_symlink() || reparse
}
fn plain_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir() && !linked(&metadata))
}
fn candidates(roots: Vec<PathBuf>) -> (Vec<PathBuf>, bool) {
    let mut pending: Vec<_> = roots.into_iter().rev().map(|path| (path, 0)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut entries = 0;
    let mut truncated = false;
    while let Some((path, depth)) = pending.pop() {
        if !plain_directory(&path) {
            truncated = true;
            continue;
        }
        let Ok(path) = path.canonicalize() else {
            truncated = true;
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        match std::fs::symlink_metadata(path.join(".git")) {
            Ok(metadata) if !linked(&metadata) && (metadata.is_dir() || metadata.is_file()) => {
                if result.len() == MAX_REPOSITORIES {
                    truncated = true;
                    break;
                }
                result.push(crate::fsutil::plain_path(&path));
                continue;
            }
            Ok(_) => {
                truncated = true;
                continue;
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                truncated = true;
                continue;
            }
            _ => {}
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(children) = std::fs::read_dir(&path) else {
            truncated = true;
            continue;
        };
        for child in children {
            if entries == MAX_ENTRIES {
                return (result, true);
            }
            entries += 1;
            let Ok(child) = child else {
                truncated = true;
                continue;
            };
            let name = child.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(name.as_ref(), "node_modules" | "target" | "vendor")
            {
                continue;
            }
            if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push((child.path(), depth + 1));
            }
        }
    }
    (result, truncated)
}

#[cfg(test)]
mod tests;
