//! The writer's external files: one profile's attachment blobs and its backups folder.
use super::blobs::{self, ReadError};
use ghost_protocol::session::BlobText;
use std::{io, path::PathBuf};

/// `<profile>/attachments`, where blobs are stored by content hash.
#[derive(Debug, Clone)]
pub(crate) struct Attachments(PathBuf);

impl Attachments {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self(dir)
    }
    pub(crate) fn write(&self, hash: &str, bytes: &[u8]) -> io::Result<u64> {
        blobs::write(&self.0, hash, bytes)
    }
    pub(crate) fn read_page(
        &self,
        hash: &str,
        offset: u64,
        len: u32,
    ) -> Result<BlobText, ReadError> {
        blobs::read_page(&self.0, hash, offset, len)
    }
    /// A whole blob of at most `max` bytes; `None` when it is missing, larger or unreadable, or
    /// `hash` is not a content address.
    pub(crate) fn read_all(&self, hash: &str, max: u64) -> Option<Vec<u8>> {
        use std::io::Read;
        let file = std::fs::File::open(blobs::path(&self.0, hash)?).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= max).then_some(bytes)
    }
}

#[derive(Clone)]
pub(crate) struct Files {
    pub attachments: Attachments,
    pub backups: PathBuf,
}

impl Files {
    pub(crate) fn new(profile: &crate::profile::Profile) -> Self {
        Self {
            attachments: Attachments::new(profile.attachments_dir()),
            backups: profile.backups_dir(),
        }
    }
}
