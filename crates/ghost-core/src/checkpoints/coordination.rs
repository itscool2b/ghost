//! Checkpoint operations serialize overlapping workspaces and shared folder storage only.
//!
//! A caller owns cancellation; workers own cloned contexts. Dropping the caller cancels future
//! work without releasing its reservation out from under a blocking worker. Workers that use
//! scratch files must also retain the operation's shared scratch-directory owner.
//! Session admission is acquired before this coordinator, never from inside a checkpoint lease.

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::watch;

use super::{CheckpointError, Result};

#[derive(Clone)]
struct Claim {
    id: String,
    storage: PathBuf,
    folder: String,
    root: PathBuf,
}

impl Claim {
    fn conflicts(&self, other: &Self) -> bool {
        (self.storage == other.storage && self.folder == other.folder)
            || self.root.starts_with(&other.root)
            || other.root.starts_with(&self.root)
    }
}

#[derive(Default)]
struct State {
    active: Vec<Claim>,
    waiting: VecDeque<Claim>,
}

struct Coordinator {
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            changed: watch::channel(0).0,
        }
    }
}

impl Coordinator {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn acquire(self: &Arc<Self>, storage: &Path, folder: &str, root: &Path) -> Owner {
        let claim = Claim {
            id: crate::ids::new_id(),
            storage: normalize(storage.to_owned()),
            folder: folder.into(),
            root: normalize(root.to_owned()),
        };
        let mut changed = self.changed.subscribe();
        let reservation = Reservation {
            coordinator: Arc::clone(self),
            id: claim.id.clone(),
        };
        self.lock().waiting.push_back(claim);
        loop {
            if self.admit(&reservation.id) {
                return Owner {
                    context: Context(Arc::new(ContextInner {
                        _reservation: reservation,
                        cancelled: AtomicBool::new(false),
                    })),
                };
            }
            // The reservation retains the sender, so its notification channel cannot close.
            let _ = changed.changed().await;
        }
    }

    fn admit(&self, id: &str) -> bool {
        let mut state = self.lock();
        let Some(index) = state.waiting.iter().position(|claim| claim.id == id) else {
            return false;
        };
        let claim = &state.waiting[index];
        if state.active.iter().any(|active| active.conflicts(claim))
            || state
                .waiting
                .iter()
                .take(index)
                .any(|earlier| earlier.conflicts(claim))
        {
            return false;
        }
        if let Some(claim) = state.waiting.remove(index) {
            state.active.push(claim);
            true
        } else {
            false
        }
    }
}

struct Reservation {
    coordinator: Arc<Coordinator>,
    id: String,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        {
            let mut state = self.coordinator.lock();
            state.active.retain(|claim| claim.id != self.id);
            state.waiting.retain(|claim| claim.id != self.id);
        }
        self.coordinator
            .changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

/// Cancels when its enclosing future is dropped. Workers retain contexts, never owners.
pub(crate) struct Owner {
    context: Context,
}

impl Owner {
    pub(super) fn context(&self) -> Context {
        self.context.clone()
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.context.0.cancelled.store(true, Ordering::Release);
    }
}

struct ContextInner {
    _reservation: Reservation,
    cancelled: AtomicBool,
}

/// Retain this in every blocking closure until its reads, writes and recovery have finished.
#[derive(Clone)]
pub(super) struct Context(Arc<ContextInner>);

impl Context {
    pub(super) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub(super) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(CheckpointError::Message(
                "The checkpoint operation was cancelled.".into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// The canonical root must be resolved before admission and revalidated by the operation.
pub(super) async fn acquire(storage: &Path, folder: &str, root: &Path) -> Result<Owner> {
    // UUID spelling is not a storage identity. In particular, Windows opens upper/lowercase
    // spellings of the same `<folder>.git` name as one repository. Preserve caller IDs elsewhere.
    let folder = uuid::Uuid::parse_str(folder)
        .map_err(|_| CheckpointError::Message("Invalid checkpoint identity.".into()))?
        .to_string();
    static COORDINATOR: OnceLock<Arc<Coordinator>> = OnceLock::new();
    Ok(COORDINATOR
        .get_or_init(|| Arc::new(Coordinator::default()))
        .acquire(storage, &folder, root)
        .await)
}

// Match session admission's conservative path identity, including not-yet-created storage paths.
fn normalize(path: PathBuf) -> PathBuf {
    let path = path
        .ancestors()
        .find_map(|ancestor| {
            std::fs::canonicalize(ancestor)
                .ok()
                .and_then(|root| path.strip_prefix(ancestor).ok().map(|tail| root.join(tail)))
        })
        .unwrap_or(path);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    #[cfg(windows)]
    {
        fold_windows_path(normalized)
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // The invariant Windows casing API has no safe standard-library equivalent.
fn fold_windows_path(path: PathBuf) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Globalization::{
        LCMAP_UPPERCASE, LCMapStringEx, LOCALE_NAME_INVARIANT,
    };

    let source = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let Ok(length) = i32::try_from(source.len()) else {
        return path;
    };
    // SAFETY: source is live, length is explicit, and this call only queries the output length.
    let required = unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if required <= 0 {
        let mut fallback = path.into_os_string();
        fallback.make_ascii_uppercase();
        return fallback.into();
    }
    let mut output = vec![0_u16; required as usize];
    // SAFETY: the allocated output has the queried length and both buffers remain live.
    let written = unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            output.as_mut_ptr(),
            required,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if written <= 0 {
        return path;
    }
    output.truncate(written as usize);
    std::ffi::OsString::from_wide(&output).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[tokio::test]
    async fn disjoint_folders_run_while_parent_child_and_same_storage_wait() {
        let coordinator = Arc::new(Coordinator::default());
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path().join("storage");
        let root = temp.path().join("work");
        let first = coordinator.acquire(&storage, "folder-a", &root).await;
        let disjoint_root = temp.path().join("other");
        let disjoint = coordinator
            .acquire(&storage, "folder-b", &disjoint_root)
            .now_or_never()
            .expect("disjoint workspace and storage are admitted");
        let child_root = root.join("child");
        let mut child = Box::pin(coordinator.acquire(&storage, "folder-c", &child_root));
        assert!(child.as_mut().now_or_never().is_none());
        let moved_root = temp.path().join("moved");
        let mut same_storage = Box::pin(coordinator.acquire(&storage, "folder-a", &moved_root));
        assert!(same_storage.as_mut().now_or_never().is_none());
        drop(first);
        let child = child.await;
        let same_storage = same_storage.await;
        drop((child, same_storage, disjoint));
        assert!(coordinator.lock().active.is_empty());
    }

    #[tokio::test]
    async fn conflicting_waiters_keep_fifo_and_cancelled_waiters_release_their_place() {
        let coordinator = Arc::new(Coordinator::default());
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path().join("storage");
        let root = temp.path().join("work");
        let active = coordinator.acquire(&storage, "a", &root).await;
        let child = root.join("child");
        let mut earlier = Box::pin(coordinator.acquire(&storage, "b", &child));
        let mut later = Box::pin(coordinator.acquire(&storage, "c", &root));
        assert!(earlier.as_mut().now_or_never().is_none());
        assert!(later.as_mut().now_or_never().is_none());
        drop(active);
        assert!(later.as_mut().now_or_never().is_none(), "no overtaking");
        drop(earlier);
        let later = later.await;
        assert!(coordinator.lock().waiting.is_empty());
        drop(later);
    }

    #[tokio::test]
    async fn cancelled_owner_keeps_its_lease_until_blocking_worker_exits() {
        let coordinator = Arc::new(Coordinator::default());
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path().join("storage");
        let root = temp.path().join("work");
        let owner = coordinator.acquire(&storage, "a", &root).await;
        let context = owner.context();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            assert!(context.check().is_err());
        });
        started.await.unwrap();
        drop(owner);
        let mut next = Box::pin(coordinator.acquire(&storage, "b", &root));
        assert!(next.as_mut().now_or_never().is_none());
        release.send(()).unwrap();
        worker.await.unwrap();
        drop(next.await);
        assert!(coordinator.lock().active.is_empty());
    }

    #[test]
    fn path_identity_resolves_existing_ancestors_and_parent_components() {
        let temp = tempfile::tempdir().unwrap();
        let plain = temp.path().join("new");
        let alias = temp.path().join("unused").join("..").join("new");
        assert_eq!(normalize(plain), normalize(alias));
    }

    #[tokio::test]
    async fn shared_coordinator_serializes_the_same_workspace_across_profile_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let first_storage = root.join("profile-a");
        let other_storage = root.join("profile-b");
        let first_id = crate::ids::new_id();
        let other_id = crate::ids::new_id();
        let first = acquire(&first_storage, &first_id, root).await.unwrap();
        let mut second = Box::pin(acquire(&other_storage, &other_id, root));
        assert!(second.as_mut().now_or_never().is_none());
        drop(first);
        drop(second.await.unwrap());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_uuid_case_aliases_share_storage_even_for_disjoint_roots() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path().join("storage");
        let first_root = temp.path().join("first");
        let second_root = temp.path().join("second");
        let folder = "123e4567-e89b-12d3-a456-426614174000";
        let alias = folder.to_ascii_uppercase();
        let first = acquire(&storage, folder, &first_root).await.unwrap();
        let mut second = Box::pin(acquire(&storage, &alias, &second_root));
        assert!(second.as_mut().now_or_never().is_none());
        drop(first);
        drop(second.await.unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_identity_folds_case_without_losing_unicode() {
        assert_eq!(
            normalize(PathBuf::from(r"C:\checkpoint-test-Ä\NEW")),
            normalize(PathBuf::from(r"c:\CHECKPOINT-TEST-ä\new"))
        );
    }
}
