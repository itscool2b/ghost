//! Deleting a tree without following links out of it, with retries for locked files.
//!
//! A link or junction inside the tree is removed as a link; what it points to is never touched.
//! Read-only files (Git objects in checkpoints) lose the attribute first. A file another process
//! holds open (a webview helper still shutting down, an editor) is retried with a growing pause
//! and then reported with how often it was tried.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Failure;

/// How often, and how patiently, a locked path is retried before it is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    pub attempts: u32,
    /// The pause after the first failure; each later pause is one step longer.
    pub step: Duration,
}

impl Retry {
    /// About ten seconds in all, enough for webview helpers to let go of their files.
    pub const PATIENT: Retry = Retry {
        attempts: 8,
        step: Duration::from_millis(350),
    };
}

/// At most this many failures are listed; the rest are counted in the last one.
const FAILURES_LISTED: usize = 50;

pub(super) struct Removal<'a> {
    retry: &'a Retry,
    pub(super) failed: Vec<Failure>,
    /// Whether a kept path stopped a folder from going.
    pub(super) kept_any: bool,
    unlisted: usize,
}

impl<'a> Removal<'a> {
    pub(super) fn new(retry: &'a Retry) -> Self {
        Removal {
            retry,
            failed: Vec::new(),
            kept_any: false,
            unlisted: 0,
        }
    }

    /// Removes `path` and everything under it except `keep` (and the folders that hold them).
    pub(super) fn remove(&mut self, path: &Path, keep: &[PathBuf]) {
        self.remove_entry(path, keep);
        if self.unlisted > 0
            && let Some(last) = self.failed.last_mut()
        {
            last.error = format!(
                "{} ({} more paths could not be removed either)",
                last.error, self.unlisted
            );
        }
    }

    /// `true` when `path` is gone.
    fn remove_entry(&mut self, path: &Path, keep: &[PathBuf]) -> bool {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
            Err(error) => {
                self.fail(path, &error, 1);
                return false;
            }
        };
        if is_link(&meta) {
            // The link itself, never its target. A Windows directory link or junction is a
            // directory entry (its attributes say so; its file type says "link"); everything
            // else is removed as a file.
            let directory = link_to_directory(&meta);
            return self.attempt(path, || {
                if directory {
                    std::fs::remove_dir(path)
                } else {
                    std::fs::remove_file(path)
                }
            });
        }
        if !meta.is_dir() {
            return self.attempt(path, || remove_file(path));
        }
        if keep.iter().any(|kept| kept == path) {
            self.kept_any = true;
            return false;
        }
        allow_changes(path, &meta);
        let mut whole = true;
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => whole &= self.remove_entry(&entry.path(), keep),
                        Err(error) => {
                            self.fail(path, &error, 1);
                            whole = false;
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
            Err(error) => {
                self.fail(path, &error, 1);
                return false;
            }
        }
        // A folder that still holds a kept or failed entry stays; that entry is the report.
        whole && self.attempt(path, || std::fs::remove_dir(path))
    }

    /// Runs `operation` until it succeeds, the path is gone, or the attempts run out.
    fn attempt(&mut self, path: &Path, mut operation: impl FnMut() -> io::Result<()>) -> bool {
        let mut tries = 0;
        loop {
            tries += 1;
            match operation() {
                Ok(()) => return true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
                Err(error) if tries >= self.retry.attempts.max(1) => {
                    self.fail(path, &error, tries);
                    return false;
                }
                Err(_) => std::thread::sleep(self.retry.step * tries),
            }
        }
    }

    fn fail(&mut self, path: &Path, error: &io::Error, attempts: u32) {
        if self.failed.len() >= FAILURES_LISTED {
            self.unlisted += 1;
            return;
        }
        self.failed.push(Failure {
            path: path.to_path_buf(),
            error: describe(error),
            attempts,
        });
    }
}

/// Removes a file, clearing a read-only attribute that stops it on Windows.
fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            let meta = std::fs::symlink_metadata(path)?;
            let mut permissions = meta.permissions();
            if permissions.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                std::fs::set_permissions(path, permissions)?;
                std::fs::remove_file(path)
            } else {
                Err(error)
            }
        }
        other => other,
    }
}

/// Lets this process delete the folder's entries: owner write and search on Unix, no read-only
/// attribute on Windows. Only folders inside the tree being removed are changed.
fn allow_changes(path: &Path, meta: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700));
        }
    }
    #[cfg(not(unix))]
    {
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// A symbolic link, or on Windows any reparse point (junctions included).
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    meta.file_type().is_symlink() || reparse
}

fn link_to_directory(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
        meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

fn describe(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::PermissionDenied => {
            "Access is denied, or another program is using it.".to_owned()
        }
        _ if error.raw_os_error() == Some(32) => "Another program is using it.".to_owned(),
        _ => error.to_string(),
    }
}

/// Sizes for the review, within one budget of entries for the whole plan so a huge folder cannot
/// stall it. Links are counted as themselves, never followed.
pub(super) struct SizeBudget {
    left: usize,
}

impl Default for SizeBudget {
    fn default() -> Self {
        SizeBudget { left: 200_000 }
    }
}

impl SizeBudget {
    /// `None` when the budget ran out before the whole tree was counted.
    pub(super) fn size(&mut self, path: &Path) -> Option<u64> {
        let mut total = 0u64;
        let mut stack = vec![path.to_path_buf()];
        while let Some(next) = stack.pop() {
            if self.left == 0 {
                return None;
            }
            self.left -= 1;
            let Ok(meta) = std::fs::symlink_metadata(&next) else {
                continue;
            };
            if meta.is_dir() && !is_link(&meta) {
                if let Ok(entries) = std::fs::read_dir(&next) {
                    stack.extend(
                        entries
                            .filter_map(|entry| entry.ok())
                            .map(|entry| entry.path()),
                    );
                }
            } else {
                total = total.saturating_add(meta.len());
            }
        }
        Some(total)
    }
}
