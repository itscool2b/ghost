//! Resolves an Instructions target (global, a project folder, a worktree chat) to the folders
//! ghost-config reads, with installed agents and Git facts. Writes stay owned through the quit
//! and update drain, like every configuration change.

use std::path::{Path, PathBuf};

use ghost_config::instructions::{
    self, ApplyResult, Context, GitFacts, Prepared, ScopeKind, Target,
};
use ghost_process::EnvKey;
use ghost_protocol::worktree_setup::WorktreeSetup;

use super::{SessionError, SessionManager, storage_error};

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}

impl SessionManager {
    /// Everything a snapshot or a change needs. Reads only named variables and the chosen folder.
    pub async fn instructions_context(
        &self,
        target: &Target,
        state_dir: PathBuf,
    ) -> Result<Context, SessionError> {
        let env = self.deps().env.get().await;
        let variable = |name: &str| {
            env.vars
                .get(&EnvKey::from(name))
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = if cfg!(windows) {
            variable("USERPROFILE").or_else(|| variable("HOME"))
        } else {
            variable("HOME")
        }
        .filter(|path| path.is_absolute())
        .ok_or_else(|| invalid("The home folder is unavailable, so agent files can't be found."))?;
        let user_roots = instructions::user_roots(&home, variable);
        let mut installed = Vec::new();
        for agent in self.agents(false).await {
            if matches!(
                agent.availability,
                ghost_protocol::session::Availability::Ready { .. }
            ) && let Some(id) = instructions::agent_for_catalog(&agent.id)
                && !installed.iter().any(|known| known == id)
            {
                installed.push(id.to_owned());
            }
        }
        let codex_limit = user_roots
            .iter()
            .find(|root| root.id == "codex")
            .and_then(|root| instructions::codex_doc_limit(&root.path));
        let mut notices = Vec::new();
        let (scope, project) = match target {
            Target::Global => (ScopeKind::Global, None),
            Target::Folder { folder_id } => {
                let preview = self.chat_create_preview(folder_id).await?;
                let path = preview.folder.path.ok_or_else(|| {
                    invalid("This project folder isn't on this device. Locate it first.")
                })?;
                (ScopeKind::Project, Some(PathBuf::from(path)))
            }
            Target::Chat { chat_id } => {
                let chat = self.git_chat(chat_id).await?;
                (ScopeKind::Project, Some(chat.folder))
            }
        };
        if let Some(folder) = &project
            && !folder.is_dir()
        {
            return Err(invalid(
                "This folder is unavailable. Locate it before editing its instructions.",
            ));
        }
        let git = match (&project, self.worktrees()) {
            (Some(folder), Ok(worktrees)) => {
                match worktrees
                    .instruction_facts(folder, &instructions::project_paths())
                    .await
                {
                    Ok(facts) => facts,
                    Err(_) => {
                        notices.push("Git could not be asked about this folder, so files that aren't committed aren't flagged.".into());
                        GitFacts::default()
                    }
                }
            }
            _ => GitFacts::default(),
        };
        Ok(Context {
            scope,
            project,
            user_roots,
            home: Some(home),
            installed,
            git,
            state_dir,
            codex_limit,
            notices,
        })
    }

    /// Applies a reviewed change inside the maintenance owner, so a quit or update waits for it
    /// and an IPC cancellation cannot interrupt the transaction.
    pub async fn instructions_apply(
        &self,
        prepared: Prepared,
    ) -> Result<ApplyResult, SessionError> {
        let manager = self.clone();
        self.deps()
            .rt
            .spawn_blocking(move || {
                let _owner = manager
                    .shared()
                    .live
                    .try_maintenance()
                    .ok_or(SessionError::ShuttingDown)?;
                if manager.shared().is_shutting_down() {
                    return Err(SessionError::ShuttingDown);
                }
                instructions::apply(prepared).map_err(|error| invalid(error.to_string()))
            })
            .await
            .map_err(|_| {
                SessionError::Internal(
                    "The instruction change stopped unexpectedly. Its backups were kept.".into(),
                )
            })?
    }

    /// Adds one instruction file to the worktree setup of the target's project folder, so new
    /// worktree chats copy it in. Never copies anything by itself.
    pub async fn instructions_include_in_worktrees(
        &self,
        target: &Target,
        path: &str,
    ) -> Result<WorktreeSetup, SessionError> {
        if !instructions::project_paths().contains(&path) {
            return Err(invalid("Only an instruction file can be added here."));
        }
        let missing = || {
            invalid(format!(
                "{path} isn't in the project folder yet. Save it first."
            ))
        };
        let folder_id = match target {
            Target::Global => {
                return Err(invalid("User-level files are not part of a worktree."));
            }
            Target::Chat { chat_id } => {
                // The chat's recipe belongs to its source folder, resolved the usual way.
                let mut setup = self.worktree_setup(chat_id).await?;
                if !Path::new(&setup.source).join(path).is_file() {
                    return Err(missing());
                }
                if setup
                    .input
                    .files
                    .iter()
                    .any(|file| file.eq_ignore_ascii_case(path))
                {
                    return Ok(setup);
                }
                setup.input.files.push(path.to_owned());
                return self.worktree_setup_save(chat_id, setup.input).await;
            }
            Target::Folder { folder_id } => folder_id.clone(),
        };
        let preview = self.chat_create_preview(&folder_id).await?;
        let source = preview
            .folder
            .path
            .ok_or_else(|| invalid("This project folder isn't on this device."))?;
        if !Path::new(&source).join(path).is_file() {
            return Err(missing());
        }
        // Saving the folder's setup orders behind other changes to this folder only.
        let _organization = self
            .shared()
            .organization
            .scoped(vec![PathBuf::from(&source)])
            .await;
        let read = |save| crate::chatstore::worktree_setup::Command {
            chat_id: String::new(),
            source: source.clone(),
            save,
            folder_id: Some(folder_id.clone()),
            folder_save: true,
        };
        let mut setup = self
            .deps()
            .store
            .worktree_setup(read(None))
            .await
            .map_err(|error| storage_error(&error))?;
        if setup
            .input
            .files
            .iter()
            .any(|file| file.eq_ignore_ascii_case(path))
        {
            return Ok(setup);
        }
        setup.input.files.push(path.to_owned());
        crate::gitworkspaces::setup::validate(&setup.input)
            .map_err(|error| invalid(error.to_string()))?;
        self.deps()
            .store
            .worktree_setup(read(Some(setup.input)))
            .await
            .map_err(|error| storage_error(&error))
    }

    /// Commits one new, untracked instruction file in a project folder. Worktree chats commit on
    /// their own branch through the Git workspace instead.
    pub async fn instructions_commit(
        &self,
        target: &Target,
        path: &str,
    ) -> Result<String, SessionError> {
        let Target::Folder { folder_id } = target else {
            return Err(invalid(
                "Commit instruction files from the project folder itself.",
            ));
        };
        let preview = self.chat_create_preview(folder_id).await?;
        let folder = preview
            .folder
            .path
            .ok_or_else(|| invalid("This project folder isn't on this device."))?;
        let worktrees = self.worktrees()?;
        let folders = vec![PathBuf::from(&folder)];
        // Adding one new file to Git: agents in the folder only wait.
        let _reservation = self.hold_quiet(folders).await?;
        worktrees
            .commit_instruction_file(Path::new(&folder), path)
            .await
            .map_err(|error| invalid(error.to_string()))
    }
}
