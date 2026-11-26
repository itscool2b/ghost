//! Spawning agents against the real OS: the cwd, the complete env, the pipes, framing, the stderr
//! drain, backpressure and exit detection. Runs on macOS locally and on Linux and Windows in CI;
//! OS-specific cases are cfg-gated. Every test kills what it spawned and checks nothing is left.

#[cfg(test)]
mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{
    GONE_WITHIN, Reaper, STARTUP, env_with, launch, next_json, read_pids, spec, testbin, wait_gone,
};
use ghost_process::{
    EnvKey, LeftoverRegistry, LogFactory, ProcessCwd, ProcessSpawner, ProtocolLog, Redactor,
    SpawnError, Spawner, StdoutItem, TopKeys,
};
use serde_json::json;
use tokio::runtime::Handle;

#[cfg(test)]
fn spawner() -> Arc<Spawner> {
    Spawner::new(LeftoverRegistry::disabled(), Handle::current())
}

/// Every record of a log file, once its exit note is there.
#[cfg(test)]
async fn log_records(path: &Path) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + STARTUP;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.contains("\"exit\"")
        {
            return text
                .lines()
                .map(|line| serde_json::from_str(line).expect("a JSON record"))
                .collect();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no exit note in {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_child_runs_in_the_chat_folder_or_its_own() {
    let chat = tempfile::tempdir().expect("tempdir");
    let own = tempfile::tempdir().expect("tempdir");
    let spawner = spawner();
    let mut reaper = Reaper::new();
    for (cwd, expected) in [
        (ProcessCwd::ChatFolder, chat.path()),
        (ProcessCwd::Fixed(own.path().to_path_buf()), own.path()),
    ] {
        let mut launch = launch(&["print-cwd"]);
        launch.cwd = cwd;
        let mut process = spawner
            .spawn(spec(launch, chat.path()), ProtocolLog::disabled())
            .await
            .expect("spawn");
        reaper.hold(process.control.pid());
        let line = next_json(&mut process.io.stdout).await;
        // Canonicalized only to compare: macOS reports /private/var for /var, and Windows CI
        // temp paths use 8.3 short names.
        let reported =
            std::fs::canonicalize(line["cwd"].as_str().expect("cwd")).expect("canonical");
        assert_eq!(
            reported,
            std::fs::canonicalize(expected).expect("canonical")
        );
        assert_eq!(process.control.exited().await.code, Some(0));
    }
    reaper.assert_gone().await;
}

#[tokio::test]
async fn the_env_is_exactly_the_resolved_env_scrubbed_plus_set() {
    let folder = tempfile::tempdir().expect("tempdir");
    let mut env = (*env_with(&[("CLAUDECODE", "1"), ("GHOST_TEST_KEEP", "kept")])).clone();
    // Gone from the resolved env, so gone from the child: the spawn clears Ghost's own env.
    env.vars.remove(&EnvKey::from("PATH"));
    let mut launch = launch(&["print-env"]);
    launch.set_env = vec![
        (OsString::from("GHOST_TEST_SET"), OsString::from("set")),
        (
            OsString::from("GHOST_TEST_KEEP"),
            OsString::from("overridden"),
        ),
    ];
    let mut spec = spec(launch, folder.path());
    spec.env = Arc::new(env);
    let mut process = spawner()
        .spawn(spec, ProtocolLog::disabled())
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let line = next_json(&mut process.io.stdout).await;
    let printed = line["env"].as_object().expect("env");
    let get = |name: &str| {
        printed
            .iter()
            .find(|(known, _)| {
                if cfg!(windows) {
                    known.eq_ignore_ascii_case(name)
                } else {
                    *known == name
                }
            })
            .map(|(_, value)| value.as_str().expect("text").to_owned())
    };
    assert_eq!(get("PATH"), None, "the child never sees Ghost's own env");
    assert_eq!(get("CLAUDECODE"), None, "scrubbed");
    assert_eq!(get("GHOST_TEST_SET").as_deref(), Some("set"));
    assert_eq!(
        get("GHOST_TEST_KEEP").as_deref(),
        Some("overridden"),
        "set wins"
    );
    process.control.exited().await;
    reaper.assert_gone().await;
}

#[tokio::test]
async fn no_env_values_in_logs() {
    let folder = tempfile::tempdir().expect("tempdir");
    let logs = tempfile::tempdir().expect("tempdir");
    let factory = LogFactory::new(
        logs.path().to_path_buf(),
        Arc::new(Redactor::standard()),
        Handle::current(),
    );
    let log = factory.open("chat", "spawn-1");
    let path = log.path().expect("logging on");
    let mut launch = launch(&["echo"]);
    launch.set_env = vec![(
        OsString::from("GHOST_TEST_SET"),
        OsString::from("set-value-that-is-never-logged"),
    )];
    let mut spec = spec(launch, folder.path());
    spec.env = env_with(&[
        ("GHOST_TEST_VALUE", "env-value-that-is-never-logged"),
        ("CLAUDECODE", "scrubbed-value-that-is-never-logged"),
    ]);
    let mut process = spawner().spawn(spec, log).await.expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    process
        .io
        .stdin
        .send(r#"{"jsonrpc":"2.0","method":"ping"}"#.into())
        .expect("stdin");
    assert_eq!(
        next_json(&mut process.io.stdout).await["method"],
        json!("ping")
    );
    process.io.stdin.close();
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"eof": true})
    );
    assert_eq!(process.control.exited().await.code, Some(0));
    drop(process);
    let records = log_records(&path).await;
    let text = std::fs::read_to_string(&path).expect("log");
    assert!(!text.contains("never-logged"), "{text}");
    let spawn = &records[0]["note"]["spawn"];
    // Names only. The test's own env may carry more session markers (a run inside Claude Code).
    let removed: Vec<&str> = spawn["removed"]
        .as_array()
        .expect("removed")
        .iter()
        .map(|name| name.as_str().expect("a name"))
        .collect();
    assert!(removed.contains(&"CLAUDECODE"), "{removed:?}");
    assert!(!removed.contains(&"GHOST_TEST_VALUE"), "{removed:?}");
    assert_eq!(spawn["set"], json!(["GHOST_TEST_SET"]));
    assert_eq!(spawn["label"], json!("ghost-testbin echo"));
    assert!(spawn["pid"].as_u64().is_some());
    // The exit note and the last stdout line come from two tasks, in either order.
    let mut directions: Vec<&str> = records
        .iter()
        .skip(1)
        .map(|record| record["d"].as_str().expect("d"))
        .collect();
    directions.sort_unstable();
    assert_eq!(directions, ["in", "in", "note", "out"], "{records:?}");
    assert!(
        records
            .iter()
            .any(|record| record["note"]["exit"]["code"] == json!(0))
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn noise_lines_are_dropped() {
    let folder = tempfile::tempdir().expect("tempdir");
    let logs = tempfile::tempdir().expect("tempdir");
    let factory = LogFactory::new(
        logs.path().to_path_buf(),
        Arc::new(Redactor::standard()),
        Handle::current(),
    );
    let log = factory.open("chat", "noise");
    let path = log.path().expect("on");
    let mut process = spawner()
        .spawn(spec(launch(&["noise"]), folder.path()), log)
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    assert_eq!(
        next_json(&mut process.io.stdout).await["method"],
        json!("first")
    );
    assert_eq!(
        next_json(&mut process.io.stdout).await["method"],
        json!("second")
    );
    assert_eq!(
        process.io.stdout.next().await,
        None,
        "nothing else reaches the driver"
    );
    process.control.exited().await;
    drop(process);
    let records = log_records(&path).await;
    let noise: Vec<u64> = records
        .iter()
        .filter_map(|record| record["note"]["noise"]["bytes"].as_u64())
        .collect();
    assert_eq!(noise, [46, 38], "the two noise lines, by size only");
    let ins = records
        .iter()
        .filter(|record| record["d"] == json!("in"))
        .count();
    assert_eq!(ins, 2);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn framer_oversized_line_from_a_real_process() {
    let folder = tempfile::tempdir().expect("tempdir");
    let spawner = spawner();
    let mut reaper = Reaper::new();
    for (kind, id, method) in [
        (
            "request",
            Some(json!(7)),
            Some("session/request_permission"),
        ),
        ("response", Some(json!(7)), None),
        ("notification", None, Some("session/update")),
    ] {
        let mut spec = spec(launch(&["oversized", "70000", kind]), folder.path());
        spec.max_line_bytes = 64 * 1024;
        let mut process = spawner
            .spawn(spec, ProtocolLog::disabled())
            .await
            .expect("spawn");
        reaper.hold(process.control.pid());
        let item = tokio::time::timeout(STARTUP, process.io.stdout.next())
            .await
            .expect("in time");
        let Some(StdoutItem::Oversized { bytes, keys }) = item else {
            panic!("an oversized item: {item:?}");
        };
        assert!(bytes > 70_000 && bytes < 70_200, "{bytes}");
        assert_eq!(
            keys,
            TopKeys {
                id,
                method: method.map(str::to_owned),
                kind: None
            },
            "{kind}"
        );
        assert_eq!(
            next_json(&mut process.io.stdout).await,
            json!({"after": true}),
            "the next line is intact"
        );
        assert_eq!(process.io.stdout.next().await, None);
        process.control.exited().await;
    }
    reaper.assert_gone().await;
}

#[tokio::test]
async fn stderr_flood_never_blocks_stdout() {
    let folder = tempfile::tempdir().expect("tempdir");
    let mut process = spawner()
        .spawn(
            spec(launch(&["flood-stderr", "2000000"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    // Nothing reads stderr here but the drain: a stdout-only reader still gets its line.
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"done": true})
    );
    assert_eq!(process.control.exited().await.code, Some(0));
    let tail = process.control.stderr_tail(usize::MAX);
    assert!(
        tail.len() <= ghost_process::STDERR_TAIL_BYTES && tail.len() > 16_000,
        "{}",
        tail.len()
    );
    assert!(tail.trim_end().ends_with('e'));
    assert!(process.control.stderr_tail(100).len() <= 100);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn slow_consumer_blocks_agent_not_memory() {
    let folder = tempfile::tempdir().expect("tempdir");
    // 2,000 lines of 4 KiB: 8 MB, far more than the channel and the pipe hold.
    let mut process = spawner()
        .spawn(
            spec(launch(&["json-lines", "2000", "4096"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        process.control.try_exited(),
        None,
        "nobody read, so the agent blocks on its own write"
    );
    for index in 0..2000u64 {
        let line = next_json(&mut process.io.stdout).await;
        assert_eq!(line["params"]["i"], json!(index));
    }
    assert_eq!(process.io.stdout.next().await, None);
    assert_eq!(process.control.exited().await.code, Some(0));
    reaper.assert_gone().await;
}

#[tokio::test]
async fn exit_with_buffered_output_is_read_to_the_end() {
    let folder = tempfile::tempdir().expect("tempdir");
    let mut process = spawner()
        .spawn(
            spec(launch(&["exit-with-buffered", "500"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    assert_eq!(process.control.exited().await.code, Some(0));
    for index in 0..500u64 {
        assert_eq!(
            next_json(&mut process.io.stdout).await["params"]["i"],
            json!(index)
        );
    }
    assert_eq!(process.io.stdout.next().await, None);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn exit_detection_uses_direct_child() {
    let folder = tempfile::tempdir().expect("tempdir");
    let pidfile = folder.path().join("child.pid");
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let process = spawner()
        .spawn(
            spec(
                launch(&["spawn-group-child", &pidfile_arg, "exit"]),
                folder.path(),
            ),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let grandchild = read_pids(&pidfile).await[0];
    reaper.hold(grandchild);
    // The agent exits while its descendant lingers in its group (Unix) or its job (Windows).
    let exit = tokio::time::timeout(STARTUP, process.control.exited())
        .await
        .expect("the direct child's exit is seen at once");
    assert_eq!(exit.code, Some(0));
    assert!(common::alive(grandchild), "the descendant lingers");
    process.control.kill_tree();
    assert!(
        wait_gone(grandchild, GONE_WITHIN).await,
        "kill_tree reaches it"
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn stdin_is_piped_and_close_sends_end_of_file() {
    let folder = tempfile::tempdir().expect("tempdir");
    let mut process = spawner()
        .spawn(
            spec(launch(&["echo"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    for index in 0..3 {
        process
            .io
            .stdin
            .send(format!(r#"{{"hello":{index}}}"#))
            .expect("open");
    }
    for index in 0..3 {
        assert_eq!(
            next_json(&mut process.io.stdout).await,
            json!({"hello": index})
        );
    }
    process.control.close_stdin();
    assert!(process.io.stdin.send("{}".into()).is_err(), "closed");
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"eof": true})
    );
    assert_eq!(process.control.exited().await.code, Some(0));
    reaper.assert_gone().await;
}

#[tokio::test]
async fn spawn_path_with_spaces() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("dir with spaces").join("ünïcödé");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let program = bin.join(testbin().file_name().expect("name"));
    std::fs::copy(testbin(), &program).expect("copy");
    let folder = dir.path().join("a chat folder");
    std::fs::create_dir(&folder).expect("mkdir");
    let mut launch = launch(&["print-cwd"]);
    launch.program = program;
    let mut process = spawner()
        .spawn(spec(launch, &folder), ProtocolLog::disabled())
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let line = next_json(&mut process.io.stdout).await;
    let reported = std::fs::canonicalize(line["cwd"].as_str().expect("cwd")).expect("canonical");
    assert_eq!(reported, std::fs::canonicalize(&folder).expect("canonical"));
    process.control.exited().await;
    reaper.assert_gone().await;
}

#[tokio::test]
async fn spawn_failures_say_what_is_wrong() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawner = spawner();
    let attempt = |launch, folder: PathBuf| {
        let spawner = Arc::clone(&spawner);
        async move {
            spawner
                .spawn(spec(launch, &folder), ProtocolLog::disabled())
                .await
                .expect_err("refused")
        }
    };

    let mut missing = launch(&["echo"]);
    missing.program = dir.path().join("aura-spike-no-such-program");
    assert!(matches!(
        attempt(missing, dir.path().to_path_buf()).await,
        SpawnError::ProgramNotFound { .. }
    ));
    let mut relative = launch(&["echo"]);
    relative.program = PathBuf::from("ghost-testbin");
    assert!(matches!(
        attempt(relative, dir.path().to_path_buf()).await,
        SpawnError::ProgramNotFound { .. }
    ));
    let mut a_folder = launch(&["echo"]);
    a_folder.program = dir.path().to_path_buf();
    assert!(matches!(
        attempt(a_folder, dir.path().to_path_buf()).await,
        SpawnError::ProgramNotFound { .. }
    ));

    // The cwd is checked first: Unix reports a missing cwd as a missing program.
    let gone = dir.path().join("deleted-project");
    let error = attempt(launch(&["echo"]), gone.clone()).await;
    assert!(
        matches!(&error, SpawnError::InvalidCwd { path, reason } if *path == gone && reason == "it does not exist"),
        "{error:?}"
    );
    let file = dir.path().join("a-file");
    std::fs::write(&file, b"x").expect("write");
    let error = attempt(launch(&["echo"]), file).await;
    assert!(
        matches!(&error, SpawnError::InvalidCwd { reason, .. } if reason == "it is not a folder"),
        "{error:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let not_executable = dir.path().join("not-executable");
        std::fs::write(&not_executable, b"#!/bin/sh\n").expect("write");
        let mut launch_it = launch(&["echo"]);
        launch_it.program = not_executable;
        assert!(matches!(
            attempt(launch_it, dir.path().to_path_buf()).await,
            SpawnError::PermissionDenied { .. }
        ));
        // A folder the OS refuses (macOS privacy answers EPERM; here, no read permission).
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).expect("mkdir");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let error = attempt(launch(&["echo"]), locked.clone()).await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        if !nix::unistd::geteuid().is_root() {
            assert!(
                matches!(error, SpawnError::CwdNotAllowed { .. }),
                "{error:?}"
            );
        }
    }
}
