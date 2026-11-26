//! Reviewed vendor CLI recipes. Never evaluates shell text or opens credentials/configuration.
use ghost_process::{EnvKey, EnvMap};
use ghost_protocol::{
    agents::{AgentHost, AgentInstallation},
    config_vendor::{
        VendorAction as A, VendorKind as K, VendorProvider as P, VendorRequest, VendorReview,
    },
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf, Prefix},
};

#[derive(Debug, thiserror::Error)]
pub enum VendorError {
    #[error("This action is not supported by this provider's reviewed CLI. No command has run.")]
    Unsupported,
    #[error(
        "Use a qualified plugin name, a marketplace name, a GitHub repository (owner/repository), an https Git address, an existing local folder or a pinned npm/GitHub package source. Credentials and command flags are not accepted."
    )]
    Target,
    #[error("The installation or configuration location changed. Refresh agents and review again.")]
    Changed,
    #[error("The native home or configuration folder is unavailable or uses a relative override.")]
    Folder,
}

pub struct VendorPlan {
    pub review: VendorReview,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub folder: PathBuf,
    pub config: PathBuf,
    pub set_env: Vec<(OsString, OsString)>,
}

pub fn prepare(
    installation: &AgentInstallation,
    env: &EnvMap,
    request: &VendorRequest,
) -> Result<VendorPlan, VendorError> {
    if installation.host != AgentHost::Native
        || crate::installation::provider_namespace(&installation.agent_id)
            != request.provider.agent_id()
    {
        return Err(VendorError::Unsupported);
    }
    let minimum = match request.provider {
        P::Claude => "2.1.310",
        P::Codex => "0.162.1",
        P::Pi => "1.0.2",
        P::Grok => "1.0.70",
        _ => "0.0.0",
    };
    if request.kind != K::Native && !crate::version_at_least(&installation.version, minimum) {
        return Err(VendorError::Unsupported);
    }
    let command = arguments(request)?;
    // A local folder a vendor clones or reads from must exist now: typed, not synced.
    if local_folder(&request.target) && !Path::new(&request.target).is_dir() {
        return Err(VendorError::Target);
    }
    crate::maintenance::check_unchanged(installation).map_err(|_| VendorError::Changed)?;
    let variable = |name: &str| {
        env.get(&EnvKey::from(name))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let folder = if cfg!(windows) {
        variable("USERPROFILE").or_else(|| variable("HOME"))
    } else {
        variable("HOME")
    }
    .ok_or(VendorError::Folder)?;
    if !folder.is_absolute() {
        return Err(VendorError::Folder);
    }
    let folder = std::fs::canonicalize(folder).map_err(|_| VendorError::Folder)?;
    let (key, fallback) = config_path(request.provider);
    let config = bind_path(&variable(key).unwrap_or_else(|| folder.join(fallback)))?;
    let crate::npm_entry::Command {
        program,
        prefix,
        set_env: launcher_env,
    } = crate::npm_entry::command(
        &installation.agent_id,
        Path::new(&installation.launch_path),
        env,
    )
    .map_err(|_| VendorError::Unsupported)?;
    let program = std::fs::canonicalize(program).map_err(|_| VendorError::Changed)?;
    let mut args = prefix.clone();
    args.extend(command.into_iter().map(OsString::from));
    let arguments: Vec<String> = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned).ok_or(VendorError::Changed))
        .collect::<Result<_, _>>()?;
    let program_text = text(&program)?;
    let folder_text = text(&folder)?;
    let config_text = text(&config)?;
    let mut hash = Sha256::new();
    for value in std::iter::once(&program_text)
        .chain([&folder_text, &config_text, &installation.version])
        .chain(arguments.iter())
    {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    crate::installer::hash_file(&mut hash, &program).map_err(|_| VendorError::Changed)?;
    for path in &prefix {
        crate::installer::hash_file(&mut hash, Path::new(path))
            .map_err(|_| VendorError::Changed)?;
    }
    // What the vendor's own launcher sets (Pi's install root) is part of what was reviewed.
    for (key, value) in &launcher_env {
        for part in [key, value] {
            let part = part.to_str().ok_or(VendorError::Changed)?;
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
    }
    let mut notices = vec![
        "Changes this device's personal agent configuration. Project and cloud-managed integrations are excluded.".into(),
        "The vendor may download or execute third-party code. Review its terminal prompts before agreeing. No confirmation is supplied automatically.".into(),
        "Closing the terminal or reaching ten minutes stops this owned command. Partial vendor changes are retained; refresh the Library before retrying. There is no automatic rollback of downloaded code.".into(),
        "Terminal output is not logged or synchronized. Ghost does not inspect vendor credentials. Refresh the Library after completion to verify the result.".into(),
    ];
    if request.kind == K::Native {
        notices=vec![format!("Opens this agent's native interactive CLI. Use its {} customization screen or command. No slash command is typed automatically.",request.target),"Native changes belong to this agent and device. They do not become Ghost shared MCP definitions. The terminal is not logged or synchronized.".into(),"Closing the terminal stops this owned process. Existing native homes and configuration locations are preserved.".into()];
    }
    if request.kind == K::Marketplace && request.action == A::Remove {
        notices.push("Removing this marketplace can uninstall its plugins and delete their vendor-managed data, options and secrets. This cannot be undone by Library copy history.".into());
    }
    if request.provider == P::Claude && request.kind == K::Plugin && request.action == A::Remove {
        notices.push(
            "Plugin removal keeps its persistent data using the vendor's --keep-data option."
                .into(),
        );
    }
    let review = VendorReview {
        token: String::new(),
        request: request.clone(),
        program: program_text,
        arguments,
        working_directory: folder_text,
        config_directory: config_text,
        revision: format!("{:x}", hash.finalize()),
        timeout_seconds: 600,
        notices,
    };
    // Pin both home spellings and the provider override so vendor cwd-dependent defaults cannot
    // disagree with the reviewed personal realm. These are paths only, never credential values.
    let mut set_env = vec![
        ("HOME".into(), folder.as_os_str().into()),
        ("USERPROFILE".into(), folder.as_os_str().into()),
    ];
    if !key.is_empty() && request.kind != K::Native {
        set_env.push((key.into(), config.as_os_str().into()));
    }
    set_env.extend(launcher_env);
    Ok(VendorPlan {
        review,
        program,
        args,
        folder,
        config,
        set_env,
    })
}

pub fn config_path(provider: P) -> (&'static str, &'static str) {
    match provider {
        P::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
        P::Codex => ("CODEX_HOME", ".codex"),
        P::Pi => ("PI_CODING_AGENT_DIR", ".pi/agent"),
        P::OpenCode => ("OPENCODE_CONFIG_DIR", ".config/opencode"),
        P::Hermes => ("HERMES_HOME", ".hermes"),
        P::Cursor => ("", ".cursor"),
        P::Grok => ("", ".grok"),
        P::Antigravity => ("", ".gemini"),
    }
}
fn text(path: &Path) -> Result<String, VendorError> {
    path.to_str().map(str::to_owned).ok_or(VendorError::Folder)
}
fn bind_path(path: &Path) -> Result<PathBuf, VendorError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(VendorError::Folder);
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let root = std::fs::canonicalize(ancestor).map_err(|_| VendorError::Folder)?;
                if !root.is_dir() {
                    return Err(VendorError::Folder);
                }
                return Ok(root.join(
                    path.strip_prefix(ancestor)
                        .map_err(|_| VendorError::Folder)?,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(VendorError::Folder),
        }
    }
    Err(VendorError::Folder)
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && !matches!(value.to_ascii_lowercase().as_str(), "synced" | "claudeai")
}
fn qualified(value: &str) -> bool {
    value
        .split_once('@')
        .is_some_and(|(plugin, market)| name(plugin) && name(market))
}
fn github(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, repo)| name(owner) && name(repo))
}
/// A vendor list label, which may hold spaces (Grok's "xAI Official"): printable, not a flag.
fn label(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= 128
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}
/// An `https` Git address without credentials, port tricks, query or fragment.
fn https_git(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['-', '.'])
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte))
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-~".contains(&byte))
        })
}
/// An absolute folder on a local disk, `C:\...` on Windows and `/...` elsewhere, that never climbs
/// out. Network, device and verbatim paths are refused by their parsed prefix before anything
/// touches the filesystem, whatever their separators: Windows reads `\/server/share` and
/// `/\server\share` as network shares too, and checking one for a folder would reach that server.
fn local_folder(value: &str) -> bool {
    let path = Path::new(value);
    let mut parts = path.components();
    let rooted = if cfg!(windows) {
        matches!(
            parts.next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        ) && matches!(parts.next(), Some(Component::RootDir))
    } else {
        matches!(parts.next(), Some(Component::RootDir))
    };
    rooted
        && path.is_absolute()
        && !value.starts_with("\\\\")
        && !value.starts_with("//")
        && !value.chars().any(char::is_control)
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}
/// Where a marketplace or plugin comes from: GitHub `owner/repo`, an `https` Git address or a
/// local folder, which each vendor CLI clones or reads itself.
fn source(value: &str) -> bool {
    github(value) || https_git(value) || local_folder(value)
}
fn reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
        && !value.contains("..")
        && !value.starts_with(['-', '/'])
}
fn package(value: &str, pinned: bool) -> bool {
    if let Some(source) = value.strip_prefix("npm:") {
        let (id, version) = source
            .rsplit_once('@')
            .filter(|(id, _)| !id.is_empty())
            .map_or((source, None), |(id, version)| (id, Some(version)));
        let valid_id = if let Some(scoped) = id.strip_prefix('@') {
            github(scoped)
        } else {
            name(id)
        };
        return valid_id
            && version.is_none_or(|value| {
                let base = value.split('-').next().unwrap_or("");
                base.split('.').count() == 3
                    && base
                        .split('.')
                        .all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                    && reference(value)
            })
            && (!pinned || version.is_some());
    }
    if let Some(source) = value.strip_prefix("git:github.com/") {
        let (repo, version) = source
            .split_once('@')
            .map_or((source, None), |(repo, version)| (repo, Some(version)));
        return github(repo) && version.is_none_or(reference) && (!pinned || version.is_some());
    }
    false
}
pub fn arguments(request: &VendorRequest) -> Result<Vec<String>, VendorError> {
    if request.kind == K::Native {
        if request.action != A::Open
            || !matches!(
                request.target.as_str(),
                "plugins"
                    | "skills"
                    | "mcp"
                    | "hooks"
                    | "commands"
                    | "agents"
                    | "rules"
                    | "memory"
                    | "settings"
            )
        {
            return Err(VendorError::Unsupported);
        }
        return Ok(if request.provider == P::Hermes {
            vec!["chat".into()]
        } else {
            Vec::new()
        });
    }
    if request.target.len() > 320
        || request.item_revision.as_ref().is_some_and(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(VendorError::Target);
    }
    let t = request.target.as_str();
    let values = match (request.provider, request.kind, request.action) {
        (P::Claude | P::Codex, K::Plugin, _) if !qualified(t) => return Err(VendorError::Target),
        (P::Claude, K::Plugin, action) => {
            let command = match action {
                A::Install => "install",
                A::Update => "update",
                A::Remove => "uninstall",
                A::Enable => "enable",
                A::Disable => "disable",
                A::Open => return Err(VendorError::Unsupported),
            };
            let mut values = vec!["plugin", command, t, "--scope", "user"];
            if action == A::Remove {
                values.push("--keep-data");
            }
            values
        }
        (P::Codex, K::Plugin, A::Install) => vec!["plugin", "add", t],
        (P::Codex, K::Plugin, A::Remove) => vec!["plugin", "remove", t],
        (P::Claude | P::Codex | P::Grok, K::Marketplace, A::Install) => {
            if !source(t) {
                return Err(VendorError::Target);
            }
            let mut values = vec!["plugin", "marketplace", "add", t];
            if request.provider == P::Claude {
                values.extend(["--scope", "user"]);
            }
            values
        }
        (P::Claude | P::Codex, K::Marketplace, A::Update | A::Remove) => {
            if !name(t) {
                return Err(VendorError::Target);
            }
            let command = match (request.provider, request.action) {
                (_, A::Remove) => "remove",
                (P::Codex, _) => "upgrade",
                _ => "update",
            };
            let mut values = vec!["plugin", "marketplace", command, t];
            if request.provider == P::Claude && request.action == A::Remove {
                values.extend(["--scope", "user"]);
            }
            values
        }
        (P::Pi, K::Package, A::Install | A::Remove | A::Update) => {
            if !package(t, request.action == A::Install) {
                return Err(VendorError::Target);
            }
            match request.action {
                A::Install => vec!["install", t, "--no-approve"],
                A::Remove => vec!["remove", t, "--no-approve"],
                _ => vec!["update", "--extension", t, "--no-approve"],
            }
        }
        (P::Grok, K::Marketplace, A::Update | A::Remove) => {
            if !label(t) {
                return Err(VendorError::Target);
            }
            let command = if request.action == A::Remove {
                "remove"
            } else {
                "update"
            };
            vec!["plugin", "marketplace", command, t]
        }
        (P::Grok, K::Plugin, A::Install) => {
            if !source(t) {
                return Err(VendorError::Target);
            }
            vec!["plugin", "install", t]
        }
        (P::Grok, K::Plugin, action) => {
            if !name(t) {
                return Err(VendorError::Target);
            }
            let command = match action {
                A::Update => "update",
                A::Remove => "uninstall",
                A::Enable => "enable",
                A::Disable => "disable",
                A::Install | A::Open => return Err(VendorError::Unsupported),
            };
            vec!["plugin", command, t]
        }
        (P::Hermes, K::Plugin, A::Install) => {
            if !(name(t) || github(t) || https_git(t)) {
                return Err(VendorError::Target);
            }
            vec!["plugins", "install", t]
        }
        (P::Hermes, K::Plugin, action) => {
            if !name(t) {
                return Err(VendorError::Target);
            }
            let command = match action {
                A::Update => "update",
                A::Remove => "remove",
                A::Enable => "enable",
                A::Disable => "disable",
                A::Install | A::Open => return Err(VendorError::Unsupported),
            };
            vec!["plugins", command, t]
        }
        // Hermes calls its skill sources taps: GitHub repositories only.
        (P::Hermes, K::Marketplace, A::Install | A::Remove) => {
            if !github(t) {
                return Err(VendorError::Target);
            }
            let command = if request.action == A::Remove {
                "remove"
            } else {
                "add"
            };
            vec!["skills", "tap", command, t]
        }
        _ => return Err(VendorError::Unsupported),
    };
    Ok(values.into_iter().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(provider: P, kind: K, action: A, target: &str) -> VendorRequest {
        VendorRequest {
            provider,
            kind,
            action,
            target: target.into(),
            item_revision: None,
        }
    }
    #[test]
    fn exact_vendor_commands_preserve_interactive_consent_and_personal_scope() {
        assert_eq!(
            arguments(&request(P::Claude, K::Plugin, A::Remove, "tools@team")).unwrap(),
            [
                "plugin",
                "uninstall",
                "tools@team",
                "--scope",
                "user",
                "--keep-data"
            ]
        );
        assert_eq!(
            arguments(&request(P::Claude, K::Marketplace, A::Remove, "team")).unwrap(),
            ["plugin", "marketplace", "remove", "team", "--scope", "user"]
        );
        assert_eq!(
            arguments(&request(P::Codex, K::Marketplace, A::Update, "team")).unwrap(),
            ["plugin", "marketplace", "upgrade", "team"]
        );
        assert_eq!(
            arguments(&request(P::Pi, K::Package, A::Update, "npm:@team/tools")).unwrap(),
            ["update", "--extension", "npm:@team/tools", "--no-approve"]
        );
        assert!(matches!(
            arguments(&request(P::Codex, K::Plugin, A::Enable, "tools@team")),
            Err(VendorError::Unsupported)
        ));
        assert!(matches!(
            arguments(&request(P::Codex, K::Plugin, A::Update, "tools@team")),
            Err(VendorError::Unsupported)
        ));
    }
    #[test]
    fn rejects_flags_private_sources_ambiguous_names_and_vendor_synced_namespace() {
        for target in [
            "",
            "--yes",
            "thing",
            "thing@synced",
            "thing@team --yes",
            "thing@a/b",
            "thing@team\n",
        ] {
            assert!(
                arguments(&request(P::Claude, K::Plugin, A::Install, target)).is_err(),
                "{target}"
            );
        }
        for target in [
            "npm:foo",
            "npm:foo@latest",
            "npm:foo@^1.2.3",
            "https://user:secret@github.com/a/b",
            "./local",
            "git:github.com/a/b",
            "npm:foo@1.2.3;echo",
        ] {
            assert!(
                arguments(&request(P::Pi, K::Package, A::Install, target)).is_err(),
                "{target}"
            );
        }
        for target in [
            "npm:tools@1.2.3",
            "npm:@team/tools@1.2.3-beta.1",
            "git:github.com/team/tools@v1.2.3",
        ] {
            assert!(
                arguments(&request(P::Pi, K::Package, A::Install, target)).is_ok(),
                "{target}"
            );
        }
    }
    #[test]
    fn executable_replacement_and_config_override_invalidate_review_without_execution() {
        use ghost_protocol::agents::{InstallChannel, VersionVerification};
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("claude.exe");
        std::fs::write(&binary, b"fixture executable, never run").unwrap();
        let binary = std::fs::canonicalize(binary).unwrap();
        let mut installation = AgentInstallation {
            agent_id: "claude".into(),
            provider: "claude".into(),
            host: AgentHost::Native,
            launch_path: binary.display().to_string(),
            resolved_path: Some(binary.display().to_string()),
            version: "2.1.310".into(),
            channel: InstallChannel::VendorStandalone,
            verification: VersionVerification::Unknown,
        };
        let mut env = EnvMap::default();
        env.insert("HOME".into(), temp.path().as_os_str().into());
        env.insert("USERPROFILE".into(), temp.path().as_os_str().into());
        let request = request(P::Claude, K::Plugin, A::Install, "tools@team");
        let first = prepare(&installation, &env, &request).unwrap();
        assert!(!first.args.iter().any(|value| {
            ["--yes", "-y", "--accept-command"]
                .iter()
                .any(|flag| value == flag)
        }));
        std::fs::write(&binary, b"replacement executable, never run").unwrap();
        assert_ne!(
            first.review.revision,
            prepare(&installation, &env, &request)
                .unwrap()
                .review
                .revision
        );
        let second = prepare(&installation, &env, &request).unwrap();
        env.insert(
            "CLAUDE_CONFIG_DIR".into(),
            temp.path().join("another").into_os_string(),
        );
        assert_ne!(
            second.review.revision,
            prepare(&installation, &env, &request)
                .unwrap()
                .review
                .revision
        );
        env.insert("CLAUDE_CONFIG_DIR".into(), "relative".into());
        assert!(matches!(
            prepare(&installation, &env, &request),
            Err(VendorError::Folder)
        ));
        installation.host = AgentHost::Unsupported;
        assert!(matches!(
            prepare(&installation, &env, &request),
            Err(VendorError::Unsupported)
        ));
    }

    #[test]
    fn marketplaces_come_from_github_https_git_or_a_local_folder() {
        let folder = if cfg!(windows) {
            r"C:\Users\person\marketplace"
        } else {
            "/home/person/marketplace"
        };
        for source in [
            "team/tools",
            "https://github.com/team/tools.git",
            "https://git.example.com:8443/team/tools",
            folder,
        ] {
            for provider in [P::Claude, P::Codex, P::Grok] {
                let args = arguments(&request(provider, K::Marketplace, A::Install, source));
                assert!(args.is_ok(), "{provider:?} {source}: {args:?}");
                assert_eq!(
                    &args.unwrap()[..4],
                    ["plugin", "marketplace", "add", source]
                );
            }
        }
        for refused in [
            "-rf",
            "https://user:secret@github.com/a/b",
            "https://github.com/a/b?ref=x",
            "https://github.com/a/b#main",
            "https://github.com/a/../b",
            "http://github.com/a/b",
            "relative/folder/x",
            "../outside",
            r"\\server\share\market",
            "//server/share",
            "team/tools; rm -rf",
        ] {
            assert!(
                arguments(&request(P::Grok, K::Marketplace, A::Install, refused)).is_err(),
                "{refused}"
            );
        }
        // Windows reads these as network shares, devices or verbatim paths whatever their
        // separators; none may pass as a local folder.
        if cfg!(windows) {
            for refused in [
                r"\/server/share",
                r"/\server\share",
                r"\/server\share\market",
                r"\\?\UNC\server\share",
                r"\\?\C:\market",
                r"/\?\C:\market",
                r"\\.\pipe\market",
                r"//./pipe/market",
                r"C:market",
                r"\??\C:\market",
                r"\market",
            ] {
                assert!(!local_folder(refused), "{refused}");
                assert!(
                    arguments(&request(P::Claude, K::Marketplace, A::Install, refused)).is_err(),
                    "{refused}"
                );
            }
            assert!(local_folder(r"D:\tools\market"));
            assert!(local_folder("C:/Users/person/market"));
        }
    }

    #[test]
    fn grok_and_hermes_get_their_own_reviewed_commands() {
        let args = |provider, kind, action, target: &str| {
            arguments(&request(provider, kind, action, target)).map_err(|error| error.to_string())
        };
        assert_eq!(
            args(P::Grok, K::Marketplace, A::Remove, "xAI Official").unwrap(),
            ["plugin", "marketplace", "remove", "xAI Official"]
        );
        assert_eq!(
            args(
                P::Grok,
                K::Marketplace,
                A::Update,
                "claude-plugins-official"
            )
            .unwrap(),
            ["plugin", "marketplace", "update", "claude-plugins-official"]
        );
        assert_eq!(
            args(P::Grok, K::Plugin, A::Install, "team/grok-tools").unwrap(),
            ["plugin", "install", "team/grok-tools"]
        );
        assert_eq!(
            args(P::Grok, K::Plugin, A::Remove, "vercel").unwrap(),
            ["plugin", "uninstall", "vercel"]
        );
        assert_eq!(
            args(P::Grok, K::Plugin, A::Disable, "vercel").unwrap(),
            ["plugin", "disable", "vercel"]
        );
        assert!(args(P::Grok, K::Marketplace, A::Remove, "-x").is_err());
        assert_eq!(
            args(
                P::Hermes,
                K::Plugin,
                A::Install,
                "anpicasso/hermes-plugin-chrome-profiles"
            )
            .unwrap(),
            [
                "plugins",
                "install",
                "anpicasso/hermes-plugin-chrome-profiles"
            ]
        );
        assert_eq!(
            args(P::Hermes, K::Plugin, A::Install, "browser-firecrawl").unwrap(),
            ["plugins", "install", "browser-firecrawl"]
        );
        assert_eq!(
            args(P::Hermes, K::Plugin, A::Enable, "browser-firecrawl").unwrap(),
            ["plugins", "enable", "browser-firecrawl"]
        );
        assert_eq!(
            args(P::Hermes, K::Marketplace, A::Install, "team/skills").unwrap(),
            ["skills", "tap", "add", "team/skills"]
        );
        assert_eq!(
            args(P::Hermes, K::Marketplace, A::Remove, "team/skills").unwrap(),
            ["skills", "tap", "remove", "team/skills"]
        );
        // Hermes taps are GitHub repositories only.
        assert!(
            args(
                P::Hermes,
                K::Marketplace,
                A::Install,
                "https://git.example.com/a/b"
            )
            .is_err()
        );
        assert!(args(P::Hermes, K::Marketplace, A::Update, "team/skills").is_err());
    }
}
