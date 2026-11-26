//! Advisory file locks that end with their owner, even while a spawn is in flight.

use std::fs::File;
use std::ops::Deref;

/// A file this process has locked with `File::try_lock` or `File::try_lock_shared`, unlocked
/// explicitly when dropped.
///
/// File locks belong to the open file description. A child forked by any thread shares that
/// description until its exec, so closing this descriptor alone could leave the lock held after
/// its owner is gone, and the next acquisition in this process would find it taken. Unlocking
/// releases it for every descriptor at once.
#[derive(Debug)]
pub struct LockedFile(File);

impl LockedFile {
    /// Takes ownership of a file whose lock this process acquired.
    pub fn new(file: File) -> LockedFile {
        LockedFile(file)
    }
}

impl Deref for LockedFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        // A file system without locks has nothing to release.
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::LockedFile;
    use std::fs::OpenOptions;

    /// `try_clone` duplicates the descriptor onto the same open file description, as fork does.
    #[test]
    fn an_inherited_descriptor_does_not_outlive_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("owner.lock");
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .expect("lock file")
        };
        let file = open();
        file.try_lock().expect("first lock");
        let inherited = file.try_clone().expect("dup");
        drop(LockedFile::new(file));
        open()
            .try_lock()
            .expect("released despite the inherited descriptor");
        drop(inherited);
    }
}
