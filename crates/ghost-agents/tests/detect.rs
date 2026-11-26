//! Detection against fake tools on a fake `PATH`: missing and broken binaries, probes that hang,
//! Node's floor, the user's own CLI behind an adapter (its variable, its floor), and npm only until
//! the adapter is installed.

#[cfg(test)]
mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Machine, Reaper, gone, spec};
use ghost_agents::LaunchError;
use ghost_process::ProcessCwd;
use ghost_protocol::session::Availability;
use tokio::sync::mpsc;

#[cfg(test)]
fn progress() -> mpsc::UnboundedSender<String> {
    mpsc::unbounded_channel().0
}

#[cfg(test)]
fn ready_path(availability: &Availability) -> PathBuf {
    match availability {
        Availability::Ready { path, .. } => PathBuf::from(path),
        other => panic!("ready: {other:?}"),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn reviewed_npm_bins_launch_through_node_without_running_shell_shims() {
    use ghost_protocol::agents::InstallChannel;

    for (id, package, version) in [
        ("pi", "@earendil-works/pi-coding-agent", "1.0.2"),
        ("omp", "@oh-my-pi/pi-coding-agent", "18.12.6"),
        ("grok", "@xai-official/grok", "1.0.70"),
    ] {
        let machine = Machine::with_tools(&["node", "npm"]);
        let shim = machine.bin.join(format!("{id}.cmd"));
        std::fs::write(&shim, "@exit /b 99\r\n").unwrap();
        let root = machine.bin.join("node_modules").join(package);
        std::fs::create_dir_all(root.join("dist")).unwrap();
        let entry = root.join("dist/cli.js");
        std::fs::write(&entry, format!("FAKE_ADAPTER_VERSION={version}\n")).unwrap();
        std::fs::write(
            root.join("package.json"),
            serde_json::json!({
                "name":package,"bin":{id:"dist/cli.js"}
            })
            .to_string(),
        )
        .unwrap();
        let env = machine.env(&[]);
        let detector = machine.detector(env.clone());
        let agent = spec(id);
        assert_eq!(ready_path(&detector.detect(&agent).await), shim);
        let launch = detector.prepare_launch(&agent, progress()).await.unwrap();
        assert_eq!(launch.program, machine.tool("node"));
        // The entry detection checked, in its plain spelling: Node cannot start a verbatim
        // (\\?\C:\...) main script.
        let canonical = std::fs::canonicalize(&entry).unwrap();
        let canonical = canonical.to_str().unwrap();
        let plain = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
        assert_eq!(launch.args[0], std::ffi::OsString::from(plain));
        assert!(!launch.args[0].to_string_lossy().starts_with(r"\\?\"));
        let installed = detector
            .detect_all(false)
            .await
            .into_iter()
            .find(|agent| agent.id == id)
            .unwrap()
            .installation
            .unwrap();
        assert_eq!(
            installed.channel,
            InstallChannel::Npm {
                package: package.into()
            }
        );
        ghost_agents::maintenance::check_unchanged(&installed).unwrap();
        let update = ghost_agents::maintenance::update_command(&installed, &env.vars).unwrap();
        assert_eq!(update.program, machine.tool("npm"));
        assert!(
            update
                .args
                .iter()
                .any(|arg| arg == std::ffi::OsStr::new(&format!("{package}@latest")))
        );
        assert!(
            machine
                .runs("node")
                .iter()
                .any(|run| { run["argv"] == serde_json::json!([plain, "--version"]) })
        );
        std::fs::write(root.join("dist/other.js"), "replacement").unwrap();
        std::fs::write(
            root.join("package.json"),
            serde_json::json!({
                "name":package,"bin":{id:"dist/other.js"}
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            ghost_agents::maintenance::check_unchanged(&installed),
            Err(ghost_agents::maintenance::MaintenanceError::Changed)
        );
    }
}

/// Pi's own installer: `bin/pi.cmd` stands for the release `install/current-version` names. It
/// runs through Node with the install root Pi's launcher would announce, and npm never updates it.
#[cfg(windows)]
#[tokio::test]
async fn pis_own_installer_launches_its_current_release_through_node() {
    use ghost_agents::maintenance::{MaintenanceError, check_unchanged, update_command};
    use ghost_protocol::agents::InstallChannel;

    let machine = Machine::with_tools(&["node", "npm"]);
    let shim = machine.bin.join("pi.cmd");
    std::fs::write(&shim, "@exit /b 99\r\n").unwrap();
    std::fs::write(machine.bin.join("pi-launcher.js"), "never run").unwrap();
    let install = machine.root.path().join("install");
    let root = install
        .join("releases")
        .join("1.0.2")
        .join("node_modules")
        .join("@earendil-works/pi-coding-agent");
    std::fs::create_dir_all(root.join("dist/bundle")).unwrap();
    let entry = root.join("dist/bundle/cli.js");
    std::fs::write(&entry, "FAKE_ADAPTER_VERSION=1.0.2\n").unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"@earendil-works/pi-coding-agent","bin":{"pi":"dist/bundle/cli.js"}}"#,
    )
    .unwrap();
    std::fs::write(
        install.join("managed-install.json"),
        r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
    )
    .unwrap();
    std::fs::write(install.join("current-version"), "1.0.2\n").unwrap();
    let env = machine.env(&[]);
    let detector = machine.detector(env.clone());
    let agent = spec("pi");
    assert_eq!(ready_path(&detector.detect(&agent).await), shim);
    let launch = detector.prepare_launch(&agent, progress()).await.unwrap();
    assert_eq!(launch.program, machine.tool("node"));
    let canonical = std::fs::canonicalize(&entry).unwrap();
    let canonical = canonical.to_str().unwrap();
    let plain = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
    assert_eq!(launch.args[0], std::ffi::OsString::from(plain));
    assert_eq!(
        launch.set_env,
        vec![(
            std::ffi::OsString::from("PI_MANAGED_INSTALL_ROOT"),
            install.clone().into_os_string()
        )]
    );
    assert!(
        machine
            .runs("node")
            .iter()
            .any(|run| run["argv"] == serde_json::json!([plain, "--version"]))
    );
    let installed = detector
        .detect_all(false)
        .await
        .into_iter()
        .find(|agent| agent.id == "pi")
        .unwrap()
        .installation
        .unwrap();
    assert_eq!(installed.channel, InstallChannel::VendorStandalone);
    check_unchanged(&installed).unwrap();
    assert!(matches!(
        update_command(&installed, &env.vars),
        Err(MaintenanceError::Unsupported)
    ));
    // A new release the version file names is a changed installation.
    let newer = install
        .join("releases")
        .join("1.1.0")
        .join("node_modules")
        .join("@earendil-works/pi-coding-agent");
    std::fs::create_dir_all(newer.join("dist/bundle")).unwrap();
    std::fs::write(
        newer.join("dist/bundle/cli.js"),
        "FAKE_ADAPTER_VERSION=1.1.0\n",
    )
    .unwrap();
    std::fs::copy(root.join("package.json"), newer.join("package.json")).unwrap();
    std::fs::write(install.join("current-version"), "1.1.0").unwrap();
    assert_eq!(check_unchanged(&installed), Err(MaintenanceError::Changed));
}

#[tokio::test]
async fn cursor_never_probes_an_unrelated_agent_executable() {
    let machine = Machine::with_tools(&["agent"]);
    machine.version("agent", "Cursor 2026.10.22-27dcf5f");
    let detector = machine.detector(machine.env(&[]));
    assert!(matches!(
        detector.detect(&spec("cursor")).await,
        Availability::Missing { .. }
    ));
    assert!(machine.runs("agent").is_empty());
}

#[tokio::test]
async fn vendor_acp_recipes_probe_only_versions_and_keep_transport_arguments() {
    for (id, program, version, args) in [
        ("cursor", "cursor-agent", "2026.10.22-27dcf5f", vec!["acp"]),
        ("omp", "omp", "18.12.6", vec!["acp"]),
        ("grok", "grok", "1.0.70", vec!["agent", "stdio"]),
    ] {
        let machine = Machine::with_tools(&[program]);
        machine.version(program, version);
        let detector = machine.detector(machine.env(&[]));
        let agent = spec(id);
        assert!(
            matches!(detector.detect(&agent).await, Availability::Ready { version:Some(version), .. } if version.contains("unverified"))
        );
        let launch = detector.prepare_launch(&agent, progress()).await.unwrap();
        assert_eq!(launch.program, machine.tool(program));
        assert_eq!(launch.args, args);
        assert!(launch.set_env.is_empty());
        // Catalog aliases may independently probe the same binary in the background.
        // Every observed invocation must still be a read-only version check.
        let runs = machine.runs(program);
        assert!(!runs.is_empty(), "{id} was not checked");
        for run in runs {
            assert_eq!(run["argv"], serde_json::json!(["--version"]), "{id}");
        }
    }
}

#[tokio::test]
async fn opencode_major_lines_and_omp_form_baseline_are_qualified_separately() {
    for (version, accepted, rejected) in [
        ("1.19.4", "opencode", "opencode-v2"),
        ("2.1.4", "opencode-v2", "opencode"),
    ] {
        let machine = Machine::with_tools(&["opencode"]);
        machine.version("opencode", version);
        let detector = machine.detector(machine.env(&[]));
        assert!(matches!(
            detector.detect(&spec(accepted)).await,
            Availability::Ready { .. }
        ));
        assert!(matches!(
            detector.detect(&spec(rejected)).await,
            Availability::Broken { .. }
        ));
    }
    let machine = Machine::with_tools(&["omp"]);
    machine.version("omp", "18.12.5");
    assert!(matches!(
        machine
            .detector(machine.env(&[]))
            .detect(&spec("omp"))
            .await,
        Availability::Broken { .. }
    ));
}

#[tokio::test]
async fn detect_missing_binary() {
    let machine = Machine::with_tools(&["node", "npm"]);
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("hermes")).await,
        Availability::Missing {
            reason: "hermes was not found on your PATH.".into()
        }
    );
    assert!(matches!(
        detector.prepare_launch(&spec("hermes"), progress()).await,
        Err(LaunchError::Missing { .. })
    ));
}

#[tokio::test]
async fn detect_broken_binary() {
    let machine = Machine::new();
    machine.mode("hermes", "fail");
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("hermes")).await,
        Availability::Broken {
            reason: "`hermes --version` failed: ImportError: No module named 'hermes_cli'".into()
        },
        "the first stderr line is quoted"
    );
    assert!(matches!(
        detector.prepare_launch(&spec("hermes"), progress()).await,
        Err(LaunchError::Broken { .. })
    ));
    assert!(
        detector
            .cached()
            .iter()
            .find(|entry| entry.id == "hermes")
            .unwrap()
            .installation
            .is_none()
    );
}

/// Real time: the probe limit is 10 s, and a paused clock would fire it before the probe started.
#[tokio::test]
async fn probe_timeout_kills_tree() {
    let machine = Machine::new();
    machine.mode("hermes", "hang");
    let detector = machine.detector(machine.env(&[]));
    let started = tokio::time::Instant::now();
    let availability = detector.detect(&spec("hermes")).await;
    assert_eq!(
        availability,
        Availability::Broken {
            reason: "`hermes --version` did not answer within 10 seconds.".into()
        }
    );
    assert!(started.elapsed() >= Duration::from_secs(10));
    let pids = machine.pids("hermes").await;
    let (_reaper, held) = Reaper::hold(&pids);
    for process in &held {
        assert!(
            gone(process).await,
            "{process:?} outlived the probe's timeout"
        );
    }
}

#[tokio::test]
async fn a_ready_binary_shows_its_version_and_launches_from_the_chat_folder() {
    let machine = Machine::new();
    let detector = machine.detector(machine.env(&[]));
    let hermes = spec("hermes");
    assert_eq!(
        detector.detect(&hermes).await,
        Availability::Ready {
            version: Some("Hermes 0.22.5".into()),
            path: machine.tool("hermes").display().to_string(),
        }
    );
    let launch = detector
        .prepare_launch(&hermes, progress())
        .await
        .expect("launch");
    assert_eq!(launch.label, "hermes acp");
    assert_eq!(launch.program, machine.tool("hermes"));
    assert_eq!(launch.args, ["acp"]);
    assert_eq!(launch.cwd, ProcessCwd::ChatFolder);
    assert_eq!(launch.daemons, hermes.daemons);
    assert_eq!(launch.startup_timeout, Duration::from_secs(60));
    assert!(launch.set_env.is_empty());
    // The probe ran in the home folder.
    let runs = machine.runs("hermes");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["argv"], serde_json::json!(["--version"]));
    assert_eq!(
        std::fs::canonicalize(runs[0]["cwd"].as_str().expect("cwd")).expect("canonical"),
        std::fs::canonicalize(&machine.home).expect("canonical")
    );
}

#[tokio::test]
async fn native_claude_launches_the_user_binary_without_node_or_npm() {
    let machine = Machine::with_tools(&["claude"]);
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude");
    assert_eq!(
        detector.detect(&claude).await,
        Availability::Ready {
            version: Some("Claude Code 2.1.310".into()),
            path: machine.tool("claude").display().to_string(),
        }
    );
    let launch = detector
        .prepare_launch(&claude, progress())
        .await
        .expect("launch");
    let entry = detector
        .cached()
        .into_iter()
        .find(|entry| entry.id == "claude")
        .unwrap();
    let installation = entry.installation.expect("successful probe metadata");
    assert_eq!(
        installation.launch_path,
        machine.tool("claude").display().to_string()
    );
    assert_eq!(installation.version, "2.1.310");
    assert_eq!(installation.host, ghost_protocol::agents::AgentHost::Native);
    assert_eq!(
        installation.channel,
        ghost_protocol::agents::InstallChannel::Unknown
    );
    assert!(installation.resolved_path.is_some());
    assert_eq!(launch.program, machine.tool("claude"));
    assert_eq!(launch.cwd, ProcessCwd::ChatFolder);
    assert_eq!(
        launch.args,
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]
    );
    assert_eq!(launch.daemons, claude.daemons);
    assert_eq!(launch.startup_timeout, Duration::from_secs(60));
    assert!(
        launch.set_env.is_empty(),
        "session state and permission settings belong to the driver"
    );
    assert!(!machine.adapters.exists());
    assert!(machine.runs("node").is_empty());
    assert!(machine.runs("npm").is_empty());
    let runs = machine.runs("claude");
    assert_eq!(runs.len(), 1, "a version probe only");
    assert_eq!(runs[0]["argv"], serde_json::json!(["--version"]));
    assert_eq!(
        std::fs::canonicalize(runs[0]["cwd"].as_str().expect("cwd")).expect("canonical"),
        std::fs::canonicalize(&machine.home).expect("canonical")
    );
    assert!(
        !detector
            .cached()
            .iter()
            .find(|entry| entry.id == "claude")
            .expect("entry")
            .needs_download
    );
}

#[tokio::test]
async fn native_claude_requires_the_verified_baseline_and_preserves_acp_compatibility() {
    for version in ["2.1.296", "2.1.309", "unknown"] {
        let machine = Machine::new();
        machine.version("claude", version);
        let detector = machine.detector(machine.env(&[]));
        let claude = spec("claude");
        assert_eq!(
            detector.detect(&claude).await,
            Availability::Broken {
                reason: format!(
                    "Ghost needs Claude Code 2.1.310 or newer (found {}).",
                    if version == "unknown" {
                        "an unknown version"
                    } else {
                        version
                    }
                ),
            }
        );
        assert!(matches!(
            detector.prepare_launch(&claude, progress()).await,
            Err(LaunchError::Broken { .. })
        ));
        if version == "2.1.309" {
            assert!(matches!(
                detector.detect(&spec("claude-acp")).await,
                Availability::Ready { .. }
            ));
        }
    }
}

#[tokio::test]
async fn native_claude_uses_the_explicit_executable_without_an_adapter() {
    let machine = Machine::with_tools(&["claude"]);
    let own = machine.install_tool(&machine.root.path().join("my Claude (native)"), "claude");
    let detector =
        machine.detector(machine.env(&[("CLAUDE_CODE_EXECUTABLE", own.to_str().expect("utf-8"))]));
    let claude = spec("claude");
    assert_eq!(ready_path(&detector.detect(&claude).await), own);
    assert_eq!(
        detector
            .prepare_launch(&claude, progress())
            .await
            .expect("launch")
            .program,
        own
    );
    assert_eq!(machine.runs("claude").len(), 1);
}

#[cfg(windows)]
#[tokio::test]
async fn native_claude_refuses_shell_shims_without_running_them() {
    let machine = Machine::with_tools(&[]);
    std::fs::write(machine.bin.join("claude.cmd"), b"@echo 2.1.310\r\n").expect("shim");
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude");
    assert_eq!(
        detector.detect(&claude).await,
        Availability::Missing {
            reason: "Ghost can't run the claude.cmd shell shim as native Claude Code on Windows. Install the standalone Claude Code, or set CLAUDE_CODE_EXECUTABLE to a claude.exe.".into(),
        }
    );
    assert!(matches!(
        detector.prepare_launch(&claude, progress()).await,
        Err(LaunchError::Missing { .. })
    ));
    assert!(machine.runs("claude").is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn native_claude_keeps_the_stable_executable_symlink() {
    let machine = Machine::with_tools(&[]);
    let versioned = machine.install_tool(&machine.root.path().join("2.1.310"), "claude");
    std::os::unix::fs::symlink(versioned, machine.tool("claude")).expect("symlink");
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude");
    assert_eq!(
        ready_path(&detector.detect(&claude).await),
        machine.tool("claude")
    );
    assert_eq!(
        detector
            .prepare_launch(&claude, progress())
            .await
            .expect("launch")
            .program,
        machine.tool("claude")
    );
}

#[tokio::test]
async fn find_program_follows_path_order() {
    let machine = Machine::with_tools(&[]);
    let first = machine.root.path().join("first");
    let second = machine.root.path().join("second");
    let shadowed = machine.install_tool(&second, "hermes");
    let winner = machine.install_tool(&first, "hermes");
    let detector = machine.detector(machine.env_with_path(&[&first, &second], &[]));
    let availability = detector.detect(&spec("hermes")).await;
    assert_eq!(ready_path(&availability), winner);
    assert_ne!(winner, shadowed);
}

#[tokio::test]
async fn adapter_recipe_checks_node() {
    let machine = Machine::with_tools(&["npm", "claude"]);
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("claude-acp")).await,
        Availability::Missing {
            reason: "Node.js 22 or newer is required (found none).".into()
        }
    );

    let machine = Machine::new();
    machine.version("node", "v20.11.1");
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("codex-acp")).await,
        Availability::Missing {
            reason: "Node.js 22 or newer is required (found 20.11.1).".into()
        }
    );

    let machine = Machine::new();
    machine.version("node", "v22.0.0");
    let detector = machine.detector(machine.env(&[]));
    assert!(matches!(
        detector.detect(&spec("codex-acp")).await,
        Availability::Ready { .. }
    ));
}

#[tokio::test]
async fn adapter_requires_user_binary() {
    let machine = Machine::with_tools(&["node", "npm"]);
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("claude-acp")).await,
        Availability::Missing {
            reason: "The Claude adapter drives your own Claude Code, and claude was not found on your PATH.".into()
        }
    );
    assert_eq!(
        detector.detect(&spec("codex-acp")).await,
        Availability::Missing {
            reason:
                "The Codex adapter drives your own Codex, and codex was not found on your PATH."
                    .into()
        }
    );
}

#[tokio::test]
async fn an_adapter_ready_names_the_binary_that_will_run() {
    let machine = Machine::new();
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude-acp");
    assert_eq!(
        detector.detect(&claude).await,
        Availability::Ready {
            version: Some("Claude Code 2.1.310 via claude-agent-acp 0.95.0".into()),
            path: machine.tool("claude").display().to_string(),
        }
    );
    let entries = detector.detect_all(false).await;
    // The adapter is Claude Code's second connection, not an agent of its own.
    assert!(!entries.iter().any(|entry| entry.id == "claude-acp"));
    let entry = entries
        .iter()
        .find(|entry| entry.id == "claude")
        .and_then(|entry| entry.connections.as_ref())
        .and_then(|connections| {
            connections
                .options
                .iter()
                .find(|option| option.id == "claude-acp")
        })
        .expect("the adapter connection");
    assert!(entry.needs_download, "not installed yet");
    // Detection never installs and never runs an adapter.
    assert!(machine.runs("npm").is_empty());
    assert!(!machine.adapters.exists());
    let node_runs = machine.runs("node");
    assert!(!node_runs.is_empty());
    assert!(
        node_runs
            .iter()
            .all(|run| run["argv"] == serde_json::json!(["--version"])),
        "only node --version: {node_runs:?}"
    );
}

#[tokio::test]
async fn version_floor() {
    let machine = Machine::new();
    machine.version("claude", "2.0.1 (Claude Code)");
    machine.version("codex", "codex-cli 0.150.0");
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("claude-acp")).await,
        Availability::Broken {
            reason: "Ghost needs Claude Code 2.1.309 or newer (found 2.0.1).".into()
        }
    );
    assert_eq!(
        detector.detect(&spec("codex-acp")).await,
        Availability::Broken {
            reason: "Ghost needs Codex 0.160.1 or newer (found 0.150.0).".into()
        }
    );
}

#[tokio::test]
async fn npm_is_needed_only_until_the_adapter_is_installed() {
    let machine = Machine::with_tools(&["node", "claude", "codex"]);
    let detector = machine.detector(machine.env(&[]));
    assert_eq!(
        detector.detect(&spec("claude-acp")).await,
        Availability::Missing {
            reason: "npm was not found; Ghost needs it once to download the adapter.".into()
        }
    );
}

#[tokio::test]
async fn user_launch_env_is_detected_and_forced() {
    let machine = Machine::new();
    let custom = machine.root.path().join("custom");
    let own_claude = machine.install_tool(&custom, "claude");
    let own_codex = machine.install_tool(&custom, "codex");
    let env = machine.env(&[
        (
            "CLAUDE_CODE_EXECUTABLE",
            own_claude.to_str().expect("utf-8"),
        ),
        ("CODEX_PATH", own_codex.to_str().expect("utf-8")),
    ]);
    let detector = machine.detector(env);
    for (id, own, variable) in [
        ("claude-acp", &own_claude, "CLAUDE_CODE_EXECUTABLE"),
        ("codex-acp", &own_codex, "CODEX_PATH"),
    ] {
        let agent = spec(id);
        assert_eq!(
            ready_path(&detector.detect(&agent).await),
            *own,
            "{id}: shown"
        );
        let launch = detector
            .prepare_launch(&agent, progress())
            .await
            .expect("installs and launches");
        assert_eq!(
            launch.set_env,
            [(variable.into(), own.clone().into_os_string())],
            "{id}: used, through the adapter's variable"
        );
    }
}

#[tokio::test]
async fn empty_user_binary_env_is_overridden() {
    for value in ["", "/nonexistent/claude", "relative/claude"] {
        let machine = Machine::new();
        let detector = machine.detector(machine.env(&[("CLAUDE_CODE_EXECUTABLE", value)]));
        let claude = spec("claude-acp");
        assert_eq!(
            ready_path(&detector.detect(&claude).await),
            machine.tool("claude"),
            "{value:?}"
        );
        let launch = detector
            .prepare_launch(&claude, progress())
            .await
            .expect("launch");
        assert_eq!(
            launch.set_env,
            [(
                "CLAUDE_CODE_EXECUTABLE".into(),
                machine.tool("claude").into_os_string()
            )],
            "{value:?}: overridden with the detected binary, never left for the adapter's bundled CLI"
        );
    }
}

#[tokio::test]
async fn a_refresh_probes_again() {
    let machine = Machine::new();
    let detector = machine.detector(machine.env(&[]));
    let hermes = spec("hermes");
    assert!(matches!(
        detector.detect(&hermes).await,
        Availability::Ready { version: Some(version), .. } if version == "Hermes 0.22.5"
    ));
    machine.version("hermes", "Hermes Agent v0.23.0");
    assert!(
        matches!(detector.detect(&hermes).await, Availability::Ready { version: Some(version), .. } if version == "Hermes 0.22.5"),
        "cached until a refresh"
    );
    let entries = detector.detect_all(true).await;
    let entry = entries
        .iter()
        .find(|entry| entry.id == "hermes")
        .expect("entry");
    assert!(
        matches!(&entry.availability, Availability::Ready { version: Some(version), .. } if version == "Hermes 0.23.0")
    );
}
