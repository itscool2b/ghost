//! Recover only Git's abandoned bulk-fsync quarantines, while owning the storage lease.
//! Normal objects and every name outside Git's exact generated pattern are left untouched.

use std::ffi::OsStr;
use std::path::Path;

use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, Metadata};

use super::{CheckpointError, Result, coordination::Context};

const PREFIX: &str = "tmp_objdir-bulk-fsync-";
const ENTRY_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 32;

fn generated_name(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(PREFIX))
        .is_some_and(|suffix| {
            suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn suspicious() -> CheckpointError {
    CheckpointError::Message("An abandoned checkpoint transaction contains an unsafe entry. Its remaining files were preserved.".into())
}

fn work_limit() -> CheckpointError {
    CheckpointError::Message(
        "Checkpoint transaction cleanup exceeded its work limit. Remaining files were preserved."
            .into(),
    )
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    let reparse = {
        use cap_std::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    metadata.file_type().is_symlink() || reparse
}

/// The selected objects root may itself be intentionally relocated. Every descent from that
/// capability is no-follow, and only the exact Git-generated quarantine names are traversed.
/// A dropped caller cancels future work but the worker retains the lease until it actually exits.
pub(super) async fn prepare(repository: &Path, context: Context) -> Result<bool> {
    let object_path = repository.join("objects");
    tokio::task::spawn_blocking(move || {
        context.check()?;
        let objects = Dir::open_ambient_dir(&object_path, cap_std::ambient_authority())?;
        let mut budget = ENTRY_LIMIT;
        let removed = cleanup(&objects, &context, &mut budget)?;
        tracing::debug!(removed, "Abandoned checkpoint transactions removed");
        // Opening/querying a relocated or network volume can block too. Keep it off the async
        // reactor and under the same owned lease, including if cancellation arrives mid-query.
        context.check()?;
        let batch_fsync = super::durability::supports_batch(&object_path);
        context.check()?;
        Ok(batch_fsync)
    })
    .await
    .map_err(|_| {
        CheckpointError::Message("Checkpoint transaction cleanup stopped unexpectedly.".into())
    })?
}

fn cleanup(objects: &Dir, context: &Context, budget: &mut usize) -> Result<usize> {
    let mut removed = 0usize;
    for entry in objects.entries()? {
        context.check()?;
        // Inspection is work even for unmatched entries and empty quarantines. Charge the
        // outer scan and every recursive entry against one shared operation budget.
        *budget = budget.checked_sub(1).ok_or_else(work_limit)?;
        let name = entry?.file_name();
        if !generated_name(&name) {
            continue;
        }
        let metadata = objects.symlink_metadata(&name)?;
        if is_link(&metadata) || !metadata.is_dir() {
            return Err(suspicious());
        }
        let quarantine = objects.open_dir_nofollow(&name)?;
        erase(quarantine, context, 0, budget)?;
        removed += 1;
    }
    Ok(removed)
}

fn erase(directory: Dir, context: &Context, depth: usize, budget: &mut usize) -> Result<()> {
    context.check()?;
    if depth > DEPTH_LIMIT {
        return Err(work_limit());
    }
    for entry in directory.entries()? {
        context.check()?;
        *budget = budget.checked_sub(1).ok_or_else(work_limit)?;
        let name = entry?.file_name();
        let metadata = directory.symlink_metadata(&name)?;
        if is_link(&metadata) {
            return Err(suspicious());
        }
        if metadata.is_dir() {
            erase(
                directory.open_dir_nofollow(&name)?,
                context,
                depth + 1,
                budget,
            )?;
        } else if metadata.is_file() {
            remove_file(&directory, &name)?;
        } else {
            return Err(suspicious());
        }
    }
    context.check()?;
    directory.remove_open_dir()?;
    Ok(())
}

#[cfg(not(windows))]
fn remove_file(directory: &Dir, name: &OsStr) -> Result<()> {
    directory.remove_file(name)?;
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)] // Delete one validated no-follow handle without modifying shared attributes.
fn remove_file(directory: &Dir, name: &OsStr) -> Result<()> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{OpenOptions, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_READ_ATTRIBUTES,
        FileDispositionInfoEx, SetFileInformationByHandle,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .access_mode(DELETE | FILE_READ_ATTRIBUTES);
    let file = directory.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(suspicious());
    }
    // Git's objects are read-only, and a partially migrated quarantine file can already have
    // a hard link in the ordinary object store. Do not clear that shared readonly attribute.
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: the live no-follow regular-file handle has DELETE access. The input structure
    // matches FileDispositionInfoEx and remains live for the supplied exact byte length.
    let removed = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if removed == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::coordination;
    use super::*;

    #[test]
    fn only_the_exact_git_generated_name_is_selected() {
        assert!(generated_name(OsStr::new("tmp_objdir-bulk-fsync-aZ019x")));
        for name in [
            "tmp_objdir-bulk-fsync-",
            "tmp_objdir-bulk-fsync-short",
            "tmp_objdir-bulk-fsync-toolong",
            "tmp_objdir-bulk-fsync-a-019x",
            "tmp_objdir-other-aZ019x",
            "objects",
            "ab",
            "pack",
        ] {
            assert!(!generated_name(OsStr::new(name)), "{name}");
        }
    }

    #[tokio::test]
    async fn top_level_unmatched_entries_and_empty_quarantines_consume_the_work_budget() {
        for matched in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let repository = temp.path().join("repo.git");
            let objects = repository.join("objects");
            std::fs::create_dir_all(&objects).unwrap();
            for index in 0..3 {
                let name = if matched {
                    format!("tmp_objdir-bulk-fsync-{index:06}")
                } else {
                    format!("unmatched-{index}")
                };
                std::fs::create_dir(objects.join(name)).unwrap();
            }
            let folder = crate::ids::new_id();
            let owner = coordination::acquire(&repository, &folder, temp.path())
                .await
                .unwrap();
            let directory = Dir::open_ambient_dir(&objects, cap_std::ambient_authority()).unwrap();
            let mut budget = 2;
            let result = cleanup(&directory, &owner.context(), &mut budget);
            assert!(
                matches!(result, Err(CheckpointError::Message(message)) if message.contains("work limit"))
            );
            assert_eq!(budget, 0);
            assert_eq!(
                std::fs::read_dir(&objects).unwrap().count(),
                if matched { 1 } else { 3 }
            );
        }
    }

    #[tokio::test]
    async fn cleanup_preserves_ordinary_objects_and_unmatched_names_including_hard_links() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repo.git");
        let objects = repository.join("objects");
        std::fs::create_dir_all(objects.join("ab")).unwrap();
        std::fs::create_dir_all(objects.join("pack")).unwrap();
        let published = objects.join("ab/published-object");
        std::fs::write(&published, b"ordinary object").unwrap();
        std::fs::write(objects.join("pack/pack-keep.pack"), b"ordinary pack").unwrap();
        let unmatched = objects.join("tmp_objdir-bulk-fsync-not-generated");
        std::fs::create_dir(&unmatched).unwrap();
        std::fs::write(unmatched.join("keep"), b"unmatched").unwrap();
        let stale = objects.join("tmp_objdir-bulk-fsync-aZ019x");
        std::fs::create_dir_all(stale.join("ab")).unwrap();
        std::fs::create_dir(stale.join("pack")).unwrap();
        std::fs::hard_link(&published, stale.join("ab/published-object")).unwrap();
        std::fs::write(stale.join("ab/unpublished-object"), b"abandoned").unwrap();
        let original_permissions = std::fs::metadata(&published).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&published, permissions).unwrap();
        let folder = crate::ids::new_id();
        let owner = coordination::acquire(&repository, &folder, temp.path())
            .await
            .unwrap();
        prepare(&repository, owner.context()).await.unwrap();
        assert!(!stale.exists());
        assert_eq!(std::fs::read(&published).unwrap(), b"ordinary object");
        assert!(
            std::fs::metadata(&published)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert_eq!(
            std::fs::read(objects.join("pack/pack-keep.pack")).unwrap(),
            b"ordinary pack"
        );
        assert_eq!(std::fs::read(unmatched.join("keep")).unwrap(), b"unmatched");
        std::fs::set_permissions(&published, original_permissions).unwrap();
    }

    fn directory_link(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let result = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "fixture junction creation failed for {} -> {} ({}): stdout: {}; stderr: {}",
                link.display(),
                target.display(),
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[tokio::test]
    async fn top_level_and_nested_links_are_rejected_without_deleting_outside_data() {
        for nested in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let repository = temp.path().join("repo.git");
            std::fs::create_dir_all(repository.join("objects")).unwrap();
            let outside = temp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("keep"), b"outside bytes").unwrap();
            let stale = repository
                .join("objects")
                .join("tmp_objdir-bulk-fsync-aZ019x");
            let link = if nested {
                std::fs::create_dir(&stale).unwrap();
                stale.join("ab")
            } else {
                stale
            };
            directory_link(&link, &outside);
            let folder = crate::ids::new_id();
            let owner = coordination::acquire(&repository, &folder, temp.path())
                .await
                .unwrap();
            assert!(prepare(&repository, owner.context()).await.is_err());
            assert_eq!(
                std::fs::read(outside.join("keep")).unwrap(),
                b"outside bytes"
            );
            assert!(std::fs::symlink_metadata(&link).is_ok());
        }
    }

    #[test]
    fn cancelled_cleanup_retains_its_lease_until_the_blocking_worker_exits() {
        use futures::FutureExt;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let repository = temp.path().join("repo.git");
            let stale = repository.join("objects/tmp_objdir-bulk-fsync-aZ019x");
            std::fs::create_dir_all(&stale).unwrap();
            std::fs::write(stale.join("keep-until-next-cleanup"), b"cancelled").unwrap();
            let folder = crate::ids::new_id();
            let owner = coordination::acquire(&repository, &folder, temp.path())
                .await
                .unwrap();
            let (entered, started) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            });
            started.await.unwrap();
            let mut cleanup = Box::pin(prepare(&repository, owner.context()));
            assert!(cleanup.as_mut().now_or_never().is_none());
            drop(cleanup);
            drop(owner);
            let mut next = Box::pin(coordination::acquire(&repository, &folder, temp.path()));
            assert!(next.as_mut().now_or_never().is_none());
            release.send(()).unwrap();
            blocker.await.unwrap();
            let next = tokio::time::timeout(std::time::Duration::from_secs(5), next)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                std::fs::read(stale.join("keep-until-next-cleanup")).unwrap(),
                b"cancelled"
            );
            drop(next);
        });
    }
}
