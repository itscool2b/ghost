//! Retention: runs at boot in the background and when a session ends. Deletes log files older than
//! `RetentionPolicy::max_age`, then the oldest files until the profile's protocol logs fit
//! `RetentionPolicy::max_total_bytes`, and removes chat folders left empty. The files of a log
//! still being written are never deleted.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{RetentionPolicy, RetentionReport};

/// One log file found under the root.
#[derive(Debug)]
struct LogFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    /// A file of a log still being written.
    live: bool,
}

/// Prunes `root` (`<profile>/logs/protocol`) as of `now`. `live` holds the `(chat dir, spawn id)`
/// of every log still being written.
pub(crate) fn prune(
    root: &Path,
    policy: RetentionPolicy,
    live: &HashSet<(PathBuf, String)>,
    now: SystemTime,
) -> RetentionReport {
    let mut report = RetentionReport::default();
    let (mut files, chat_dirs) = match scan(root, live) {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .errors
                .push(format!("could not read {}: {error}", root.display()));
            return report;
        }
    };
    // Oldest first.
    files.sort_by(|a, b| {
        a.modified
            .cmp(&b.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut total: u64 = files.iter().map(|file| file.bytes).sum();
    for file in &files {
        if file.live {
            continue;
        }
        let expired = now
            .duration_since(file.modified)
            .is_ok_and(|age| age > policy.max_age);
        if !expired && total <= policy.max_total_bytes {
            continue;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) => {
                report.files_removed += 1;
                report.bytes_removed += file.bytes;
                total = total.saturating_sub(file.bytes);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                total = total.saturating_sub(file.bytes);
            }
            Err(error) => report
                .errors
                .push(format!("could not delete {}: {error}", file.path.display())),
        }
    }
    for dir in chat_dirs {
        let empty = std::fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_none());
        if empty {
            // Another session may be creating its first file here right now; losing that race
            // only means the folder is created again.
            let _ = std::fs::remove_dir(&dir);
        }
    }
    report
}

/// Every `.jsonl` file one folder below `root`, and every such folder.
fn scan(
    root: &Path,
    live: &HashSet<(PathBuf, String)>,
) -> io::Result<(Vec<LogFile>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in entries.flatten() {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.ends_with(".jsonl") {
                continue;
            }
            let Ok(meta) = file.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let spawn = name.split('.').next().unwrap_or_default();
            let live = live.contains(&(dir.clone(), spawn.to_owned()));
            files.push(LogFile {
                bytes: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                live,
                path,
            });
        }
        dirs.push(dir);
    }
    Ok((files, dirs))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn write(path: &Path, bytes: usize, age: Duration, now: SystemTime) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, vec![b'x'; bytes]).expect("write");
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open");
        file.set_modified(now - age).expect("mtime");
    }

    #[test]
    fn old_files_go_first_then_the_oldest_until_the_total_fits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let now = SystemTime::now();
        write(&root.join("a/old.jsonl"), 100, 20 * DAY, now);
        write(&root.join("a/older.1.jsonl"), 100, 30 * DAY, now);
        write(&root.join("b/s1.jsonl"), 400, 3 * DAY, now);
        write(&root.join("b/s2.jsonl"), 400, 2 * DAY, now);
        write(&root.join("b/s3.jsonl"), 400, DAY, now);
        write(&root.join("b/notes.txt"), 9999, 40 * DAY, now);
        let policy = RetentionPolicy {
            max_total_bytes: 900,
            max_age: Duration::from_secs(14 * 24 * 60 * 60),
        };
        let report = prune(root, policy, &HashSet::new(), now);
        assert_eq!(report.files_removed, 3, "{report:?}");
        assert_eq!(report.bytes_removed, 600);
        assert!(report.errors.is_empty());
        assert!(
            !root.join("a").exists(),
            "the emptied chat folder is removed"
        );
        assert!(
            !root.join("b/s1.jsonl").exists(),
            "the oldest goes until the total fits"
        );
        assert!(root.join("b/s2.jsonl").exists());
        assert!(root.join("b/s3.jsonl").exists());
        assert!(
            root.join("b/notes.txt").exists(),
            "only log files are touched"
        );
    }

    #[test]
    fn a_log_still_being_written_is_never_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let now = SystemTime::now();
        write(&root.join("chat/live.jsonl"), 500, 30 * DAY, now);
        write(&root.join("chat/live.1.jsonl"), 500, 31 * DAY, now);
        write(&root.join("chat/dead.jsonl"), 500, 30 * DAY, now);
        let live = HashSet::from([(root.join("chat"), "live".to_owned())]);
        let report = prune(root, RetentionPolicy::STANDARD, &live, now);
        assert_eq!(report.files_removed, 1);
        assert!(root.join("chat/live.jsonl").exists());
        assert!(root.join("chat/live.1.jsonl").exists());
        assert!(!root.join("chat/dead.jsonl").exists());
    }

    #[test]
    fn a_missing_root_is_nothing_to_do() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = prune(
            &dir.path().join("none"),
            RetentionPolicy::STANDARD,
            &HashSet::new(),
            SystemTime::now(),
        );
        assert_eq!(report, RetentionReport::default());
    }
}
