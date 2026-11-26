//! Reviewed commits and pushes of a chat's folder.
//!
//! Like every Git command Ghost runs, neither runs the repository's hooks (an agent could have
//! written them) nor asks for credentials. A commit records exactly the reviewed files' contents:
//! its tree is built in a private index, and the branch moves only from the reviewed commit, so
//! nothing is staged unless it succeeds. A push only ever fast-forwards the remote branch.
//!
//! The repository's own configuration can name programs Git runs (a signing program, an SSH
//! command, a credential helper). An agent working in the folder could have written it, so a
//! commit is not signed and a push is not made while such a setting comes from there.

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ghost_protocol::git::{
    CommitInput, CommitPreview, CommitResult, GitFileState, PushPreview, PushResult,
};

use super::{
    GitError, GitWorkspaces, Result,
    git::{Extra, Git, args, oid, text},
    inspect::{Inspection, NO_COMMIT, inspect},
    publish::{remote_head, validate_branch},
};

const REVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);
const REVIEWS: usize = 16;

pub(super) struct PreparedCommit {
    root: PathBuf,
    /// The repository's common directory, which its apply holds.
    common: PathBuf,
    branch: String,
    parent: Option<String>,
    paths: Vec<String>,
    tree: String,
    message: String,
    signed: bool,
    expires: Instant,
}

pub(super) struct PreparedPush {
    root: PathBuf,
    /// The repository's common directory, which its apply holds.
    common: PathBuf,
    branch: String,
    head: String,
    remote: String,
    /// Every address Git pushes to, as reviewed; the push is refused if they change.
    urls: Vec<String>,
    target: String,
    remote_head: Option<String>,
    sets_upstream: bool,
    expires: Instant,
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::Message(message.into())
}

fn exit(output: &ghost_process::HelperOutput) -> Option<i32> {
    output.exit.as_ref().and_then(|exit| exit.code)
}

/// The folder's branch, when nothing half done stands in the way. A repository without a commit
/// can still take its first.
fn ready(source: &Inspection) -> Result<String> {
    let branch = source.view.branch.clone().ok_or_else(|| {
        invalid("HEAD is detached. Switch to a branch before committing or pushing.")
    })?;
    if let Some(blocker) = source
        .view
        .blockers
        .iter()
        .find(|blocker| blocker.as_str() != NO_COMMIT)
    {
        return Err(invalid(blocker.clone()));
    }
    Ok(branch)
}

/// The message as Git keeps it: lines without trailing spaces, no blank lines around it or two
/// in a row, and one final newline.
fn clean_message(message: &str) -> Result<String> {
    if message.contains('\0') || message.len() > 64 * 1024 {
        return Err(invalid("Use a commit message no larger than 64 KiB."));
    }
    let mut kept: Vec<&str> = Vec::new();
    for line in message.lines().map(str::trim_end) {
        if line.is_empty() && kept.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|line| line.is_empty()) {
        kept.pop();
    }
    if kept.is_empty() {
        return Err(invalid("Write a commit message."));
    }
    Ok(format!("{}\n", kept.join("\n")))
}

/// The changed files chosen, as Git lists them, and every path the commit takes: a renamed
/// file's old path goes with it, so its removal is committed too.
fn chosen(source: &Inspection, paths: &[String]) -> Result<(Vec<GitFileState>, Vec<String>)> {
    if paths.is_empty() || paths.len() > 20_000 {
        return Err(invalid("Choose at least one changed file to commit."));
    }
    let mut files: Vec<GitFileState> = Vec::new();
    let mut all = BTreeSet::new();
    for path in paths {
        let file = source
            .view
            .files
            .iter()
            .find(|file| &file.path == path)
            .ok_or_else(|| {
                invalid(format!(
                    "{path} is no longer changed. Refresh the Git workspace."
                ))
            })?;
        if !files.contains(file) {
            files.push(file.clone());
        }
        all.insert(file.path.clone());
        if let Some(original) = &file.original_path {
            all.insert(original.clone());
        }
    }
    Ok((files, all.into_iter().collect()))
}

/// Paths for `--pathspec-from-file=- --pathspec-file-nul`, which no command line limits.
fn pathspecs(paths: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for path in paths {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn validate_remote(remote: &str) -> Result<()> {
    if remote.is_empty()
        || remote.len() > 100
        || remote.starts_with(['-', '.', '/'])
        || remote.contains("..")
        || !remote
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
    {
        return Err(invalid(
            "The branch's remote has an unsupported name. Push it with Git directly.",
        ));
    }
    Ok(())
}

/// A remote address to show: any user name or password in it is left out.
pub(super) fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if rest.contains('/') {
        format!("{scheme}://{host}/{path}")
    } else {
        format!("{scheme}://{host}")
    }
}

/// Signing programs Git would run for `commit -S`.
pub(super) const SIGNING_PROGRAMS: &str =
    r"^gpg\.(program|openpgp\.program|x509\.program|ssh\.program|ssh\.defaultkeycommand)$";
/// Clean, smudge and process filters Git would run while staging files.
pub(super) const FILTER_PROGRAMS: &str = r"^filter\..*\.(clean|smudge|process)$";
/// Programs Git would run to reach a remote.
pub(super) const REMOTE_PROGRAMS: &str = r"^(core\.(sshcommand|askpass|gitproxy)|credential\..*helper|remote\..*\.(receivepack|uploadpack|vcs)|protocol\..*allow)$";

/// The settings matching `keys` that the repository's own configuration (its `.git/config`,
/// a worktree's, or a file either includes) sets, which an agent in the folder could have
/// written. Settings of the person's own (global or system) configuration are theirs to keep.
pub(super) async fn repository_programs(git: &Git, root: &Path, keys: &str) -> Result<Vec<String>> {
    let output = git
        .output(
            root,
            args(&["config", "--show-scope", "--includes", "--get-regexp", keys]),
            64 * 1024,
        )
        .await?;
    // Git exits 1 when nothing matches.
    if !matches!(exit(&output), Some(0 | 1)) {
        return Err(invalid("Git could not read this repository's settings."));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (scope, setting) = line.split_once('\t')?;
            matches!(scope, "local" | "worktree").then(|| {
                setting
                    .split_once(' ')
                    .map_or(setting, |(key, _)| key)
                    .to_owned()
            })
        })
        .collect())
}

/// Refuses when the repository's own configuration names a program for this operation.
pub(super) async fn refuse_repository_programs(
    git: &Git,
    root: &Path,
    keys: &str,
    what: &str,
) -> Result<()> {
    let found = repository_programs(git, root, keys).await?;
    if found.is_empty() {
        return Ok(());
    }
    Err(invalid(format!(
        "This repository's own Git settings choose a program Git would run to {what} ({}). Ghost does not run programs an agent could have set; {what} from a terminal instead, or move the setting to your global Git configuration.",
        found.join(", ")
    )))
}

/// The person's own values of `key`, from their system and then global configuration, in the
/// order Git reads them.
async fn own_values(git: &Git, root: &Path, key: &str) -> Result<Vec<String>> {
    let output = git
        .output(
            root,
            args(&["config", "--show-scope", "--includes", "--get-all", key]),
            64 * 1024,
        )
        .await?;
    if !matches!(exit(&output), Some(0 | 1)) {
        return Err(invalid("Git could not read your Git settings."));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (scope, value) = line.split_once('\t')?;
            matches!(scope, "system" | "global").then(|| value.to_owned())
        })
        .collect())
}

/// Settings given on Git's own command line, which outrank every configuration file: each
/// program setting takes the person's own value or Git's default, so nothing the repository's
/// configuration says (even if it changed after it was checked) chooses what runs.
async fn pin(git: &Git, root: &Path, settings: &[(String, String)]) -> Result<Vec<OsString>> {
    let mut pinned = Vec::new();
    for (key, default) in settings {
        let value = own_values(git, root, key)
            .await?
            .pop()
            .unwrap_or_else(|| default.clone());
        pinned.extend([OsString::from("-c"), format!("{key}={value}").into()]);
    }
    Ok(pinned)
}

/// The signing programs, pinned for `commit-tree -S`.
async fn signing_pins(git: &Git, root: &Path) -> Result<Vec<OsString>> {
    // gpg.program is the older name for gpg.openpgp.program; both name the same program.
    let openpgp = match own_values(git, root, "gpg.openpgp.program").await?.pop() {
        Some(program) => program,
        None => own_values(git, root, "gpg.program")
            .await?
            .pop()
            .unwrap_or_else(|| "gpg".into()),
    };
    let mut pinned = pin(
        git,
        root,
        &[
            ("gpg.x509.program".into(), "gpgsm".into()),
            ("gpg.ssh.program".into(), "ssh-keygen".into()),
            ("gpg.ssh.defaultKeyCommand".into(), String::new()),
        ],
    )
    .await?;
    for key in ["gpg.program", "gpg.openpgp.program"] {
        pinned.extend([OsString::from("-c"), format!("{key}={openpgp}").into()]);
    }
    Ok(pinned)
}

/// The programs Git runs to reach a remote, pinned. `remote` names the remote when Git reaches
/// it by name, whose own upload and receive programs then apply.
pub(super) async fn remote_pins(
    git: &Git,
    root: &Path,
    remote: Option<&str>,
) -> Result<Vec<OsString>> {
    let mut settings = vec![
        ("core.sshCommand".to_owned(), "ssh".to_owned()),
        ("core.askPass".to_owned(), String::new()),
        ("core.gitProxy".to_owned(), "none".to_owned()),
    ];
    if let Some(remote) = remote {
        settings.push((
            format!("remote.{remote}.uploadpack"),
            "git-upload-pack".into(),
        ));
        settings.push((
            format!("remote.{remote}.receivepack"),
            "git-receive-pack".into(),
        ));
    }
    let mut pinned = pin(git, root, &settings).await?;
    pinned.extend(args(&["-c", "protocol.ext.allow=never"]));
    // An empty helper empties the list, as Git reads it; then the person's own helpers follow.
    let mut helpers: Vec<String> = Vec::new();
    for helper in own_values(git, root, "credential.helper").await? {
        if helper.is_empty() {
            helpers.clear();
        } else {
            helpers.push(helper);
        }
    }
    pinned.extend(args(&["-c", "credential.helper="]));
    for helper in helpers {
        pinned.extend([
            OsString::from("-c"),
            format!("credential.helper={helper}").into(),
        ]);
    }
    Ok(pinned)
}

/// Whether a push address is a folder on this computer, where pushing runs that repository's
/// own hooks. `host:path` is SSH; a drive letter is a folder.
fn local_address(url: &str) -> bool {
    if url.starts_with("file://") {
        return true;
    }
    if url.contains("://") {
        return false;
    }
    let bytes = url.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    match url.find(':') {
        Some(colon) => url[..colon].contains(['/', '\\']),
        None => true,
    }
}

/// A `helper::address` push address, which runs the Git remote helper it names.
fn remote_helper(url: &str) -> Option<&str> {
    let (helper, _) = url.split_once("::")?;
    (!helper.is_empty()
        && !helper.contains("://")
        && helper
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-._".contains(&byte)))
    .then_some(helper)
}

/// Every address `remote` pushes to, as Git resolves them.
async fn push_urls(git: &Git, root: &Path, remote: &str) -> Result<Vec<String>> {
    let output = git
        .output(
            root,
            args(&["remote", "get-url", "--push", "--all", remote]),
            8192,
        )
        .await?;
    let urls = text(&output.stdout)
        .ok()
        .filter(|_| exit(&output) == Some(0))
        .map(|value| value.lines().map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    if urls.is_empty() {
        return Err(invalid(format!(
            "This repository has no {remote} remote to push to. Add one with git remote add."
        )));
    }
    if let Some(helper) = urls.iter().find_map(|url| remote_helper(url)) {
        return Err(invalid(format!(
            "The {remote} remote is reached through a Git remote helper ({helper}::), a program Ghost does not run. Push it with Git directly."
        )));
    }
    Ok(urls)
}

fn take<T>(
    reviews: &std::sync::Mutex<HashMap<String, T>>,
    token: &str,
    expires: impl Fn(&T) -> Instant,
) -> Result<T> {
    reviews
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(token)
        .filter(|review| expires(review) > Instant::now())
        .ok_or_else(|| invalid("This review expired or was already used. Review it again."))
}

fn hold<T>(
    reviews: &std::sync::Mutex<HashMap<String, T>>,
    token: String,
    review: T,
    expires: impl Fn(&T) -> Instant,
) -> Result<()> {
    let mut reviews = reviews
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    reviews.retain(|_, value| expires(value) > Instant::now());
    if reviews.len() >= REVIEWS {
        return Err(invalid(
            "Too many Git reviews are open. Try again in a few minutes.",
        ));
    }
    reviews.insert(token, review);
    Ok(())
}

impl GitWorkspaces {
    /// Reviews committing the chosen files of the chat's folder as they are now.
    pub async fn commit_preview(&self, folder: &Path, input: CommitInput) -> Result<CommitPreview> {
        let message = clean_message(&input.message)?;
        let git = self.git().await?;
        let source = inspect(&git, folder).await?;
        let branch = ready(&source)?;
        let (files, paths) = chosen(&source, &input.paths)?;
        refuse_repository_programs(&git, &source.root, FILTER_PROGRAMS, "commit").await?;
        let parent = source.view.head.clone();
        let author = git
            .output(&source.root, args(&["var", "GIT_AUTHOR_IDENT"]), 4096)
            .await?;
        let author = text(&author.stdout)
            .ok()
            .filter(|_| exit(&author) == Some(0))
            .and_then(|ident| ident.rsplit_once('>').map(|(name, _)| format!("{name}>")))
            .ok_or_else(|| {
                invalid("Set your Git name and email first: git config --global user.name and user.email.")
            })?;
        let signed = git
            .output(
                &source.root,
                args(&["config", "--bool", "--get", "commit.gpgSign"]),
                256,
            )
            .await
            .is_ok_and(|output| text(&output.stdout).is_ok_and(|value| value == "true"));
        if signed {
            refuse_repository_programs(&git, &source.root, SIGNING_PROGRAMS, "sign commits")
                .await?;
        }
        let (tree, diff_stat) = self
            .stage(&git, &source.root, parent.as_deref(), &paths)
            .await?;
        if let Some(parent) = &parent {
            let before = text(
                &git.run(
                    &source.root,
                    args(&["rev-parse", &format!("{parent}^{{tree}}")]),
                    256,
                )
                .await?,
            )?;
            if before == tree {
                return Err(invalid(
                    "The chosen files have nothing to commit. Refresh the Git workspace.",
                ));
            }
        }
        let token = crate::ids::new_id();
        let mut warnings = vec!["Git hooks do not run: Ghost commits exactly the reviewed files. Run your project's checks yourself if it relies on them.".to_owned()];
        if signed {
            warnings.push(
                "Your Git signs commits, so its signing program may ask for your passphrase."
                    .into(),
            );
        }
        hold(
            &self.commits,
            token.clone(),
            PreparedCommit {
                root: source.root.clone(),
                common: source.common.clone(),
                branch: branch.clone(),
                parent: parent.clone(),
                paths,
                tree,
                message: message.clone(),
                signed,
                expires: Instant::now() + REVIEW_LIFETIME,
            },
            |review| review.expires,
        )?;
        Ok(CommitPreview {
            token,
            branch,
            parent,
            author,
            signed,
            files,
            message,
            diff_stat,
            warnings,
            expires_at: crate::time::now_millis() + 10 * 60 * 1000,
        })
    }

    /// Commits the reviewed files, when the branch and their contents are still as reviewed.
    pub async fn commit_apply(&self, folder: &Path, token: &str) -> Result<CommitResult> {
        let prepared = take(&self.commits, token, |review| review.expires)?;
        let _repository = self.hold_common(&prepared.common).await;
        let git = self.git().await?;
        let source = inspect(&git, folder).await?;
        let branch = ready(&source)?;
        if source.root != prepared.root
            || branch != prepared.branch
            || source.view.head != prepared.parent
        {
            return Err(invalid(
                "The branch moved since the review. Review the commit again.",
            ));
        }
        refuse_repository_programs(&git, &source.root, FILTER_PROGRAMS, "commit").await?;
        let (tree, _) = self
            .stage(
                &git,
                &source.root,
                prepared.parent.as_deref(),
                &prepared.paths,
            )
            .await?;
        if tree != prepared.tree {
            return Err(invalid(
                "The files changed since the review. Review the commit again.",
            ));
        }
        let mut command = Vec::new();
        if prepared.signed {
            refuse_repository_programs(&git, &source.root, SIGNING_PROGRAMS, "sign commits")
                .await?;
            command = signing_pins(&git, &source.root).await?;
        }
        command.extend(args(&["commit-tree", &tree]));
        if let Some(parent) = &prepared.parent {
            command.extend(args(&["-p", parent]));
        }
        if prepared.signed {
            command.push("-S".into());
        }
        let made = git
            .output_with(
                &source.root,
                command,
                256,
                Extra {
                    input: Some(prepared.message.clone().into_bytes()),
                    timeout: Some(Duration::from_secs(120)),
                    ..Extra::default()
                },
            )
            .await?;
        let commit = text(&made.stdout).unwrap_or_default();
        if exit(&made) != Some(0) || !oid(&commit) {
            return Err(invalid(if prepared.signed {
                "Git could not sign the commit. Check that signing works in a terminal, then try again."
            } else {
                "Git could not create the commit. Check your Git name and email, then try again."
            }));
        }
        // The branch moves only from the reviewed commit; a repository's first commit only creates it.
        let old = match &prepared.parent {
            Some(parent) => parent.clone(),
            None => "0".repeat(commit.len()),
        };
        let subject = prepared.message.lines().next().unwrap_or_default();
        git.run(
            &source.root,
            args(&[
                "update-ref",
                "-m",
                &format!("commit: {subject}"),
                &format!("refs/heads/{branch}"),
                &commit,
                &old,
            ]),
            256,
        )
        .await
        .map_err(|_| {
            invalid("The branch moved while committing, so it was left as it is. Review the commit again.")
        })?;
        // The person's index now agrees with the commit for these files; their other staged
        // changes stay as they were.
        let synced = git
            .output_with(
                &source.root,
                args(&[
                    "reset",
                    "--quiet",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]),
                4096,
                Extra {
                    input: Some(pathspecs(&prepared.paths)),
                    ..Extra::default()
                },
            )
            .await
            .is_ok_and(|output| exit(&output) == Some(0));
        Ok(CommitResult {
            commit,
            branch,
            warning: (!synced).then(|| {
                "The commit was made, but Git's index still lists these files as staged. Run git status to check.".into()
            }),
        })
    }

    /// The tree the chosen paths make on top of `parent`, built in a private index so the
    /// person's own index is untouched, with the change's stat.
    async fn stage(
        &self,
        git: &Git,
        root: &Path,
        parent: Option<&str>,
        paths: &[String],
    ) -> Result<(String, String)> {
        let folder = self.root.join("indexes");
        crate::fsutil::ensure_private_dir(&folder)?;
        let index = folder.join(format!("{}.index", crate::ids::new_id()));
        let result = stage_in(git, root, parent, paths, &index).await;
        let _ = std::fs::remove_file(&index);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}.lock", index.display())));
        result
    }

    /// Reviews a fast-forward push of the folder's branch to its upstream, or to origin.
    pub async fn push_preview(&self, folder: &Path) -> Result<PushPreview> {
        let git = self.git().await?;
        let source = inspect(&git, folder).await?;
        let branch = ready(&source)?;
        let head = source
            .view
            .head
            .clone()
            .ok_or_else(|| invalid("This branch has no commit to push."))?;
        let root = source.root.clone();
        let setting = |key: String| {
            let git = &git;
            let root = &root;
            async move {
                let output = git
                    .output(root, args(&["config", "--get", &key]), 4096)
                    .await?;
                Ok::<_, GitError>(
                    text(&output.stdout)
                        .ok()
                        .filter(|value| exit(&output) == Some(0) && !value.is_empty()),
                )
            }
        };
        let upstream = (
            setting(format!("branch.{branch}.remote")).await?,
            setting(format!("branch.{branch}.merge")).await?,
        );
        let (remote, target, sets_upstream) = match upstream {
            (Some(remote), Some(merge)) if remote != "." => {
                let target = merge.strip_prefix("refs/heads/").ok_or_else(|| {
                    invalid("The branch's upstream is not a branch. Push it with Git directly.")
                })?;
                (remote, target.to_owned(), false)
            }
            _ => ("origin".to_owned(), branch.clone(), true),
        };
        validate_remote(&remote)?;
        // Before Git reaches the remote, which runs any program the settings name.
        refuse_repository_programs(&git, &root, REMOTE_PROGRAMS, "push").await?;
        validate_branch(&target).map_err(|_| {
            invalid("The remote branch's name is unsupported. Push it with Git directly.")
        })?;
        let urls = push_urls(&git, &root, &remote).await?;
        let pins = remote_pins(&git, &root, Some(&remote)).await?;
        let remote_head = remote_head(&git, &root, &remote, &target, &pins).await.map_err(|_| {
            invalid(format!(
                "Git could not reach {remote}. If it needs a sign-in, run git fetch {remote} in a terminal once so Git keeps it, then try again."
            ))
        })?;
        if remote_head.as_deref() == Some(head.as_str()) {
            return Err(invalid(format!(
                "{remote}/{target} already has this branch's latest commit."
            )));
        }
        let range = match &remote_head {
            Some(remote_head) => {
                let check = git
                    .output(
                        &root,
                        args(&["merge-base", "--is-ancestor", remote_head, &head]),
                        256,
                    )
                    .await?;
                match exit(&check) {
                    Some(0) => {}
                    Some(1) => {
                        return Err(invalid(format!(
                            "{remote}/{target} has commits this branch does not. Pull or rebase with Git, then push."
                        )));
                    }
                    _ => {
                        return Err(invalid(format!(
                            "This folder has not seen {remote}/{target}'s latest commit. Fetch with Git, then push."
                        )));
                    }
                }
                vec![OsString::from(format!("{remote_head}..{head}"))]
            }
            None => args(&[&head, "--not", &format!("--remotes={remote}")]),
        };
        let mut count = args(&["rev-list", "--count"]);
        count.extend(range.iter().cloned());
        let count = text(&git.run(&root, count, 256).await?)?
            .parse::<u32>()
            .map_err(|_| invalid("Git returned an unsupported commit count."))?;
        let mut log = args(&["log", "--no-color", "--format=%h %s", "-n", "20"]);
        log.extend(range);
        let commits = text(&git.run(&root, log, 64 * 1024).await?)?
            .lines()
            .map(str::to_owned)
            .collect();
        let token = crate::ids::new_id();
        let mut warnings = vec!["Push sends only these commits. It never forces: if the remote branch moves first, Git refuses and nothing changes.".to_owned()];
        if urls.iter().any(|url| local_address(url)) {
            warnings.push("This remote is a folder on this computer. Pushing to it runs that repository's own Git hooks, outside any agent's sandbox, so push there only if you trust it.".into());
        }
        hold(
            &self.pushes,
            token.clone(),
            PreparedPush {
                common: source.common.clone(),
                root,
                branch: branch.clone(),
                head: head.clone(),
                remote: remote.clone(),
                urls: urls.clone(),
                target: target.clone(),
                remote_head: remote_head.clone(),
                sets_upstream,
                expires: Instant::now() + REVIEW_LIFETIME,
            },
            |review| review.expires,
        )?;
        Ok(PushPreview {
            token,
            branch,
            remote,
            remote_urls: urls.iter().map(|url| without_credentials(url)).collect(),
            target,
            head,
            remote_head,
            commits,
            count,
            sets_upstream,
            warnings,
            expires_at: crate::time::now_millis() + 10 * 60 * 1000,
        })
    }

    /// Pushes the reviewed commit, fast-forward only, when the branch is still where it was.
    pub async fn push_apply(&self, folder: &Path, token: &str) -> Result<PushResult> {
        let prepared = take(&self.pushes, token, |review| review.expires)?;
        let _repository = self.hold_common(&prepared.common).await;
        let git = self.git().await?;
        let source = inspect(&git, folder).await?;
        if source.root != prepared.root
            || source.view.branch.as_deref() != Some(prepared.branch.as_str())
            || source.view.head.as_deref() != Some(prepared.head.as_str())
        {
            return Err(invalid(
                "The branch moved since the review. Review the push again.",
            ));
        }
        refuse_repository_programs(&git, &prepared.root, REMOTE_PROGRAMS, "push").await?;
        if push_urls(&git, &prepared.root, &prepared.remote).await? != prepared.urls {
            return Err(invalid(
                "The remote's address changed since the review. Review the push again.",
            ));
        }
        // The remote branch must still be as reviewed (absent for a new one). The reviewed
        // commit descends from it, so this lease never lets the push replace anything.
        let lease = format!(
            "--force-with-lease=refs/heads/{}:{}",
            prepared.target,
            prepared.remote_head.as_deref().unwrap_or_default()
        );
        let mut command = remote_pins(&git, &prepared.root, Some(&prepared.remote)).await?;
        command.extend(args(&[
            "-c",
            "push.followTags=false",
            "-c",
            "push.recurseSubmodules=no",
            "push",
            "--porcelain",
            &lease,
            &prepared.remote,
            &format!("{}:refs/heads/{}", prepared.head, prepared.target),
        ]));
        let output = git
            .output_with(
                &prepared.root,
                command,
                32 * 1024,
                Extra {
                    timeout: Some(Duration::from_secs(180)),
                    ..Extra::default()
                },
            )
            .await?;
        if exit(&output) != Some(0) {
            let report = String::from_utf8_lossy(&output.stdout);
            let remote = &prepared.remote;
            return Err(invalid(if report.contains("[remote rejected]") {
                format!(
                    "{remote} refused the push. Its branch may be protected; check it with Git."
                )
            } else if report.contains("[rejected]") {
                format!(
                    "{remote} refused the push because its branch changed since the review. Fetch with Git and review again."
                )
            } else {
                format!(
                    "Git could not push to {remote}. If it needs a sign-in, push once in a terminal so Git keeps it, then try again."
                )
            }));
        }
        let mut warning = None;
        if prepared.sets_upstream {
            for (key, value) in [
                (
                    format!("branch.{}.remote", prepared.branch),
                    prepared.remote.clone(),
                ),
                (
                    format!("branch.{}.merge", prepared.branch),
                    format!("refs/heads/{}", prepared.target),
                ),
            ] {
                if git
                    .run(&prepared.root, args(&["config", &key, &value]), 256)
                    .await
                    .is_err()
                {
                    warning = Some("Pushed, but Git could not record the upstream branch. Set it with git branch --set-upstream-to.".into());
                }
            }
        }
        Ok(PushResult {
            remote: prepared.remote,
            target: prepared.target,
            head: prepared.head,
            created: prepared.remote_head.is_none(),
            warning,
        })
    }
}

async fn stage_in(
    git: &Git,
    root: &Path,
    parent: Option<&str>,
    paths: &[String],
    index: &Path,
) -> Result<(String, String)> {
    let private = |input: Option<Vec<u8>>| Extra {
        set: vec![("GIT_INDEX_FILE".into(), index.as_os_str().to_owned())],
        input,
        ..Extra::default()
    };
    let run = |command: Vec<OsString>, maximum: usize, input: Option<Vec<u8>>| async move {
        let output = git
            .output_with(root, command, maximum, private(input))
            .await?;
        if exit(&output) != Some(0) {
            return Err(invalid(
                "Git could not read the chosen files. Refresh the Git workspace and try again.",
            ));
        }
        Ok(output.stdout)
    };
    match parent {
        Some(parent) => run(args(&["read-tree", parent]), 256, None).await?,
        None => run(args(&["read-tree", "--empty"]), 256, None).await?,
    };
    run(
        args(&[
            "add",
            "--all",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ]),
        4096,
        Some(pathspecs(paths)),
    )
    .await?;
    let tree = text(&run(args(&["write-tree"]), 256, None).await?)?;
    if !oid(&tree) {
        return Err(invalid("Git returned an unsupported tree."));
    }
    let stat = text(
        &run(
            args(&[
                "diff",
                "--cached",
                "--stat",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
            ]),
            64 * 1024,
            None,
        )
        .await?,
    )?;
    Ok((tree, stat))
}
