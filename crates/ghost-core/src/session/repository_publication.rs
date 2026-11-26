//! Profile-bound, reviewed repository publication. No prompt, automatic commit or credential read.
use super::*;
use crate::chatstore::organization::OrganizationCommand;
use ghost_protocol::{
    repository_publication::{
        RepositoryPublicationInput, RepositoryPublicationResult, RepositoryPublicationReview,
        RepositoryPublicationState,
    },
    workspace::{FolderKind, ProjectFolder, WorkspaceFilter},
};
fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}
impl SessionManager {
    pub async fn repository_publication_draft(
        &self,
        folder_id: &str,
    ) -> Result<
        Option<ghost_protocol::repository_publication::RepositoryPublicationDraft>,
        SessionError,
    > {
        self.publication_folder(folder_id).await?;
        self.deps()
            .worktrees
            .as_ref()
            .ok_or_else(|| invalid("Git tools are unavailable."))?
            .repository_publication_draft(folder_id)
            .await
            .map_err(|error| invalid(error.to_string()))
    }
    async fn publication_folder(&self, id: &str) -> Result<ProjectFolder, SessionError> {
        let folder = self
            .workspace_snapshot(WorkspaceFilter {
                include_archived: true,
                project_id: None,
            })
            .await?
            .projects
            .into_iter()
            .flat_map(|project| project.folders)
            .find(|folder| folder.id == id)
            .ok_or_else(|| invalid("This project folder no longer exists."))?;
        if folder.folder_state != FolderState::Ok || folder.path.is_none() {
            return Err(invalid(
                "Locate the original folder on this device before publishing it.",
            ));
        }
        if folder.kind != FolderKind::Local {
            return Err(invalid(
                "This folder already has a portable Git origin. Use its existing repository controls.",
            ));
        }
        Ok(folder)
    }
    pub async fn repository_publication_review(
        &self,
        folder_id: &str,
        input: RepositoryPublicationInput,
    ) -> Result<RepositoryPublicationReview, SessionError> {
        let folder = self.publication_folder(folder_id).await?;
        let path = folder
            .path
            .ok_or_else(|| invalid("Locate this folder before publishing."))?;
        self.deps()
            .worktrees
            .as_ref()
            .ok_or_else(|| invalid("Git tools are unavailable."))?
            .repository_publication_review(folder_id, Path::new(&path), input)
            .await
            .map_err(|error| invalid(error.to_string()))
    }
    pub async fn repository_publication_apply(
        &self,
        folder_id: String,
        token: String,
    ) -> Result<RepositoryPublicationResult, SessionError> {
        let manager = self.clone();
        self.deps().rt.spawn(async move { manager.apply_publication(&folder_id,&token).await }).await.map_err(|_|invalid("Publication stopped unexpectedly. Review its saved recovery record before retrying."))?
    }
    async fn apply_publication(
        &self,
        folder_id: &str,
        token: &str,
    ) -> Result<RepositoryPublicationResult, SessionError> {
        let read = (self.clone(), folder_id.to_owned());
        let (_organization, folder) = self
            .hold_folders(move || {
                let (manager, folder_id) = read.clone();
                Box::pin(async move {
                    let folder = manager.publication_folder(&folder_id).await?;
                    Ok((folder.path.iter().map(PathBuf::from).collect(), folder))
                })
            })
            .await?;
        let path = PathBuf::from(
            folder
                .path
                .ok_or_else(|| invalid("Locate this folder before publishing."))?,
        );
        let _reservations = self.hold_quiet(vec![path.clone()]).await?;
        let service = self
            .deps()
            .worktrees
            .as_ref()
            .ok_or_else(|| invalid("Git tools are unavailable."))?;
        let mut result = service
            .repository_publication_apply(folder_id, &path, token)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        if result.state == RepositoryPublicationState::Complete {
            let identity = service
                .folder_identity(&path)
                .await
                .map_err(|error| invalid(error.to_string()))?;
            if !matches!(&identity,crate::gitworkspaces::FolderIdentity::Git{remote,..} if remote==&result.url)
            {
                result.state = RepositoryPublicationState::NeedsReview;
                result.detail="The repository is published, but the local origin no longer matches. Review the folder before finishing its portable identity.".into();
                return Ok(result);
            }
            let updated = self
                .deps()
                .store
                .organize(OrganizationCommand::PublishFolderIdentity {
                    folder_id: folder.id,
                    expected_path: path,
                    identity,
                })
                .await;
            if updated.is_err() {
                result.state = RepositoryPublicationState::NeedsReview;
                result.detail="The repository is published, but its portable project identity could not be saved. Review publication to finish; no remote retry is needed.".into();
            }
        }
        Ok(result)
    }
}
