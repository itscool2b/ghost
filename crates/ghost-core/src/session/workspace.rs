//! Project organization and device-local folder preflight. No operation moves files on disk.

use ghost_protocol::workspace::{
    ChatCreatePreview, ChatMove, ChatRole, FolderMatch, FolderMatches, ProjectChatCreate,
    ProjectCreate, ProjectFolder, ProjectInput, ProjectSummary, RoleModel, WorkspaceChat,
    WorkspaceFilter, WorkspaceSnapshot, WorktreeOrigin,
};

use super::*;
use crate::chatstore::organization::{Organization, OrganizationCommand};

impl SessionManager {
    async fn portable_folder_identity(
        &self,
        folder: &std::path::Path,
    ) -> Option<crate::gitworkspaces::FolderIdentity> {
        self.deps()
            .worktrees
            .as_ref()?
            .folder_identity(folder)
            .await
            .ok()
    }

    pub async fn workspace_snapshot(
        &self,
        filter: WorkspaceFilter,
    ) -> Result<WorkspaceSnapshot, SessionError> {
        if let Some(id) = &filter.project_id {
            bounded(id, "Project ID", 128)?;
        }
        let organization = self
            .organization(OrganizationCommand::Snapshot(filter))
            .await?;
        Ok(self.workspace_view(organization).await)
    }

    pub async fn project_create(
        &self,
        request: ProjectCreate,
    ) -> Result<ProjectSummary, SessionError> {
        self.validate_project(&request.project)?;
        let folder = checked_folder(&request.folder).await?;
        let identity = self.portable_folder_identity(&folder).await;
        let organization = self
            .organization(OrganizationCommand::CreateProject {
                id: crate::ids::new_id(),
                input: request.project,
                folder,
                identity,
            })
            .await?;
        let id = organization.affected_id.clone();
        self.workspace_view(organization)
            .await
            .projects
            .into_iter()
            .find(|project| Some(&project.id) == id.as_ref())
            .ok_or_else(missing)
    }

    pub async fn project_update(
        &self,
        project_id: &str,
        input: ProjectInput,
    ) -> Result<ProjectSummary, SessionError> {
        bounded(project_id, "Project ID", 128)?;
        self.validate_project(&input)?;
        let organization = self
            .organization(OrganizationCommand::UpdateProject {
                id: project_id.into(),
                input,
            })
            .await?;
        self.workspace_view(organization)
            .await
            .projects
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(missing)
    }

    pub async fn project_add_folder(
        &self,
        project_id: &str,
        path: &str,
    ) -> Result<ProjectFolder, SessionError> {
        bounded(project_id, "Project ID", 128)?;
        let folder = checked_folder(path).await?;
        let identity = self.portable_folder_identity(&folder).await;
        let organization = self
            .organization(OrganizationCommand::AddFolder {
                project_id: project_id.into(),
                folder,
                identity,
            })
            .await?;
        let id = organization.affected_id.clone();
        self.workspace_view(organization)
            .await
            .projects
            .into_iter()
            .flat_map(|project| project.folders)
            .find(|folder| Some(&folder.id) == id.as_ref())
            .ok_or_else(missing)
    }

    pub async fn chat_rename(
        &self,
        chat_id: &str,
        title: &str,
    ) -> Result<WorkspaceChat, SessionError> {
        bounded(title, "Chat title", 512)?;
        self.organize_chat(
            chat_id,
            OrganizationCommand::RenameChat {
                id: chat_id.into(),
                title: title.trim().into(),
            },
        )
        .await
    }

    pub async fn chat_pin(
        &self,
        chat_id: &str,
        pinned: bool,
    ) -> Result<WorkspaceChat, SessionError> {
        self.organize_chat(
            chat_id,
            OrganizationCommand::PinChat {
                id: chat_id.into(),
                pinned,
            },
        )
        .await
    }

    pub async fn chat_archive(
        &self,
        chat_id: &str,
        archived: bool,
    ) -> Result<WorkspaceChat, SessionError> {
        self.organize_chat(
            chat_id,
            OrganizationCommand::ArchiveChat {
                id: chat_id.into(),
                archived,
            },
        )
        .await
    }

    /// Names the agent, model and thinking level this chat plans or builds with, or clears it. A
    /// connection is kept as its agent, as the default agent is. Nothing runs here: the window
    /// applies the choice through the chat's own model picker, and a hand-over to another agent
    /// or a coordinator's worker checks then that the agent is ready.
    pub async fn chat_set_role(
        &self,
        chat_id: &str,
        role: ChatRole,
        choice: Option<RoleModel>,
    ) -> Result<WorkspaceChat, SessionError> {
        let choice = match choice {
            None => None,
            Some(choice) => {
                bounded(&choice.agent, "Agent identifier", 128)?;
                if let Some(model) = &choice.model {
                    bounded(model, "Model identifier", 256)?;
                }
                if let Some(effort) = &choice.effort {
                    bounded(effort, "Thinking level", 128)?;
                }
                let agent = ghost_agents::agent_of(choice.agent.trim());
                Some(RoleModel {
                    agent: agent.to_owned(),
                    model: choice.model.map(|model| model.trim().to_owned()),
                    effort: choice.effort.map(|effort| effort.trim().to_owned()),
                })
            }
        };
        self.organize_chat(
            chat_id,
            OrganizationCommand::SetChatRole {
                id: chat_id.into(),
                role,
                choice,
            },
        )
        .await
    }

    pub async fn chat_move(
        &self,
        chat_id: &str,
        request: ChatMove,
    ) -> Result<WorkspaceChat, SessionError> {
        bounded(&request.project_id, "Project ID", 128)?;
        bounded(chat_id, "Chat ID", 128)?;
        // The chat's folder and the destination, each read again once held: a busy check made
        // under them stays true until the move is saved.
        let read = (self.clone(), chat_id.to_owned(), request.folder_id.clone());
        let (_organization, (current, destination)) = self
            .hold_folders(move || {
                let (manager, chat_id, folder_id) = read.clone();
                Box::pin(async move {
                    let current = manager
                        .chat_record(&chat_id)
                        .await
                        .map_err(|_| missing())?
                        .folder;
                    let destination = match &folder_id {
                        Some(folder_id) => Some(manager.chat_create_preview(folder_id).await?),
                        None => None,
                    };
                    let mut folders = vec![current.clone()];
                    if let Some(preview) = &destination {
                        folders.push(PathBuf::from(usable(&preview.folder)?));
                    }
                    Ok((folders, (current, destination)))
                })
            })
            .await?;
        let mut preserves_folder = true;
        if let Some(preview) = destination {
            if preview.folder.project_id != request.project_id {
                return Err(invalid(
                    "The folder does not belong to the destination project.",
                ));
            }
            let destination = PathBuf::from(usable(&preview.folder)?);
            preserves_folder = tokio::task::spawn_blocking(move || {
                live::normalize_path(current) == live::normalize_path(destination)
            })
            .await
            .map_err(|_| SessionError::Internal("Checking the chat folder failed.".into()))?;
            if !request.allow_busy_folder
                && preview.busy_chats.iter().any(|chat| chat.id != chat_id)
            {
                return Err(invalid(
                    "This folder has an active chat. Review the busy-folder warning before continuing.",
                ));
            }
        }
        self.organize_chat(
            chat_id,
            OrganizationCommand::MoveChat {
                id: chat_id.into(),
                project_id: request.project_id,
                folder_id: request.folder_id,
                preserves_folder,
            },
        )
        .await
    }

    /// What a picked folder already is on this device: the project folders it is, however the
    /// pick spelled it, and whether it is a worktree Ghost manages. Both sides are compared
    /// canonical, as folder keys are, so a pick through a link or in other letter case on a
    /// case-insensitive file system finds its project instead of becoming a new one. The folder
    /// whose chat was active most recently comes first, then the workspace's order.
    pub async fn workspace_folder_matches(
        &self,
        path: &str,
    ) -> Result<FolderMatches, SessionError> {
        bounded(path, "Folder path", 32_768)?;
        let picked = PathBuf::from(path);
        if !picked.is_absolute() {
            return Err(invalid("Choose an absolute folder path."));
        }
        // The saved projects as they are, without the chat list's folder checks: the folders
        // are read below, within one short wait.
        let workspace = self
            .organization(OrganizationCommand::Snapshot(WorkspaceFilter {
                include_archived: true,
                project_id: None,
            }))
            .await?;
        let worktrees = match self.worktrees() {
            Ok(service) => service.profile_records().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        // The picked folder, and every stored one, each read on its own thread and all within the
        // folder check's short wait: a folder that does not answer (a disconnected network share)
        // is in no project, and never holds up the picker.
        let shared = Arc::clone(self.shared());
        let wait = shared.knobs.folder_check_wait;
        let pick = tokio::time::timeout(
            wait,
            tokio::task::spawn_blocking(move || {
                shared.slow_disk(&picked);
                let canonical = std::fs::canonicalize(&picked)
                    .map_err(|error| invalid(&format!("Cannot resolve this folder: {error}")))?;
                if !canonical.is_dir() {
                    return Err(invalid("Choose a folder, not a file."));
                }
                let stored = crate::fsutil::plain_path(&canonical);
                let key = live::normalize_path(stored.clone());
                Ok((stored, key))
            }),
        );
        let candidates: Vec<PathBuf> = workspace
            .projects
            .iter()
            .flat_map(|project| &project.folders)
            .filter_map(|folder| folder.path.as_ref().map(PathBuf::from))
            .chain(worktrees.iter().map(|record| PathBuf::from(&record.path)))
            .collect();
        let (pick, keys) = tokio::join!(pick, self.resolve_folders(candidates));
        let (stored, key) = pick
            .map_err(|_| {
                invalid("This folder did not answer. Check that its drive or share is connected, then choose it again.")
            })?
            .map_err(|_| SessionError::Internal("Checking the folder failed.".into()))??;
        {
            let same = |other: &str| keys.get(Path::new(other)) == Some(&key);
            let worktree = worktrees
                .iter()
                .find(|record| same(&record.path))
                .map(|record| WorktreeOrigin {
                    branch: record.branch.clone(),
                    source: crate::fsutil::plain_path(Path::new(&record.source))
                        .to_string_lossy()
                        .into_owned(),
                });
            let mut matches: Vec<FolderMatch> = workspace
                .projects
                .iter()
                .flat_map(|project| {
                    project.folders.iter().filter_map(move |folder| {
                        let path = folder.path.as_deref()?;
                        same(path).then(|| FolderMatch {
                            folder_id: folder.id.clone(),
                            project_id: project.id.clone(),
                            project_name: project.name.clone(),
                            last_chat_at: None,
                        })
                    })
                })
                .collect();
            for found in &mut matches {
                found.last_chat_at = workspace
                    .chats
                    .iter()
                    .filter(|entry| entry.folder_id.as_deref() == Some(found.folder_id.as_str()))
                    .map(|entry| entry.chat.updated_at)
                    .max();
            }
            // Stable: equally recent folders keep the workspace's order.
            matches.sort_by_key(|found| std::cmp::Reverse(found.last_chat_at));
            Ok(FolderMatches {
                path: stored.to_string_lossy().into_owned(),
                matches,
                worktree,
            })
        }
    }

    /// Each of `paths` resolved as folder keys are, each on its own blocking thread, all within
    /// the folder check's short wait. One that does not answer in time is left out, and is not
    /// asked again while its thread still waits on the disk.
    async fn resolve_folders(&self, mut paths: Vec<PathBuf>) -> HashMap<PathBuf, PathBuf> {
        paths.sort();
        paths.dedup();
        let shared = Arc::clone(self.shared());
        let found = Arc::new(Mutex::new(HashMap::new()));
        let started: Vec<PathBuf> = {
            let mut resolving = lock(&shared.resolving);
            paths
                .into_iter()
                .filter(|path| resolving.insert(path.clone()))
                .collect()
        };
        let tasks: futures::stream::FuturesUnordered<_> = started
            .into_iter()
            .map(|path| {
                let (shared, found) = (Arc::clone(&shared), Arc::clone(&found));
                self.deps().rt.spawn_blocking(move || {
                    shared.slow_disk(&path);
                    let key = live::normalize_path(path.clone());
                    lock(&shared.resolving).remove(&path);
                    lock(&found).insert(path, key);
                })
            })
            .collect();
        let _ = tokio::time::timeout(
            shared.knobs.folder_check_wait,
            futures::StreamExt::collect::<Vec<_>>(tasks),
        )
        .await;
        std::mem::take(&mut *lock(&found))
    }

    pub async fn chat_create_preview(
        &self,
        folder_id: &str,
    ) -> Result<ChatCreatePreview, SessionError> {
        bounded(folder_id, "Folder ID", 128)?;
        let workspace = self
            .workspace_snapshot(WorkspaceFilter {
                include_archived: true,
                project_id: None,
            })
            .await?;
        let folder = workspace
            .projects
            .into_iter()
            .flat_map(|project| project.folders)
            .find(|folder| folder.id == folder_id)
            .ok_or_else(missing)?;
        let candidates: Vec<_> = workspace
            .chats
            .into_iter()
            .map(|chat| chat.chat)
            .filter(|chat| {
                // Working or waiting for an answer: an agent idle here edits nothing until it is
                // given a message, so it does not make the folder busy.
                self.shared()
                    .live
                    .busy(&chat.id, self.shared().knobs.busy_settle)
            })
            .collect();
        let path = folder.path.clone();
        let busy_chats = tokio::task::spawn_blocking(move || {
            let Some(path) = path else {
                return Vec::new();
            };
            let key = live::normalize_path(PathBuf::from(path));
            candidates
                .into_iter()
                .filter(|chat| live::normalize_path(PathBuf::from(&chat.folder)) == key)
                .collect()
        })
        .await
        .map_err(|_| SessionError::Internal("Checking active folders failed.".into()))?;
        Ok(ChatCreatePreview { folder, busy_chats })
    }

    /// Persists a chat before starting its agent. A failed start remains a visible saved chat.
    pub async fn chat_project_create(
        &self,
        request: ProjectChatCreate,
    ) -> Result<WorkspaceChat, SessionError> {
        self.create_project_chat(request, true).await
    }

    /// [`chat_project_create`](Self::chat_project_create), starting the agent or not. A project's
    /// coordinator and workers are recorded as such before their agent starts, so the session
    /// that starts has their tools.
    pub(crate) async fn create_project_chat(
        &self,
        request: ProjectChatCreate,
        start: bool,
    ) -> Result<WorkspaceChat, SessionError> {
        bounded(&request.agent_id, "Agent ID", 128)?;
        if self.deps().detector.agent(&request.agent_id).is_none() {
            return Err(SessionError::UnknownAgent(request.agent_id));
        }
        // Creates in one folder go one at a time, so a second cannot pass the busy check before
        // the first one's agent counts; other folders' creates go at once.
        let read = (self.clone(), request.folder_id.clone());
        let (_organization, preview) = self
            .hold_folders(move || {
                let (manager, folder_id) = read.clone();
                Box::pin(async move {
                    let preview = manager.chat_create_preview(&folder_id).await?;
                    Ok((vec![PathBuf::from(usable(&preview.folder)?)], preview))
                })
            })
            .await?;
        let path = usable(&preview.folder)?;
        checked_folder(path).await?;
        if !request.allow_busy_folder && !preview.busy_chats.is_empty() {
            return Err(invalid(
                "This folder has an active chat. Review the busy-folder warning before continuing.",
            ));
        }
        let chat_id = crate::ids::new_id();
        let organization = self
            .organization(OrganizationCommand::CreateChat {
                id: chat_id.clone(),
                folder_id: request.folder_id,
                agent_id: ghost_agents::agent_of(&request.agent_id).to_owned(),
            })
            .await?;
        if start {
            let _ = self
                .ask(&chat_id, |reply| ChatMsg::Start {
                    fresh: false,
                    explicit: false,
                    reply,
                })
                .await;
        }
        self.workspace_view(organization)
            .await
            .chats
            .into_iter()
            .find(|chat| chat.chat.id == chat_id)
            .ok_or_else(missing)
    }

    fn validate_project(&self, input: &ProjectInput) -> Result<(), SessionError> {
        bounded(&input.name, "Project name", 256)?;
        if let Some(color) = &input.color
            && (color.len() != 7
                || !color.starts_with('#')
                || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(invalid("Project color must be a six-digit hex color."));
        }
        if let Some(agent) = &input.default_agent {
            bounded(agent, "Default agent", 128)?;
            if self.deps().detector.agent(agent).is_none() {
                return Err(SessionError::UnknownAgent(agent.clone()));
            }
        }
        if let Some(model) = &input.default_model {
            bounded(model, "Default model", 256)?;
        }
        Ok(())
    }

    pub(super) async fn organization(
        &self,
        command: OrganizationCommand,
    ) -> Result<Organization, SessionError> {
        if !command.is_read() && self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.deps()
            .store
            .organize(command)
            .await
            .map_err(|error| storage_error(&error))
    }

    async fn organize_chat(
        &self,
        chat_id: &str,
        command: OrganizationCommand,
    ) -> Result<WorkspaceChat, SessionError> {
        bounded(chat_id, "Chat ID", 128)?;
        let result = self
            .ask(chat_id, |reply| ChatMsg::Organize {
                command: command.clone(),
                reply,
            })
            .await?;
        self.workspace_view(result)
            .await
            .chats
            .into_iter()
            .find(|chat| chat.chat.id == chat_id)
            .ok_or_else(missing)
    }

    pub(super) async fn workspace_view(&self, mut organization: Organization) -> WorkspaceSnapshot {
        let worktrees = match self.worktrees() {
            Ok(service) => service.profile_records().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let paths = organization
            .projects
            .iter()
            .flat_map(|project| &project.folders)
            .filter_map(|folder| folder.path.as_ref().map(PathBuf::from))
            .collect();
        let checked = self.check_folders(paths).await;
        for project in &mut organization.projects {
            for folder in &mut project.folders {
                if let Some(path) = &folder.path {
                    if let Some(record) = worktrees.iter().find(|record| {
                        crate::fsutil::plain_path(Path::new(&record.path))
                            == crate::fsutil::plain_path(Path::new(path))
                    }) {
                        let source = crate::fsutil::plain_path(Path::new(&record.source));
                        folder.display_name =
                            format!("{} (worktree from {})", record.branch, source.display());
                        folder.worktree_of = Some(source.to_string_lossy().into_owned());
                    }
                    folder.folder_state = checked
                        .get(&PathBuf::from(path))
                        .copied()
                        .unwrap_or(FolderState::Ok);
                    folder.unavailable_reason = match (checked.get(&PathBuf::from(path)), folder.folder_state) {
                        (None, _) => Some("The folder check has not finished. Try again shortly.".into()),
                        (_, FolderState::Ok) => None,
                        (_, FolderState::Missing) => Some("This folder is missing on this device. Locate it before starting a chat.".into()),
                        (_, FolderState::NotAllowed) => Some(FOLDER_NOT_ALLOWED.into()),
                    };
                }
            }
        }
        let chats = organization
            .chats
            .into_iter()
            .map(|record| {
                let state = self.shared().state(&record.chat.id);
                let found = checked.get(&record.chat.folder).copied().or_else(|| {
                    record
                        .chat
                        .folder
                        .as_os_str()
                        .is_empty()
                        .then_some(FolderState::Missing)
                });
                WorkspaceChat {
                    chat: self.summary(record.chat, state, found),
                    project_id: record.project_id,
                    folder_id: record.folder_id,
                    pinned: record.pinned,
                    archived: record.archived,
                    roles: record.roles,
                }
            })
            .collect();
        WorkspaceSnapshot {
            projects: organization.projects,
            chats,
        }
    }
}

pub(super) fn bounded(value: &str, label: &str, max: usize) -> Result<(), SessionError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(invalid(&format!(
            "{label} must contain 1 to {max} bytes without control characters."
        )));
    }
    Ok(())
}

pub(super) async fn checked_folder(path: &str) -> Result<PathBuf, SessionError> {
    bounded(path, "Folder path", 32_768)?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(invalid("Choose an absolute folder path."));
    }
    tokio::task::spawn_blocking(move || {
        check_new_folder(&path)?;
        // Windows canonical paths are verbatim (`\\?\C:\...`); store and show the plain spelling
        // whenever it names the same folder.
        std::fs::canonicalize(path)
            .map(|path| crate::fsutil::plain_path(&path))
            .map_err(|error| invalid(&format!("Cannot resolve this folder: {error}")))
    })
    .await
    .map_err(|_| SessionError::Internal("Checking the folder failed.".into()))?
}

pub(super) fn usable(folder: &ProjectFolder) -> Result<&str, SessionError> {
    if folder.folder_state != FolderState::Ok || folder.unavailable_reason.is_some() {
        return Err(invalid(
            folder
                .unavailable_reason
                .as_deref()
                .unwrap_or("This folder is unavailable."),
        ));
    }
    folder
        .path
        .as_deref()
        .ok_or_else(|| invalid("Locate this folder on this device."))
}

pub(super) fn missing() -> SessionError {
    invalid("The project, folder, or chat no longer exists.")
}
fn invalid(message: &str) -> SessionError {
    SessionError::Organization(message.into())
}
