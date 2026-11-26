//! The order of changes to projects, chats and their folders.
//!
//! A change to some folders ([`FolderLocks::scoped`]) waits only for changes to the same folders
//! that arrived first, so work in one folder never waits for another folder's: a slow worktree
//! creation in one project does not hold up a chat, a terminal or a file link in another. A change
//! that can touch any folder ([`FolderLocks::exclusive`]: removing projects or folders, locating
//! one) waits for every change in flight and holds back the ones after it.
//!
//! These locks only order work. Which paths overlap, and whether an agent or a terminal is in one,
//! is decided atomically by `LiveAgents::reserve_folders`, which every folder mutation still takes.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use super::live;
use crate::keyed_lock::{KeyedGuard, KeyedLock};

pub(crate) struct FolderLocks {
    gate: Arc<RwLock<()>>,
    folders: KeyedLock<PathBuf>,
}

/// One read of the folders a change holds, and what it found there.
pub(super) type FolderRead<T> =
    futures::future::BoxFuture<'static, Result<(Vec<PathBuf>, T), super::SessionError>>;

/// Held for one change; the folders are released before the gate.
#[must_use = "the folders are released as soon as the guard is dropped"]
pub(crate) struct FolderGuard {
    _folders: Vec<KeyedGuard>,
    _gate: Gate,
}

enum Gate {
    Shared(#[allow(dead_code)] OwnedRwLockReadGuard<()>),
    Exclusive(#[allow(dead_code)] OwnedRwLockWriteGuard<()>),
}

impl FolderLocks {
    pub(crate) fn new() -> Self {
        Self {
            gate: Arc::new(RwLock::new(())),
            folders: KeyedLock::new(),
        }
    }

    /// Waits for the changes to `folders` that arrived first, and for an exclusive change in
    /// flight. Spellings of one folder share a key (symbolic links, and letter case on Windows).
    pub(crate) async fn scoped(&self, folders: Vec<PathBuf>) -> FolderGuard {
        self.scoped_by(folders, live::normalize_path).await
    }

    /// [`Self::scoped`], with each folder resolved by `resolve` before the gate is taken.
    /// Resolving reads the disk, which can hang on a network mount: holding the gate meanwhile
    /// would hold up a change waiting for every folder, and, as new changes queue behind a
    /// waiting one, every other folder's change in every project after it.
    async fn scoped_by(
        &self,
        folders: Vec<PathBuf>,
        resolve: impl Fn(PathBuf) -> PathBuf + Send + 'static,
    ) -> FolderGuard {
        let given = folders.clone();
        let keys = tokio::task::spawn_blocking(move || {
            folders.into_iter().map(resolve).collect::<Vec<_>>()
        })
        .await
        .unwrap_or(given);
        let gate = Arc::clone(&self.gate).read_owned().await;
        FolderGuard {
            _folders: self.folders.lock_all(keys).await,
            _gate: Gate::Shared(gate),
        }
    }

    /// Waits for every change in flight; changes that arrive later wait for this one.
    pub(crate) async fn exclusive(&self) -> FolderGuard {
        FolderGuard {
            _folders: Vec::new(),
            _gate: Gate::Exclusive(Arc::clone(&self.gate).write_owned().await),
        }
    }
}

impl super::SessionManager {
    /// Holds the folders `read` names, then reads them again: a change that moved them
    /// meanwhile (a chat moved, a worktree relocated) is waited for and its new folders held
    /// instead. Gives up, saying so, if they keep moving.
    pub(super) async fn hold_folders<T: Send>(
        &self,
        read: impl Fn() -> FolderRead<T> + Send + Sync,
    ) -> Result<(FolderGuard, T), super::SessionError> {
        let set = |mut folders: Vec<PathBuf>| {
            folders.sort();
            folders.dedup();
            folders
        };
        let mut folders = set(read().await?.0);
        for _ in 0..3 {
            let guard = self.shared().organization.scoped(folders.clone()).await;
            let (current, value) = read().await?;
            let current = set(current);
            if current == folders {
                return Ok((guard, value));
            }
            drop(guard);
            folders = current;
        }
        Err(super::SessionError::Organization(
            "These folders changed while this waited for another change to them. Try again.".into(),
        ))
    }

    /// Holds the chat's folder and `also`, with the chat as it is once they are held.
    pub(super) async fn hold_chat_folder(
        &self,
        chat_id: &str,
        also: &[PathBuf],
    ) -> Result<(FolderGuard, crate::chatstore::ChatRecord), super::SessionError> {
        let (manager, chat_id, also) = (self.clone(), chat_id.to_owned(), also.to_vec());
        self.hold_folders(move || {
            let (manager, chat_id, mut folders) = (manager.clone(), chat_id.clone(), also.clone());
            Box::pin(async move {
                let chat = manager.chat_record(&chat_id).await?;
                folders.push(chat.folder.clone());
                Ok((folders, chat))
            })
        })
        .await
    }

    /// The chat's saved record, archived chats included.
    pub(super) async fn chat_record(
        &self,
        chat_id: &str,
    ) -> Result<crate::chatstore::ChatRecord, super::SessionError> {
        self.deps()
            .store
            .checkpoint_context(chat_id, None)
            .await
            .map(|(chat, _)| chat)
            .map_err(|error| super::actor::storage_error(&error))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_folder_slow_to_resolve_holds_no_one_up() {
        let locks = Arc::new(FolderLocks::new());
        let (resolving, resolved) = std::sync::mpsc::channel::<()>();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Mutex::new(wait);
        // A folder on a network mount that does not answer, until it does.
        let slow = tokio::spawn({
            let locks = Arc::clone(&locks);
            async move {
                locks
                    .scoped_by(vec![PathBuf::from("/mnt/share")], move |path| {
                        let _ = resolving.send(());
                        let _ = wait.lock().map(|wait| wait.recv());
                        path
                    })
                    .await
            }
        });
        tokio::task::spawn_blocking(move || resolved.recv())
            .await
            .unwrap()
            .unwrap();
        // A change that waits for every folder, and one to another folder, go meanwhile.
        let every = tokio::time::timeout(Duration::from_secs(2), locks.exclusive())
            .await
            .expect("an exclusive change does not wait for a folder still resolving");
        drop(every);
        let other = tempfile::tempdir().unwrap();
        drop(
            tokio::time::timeout(
                Duration::from_secs(2),
                locks.scoped(vec![other.path().to_path_buf()]),
            )
            .await
            .expect("another folder's change does not wait for it"),
        );
        release.send(()).unwrap();
        drop(
            tokio::time::timeout(Duration::from_secs(2), slow)
                .await
                .unwrap()
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn folders_wait_only_for_themselves_and_exclusive_waits_for_all() {
        let locks = Arc::new(FolderLocks::new());
        let temp = tempfile::tempdir().unwrap();
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        std::fs::create_dir(&one).unwrap();
        std::fs::create_dir(&two).unwrap();
        let held = locks.scoped(vec![one.clone()]).await;
        // Another folder goes at once; the same folder, spelled another way, waits.
        drop(
            tokio::time::timeout(Duration::from_secs(1), locks.scoped(vec![two.clone()]))
                .await
                .expect("another folder does not wait"),
        );
        let spelled = one.join("..").join("one");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), locks.scoped(vec![spelled]))
                .await
                .is_err(),
            "the same folder waits"
        );
        let exclusive = {
            let locks = Arc::clone(&locks);
            tokio::spawn(async move { drop(locks.exclusive().await) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !exclusive.is_finished(),
            "exclusive waits for the change in flight"
        );
        drop(held);
        tokio::time::timeout(Duration::from_secs(1), exclusive)
            .await
            .expect("exclusive runs once the folders are free")
            .unwrap();
    }
}
