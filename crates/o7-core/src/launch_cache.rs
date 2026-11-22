//! The sidebar as the app last showed it, kept beside the profile's database so that the window's
//! first frame can show it again while the database is still answering.
//!
//! It is a convenience copy and nothing reads it for the truth: the page shows it until the real
//! lists arrive and then drops it. What it holds (chat titles, folders, project names) the
//! database beside it holds too, so it lives where the database lives and nowhere else: in the
//! profile's own folder, owner-only, removed with the profile. The page's own storage is shared by
//! every profile and outlives a deleted one, which is why it is not kept there.
//!
//! Each part is a file of its own, `{"v": 1, "app": "<the build that wrote it>", "data": <the list
//! as the page received it>}`, written atomically and only when the list changed. Only the build
//! that wrote a list reads it back: the lists are the page's own types, which another build may
//! have changed, and a first frame without them is an ordinary first frame.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::error::{CoreError, Result};
use crate::fsutil;

/// The folder inside a profile's folder.
const DIR: &str = "launch-cache";
/// The shape of the file around the list.
const VERSION: u64 = 1;
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

/// Keeps `data`, a list already serialized as JSON by the build `app`, as `part` of the profile in
/// `profile_dir`. Returns whether it wrote: an unchanged list, or one too large to keep, writes
/// nothing (and one too large removes what was kept, so that a stale list is not shown for it).
pub fn save(profile_dir: &Path, part: Part, app: &str, data: &[u8]) -> Result<bool> {
    let path = path(profile_dir, part);
    let mut hasher = DefaultHasher::new();
    app.hash(&mut hasher);
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
    let app = serde_json::Value::from(app).to_string();
    let mut bytes = format!("{{\"v\":{VERSION},\"app\":{app},\"data\":").into_bytes();
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(b"}\n");
    fsutil::write_atomic(&path, &bytes)?;
    written.insert(path, hash);
    Ok(true)
}

/// The list kept for `part`, as JSON text, when the build `app` wrote it and it is of a sensible
/// size. Never an error: whatever cannot be read back is simply not shown.
pub fn read(profile_dir: &Path, part: Part, app: &str) -> Option<String> {
    let path = path(profile_dir, part);
    let size = std::fs::metadata(&path).ok()?.len();
    let most = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
    if size > most.saturating_add(1024) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let kept: serde_json::Value = serde_json::from_str(&text).ok()?;
    if kept.get("v")?.as_u64()? != VERSION || kept.get("app")?.as_str()? != app {
        return None;
    }
    Some(kept.get("data")?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "0.1.7";

    #[test]
    fn a_list_is_kept_once_and_read_back_by_the_build_that_wrote_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(dir.path(), Part::Chats, APP), None);
        assert!(save(dir.path(), Part::Chats, APP, br#"[{"id":"a"}]"#).expect("save"));
        assert!(
            !save(dir.path(), Part::Chats, APP, br#"[{"id":"a"}]"#).expect("same"),
            "an unchanged list is not written again"
        );
        assert_eq!(
            read(dir.path(), Part::Chats, APP).as_deref(),
            Some(r#"[{"id":"a"}]"#)
        );
        assert_eq!(
            read(dir.path(), Part::Workspace, APP),
            None,
            "parts are apart"
        );
        assert_eq!(
            read(dir.path(), Part::Chats, "0.1.8"),
            None,
            "another build's list is not shown"
        );
        assert!(save(dir.path(), Part::Chats, APP, b"[]").expect("changed"));
        assert_eq!(read(dir.path(), Part::Chats, APP).as_deref(), Some("[]"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(path(dir.path(), Part::Chats))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the owner may read the titles");
        }
    }

    #[test]
    fn profiles_keep_their_own_lists_and_a_removed_profile_keeps_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        assert!(save(&one, Part::Chats, APP, br#"["one"]"#).expect("one"));
        assert!(save(&two, Part::Chats, APP, br#"["two"]"#).expect("two"));
        assert_eq!(read(&one, Part::Chats, APP).as_deref(), Some(r#"["one"]"#));
        assert_eq!(read(&two, Part::Chats, APP).as_deref(), Some(r#"["two"]"#));
        std::fs::remove_dir_all(&one).expect("remove the profile");
        assert_eq!(read(&one, Part::Chats, APP), None);
        assert!(read(&two, Part::Chats, APP).is_some());
    }

    #[test]
    fn what_cannot_be_trusted_is_not_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(save(dir.path(), Part::Workspace, APP, b"[1]").expect("small"));
        let large = vec![b'1'; MAX_BYTES + 1];
        assert!(!save(dir.path(), Part::Workspace, APP, &large).expect("large"));
        assert_eq!(
            read(dir.path(), Part::Workspace, APP),
            None,
            "a list too large is dropped, not kept stale"
        );
        // A file that is not what `save` writes: cut short, another shape, another version.
        let file = path(dir.path(), Part::Chats);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for text in [
            r#"{"v":1,"app":"0.1.7","data":[{"id""#,
            r#"[{"id":"a"}]"#,
            r#"{"v":2,"app":"0.1.7","data":[]}"#,
            r#"{"v":1,"data":[]}"#,
            "",
        ] {
            std::fs::write(&file, text).expect("write");
            assert_eq!(read(dir.path(), Part::Chats, APP), None, "{text}");
        }
    }
}
