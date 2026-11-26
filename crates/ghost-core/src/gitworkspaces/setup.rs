//! Setup recipes are inert until a separate, one-use review is applied.
use std::collections::BTreeSet;
use std::path::{Component, Path};

use ghost_protocol::worktree_setup::WorktreeSetupInput;

use super::{GitError, GitWorkspaces, Result};
use super::{inspect, setup_files};
use ghost_protocol::{
    git::ManagedWorktree,
    worktree_setup::{SetupCommand, SetupFile, WorktreeSetupPreview, WorktreeSetupResult},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone)]
pub(super) struct Prepared {
    record: ManagedWorktree,
    input: WorktreeSetupInput,
    files: BTreeMap<String, setup_files::Fingerprint>,
    executable: Option<(PathBuf, u64, Option<SystemTime>)>,
    fingerprint: String,
    expires: Instant,
    command_ready: bool,
    generation: Option<u32>,
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) async fn executable(
    service: &GitWorkspaces,
    command: &Option<SetupCommand>,
) -> Result<Option<(PathBuf, u64, Option<SystemTime>)>> {
    let Some(command) = command else {
        return Ok(None);
    };
    let env = service.env.get().await;
    let requested = Path::new(&command.program);
    let program = if requested.is_absolute() {
        requested.to_path_buf()
    } else if requested.components().count() == 1 {
        ghost_process::find_exe(&command.program,&env.vars).ok_or_else(||invalid("The setup executable was not found. Use an installed native executable and explicit arguments."))?
    } else {
        return Err(invalid(
            "Use an absolute executable path or a program name found on PATH.",
        ));
    };
    #[cfg(windows)]
    if program
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid(
            "Windows setup needs a native executable. For a script, explicitly choose its interpreter and pass the script as an argument.",
        ));
    }
    let program = program.canonicalize()?;
    let metadata = std::fs::metadata(&program)?;
    if !metadata.is_file() {
        return Err(invalid("The setup executable is not a regular file."));
    }
    Ok(Some((program, metadata.len(), metadata.modified().ok())))
}

impl GitWorkspaces {
    pub(crate) async fn setup_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
        input: WorktreeSetupInput,
    ) -> Result<WorktreeSetupPreview> {
        validate(&input)?;
        if input.files.is_empty() && input.command.is_none() {
            return Err(invalid("Save at least one setup file or command first."));
        }
        let record = self.owned_record(chat_id, worktree_id)?;
        let git = self.git().await?;
        let view = inspect::inspect(&git, Path::new(&record.path)).await?;
        let source = inspect::inspect(&git, Path::new(&record.source)).await?;
        if source.common != view.common
            || view.view.branch.as_deref() != Some(&record.branch)
            || !view.view.is_linked_worktree
            || !view.view.blockers.is_empty()
        {
            return Err(invalid(
                "Resolve worktree branch or Git operation changes before setup.",
            ));
        }
        let protected = git
            .protected
            .iter()
            .cloned()
            .chain(self.root.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let source = PathBuf::from(&record.source);
        let paths = input.files.clone();
        let destination = PathBuf::from(&record.path);
        let files = tokio::task::spawn_blocking(move || {
            let files = setup_files::collect(&source, &paths, &protected)?;
            setup_files::absent(&destination, files.keys().cloned())?;
            Ok::<_, GitError>(files)
        })
        .await
        .map_err(|_| invalid("Could not inspect setup files."))??;
        let executable = executable(self, &input.command).await?;
        let command = input
            .command
            .as_ref()
            .zip(executable.as_ref())
            .map(|(command, exe)| SetupCommand {
                program: exe.0.to_string_lossy().into(),
                args: command.args.clone(),
            });
        let token = crate::ids::new_id();
        let preview=WorktreeSetupPreview {token:token.clone(),worktree_id:record.id.clone(),source:record.source.clone(),destination:record.path.clone(),files:files.iter().map(|(path,(fingerprint,_))|SetupFile {path:path.clone(),bytes:fingerprint.bytes}).collect(),command,warnings:vec!["Only the listed files will be copied. Existing destination files are never overwritten.".into(),"Environment files may contain application secrets. Their contents are not shown, logged, or stored in setup settings.".into(),"The command runs only after a separate explicit Start action in the owned terminal. It can modify this worktree and access your normal command environment. Setup stops after ten minutes.".into()],expires_at:crate::time::now_millis()+600_000};
        let mut prepared = lock(&self.setups);
        // Reclaim this chat's previous review before admission. Keep its bound command until
        // apply replaces the generation, so expiry still identifies the exact pending attempt.
        let now = Instant::now();
        prepared.retain(|_, value| {
            (value.expires > now || value.generation.is_some())
                && (value.record.chat_id != chat_id
                    || (value.command_ready && value.generation.is_some()))
        });
        // Each chat has at most its bound command and one replacement review. Bound chat slots
        // rather than receipts so all 64 admitted chats retain room to retry after expiry.
        let chats = prepared
            .values()
            .map(|value| value.record.chat_id.as_str())
            .collect::<BTreeSet<_>>();
        if chats.len() >= 64 && !chats.contains(chat_id) {
            return Err(invalid(
                "Too many pending setup reviews. Finish or retry setup in another chat before reviewing more.",
            ));
        }
        prepared.insert(
            token,
            Prepared {
                record,
                input,
                files: files
                    .into_iter()
                    .map(|(path, (fingerprint, _))| (path, fingerprint))
                    .collect(),
                executable,
                fingerprint: view.fingerprint,
                expires: Instant::now() + Duration::from_secs(600),
                command_ready: false,
                generation: None,
            },
        );
        Ok(preview)
    }

    pub(crate) fn setup_folders(&self, chat_id: &str, token: &str) -> Result<Vec<PathBuf>> {
        let prepared = lock(&self.setups);
        let plan = prepared
            .get(token)
            .filter(|plan| plan.record.chat_id == chat_id && plan.expires > Instant::now())
            .ok_or_else(|| {
                invalid("Setup review expired or was already used. Review setup again.")
            })?;
        let mut folders = vec![PathBuf::from(&plan.record.path)];
        if !plan.command_ready && !plan.input.files.is_empty() {
            folders.push(PathBuf::from(&plan.record.source));
        }
        Ok(folders)
    }

    pub(crate) fn setup_generation(&self, chat_id: &str, token: &str) -> Result<Option<u32>> {
        lock(&self.setups)
            .get(token)
            .filter(|plan| plan.record.chat_id == chat_id && plan.command_ready)
            .map(|plan| plan.generation)
            .ok_or_else(|| invalid("Review setup again before starting this command."))
    }

    pub(crate) fn setup_review_generation(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<Option<u32>> {
        lock(&self.setups)
            .get(token)
            .filter(|plan| plan.record.chat_id == chat_id && !plan.command_ready)
            .map(|plan| plan.generation)
            .ok_or_else(|| invalid("Review setup again before preparing this command."))
    }

    pub(crate) fn bind_setup_review_generation(
        &self,
        chat_id: &str,
        token: &str,
        generation: u32,
    ) -> Result<()> {
        let mut plans = lock(&self.setups);
        plans.retain(|key, plan| {
            key == token || plan.record.chat_id != chat_id || plan.command_ready
        });
        let plan = plans
            .get_mut(token)
            .filter(|plan| plan.record.chat_id == chat_id && !plan.command_ready)
            .ok_or_else(|| invalid("The setup review is unavailable."))?;
        plan.generation = Some(generation);
        Ok(())
    }

    pub(crate) fn bind_setup_generation(
        &self,
        chat_id: &str,
        token: &str,
        generation: u32,
    ) -> Result<()> {
        let mut plans = lock(&self.setups);
        plans.retain(|key, plan| {
            key == token || plan.record.chat_id != chat_id || plan.generation.is_none()
        });
        let plan = plans
            .get_mut(token)
            .filter(|plan| plan.record.chat_id == chat_id && plan.command_ready)
            .ok_or_else(|| invalid("The setup command review is unavailable."))?;
        plan.generation = Some(generation);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn expire_setup_for_test(&self, token: &str) {
        lock(&self.setups).get_mut(token).unwrap().expires =
            Instant::now() - Duration::from_secs(1);
    }

    fn take_setup(&self, chat_id: &str, token: &str, command_ready: bool) -> Result<Prepared> {
        let mut plans = lock(&self.setups);
        if plans.get(token).is_none_or(|plan| {
            plan.record.chat_id != chat_id || plan.command_ready != command_ready
        }) {
            return Err(invalid("This setup review does not belong to this action."));
        }
        let plan = plans
            .remove(token)
            .ok_or_else(|| invalid("Review setup again."))?;
        if plan.expires <= Instant::now() {
            return Err(invalid("Setup review expired. Review it again."));
        }
        Ok(plan)
    }

    async fn check_setup(&self, plan: &Prepared) -> Result<()> {
        if self.owned_record(&plan.record.chat_id, &plan.record.id)? != plan.record {
            return Err(invalid("Worktree ownership changed after setup review."));
        }
        let view = inspect::inspect(&self.git().await?, Path::new(&plan.record.path)).await?;
        let source = inspect::inspect(&self.git().await?, Path::new(&plan.record.source)).await?;
        if source.common != view.common
            || view.fingerprint != plan.fingerprint
            || executable(self, &plan.input.command).await? != plan.executable
        {
            return Err(invalid(
                "The worktree or setup executable changed. Review setup again.",
            ));
        }
        if plan.command_ready {
            let destination = PathBuf::from(&plan.record.path);
            let paths = plan.input.files.clone();
            let expected = plan.files.clone();
            tokio::task::spawn_blocking(move || {
                let files = setup_files::collect(&destination, &paths, &[])?;
                if files
                    .into_iter()
                    .map(|(path, (fingerprint, _))| (path, fingerprint))
                    .collect::<BTreeMap<_, _>>()
                    != expected
                {
                    return Err(invalid(
                        "Copied setup files changed before command startup. Review setup again.",
                    ));
                }
                Ok(())
            })
            .await
            .map_err(|_| invalid("Could not verify copied setup files."))??;
        }
        Ok(())
    }

    pub(crate) async fn setup_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<WorktreeSetupResult> {
        let mut plan = self.take_setup(chat_id, token, false)?;
        let git = self.git().await?;
        let _repository = self
            .hold_repository(&git, Path::new(&plan.record.source))
            .await;
        self.check_setup(&plan).await?;
        let protected = git
            .protected
            .iter()
            .cloned()
            .chain(self.root.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let source = PathBuf::from(&plan.record.source);
        let destination = PathBuf::from(&plan.record.path);
        let paths = plan.input.files.clone();
        let expected = plan.files.clone();
        let (copied, mut warning) = tokio::task::spawn_blocking(move || {
            let files = setup_files::collect(&source, &paths, &protected)?;
            if files
                .iter()
                .map(|(path, (fingerprint, _))| (path.clone(), fingerprint.clone()))
                .collect::<BTreeMap<_, _>>()
                != expected
            {
                return Err(invalid(
                    "Setup source files changed after review. No setup file was copied.",
                ));
            }
            Ok(setup_files::publish(&destination, &files))
        })
        .await
        .map_err(|_| {
            invalid("The setup copy task ended. Inspect the worktree before retrying.")
        })??;
        let mut command_token = None;
        if warning.is_none() && plan.input.command.is_some() {
            match inspect::inspect(&git, Path::new(&plan.record.path)).await {
                Ok(view) => {
                    plan.fingerprint = view.fingerprint;
                    plan.command_ready = true;
                    plan.expires = Instant::now() + Duration::from_secs(600);
                    let token = crate::ids::new_id();
                    lock(&self.setups).insert(token.clone(), plan);
                    command_token = Some(token);
                }
                Err(error) => {
                    warning = Some(format!(
                        "Files were copied, but the worktree could not be verified afterward: {error}. No command was started."
                    ))
                }
            }
        }
        Ok(WorktreeSetupResult {
            copied,
            command_token,
            warning,
        })
    }

    pub(crate) async fn setup_command(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<ghost_process::SpawnSpec> {
        let plan = self.take_setup(chat_id, token, true)?;
        self.check_setup(&plan).await?;
        let command = plan
            .input
            .command
            .ok_or_else(|| invalid("No setup command was reviewed."))?;
        let program = plan
            .executable
            .ok_or_else(|| invalid("No setup executable was reviewed."))?
            .0;
        let mut env = (*self.env.get().await).clone();
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        let folder = PathBuf::from(&plan.record.path);
        // The worktree's first port, so setup can write it where the project reads one. Setup
        // still runs if no block is free; it then has none.
        let set_env = self
            .port_under_gate(chat_id)
            .map(|(_, port)| super::run::port_env(port, false))
            .unwrap_or_default();
        Ok(ghost_process::SpawnSpec {
            launch: ghost_process::LaunchSpec {
                label: "Worktree setup".into(),
                program,
                args: command.args.into_iter().map(Into::into).collect(),
                cwd: ghost_process::ProcessCwd::Fixed(folder.clone()),
                set_env,
                daemons: vec![],
                startup_timeout: Duration::from_secs(15),
            },
            folder,
            env: Arc::new(env),
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        })
    }
}

pub(crate) fn validate(input: &WorktreeSetupInput) -> Result<()> {
    if input.files.len() > 32 {
        return Err(invalid("Choose at most 32 explicit setup files."));
    }
    let mut seen = BTreeSet::new();
    for path in &input.files {
        validate_path(path)?;
        if !seen.insert(path.to_lowercase()) {
            return Err(invalid(
                "Setup filenames must be distinct, including letter case.",
            ));
        }
    }
    if let Some(command) = &input.command
        && (command.program.is_empty()
            || command.program.len() > 4096
            || command.program.chars().any(char::is_control)
            || command.args.len() > 128
            || command
                .args
                .iter()
                .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            || command.args.iter().map(String::len).sum::<usize>() > 32768)
    {
        return Err(invalid(
            "Enter a program and bounded arguments. Do not put credentials in a saved command.",
        ));
    }
    Ok(())
}

pub(super) fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

pub(super) fn validate_path(path: &str) -> Result<()> {
    if path.len() > 1024
        || path.is_empty()
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.split('/').any(|part| {
            part.is_empty() || part.ends_with(['.', ' ']) || {
                let base = part
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                matches!(base.as_str(), "con" | "prn" | "aux" | "nul")
                    || (base.len() == 4
                        && (base.starts_with("com") || base.starts_with("lpt"))
                        && base.as_bytes()[3].is_ascii_digit())
            }
        })
    {
        return Err(invalid("Setup files must be ordinary relative file paths."));
    }
    let mut checked = path.to_owned();
    let name = path.rsplit('/').next().unwrap_or_default();
    if name.eq_ignore_ascii_case(".env") || name.to_ascii_lowercase().starts_with(".env.") {
        checked.truncate(path.len() - name.len());
        checked.push_str("explicit-environment-file");
    }
    if path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            ".claude.json" | ".mcp.json" | ".auracode-library"
        ) || part.to_ascii_lowercase().starts_with(".auracode-setup-")
    }) {
        return Err(invalid(
            "Agent configuration and private app metadata cannot be setup copies.",
        ));
    }
    crate::checkpoints::scan::validate(&checked).map_err(|_| {
        invalid(
            "Agent credentials, configuration, links and private key files cannot be setup copies.",
        )
    })
}

impl GitWorkspaces {
    pub(crate) async fn setup_source(
        &self,
        chat_id: &str,
        folder: &Path,
    ) -> Result<std::path::PathBuf> {
        let folder = folder.canonicalize()?;
        for record in self.list(chat_id).await? {
            if Path::new(&record.path).canonicalize().ok().as_ref() == Some(&folder) {
                return Ok(Path::new(&record.source).canonicalize()?);
            }
        }
        Ok(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn full_setup_cache_allows_expired_chat_to_review_and_replace_its_generation() {
        let rig = super::super::tests::rig().await;
        let record = super::super::tests::isolated(&rig).await;
        let input = WorktreeSetupInput {
            run: None,
            files: vec![],
            command: Some(SetupCommand {
                program: std::env::current_exe().unwrap().to_string_lossy().into(),
                args: vec!["--not-executed".into()],
            }),
        };
        let preview = rig
            .service
            .setup_preview("chat", &record.id, input.clone())
            .await
            .unwrap();
        let token = rig
            .service
            .setup_apply("chat", &preview.token)
            .await
            .unwrap()
            .command_token
            .unwrap();
        rig.service
            .bind_setup_generation("chat", &token, 1)
            .unwrap();
        {
            let mut plans = lock(&rig.service.setups);
            let mut expired = plans.get(&token).unwrap().clone();
            expired.expires = Instant::now() - Duration::from_secs(1);
            plans.insert(token.clone(), expired.clone());
            for index in 1..64 {
                let mut other = expired.clone();
                other.record.chat_id = format!("other-{index}");
                plans.insert(format!("other-token-{index}"), other);
            }
        }
        assert!(rig.service.setup_folders("chat", &token).is_err());
        let expired_review = rig
            .service
            .setup_preview("chat", &record.id, input.clone())
            .await
            .expect("a full cache must admit an existing chat's retry");
        rig.service
            .bind_setup_review_generation("chat", &expired_review.token, 1)
            .unwrap();
        rig.service.expire_setup_for_test(&expired_review.token);
        assert!(
            rig.service
                .setup_folders("chat", &expired_review.token)
                .is_err()
        );
        let retry = rig
            .service
            .setup_preview("chat", &record.id, input)
            .await
            .expect("an expired review must not consume the chat's retry capacity");
        rig.service
            .bind_setup_review_generation("chat", &retry.token, 1)
            .unwrap();
        assert!(
            rig.service
                .setup_review_generation("chat", &expired_review.token)
                .is_err()
        );
        assert_eq!(
            rig.service.setup_generation("chat", &token).unwrap(),
            Some(1)
        );
        assert_eq!(
            rig.service
                .setup_generation("other-1", "other-token-1")
                .unwrap(),
            Some(1),
            "another chat's expired receipt still identifies its failure owner"
        );
        let fresh = rig
            .service
            .setup_apply("chat", &retry.token)
            .await
            .unwrap()
            .command_token
            .unwrap();
        rig.service
            .bind_setup_generation("chat", &fresh, 2)
            .unwrap();
        assert!(rig.service.setup_generation("chat", &token).is_err());
        assert_eq!(
            rig.service.setup_generation("chat", &fresh).unwrap(),
            Some(2)
        );
        assert!(rig.service.setup_command("chat", &fresh).await.is_ok());
    }

    #[tokio::test]
    async fn setup_review_rejects_changed_contents_and_runs_no_implicit_command() {
        let rig = super::super::tests::rig().await;
        let record = super::super::tests::isolated(&rig).await;
        std::fs::write(rig.work.join(".env"), "FIXTURE=one").unwrap();
        let input = WorktreeSetupInput {
            run: None,
            files: vec![".env".into()],
            command: Some(SetupCommand {
                program: std::env::current_exe().unwrap().to_string_lossy().into(),
                args: vec!["--not-executed".into()],
            }),
        };
        let mut forged = record.clone();
        forged.path = rig.work.to_string_lossy().into();
        rig.service.write_records(vec![forged]).unwrap();
        assert!(
            rig.service
                .setup_preview("chat", &record.id, input.clone())
                .await
                .is_err()
        );
        rig.service.write_records(vec![record.clone()]).unwrap();
        let preview = rig
            .service
            .setup_preview("chat", &record.id, input.clone())
            .await
            .unwrap();
        assert_eq!(preview.files[0].path, ".env");
        std::fs::write(rig.work.join(".env"), "FIXTURE=two").unwrap();
        assert!(
            rig.service
                .setup_apply("chat", &preview.token)
                .await
                .is_err()
        );
        assert!(!Path::new(&record.path).join(".env").exists());
        let preview = rig
            .service
            .setup_preview("chat", &record.id, input.clone())
            .await
            .unwrap();
        let result = rig
            .service
            .setup_apply("chat", &preview.token)
            .await
            .unwrap();
        assert_eq!(result.copied, vec![".env"]);
        assert!(result.warning.is_none());
        assert_eq!(
            std::fs::read(Path::new(&record.path).join(".env")).unwrap(),
            b"FIXTURE=two"
        );
        assert!(
            rig.service
                .setup_apply("chat", &preview.token)
                .await
                .is_err()
        );
        let token = result.command_token.unwrap();
        std::fs::write(
            Path::new(&record.path).join(".env"),
            "FIXTURE=changed-after-copy",
        )
        .unwrap();
        assert!(rig.service.setup_command("chat", &token).await.is_err());
        let command_review = rig
            .service
            .setup_preview(
                "chat",
                &record.id,
                WorktreeSetupInput {
                    run: None,
                    files: vec![],
                    command: input.command,
                },
            )
            .await
            .unwrap();
        let token = rig
            .service
            .setup_apply("chat", &command_review.token)
            .await
            .unwrap()
            .command_token
            .unwrap();
        let launch = rig.service.setup_command("chat", &token).await.unwrap();
        assert_eq!(launch.folder, PathBuf::from(&record.path));
        assert_eq!(
            launch.launch.args,
            vec![std::ffi::OsString::from("--not-executed")]
        );
        assert!(rig.service.setup_command("chat", &token).await.is_err());
    }
    #[test]
    fn explicit_environment_files_do_not_relax_provider_or_path_guards() {
        for path in [".env", "nested/.env.local", "config/development.json"] {
            validate_path(path).unwrap();
        }
        for path in [
            "../.env",
            ".env/child",
            ".codex/auth.json",
            "auth.json",
            ".claude.json",
            ".mcp.json",
            "id_rsa",
            "secret.pem",
            "file:stream",
            "aux.txt",
            "nested/name.",
        ] {
            assert!(validate_path(path).is_err(), "{path}");
        }
        let input = WorktreeSetupInput {
            run: None,
            files: vec![".env".into(), ".ENV".into()],
            command: None,
        };
        assert!(validate(&input).is_err());
    }
}
