//! Explicit project folders passed to a native harness. Paths stay in device-local IPC only.
use super::{SessionError, SessionManager, actor::ChatMsg};
use crate::chatstore::{ChatRecord, ChatStore, organization::OrganizationCommand};
use ghost_protocol::{
    agents::AgentHost,
    workspace::{ChatFolderContext, WorkspaceFilter},
};
use std::{collections::HashSet, path::PathBuf, sync::Arc};

pub(crate) struct LaunchFolders {
    pub chat: ChatRecord,
    pub store: Arc<ChatStore>,
    pub worktrees: Option<Arc<crate::gitworkspaces::GitWorkspaces>>,
}

pub(crate) fn supported(spec: Option<&ghost_agents::AgentSpec>, host: &AgentHost) -> bool {
    matches!(host, AgentHost::Native)
        && spec.is_some_and(|spec| spec.kind == ghost_protocol::DriverKind::Claude)
}

impl SessionManager {
    pub async fn chat_folder_context(
        &self,
        chat_id: &str,
    ) -> Result<ChatFolderContext, SessionError> {
        let (chat, active, spec) = self
            .ask(chat_id, |reply| ChatMsg::FolderContext { reply })
            .await?;
        if let Some(active) = active {
            return Ok(active);
        }
        let supports = supported(spec.as_ref(), &chat.agent_host);
        resolve(
            LaunchFolders {
                chat,
                store: Arc::clone(&self.deps().store),
                worktrees: self.deps().worktrees.clone(),
            },
            supports,
        )
        .await
    }
}

pub(crate) async fn resolve(
    context: LaunchFolders,
    supports_additional: bool,
) -> Result<ChatFolderContext, SessionError> {
    let LaunchFolders {
        chat,
        store,
        worktrees,
    } = context;
    let mut result = ChatFolderContext {
        primary: chat.folder.to_string_lossy().into_owned(),
        additional: Vec::new(),
        notices: Vec::new(),
        active: false,
        supports_additional,
    };
    let organization = store
        .organize(OrganizationCommand::Snapshot(WorkspaceFilter {
            include_archived: true,
            project_id: None,
        }))
        .await
        .map_err(|error| super::storage_error(&error))?;
    let project = organization
        .chats
        .iter()
        .find(|entry| entry.chat.id == chat.id)
        .and_then(|entry| {
            organization
                .projects
                .iter()
                .find(|project| project.id == entry.project_id)
        });
    let Some(project) = project else {
        return Ok(result);
    };
    let primary = chat.folder.clone();
    let primary_id = chat.folder_id.clone();
    let folders = project.folders.clone();
    let isolated_source = if let Some(service) = &worktrees {
        service
            .list(&chat.id)
            .await
            .map_err(|_| {
                SessionError::Organization(
                    "The isolation record could not be read before preparing project folders."
                        .into(),
                )
            })?
            .into_iter()
            .find(|record| {
                super::live::normalize_path(PathBuf::from(&record.path))
                    == super::live::normalize_path(chat.folder.clone())
            })
            .map(|record| PathBuf::from(record.source))
    } else {
        None
    };
    let mut result = tokio::task::spawn_blocking(move || {
        let primary = std::fs::canonicalize(&primary).unwrap_or(primary);
        let mut seen = HashSet::from([super::live::normalize_path(primary)]);
        // A project with thousands of locations cannot expand one native command without bound.
        for folder in folders.iter().take(65) {
            if primary_id.as_deref() == Some(folder.id.as_str()) { continue; }
            if !supports_additional {
                if result.notices.is_empty() { result.notices.push("This driver receives only the primary folder. Additional project folders are not passed to it.".into()); }
                continue;
            }
            if result.additional.len() == 32 {
                result.notices.push("Only the first 32 available additional folders are included in this session.".into());
                break;
            }
            let Some(path) = &folder.path else {
                result.notices.push(format!("{} has no location on this device and is not included.", folder.display_name));
                continue;
            };
            let path = PathBuf::from(path);
            if super::check_new_folder(&path).is_err() {
                result.notices.push(format!("{} is unavailable and is not included.", folder.display_name));
                continue;
            }
            let Ok(path) = std::fs::canonicalize(path) else { continue };
            if seen.insert(super::live::normalize_path(path.clone())) {
                result.additional.push(path.to_string_lossy().into_owned());
            }
        }
        if folders.len() > 65 { result.notices.push("Additional project locations exceed the session's discovery limit.".into()); }
        Ok::<_, SessionError>(result)
    }).await.map_err(|_| SessionError::Internal("Checking additional project folders failed.".into()))??;
    if let (Some(source), Some(service)) = (isolated_source, worktrees) {
        let family = service.repository_family(&source).await;
        if let Ok(Some(family)) = family {
            let source = super::live::normalize_path(source);
            let additional = std::mem::take(&mut result.additional);
            for path in additional {
                let normalized = super::live::normalize_path(PathBuf::from(&path));
                let candidate = service.repository_family(&normalized).await;
                if normalized.starts_with(&source)
                    || source.starts_with(&normalized)
                    || candidate
                        .as_ref()
                        .is_ok_and(|candidate| candidate.as_ref() == Some(&family))
                {
                    result.notices.push(format!("{path} belongs to the original checkout and is excluded from this isolated session."));
                } else if candidate.is_err() {
                    result.notices.push(format!(
                        "{path} could not be checked for isolation and is not included."
                    ));
                } else {
                    result.additional.push(path);
                }
            }
        } else {
            result.additional.clear();
            result.notices.push("Additional folders were not included because the original repository's isolation identity could not be verified.".into());
        }
    }
    Ok(result)
}
