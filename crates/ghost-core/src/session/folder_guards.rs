//! What a Git or file change does with the chats working in its folders, and how it says who is
//! in the way.
//!
//! A quiet change (a commit, a pull request, a copy of working files, a setup copy, an instruction
//! commit) reads the folder or adds to Git only: idle agents and project terminals there stay, and
//! the chats wait with new turns and starts until it ends. It is refused only while one of them
//! works. An exclusive change (removing, relocating, recreating or recovering a worktree,
//! isolating, reverting, initialising, removing a project) rewrites the folder: the agents there
//! that are not working are stopped first (each resumes its session on its next message), then
//! the folder is reserved as before. Either way, what is still in the way is named, chat by chat
//! with its project, in a [`SessionError::FolderBusy`] the person can act on.

use ghost_protocol::workspace::{BlockerState, FolderBlocker, FolderBlockers, FolderScope};

use super::gitworkspaces::Reservations;
use super::live::Doing;
use super::stopping::StopScope;
use super::*;
use crate::chatstore::organization::OrganizationCommand;

impl SessionManager {
    /// The folders `folders` name, as given and resolved: agents register them both ways.
    pub(super) async fn spellings(&self, folders: Vec<PathBuf>) -> Vec<PathBuf> {
        let given = folders.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            folders
                .into_iter()
                .map(live::normalize_path)
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let mut all: Vec<PathBuf> = given.into_iter().chain(resolved).collect();
        all.sort();
        all.dedup();
        all
    }

    /// Holds `folders` for a quiet change: the chats there (stopped ones included) defer new
    /// turns and starts until the returned guard is dropped. Refused, naming them, while one of
    /// them works, or while another change holds an overlapping folder.
    pub(super) async fn hold_quiet(
        &self,
        folders: Vec<PathBuf>,
    ) -> Result<Reservations, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if self.deps().store.is_paused() {
            return Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before Git actions.".into(),
            ));
        }
        let (folders, mut chats) = self.chats_in(folders).await?;
        let mut held = Reservations::quiet(Arc::clone(self.shared()));
        if !self.shared().live.hold_quietly(&folders) {
            if self.shared().live.quiescing_now() {
                return Err(SessionError::ShuttingDown);
            }
            // Only a change that rewrites the folder keeps a quiet one out.
            return Err(self.quiet_busy(Vec::new(), true).await);
        }
        held.folders.clone_from(&folders);
        chats.extend(
            self.shared()
                .live
                .holders(&self.spellings(folders.clone()).await),
        );
        chats.sort();
        chats.dedup();
        let mut working = Vec::new();
        for chat in chats {
            let id = crate::ids::new_id();
            let mailbox = self.actor(&chat);
            let (reply, answer) = oneshot::channel();
            mailbox
                .send(ChatMsg::HoldQuiet {
                    id: id.clone(),
                    reply,
                })
                .map_err(|_| stopped())?;
            // Registered before awaiting, so a cancelled caller cannot leave a chat held.
            held.actors.push((mailbox, id));
            match answer.await {
                Ok(Ok(())) => {}
                Ok(Err(SessionError::Organization(_))) => working.push(chat),
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(stopped()),
            }
        }
        if !working.is_empty() {
            drop(held);
            return Err(self.quiet_busy(working, false).await);
        }
        Ok(held)
    }

    /// The refusal of a quiet change, naming only what blocks it: the chats working there
    /// (`working`), or another change rewriting the folders (`changing`). Idle agents and
    /// project terminals may stay beside a quiet change, so they are not named: "Stop these
    /// chats" stops only what is in its way.
    async fn quiet_busy(&self, mut working: Vec<String>, changing: bool) -> SessionError {
        working.sort();
        working.dedup();
        let blockers = FolderBlockers {
            chats: self.blockers(working).await,
            terminals: 0,
        };
        let message = busy_message(&blockers, changing);
        SessionError::FolderBusy { message, blockers }
    }

    /// Holds `folders` for an exclusive change: the agents there that are not working are
    /// stopped first and waited for, then the folders are reserved. `except` (a chat moving
    /// itself) is neither stopped nor reserved; `setup_chat` is reserved for its own setup.
    pub(super) async fn hold_exclusive(
        &self,
        folders: Vec<PathBuf>,
        except: Option<&str>,
        setup_chat: Option<&str>,
    ) -> Result<Reservations, SessionError> {
        let holders = self.exclusive_can_go(&folders, except, &[]).await?;
        if !holders.is_empty() {
            self.stop_unless_working(StopScope::Chats(holders)).await;
        }
        self.reserve_git_folders_with(folders, except, setup_chat)
            .await
    }

    /// Whether an exclusive change to `folders` can go ahead once the agents there that are not
    /// working stop. Refused now, naming what would still be in its way (a chat that works,
    /// waits for an answer, waits to start or is open in its terminal; a project terminal;
    /// another change), so nothing is stopped for a change that cannot happen. Answers the chats
    /// to stop first: those there, and `also` (a removed project's chats), but `except`.
    pub(super) async fn exclusive_can_go(
        &self,
        folders: &[PathBuf],
        except: Option<&str>,
        also: &[String],
    ) -> Result<Vec<String>, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let spellings = self.spellings(folders.to_vec()).await;
        let live = &self.shared().live;
        if live.quiescing_now() {
            return Err(SessionError::ShuttingDown);
        }
        let mut holders = live.holders(&spellings);
        holders.extend(also.iter().cloned());
        holders.retain(|chat| Some(chat.as_str()) != except);
        holders.sort();
        holders.dedup();
        let blocking: Vec<String> = holders
            .iter()
            .filter(|chat| matches!(live.doing(chat), Some(doing) if doing != Doing::Idle))
            .cloned()
            .collect();
        if !blocking.is_empty() || live.terminals_in(&spellings) > 0 || live.mutation_in(&spellings)
        {
            return Err(self.busy_naming(&spellings, blocking).await);
        }
        Ok(holders)
    }

    /// The chats saved in `folders` (by their resolved folder) and the folders resolved.
    pub(super) async fn chats_in(
        &self,
        folders: Vec<PathBuf>,
    ) -> Result<(Vec<PathBuf>, Vec<String>), SessionError> {
        let chats = self
            .deps()
            .store
            .chats()
            .await
            .map_err(|error| storage_error(&error))?;
        self.deps()
            .rt
            .spawn_blocking(move || {
                let mut folders = folders
                    .into_iter()
                    .map(live::normalize_path)
                    .collect::<Vec<_>>();
                folders.sort();
                folders.dedup();
                let chats = chats
                    .into_iter()
                    .filter(|chat| folders.contains(&live::normalize_path(chat.folder.clone())))
                    .map(|chat| chat.id)
                    .collect::<Vec<_>>();
                (folders, chats)
            })
            .await
            .map_err(|_| {
                SessionError::Organization("Could not resolve Git folder ownership.".into())
            })
    }

    /// The refusal of a change to `folders`: the chats working there (and `refused`, the ones
    /// that would not be held), the project terminals open there, or another change in flight.
    pub(super) async fn folder_busy(
        &self,
        folders: &[PathBuf],
        refused: Vec<String>,
        except: Option<&str>,
    ) -> SessionError {
        let spellings = self.spellings(folders.to_vec()).await;
        let mut chats = self.shared().live.holders(&spellings);
        chats.extend(refused);
        chats.retain(|chat| Some(chat.as_str()) != except);
        self.busy_naming(&spellings, chats).await
    }

    /// The refusal of a change to the folders `spellings` names, naming `chats` as the ones in
    /// its way, with the project terminals open there and any other change to them.
    pub(super) async fn busy_naming(
        &self,
        spellings: &[PathBuf],
        mut chats: Vec<String>,
    ) -> SessionError {
        chats.sort();
        chats.dedup();
        let live = &self.shared().live;
        let blockers = FolderBlockers {
            chats: self.blockers(chats).await,
            terminals: u32::try_from(live.terminals_in(spellings)).unwrap_or(u32::MAX),
        };
        let message = busy_message(&blockers, live.mutation_in(spellings));
        SessionError::FolderBusy { message, blockers }
    }

    /// `chats` as the person knows them, for a sentence: each one's title (or its agent's chat),
    /// quoted, and its project.
    pub async fn chat_names(&self, chats: Vec<String>) -> Vec<String> {
        self.blockers(chats)
            .await
            .into_iter()
            .map(|blocker| {
                let name = blocker
                    .title
                    .unwrap_or_else(|| format!("{} chat", blocker.agent_name));
                match blocker.project {
                    Some(project) => format!("\u{201c}{name}\u{201d} in {project}"),
                    None => format!("\u{201c}{name}\u{201d}"),
                }
            })
            .collect()
    }

    /// Each of `chats` as the person knows it: its title or agent, its project, what it does.
    async fn blockers(&self, chats: Vec<String>) -> Vec<FolderBlocker> {
        if chats.is_empty() {
            return Vec::new();
        }
        let organization = self
            .organization(OrganizationCommand::Snapshot(
                ghost_protocol::workspace::WorkspaceFilter {
                    include_archived: true,
                    project_id: None,
                },
            ))
            .await
            .ok();
        let live = &self.shared().live;
        chats
            .into_iter()
            .map(|chat_id| {
                let saved = organization.as_ref().and_then(|organization| {
                    organization
                        .chats
                        .iter()
                        .find(|entry| entry.chat.id == chat_id)
                });
                let project = saved.as_ref().and_then(|entry| {
                    organization.as_ref().and_then(|organization| {
                        organization
                            .projects
                            .iter()
                            .find(|project| project.id == entry.project_id)
                            .map(|project| project.name.clone())
                    })
                });
                let state = match live.doing(&chat_id) {
                    Some(Doing::Waiting) => BlockerState::Waiting,
                    Some(Doing::Terminal) => BlockerState::Terminal,
                    Some(Doing::Idle) => BlockerState::Idle,
                    Some(Doing::Working) | None => BlockerState::Working,
                };
                let (title, agent_name) = match saved {
                    Some(entry) => {
                        let summary =
                            self.summary(entry.chat.clone(), self.shared().state(&chat_id), None);
                        (summary.title, summary.agent_name)
                    }
                    None => (None, "An agent".to_owned()),
                };
                FolderBlocker {
                    chat_id,
                    title,
                    agent_name,
                    project,
                    state,
                }
            })
            .collect()
    }

    /// What is in the way of a change to the folders `scope` names: every chat with a live
    /// agent there (idle ones included, marked so) and the project terminals open there.
    pub async fn folder_blockers(
        &self,
        scope: FolderScope,
    ) -> Result<FolderBlockers, SessionError> {
        let folders: Vec<PathBuf> = match scope {
            FolderScope::Chat { chat_id } => {
                super::workspace::bounded(&chat_id, "Chat ID", 128)?;
                vec![self.chat_record(&chat_id).await?.folder]
            }
            FolderScope::Folder { folder_id } => self
                .chat_create_preview(&folder_id)
                .await?
                .folder
                .path
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            FolderScope::Project { project_id } => {
                super::workspace::bounded(&project_id, "Project ID", 128)?;
                self.organization(OrganizationCommand::Snapshot(
                    ghost_protocol::workspace::WorkspaceFilter {
                        include_archived: true,
                        project_id: Some(project_id.clone()),
                    },
                ))
                .await?
                .projects
                .into_iter()
                .filter(|project| project.id == project_id)
                .flat_map(|project| project.folders)
                .filter_map(|folder| folder.path.map(PathBuf::from))
                .collect()
            }
        };
        let spellings = self.spellings(folders).await;
        let live = &self.shared().live;
        Ok(FolderBlockers {
            chats: self.blockers(live.holders(&spellings)).await,
            terminals: u32::try_from(live.terminals_in(&spellings)).unwrap_or(u32::MAX),
        })
    }
}

/// The sentence that names what is in the way, and what to do about it.
pub(super) fn busy_message(blockers: &FolderBlockers, changing: bool) -> String {
    let mut sentences = Vec::new();
    if !blockers.chats.is_empty() {
        let items: Vec<String> = blockers
            .chats
            .iter()
            .map(|blocker| {
                let name = blocker
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("{} chat", blocker.agent_name));
                let place = blocker
                    .project
                    .as_ref()
                    .map(|project| format!(" in {project}"))
                    .unwrap_or_default();
                let doing = match blocker.state {
                    BlockerState::Working => "is working",
                    BlockerState::Waiting => "is waiting for your answer",
                    BlockerState::Terminal => "is open in a terminal",
                    BlockerState::Idle => "is open",
                };
                format!("\u{201c}{name}\u{201d}{place} {doing}")
            })
            .collect();
        let them = if items.len() == 1 { "it" } else { "them" };
        sentences.push(format!(
            "{} in this folder. Stop {them}, then try again.",
            join_and(&items)
        ));
    }
    match blockers.terminals {
        0 => {}
        1 => sentences
            .push("A project terminal is open in this folder. Close it, then try again.".into()),
        count => sentences.push(format!(
            "{count} project terminals are open in this folder. Close them, then try again."
        )),
    }
    if changing {
        sentences.push(
            "Another change to this folder is in progress. Wait for it to finish, then try again."
                .into(),
        );
    }
    if sentences.is_empty() {
        sentences
            .push("Something else is using this folder right now. Try again in a moment.".into());
    }
    sentences.join(" ")
}

/// `a`, `a and b`, `a, b and c`.
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
