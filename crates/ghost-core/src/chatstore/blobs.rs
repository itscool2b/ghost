//! Blob files: `<profile>/attachments/<sha256>`, written `.partial`, synced and renamed, at most
//! 64 MiB stored per blob, read back in pages that end on a character boundary.
//!
//! Blobs are content-addressed, so a file that already exists is never written again. The writer
//! thread writes a blob before the op whose row names it, so no row ever names a missing file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use ghost_protocol::session::BlobText;

use crate::fsutil;

/// The most of one blob that is stored; a longer original keeps its first 64 MiB, and the
/// `Truncation` that names it says how long the original was.
pub(crate) const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

/// The most one `chat_blob` call returns.
pub(crate) const MAX_BLOB_PAGE: u32 = 256 * 1024;

/// What a blob read answers for a file that is gone.
pub(crate) const BLOB_GONE: &str = "The full output is no longer available.";

/// What a blob read answers for a blob that still waits in the failed batch or the FIFO.
pub(crate) const BLOB_UNSAVED: &str = "The full output isn't saved yet because the disk is full.";

/// Whether `sha256` is a content address (64 lowercase hex digits), so it can name a file.
pub(crate) fn is_sha256(sha256: &str) -> bool {
    sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `<dir>/<sha256>`; `None` for anything that is not a content address.
pub(crate) fn path(dir: &Path, sha256: &str) -> Option<PathBuf> {
    is_sha256(sha256).then(|| dir.join(sha256))
}

/// Writes a blob unless its file already exists: `.partial`, synced, renamed, then the folder is
/// synced. Returns the stored size.
pub(crate) fn write(dir: &Path, sha256: &str, bytes: &[u8]) -> io::Result<u64> {
    let target = path(dir, sha256).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "not a sha256 content address")
    })?;
    let stored = &bytes[..bytes.len().min(MAX_BLOB_BYTES)];
    if fs::metadata(&target).is_ok_and(|meta| meta.is_file()) {
        return Ok(stored.len() as u64);
    }
    fsutil::create_private_dir_all(dir).map_err(io::Error::other)?;
    let partial = fsutil::with_suffix(&target, fsutil::PARTIAL_SUFFIX);
    let written = (|| {
        let mut file = create_private(&partial)?;
        file.write_all(stored)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&partial, &target)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    fsutil::sync_dir(dir);
    Ok(stored.len() as u64)
}

fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Why a blob page could not be read.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// No such file (or not a content address): the output is no longer available.
    Gone,
    Io(io::Error),
}

/// One page of a blob's text: from `offset` (moved forward to a character boundary) up to
/// `offset + len`, cut back to the last character boundary, but always at least one character.
pub(crate) fn read_page(
    dir: &Path,
    sha256: &str,
    offset: u64,
    len: u32,
) -> Result<BlobText, ReadError> {
    let path = path(dir, sha256).ok_or(ReadError::Gone)?;
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(ReadError::Gone),
        Err(error) => return Err(ReadError::Io(error)),
    };
    let total = file.metadata().map_err(ReadError::Io)?.len();
    let len = len.clamp(1, MAX_BLOB_PAGE);
    if offset >= total {
        return Ok(BlobText {
            sha256: sha256.to_owned(),
            offset: total,
            next_offset: total,
            total,
            text: String::new(),
            eof: true,
        });
    }
    // Three bytes either side cover a character cut at the start and one cut at the end.
    let window_start = offset;
    let window_len = (u64::from(len) + 4).min(total - window_start);
    file.seek(SeekFrom::Start(window_start))
        .map_err(ReadError::Io)?;
    let mut window = vec![0u8; usize::try_from(window_len).unwrap_or(usize::MAX)];
    file.read_exact(&mut window).map_err(ReadError::Io)?;

    Ok(text_window(sha256, offset, len, total, &window))
}

pub(crate) fn text_window(
    sha256: &str,
    offset: u64,
    len: u32,
    total: u64,
    window: &[u8],
) -> BlobText {
    let start = window
        .iter()
        .position(|byte| !is_continuation(*byte))
        .unwrap_or(window.len());
    let wanted_end = start.saturating_add(len as usize).min(window.len());
    let end = if wanted_end >= window.len() && offset + window.len() as u64 == total {
        window.len()
    } else {
        char_floor(window, start, wanted_end)
    };
    let text = String::from_utf8_lossy(&window[start..end]).into_owned();
    let next_offset = offset + end as u64;
    BlobText {
        sha256: sha256.to_owned(),
        offset: offset + start as u64,
        next_offset,
        total,
        text,
        eof: next_offset >= total,
    }
}

fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

/// The last character boundary in `window[start..]` at or before `end`, but past at least one whole
/// character, so a page never comes back empty.
fn char_floor(window: &[u8], start: usize, end: usize) -> usize {
    let mut at = end;
    while at > start && at < window.len() && is_continuation(window[at]) {
        at -= 1;
    }
    if at > start {
        return at;
    }
    // Not even one character fits in `len`: include the first one whole.
    let mut at = start + 1;
    while at < window.len() && is_continuation(window[at]) {
        at += 1;
    }
    at.min(window.len())
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::chatstore::rows::hex;

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn writing_is_atomic_and_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let attachments = dir.path().join("attachments");
        let bytes = b"hello blob";
        let id = sha(bytes);
        assert_eq!(write(&attachments, &id, bytes).expect("write"), 10);
        assert_eq!(fs::read(attachments.join(&id)).expect("read"), bytes);
        assert!(!fsutil::with_suffix(&attachments.join(&id), ".partial").exists());
        // A second write of the same address leaves the file as it is.
        assert_eq!(write(&attachments, &id, bytes).expect("again"), 10);
        assert!(write(&attachments, "../../etc/passwd", bytes).is_err());
        assert!(write(&attachments, &id.to_uppercase(), bytes).is_err());
    }

    #[test]
    fn missing_or_malformed_blobs_are_gone() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            read_page(dir.path(), &sha(b"x"), 0, 10),
            Err(ReadError::Gone)
        ));
        assert!(matches!(
            read_page(dir.path(), "..", 0, 10),
            Err(ReadError::Gone)
        ));
    }

    /// U16: a multibyte character at a 256 KiB page boundary is never split.
    #[test]
    fn blob_pages_on_char_boundaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 256 KiB - 1 ASCII bytes, then a 4-byte emoji straddling the page end, then more text.
        let mut text = "a".repeat(MAX_BLOB_PAGE as usize - 1);
        text.push('😀');
        text.push_str("tail é and more");
        let id = sha(text.as_bytes());
        write(dir.path(), &id, text.as_bytes()).expect("write");

        let first = read_page(dir.path(), &id, 0, MAX_BLOB_PAGE).expect("first page");
        assert_eq!(first.offset, 0);
        assert_eq!(
            first.text.len(),
            MAX_BLOB_PAGE as usize - 1,
            "the emoji moves to the next page"
        );
        assert_eq!(first.next_offset, MAX_BLOB_PAGE as u64 - 1);
        assert!(!first.eof);
        assert_eq!(first.total, text.len() as u64);

        let second =
            read_page(dir.path(), &id, first.next_offset, MAX_BLOB_PAGE).expect("second page");
        assert!(second.text.starts_with('😀'));
        assert!(second.eof);
        assert_eq!(format!("{}{}", first.text, second.text), text);

        // Paging by a few bytes at a time still rebuilds the text exactly, every page whole.
        let mut offset = 0;
        let mut rebuilt = String::new();
        let small = "x😀yé😀😀z".repeat(3);
        let small_id = sha(small.as_bytes());
        write(dir.path(), &small_id, small.as_bytes()).expect("write small");
        loop {
            let page = read_page(dir.path(), &small_id, offset, 3).expect("page");
            assert!(!page.text.is_empty() || page.eof);
            assert!(!page.text.contains('\u{FFFD}'), "{page:?}");
            rebuilt.push_str(&page.text);
            offset = page.next_offset;
            if page.eof {
                break;
            }
        }
        assert_eq!(rebuilt, small);

        // An offset inside a character moves forward to the next boundary.
        let inside = read_page(dir.path(), &small_id, 2, 64).expect("inside");
        assert_eq!(inside.offset, 5, "after the emoji that starts at byte 1");
        assert!(inside.text.starts_with('y'));
    }

    #[test]
    fn a_page_shorter_than_one_character_still_returns_it_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = "😀😀";
        let id = sha(text.as_bytes());
        write(dir.path(), &id, text.as_bytes()).expect("write");
        let page = read_page(dir.path(), &id, 0, 1).expect("page");
        assert_eq!(page.text, "😀");
        assert_eq!(page.next_offset, 4);
        let end = read_page(dir.path(), &id, 8, 1).expect("end");
        assert!(end.eof);
        assert_eq!(end.text, "");
    }
}
