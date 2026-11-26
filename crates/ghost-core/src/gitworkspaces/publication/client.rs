//! gh owns its credentials. Only bounded public metadata is decoded; raw output is never logged.
use super::*;
use futures::future::BoxFuture;
use ghost_process::{EnvKey, HelperEnv, HelperSpec, ResolvedEnv, find_exe, run_helper};
use std::{ffi::OsString, sync::Arc, time::Duration};

pub(super) struct Response {
    pub status: u16,
    pub body: serde_json::Value,
}
pub(super) trait Remote: Send + Sync {
    fn request<'a>(
        &'a self,
        folder: &'a Path,
        method: &'a str,
        endpoint: &'a str,
        fields: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Response>>;
    fn push<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        url: &'a str,
        branch: &'a str,
        head: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
}
pub(super) struct Client {
    program: PathBuf,
    env: Arc<ResolvedEnv>,
    registry: Arc<ghost_process::LeftoverRegistry>,
}
impl Client {
    pub async fn new(service: &GitWorkspaces) -> Result<Self> {
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
            "PAGER",
        ] {
            env.vars.remove(&EnvKey::from(name));
        }
        let program = find_exe("gh", &env.vars).ok_or_else(|| {
            invalid(
                "Install GitHub CLI and sign in with gh auth login before reviewing publication.",
            )
        })?;
        Ok(Self {
            program,
            env: Arc::new(env),
            registry: service.registry.clone(),
        })
    }
}
impl Remote for Client {
    fn request<'a>(
        &'a self,
        folder: &'a Path,
        method: &'a str,
        endpoint: &'a str,
        fields: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Response>> {
        Box::pin(async move {
            let mut argv = args(&[
                "api",
                "--hostname",
                "github.com",
                "--include",
                "--method",
                method,
                endpoint,
            ]);
            for (key, value) in fields {
                argv.push(OsString::from("--raw-field"));
                argv.push(format!("{key}={value}").into());
            }
            // Boolean fields are typed and fixed by Rust, never supplied by IPC.
            if method == "POST" {
                argv.extend(args(&[
                    "--field",
                    "private=true",
                    "--field",
                    "auto_init=false",
                ]));
            }
            let output=run_helper(&self.registry,HelperSpec{program:self.program.clone(),args:argv,cwd:folder.into(),env:HelperEnv::Resolved(self.env.clone()),set:vec![("GH_PROMPT_DISABLED".into(),"1".into()),("NO_COLOR".into(),"1".into()),("GH_HOST".into(),"github.com".into())],new_session:true,timeout:Duration::from_secs(45),max_stdout:128*1024}).await.map_err(|_|invalid("GitHub CLI could not finish. Review the saved publication before trying again."))?;
            if output.timed_out || output.stdout_truncated || output.exit.is_none() {
                return Err(invalid(
                    "GitHub did not confirm the request. Review the saved publication before trying again.",
                ));
            }
            parse(&output.stdout)
        })
    }
    fn push<'a>(
        &'a self,
        git: &'a Git,
        folder: &'a Path,
        url: &'a str,
        branch: &'a str,
        head: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Check Git's effective URL without contacting the remote. An insteadOf rule cannot
            // redirect the reviewed publication to an unreviewed destination.
            let effective = text(
                &git.run(folder, args(&["ls-remote", "--get-url", url]), 8192)
                    .await?,
            )?;
            if effective != url {
                return Err(invalid(
                    "Git rewrites this destination. Review that configuration before publishing.",
                ));
            }
            git.run(
                folder,
                args(&[
                    "push",
                    "--porcelain",
                    "--no-follow-tags",
                    "--no-verify",
                    &format!("--force-with-lease=refs/heads/{branch}:"),
                    url,
                    &format!("{head}:refs/heads/{branch}"),
                ]),
                32768,
            )
            .await?;
            Ok(())
        })
    }
}
fn parse(bytes: &[u8]) -> Result<Response> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("GitHub returned unsupported metadata."))?;
    let (headers, body) = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .ok_or_else(|| {
            invalid("GitHub did not confirm an HTTP response. Check gh auth login and access.")
        })?;
    let mut status = headers
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    if !matches!(
        status.next(),
        Some("HTTP/1.1" | "HTTP/2.0" | "HTTP/2" | "HTTP/3.0")
    ) {
        return Err(invalid("GitHub returned unsupported response headers."));
    }
    let status = status
        .next()
        .and_then(|value| value.parse().ok())
        .filter(|value: &u16| (100..=599).contains(value))
        .ok_or_else(|| invalid("GitHub returned an invalid status."))?;
    let body = serde_json::from_str(body)
        .map_err(|_| invalid("GitHub returned unsupported JSON metadata."))?;
    Ok(Response { status, body })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_does_not_confuse_missing_with_authentication_or_permission_errors() {
        for status in [200, 201, 401, 403, 404, 422, 500] {
            let raw = format!(
                "HTTP/2.0 {status} fixture\nContent-Type: application/json\r\n\r\n{{\"message\":\"fixture\"}}"
            );
            assert_eq!(parse(raw.as_bytes()).unwrap().status, status);
        }
        assert!(parse(b"gh: failed").is_err());
        assert!(parse(b"HTTP/2.0 200 OK\r\n\r\nnot json").is_err());
    }
    #[tokio::test]
    async fn production_push_uses_an_absent_branch_lease_against_an_owned_bare_repository() {
        let rig = crate::gitworkspaces::tests::rig().await;
        let bare = rig.work.parent().unwrap().join("owned-remote.git");
        std::fs::create_dir(&bare).unwrap();
        rig.git
            .run(&bare, args(&["init", "--bare"]), 8192)
            .await
            .unwrap();
        let path = bare.to_string_lossy().replace('\\', "/");
        let url = if cfg!(windows) {
            format!("file:///{path}")
        } else {
            format!("file://{path}")
        };
        let client = Client {
            program: PathBuf::from("unused-gh"),
            env: Arc::new(ResolvedEnv {
                vars: Default::default(),
                source: ghost_process::EnvSource::Fallback {
                    reason: "fixture".into(),
                },
                warnings: vec![],
                resolved_at_ms: 0,
            }),
            registry: ghost_process::LeftoverRegistry::disabled(),
        };
        let first = text(
            &rig.git
                .run(&rig.work, args(&["rev-parse", "HEAD"]), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        let index = std::fs::read(rig.work.join(".git/index")).unwrap();
        client
            .push(&rig.git, &rig.work, &url, "main", &first)
            .await
            .unwrap();
        assert_eq!(std::fs::read(rig.work.join(".git/index")).unwrap(), index);
        crate::gitworkspaces::tests::commit_file(&rig, &rig.work, "next commit\n").await;
        let next = text(
            &rig.git
                .run(&rig.work, args(&["rev-parse", "HEAD"]), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_ne!(first, next);
        assert!(
            client
                .push(&rig.git, &rig.work, &url, "main", &next)
                .await
                .is_err()
        );
        assert_eq!(
            text(
                &rig.git
                    .run(&bare, args(&["rev-parse", "refs/heads/main"]), 8192)
                    .await
                    .unwrap()
            )
            .unwrap(),
            first
        );
    }
}
