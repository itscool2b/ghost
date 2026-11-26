//! Databases of the removed Ghost accounts were encrypted and carry a durable marker file
//! (`<database>.encryption`). This version cannot read them, so a marked database is refused
//! before any open: it never enters the plaintext recovery path, which would take its ciphertext
//! for damage and move it aside.
use super::*;

fn marker_path(path: &Path) -> PathBuf {
    fsutil::with_suffix(path, ".encryption")
}

pub(super) fn require_plain(path: &Path) -> Result<()> {
    match fs::symlink_metadata(marker_path(path)) {
        Ok(_) => Err(CoreError::EncryptedDatabase {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::io(marker_path(path))(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encryption_marker_blocks_opens_and_leaves_the_database_alone() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("aura.encrypted.db");
        let backups = temp.path().join("backups");
        // Even an unknown or damaged marker cannot send ciphertext into plaintext repair.
        fs::write(marker_path(&db), b"future format").unwrap();
        fs::write(&db, b"preserve encrypted bytes").unwrap();
        assert!(matches!(
            Store::open_at(&db, &backups),
            Err(CoreError::EncryptedDatabase { .. })
        ));
        assert!(matches!(
            super::super::check_integrity(&db),
            Err(CoreError::EncryptedDatabase { .. })
        ));
        assert_eq!(fs::read(&db).unwrap(), b"preserve encrypted bytes");
        assert!(!backups.exists());
    }
}
