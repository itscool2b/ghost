//! "Show in Finder" for the data folder: the only path the page can ask the shell to reveal.
//!
//! The opener plugin's own `reveal_item_in_dir` command takes paths from the page and checks no
//! scope. Granting it would let page script test which paths exist (a missing one fails, an
//! existing one opens a window), open file manager windows at will, and on Windows make the shell
//! connect to the server in a UNC path, which sends the user's credentials to it. The
//! `reveal_data_dir` command instead takes no argument: the folder comes from startup.

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppErrorCode};

/// What to reveal for the data folder `data_dir`: the folder itself or, when it does not exist
/// (the error screen after startup failed before creating it), its nearest existing parent, so
/// the button still opens the place where the folder belongs.
pub fn target(data_dir: Option<&Path>) -> Result<PathBuf, AppError> {
    let data_dir = data_dir.ok_or_else(|| {
        AppError::new(
            AppErrorCode::DataDirUnknown,
            "Ghost does not know where its data folder is.",
        )
    })?;
    data_dir
        .ancestors()
        // A relative path's last ancestor is the empty path, which names nothing to show.
        .filter(|path| !path.as_os_str().is_empty())
        .find(|path| path.exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            AppError::new(
                AppErrorCode::RevealFailed,
                format!(
                    "Neither the data folder {} nor any folder above it exists.",
                    data_dir.display()
                ),
            )
        })
}

/// Reveals [`target`] with `reveal` (the opener plugin's `reveal_item_in_dir` in the app) and
/// returns the path it revealed. Generic over `reveal` so tests can check what would be shown
/// without opening a file manager window.
pub fn reveal_data_dir<E: std::fmt::Display>(
    data_dir: Option<&Path>,
    reveal: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<PathBuf, AppError> {
    let path = target(data_dir)?;
    reveal(&path).map_err(|error| {
        AppError::new(
            AppErrorCode::RevealFailed,
            format!(
                "Could not show {} in the file manager: {error}",
                path.display()
            ),
        )
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Runs [`reveal_data_dir`] with a reveal that records what it was asked to show.
    fn revealed(data_dir: Option<&Path>) -> (Result<PathBuf, AppError>, Vec<PathBuf>) {
        let calls = RefCell::new(Vec::new());
        let result = reveal_data_dir(data_dir, |path| {
            calls.borrow_mut().push(path.to_path_buf());
            Ok::<(), String>(())
        });
        (result, calls.into_inner())
    }

    #[test]
    fn an_existing_data_folder_is_revealed_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (result, calls) = revealed(Some(dir.path()));
        assert_eq!(result.expect("revealed"), dir.path());
        assert_eq!(calls, vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn a_missing_data_folder_reveals_its_nearest_existing_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("not").join("created").join("yet");
        let (result, calls) = revealed(Some(&missing));
        assert_eq!(result.expect("revealed"), dir.path());
        assert_eq!(
            calls,
            vec![dir.path().to_path_buf()],
            "only the stored path's parent"
        );
    }

    #[test]
    fn a_file_in_the_way_is_revealed_rather_than_skipped() {
        // Startup's "unusable data folder" case: a file where a parent folder should be.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("data");
        std::fs::write(&blocker, b"not a folder").expect("write");
        let (result, _) = revealed(Some(&blocker.join("nested")));
        assert_eq!(result.expect("revealed"), blocker);
    }

    #[test]
    fn no_data_folder_reveals_nothing() {
        let (result, calls) = revealed(None);
        assert_eq!(
            result.expect_err("refused").code,
            AppErrorCode::DataDirUnknown
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn a_relative_path_with_nothing_on_disk_reveals_nothing() {
        let missing = Path::new("auracode-reveal-test-does-not-exist/data");
        let (result, calls) = revealed(Some(missing));
        let error = result.expect_err("refused");
        assert_eq!(error.code, AppErrorCode::RevealFailed);
        assert!(
            error
                .message
                .contains("auracode-reveal-test-does-not-exist"),
            "{}",
            error.message
        );
        assert!(calls.is_empty(), "the empty path is never revealed");
    }

    #[test]
    fn a_file_manager_failure_becomes_a_sentence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = reveal_data_dir(Some(dir.path()), |_| Err("no file manager is running"))
            .expect_err("failed");
        assert_eq!(error.code, AppErrorCode::RevealFailed);
        assert!(
            error.message.starts_with("Could not show "),
            "{}",
            error.message
        );
        assert!(
            error
                .message
                .ends_with("in the file manager: no file manager is running"),
            "{}",
            error.message
        );
    }
}
