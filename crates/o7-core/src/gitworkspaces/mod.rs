//! Explicit local worktree lifecycle. No operation calls an agent or infers user confirmation.

mod actions;
mod branch_diff;
mod commit;
mod create;
mod git;
mod identity;
mod initialize;
mod inspect;
mod instructions;
mod new_chat;
pub(crate) use new_chat::{NewChatPlan, NewChatRequest};
pub(crate) use records::unchanged_since_review;
#[cfg(test)]
mod new_chat_tests;
mod publication;
mod pull_request;

pub use branch_diff::{BranchChanges, BranchDiff, BranchFile};
pub use pull_request::{Checks, Note, PullRequestView};
mod publish;
mod records;
mod recovery;
mod recreate;
#[cfg(test)]
mod recreate_tests;
pub(crate) mod run;
pub(crate) mod setup;
pub(crate) use identity::FolderIdentity;
mod setup_files;
#[cfg(test)]
mod tests;
mod transfer;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use o7_process::{EnvResolver, LeftoverRegistry};
use o7_protocol::git::GitWorkspace;

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{0}")]
    Message(String),
    #[error("The workspace metadata could not be read or saved: {0}")]
    Io(#[from] std::io::Error),
    #[error("The workspace record could not be read or saved: {0}")]
    Core(#[from] crate::error::CoreError),
}

pub struct GitWorkspaces {
    env: Arc<EnvResolver>,
    registry: Arc<LeftoverRegistry>,
    root: PathBuf,
    /// Held only around one read-modify-write of `registry.json` or `publications.json`, never
    /// across an await. Reads take nothing: each file is replaced atomically.
    records: std::sync::Mutex<()>,
    /// One change at a time per repository (keyed by its Git common directory, which its
    /// worktrees share). Other repositories' changes run at once; previews and reads wait for none.
    repositories: crate::keyed_lock::KeyedLock<PathBuf>,
    prepared: std::sync::Mutex<HashMap<String, create::Prepared>>,
    actions: std::sync::Mutex<HashMap<String, actions::PreparedAction>>,
    recoveries: std::sync::Mutex<HashMap<String, recovery::PreparedRecovery>>,
    new_chats: std::sync::Mutex<HashMap<String, new_chat::Prepared>>,
    recreations: std::sync::Mutex<HashMap<String, recreate::Prepared>>,
    pull_requests: std::sync::Mutex<HashMap<String, publish::PreparedPullRequest>>,
    setups: std::sync::Mutex<HashMap<String, setup::Prepared>>,
    initializations: std::sync::Mutex<HashMap<String, initialize::Prepared>>,
    publications: std::sync::Mutex<HashMap<String, publication::Prepared>>,
    commits: std::sync::Mutex<HashMap<String, commit::PreparedCommit>>,
    pushes: std::sync::Mutex<HashMap<String, commit::PreparedPush>>,
}

impl GitWorkspaces {
    pub fn new(
        profile: &crate::profile::Profile,
        env: Arc<EnvResolver>,
        registry: Arc<LeftoverRegistry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            env,
            registry,
            root: profile.worktrees_dir(),
            records: std::sync::Mutex::new(()),
            repositories: crate::keyed_lock::KeyedLock::new(),
            prepared: std::sync::Mutex::new(HashMap::new()),
            actions: std::sync::Mutex::new(HashMap::new()),
            recoveries: std::sync::Mutex::new(HashMap::new()),
            new_chats: std::sync::Mutex::new(HashMap::new()),
            recreations: std::sync::Mutex::new(HashMap::new()),
            pull_requests: std::sync::Mutex::new(HashMap::new()),
            setups: std::sync::Mutex::new(HashMap::new()),
            initializations: std::sync::Mutex::new(HashMap::new()),
            publications: std::sync::Mutex::new(HashMap::new()),
            commits: std::sync::Mutex::new(HashMap::new()),
            pushes: std::sync::Mutex::new(HashMap::new()),
        })
    }

    async fn git(&self) -> Result<git::Git> {
        crate::fsutil::ensure_private_dir(&self.root)?;
        let hooks = self.root.join("empty-hooks");
        crate::fsutil::ensure_private_dir(&hooks)?;
        if std::fs::read_dir(&hooks)?.next().is_some() {
            return Err(GitError::Message("The app's empty Git hook directory contains unexpected files. Worktree operations are unavailable until it is reviewed.".into()));
        }
        git::Git::new(&self.env, Arc::clone(&self.registry), hooks).await
    }

    pub async fn inspect(&self, folder: &Path) -> Result<GitWorkspace> {
        Ok(inspect::inspect(&self.git().await?, folder).await?.view)
    }

    /// Holds the repository `folder` belongs to for one change, behind the changes to it that
    /// arrived first. A folder Git cannot place (no repository yet, gone) is its own key.
    async fn hold_repository(
        &self,
        git: &git::Git,
        folder: &Path,
    ) -> crate::keyed_lock::KeyedGuard {
        let key = match inspect::common_dir(git, folder).await {
            Some(common) => common,
            None => folder
                .canonicalize()
                .unwrap_or_else(|_| folder.to_path_buf()),
        };
        self.repositories.lock(key).await
    }

    /// Tests: holds `folder`'s repository as a long Git change would.
    #[cfg(test)]
    pub(crate) async fn hold_repository_for_test(
        &self,
        folder: &Path,
    ) -> crate::keyed_lock::KeyedGuard {
        let git = self.git().await.expect("git");
        self.hold_repository(&git, folder).await
    }

    /// Holds the repository whose common directory an earlier inspection found.
    async fn hold_common(&self, common: &Path) -> crate::keyed_lock::KeyedGuard {
        self.repositories.lock(common.to_path_buf()).await
    }

    /// The short lock around one read-modify-write of this profile's records.
    fn records_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
