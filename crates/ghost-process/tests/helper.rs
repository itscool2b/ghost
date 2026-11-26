//! Short-lived helpers against the real OS: output capture and its cap, timeouts that kill the
//! whole tree, a dropped future that kills it too, and nothing left behind after a normal exit.

#[cfg(test)]
mod common;

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use common::{GONE_WITHIN, Reaper, env, read_pids, testbin, wait_gone};
use ghost_process::{
    HelperEnv, HelperSpec, LeftoverRegistry, SpawnError, find_program, process_env, run_helper,
    run_helper_input,
};
use tokio::runtime::Handle;

#[cfg(test)]
fn helper(args: &[&str], cwd: &Path) -> HelperSpec {
    HelperSpec {
        program: testbin(),
        args: args.iter().map(OsString::from).collect(),
        cwd: cwd.to_path_buf(),
        env: HelperEnv::Resolved(env()),
        set: Vec::new(),
        new_session: false,
        timeout: Duration::from_secs(30),
        max_stdout: 64 * 1024,
    }
}

#[tokio::test]
async fn captures_git_version_from_stdout() {
    let folder = tempfile::tempdir().expect("tempdir");
    let Some(git) = find_program("git", &process_env()) else {
        // The spike ran real git; a machine without it still runs the rest of the suite.
        eprintln!("git is not on PATH; skipping");
        return;
    };
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            program: git,
            args: vec!["--version".into()],
            env: HelperEnv::ProcessEnv,
            ..helper(&[], folder.path())
        },
    )
    .await
    .expect("git runs");
    assert_eq!(
        output.exit.and_then(|exit| exit.code),
        Some(0),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("git version "),
        "{output:?}"
    );
    assert!(!output.timed_out && !output.stdout_truncated);
}

#[tokio::test]
async fn helper_input_is_complete_before_eof_without_argv_or_files() {
    let folder = tempfile::tempdir().expect("tempdir");
    let input = "reviewed multiline body\n".repeat(20_000);
    let output = run_helper_input(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            max_stdout: 1024 * 1024,
            ..helper(&["echo"], folder.path())
        },
        input.as_bytes().to_vec(),
    )
    .await
    .expect("input helper");
    assert_eq!(output.exit.and_then(|exit| exit.code), Some(0));
    assert_eq!(
        output.stdout,
        format!("{input}{{\"eof\":true}}\n").as_bytes()
    );
    assert!(!output.timed_out);
    assert_eq!(std::fs::read_dir(folder.path()).expect("files").count(), 0);
}

#[tokio::test]
async fn blocked_helper_input_times_out_and_oversized_input_is_refused() {
    let folder = tempfile::tempdir().expect("tempdir");
    let output = run_helper_input(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            timeout: Duration::from_millis(100),
            ..helper(&["hang"], folder.path())
        },
        vec![b'x'; 1024 * 1024],
    )
    .await
    .expect("bounded timeout");
    assert!(output.timed_out);
    let error = run_helper_input(
        &LeftoverRegistry::disabled(),
        helper(&["echo"], folder.path()),
        vec![0; 1024 * 1024 + 1],
    )
    .await
    .expect_err("limit");
    assert!(
        matches!(error, SpawnError::Spawn { source, .. } if source.kind() == std::io::ErrorKind::InvalidInput)
    );
}

/// Linux refuses to run a file someone holds open for writing ("text file busy"), as a freshly
/// written binary is while another thread's fork still holds it: the start waits for it to clear.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_binary_busy_for_a_moment_still_starts() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let folder = tempfile::tempdir().expect("tempdir");
    let tool = folder.path().join("tool");
    let mut file = std::fs::File::create(&tool).expect("create");
    file.write_all(b"#!/bin/sh\necho ready\n").expect("write");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Still open for writing, so exec says busy until this thread closes it.
    let closer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(15));
        drop(file);
    });
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            program: tool,
            env: HelperEnv::ProcessEnv,
            ..helper(&[], folder.path())
        },
    )
    .await
    .expect("starts once the file is closed");
    closer.join().expect("the closer ends");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ready");
}

#[tokio::test]
async fn a_helper_runs_where_it_is_told_with_what_it_is_given() {
    let folder = tempfile::tempdir().expect("tempdir");
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        helper(&["print-cwd"], folder.path()),
    )
    .await
    .expect("runs");
    let line: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        std::fs::canonicalize(line["cwd"].as_str().expect("cwd")).expect("canonical"),
        std::fs::canonicalize(folder.path()).expect("canonical")
    );
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            set: vec![("GHOST_TEST_SET".into(), "yes".into())],
            env: HelperEnv::Resolved(common::env_with(&[("CODEX_SANDBOX", "seatbelt")])),
            ..helper(&["print-env"], folder.path())
        },
    )
    .await
    .expect("runs");
    let line: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(line["env"]["GHOST_TEST_SET"], serde_json::json!("yes"));
    assert!(
        line["env"].get("CODEX_SANDBOX").is_none(),
        "helpers are scrubbed too"
    );
}

#[tokio::test]
async fn helper_stdout_is_capped_but_read_to_the_end() {
    let folder = tempfile::tempdir().expect("tempdir");
    // About 12 MB: if the helper were not drained it would block and time out.
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            max_stdout: 1024 * 1024,
            ..helper(&["json-lines", "3000", "4096"], folder.path())
        },
    )
    .await
    .expect("runs");
    assert_eq!(output.exit.and_then(|exit| exit.code), Some(0));
    assert_eq!(output.stdout.len(), 1024 * 1024);
    assert!(output.stdout_truncated && !output.timed_out);
}

#[tokio::test]
async fn a_failing_helper_reports_its_exit_and_stderr() {
    let folder = tempfile::tempdir().expect("tempdir");
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        helper(&["no-such-mode"], folder.path()),
    )
    .await
    .expect("runs");
    assert_eq!(output.exit.and_then(|exit| exit.code), Some(1));
    assert!(output.stderr_tail.contains("unknown mode"), "{output:?}");
}

#[tokio::test]
async fn probe_timeout_kills_tree() {
    let folder = tempfile::tempdir().expect("tempdir");
    let pidfile = folder.path().join("helper.pid");
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let registry = LeftoverRegistry::disabled();
    let mut reaper = Reaper::new();
    // Real time (a paused clock would fire the limit before the helper even starts), with a
    // short limit.
    let output = run_helper(
        &registry,
        HelperSpec {
            timeout: Duration::from_secs(2),
            ..helper(&["spawn-group-child", &pidfile_arg, "hang"], folder.path())
        },
    )
    .await
    .expect("runs");
    let pids = read_pids(&pidfile).await;
    reaper.hold_all(&pids);
    assert!(output.timed_out, "{output:?}");
    assert_eq!(output.exit, None);
    assert!(String::from_utf8_lossy(&output.stdout).contains("ready"));
    for pid in pids {
        assert!(
            wait_gone(pid, GONE_WITHIN).await,
            "{pid} outlived the timeout"
        );
    }
    reaper.assert_gone().await;
}

#[tokio::test]
async fn a_helper_leaves_nothing_behind() {
    let folder = tempfile::tempdir().expect("tempdir");
    let pidfile = folder.path().join("left.pid");
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let mut reaper = Reaper::new();
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        helper(&["spawn-group-child", &pidfile_arg, "exit"], folder.path()),
    )
    .await
    .expect("runs");
    assert_eq!(output.exit.and_then(|exit| exit.code), Some(0));
    let pids = read_pids(&pidfile).await;
    reaper.hold_all(&pids);
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "the helper's leftover was killed"
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn dropping_a_helper_kills_its_tree() {
    let folder = tempfile::tempdir().expect("tempdir");
    let run = folder.path().join("run");
    let registry = LeftoverRegistry::open(&run, Handle::current()).expect("registry");
    let pidfile = folder.path().join("dropped.pid");
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let spec = helper(&["spawn-group-child", &pidfile_arg, "hang"], folder.path());
    let task = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { run_helper(&registry, spec).await }
    });
    let pids = read_pids(&pidfile).await;
    let mut reaper = Reaper::new();
    reaper.hold_all(&pids);
    #[cfg(unix)]
    {
        // Registered for the sweep while it runs.
        let file = registry_file(&run);
        let deadline = tokio::time::Instant::now() + common::STARTUP;
        loop {
            let body: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&file).expect("read")).expect("json");
            if body["entries"][0]["label"] == serde_json::json!("helper ghost-testbin") {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "never registered: {body}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    task.abort();
    assert!(task.await.expect_err("aborted").is_cancelled());
    for pid in &pids {
        assert!(
            wait_gone(*pid, GONE_WITHIN).await,
            "{pid} outlived the dropped helper"
        );
    }
    #[cfg(unix)]
    {
        let file = registry_file(&run);
        let deadline = tokio::time::Instant::now() + GONE_WITHIN;
        loop {
            let body: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&file).expect("read")).expect("json");
            if body["entries"].as_array().is_some_and(Vec::is_empty) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "still registered: {body}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    reaper.assert_gone().await;
    registry.close().expect("close");
}

#[cfg(unix)]
#[cfg(test)]
fn registry_file(run: &Path) -> std::path::PathBuf {
    std::fs::read_dir(run)
        .expect("run dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("the registry file")
}

#[tokio::test]
async fn helper_failures_are_spawn_errors() {
    let folder = tempfile::tempdir().expect("tempdir");
    let registry = LeftoverRegistry::disabled();
    let missing = run_helper(
        &registry,
        HelperSpec {
            program: folder.path().join("nothing-here"),
            ..helper(&[], folder.path())
        },
    )
    .await;
    assert!(
        matches!(missing, Err(SpawnError::ProgramNotFound { .. })),
        "{missing:?}"
    );
    let gone = run_helper(
        &registry,
        helper(&["print-cwd"], &folder.path().join("gone")),
    )
    .await;
    assert!(
        matches!(gone, Err(SpawnError::InvalidCwd { .. })),
        "{gone:?}"
    );
}

#[test]
fn run_helper_outside_a_runtime_is_an_error() {
    let registry = LeftoverRegistry::disabled();
    let spec = helper(&["print-cwd"], &std::env::temp_dir());
    let mut running = Box::pin(run_helper(&registry, spec));
    let mut context = Context::from_waker(Waker::noop());
    match running.as_mut().poll(&mut context) {
        Poll::Ready(result) => assert!(matches!(result, Err(SpawnError::NoRuntime)), "{result:?}"),
        Poll::Pending => panic!("a helper without a runtime must fail at once"),
    }
}
