//! Explicitly reviewed publication through the user's Git and GitHub CLI installations.

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;

use futures::future::BoxFuture;
use ghost_process::{
    EnvKey, HelperEnv, HelperSpec, ResolvedEnv, find_exe, run_helper, run_helper_input,
};
use ghost_protocol::git::{
    ManagedWorktree, PullRequestInput, PullRequestPreview, PullRequestResult,
};
use serde::Deserialize;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use super::{
    GitError, GitWorkspaces, Result,
    git::{Git, args, oid, text},
    inspect::{Inspection, inspect},
};

pub(super) struct PreparedPullRequest {
    record: ManagedWorktree,
    source: Inspection,
    remote: String,
    base_oid: String,
    preview: PullRequestPreview,
    expires: Instant,
}

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

struct GithubCli {
    program: PathBuf,
    env: Arc<ResolvedEnv>,
    registry: Arc<ghost_process::LeftoverRegistry>,
}

pub(super) trait Publisher: Send + Sync {
    fn command<'a>(
        &'a self,
        folder: &'a Path,
        args: Vec<OsString>,
        input: Option<Vec<u8>>,
    ) -> BoxFuture<'a, Result<Vec<u8>>>;
    fn branch<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        remote: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>>;
    fn push<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        args: Vec<OsString>,
    ) -> BoxFuture<'a, Result<()>>;
}

impl Publisher for GithubCli {
    fn command<'a>(
        &'a self,
        folder: &'a Path,
        args: Vec<OsString>,
        input: Option<Vec<u8>>,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(self.run(folder, args, input))
    }
    fn branch<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        remote: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            let pins = super::commit::remote_pins(git, folder, None).await?;
            remote_head(git, folder, remote, branch, &pins).await
        })
    }
    fn push<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        args: Vec<OsString>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut command = super::commit::remote_pins(git, folder, None).await?;
            command.extend(args);
            git.run(folder, command, 32 * 1024).await.map(|_| ())
        })
    }
}

impl GithubCli {
    async fn new(service: &GitWorkspaces) -> Result<Self> {
        let mut env = (*service.env.get().await).clone();
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        for name in [
            "GH_REPO",
            "GH_HOST",
            "GH_DEBUG",
            "GH_FORCE_TTY",
            "GH_EDITOR",
            "GH_PAGER",
        ] {
            env.vars.remove(&EnvKey::from(name));
        }
        let program = find_exe("gh", &env.vars).ok_or_else(|| invalid("Install GitHub CLI from cli.github.com, then sign in with gh auth login before creating a pull request."))?;
        Ok(Self {
            program,
            env: Arc::new(env),
            registry: Arc::clone(&service.registry),
        })
    }

    async fn run(
        &self,
        folder: &Path,
        args: Vec<OsString>,
        input: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let spec = HelperSpec {
            program: self.program.clone(),
            args,
            cwd: folder.into(),
            env: HelperEnv::Resolved(Arc::clone(&self.env)),
            set: vec![
                ("GH_PROMPT_DISABLED".into(), "1".into()),
                ("GH_HOST".into(), "github.com".into()),
                ("NO_COLOR".into(), "1".into()),
            ],
            new_session: true,
            timeout: Duration::from_secs(45),
            max_stdout: 128 * 1024,
        };
        let output = match input {
            Some(input) => run_helper_input(&self.registry, spec, input).await,
            None => run_helper(&self.registry, spec).await,
        }.map_err(|_| invalid("GitHub CLI could not finish. Check its installation and sign-in, then review the remote before retrying."))?;
        if output.timed_out
            || output.stdout_truncated
            || output.exit.and_then(|exit| exit.code) != Some(0)
        {
            return Err(invalid(
                "GitHub CLI could not confirm the request. Check gh auth login and repository access, then refresh to check for an existing pull request before retrying.",
            ));
        }
        Ok(output.stdout)
    }
}

impl GitWorkspaces {
    pub async fn pull_request_preview(
        &self,
        chat: &str,
        worktree: &str,
        input: PullRequestInput,
    ) -> Result<PullRequestPreview> {
        self.pull_request_preview_with(chat, worktree, input, &GithubCli::new(self).await?)
            .await
    }

    pub(super) async fn pull_request_preview_with(
        &self,
        chat: &str,
        worktree: &str,
        input: PullRequestInput,
        cli: &dyn Publisher,
    ) -> Result<PullRequestPreview> {
        validate_input(&input)?;
        let record = self.owned_record(chat, worktree)?;
        let git = self.git().await?;
        let source = inspect(&git, Path::new(&record.path)).await?;
        publishable(&record, &source)?;
        super::commit::refuse_repository_programs(
            &git,
            &source.root,
            super::commit::REMOTE_PROGRAMS,
            "push",
        )
        .await?;
        let (remote, repository) = remote(&git, &source.root).await?;
        let metadata = cli
            .command(
                &source.root,
                args(&[
                    "repo",
                    "view",
                    &repository,
                    "--json",
                    "nameWithOwner,url,defaultBranchRef",
                ]),
                None,
            )
            .await?;
        let metadata: Repository = serde_json::from_slice(&metadata)
            .map_err(|_| invalid("GitHub returned unsupported repository metadata."))?;
        if !metadata.name_with_owner.eq_ignore_ascii_case(&repository)
            || metadata.url != format!("https://github.com/{}", metadata.name_with_owner)
        {
            return Err(invalid(
                "GitHub resolved a different repository. Review the origin remote before publishing.",
            ));
        }
        let base = input
            .base_branch
            .unwrap_or(metadata.default_branch_ref.name);
        validate_branch(&base)?;
        if base == record.branch {
            return Err(invalid(
                "Choose a different base branch for this pull request.",
            ));
        }
        let head = source
            .view
            .head
            .clone()
            .ok_or_else(|| invalid("The worktree has no commit to publish."))?;
        let published_head = cli
            .branch(&git, &source.root, &remote, &record.branch)
            .await?;
        let will_push = push_required(published_head.as_deref(), &head)?;
        let base_oid = cli
            .branch(&git, &source.root, &remote, &base)
            .await?
            .ok_or_else(|| invalid("The chosen base branch does not exist on origin."))?;
        let comparison = format!("{base_oid}...{head}");
        let stat = git
            .run(
                &source.root,
                args(&[
                    "diff",
                    "--stat",
                    "--no-ext-diff",
                    "--no-textconv",
                    &comparison,
                    "--",
                ]),
                32 * 1024,
            )
            .await
            .map_err(|_| {
                invalid(
                    "Fetch the reviewed base branch with Git before preparing this pull request.",
                )
            })?;
        let diff_stat = text(&stat)?;
        if diff_stat.is_empty() {
            return Err(invalid(
                "There are no committed file changes against the chosen base branch.",
            ));
        }
        let existing_url =
            existing(cli, &source.root, &repository, &record.branch, &base, &head).await?;
        let token = crate::ids::new_id();
        let will_push = will_push && existing_url.is_none();
        let warnings = if existing_url.is_some() {
            vec!["An open pull request already contains this exact commit. Apply returns its URL without editing the PR text or publishing another branch.".into()]
        } else {
            vec!["Publish sends the displayed commit and PR text to the reviewed GitHub repository. It never commits local files or merges the pull request.".into()]
        };
        let preview = PullRequestPreview {
            token: token.clone(),
            worktree_id: record.id.clone(),
            repository,
            repository_url: metadata.url,
            head_branch: record.branch.clone(),
            head_commit: head,
            base_branch: base,
            title: input.title,
            body: input.body,
            will_push,
            existing_url,
            diff_stat,
            warnings,
            expires_at: crate::time::now_millis() + 600_000,
        };
        let mut prepared = self
            .pull_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, value| value.expires > Instant::now());
        if prepared.len() >= 16 {
            return Err(invalid("Too many pull request previews are open."));
        }
        prepared.insert(
            token,
            PreparedPullRequest {
                record,
                source,
                remote,
                base_oid,
                preview: preview.clone(),
                expires: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(preview)
    }

    pub async fn pull_request_apply(&self, chat: &str, token: &str) -> Result<PullRequestResult> {
        self.pull_request_apply_with(chat, token, &GithubCli::new(self).await?)
            .await
    }

    pub(super) async fn pull_request_apply_with(
        &self,
        chat: &str,
        token: &str,
        cli: &dyn Publisher,
    ) -> Result<PullRequestResult> {
        let prepared = self
            .pull_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| invalid("This pull request preview expired or was already used."))?;
        if prepared.record.chat_id != chat || prepared.expires <= Instant::now() {
            return Err(invalid("Review a new pull request preview for this chat."));
        }
        let _repository = self.hold_common(&prepared.source.common).await;
        let record = self.owned_record(chat, &prepared.record.id)?;
        let git = self.git().await?;
        let current = inspect(&git, &prepared.source.root).await?;
        publishable(&record, &current)?;
        super::commit::refuse_repository_programs(
            &git,
            &current.root,
            super::commit::REMOTE_PROGRAMS,
            "push",
        )
        .await?;
        if !super::records::unchanged_since_review(&record, &prepared.record)
            || current.fingerprint != prepared.source.fingerprint
            || current.common != prepared.source.common
        {
            return Err(invalid(
                "The worktree changed after preview. Review the pull request again.",
            ));
        }
        let (remote, repository) = remote(&git, &current.root).await?;
        if remote != prepared.remote || repository != prepared.preview.repository {
            return Err(invalid("The origin remote changed after preview."));
        }
        let preview = prepared.preview;
        if let Some(url) = existing(
            cli,
            &current.root,
            &repository,
            &record.branch,
            &preview.base_branch,
            &preview.head_commit,
        )
        .await?
        {
            return Ok(PullRequestResult {
                url,
                created: false,
                head_commit: preview.head_commit,
                warning: None,
            });
        }
        if cli
            .branch(&git, &current.root, &remote, &preview.base_branch)
            .await?
            .as_deref()
            != Some(&prepared.base_oid)
        {
            return Err(invalid(
                "The base branch changed after preview. Review the updated comparison before publishing.",
            ));
        }
        let published_head = cli
            .branch(&git, &current.root, &remote, &record.branch)
            .await?;
        let needs_push = push_required(published_head.as_deref(), &preview.head_commit)?;
        if needs_push != preview.will_push {
            return Err(invalid(
                "The published branch changed after preview. Review again.",
            ));
        }
        if needs_push {
            cli.push(&git, &current.root, push_arguments(&remote, &record.branch, &preview.head_commit)).await
                .map_err(|_| invalid("The reviewed branch could not be published. It may now exist remotely; refresh before retrying. Local files and commits were preserved."))?;
        }
        if cli
            .branch(&git, &current.root, &remote, &record.branch)
            .await?
            .as_deref()
            != Some(&preview.head_commit)
        {
            return Err(invalid(
                "The remote branch no longer matches the reviewed commit. No pull request was created.",
            ));
        }
        let output = cli
            .command(
                &current.root,
                create_arguments(
                    &repository,
                    &record.branch,
                    &preview.base_branch,
                    &preview.title,
                ),
                Some(preview.body.into_bytes()),
            )
            .await?;
        let url = text(&output)?;
        validate_pr_url(&repository, &url)?;
        let warning = match existing(cli, &current.root, &repository, &record.branch, &preview.base_branch, &preview.head_commit).await {
            Ok(Some(confirmed)) if confirmed.eq_ignore_ascii_case(&url) => None,
            _ => Some("The pull request was created, but its latest branch state could not be confirmed. Open the returned URL before taking further action.".into()),
        };
        Ok(PullRequestResult {
            url,
            created: true,
            head_commit: preview.head_commit,
            warning,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
    url: String,
    default_branch_ref: Branch,
}
#[derive(Deserialize)]
struct Branch {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    url: String,
    base_ref_name: String,
    head_ref_oid: String,
    is_cross_repository: bool,
}

async fn existing(
    cli: &dyn Publisher,
    folder: &Path,
    repository: &str,
    branch: &str,
    base: &str,
    head: &str,
) -> Result<Option<String>> {
    let bytes = cli
        .command(
            folder,
            args(&[
                "pr",
                "list",
                "--repo",
                repository,
                "--head",
                branch,
                "--state",
                "open",
                "--limit",
                "2",
                "--json",
                "url,baseRefName,headRefOid,isCrossRepository",
            ]),
            None,
        )
        .await?;
    let prs: Vec<PullRequest> = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("GitHub returned unsupported pull request metadata."))?;
    if prs.is_empty() {
        return Ok(None);
    }
    if prs.len() != 1
        || prs[0].is_cross_repository
        || prs[0].base_ref_name != base
        || prs[0].head_ref_oid != head
    {
        return Err(invalid(
            "An existing pull request uses this branch with different reviewed state. Open it on GitHub before publishing again.",
        ));
    }
    validate_pr_url(repository, &prs[0].url)?;
    Ok(Some(prs[0].url.clone()))
}

fn publishable(record: &ManagedWorktree, source: &Inspection) -> Result<()> {
    if source.view.branch.as_deref() != Some(&record.branch)
        || !source.view.files.is_empty()
        || !source.view.blockers.is_empty()
        || !source.view.is_linked_worktree
    {
        return Err(invalid(
            "Commit the intended work and resolve Git state before publishing. Ghost does not commit files automatically.",
        ));
    }
    Ok(())
}

async fn remote(git: &Git, folder: &Path) -> Result<(String, String)> {
    let bytes = git
        .run(
            folder,
            args(&["remote", "get-url", "--push", "--all", "origin"]),
            8192,
        )
        .await?;
    let remote = text(&bytes)?;
    let repository = repository_from_remote(&remote)?;
    Ok((remote, repository))
}

fn repository_from_remote(remote: &str) -> Result<String> {
    let path = remote.strip_prefix("https://github.com/").or_else(||remote.strip_prefix("git@github.com:")).or_else(||remote.strip_prefix("ssh://git@github.com/"))
        .ok_or_else(||invalid("This build publishes only an origin remote on GitHub.com using HTTPS or Git SSH. Configure that remote explicitly first."))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    {
        return Err(invalid(
            "The origin repository URL is unsupported. Credentials and alternate push destinations are not accepted.",
        ));
    }
    Ok(path.into())
}

/// The commit a remote branch is at, with `pins` (see `commit::remote_pins`) before the command.
pub(super) async fn remote_head(
    git: &Git,
    folder: &Path,
    remote: &str,
    branch: &str,
    pins: &[OsString],
) -> Result<Option<String>> {
    let name = format!("refs/heads/{branch}");
    let mut command = pins.to_vec();
    command.extend(args(&["ls-remote", "--refs", remote, &name]));
    let bytes = git.run(folder, command, 8192).await?;
    let value = text(&bytes)?;
    if value.is_empty() {
        return Ok(None);
    }
    let (commit, found) = value
        .split_once('\t')
        .ok_or_else(|| invalid("Git returned unsupported remote branch data."))?;
    if !oid(commit) || found != name {
        return Err(invalid("The remote branch could not be verified."));
    }
    Ok(Some(commit.into()))
}

fn push_required(remote: Option<&str>, head: &str) -> Result<bool> {
    match remote {
        None => Ok(true),
        Some(value) if value == head => Ok(false),
        Some(_) => Err(invalid(
            "The remote branch has a different commit. Reconcile and push it explicitly with Git before opening this PR.",
        )),
    }
}

fn push_arguments(remote: &str, branch: &str, head: &str) -> Vec<OsString> {
    // An explicitly empty expected value allows creation only, never replacement of a remote ref.
    args(&[
        "-c",
        "push.followTags=false",
        "-c",
        "push.recurseSubmodules=no",
        "push",
        "--porcelain",
        &format!("--force-with-lease=refs/heads/{branch}:"),
        remote,
        &format!("{head}:refs/heads/{branch}"),
    ])
}

fn create_arguments(repository: &str, branch: &str, base: &str, title: &str) -> Vec<OsString> {
    args(&[
        "pr",
        "create",
        "--repo",
        repository,
        "--head",
        branch,
        "--base",
        base,
        "--title",
        title,
        "--body-file",
        "-",
        "--no-maintainer-edit",
    ])
}

fn validate_input(input: &PullRequestInput) -> Result<()> {
    if input.title.trim().is_empty()
        || input.title.len() > 256
        || input.title.chars().any(char::is_control)
        || input.body.len() > 64 * 1024
        || input.body.contains('\0')
    {
        return Err(invalid(
            "Use a title of 1 to 256 bytes and a body no larger than 64 KiB.",
        ));
    }
    if let Some(base) = &input.base_branch {
        validate_branch(base)?;
    }
    Ok(())
}

pub(super) fn validate_branch(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.len() > 200
        || branch.starts_with('-')
        || branch.ends_with('/')
        || branch.contains("..")
        || branch.contains("@{")
        || branch
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || !branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
    {
        return Err(invalid("The base branch name is unsupported."));
    }
    Ok(())
}

fn validate_pr_url(repository: &str, url: &str) -> Result<()> {
    let prefix = format!("https://github.com/{repository}/pull/");
    if !url
        .to_ascii_lowercase()
        .strip_prefix(&prefix.to_ascii_lowercase())
        .is_some_and(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid(
            "The PR result could not be verified. Refresh the repository before retrying; publication may have succeeded.",
        ));
    }
    Ok(())
}
