//! Explicit, reversible removal with one-use review and owned actor/folder admission.
use super::*;
use crate::chatstore::workspace_removal::{Command, Reply, Snapshot};
use ghost_protocol::workspace_removal::{
    RemovedWorkspace, WorkspaceRemovalAction, WorkspaceRemovalPreview, WorkspaceRemovalTarget,
};

pub(super) struct Prepared {
    expires: Instant,
    before: Snapshot,
    action: WorkspaceRemovalAction,
}
use super::gitworkspaces::Reservations;
fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}
impl SessionManager {
    pub async fn workspace_removed(&self) -> Result<Vec<RemovedWorkspace>, SessionError> {
        match self
            .deps()
            .store
            .workspace_removal(Command::List)
            .await
            .map_err(|e| storage_error(&e))?
        {
            Reply::List(items) => Ok(items),
            _ => Err(invalid("The removed workspace list was unavailable.")),
        }
    }
    pub async fn workspace_removal_preview(
        &self,
        target: WorkspaceRemovalTarget,
        action: WorkspaceRemovalAction,
    ) -> Result<WorkspaceRemovalPreview, SessionError> {
        let id = crate::chatstore::workspace_removal::target_id(&target);
        if id.is_empty() || id.len() > 128 {
            return Err(invalid("The workspace identifier is invalid."));
        }
        let Reply::Snapshot(before) = self
            .deps()
            .store
            .workspace_removal(Command::Inspect(target.clone()))
            .await
            .map_err(|e| storage_error(&e))?
        else {
            return Err(invalid("The workspace could not be reviewed."));
        };
        if before.deleted == (action == WorkspaceRemovalAction::Remove) || before.parent_deleted {
            return Err(invalid(
                "Refresh the workspace. Restore the project before restoring one of its folders.",
            ));
        }
        let token = crate::ids::new_id();
        let result = WorkspaceRemovalPreview {
            token: token.clone(),
            target,
            action,
            name: before.name.clone(),
            chat_count: before.chats.len() as u32,
            folders: before
                .folders
                .iter()
                .filter_map(|(_, path, _, _)| path.clone())
                .collect(),
            notices: vec![
                "Files and Git worktrees on disk are unchanged. No folder or file is deleted."
                    .into(),
                if action == WorkspaceRemovalAction::Remove {
                    "This item and its chats will be hidden. Their messages, drafts, attachments and native history stay saved. Use Removed projects and folders to restore them.".into()
                } else {
                    "This restores the same saved item and chats. Previously archived chats stay archived; removed child folders stay removed.".into()
                },
                if action == WorkspaceRemovalAction::Remove {
                    "Stop affected agents, clear queued prompts and close project terminals before applying.".into()
                } else {
                    "Stop affected agents and close project terminals before restoring. Saved queued prompts remain held until you explicitly resume them.".into()
                },
            ],
        };
        let mut reviews = lock(&self.shared().removals);
        reviews.retain(|_, review| review.expires > Instant::now());
        if reviews.len() >= 16 {
            return Err(invalid(
                "Too many workspace reviews are open. Close an earlier review and try again shortly.",
            ));
        }
        reviews.insert(
            token,
            Prepared {
                expires: Instant::now() + Duration::from_secs(600),
                before,
                action,
            },
        );
        Ok(result)
    }
    /// Holds the native profile lease through the actual transaction even if the invoking page
    /// closes. A cancelled caller cannot release admission while an accepted write is pending.
    pub async fn workspace_removal_apply(
        &self,
        token: &str,
        lifetime: Box<dyn Send + Sync>,
    ) -> Result<(), SessionError> {
        let review = lock(&self.shared().removals)
            .remove(token)
            .filter(|review| review.expires > Instant::now())
            .ok_or_else(|| {
                invalid("This review expired or was already used. Review the workspace again.")
            })?;
        let manager = self.clone();
        let token = token.to_owned();
        self.deps()
            .rt
            .spawn(async move {
                let _lifetime = lifetime;
                let paths = review
                    .before
                    .folders
                    .iter()
                    .filter_map(|(_, path, _, _)| path.as_ref().map(PathBuf::from))
                    .collect::<Vec<_>>();
                let folders = tokio::task::spawn_blocking(move || {
                    let mut paths = paths
                        .into_iter()
                        .map(live::normalize_path)
                        .collect::<Vec<_>>();
                    paths.sort();
                    paths.dedup();
                    let mut roots: Vec<PathBuf> = Vec::new();
                    for path in paths {
                        if !roots.iter().any(|root| path.starts_with(root)) {
                            roots.push(path);
                        }
                    }
                    roots
                })
                .await
                .map_err(|_| invalid("Folder ownership could not be checked."))?;
                // The removed chats' agents, and any agent in these folders, stop first unless
                // they are working; each resumes its session if the workspace is restored. Both
                // this wait and resolving the folders above read the disk or wait for processes,
                // so they happen before every other change is held up.
                let removed: Vec<String> = review
                    .before
                    .chats
                    .iter()
                    .map(|(chat, _)| chat.clone())
                    .collect();
                // What would still be in the way is named before anything stops; the review
                // stays usable for the retry.
                let holders = match manager.exclusive_can_go(&folders, None, &removed).await {
                    Ok(holders) => holders,
                    Err(error) => {
                        lock(&manager.shared().removals).insert(token, review);
                        return Err(error);
                    }
                };
                manager
                    .stop_unless_working(super::stopping::StopScope::Chats(holders))
                    .await;
                // Removal can touch any folder: the change itself waits for every change in
                // flight, and holds the ones after it only while it is made.
                let _organization = manager.shared().organization.exclusive().await;
                let _maintenance = manager
                    .shared()
                    .live
                    .try_maintenance()
                    .ok_or(SessionError::ShuttingDown)?;
                let mut reservations = Reservations::new(Arc::clone(manager.shared()));
                let mut refused = Vec::new();
                for (chat, _) in &review.before.chats {
                    let mailbox = manager.actor(chat);
                    let id = crate::ids::new_id();
                    let (reply, answer) = oneshot::channel();
                    mailbox
                        .send(ChatMsg::ReserveWorkspace {
                            id: id.clone(),
                            restoring: review.action == WorkspaceRemovalAction::Restore,
                            reply,
                        })
                        .map_err(|_| stopped())?;
                    reservations.actors.push((mailbox, id));
                    match answer.await.map_err(|_| stopped())? {
                        Ok(()) => {}
                        Err(SessionError::Organization(_)) => {
                            refused.push(chat.clone());
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                    // Flush each actor before comparing reviewed membership and update times.
                    manager
                        .ask(chat, |reply| ChatMsg::CheckpointContext {
                            turn_id: None,
                            reply,
                        })
                        .await?;
                }
                let busy = if refused.is_empty() {
                    folders.iter().find_map(|folder| {
                        if manager.shared().live.reserve_folder(folder) {
                            reservations.folders.push(folder.clone());
                            None
                        } else {
                            Some(())
                        }
                    })
                } else {
                    Some(())
                };
                if busy.is_some() {
                    drop(reservations);
                    let error = manager.folder_busy(&folders, refused, None).await;
                    // The review stays usable: stop what is in the way, then apply it again.
                    lock(&manager.shared().removals).insert(token, review);
                    return Err(error);
                }
                match manager
                    .deps()
                    .store
                    .workspace_removal(Command::Apply {
                        before: review.before,
                        action: review.action,
                    })
                    .await
                    .map_err(|e| storage_error(&e))?
                {
                    Reply::Applied => Ok(()),
                    _ => Err(invalid(
                        "The workspace change returned an invalid response.",
                    )),
                }
            })
            .await
            .map_err(|_| {
                invalid("The workspace change was interrupted. Refresh before reviewing it again.")
            })?
    }
}
