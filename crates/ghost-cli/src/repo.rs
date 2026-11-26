//! A run's folders: the run folder in the system's temporary folder, and the chat folder in it,
//! a fresh git repository (outside any project and outside `~/Desktop`, spec 10.1).
//!
//! The probe's repository is hostile on purpose (spec 10.2 step 1): an `.npmrc` that points the
//! adapters' npm scope and the registry at a closed local port, and a committed `node_modules`
//! holding a fake copy of every catalog adapter at its catalog version, whose bins print
//! [`HOSTILE_MARK`]. `npx` started in the folder would run those, so the probe checks that the mark
//! never shows up: Ghost installs and launches adapters from their own folder only. Smoke runs
//! get a plain repository with a README.
//!
//! git runs through `run_helper` (bounded, registered for the sweep) with no system or global
//! configuration, so no hook, template or signing setting of the user's applies, and with a fixed
//! author.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ghost_agents::{Recipe, catalog};
use ghost_process::{
    HelperEnv, HelperSpec, LeftoverRegistry, find_program, process_env, run_helper,
};
use serde_json::json;

use crate::CliError;

/// What the fake adapters' bins print.
pub const HOSTILE_MARK: &str = "HOSTILE";

/// What the probe's `.npmrc` points the registry at: a port nothing listens on.
pub const HOSTILE_REGISTRY: &str = "http://127.0.0.1:9/";

/// How long one git command may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The run folder: `<temp>/ghost-cli-<label>-<random>`, deleted on drop unless kept.
#[derive(Debug)]
pub struct RunFolder {
    dir: Option<tempfile::TempDir>,
    path: PathBuf,
}

impl RunFolder {
    /// A new run folder in the system's temporary folder, by its canonical path on Unix (macOS
    /// reaches `/var/folders` through a link, and agents report the real path).
    pub fn create(label: &str) -> Result<RunFolder, CliError> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("ghost-cli-{label}-"))
            .tempdir()
            .map_err(|error| CliError::io("create a temporary run folder", error))?;
        let path = if cfg!(unix) {
            std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf())
        } else {
            dir.path().to_path_buf()
        };
        Ok(RunFolder {
            dir: Some(dir),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keeps the folder after the run: its protocol logs and chat folder are what a probe or a
    /// smoke run leaves for the fixtures.
    pub fn keep(mut self) -> PathBuf {
        if let Some(dir) = self.dir.take() {
            let _ = dir.keep();
        }
        self.path.clone()
    }
}

/// What the chat folder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoKind {
    /// A README.
    Plain,
    /// The npm files of a hostile clone (see the module docs).
    Hostile,
}

/// Creates `dir` as a git repository with its files committed, and returns it.
pub async fn create(
    dir: &Path,
    kind: RepoKind,
    registry: &LeftoverRegistry,
) -> Result<PathBuf, CliError> {
    let env = process_env();
    let git = find_program("git", &env).ok_or_else(|| {
        CliError::Message(
            "git was not found on PATH: ghost-cli needs it to make the chat folder a repository"
                .into(),
        )
    })?;
    std::fs::create_dir_all(dir)
        .map_err(|error| CliError::io(format!("create {}", dir.display()), error))?;
    write_files(dir, kind)?;
    // An empty global configuration, so nothing of the user's applies.
    let config = dir.with_extension("gitconfig");
    std::fs::write(&config, "")
        .map_err(|error| CliError::io(format!("write {}", config.display()), error))?;
    let message = match kind {
        RepoKind::Plain => "The chat folder of a ghost-cli run",
        RepoKind::Hostile => "A hostile clone: npm files that must never be used",
    };
    for args in [
        &["init", "--quiet"][..],
        &["add", "--all"],
        &["commit", "--quiet", "--no-verify", "--message", message],
    ] {
        git_run(registry, &git, dir, &config, args).await?;
    }
    Ok(dir.to_path_buf())
}

fn write_files(dir: &Path, kind: RepoKind) -> Result<(), CliError> {
    let write = |relative: &str, text: &str| -> Result<(), CliError> {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CliError::io(format!("create {}", parent.display()), error))?;
        }
        std::fs::write(&path, text)
            .map_err(|error| CliError::io(format!("write {}", path.display()), error))
    };
    match kind {
        RepoKind::Plain => write(
            "README.md",
            "# Scratch repository\n\naura-cli created this folder for one run. The agent may change anything here.\n",
        ),
        RepoKind::Hostile => {
            write(
                "README.md",
                "# Hostile clone\n\naura-cli's probe created this folder. Its npm files point at a closed port and its node_modules holds fake adapters that print HOSTILE: Ghost must never use them.\n",
            )?;
            let adapters = npm_adapters();
            write(
                ".npmrc",
                &format!(
                    "registry={HOSTILE_REGISTRY}\n{}",
                    adapters
                        .iter()
                        .filter_map(|(package, _, _)| package.split_once('/'))
                        .map(|(scope, _)| format!("{scope}:registry={HOSTILE_REGISTRY}\n"))
                        .collect::<Vec<_>>()
                        .join("")
                ),
            )?;
            let dependencies: serde_json::Map<String, serde_json::Value> = adapters
                .iter()
                .map(|(package, version, _)| ((*package).to_owned(), json!(version)))
                .collect();
            write(
                "package.json",
                &format!(
                    "{:#}\n",
                    json!({"name": "hostile-clone", "private": true, "dependencies": dependencies})
                ),
            )?;
            let script = format!("#!/bin/sh\necho {HOSTILE_MARK}\n");
            for (package, version, bin) in adapters {
                let folder = format!("node_modules/{package}");
                write(
                    &format!("{folder}/package.json"),
                    &format!(
                        "{:#}\n",
                        json!({"name": package, "version": version, "bin": {bin: "bin.js"}})
                    ),
                )?;
                write(
                    &format!("{folder}/bin.js"),
                    &format!("#!/usr/bin/env node\nconsole.log(\"{HOSTILE_MARK}\");\n"),
                )?;
                write(&format!("node_modules/.bin/{bin}"), &script)?;
                make_executable(&dir.join(format!("{folder}/bin.js")))?;
                make_executable(&dir.join(format!("node_modules/.bin/{bin}")))?;
            }
            Ok(())
        }
    }
}

/// Every catalog adapter: package, version, bin.
fn npm_adapters() -> Vec<(&'static str, &'static str, &'static str)> {
    catalog()
        .iter()
        .filter_map(|spec| match &spec.recipe {
            Recipe::NpmAdapter {
                package,
                version,
                bin,
                ..
            } => Some((*package, *version, *bin)),
            Recipe::Binary { .. }
            | Recipe::QualifiedBinary { .. }
            | Recipe::NativeBinary { .. }
            | Recipe::Builtin => None,
        })
        .collect()
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| CliError::io(format!("make {} executable", path.display()), error))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), CliError> {
    Ok(())
}

async fn git_run(
    registry: &LeftoverRegistry,
    git: &Path,
    dir: &Path,
    config: &Path,
    args: &[&str],
) -> Result<(), CliError> {
    let mut argv: Vec<OsString> = [
        "-c",
        "user.name=Ghost",
        "-c",
        "user.email=ghost@localhost",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "init.defaultBranch=main",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    argv.extend(args.iter().map(OsString::from));
    let output = run_helper(
        registry,
        HelperSpec {
            program: git.to_path_buf(),
            args: argv,
            cwd: dir.to_path_buf(),
            env: HelperEnv::ProcessEnv,
            set: vec![
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                ("GIT_CONFIG_GLOBAL".into(), config.as_os_str().to_owned()),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ],
            new_session: false,
            timeout: GIT_TIMEOUT,
            max_stdout: 64 * 1024,
        },
    )
    .await
    .map_err(|error| CliError::Message(format!("git {} failed: {error}", args.join(" "))))?;
    let succeeded = output.exit.is_some_and(|exit| exit.code == Some(0));
    if succeeded {
        return Ok(());
    }
    let why = if output.timed_out {
        format!("it did not finish within {} s", GIT_TIMEOUT.as_secs())
    } else {
        output.stderr_tail.trim().to_owned()
    };
    Err(CliError::Message(format!(
        "git {} failed in {}: {why}",
        args.first().copied().unwrap_or_default(),
        dir.display()
    )))
}
