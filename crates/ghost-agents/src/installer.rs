//! Explicit fresh installation recipes. This module only inspects executable/package metadata.
//! It never opens npm configuration, credentials, agent state or a shell shim's contents.
use ghost_process::{EnvMap, find_exe, find_program};
use ghost_protocol::setup_actions::AgentInstallReview;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("This agent has no reviewed automatic installer. Use its official installation guide.")]
    Unsupported,
    #[error(
        "Install Node.js 22.19 or newer with npm, then refresh agents. No setup command has run."
    )]
    MissingNpm,
    #[error("The installer or its folder changed. Review the installation again.")]
    Changed,
    #[error("The home folder is unavailable. No setup command has run.")]
    Home,
}

pub struct InstallPlan {
    pub review: AgentInstallReview,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub folder: PathBuf,
}

// Versions are the integration's pinned acceptance versions, not an unreviewed latest release.
fn package(agent: &str) -> Result<(&'static str, &'static str), InstallError> {
    match agent {
        "claude" => Ok(("@anthropic-ai/claude-code", "2.1.310")),
        "codex" => Ok(("@openai/codex", "0.162.1")),
        "pi" => Ok(("@earendil-works/pi-coding-agent", "1.0.2")),
        // The official package (published by OpenCode's author). Its postinstall links the
        // platform binary, so scripts stay on like Claude's and Codex's.
        "opencode" => Ok(("opencode-ai", "1.19.4")),
        // xAI documents `npm install -g @xai-official/grok` for Grok Build; the package is
        // published and signed by xAI's own npm account. 1.0.70 is its `latest` tag and the
        // catalog's minimum. Scripts stay on as for the other vendor packages.
        "grok" => Ok(("@xai-official/grok", "1.0.70")),
        // Guides only, each for a stated reason (docs/setup-actions.md): OpenCode 2 ships as
        // @opencode/cli, whose `opencode` command would collide with OpenCode 1's, and its
        // Windows install is a download; oh-my-pi runs on Bun, not Node; Cursor, Hermes and
        // Antigravity publish install scripts or apps that fetch the latest build unpinned.
        _ => Err(InstallError::Unsupported),
    }
}

pub fn prepare(agent: &str, env: &EnvMap) -> Result<InstallPlan, InstallError> {
    if !cfg!(any(windows, target_os = "macos")) {
        return Err(InstallError::Unsupported);
    }
    let node = find_exe("node", env).ok_or(InstallError::MissingNpm)?;
    let npm = find_program("npm", env).ok_or(InstallError::MissingNpm)?;
    let folder = env
        .get(&if cfg!(windows) { "USERPROFILE" } else { "HOME" }.into())
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
        .ok_or(InstallError::Home)?;
    plan(agent, &node, &npm, &folder)
}

fn plan(agent: &str, node: &Path, npm: &Path, folder: &Path) -> Result<InstallPlan, InstallError> {
    let (package, version) = package(agent)?;
    let program = std::fs::canonicalize(node).map_err(|_| InstallError::MissingNpm)?;
    if !program.is_file() {
        return Err(InstallError::MissingNpm);
    }
    let npm = npm_entry(npm).ok_or(InstallError::MissingNpm)?;
    let folder = std::fs::canonicalize(folder).map_err(|_| InstallError::Home)?;
    if !folder.is_dir() {
        return Err(InstallError::Home);
    }
    let mut arguments: Vec<OsString> =
        vec![npm.clone().into(), "install".into(), "--global".into()];
    if agent == "pi" {
        arguments.push("--ignore-scripts".into());
    }
    arguments.push(format!("{package}@{version}").into());
    let program_text = program.to_str().ok_or(InstallError::Changed)?.to_owned();
    let folder_text = folder.to_str().ok_or(InstallError::Changed)?.to_owned();
    let argument_text: Vec<String> = arguments
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned).ok_or(InstallError::Changed))
        .collect::<Result<_, _>>()?;
    let mut hash = Sha256::new();
    for value in std::iter::once(program_text.as_str())
        .chain(std::iter::once(folder_text.as_str()))
        .chain(argument_text.iter().map(String::as_str))
    {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    // Replacing either the Node executable or the npm entry invalidates the review. These are
    // executable bytes only, not configuration. Bound inspection to ordinary installer sizes.
    for path in [&program, &npm] {
        hash_file(&mut hash, path)?;
    }
    let review = AgentInstallReview {
        agent_id: agent.into(), package: package.into(), version: version.into(),
        program: program_text, arguments: argument_text, working_directory: folder_text,
        revision: format!("{:x}", hash.finalize()), timeout_seconds: 600,
        notices: vec![
            "Installs the integration's last verified version into npm's global prefix. npm uses its existing settings; Ghost does not read them.".into(),
            "Requires Node.js 22.19 or newer. Permission or network failures remain visible in the terminal. No administrator elevation is requested.".into(),
            "Closing the terminal or reaching ten minutes stops this owned installation. A partial package may require another installation attempt.".into(),
            "Sign-in is a separate action. A successful command still needs a fresh agent version check.".into(),
        ],
    };
    Ok(InstallPlan {
        review,
        program,
        arguments,
        folder,
    })
}

pub(crate) fn hash_file(hash: &mut Sha256, path: &Path) -> Result<(), InstallError> {
    let mut file = std::fs::File::open(path).map_err(|_| InstallError::Changed)?;
    let length = file.metadata().map_err(|_| InstallError::Changed)?.len();
    if length > 256 * 1024 * 1024 {
        return Err(InstallError::Changed);
    }
    hash.update(length.to_le_bytes());
    let mut buffer = [0; 64 * 1024];
    let mut seen = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|_| InstallError::Changed)?;
        if read == 0 {
            break;
        }
        seen += read as u64;
        if seen > length {
            return Err(InstallError::Changed);
        }
        hash.update(&buffer[..read]);
    }
    if seen != length {
        return Err(InstallError::Changed);
    }
    Ok(())
}

/// Resolve the documented npm package entry without ever evaluating npm.cmd or npm.ps1.
fn npm_entry(npm: &Path) -> Option<PathBuf> {
    let entry = if crate::npm_entry::is_shim(npm) {
        if !npm.file_stem()?.eq_ignore_ascii_case("npm") || !npm.is_file() {
            return None;
        }
        std::fs::canonicalize(npm.parent()?.join("node_modules/npm/bin/npm-cli.js")).ok()?
    } else {
        std::fs::canonicalize(npm).ok()?
    };
    if entry.file_name()? != "npm-cli.js" || entry.parent()?.file_name()? != "bin" {
        return None;
    }
    let root = entry.parent()?.parent()?;
    let file = std::fs::File::open(root.join("package.json")).ok()?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    (manifest.get("name")?.as_str()? == "npm" && entry.is_file()).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviewed_argv_is_pinned_shim_free_and_replacement_invalidates_review() {
        let temp = tempfile::tempdir().unwrap();
        let node = temp.path().join("node.exe");
        let shim = temp.path().join("npm.cmd");
        let npm = temp.path().join("node_modules/npm/bin/npm-cli.js");
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::write(&node, "synthetic executable").unwrap();
        std::fs::write(&shim, "do not execute or interpret").unwrap();
        std::fs::write(&npm, "synthetic npm entry").unwrap();
        std::fs::write(
            npm.parent().unwrap().parent().unwrap().join("package.json"),
            r#"{"name":"npm"}"#,
        )
        .unwrap();
        let first = plan("pi", &node, &shim, temp.path()).unwrap();
        assert_eq!(
            &first.review.arguments[1..],
            &[
                "install",
                "--global",
                "--ignore-scripts",
                "@earendil-works/pi-coding-agent@1.0.2"
            ]
        );
        assert!(first.review.arguments[0].ends_with("npm-cli.js"));
        for agent in ["claude", "codex"] {
            assert!(plan(agent, &node, &shim, temp.path()).is_ok());
        }
        let opencode = plan("opencode", &node, &shim, temp.path()).unwrap();
        assert_eq!(
            &opencode.review.arguments[1..],
            &["install", "--global", "opencode-ai@1.19.4"]
        );
        let grok = plan("grok", &node, &shim, temp.path()).unwrap();
        assert_eq!(
            &grok.review.arguments[1..],
            &["install", "--global", "@xai-official/grok@1.0.70"]
        );
        assert_eq!(grok.review.package, "@xai-official/grok");
        for agent in ["opencode-v2", "omp", "cursor", "hermes", "antigravity"] {
            assert!(
                matches!(
                    plan(agent, &node, &shim, temp.path()),
                    Err(InstallError::Unsupported)
                ),
                "{agent}"
            );
        }
        assert!(matches!(
            plan("pi;exec", &node, &shim, temp.path()),
            Err(InstallError::Unsupported)
        ));
        std::fs::write(&npm, "changed executable").unwrap();
        assert_ne!(
            first.review.revision,
            plan("pi", &node, &shim, temp.path())
                .unwrap()
                .review
                .revision
        );
        std::fs::write(
            npm.parent().unwrap().parent().unwrap().join("package.json"),
            r#"{"name":"different"}"#,
        )
        .unwrap();
        assert!(matches!(
            plan("pi", &node, &shim, temp.path()),
            Err(InstallError::MissingNpm)
        ));
    }
}
