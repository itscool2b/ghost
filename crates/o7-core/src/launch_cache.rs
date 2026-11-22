//! The sidebar as the app last showed it, kept beside the profile's database so that the window's
//! first frame can show it again while the database is still answering.
//!
//! It is a convenience copy and nothing reads it for the truth: the page shows it until the real
//! lists arrive and then drops it. What it holds (chat titles, folders, project names) the
//! database beside it holds too, so it lives where the database lives and nowhere else: in the
//! profile's own folder, owner-only, removed with the profile. The page's own storage is shared by
//! every profile and outlives a deleted one, which is why it is not kept there.
//!
//! Each part is a file of its own, `{"v": <version>, "data": <the list as the page received it>}`,
//! written atomically and only when the list changed.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::error::{CoreError, Result};
use crate::fsutil;

/// The folder inside a profile's folder.
const DIR: &str = "launch-cache";
/// The shape of what is kept. The page ignores a file of any other version.
pub const VERSION: u32 = 1;
/// A list larger than this is neither kept nor read back: the first frame is not worth megabytes.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// One of the lists the sidebar starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// Every chat, as `chats_list` answers.
    Chats,
    /// Projects, folders and their chats, as the unfiltered `workspace_snapshot` answers.
    Workspace,
}

impl Part {
    fn file(self) -> &'static str {
        match self {
            Part::Chats => "chats.json",
            Part::Workspace => "workspace.json",
        }
    }
}

fn path(profile_dir: &Path, part: Part) -> PathBuf {
    profile_dir.join(DIR).join(part.file())
}

/// What each file was last written with, so an unchanged list costs no write.
fn written() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static WRITTEN: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    WRITTEN.get_or_init(Mutex::default)
}

/// Keeps `data`, a list already serialized as JSON, as `part` of the profile in `profile_dir`.
/// Returns whether it wrote: an unchanged list, or one too large to keep, writes nothing (and one
/// too large removes what was kept, so that a stale list is not shown for it).
pub fn save(profile_dir: &Path, part: Part, data: &[u8]) -> Result<bool> {
    let path = path(profile_dir, part);
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let hash = hasher.finish();
    let mut written = written().lock().unwrap_or_else(PoisonError::into_inner);
    if written.get(&path) == Some(&hash) {
        return Ok(false);
    }
    if data.len() > MAX_BYTES {
        written.insert(path.clone(), hash);
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(CoreError::io(path)(error)),
        };
    }
    fsutil::create_private_dir_all(fsutil::parent_dir(&path))?;
    let mut bytes = format!("{{\"v\":{VERSION},\"data\":").into_bytes();
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(b"}\n");
    fsutil::write_atomic(&path, &bytes)?;
    written.insert(path, hash);
    Ok(true)
}

/// The file kept for `part`, as text for the page to parse, when there is one of a sensible size.
/// Never an error: a first frame without the last lists is an ordinary first frame.
pub fn read(profile_dir: &Path, part: Part) -> Option<String> {
    let path = path(profile_dir, part);
    let size = std::fs::metadata(&path).ok()?.len();
    if size
        > u64::try_from(MAX_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_add(64)
    {
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_kept_once_and_read_back_in_its_envelope() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(dir.path(), Part::Chats), None);
        assert!(save(dir.path(), Part::Chats, br#"[{"id":"a"}]"#).expect("save"));
        assert!(
            !save(dir.path(), Part::Chats, br#"[{"id":"a"}]"#).expect("same"),
            "an unchanged list is not written again"
        );
        assert_eq!(
            read(dir.path(), Part::Chats).as_deref(),
            Some("{\"v\":1,\"data\":[{\"id\":\"a\"}]}\n")
        );
        assert_eq!(read(dir.path(), Part::Workspace), None, "parts are apart");
        assert!(save(dir.path(), Part::Chats, b"[]").expect("changed"));
        assert_eq!(
            read(dir.path(), Part::Chats).as_deref(),
            Some("{\"v\":1,\"data\":[]}\n")
        );
        #[cfg(unix)]
        assert!(
            fsutil::is_owner_only(&path(dir.path(), Part::Chats)).expect("mode"),
            "only the owner may read the titles"
        );
    }

    #[test]
    fn profiles_keep_their_own_lists_and_a_removed_profile_keeps_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        assert!(save(&one, Part::Chats, br#"["one"]"#).expect("one"));
        assert!(save(&two, Part::Chats, br#"["two"]"#).expect("two"));
        assert!(read(&one, Part::Chats).expect("one").contains("one"));
        assert!(read(&two, Part::Chats).expect("two").contains("two"));
        std::fs::remove_dir_all(&one).expect("remove the profile");
        assert_eq!(read(&one, Part::Chats), None);
        assert!(read(&two, Part::Chats).is_some());
    }

    #[test]
    fn a_list_too_large_is_dropped_instead_of_kept_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(save(dir.path(), Part::Workspace, b"[1]").expect("small"));
        let large = vec![b'1'; MAX_BYTES + 1];
        assert!(!save(dir.path(), Part::Workspace, &large).expect("large"));
        assert_eq!(read(dir.path(), Part::Workspace), None);
    }
}
