//! Portable identity reads for an explicitly selected folder. No remote access or index writes.

use std::path::Path;

use super::{
    GitWorkspaces, Result,
    git::{args, text},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderIdentity {
    Local,
    Git {
        remote: String,
        default_branch: Option<String>,
    },
}

impl FolderIdentity {
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::Local => 0,
            Self::Git {
                remote,
                default_branch,
            } => remote.len() + default_branch.as_ref().map_or(0, String::len),
        }
    }
}

impl GitWorkspaces {
    /// Local repository-family identity for isolation checks. No fetch, config expansion or index
    /// operation. Subdirectories deliberately resolve to the same canonical common directory.
    pub(crate) async fn repository_family(
        &self,
        folder: &Path,
    ) -> Result<Option<std::path::PathBuf>> {
        let git = self.git().await?;
        let folder = folder.canonicalize()?;
        if git.protected.iter().any(|path| {
            path.canonicalize()
                .is_ok_and(|path| folder.starts_with(path))
        }) {
            return Err(super::GitError::Message(
                "This agent configuration folder is not a project workspace.".into(),
            ));
        }
        let output = git
            .output(
                &folder,
                args(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                8192,
            )
            .await?;
        if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            // A failed probe in an existing repository is unknown, not proof of an unrelated
            // non-Git folder. Keep it out of an isolated session until it can be inspected.
            for ancestor in folder.ancestors() {
                match std::fs::symlink_metadata(ancestor.join(".git")) {
                    Ok(_) => {
                        return Err(super::GitError::Message(
                            "This repository's isolation identity could not be verified.".into(),
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        return Err(super::GitError::Message(
                            "This folder's repository metadata could not be inspected.".into(),
                        ));
                    }
                }
            }
            return Ok(None);
        }
        let path = std::path::PathBuf::from(text(&output.stdout)?);
        if !path.is_absolute() {
            return Err(super::GitError::Message(
                "Git did not return an absolute repository identity.".into(),
            ));
        }
        Ok(Some(path.canonicalize()?))
    }

    /// Only the repository root is portable: a subfolder has a different local meaning.
    /// Unavailable inspection leaves existing identity alone. A new unqualified folder stays local.
    pub(crate) async fn folder_identity(&self, folder: &Path) -> Result<FolderIdentity> {
        let git = self.git().await?;
        let folder = folder.canonicalize()?;
        if git.protected.iter().any(|path| {
            path.canonicalize()
                .is_ok_and(|path| folder.starts_with(path))
        }) {
            return Ok(FolderIdentity::Local);
        }
        let root = git
            .output(&folder, args(&["rev-parse", "--show-toplevel"]), 8192)
            .await?;
        if root.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            return Ok(FolderIdentity::Local);
        }
        if Path::new(&text(&root.stdout)?).canonicalize()? != folder {
            return Ok(FolderIdentity::Local);
        }
        // Read only the selected repository's origin value, without expanding includes or
        // insteadOf substitutions into URLs that may contain private credentials.
        let origin = git
            .output(
                &folder,
                args(&[
                    "config",
                    "--local",
                    "--no-includes",
                    "--get",
                    "remote.origin.url",
                ]),
                4096,
            )
            .await?;
        if origin.exit.as_ref().and_then(|exit| exit.code) == Some(1) {
            return Ok(FolderIdentity::Local);
        }
        if origin.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
            return Err(super::GitError::Message(
                "The repository identity could not be inspected.".into(),
            ));
        }
        let Some(remote) = portable_remote(&text(&origin.stdout)?) else {
            return Ok(FolderIdentity::Local);
        };
        // A feature branch is not a default branch. Keep an absent origin/HEAD unknown.
        let head = git
            .output(
                &folder,
                args(&["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]),
                2048,
            )
            .await?;
        let default_branch = if head.exit.as_ref().and_then(|exit| exit.code) == Some(0) {
            text(&head.stdout)?
                .strip_prefix("refs/remotes/origin/")
                .filter(|value| valid_branch(value))
                .map(str::to_owned)
        } else {
            None
        };
        Ok(FolderIdentity::Git {
            remote,
            default_branch,
        })
    }
}

/// A conservative portable URL subset. Raw remote values are never logged or persisted.
fn portable_remote(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || value.contains(['?', '#', '\\', '%'])
    {
        return None;
    }
    let (scheme, host, path) = if let Some(rest) = value.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        ("ssh", host, path)
    } else if let Some(rest) = value.strip_prefix("ssh://git@") {
        let (host, path) = rest.split_once('/')?;
        ("ssh", host, path)
    } else {
        let (scheme, rest) = value.split_once("://")?;
        if !matches!(scheme, "http" | "https") {
            return None;
        }
        let (host, path) = rest.split_once('/')?;
        (scheme, host, path)
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
    {
        return None;
    }
    let (name, port) = host
        .split_once(':')
        .map_or((host, None), |(name, port)| (name, Some(port)));
    if name.is_empty()
        || name.starts_with(['.', '-'])
        || name.ends_with(['.', '-'])
        || name.contains("..")
        || port.is_some_and(|port| port.parse::<u16>().ok().is_none_or(|port| port == 0))
    {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-~".contains(&b))
        })
    {
        return None;
    }
    let host = host.to_ascii_lowercase();
    if host == "github.com" {
        return (path.split('/').count() == 2).then(|| format!("https://github.com/{path}"));
    }
    Some(if scheme == "ssh" {
        format!("ssh://git@{host}/{path}")
    } else {
        format!("{scheme}://{host}/{path}")
    })
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value != "HEAD"
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_remote_normalizes_supported_forms_without_secrets_or_local_paths() {
        for value in [
            "https://GitHub.com/Owner/Repo.git",
            "git@github.com:Owner/Repo",
            "ssh://git@github.com/Owner/Repo.git/",
        ] {
            assert_eq!(
                portable_remote(value).as_deref(),
                Some("https://github.com/Owner/Repo")
            );
        }
        assert_eq!(
            portable_remote("ssh://git@forge.example:2222/team/repo.git").as_deref(),
            Some("ssh://git@forge.example:2222/team/repo")
        );
        for value in [
            "https://token@github.com/org/repo",
            "https://user:secret@github.com/org/repo",
            "https://github.com/org/repo?token=secret",
            "https://github.com/org/repo#secret",
            "ssh://user@host/repo",
            "file:///private/repo",
            "C:/private/repo",
            "/private/repo",
            "https://github.com/%2fsecret/repo",
            "ext::command",
            "git@host:../repo",
            "https://github.com/org/repo\n",
            "https://host:99999/repo",
        ] {
            assert!(
                portable_remote(value).is_none(),
                "unsafe remote was accepted"
            );
        }
    }

    #[tokio::test]
    async fn portable_identity_reads_only_origin_and_remote_head_without_touching_index() {
        let rig = super::super::tests::rig().await;
        assert_eq!(
            rig.service
                .folder_identity(&rig.work)
                .await
                .expect("no remote"),
            FolderIdentity::Local
        );
        rig.git
            .run(
                &rig.work,
                args(&[
                    "config",
                    "remote.origin.url",
                    "git@github.com:fixture/project.git",
                ]),
                8192,
            )
            .await
            .expect("origin");
        assert_eq!(
            rig.service
                .folder_identity(&rig.work)
                .await
                .expect("unknown default"),
            FolderIdentity::Git {
                remote: "https://github.com/fixture/project".into(),
                default_branch: None,
            }
        );
        rig.git
            .run(
                &rig.work,
                args(&[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/trunk",
                ]),
                8192,
            )
            .await
            .expect("remote head");
        let index = std::fs::read(rig.work.join(".git/index")).expect("index");
        assert_eq!(
            rig.service
                .folder_identity(&rig.work)
                .await
                .expect("identity"),
            FolderIdentity::Git {
                remote: "https://github.com/fixture/project".into(),
                default_branch: Some("trunk".into()),
            }
        );
        assert_eq!(
            std::fs::read(rig.work.join(".git/index")).expect("index unchanged"),
            index
        );
        let subfolder = rig.work.join("subfolder");
        std::fs::create_dir(&subfolder).expect("subfolder");
        assert_eq!(
            rig.service
                .folder_identity(&subfolder)
                .await
                .expect("subfolder identity"),
            FolderIdentity::Local
        );
        rig.git
            .run(
                &rig.work,
                args(&[
                    "config",
                    "remote.origin.url",
                    "https://private-token@github.com/fixture/project",
                ]),
                8192,
            )
            .await
            .expect("fixture secret remote");
        assert_eq!(
            rig.service
                .folder_identity(&rig.work)
                .await
                .expect("private identity"),
            FolderIdentity::Local
        );
    }
}
