//! Reviewed, private-only repository creation with durable recovery before remote mutations.
mod client;
mod storage;
#[cfg(test)]
mod tests;
use super::{
    GitError, GitWorkspaces, Result,
    git::{Git, args, oid, text},
    inspect::{Inspection, inspect},
};
use client::{Client, Remote, Response};
use ghost_protocol::repository_publication::{
    RepositoryPublicationInput as Input, RepositoryPublicationResult as Outcome,
    RepositoryPublicationReview as Review, RepositoryPublicationState as State,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Creating,
    Created,
    Pushing,
    Pushed,
    Complete,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    id: String,
    folder_id: String,
    path: String,
    common: String,
    repository: String,
    branch: String,
    head: String,
    repository_id: Option<u64>,
    phase: Phase,
}
pub(super) struct Prepared {
    source: Inspection,
    record: Record,
    review: Review,
    expires: Instant,
    viewer: String,
    organization: bool,
    remote_id: Option<u64>,
}
#[derive(Deserialize)]
struct Repository {
    id: u64,
    full_name: String,
    html_url: String,
    private: bool,
    archived: bool,
    fork: bool,
    permissions: Option<Permissions>,
}
#[derive(Deserialize)]
struct Permissions {
    push: bool,
}
#[derive(Deserialize)]
struct User {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

impl GitWorkspaces {
    pub async fn repository_publication_draft(
        &self,
        folder_id: &str,
    ) -> Result<Option<ghost_protocol::repository_publication::RepositoryPublicationDraft>> {
        self.read_publications()?
            .into_iter()
            .find(|entry| entry.folder_id == folder_id)
            .map(|entry| {
                let (owner, name) = entry
                    .repository
                    .split_once('/')
                    .ok_or_else(|| invalid("The saved repository target is invalid."))?;
                Ok(
                    ghost_protocol::repository_publication::RepositoryPublicationDraft {
                        input: Input {
                            owner: owner.into(),
                            name: name.into(),
                        },
                        unfinished: entry.phase != Phase::Complete,
                    },
                )
            })
            .transpose()
    }
    pub async fn repository_publication_review(
        &self,
        folder_id: &str,
        path: &Path,
        input: Input,
    ) -> Result<Review> {
        let client = match Client::new(self).await {
            Ok(client) => client,
            Err(error) => {
                // Local initialization/commit guidance takes priority over installing gh.
                publication_source(&self.git().await?, path).await?;
                return Err(error);
            }
        };
        self.publication_review_with(folder_id, path, input, &client)
            .await
    }
    pub async fn repository_publication_apply(
        &self,
        folder_id: &str,
        path: &Path,
        token: &str,
    ) -> Result<Outcome> {
        let canonical = path.canonicalize()?;
        {
            let previews = self
                .publications
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !previews.get(token).is_some_and(|preview| {
                preview.source.root == canonical && preview.record.folder_id == folder_id
            }) {
                return Err(invalid(
                    "The project folder changed. Review publication again.",
                ));
            }
        }
        self.publication_apply_with(folder_id, token, &Client::new(self).await?)
            .await
    }
    async fn publication_review_with(
        &self,
        folder_id: &str,
        path: &Path,
        input: Input,
        remote: &dyn Remote,
    ) -> Result<Review> {
        validate_name(&input.name, 100)?;
        if !input.owner.is_empty() {
            validate_owner(&input.owner)?;
        }
        if folder_id.is_empty() || folder_id.len() > 128 {
            return Err(invalid("The project folder ID is invalid."));
        }
        let git = self.git().await?;
        let source = publication_source(&git, path).await?;
        let viewer: User = decode(
            remote
                .request(&source.root, "GET", "user", Vec::new())
                .await?,
            200,
        )?;
        validate_owner(&viewer.login)?;
        let owner = if input.owner.is_empty() {
            viewer.login.clone()
        } else {
            input.owner
        };
        let organization = if owner.eq_ignore_ascii_case(&viewer.login) {
            false
        } else {
            let user: User = decode(
                remote
                    .request(&source.root, "GET", &format!("users/{owner}"), Vec::new())
                    .await?,
                200,
            )?;
            if user.kind != "Organization" || !user.login.eq_ignore_ascii_case(&owner) {
                return Err(invalid(
                    "Choose your signed-in account or a GitHub organization where you can create repositories.",
                ));
            }
            true
        };
        let repository = format!("{owner}/{}", input.name);
        let records = self.read_publications()?;
        let previous = records.iter().find(|entry| entry.folder_id == folder_id);
        if let Some(previous) = previous
            && (!previous.repository.eq_ignore_ascii_case(&repository)
                || previous.path != source.view.root
                || previous.common != source.common.to_string_lossy()
                || Some(&previous.head) != source.view.head.as_ref()
                || Some(&previous.branch) != source.view.branch.as_ref())
        {
            return Err(invalid(
                "This folder has an earlier publication record. Review its original repository and commit before starting another publication.",
            ));
        }
        let origin = origin(&git, &source.root).await?;
        let expected_url = format!("https://github.com/{repository}.git");
        if origin.is_some() && (previous.is_none() || origin.as_deref() != Some(&expected_url)) {
            return Err(invalid(
                "This repository already has an origin. Use its existing publication controls; no remote was changed.",
            ));
        }
        let found = repository_state(remote, &source.root, &repository).await?;
        if found.is_some() && previous.is_none() {
            return Err(invalid(
                "That repository already exists. Choose a new private repository name.",
            ));
        }
        if let Some(previous) = previous
            && let Some(expected) = previous.repository_id
            && found.as_ref().map(|repo| repo.id) != Some(expected)
        {
            return Err(invalid(
                "The saved GitHub repository identity changed or became unavailable. No remote was modified.",
            ));
        }
        let head = source.view.head.clone().ok_or_else(|| {
            invalid("Create the first commit explicitly before publishing this repository.")
        })?;
        let branch = source
            .view
            .branch
            .clone()
            .ok_or_else(|| invalid("Choose a branch before publishing."))?;
        let branch_head = if found.is_some() {
            remote_branch(remote, &source.root, &repository, &branch).await?
        } else {
            None
        };
        if branch_head.as_ref().is_some_and(|current| current != &head) {
            return Err(invalid(
                "The destination branch contains a different commit. No force push or overwrite is available here.",
            ));
        }
        let record = previous.cloned().unwrap_or(Record {
            id: crate::ids::new_id(),
            folder_id: folder_id.into(),
            path: source.view.root.clone(),
            common: source.common.to_string_lossy().into_owned(),
            repository: repository.clone(),
            branch: branch.clone(),
            head: head.clone(),
            repository_id: None,
            phase: Phase::Creating,
        });
        let mut notices=vec!["Creates a private GitHub repository and publishes the reviewed commit and its ancestors. Review committed content before confirming.".into(),"Staged, unstaged and untracked files remain on this device. Ghost does not stage or commit them.".into(),"Adds origin only after the remote commit is confirmed. Existing origins and remote branches are never overwritten.".into()];
        if previous.is_some() {
            notices.push("An earlier publication was recorded. Confirm the repository and current remote state to finish it; uncertain requests are not repeated automatically.".into());
        }
        if found.is_some() && record.repository_id.is_none() {
            notices.push("The earlier create result was uncertain. Confirm using the existing private repository shown here before publishing the reviewed branch.".into());
        }
        let review = Review {
            token: crate::ids::new_id(),
            folder_id: folder_id.into(),
            path: source.view.root.clone(),
            repository: repository.clone(),
            url: format!("https://github.com/{repository}"),
            branch,
            head_commit: head,
            private: true,
            recovering: previous.is_some(),
            will_create: found.is_none(),
            will_push: branch_head.is_none(),
            notices,
        };
        let prepared = Prepared {
            source,
            record,
            review: review.clone(),
            expires: Instant::now() + Duration::from_secs(300),
            viewer: viewer.login,
            organization,
            remote_id: found.map(|repo| repo.id),
        };
        let mut previews = self
            .publications
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        previews.retain(|_, preview| {
            preview.expires > Instant::now() && preview.record.folder_id != folder_id
        });
        if previews.len() >= 32 {
            return Err(invalid(
                "Too many publication reviews are open. Close them or wait five minutes.",
            ));
        }
        previews.insert(review.token.clone(), prepared);
        Ok(review)
    }
    async fn publication_apply_with(
        &self,
        folder_id: &str,
        token: &str,
        remote: &dyn Remote,
    ) -> Result<Outcome> {
        let prepared = self
            .publications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(token)
            .ok_or_else(|| invalid("Review this publication again before continuing."))?;
        if prepared.expires <= Instant::now() || prepared.record.folder_id != folder_id {
            return Err(invalid(
                "This publication review expired or belongs to another folder.",
            ));
        }
        let _repository = self.hold_common(&prepared.source.common).await;
        let git = self.git().await?;
        let source = publication_source(&git, &prepared.source.root).await?;
        if source.fingerprint != prepared.source.fingerprint {
            return Err(invalid(
                "The repository changed after review. Review the publication again.",
            ));
        }
        let viewer: User = decode(
            remote
                .request(&source.root, "GET", "user", Vec::new())
                .await?,
            200,
        )?;
        if !viewer.login.eq_ignore_ascii_case(&prepared.viewer) {
            return Err(invalid(
                "The signed-in GitHub account changed. Review the target again.",
            ));
        }
        let mut record = prepared.record;
        let url = format!("https://github.com/{}.git", record.repository);
        let current_origin = origin(&git, &source.root).await?;
        if current_origin
            .as_ref()
            .is_some_and(|current| current != &url)
        {
            return Err(invalid(
                "An origin was added or changed after review. It was preserved.",
            ));
        }
        let found = repository_state(remote, &source.root, &record.repository).await?;
        if found.as_ref().map(|repo| repo.id) != prepared.remote_id {
            return Err(invalid(
                "The remote changed after review. Review the publication again.",
            ));
        }
        let mut records = self.read_publications()?;
        if let Some(previous) = records.iter().find(|entry| entry.folder_id == folder_id) {
            if previous != &record {
                return Err(invalid("The saved publication changed. Review it again."));
            }
        } else if records.len() >= 256 {
            return Err(invalid(
                "This profile's publication recovery journal is full.",
            ));
        }
        if let Some(found) = found {
            record.repository_id = Some(found.id);
        }
        self.save_publication(&mut records, &record)?;
        if prepared.review.will_create {
            let owner = record
                .repository
                .split_once('/')
                .ok_or_else(|| invalid("The target repository is invalid."))?
                .0;
            let endpoint = if prepared.organization {
                format!("orgs/{owner}/repos")
            } else {
                "user/repos".into()
            };
            let name = record
                .repository
                .split_once('/')
                .map(|(_, name)| name)
                .ok_or_else(|| invalid("The target repository is invalid."))?;
            let created = remote
                .request(
                    &source.root,
                    "POST",
                    &endpoint,
                    vec![("name".into(), name.into())],
                )
                .await;
            let created = created
                .and_then(|response| decode::<Repository>(response, 201))
                .and_then(|repo| validate_repository(repo, &record.repository, false));
            let Ok(created) = created else {
                return Ok(outcome(
                    &record,
                    State::NeedsReview,
                    "GitHub did not confirm repository creation. Review this publication to check the remote before trying again.",
                ));
            };
            record.repository_id = Some(created.id);
            record.phase = Phase::Created;
            self.save_publication(&mut records, &record)?;
        }
        let state = repository_state(remote, &source.root, &record.repository).await;
        if !matches!(state,Ok(Some(ref repo)) if Some(repo.id)==record.repository_id) {
            return Ok(outcome(
                &record,
                State::NeedsReview,
                "The private repository could not be confirmed. Review its state before continuing.",
            ));
        }
        let branch = remote_branch(remote, &source.root, &record.repository, &record.branch).await;
        let branch = match branch {
            Ok(value) => value,
            Err(_) => {
                return Ok(outcome(
                    &record,
                    State::NeedsReview,
                    "The remote branch could not be confirmed. Review before continuing.",
                ));
            }
        };
        if branch.as_ref().is_some_and(|head| head != &record.head) {
            return Ok(outcome(
                &record,
                State::NeedsReview,
                "The remote branch changed. Existing remote work was preserved.",
            ));
        }
        if branch.is_none() {
            // A preview that saw an existing branch cannot silently recreate a removed branch.
            if !prepared.review.will_push {
                return Ok(outcome(
                    &record,
                    State::NeedsReview,
                    "The reviewed branch disappeared. Review before publishing it again.",
                ));
            }
            record.phase = Phase::Pushing;
            self.save_publication(&mut records, &record)?;
            if remote
                .push(&git, &source.root, &url, &record.branch, &record.head)
                .await
                .is_err()
            {
                return Ok(outcome(
                    &record,
                    State::NeedsReview,
                    "Git did not confirm the push. Review the remote before retrying; local files and the index were preserved.",
                ));
            }
        }
        if !matches!(remote_branch(remote,&source.root,&record.repository,&record.branch).await,Ok(Some(ref head)) if head==&record.head)
        {
            return Ok(outcome(
                &record,
                State::NeedsReview,
                "The pushed commit could not be confirmed. Review the remote before continuing.",
            ));
        }
        if !matches!(repository_state(remote,&source.root,&record.repository).await,Ok(Some(ref repo)) if Some(repo.id)==record.repository_id)
        {
            return Ok(outcome(
                &record,
                State::NeedsReview,
                "The published repository identity or visibility changed. Review it on GitHub before finishing.",
            ));
        }
        record.phase = Phase::Pushed;
        self.save_publication(&mut records, &record)?;
        match origin(&git, &source.root).await? {
            Some(current) if current == url => {}
            Some(_) => {
                return Ok(outcome(
                    &record,
                    State::NeedsReview,
                    "The commit is published, but a different origin was added. That origin was preserved.",
                ));
            }
            None => {
                if git
                    .run(&source.root, args(&["remote", "add", "origin", &url]), 8192)
                    .await
                    .is_err()
                {
                    return Ok(outcome(
                        &record,
                        State::NeedsReview,
                        "The commit is published, but origin could not be added. Review before finishing the local mapping.",
                    ));
                }
            }
        }
        record.phase = Phase::Complete;
        self.save_publication(&mut records, &record)?;
        Ok(outcome(
            &record,
            State::Complete,
            "The reviewed commit is published in a private repository. Local uncommitted files remain unchanged.",
        ))
    }
}
fn outcome(record: &Record, state: State, detail: &str) -> Outcome {
    Outcome {
        folder_id: record.folder_id.clone(),
        url: format!("https://github.com/{}", record.repository),
        repository: record.repository.clone(),
        head_commit: record.head.clone(),
        state,
        detail: detail.into(),
    }
}
async fn publication_source(git: &Git, path: &Path) -> Result<Inspection> {
    let canonical = path.canonicalize()?;
    if git.protected.iter().any(|path| {
        path.canonicalize()
            .is_ok_and(|path| canonical.starts_with(path))
    }) {
        return Err(invalid(
            "Agent configuration folders cannot be published as project workspaces.",
        ));
    }
    let source = match inspect(git, &canonical).await {
        Err(GitError::Message(message)) if message.contains("not a Git working tree") => {
            return Err(invalid(
                "Initialize this folder as Git explicitly, then create its first commit before publishing.",
            ));
        }
        other => other?,
    };
    if source.view.head.is_none() {
        return Err(invalid(
            "Create the first commit explicitly with the existing commit flow before publishing. Nothing was staged or committed.",
        ));
    }
    if source.view.branch.is_none()
        || !source.view.blockers.is_empty()
        || source.view.is_linked_worktree
    {
        return Err(invalid(
            "Choose a regular repository branch and finish pending Git operations before publishing.",
        ));
    }
    Ok(source)
}
async fn origin(git: &Git, path: &Path) -> Result<Option<String>> {
    let output = git
        .output(
            path,
            args(&[
                "config",
                "--local",
                "--no-includes",
                "--get-all",
                "remote.origin.url",
            ]),
            8192,
        )
        .await?;
    match output.exit.and_then(|exit| exit.code) {
        Some(1) => {
            let names = text(&git.run(path, args(&["remote"]), 8192).await?)?;
            if names.lines().any(|name| name == "origin") {
                return Err(invalid(
                    "An origin with inherited or unsupported configuration already exists. It was preserved.",
                ));
            }
            Ok(None)
        }
        Some(0) => Ok(Some(text(&output.stdout)?)),
        _ => Err(invalid("The origin configuration could not be inspected.")),
    }
}
fn validate_owner(owner: &str) -> Result<()> {
    if owner.is_empty()
        || owner.len() > 39
        || owner.starts_with('-')
        || owner.ends_with('-')
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Err(invalid("Enter a valid GitHub user or organization."))
    } else {
        Ok(())
    }
}
fn validate_name(name: &str, max: usize) -> Result<()> {
    if name.is_empty()
        || name.len() > max
        || matches!(name, "." | "..")
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        Err(invalid(
            "Use a repository name with letters, numbers, dots, underscores or hyphens.",
        ))
    } else {
        Ok(())
    }
}
fn decode<T: serde::de::DeserializeOwned>(response: Response, expected: u16) -> Result<T> {
    if response.status != expected {
        return Err(invalid(
            "GitHub did not confirm this request. Check sign-in, repository access and the reviewed target.",
        ));
    }
    serde_json::from_value(response.body)
        .map_err(|_| invalid("GitHub returned unsupported metadata."))
}
fn validate_repository(repo: Repository, expected: &str, require_push: bool) -> Result<Repository> {
    if repo.id == 0
        || !repo.full_name.eq_ignore_ascii_case(expected)
        || repo.html_url != format!("https://github.com/{}", repo.full_name)
        || !repo.private
        || repo.archived
        || repo.fork
        || (require_push
            && !repo
                .permissions
                .as_ref()
                .is_some_and(|permissions| permissions.push))
    {
        return Err(invalid(
            "The target is not the reviewed writable private repository. It was left unchanged.",
        ));
    }
    Ok(repo)
}
async fn repository_state(
    remote: &dyn Remote,
    path: &Path,
    repository: &str,
) -> Result<Option<Repository>> {
    let response = remote
        .request(path, "GET", &format!("repos/{repository}"), Vec::new())
        .await?;
    if response.status == 404 {
        return Ok(None);
    }
    Ok(Some(validate_repository(
        decode(response, 200)?,
        repository,
        true,
    )?))
}
async fn remote_branch(
    remote: &dyn Remote,
    path: &Path,
    repository: &str,
    branch: &str,
) -> Result<Option<String>> {
    let encoded: String = branch
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    let response = remote
        .request(
            path,
            "GET",
            &format!("repos/{repository}/git/ref/heads/{encoded}"),
            Vec::new(),
        )
        .await?;
    if matches!(response.status, 404 | 409) {
        return Ok(None);
    }
    let value: serde_json::Value = decode(response, 200)?;
    let sha = value
        .pointer("/object/sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| oid(sha))
        .ok_or_else(|| invalid("GitHub returned an invalid branch commit."))?;
    if value.get("ref").and_then(serde_json::Value::as_str) != Some(&format!("refs/heads/{branch}"))
        || value
            .pointer("/object/type")
            .and_then(serde_json::Value::as_str)
            != Some("commit")
    {
        return Err(invalid("GitHub returned a different branch reference."));
    }
    Ok(Some(sha.into()))
}
