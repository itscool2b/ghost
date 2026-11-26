//! Explicit vendor maintenance. No credential files are opened and auth output is never retained.
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ghost_process::{
    EnvKey, EnvMap, HelperEnv, HelperOutput, HelperSpec, LeftoverRegistry, ResolvedEnv,
    find_program, run_helper,
};
use ghost_protocol::agents::{AgentAuthStatus, AgentHost, AgentInstallation, InstallChannel};

use crate::installation::provider_namespace;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaintenanceError {
    // One update or detection runs at a time, for every agent: the message must not claim it is
    // this agent's own.
    #[error("Ghost is updating or checking an agent right now. Try again in a moment.")]
    AlreadyRunning,
    #[error(
        "The vendor update exited successfully, but the installed binary did not pass its version check."
    )]
    VerificationFailed,
    #[error("This installation does not have a supported vendor maintenance command.")]
    Unsupported,
    #[error("This copy is managed by {0}; update that application instead.")]
    Bundled(String),
    #[error("The installation changed since detection. Refresh agents and try again.")]
    Changed,
    #[error("The installation channel is unknown. Use the vendor's installer.")]
    UnknownChannel,
    #[error("The required {0} command is unavailable.")]
    MissingProgram(&'static str),
    #[error("The vendor maintenance command failed to start.")]
    SpawnFailed,
    #[error("The vendor maintenance command timed out.")]
    TimedOut,
    /// The app offers to run the same command in its own terminal, where its output shows.
    #[error("The vendor's command failed{}.", exit_code(.0))]
    Failed(Option<i32>),
}

fn exit_code(code: &Option<i32>) -> String {
    code.map_or_else(String::new, |code| format!(" with exit code {code}"))
}

/// A reviewed program and argument vector. Never concatenate it into a shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

fn native(installation: &AgentInstallation) -> Result<&str, MaintenanceError> {
    if installation.host != AgentHost::Native {
        return Err(MaintenanceError::Unsupported);
    }
    Ok(provider_namespace(&installation.agent_id))
}

fn vendor(installation: &AgentInstallation, args: &[&str]) -> MaintenanceCommand {
    MaintenanceCommand {
        program: installation.launch_path.clone().into(),
        args: args.iter().map(OsString::from).collect(),
    }
}

/// Only the supported vendor CLI checks are run. Unknown agents do not get a guessed command.
pub fn auth_command(
    installation: &AgentInstallation,
) -> Result<MaintenanceCommand, MaintenanceError> {
    match native(installation)? {
        "claude" => Ok(vendor(installation, &["auth", "status", "--json"])),
        "codex" => Ok(vendor(installation, &["login", "status"])),
        _ => Err(MaintenanceError::Unsupported),
    }
}

/// The terminal sheet may launch this command when the user asks to sign in. It cannot run as a
/// background status probe: login owns the vendor's browser/terminal flow and may change auth state.
///
/// Only commands the vendor documents as its CLI's sign-in (checked 2026-10-27, sources in
/// docs/setup-actions.md). Pi, oh-my-pi and Antigravity sign in inside their own interface, so
/// they have none here.
pub fn login_command(
    installation: &AgentInstallation,
) -> Result<MaintenanceCommand, MaintenanceError> {
    match native(installation)? {
        "claude" => Ok(vendor(installation, &["auth", "login"])),
        "codex" => Ok(vendor(installation, &["login"])),
        // `cursor-agent login` opens the browser (Cursor CLI authentication reference).
        "cursor" => Ok(vendor(installation, &["login"])),
        // `grok login` signs in to Grok Build (xAI CLI reference).
        "grok" => Ok(vendor(installation, &["login"])),
        // `opencode auth login` chooses a provider and signs in or takes its key.
        "opencode" => Ok(vendor(installation, &["auth", "login"])),
        // `hermes auth` is Hermes' interactive sign-in for its providers.
        "hermes" => Ok(vendor(installation, &["auth"])),
        _ => Err(MaintenanceError::Unsupported),
    }
}

/// The vendor command that prints a long-lived sign-in token for an account added in Ghost
/// (docs/agent-accounts.md). Only Claude Code has one: `claude setup-token` runs the same browser
/// sign-in as `/login`, prints a one-year token and saves nothing (Claude Code authentication
/// docs, "Generate a long-lived token", checked 2026-11-01).
pub fn token_command(
    installation: &AgentInstallation,
) -> Result<MaintenanceCommand, MaintenanceError> {
    match native(installation)? {
        "claude" => Ok(vendor(installation, &["setup-token"])),
        _ => Err(MaintenanceError::Unsupported),
    }
}

/// Preserve channel ownership. A local package dependency is never converted to a global update.
pub fn update_command(
    installation: &AgentInstallation,
    env: &EnvMap,
) -> Result<MaintenanceCommand, MaintenanceError> {
    let provider = native(installation)?;
    match &installation.channel {
        InstallChannel::Bundled { application } => {
            Err(MaintenanceError::Bundled(application.clone()))
        }
        InstallChannel::Unknown => Err(MaintenanceError::UnknownChannel),
        // Each vendor's own installer updates through its CLI: `claude update`, `codex update`,
        // `grok update` and `cursor-agent update`.
        InstallChannel::VendorStandalone
            if matches!(provider, "claude" | "codex" | "grok" | "cursor") =>
        {
            Ok(vendor(installation, &["update"]))
        }
        InstallChannel::VendorStandalone => Err(MaintenanceError::Unsupported),
        InstallChannel::Npm { package } => {
            let expected = match provider {
                "claude" => "@anthropic-ai/claude-code",
                "codex" => "@openai/codex",
                "pi" => "@earendil-works/pi-coding-agent",
                "omp" => "@oh-my-pi/pi-coding-agent",
                "grok" => "@xai-official/grok",
                _ => return Err(MaintenanceError::Unsupported),
            };
            if package != expected {
                return Err(MaintenanceError::UnknownChannel);
            }
            let prefix = npm_global_prefix(installation).ok_or(MaintenanceError::UnknownChannel)?;
            let program =
                find_program("npm", env).ok_or(MaintenanceError::MissingProgram("npm"))?;
            Ok(MaintenanceCommand {
                program,
                args: vec![
                    "install".into(),
                    "--global".into(),
                    "--prefix".into(),
                    prefix.into(),
                    format!("{package}@latest").into(),
                ],
            })
        }
        InstallChannel::Homebrew { formula } => {
            let expected = if provider == "claude" {
                "claude-code"
            } else {
                provider
            };
            if formula != expected && formula != &format!("{expected}@latest") {
                return Err(MaintenanceError::UnknownChannel);
            }
            let program =
                find_program("brew", env).ok_or(MaintenanceError::MissingProgram("brew"))?;
            let resolved = normalized(
                installation
                    .resolved_path
                    .as_deref()
                    .ok_or(MaintenanceError::Changed)?,
            )
            .to_ascii_lowercase();
            let mut args = vec![OsString::from("upgrade")];
            if resolved.contains("/caskroom/") {
                args.push("--cask".into());
            } else if !resolved.contains("/cellar/") {
                return Err(MaintenanceError::UnknownChannel);
            }
            args.push(formula.into());
            Ok(MaintenanceCommand { program, args })
        }
    }
}

fn normalized(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path);
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

fn npm_global_prefix(installation: &AgentInstallation) -> Option<String> {
    let resolved = installation.resolved_path.as_deref()?.replace('\\', "/");
    let (root, _) = resolved.split_once("/node_modules/")?;
    let root = root.strip_suffix("/lib").unwrap_or(root);
    // Detection canonicalizes the package entry, while PATH can retain a short Windows directory
    // name (hosted runners use RUNNER~1), a junction, or a lexical alias. Compare the same directory
    // identity before deciding that a reviewed global package is a project-local dependency.
    let launch_parent = std::path::Path::new(&installation.launch_path)
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .and_then(|parent| parent.into_os_string().into_string().ok());
    let launch = installation.launch_path.replace('\\', "/");
    let (parent, _) = launch.rsplit_once('/')?;
    let parent = normalized(launch_parent.as_deref().unwrap_or(parent));
    (parent == normalized(root) || parent == normalized(&format!("{root}/bin")))
        .then(|| root.into())
}

/// Before any helper runs, reject a launch symlink that now points at a different installation.
pub fn check_unchanged(installation: &AgentInstallation) -> Result<(), MaintenanceError> {
    let path = std::path::Path::new(&installation.launch_path);
    let actual = if cfg!(windows) && crate::npm_entry::is_shim(path) {
        crate::npm_entry::entry(&installation.agent_id, path).ok_or(MaintenanceError::Changed)?
    } else {
        std::fs::canonicalize(path).map_err(|_| MaintenanceError::Changed)?
    };
    let expected = installation
        .resolved_path
        .as_deref()
        .ok_or(MaintenanceError::Changed)?;
    if actual != std::path::Path::new(expected) {
        return Err(MaintenanceError::Changed);
    }
    Ok(())
}

/// Parse only the status. Email addresses, account identities and tokens never enter our model.
pub fn auth_status(provider: &str, output: &HelperOutput) -> AgentAuthStatus {
    let unknown = |reason: &str| AgentAuthStatus::Unknown {
        reason: reason.into(),
    };
    if output.timed_out {
        return unknown("The vendor authentication check timed out.");
    }
    if output.stdout_truncated {
        return unknown("The vendor authentication response was too large.");
    }
    let code = output.exit.and_then(|exit| exit.code);
    match provider {
        "claude" => {
            let value = serde_json::from_slice::<serde_json::Value>(&output.stdout).ok();
            match (
                code,
                value
                    .as_ref()
                    .and_then(|value| value.get("loggedIn"))
                    .and_then(serde_json::Value::as_bool),
            ) {
                (Some(0), Some(true)) => AgentAuthStatus::CredentialsPresent,
                (Some(1), Some(false)) => AgentAuthStatus::SignInRequired,
                _ => unknown("Claude did not return a recognized authentication status."),
            }
        }
        "codex" => match code {
            Some(0) => AgentAuthStatus::CredentialsPresent,
            Some(1)
                if output
                    .stderr_tail
                    .lines()
                    .any(|line| line.trim() == "Not logged in") =>
            {
                AgentAuthStatus::SignInRequired
            }
            _ => unknown("Codex could not determine its authentication status."),
        },
        _ => AgentAuthStatus::Unsupported,
    }
}

/// The most of an updater's output that is kept to show: its last lines, not a log.
const OUTPUT_LINES: usize = 40;
const OUTPUT_BYTES: usize = 4 * 1024;

/// What a vendor's command printed, as plain text a person can read: standard output then standard
/// error, without terminal escapes or control characters, a redrawn progress line reduced to what
/// it last showed, and only the last lines. It is shown to the person who ran the update and
/// nowhere else: it is not written to disk, logged or sent anywhere.
pub fn readable_output(output: &HelperOutput) -> String {
    let mut raw = String::from_utf8_lossy(&output.stdout).into_owned();
    if !raw.is_empty() && !raw.ends_with('\n') {
        raw.push('\n');
    }
    raw.push_str(&output.stderr_tail);
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        // A carriage return redraws the line: what follows the last one is what stayed on screen.
        let shown = line
            .trim_end_matches('\r')
            .rsplit('\r')
            .next()
            .unwrap_or_default();
        let clean = without_escapes(shown);
        let clean = clean.trim_end();
        // One blank line between paragraphs is kept; runs of them are not.
        if clean.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(clean.to_owned());
    }
    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev().take(OUTPUT_LINES) {
        bytes += line.len() + 1;
        if bytes > OUTPUT_BYTES && !kept.is_empty() {
            break;
        }
        kept.push(line);
    }
    kept.reverse();
    let mut text = kept.join("\n");
    if text.len() > OUTPUT_BYTES {
        // One very long line: keep its end, cut on a character boundary.
        let mut start = text.len() - OUTPUT_BYTES;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text = text[start..].to_owned();
    }
    text
}

/// `line` without ANSI escape sequences (colours, cursor moves, window titles) and without any
/// other control character but a tab.
fn without_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c == '\t' || !c.is_control() {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, ended by a byte from @ to ~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC (a window title, a hyperlink): ended by BEL or by ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // A two-character escape, or the end of the line.
            _ => {}
        }
    }
    out
}

pub(crate) async fn run(
    command: MaintenanceCommand,
    env: Arc<ResolvedEnv>,
    registry: &LeftoverRegistry,
    timeout: Duration,
) -> Result<HelperOutput, MaintenanceError> {
    let home = env
        .vars
        .get(&EnvKey::from(if cfg!(windows) {
            "USERPROFILE"
        } else {
            "HOME"
        }))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    run_helper(
        registry,
        HelperSpec {
            program: command.program,
            args: command.args,
            cwd: home,
            env: HelperEnv::Resolved(env),
            set: Vec::new(),
            new_session: false,
            timeout,
            max_stdout: 64 * 1024,
        },
    )
    .await
    .map_err(|_| MaintenanceError::SpawnFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_process::ExitInfo;
    use ghost_protocol::agents::VersionVerification;

    fn installation(channel: InstallChannel) -> AgentInstallation {
        AgentInstallation {
            agent_id: "codex".into(),
            provider: "codex".into(),
            host: AgentHost::Native,
            launch_path: "/opt/npm/bin/codex".into(),
            resolved_path: Some("/opt/npm/lib/node_modules/@openai/codex/bin/codex.js".into()),
            version: "0.162.1".into(),
            channel,
            verification: VersionVerification::Unknown,
        }
    }

    #[test]
    fn grok_and_cursor_from_their_own_installers_update_through_their_cli() {
        for (agent, launch) in [
            ("grok", "/home/me/.grok/bin/grok"),
            ("cursor", "/home/me/.local/share/cursor-agent/cursor-agent"),
        ] {
            let mut installed = installation(InstallChannel::VendorStandalone);
            installed.agent_id = agent.into();
            installed.launch_path = launch.into();
            let command = update_command(&installed, &EnvMap::new()).unwrap();
            assert_eq!(command.program, std::path::PathBuf::from(launch));
            assert_eq!(command.args, vec![OsString::from("update")]);
        }
        // An agent whose own installer has no reviewed update command still gets none.
        let mut other = installation(InstallChannel::VendorStandalone);
        other.agent_id = "hermes".into();
        assert_eq!(
            update_command(&other, &EnvMap::new()),
            Err(MaintenanceError::Unsupported)
        );
    }

    #[test]
    fn unknown_and_bundled_copies_are_not_updated_and_unsupported_hosts_never_launch_locally() {
        assert_eq!(
            update_command(&installation(InstallChannel::Unknown), &EnvMap::new()),
            Err(MaintenanceError::UnknownChannel)
        );
        assert!(matches!(
            update_command(
                &installation(InstallChannel::Bundled {
                    application: "Editor".into()
                }),
                &EnvMap::new()
            ),
            Err(MaintenanceError::Bundled(_))
        ));
        let mut wsl = installation(InstallChannel::VendorStandalone);
        wsl.host = AgentHost::Wsl {
            distro: "Ubuntu".into(),
        };
        assert_eq!(auth_command(&wsl), Err(MaintenanceError::Unsupported));
        assert_eq!(
            update_command(&wsl, &EnvMap::new()),
            Err(MaintenanceError::Unsupported)
        );
    }

    #[test]
    fn sign_in_runs_only_the_vendors_documented_login_of_the_detected_executable() {
        for (agent, args) in [
            ("claude", &["auth", "login"][..]),
            ("codex", &["login"]),
            ("cursor", &["login"]),
            ("grok", &["login"]),
            ("grok-stream", &["login"]),
            ("opencode", &["auth", "login"]),
            ("opencode-v2", &["auth", "login"]),
            ("hermes", &["auth"]),
        ] {
            let mut installed = installation(InstallChannel::Unknown);
            installed.agent_id = agent.into();
            installed.launch_path = format!("/opt/bin/{agent}");
            let command = login_command(&installed).unwrap();
            assert_eq!(command.program, PathBuf::from(format!("/opt/bin/{agent}")));
            assert_eq!(
                command.args,
                args.iter().map(OsString::from).collect::<Vec<_>>()
            );
        }
        // These sign in inside their own interface; no command is guessed for them.
        for agent in ["pi", "omp", "antigravity", "agy-stream", "mock"] {
            let mut installed = installation(InstallChannel::Unknown);
            installed.agent_id = agent.into();
            assert_eq!(
                login_command(&installed),
                Err(MaintenanceError::Unsupported),
                "{agent}"
            );
        }
        let mut wsl = installation(InstallChannel::Unknown);
        wsl.agent_id = "cursor".into();
        wsl.host = AgentHost::Wsl {
            distro: "Ubuntu".into(),
        };
        assert_eq!(login_command(&wsl), Err(MaintenanceError::Unsupported));
    }

    #[test]
    fn local_npm_packages_never_become_global_updates() {
        let mut installed = installation(InstallChannel::Npm {
            package: "@openai/codex".into(),
        });
        assert_eq!(npm_global_prefix(&installed), Some("/opt/npm".into()));
        installed.launch_path = "/repo/node_modules/.bin/codex".into();
        installed.resolved_path = Some("/repo/node_modules/@openai/codex/bin/codex.js".into());
        assert_eq!(npm_global_prefix(&installed), None);
        assert_eq!(
            update_command(&installed, &EnvMap::new()),
            Err(MaintenanceError::UnknownChannel)
        );
        installed.agent_id = "pi".into();
        installed.provider = "pi".into();
        installed.channel = InstallChannel::Npm {
            package: "@earendil-works/pi-coding-agent".into(),
        };
        installed.launch_path = "/home/user/.bun/bin/pi".into();
        installed.resolved_path = Some("/home/user/.bun/install/global/node_modules/@earendil-works/pi-coding-agent/dist/cli.js".into());
        assert_eq!(
            update_command(&installed, &EnvMap::new()),
            Err(MaintenanceError::UnknownChannel),
            "a Bun global package must not be handed to npm"
        );
    }

    #[test]
    fn global_npm_prefix_compares_canonical_directory_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let prefix = fixture.path().join("prefix");
        let package = prefix.join("node_modules/@openai/codex/bin");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("codex.js"), "synthetic package").unwrap();
        let mut installed = installation(InstallChannel::Npm {
            package: "@openai/codex".into(),
        });
        installed.launch_path = prefix
            .join("../prefix/codex.cmd")
            .to_string_lossy()
            .into_owned();
        installed.resolved_path = Some(
            std::fs::canonicalize(package.join("codex.js"))
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let expected = std::fs::canonicalize(&prefix).unwrap();
        assert_eq!(
            npm_global_prefix(&installed).map(|prefix| normalized(&prefix)),
            Some(normalized(&expected.to_string_lossy())),
        );
        // A canonicalized local .bin still cannot acquire permission for a global npm update.
        std::fs::create_dir_all(prefix.join("node_modules/.bin")).unwrap();
        installed.launch_path = prefix
            .join("node_modules/.bin/codex.cmd")
            .to_string_lossy()
            .into_owned();
        assert_eq!(npm_global_prefix(&installed), None);
    }

    #[test]
    fn authentication_errors_do_not_masquerade_as_logged_out_or_retain_output() {
        let output = |code, text: &str, stderr: &str| HelperOutput {
            exit: Some(ExitInfo {
                code: Some(code),
                signal: None,
                at_ms: 0,
            }),
            stdout: text.as_bytes().to_vec(),
            stderr_tail: stderr.into(),
            ..Default::default()
        };
        assert_eq!(
            auth_status(
                "claude",
                &output(
                    0,
                    r#"{"loggedIn":true,"email":"private@example.test","token":"discard"}"#,
                    ""
                )
            ),
            AgentAuthStatus::CredentialsPresent
        );
        assert_eq!(
            auth_status("claude", &output(1, r#"{"loggedIn":false}"#, "")),
            AgentAuthStatus::SignInRequired
        );
        assert!(matches!(
            auth_status("claude", &output(1, "error", "private details")),
            AgentAuthStatus::Unknown { .. }
        ));
        assert_eq!(
            auth_status("codex", &output(1, "", "Not logged in\n")),
            AgentAuthStatus::SignInRequired
        );
        assert!(matches!(
            auth_status(
                "codex",
                &output(1, "", "Error checking login status: private details")
            ),
            AgentAuthStatus::Unknown { .. }
        ));
    }

    fn printed(stdout: &str, stderr: &str) -> HelperOutput {
        HelperOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr_tail: stderr.into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_updaters_output_reads_as_plain_text() {
        // Colours, a window title, a hyperlink and a redrawn progress line: what stays is what a
        // terminal would have left on screen, with standard error after standard output.
        let output = printed(
            "\u{1b}[32mDownloading\u{1b}[0m 1.0.80\r\n10%\r50%\r100% done\n\n\n\u{1b}]0;updater\u{7}Installed\n",
            "\u{1b}]8;;https://example.test\u{1b}\\warning\u{1b}]8;;\u{1b}\\: restart to finish \u{7}\n",
        );
        assert_eq!(
            readable_output(&output),
            "Downloading 1.0.80\n100% done\n\nInstalled\nwarning: restart to finish"
        );
        assert_eq!(readable_output(&printed("", "")), "");
        assert_eq!(readable_output(&printed("\n \n", "\r\n")), "");
    }

    #[test]
    fn only_the_end_of_a_long_output_is_kept() {
        let many: String = (1..=500).map(|line| format!("line {line}\n")).collect();
        let kept = readable_output(&printed(&many, "the error\n"));
        assert_eq!(kept.lines().count(), OUTPUT_LINES);
        assert!(kept.ends_with("line 500\nthe error"), "{kept}");
        assert!(!kept.contains("line 1\n"));
        // One enormous line is cut to the limit on a character boundary, keeping its end.
        let wide = format!("{}é end", "é".repeat(OUTPUT_BYTES));
        let kept = readable_output(&printed(&wide, ""));
        assert!(kept.len() <= OUTPUT_BYTES);
        assert!(kept.ends_with("é end"));
        // Bytes that are not text do not break it.
        let binary = HelperOutput {
            stdout: vec![0xff, 0xfe, b'o', b'k', b'\n'],
            ..Default::default()
        };
        assert!(readable_output(&binary).ends_with("ok"));
    }

    #[test]
    fn a_failed_command_names_its_exit_code_and_nothing_else() {
        assert_eq!(
            MaintenanceError::Failed(Some(1)).to_string(),
            "The vendor's command failed with exit code 1."
        );
        assert_eq!(
            MaintenanceError::Failed(None).to_string(),
            "The vendor's command failed."
        );
    }
}
