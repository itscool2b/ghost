//! The profile owner: one Ghost process runs a profile's sessions.
//!
//! Recovery and sequence numbers assume one writer per chat: a second process that recovered the
//! first one's live turns would break its numbering and send its transcript to the dead-letter file.
//! So the process that runs sessions holds `<profile>/owner.lock` exclusively for as long as it runs,
//! taken right after the profile opens and before its store does. A second launch with the same
//! `GHOST_DATA_DIR`, or `ghost-cli --data-dir` while the app runs, fails with
//! [`CoreError::ProfileInUse`]. Only the owner starts the chat store, so only the owner runs
//! recovery, session actors and agent spawns. M0's integrity check reads through its own read-only
//! connection in the same process and is unaffected.
//!
//! The lock file is never deleted: removing a lock file is how two processes end up locking
//! different files. Where the filesystem cannot lock at all (some network mounts), acquiring
//! succeeds with a warning, as the store's open lock does.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::profile::Profile;

/// `<profile>/owner.lock`
const OWNER_LOCK: &str = "owner.lock";

/// This process owns the profile while the value lives.
#[derive(Debug)]
pub struct ProfileOwner {
    path: PathBuf,
    /// The locked file; `None` where the filesystem cannot lock. Dropping it releases the lock.
    _file: Option<ghost_process::locked_file::LockedFile>,
}

impl ProfileOwner {
    /// Takes `<profile>/owner.lock` exclusively. Held by another process (or another owner in this
    /// one) -> [`CoreError::ProfileInUse`]; a filesystem that cannot lock -> `Ok`, with a warning.
    pub fn acquire(profile: &Profile) -> Result<ProfileOwner> {
        let path = profile.dir.join(OWNER_LOCK);
        let file = open_lock_file(&path).map_err(CoreError::io(&path))?;
        match file.try_lock() {
            Ok(()) => Ok(ProfileOwner {
                path,
                _file: Some(ghost_process::locked_file::LockedFile::new(file)),
            }),
            Err(TryLockError::WouldBlock) => Err(CoreError::ProfileInUse {
                path: profile.dir.clone(),
            }),
            Err(TryLockError::Error(error)) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "this filesystem can't lock files; running without guarding the profile against a second Ghost process"
                );
                Ok(ProfileOwner { path, _file: None })
            }
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
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
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::paths::DataRoot;
    use crate::profile::Profiles;

    fn profile() -> (tempfile::TempDir, Profile) {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
            .open_active()
            .expect("profile");
        (dir, profile)
    }

    #[test]
    fn an_owner_holds_the_profile_until_it_drops() {
        let (_dir, profile) = profile();
        let owner = ProfileOwner::acquire(&profile).expect("first owner");
        assert_eq!(owner.path(), profile.dir.join("owner.lock"));
        assert!(owner.path().is_file());
        let second = ProfileOwner::acquire(&profile);
        let Err(CoreError::ProfileInUse { path }) = &second else {
            panic!("expected ProfileInUse, got {second:?}");
        };
        assert_eq!(path, &profile.dir);
        drop(owner);
        let again = ProfileOwner::acquire(&profile).expect("free once the owner dropped");
        assert!(again.path().is_file(), "the lock file is never deleted");
    }

    #[test]
    fn the_refusal_says_what_to_do() {
        let message = CoreError::ProfileInUse {
            path: PathBuf::from("/data/profiles/local"),
        }
        .to_string();
        assert_eq!(
            message,
            "Ghost is already running with the profile in /data/profiles/local. Quit it first, or start this copy with its own GHOST_DATA_DIR."
        );
    }

    /// Marks the child copy of the next test; holds the data root it should own.
    const CHILD_OWNS: &str = "GHOST_CORE_TEST_OWNER_CHILD_ROOT";

    /// Where the child writes a file once it owns the profile. A file, not its stdout: the test
    /// harness captures a test's output.
    const CHILD_SIGNALS: &str = "GHOST_CORE_TEST_OWNER_CHILD_MARKER";

    /// The real case: another process owns the profile. The test binary runs itself as that other
    /// process, which holds the lock until its stdin closes.
    #[test]
    fn a_second_process_is_refused_while_the_first_owns_the_profile() {
        if let (Some(root), Some(marker)) = (
            std::env::var_os(CHILD_OWNS),
            std::env::var_os(CHILD_SIGNALS),
        ) {
            let profile = Profiles::new(DataRoot::at(root))
                .open_active()
                .expect("child profile");
            let owner = ProfileOwner::acquire(&profile).expect("the child owns the profile");
            std::fs::write(&marker, b"owned").expect("signal the parent");
            let mut rest = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut rest);
            drop(owner);
            return;
        }

        let (dir, profile) = profile();
        let marker = dir.path().join("child-owns");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "owner::tests::a_second_process_is_refused_while_the_first_owns_the_profile",
                "--test-threads=1",
            ])
            .env(CHILD_OWNS, dir.path().join("data"))
            .env(CHILD_SIGNALS, &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn the child");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !marker.exists() {
            if Instant::now() > deadline || child.try_wait().ok().flatten().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child never took the owner lock");
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let refused = ProfileOwner::acquire(&profile);
        assert!(
            matches!(&refused, Err(CoreError::ProfileInUse { path }) if path == &profile.dir),
            "{refused:?}"
        );

        drop(child.stdin.take());
        let status = child.wait().expect("wait for the child");
        assert!(status.success(), "the child failed: {status}");
        ProfileOwner::acquire(&profile).expect("free once the other process exited");
    }
}
