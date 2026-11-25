//! What a chat's own branch changed against the commit it started from, committed or not, and one
//! changed file before and after. Read only: Git compares and refreshes a private copy of the
//! index, never the person's own.

#[cfg(test)]
#[path = "branch_diff_tests.rs"]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use o7_process::HelperOutput;
use o7_protocol::changes::ChangeKind;

use super::{
    GitError, GitWorkspaces, Result,
    git::{Extra, Git, args, oid, text},
    publish::validate_branch,
};

/// What a chat's own branch changed against its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChanges {
    /// The commit compared against (full id).
    pub base: String,
    /// The branch checked out in the chat's folder, short name, when on one.
    pub branch: Option<String>,
    /// Sorted by path, at most the asked number.
    pub files: Vec<BranchFile>,
    /// More files changed than were listed.
    pub truncated: bool,
}

/// One changed file, by its path as Git gives it: relative to the repository root, `/`-separated.
/// A renamed file is its old path deleted and its new path added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchFile {
    pub path: String,
    pub kind: ChangeKind,
}

/// One changed file at the base and in the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchDiff {
    /// Its text at the base; `None` when it was not there or the file is binary.
    pub before: Option<String>,
    /// Its text in the working tree; `None` when it is gone or the file is binary.
    pub after: Option<String>,
    /// A side is not text, or not a regular file (a submodule, a link). Neither text is given.
    pub binary: bool,
    /// A side was longer than asked, and its text is cut.
    pub truncated: bool,
}

/// The most of each Git listing read. A branch with more changes is listed in part, and says so.
const MAX_LISTING: usize = 8 * 1024 * 1024;
/// The most of one side of a file read, whatever is asked.
const MAX_SIDE: usize = 16 * 1024 * 1024;
/// The largest index copied, as an inspection accepts.
const MAX_INDEX: u64 = 64 * 1024 * 1024;
/// Git decides a file is binary from a NUL in its first 8000 bytes.
const BINARY_PROBE: usize = 8000;

impl GitWorkspaces {
    /// The commit `folder`'s branch is compared against, from what the caller and the records
    /// say: `given` when it is a full commit id in the folder's repository, else the base commit
    /// recorded for `chat_id`'s worktree when `folder` is that worktree. `None` when neither
    /// applies or `folder` is in no Git working tree.
    pub(crate) async fn recorded_branch_base(
        &self,
        chat_id: &str,
        folder: &Path,
        given: Option<&str>,
    ) -> Result<Option<String>> {
        let git = self.git().await?;
        let Some(root) = work_tree_root(&git, folder).await? else {
            return Ok(None);
        };
        if let Some(given) = given
            && let Some(commit) = named_commit(&git, &root, given).await?
        {
            return Ok(Some(commit));
        }
        for record in self.list(chat_id).await? {
            if Path::new(&record.path)
                .canonicalize()
                .is_ok_and(|path| path == root)
                && let Some(commit) = named_commit(&git, &root, &record.base_commit).await?
            {
                return Ok(Some(commit));
            }
        }
        Ok(None)
    }

    /// Where `folder`'s branch left `default_branch`: the merge base of the two, when `folder` is
    /// on another branch. `None` on the default branch or a detached HEAD, for a name that is not
    /// a plain branch name or a branch this repository lacks, and for branches with no common
    /// history.
    pub(crate) async fn default_branch_base(
        &self,
        folder: &Path,
        default_branch: &str,
    ) -> Result<Option<String>> {
        if validate_branch(default_branch).is_err() {
            return Ok(None);
        }
        let git = self.git().await?;
        let Some(root) = work_tree_root(&git, folder).await? else {
            return Ok(None);
        };
        if current_branch(&git, &root)
            .await?
            .is_none_or(|branch| branch == default_branch)
        {
            return Ok(None);
        }
        let output = git
            .output(
                &root,
                vec![
                    "merge-base".into(),
                    "HEAD".into(),
                    format!("refs/heads/{default_branch}").into(),
                ],
                256,
            )
            .await?;
        if exit(&output) != Some(0) {
            return Ok(None);
        }
        let base = text(&output.stdout)?;
        Ok(oid(&base).then_some(base))
    }

    /// What changed in `folder`'s working tree against `base`, committed or not, untracked files
    /// that are not ignored included: at most `max_files` of them.
    pub(crate) async fn branch_changes(
        &self,
        folder: &Path,
        base: &str,
        max_files: usize,
    ) -> Result<BranchChanges> {
        let git = self.git().await?;
        let root = work_tree_root(&git, folder)
            .await?
            .ok_or_else(not_a_work_tree)?;
        let base = full_commit(base)?;
        let branch = current_branch(&git, &root).await?;
        let listing = self.changed(&git, &root, &base, None).await?;
        let truncated = listing.partial || listing.files.len() > max_files;
        Ok(BranchChanges {
            base,
            branch,
            files: listing
                .files
                .into_iter()
                .take(max_files)
                .map(|(path, kind)| BranchFile { path, kind })
                .collect(),
            truncated,
        })
    }

    /// One file of those changes: before, at `base`, read with Git; after, in the working tree,
    /// read from disk inside the repository. Each side at most `max_bytes`. `None` when `path`
    /// is not among the changes, and then neither side is read.
    pub(crate) async fn branch_diff(
        &self,
        folder: &Path,
        base: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<BranchDiff>> {
        if !plain_relative(path) {
            return Ok(None);
        }
        let git = self.git().await?;
        let root = work_tree_root(&git, folder)
            .await?
            .ok_or_else(not_a_work_tree)?;
        let base = full_commit(base)?;
        let listing = self.changed(&git, &root, &base, Some(path)).await?;
        let Some(&kind) = listing.files.get(path) else {
            return Ok(None);
        };
        let max = max_bytes.min(MAX_SIDE);
        let before = match kind {
            ChangeKind::Added => Side::Absent,
            _ => base_side(&git, &root, &base, path, max).await?,
        };
        let after = match kind {
            ChangeKind::Deleted => Side::Absent,
            _ => {
                let path = path.to_owned();
                tokio::task::spawn_blocking(move || working_side(&root, &path, max))
                    .await
                    .map_err(|_| unreadable())??
            }
        };
        Ok(Some(diff(before, after, max)))
    }

    /// The files changed against `base`, restricted to `path` when given. Git compares a file
    /// whose stat changed by its contents, as `git status` does, in a private copy of the index
    /// it may refresh; so a file only touched, or changed back to its text at the base, is not
    /// listed.
    async fn changed(
        &self,
        git: &Git,
        root: &Path,
        base: &str,
        path: Option<&str>,
    ) -> Result<Listing> {
        let index = PrivateIndex::copy(git, root, &self.root.join("indexes")).await?;
        let private = || Extra {
            set: vec![("GIT_INDEX_FILE".into(), index.path.as_os_str().to_owned())],
            allow_cut: true,
            ..Extra::default()
        };
        let mut command = args(&[
            "-c",
            "diff.autoRefreshIndex=true",
            // A refresh writes one whole index: never a shared index file in the repository.
            "-c",
            "core.splitIndex=false",
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            base,
            "--",
        ]);
        command.extend(path.map(OsString::from));
        let tracked = git
            .output_with(root, command, MAX_LISTING, private())
            .await?;
        let mut command = args(&["ls-files", "-z", "--others", "--exclude-standard", "--"]);
        command.extend(path.map(OsString::from));
        let untracked = git
            .output_with(root, command, MAX_LISTING, private())
            .await?;
        drop(index);
        if exit(&tracked) != Some(0) || exit(&untracked) != Some(0) {
            return Err(GitError::Message(
                "Git could not list this branch's changes.".into(),
            ));
        }
        let mut listing = Listing::default();
        listing.add_tracked(&tracked.stdout, tracked.stdout_truncated)?;
        listing.add_untracked(&untracked.stdout, untracked.stdout_truncated);
        Ok(listing)
    }
}

/// A private copy of the repository's index for Git to refresh. Removed when dropped.
struct PrivateIndex {
    path: PathBuf,
}

impl PrivateIndex {
    /// Copies the index of the working tree at `root` into `folder`. A repository without an
    /// index gets none: Git starts from an empty one, and still writes only here.
    async fn copy(git: &Git, root: &Path, folder: &Path) -> Result<Self> {
        use std::io::{Read, Write};
        let output = git
            .output(
                root,
                args(&["rev-parse", "--path-format=absolute", "--git-path", "index"]),
                8192,
            )
            .await?;
        let source = PathBuf::from(text(&output.stdout)?);
        if exit(&output) != Some(0) || !source.is_absolute() {
            return Err(GitError::Message(
                "Git could not say where this repository's index is.".into(),
            ));
        }
        crate::fsutil::ensure_private_dir(folder)?;
        let copy = Self {
            path: folder.join(format!("{}.index", crate::ids::new_id())),
        };
        match std::fs::symlink_metadata(&source) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(unsupported_index()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(copy),
            Err(error) => return Err(error.into()),
        }
        let file = std::fs::File::open(&source)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() > MAX_INDEX {
            return Err(unsupported_index());
        }
        // Git trusts a file's stat only when the file is older than the index that recorded
        // it, and reads that age from the index file's own time: the copy keeps it.
        let modified = meta.modified()?;
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_INDEX + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_INDEX {
            return Err(unsupported_index());
        }
        let mut written = std::fs::File::create(&copy.path)?;
        written.write_all(&bytes)?;
        written.set_modified(modified)?;
        Ok(copy)
    }
}

impl Drop for PrivateIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let mut lock = self.path.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

/// Changed paths, sorted, each once.
#[derive(Default)]
struct Listing {
    files: BTreeMap<String, ChangeKind>,
    /// Git's output was cut, or a path cannot be shown: more files changed than are here.
    partial: bool,
}

impl Listing {
    /// `git diff --name-status -z` records: a status letter, then a path.
    fn add_tracked(&mut self, bytes: &[u8], cut: bool) -> Result<()> {
        self.partial |= cut;
        let mut fields = complete(bytes, cut)
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty());
        while let Some(status) = fields.next() {
            let kind = match status {
                b"A" => ChangeKind::Added,
                b"D" => ChangeKind::Deleted,
                // Modified, a type change, unmerged or unknown: changed either way.
                [letter] if letter.is_ascii_uppercase() => ChangeKind::Modified,
                _ => {
                    return Err(GitError::Message(
                        "Git returned an unsupported change record.".into(),
                    ));
                }
            };
            let Some(path) = fields.next() else {
                break;
            };
            match shown(path) {
                Some(path) => {
                    self.files
                        .entry(path.to_owned())
                        .and_modify(|existing| {
                            if *existing != kind {
                                *existing = ChangeKind::Modified;
                            }
                        })
                        .or_insert(kind);
                }
                None => self.partial = true,
            }
        }
        Ok(())
    }

    /// `git ls-files -z --others` paths, each new to the branch.
    fn add_untracked(&mut self, bytes: &[u8], cut: bool) {
        self.partial |= cut;
        for field in complete(bytes, cut)
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
        {
            // A repository inside the working tree is listed as its folder, with a slash.
            match shown(field).map(|path| path.trim_end_matches('/')) {
                Some(path) if !path.is_empty() => {
                    self.files
                        .entry(path.to_owned())
                        // Gone from the index but still on disk: there at the base and now.
                        .and_modify(|kind| {
                            if *kind == ChangeKind::Deleted {
                                *kind = ChangeKind::Modified;
                            }
                        })
                        .or_insert(ChangeKind::Added);
                }
                _ => self.partial = true,
            }
        }
    }
}

/// Git's output up to its last whole record when it was cut.
fn complete(bytes: &[u8], cut: bool) -> &[u8] {
    if !cut {
        return bytes;
    }
    bytes
        .iter()
        .rposition(|byte| *byte == 0)
        .and_then(|end| bytes.get(..end))
        .unwrap_or_default()
}

/// A path Git gave, when this app can show it: UTF-8 without control characters.
fn shown(path: &[u8]) -> Option<&str> {
    std::str::from_utf8(path)
        .ok()
        .filter(|path| !path.chars().any(char::is_control))
}

/// A path as Git lists one: relative, `/`-separated, plain names only.
fn plain_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// One side of a file: not there, not a regular file, or its first bytes.
enum Side {
    Absent,
    Special,
    Bytes(Vec<u8>),
}

/// `path` at `base`, read with Git, at most `max + 1` bytes of it.
async fn base_side(git: &Git, root: &Path, base: &str, path: &str, max: usize) -> Result<Side> {
    let unread = || GitError::Message("Git could not read the file at the base commit.".into());
    let listed = git
        .output(
            root,
            args(&["ls-tree", "-z", "-l", "--full-tree", base, "--", path]),
            64 * 1024,
        )
        .await?;
    if exit(&listed) != Some(0) {
        return Err(unread());
    }
    // `<mode> <type> <object> <size>\t<path>`, the size padded with spaces.
    let entry = listed.stdout.split(|byte| *byte == 0).find_map(|record| {
        let tab = record.iter().position(|byte| *byte == b'\t')?;
        (record.get(tab + 1..)? == path.as_bytes())
            .then(|| record.get(..tab))
            .flatten()
    });
    let Some(entry) = entry else {
        return Ok(Side::Absent);
    };
    let fields: Vec<&[u8]> = entry
        .split(|byte| *byte == b' ')
        .filter(|field| !field.is_empty())
        .collect();
    let [mode, kind, object, ..] = fields.as_slice() else {
        return Err(unsupported_entry());
    };
    if *kind != b"blob" || !mode.starts_with(b"100") {
        return Ok(Side::Special);
    }
    let object = std::str::from_utf8(object)
        .ok()
        .filter(|object| oid(object))
        .ok_or_else(unsupported_entry)?;
    let output = git
        .output_with(
            root,
            args(&["cat-file", "blob", object]),
            max + 1,
            Extra {
                allow_cut: true,
                ..Extra::default()
            },
        )
        .await?;
    if exit(&output) != Some(0) {
        return Err(unread());
    }
    Ok(Side::Bytes(output.stdout))
}

/// `path` in the working tree, read from disk inside `root`: each folder is opened without
/// following links, and only a regular file is read, at most `max + 1` bytes of it.
fn working_side(root: &Path, path: &str, max: usize) -> Result<Side> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    use std::io::{ErrorKind, Read};
    let mut directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|_| unreadable())?;
    let mut parts = Path::new(path).components().peekable();
    while let Some(part) = parts.next() {
        let name = part.as_os_str();
        let metadata = match directory.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Side::Absent),
            Err(_) => return Err(unreadable()),
        };
        if parts.peek().is_some() {
            // A link or a file where a folder was: Git sees nothing at this path either.
            if !metadata.is_dir() {
                return Ok(Side::Absent);
            }
            directory = directory
                .open_dir_nofollow(name)
                .map_err(|_| unreadable())?;
            continue;
        }
        if !metadata.is_file() {
            return Ok(Side::Special);
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = directory
            .open_with(name, &options)
            .map_err(|_| unreadable())?;
        if !file.metadata().map_err(|_| unreadable())?.is_file() {
            return Ok(Side::Special);
        }
        let mut bytes = Vec::new();
        file.take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unreadable())?;
        return Ok(Side::Bytes(bytes));
    }
    Ok(Side::Absent)
}

/// Both sides as text, or neither when either is binary.
fn diff(before: Side, after: Side, max: usize) -> BranchDiff {
    let mut binary = false;
    let mut truncated = false;
    let mut as_text = |side: Side| match side {
        Side::Absent => None,
        Side::Special => {
            binary = true;
            None
        }
        Side::Bytes(bytes) => {
            truncated |= bytes.len() > max;
            let text = side_text(bytes, max);
            binary |= text.is_none();
            text
        }
    };
    let (before, after) = (as_text(before), as_text(after));
    if binary {
        return BranchDiff {
            before: None,
            after: None,
            binary,
            truncated,
        };
    }
    BranchDiff {
        before,
        after,
        binary,
        truncated,
    }
}

/// The first `max` bytes as text, cut back to a character boundary. `None` when they are binary:
/// a NUL in the first 8000 bytes, or not UTF-8.
fn side_text(mut bytes: Vec<u8>, max: usize) -> Option<String> {
    let cut = bytes.len() > max;
    bytes.truncate(max);
    if bytes.iter().take(BINARY_PROBE).any(|byte| *byte == 0) {
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        // The cut may split the last character: only that goes.
        Err(error) if cut && error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()
        }
        Err(_) => None,
    }
}

/// The root of the working tree `folder` is in; `None` when it is in none.
async fn work_tree_root(git: &Git, folder: &Path) -> Result<Option<PathBuf>> {
    let folder = folder.canonicalize().map_err(|_| {
        GitError::Message(
            "The working folder is unavailable. Locate it before using Git actions.".into(),
        )
    })?;
    let output = git
        .output(&folder, args(&["rev-parse", "--show-toplevel"]), 8192)
        .await?;
    if exit(&output) != Some(0) {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(text(&output.stdout)?).canonicalize()?))
}

/// The branch checked out at `root`, short name; `None` on a detached HEAD.
async fn current_branch(git: &Git, root: &Path) -> Result<Option<String>> {
    let output = git
        .output(root, args(&["symbolic-ref", "--quiet", "HEAD"]), 4096)
        .await?;
    if exit(&output) != Some(0) {
        return Ok(None);
    }
    Ok(text(&output.stdout)
        .ok()
        .and_then(|reference| reference.strip_prefix("refs/heads/").map(str::to_owned))
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control)))
}

/// `value`, when it is the full id of a commit this repository has.
async fn named_commit(git: &Git, root: &Path, value: &str) -> Result<Option<String>> {
    if !oid(value) {
        return Ok(None);
    }
    let output = git
        .output(
            root,
            vec![
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("{value}^{{commit}}").into(),
            ],
            256,
        )
        .await?;
    if exit(&output) != Some(0) {
        return Ok(None);
    }
    let commit = text(&output.stdout)?;
    Ok(commit.eq_ignore_ascii_case(value).then_some(commit))
}

fn full_commit(base: &str) -> Result<String> {
    if !oid(base) {
        return Err(GitError::Message(
            "The base is not a full commit id.".into(),
        ));
    }
    Ok(base.to_ascii_lowercase())
}

fn exit(output: &HelperOutput) -> Option<i32> {
    output.exit.as_ref().and_then(|exit| exit.code)
}

fn not_a_work_tree() -> GitError {
    GitError::Message("This folder is not a Git working tree.".into())
}

fn unsupported_index() -> GitError {
    GitError::Message("The Git index has an unsupported layout or size.".into())
}

fn unsupported_entry() -> GitError {
    GitError::Message("Git returned an unsupported file entry.".into())
}

fn unreadable() -> GitError {
    GitError::Message("The changed file could not be read.".into())
}
