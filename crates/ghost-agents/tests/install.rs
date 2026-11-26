//! The one-time adapter install against a fake npm and node: only complete installs are marked,
//! nothing runs in the chat folder, an interrupted download is redone, two starts share one
//! download, and a forgotten install downloads again.

#[cfg(test)]
mod common;

use std::sync::Arc;

use common::{Machine, Reaper, gone, spec};
use ghost_agents::{Detector, LaunchError};
use ghost_process::{
    LeftoverRegistry, ProcessCwd, ProcessSpawner, ProtocolLog, SpawnSpec, Spawner, StdoutItem,
};
use serde_json::json;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

const CLAUDE_FOLDER: &str = "agentclientprotocol-claude-agent-acp-0.95.0";

const DOWNLOADING: &str = "Downloading the Claude adapter (@agentclientprotocol/claude-agent-acp 0.95.0). This happens once and can take a few minutes.";

#[cfg(test)]
fn committed(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("adapters")
        .join(CLAUDE_FOLDER)
        .join(name);
    std::fs::read_to_string(path).expect("committed file")
}

/// Whether the connection `id` still needs its download, as its agent's entry lists it.
#[cfg(test)]
fn needs_download(detector: &Detector, id: &str) -> bool {
    let entry = detector
        .cached()
        .into_iter()
        .find(|entry| entry.id == ghost_agents::agent_of(id))
        .expect("entry");
    match entry.connections {
        Some(connections) => {
            connections
                .options
                .into_iter()
                .find(|option| option.id == id)
                .expect("connection")
                .needs_download
        }
        None => entry.needs_download,
    }
}

#[tokio::test]
async fn install_marks_only_complete_installs() {
    let machine = Machine::new();
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude-acp");
    let folder = machine.adapter_folder(CLAUDE_FOLDER);
    detector.detect(&claude).await;
    assert!(needs_download(&detector, "claude-acp"));

    for (mode, reason) in [
        (
            "fail",
            "Could not download the Claude adapter: npm error code E404",
        ),
        (
            "partial",
            "Could not download the Claude adapter: @agentclientprotocol/claude-agent-acp's program file is missing.",
        ),
        (
            "broken-entry",
            "Could not download the Claude adapter: the adapter did not start (Error: Cannot find module",
        ),
    ] {
        machine.mode("npm", mode);
        let (details, mut received) = mpsc::unbounded_channel();
        let result = detector.prepare_launch(&claude, details).await;
        let Err(LaunchError::InstallFailed { reason: got }) = result else {
            panic!("{mode}: an install failure: {result:?}");
        };
        assert!(got.starts_with(reason), "{mode}: {got}");
        assert_eq!(received.try_recv().as_deref(), Ok(DOWNLOADING), "{mode}");
        assert!(
            !folder.join("installed.json").exists(),
            "{mode}: nothing marked"
        );
        assert!(needs_download(&detector, "claude-acp"), "{mode}");
    }

    machine.mode("npm", "ok");
    let (details, mut received) = mpsc::unbounded_channel();
    let launch = detector
        .prepare_launch(&claude, details)
        .await
        .expect("installs");
    assert_eq!(received.try_recv().as_deref(), Ok(DOWNLOADING));
    assert!(!needs_download(&detector, "claude-acp"));

    // npm ran exactly as the spec says, in the adapter folder, from the committed files.
    let runs = machine.runs("npm");
    assert_eq!(runs.len(), 4);
    let last = runs.last().expect("a run");
    assert_eq!(
        last["argv"],
        json!([
            "ci",
            "--omit=optional",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund"
        ])
    );
    assert_eq!(last["update_notifier"], json!("false"));
    assert_eq!(
        std::fs::read_to_string(folder.join("package.json")).expect("read"),
        committed("package.json")
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("package-lock.json")).expect("read"),
        committed("package-lock.json")
    );
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join("installed.json")).expect("marker"))
            .expect("json");
    assert_eq!(
        marker["package"],
        json!("@agentclientprotocol/claude-agent-acp")
    );
    assert_eq!(marker["version"], json!("0.95.0"));
    assert_eq!(
        marker["entry"],
        json!("node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js")
    );
    assert!(marker["installed_at_ms"].as_i64().is_some_and(|ms| ms > 0));

    // The launch: node <absolute entry>, from the adapter folder, the user's binary forced.
    let entry = folder
        .join("node_modules")
        .join("@agentclientprotocol")
        .join("claude-agent-acp")
        .join("dist")
        .join("index.js");
    assert_eq!(launch.label, "claude-agent-acp");
    assert_eq!(launch.program, machine.tool("node"));
    assert_eq!(launch.args.len(), 1);
    assert_eq!(std::path::PathBuf::from(&launch.args[0]), entry);
    assert_eq!(launch.cwd, ProcessCwd::Fixed(folder.clone()));
    assert_eq!(
        launch.set_env,
        [(
            "CLAUDE_CODE_EXECUTABLE".into(),
            machine.tool("claude").into_os_string()
        )]
    );
    assert_eq!(launch.daemons, claude.daemons);

    // A second start finds the install and downloads nothing.
    let (details, mut received) = mpsc::unbounded_channel();
    assert_eq!(detector.prepare_launch(&claude, details).await, Ok(launch));
    assert!(received.try_recv().is_err());
    assert_eq!(machine.runs("npm").len(), 4);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&folder)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "the adapter folder is private");
    }
}

#[tokio::test]
async fn adapter_never_runs_in_chat_folder() {
    let machine = Machine::new();
    // A cloned repository that would redirect the adapter and ship its own copy of it.
    let chat = machine.root.path().join("hostile-repo");
    let planted = chat.join("node_modules/@agentclientprotocol/claude-agent-acp");
    std::fs::create_dir_all(planted.join("dist")).expect("mkdir");
    std::fs::write(
        chat.join(".npmrc"),
        "@agentclientprotocol:registry=http://127.0.0.1:9\n",
    )
    .expect("npmrc");
    std::fs::write(
        planted.join("package.json"),
        json!({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.95.0",
               "bin": {"claude-agent-acp": "dist/index.js"}})
        .to_string(),
    )
    .expect("write");
    std::fs::write(planted.join("dist/index.js"), "console.log('HOSTILE')\n").expect("write");

    let env = machine.env(&[]);
    let detector = machine.detector(env.clone());
    let launch = detector
        .prepare_launch(&spec("claude-acp"), mpsc::unbounded_channel().0)
        .await
        .expect("installs");
    let folder = machine.adapter_folder(CLAUDE_FOLDER);
    let canonical = |path: &str| std::fs::canonicalize(path).expect("canonical");
    for run in machine
        .runs("npm")
        .iter()
        .chain(machine.runs("node").iter())
    {
        let cwd = canonical(run["cwd"].as_str().expect("cwd"));
        assert_ne!(
            cwd,
            std::fs::canonicalize(&chat).expect("canonical"),
            "{run}"
        );
    }
    let npm = machine.runs("npm");
    assert_eq!(
        canonical(npm[0]["cwd"].as_str().expect("cwd")),
        std::fs::canonicalize(&folder).expect("canonical"),
        "the install runs in the adapter folder"
    );
    assert_eq!(launch.cwd, ProcessCwd::Fixed(folder.clone()));
    assert!(std::path::Path::new(&launch.args[0]).starts_with(&machine.adapters));

    // Spawned for a chat in the hostile folder, the adapter still runs in its own folder.
    let spawner = Spawner::new(LeftoverRegistry::disabled(), Handle::current());
    let mut process = spawner
        .spawn(
            SpawnSpec {
                launch,
                folder: chat.clone(),
                env: Arc::new(env),
                max_line_bytes: ghost_process::MAX_LINE_BYTES,
            },
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let (_reaper, _) = Reaper::hold(&[process.control.pid()]);
    let Some(StdoutItem::Line(line)) = process.io.stdout.next().await else {
        panic!("the fake adapter reports where it runs");
    };
    let reported: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(
        canonical(reported["cwd"].as_str().expect("cwd")),
        std::fs::canonicalize(&folder).expect("canonical")
    );
    assert!(!line.contains("HOSTILE"));
    assert_eq!(process.control.exited().await.code, Some(0));
}

#[tokio::test]
async fn interrupted_install_is_redone() {
    let machine = Machine::new();
    machine.mode("npm", "hang");
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude-acp");
    let preparing = tokio::spawn({
        let detector = Arc::clone(&detector);
        let claude = claude.clone();
        async move {
            detector
                .prepare_launch(&claude, mpsc::unbounded_channel().0)
                .await
        }
    });
    let pids = machine.pids("npm").await;
    let (_reaper, held) = Reaper::hold(&pids);
    let folder = machine.adapter_folder(CLAUDE_FOLDER);
    let partial = folder.join("node_modules/@agentclientprotocol/claude-agent-acp/package.json");
    assert!(partial.exists(), "npm got part of the way");
    // Esc, force stop, Stop or quit during the download: the preparation is dropped.
    preparing.abort();
    assert!(preparing.await.expect_err("aborted").is_cancelled());
    for process in &held {
        assert!(
            gone(process).await,
            "npm ({process:?}) was killed with the dropped preparation"
        );
    }
    assert!(!folder.join("installed.json").exists(), "nothing marked");
    assert!(needs_download(&detector, "claude-acp"));

    machine.mode("npm", "ok");
    let launch = detector
        .prepare_launch(&claude, mpsc::unbounded_channel().0)
        .await
        .expect("the next start installs again");
    assert_eq!(machine.runs("npm").len(), 2);
    assert!(folder.join("installed.json").exists());
    assert!(std::path::Path::new(&launch.args[0]).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_installs_share_one_attempt() {
    let machine = Machine::new();
    machine.mode("npm", "slow");
    let detector = machine.detector(machine.env(&[]));
    let claude = spec("claude-acp");
    let start = |detector: Arc<Detector>| {
        let claude = claude.clone();
        tokio::spawn(async move {
            let (details, mut received) = mpsc::unbounded_channel();
            let launch = detector.prepare_launch(&claude, details).await;
            (launch, received.try_recv().ok())
        })
    };
    let (first, second) = (start(Arc::clone(&detector)), start(Arc::clone(&detector)));
    let (first, second) = (first.await.expect("join"), second.await.expect("join"));
    let launch = first.0.expect("installed");
    assert_eq!(second.0, Ok(launch), "both wait for the same install");
    assert_eq!(first.1.as_deref(), Some(DOWNLOADING));
    assert_eq!(second.1.as_deref(), Some(DOWNLOADING));
    assert_eq!(machine.runs("npm").len(), 1, "one install");
}

#[tokio::test]
async fn a_forgotten_install_downloads_again() {
    let machine = Machine::new();
    let detector = machine.detector(machine.env(&[]));
    let codex = spec("codex-acp");
    detector
        .prepare_launch(&codex, mpsc::unbounded_channel().0)
        .await
        .expect("installs");
    assert!(!needs_download(&detector, "codex-acp"));
    detector.forget_install(&codex);
    assert_eq!(detector.forgotten(), ["codex-acp"]);
    assert!(needs_download(&detector, "codex-acp"));
    assert!(
        !machine
            .adapter_folder("agentclientprotocol-codex-acp-2.2.2")
            .join("installed.json")
            .exists()
    );
    let (details, mut received) = mpsc::unbounded_channel();
    detector
        .prepare_launch(&codex, details)
        .await
        .expect("installs again");
    assert_eq!(
        received.try_recv().as_deref(),
        Ok(
            "Downloading the Codex adapter (@agentclientprotocol/codex-acp 2.2.2). This happens once and can take a few minutes."
        )
    );
    assert_eq!(machine.runs("npm").len(), 2);
}

#[tokio::test]
async fn a_start_of_an_unavailable_adapter_never_installs() {
    let machine = Machine::with_tools(&["node", "npm"]);
    let detector = machine.detector(machine.env(&[]));
    let result = detector
        .prepare_launch(&spec("claude-acp"), mpsc::unbounded_channel().0)
        .await;
    assert!(
        matches!(result, Err(LaunchError::Missing { .. })),
        "{result:?}"
    );
    assert!(machine.runs("npm").is_empty());
}
