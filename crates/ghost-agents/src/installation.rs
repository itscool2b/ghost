//! Installation classification uses paths as evidence, never as shell commands.
use ghost_protocol::agents::{AgentHost, AgentInstallation, InstallChannel, VersionVerification};

/// Metadata-only resolution follows symlinks without reading agent configuration or credentials.
/// Failure to resolve never changes the working launch path or invents an update channel.
pub(crate) fn native(
    agent_id: &str,
    printed_version: &str,
    launch_path: &str,
) -> Option<AgentInstallation> {
    let version = crate::catalog::parse_version(printed_version)?;
    let resolved_entry =
        if cfg!(windows) && crate::npm_entry::is_shim(std::path::Path::new(launch_path)) {
            crate::npm_entry::entry(agent_id, std::path::Path::new(launch_path))
        } else {
            None
        };
    let resolved_path = resolved_entry
        .or_else(|| std::fs::canonicalize(launch_path).ok())
        .and_then(|path| path.into_os_string().into_string().ok());
    let channel = resolved_path
        .as_deref()
        .map(|path| classify_channel(agent_id, path))
        .unwrap_or_default();
    let verified = match agent_id {
        "claude" => Some(("2.1.310", "Windows native smoke and protocol fixtures")),
        "claude-acp" => Some(("2.1.310", "ACP adapter 0.95.0 compatibility fixtures")),
        "codex" => Some((
            "0.162.1",
            "Windows free controls and protocol fixtures; tool smoke pending",
        )),
        _ => None,
    };
    Some(AgentInstallation {
        agent_id: agent_id.into(),
        provider: provider_namespace(agent_id).into(),
        host: AgentHost::Native,
        launch_path: launch_path.into(),
        resolved_path,
        verification: verification(&version, verified),
        version,
        channel,
    })
}

/// Native and alternate transports of the same vendor share one transcript namespace: the agent
/// the catalog files the connection under.
pub fn provider_namespace(agent_id: &str) -> &str {
    crate::catalog::agent_of(agent_id)
}

/// Canonical path classification is conservative. A copied or unknown binary stays unknown.
/// The caller still launches the original symlink/shim, never this classified target path.
pub fn classify_channel(agent_id: &str, resolved_path: &str) -> InstallChannel {
    let normalized = resolved_path.replace('\\', "/");
    let parts: Vec<_> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let lower: Vec<_> = parts.iter().map(|part| part.to_ascii_lowercase()).collect();
    // Bundled paths take precedence over nested npm/Homebrew-looking subdirectories.
    if lower
        .windows(3)
        .any(|parts| parts == ["programs", "openai", "codex"])
    {
        return InstallChannel::Bundled {
            application: "Codex Desktop".into(),
        };
    }
    if let Some(application) = parts.iter().zip(&lower).find_map(|(original, lower)| {
        lower
            .ends_with(".app")
            .then(|| original.trim_end_matches(".app").to_string())
    }) {
        return InstallChannel::Bundled { application };
    }
    if lower
        .windows(2)
        .any(|pair| pair == ["resources", "app"] || pair == ["resources", "app.asar.unpacked"])
    {
        return InstallChannel::Bundled {
            application: "desktop application".into(),
        };
    }
    if lower.iter().any(|part| part == "extensions")
        && lower.iter().any(|part| {
            part.starts_with("anthropic.claude-code-") || part.starts_with("openai.chatgpt-")
        })
    {
        return InstallChannel::Bundled {
            application: "editor extension".into(),
        };
    }
    let provider = provider_namespace(agent_id);
    // Pi's own installer keeps each release's package under install/releases/<version>; it is
    // not a global npm install, so npm never updates it.
    if provider == "pi"
        && lower.windows(6).any(|parts| {
            parts[0] == "install"
                && parts[1] == "releases"
                && parts[3..] == ["node_modules", "@earendil-works", "pi-coding-agent"]
        })
    {
        return InstallChannel::VendorStandalone;
    }
    let package = match provider {
        "claude" => Some(["@anthropic-ai", "claude-code"]),
        "codex" => Some(["@openai", "codex"]),
        "pi" => Some(["@earendil-works", "pi-coding-agent"]),
        "omp" => Some(["@oh-my-pi", "pi-coding-agent"]),
        "grok" => Some(["@xai-official", "grok"]),
        _ => None,
    };
    if let Some(package) = package.filter(|package| {
        lower
            .windows(3)
            .any(|parts| parts == ["node_modules", package[0], package[1]])
    }) {
        return InstallChannel::Npm {
            package: format!("{}/{}", package[0], package[1]),
        };
    }
    let formula = if provider == "claude" {
        "claude-code"
    } else {
        provider
    };
    if let Some(parts) = lower.windows(2).find(|parts| {
        matches!(parts[0].as_str(), "cellar" | "caskroom")
            && (parts[1] == formula || parts[1] == format!("{formula}@latest"))
    }) {
        return InstallChannel::Homebrew {
            formula: parts[1].clone(),
        };
    }
    if (provider == "claude"
        && lower
            .windows(4)
            .any(|parts| parts == [".local", "share", "claude", "versions"]))
        || (provider == "claude"
            && lower.ends_with(&[".local".into(), "bin".into(), "claude.exe".into()]))
        || (provider == "codex"
            && lower
                .windows(3)
                .any(|parts| parts == [".codex", "packages", "standalone"]))
        // Grok Build's installer: ~/.grok/bin/grok.
        || (provider == "grok" && lower.windows(2).any(|parts| parts == [".grok", "bin"]))
        // Cursor's installer: its own cursor-agent folder holding the versions it downloaded and
        // the launchers (%LOCALAPPDATA%cursor-agent on Windows, ~/.local/share/cursor-agent).
        || (provider == "cursor"
            && lower.windows(2).any(|parts| {
                parts[0] == "cursor-agent"
                    && (parts[1] == "versions"
                        || parts[1].starts_with("cursor-agent")
                        || parts[1].starts_with("agent"))
            }))
    {
        return InstallChannel::VendorStandalone;
    }
    InstallChannel::Unknown
}

/// A badge records exact compatibility evidence, not an inferred promise about a newer version.
pub fn verification(found: &str, verified: Option<(&str, &str)>) -> VersionVerification {
    match verified {
        Some((version, scope)) if found == version => VersionVerification::Verified {
            version: version.into(),
            scope: scope.into(),
        },
        Some((version, _)) => VersionVerification::Untested {
            last_verified: Some(version.into()),
        },
        None => VersionVerification::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_omp_and_grok_keep_their_distinct_reviewed_package_owners() {
        for (agent, package) in [
            ("pi", "@earendil-works/pi-coding-agent"),
            ("omp", "@oh-my-pi/pi-coding-agent"),
            ("grok", "@xai-official/grok"),
        ] {
            assert_eq!(
                classify_channel(agent, &format!("/prefix/lib/node_modules/{package}/cli.js")),
                InstallChannel::Npm {
                    package: package.into()
                }
            );
        }
        assert_eq!(
            classify_channel(
                "pi",
                "/prefix/lib/node_modules/@oh-my-pi/pi-coding-agent/cli.js"
            ),
            InstallChannel::Unknown
        );
    }

    #[test]
    fn pis_own_installer_is_its_vendors_channel_not_npm() {
        let managed = r"C:\Users\me\.pi\agent\install\releases\1.0.2\node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js";
        assert_eq!(
            classify_channel("pi", managed),
            InstallChannel::VendorStandalone
        );
        // oh-my-pi's package in the same place is not Pi's installer.
        assert_eq!(
            classify_channel(
                "omp",
                "/home/me/.pi/agent/install/releases/1.0.2/node_modules/@oh-my-pi/pi-coding-agent/cli.js"
            ),
            InstallChannel::Npm {
                package: "@oh-my-pi/pi-coding-agent".into()
            }
        );
        // A release folder by another name is a plain npm package.
        assert_eq!(
            classify_channel(
                "pi",
                "/home/me/releases/1.0.2/node_modules/@earendil-works/pi-coding-agent/cli.js"
            ),
            InstallChannel::Npm {
                package: "@earendil-works/pi-coding-agent".into()
            }
        );
    }

    #[test]
    fn grok_and_cursor_installers_are_their_vendors_own() {
        for (agent, path) in [
            ("grok", r"C:\Users\me\.grok\bin\grok"),
            ("grok", "/home/me/.grok/bin/grok"),
            (
                "cursor",
                r"C:\Users\me\AppData\Local\cursor-agent\cursor-agent.cmd",
            ),
            (
                "cursor",
                r"C:\Users\me\AppData\Local\cursor-agent\agent.cmd",
            ),
            (
                "cursor",
                "/home/me/.local/share/cursor-agent/versions/2026.10.22/cursor-agent",
            ),
        ] {
            assert_eq!(
                classify_channel(agent, path),
                InstallChannel::VendorStandalone,
                "{path}"
            );
        }
        // Lookalikes stay unknown: another agent's folder, or a copy elsewhere.
        assert_eq!(
            classify_channel("grok", "/home/me/.grok-old/grok"),
            InstallChannel::Unknown
        );
        assert_eq!(
            classify_channel("codex", "/home/me/.grok/bin/grok"),
            InstallChannel::Unknown
        );
        assert_eq!(
            classify_channel("cursor", "/home/me/tools/cursor-agent"),
            InstallChannel::Unknown
        );
    }

    #[test]
    fn known_package_and_vendor_layouts_are_classified_without_changing_launch_paths() {
        assert_eq!(
            classify_channel(
                "claude-acp",
                "C:\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js"
            ),
            InstallChannel::Npm {
                package: "@anthropic-ai/claude-code".into()
            }
        );
        assert_eq!(
            classify_channel(
                "codex",
                "/home/user/.codex/packages/standalone/releases/0.164.0/codex"
            ),
            InstallChannel::VendorStandalone
        );
        assert_eq!(
            classify_channel("claude", "/home/user/.local/share/claude/versions/2.1.310"),
            InstallChannel::VendorStandalone
        );
        assert_eq!(
            classify_channel("codex", "/opt/homebrew/Cellar/codex/0.164.0/bin/codex"),
            InstallChannel::Homebrew {
                formula: "codex".into()
            }
        );
        assert_eq!(
            classify_channel("codex", "/some/unknown/bin/codex"),
            InstallChannel::Unknown
        );
        assert_eq!(
            classify_channel(
                "claude",
                "/tmp/not-node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            InstallChannel::Unknown
        );
    }

    #[test]
    fn bundled_copies_are_read_only_even_when_they_contain_a_known_package() {
        assert!(matches!(
            classify_channel(
                "codex",
                "/Applications/Editor.app/Contents/Resources/node_modules/@openai/codex/bin/codex"
            ),
            InstallChannel::Bundled { .. }
        ));
        assert!(matches!(
            classify_channel(
                "claude",
                "C:\\Editor\\resources\\app\\node_modules\\@anthropic-ai\\claude-code\\cli.js"
            ),
            InstallChannel::Bundled { .. }
        ));
        assert!(matches!(
            classify_channel(
                "claude",
                "/home/user/.vscode/extensions/anthropic.claude-code-2.1.310/resources/claude"
            ),
            InstallChannel::Bundled { .. }
        ));
    }

    #[test]
    fn transport_aliases_share_a_provider_but_unrelated_agents_do_not() {
        assert_eq!(provider_namespace("codex"), provider_namespace("codex-acp"));
        assert_ne!(provider_namespace("codex"), provider_namespace("claude"));
        assert_eq!(provider_namespace("custom-codex"), "custom-codex");
    }

    #[test]
    fn prereleases_and_newer_versions_do_not_inherit_verified_badges() {
        let verified = Some(("0.164.0", "Windows protocol fixtures"));
        assert!(matches!(
            verification("0.164.0", verified),
            VersionVerification::Verified { .. }
        ));
        for version in ["0.164.0-alpha.1", "0.165.0", "0.163.0"] {
            assert!(matches!(
                verification(version, verified),
                VersionVerification::Untested { .. }
            ));
        }
    }
}
