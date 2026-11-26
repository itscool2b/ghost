//! The open lock: an advisory lock on `<db>.lock` that coordinates every Ghost process using a
//! profile database.
//!
//! - [`Store::open`](super::Store::open) takes it exclusively for the whole open: probe, repair,
//!   WAL switch, migration. Two processes creating a profile at the same moment would otherwise both
//!   try to switch a fresh file to WAL (one gets SQLITE_BUSY at once) and both run migration 1.
//! - An open store keeps a shared hold for as long as it lives. Another process can still open the
//!   database alongside it (the CLI, a second window), but it gets only a shared hold, and an open
//!   with a shared hold never moves or migrates the file: that would pull the database out from
//!   under a process that has it open.
//!
//! The lock file is never deleted; removing a lock file is how two processes end up locking
//! different files. Where the filesystem can't lock at all (some network mounts), opening carries on
//! without the lock, as before it existed, and SQLite's own locking is the only guard.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ghost_process::locked_file::LockedFile;

use crate::error::{CoreError, InUseReason, Result};
use crate::fsutil;

/// How often a waiting open checks the lock again.
pub(super) const POLL: Duration = Duration::from_millis(20);

/// What this process holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hold {
    /// No one else has the database open or is opening it: this open may repair and migrate.
    Exclusive,
    /// Other processes have the database open: this open may use it but not move or migrate it.
    Shared,
    /// The filesystem refused to lock: no coordination, so this open behaves as if exclusive.
    Unavailable,
}

/// Locks are released explicitly, so a child forked during a spawn elsewhere in this process
/// cannot keep a finished open or a closed store looking active to the next open here.
#[derive(Debug)]
pub(super) struct OpenLock {
    file: Option<LockedFile>,
    hold: Hold,
}

/// The shared hold an open store keeps for its whole life.
pub(super) type StoreHold = LockedFile;

/// `<db>.lock`
pub(super) fn lock_path(db_path: &Path) -> PathBuf {
    fsutil::with_suffix(db_path, ".lock")
}

impl OpenLock {
    /// Takes the lock exclusively if no one else holds it, or shared if other processes only have
    /// the database open. Waits up to `timeout` while another process is in the middle of opening
    /// it, then gives up with [`InUseReason::Opening`].
    pub(super) fn acquire(db_path: &Path, timeout: Duration) -> Result<OpenLock> {
        let path = lock_path(db_path);
        let file = open_lock_file(&path).map_err(CoreError::io(&path))?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(OpenLock::held(file, Hold::Exclusive)),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => return Ok(OpenLock::unavailable(&path, error)),
            }
            // Someone holds it. A shared hold succeeds only if no one is opening it right now.
            match file.try_lock_shared() {
                Ok(()) => return Ok(OpenLock::held(file, Hold::Shared)),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => return Ok(OpenLock::unavailable(&path, error)),
            }
            if Instant::now() >= deadline {
                return Err(CoreError::DatabaseInUse {
                    path: db_path.to_path_buf(),
                    reason: InUseReason::Opening,
                });
            }
            std::thread::sleep(POLL);
        }
    }

    fn held(file: File, hold: Hold) -> OpenLock {
        OpenLock {
            file: Some(LockedFile::new(file)),
            hold,
        }
    }

    fn unavailable(path: &Path, error: io::Error) -> OpenLock {
        tracing::warn!(
            path = %path.display(),
            %error,
            "this filesystem can't lock files; opening the database without coordinating with other processes"
        );
        OpenLock {
            file: None,
            hold: Hold::Unavailable,
        }
    }

    #[cfg(test)]
    pub(super) fn hold(&self) -> Hold {
        self.hold
    }

    /// Whether this open may move, replace, or migrate the database.
    pub(super) fn may_change(&self) -> bool {
        self.hold != Hold::Shared
    }

    /// After a successful open: trades an exclusive hold for the shared one an open store keeps for
    /// its whole life. The trade is not atomic (std can't downgrade in place), so another opener
    /// may slip in between; it then finds a healthy, migrated database and leaves it alone. Returns
    /// `None` when there is nothing to keep (no locking here) or the shared hold can't be had, in
    /// which case the store still works, just without protection from repairs by others.
    pub(super) fn into_store_hold(self, timeout: Duration) -> Option<StoreHold> {
        let file = self.file?;
        match self.hold {
            Hold::Shared => Some(file),
            Hold::Unavailable => None,
            Hold::Exclusive => {
                if let Err(error) = file.unlock() {
                    tracing::warn!(%error, "could not release the exclusive open lock");
                    return None;
                }
                let deadline = Instant::now() + timeout;
                loop {
                    match file.try_lock_shared() {
                        Ok(()) => return Some(file),
                        Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                            std::thread::sleep(POLL);
                        }
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                "could not keep a shared hold on the open lock; other processes won't see this store as open"
                            );
                            return None;
                        }
                    }
                }
            }
        }
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("aura.db");
        (dir, db)
    }

    #[test]
    fn the_first_open_is_exclusive_and_later_ones_share() {
        let (_dir, db) = db();
        let first = OpenLock::acquire(&db, Duration::from_millis(100)).expect("first");
        assert_eq!(first.hold(), Hold::Exclusive);
        assert!(first.may_change());
        assert!(lock_path(&db).is_file());

        let kept = first
            .into_store_hold(Duration::from_millis(100))
            .expect("a shared hold for the store");
        let second = OpenLock::acquire(&db, Duration::from_millis(100)).expect("second");
        assert_eq!(second.hold(), Hold::Shared);
        assert!(!second.may_change());

        drop(second);
        drop(kept);
        let third = OpenLock::acquire(&db, Duration::from_millis(100)).expect("third");
        assert_eq!(
            third.hold(),
            Hold::Exclusive,
            "the store's hold went with it"
        );
    }

    #[test]
    fn an_open_in_progress_makes_others_wait_then_give_up() {
        let (_dir, db) = db();
        let opening = OpenLock::acquire(&db, Duration::from_millis(100)).expect("first");
        assert_eq!(opening.hold(), Hold::Exclusive);
        let started = Instant::now();
        let waited = OpenLock::acquire(&db, Duration::from_millis(150));
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(
            matches!(
                waited,
                Err(CoreError::DatabaseInUse {
                    reason: InUseReason::Opening,
                    ..
                })
            ),
            "{waited:?}"
        );
        drop(opening);
    }

    #[test]
    fn a_waiting_open_proceeds_when_the_other_finishes() {
        let (_dir, db) = db();
        let opening = OpenLock::acquire(&db, Duration::from_millis(100)).expect("first");
        let path = db.clone();
        let started = Instant::now();
        let waiter = std::thread::spawn(move || {
            OpenLock::acquire(&path, Duration::from_secs(5))
                .map(|lock| (lock.hold(), started.elapsed()))
        });
        std::thread::sleep(Duration::from_millis(100));
        // That open ended without keeping a store (it failed, say), so nothing is held any more.
        drop(opening);
        let (hold, waited) = waiter.join().expect("thread").expect("second");
        assert_eq!(hold, Hold::Exclusive);
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
    }

    /// A child forked by another thread holds a copy of the lock descriptor until its exec.
    /// `try_clone` duplicates the descriptor the same way, onto the same open file description.
    #[test]
    fn an_inherited_descriptor_does_not_keep_a_closed_store_locked() {
        let (_dir, db) = db();
        let opening = OpenLock::acquire(&db, Duration::from_millis(100)).expect("first");
        let inherited = opening
            .file
            .as_ref()
            .expect("lock file")
            .try_clone()
            .expect("dup");
        let kept = opening
            .into_store_hold(Duration::from_millis(100))
            .expect("a shared hold for the store");
        let child = kept.try_clone().expect("dup");
        drop(kept);
        let next = OpenLock::acquire(&db, Duration::from_millis(100)).expect("next");
        assert_eq!(
            next.hold(),
            Hold::Exclusive,
            "a closed store may migrate its database"
        );
        drop((inherited, child));
    }
}
