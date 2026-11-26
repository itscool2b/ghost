use super::*;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
use crate::session::EditorOpen;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
use ghost_protocol::project_tools::ProjectEditor;

fn enable_git(rig: &mut Rig) {
    let env = EnvResolver::fixed(ResolvedEnv {
        vars: ghost_process::process_env(),
        source: EnvSource::Fallback {
            reason: "owned project utility fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let service = crate::gitworkspaces::GitWorkspaces::new(
        &rig.profile,
        env,
        ghost_process::LeftoverRegistry::disabled(),
    );
    let inner = Arc::get_mut(&mut rig.manager.inner).expect("one fixture owner");
    Arc::get_mut(&mut inner.shared)
        .expect("no actor yet")
        .deps
        .worktrees = Some(service);
}

struct Admission(Arc<AtomicBool>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn a_project_terminal_opens_in_one_step_and_holds_its_folder_beside_live_agents() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("native-terminal"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(ResolvedEnv {
        vars: ghost_process::process_env(),
        source: EnvSource::Fallback {
            reason: "metadata-only terminal fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let review = rig.manager.project_terminal_review(&chat.id).await.unwrap();
    assert!(!review.shells.is_empty());
    let shell = review.shells[0].id;
    let (plan, opened) = rig
        .manager
        .open_project_terminal(&chat.id, Some(shell))
        .await
        .unwrap();
    assert_eq!(
        (opened.id, &opened.program),
        (shell, &review.shells[0].program)
    );
    assert_eq!(plan.spec.folder, rig.folder.canonicalize().unwrap());
    // Shells start from plain paths: Windows PowerShell fails to start from a verbatim
    // (`\\?\C:\...`) program path, and cmd.exe falls back to the Windows folder from a verbatim
    // working folder.
    let launch = &plan.spec.launch;
    assert!(!launch.program.to_string_lossy().starts_with(r"\\?\"));
    let ghost_process::ProcessCwd::Fixed(cwd) = &launch.cwd else {
        panic!("a terminal starts in its reviewed folder");
    };
    assert!(!cwd.to_string_lossy().starts_with(r"\\?\"), "{cwd:?}");
    assert_eq!(
        same_file::Handle::from_path(cwd).unwrap(),
        same_file::Handle::from_path(&plan.spec.folder).unwrap()
    );
    assert!(rig.manager.shared().live.contains(&chat.id));
    assert_eq!(rig.manager.project_terminal_count(), 1);
    assert_eq!(rig.manager.shared().live.owned_work_count(), 0);
    // The review held nothing: another shell opens without one, and no choice takes the first.
    let (second, first) = rig
        .manager
        .open_project_terminal(&chat.id, None)
        .await
        .unwrap();
    assert_eq!(first.id, shell);
    assert_eq!(rig.manager.project_terminal_count(), 2);
    drop(second);
    rig.manager.stop(&chat.id).await.unwrap();
    eventually("retired", || !rig.manager.shared().live.contains(&chat.id)).await;
    let key = live::normalize_path(rig.folder.clone());
    assert!(!rig.manager.shared().live.reserve_folder(&key));
    drop(plan);
    assert!(rig.manager.shared().live.reserve_folder(&key));
    // While a change rewrites the folder, a terminal is refused, and the refusal says why.
    let refused = rig
        .manager
        .open_project_terminal(&chat.id, Some(shell))
        .await
        .err()
        .expect("refused while the folder changes");
    assert!(
        refused.to_string().contains("This folder is being changed"),
        "{refused}"
    );
    assert_eq!(rig.manager.project_terminal_count(), 0);
    rig.manager.shared().live.release_folder(&key);
    // A folder that is gone is named, not opened.
    let moved = rig.folder.with_extension("moved");
    std::fs::rename(&rig.folder, &moved).unwrap();
    let gone = rig
        .manager
        .open_project_terminal(&chat.id, Some(shell))
        .await
        .err()
        .expect("refused without its folder");
    assert!(
        gone.to_string().contains("Locate this chat's folder"),
        "{gone}"
    );
    std::fs::create_dir(&rig.folder).unwrap();
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}

/// The person's own shell is offered first, started as their terminal app starts it (interactive,
/// with their startup files; a login shell on macOS), and the plain shells follow it.
#[cfg(unix)]
#[tokio::test]
async fn the_persons_own_shell_comes_first() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("own-shell"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let mut vars = ghost_process::process_env();
    vars.insert("SHELL".into(), "/bin/sh".into());
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "own shell fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let review = rig.manager.project_tools_review(&chat.id).await.unwrap();
    let first = &review.shells[0];
    assert_eq!(first.id, ghost_protocol::project_tools::ProjectShell::Login);
    assert_eq!(first.label, "Your shell (sh)");
    let expected: &[&str] = if cfg!(target_os = "macos") {
        &["-l", "-i"]
    } else {
        &["-i"]
    };
    assert_eq!(first.args, expected);
    assert!(
        review.shells[1..]
            .iter()
            .all(|shell| shell.label.starts_with("Plain ")),
        "{:?}",
        review.shells
    );
    let (plan, choice) = rig
        .manager
        .open_project_terminal(&chat.id, Some(first.id))
        .await
        .unwrap();
    assert_eq!(choice.id, first.id);
    assert_eq!(
        plan.spec.launch.args,
        expected
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
    drop(plan);
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}

/// The docked panel opens several shells in one chat's folder. Each holds its own folder use, so
/// the quit question counts every shell and the folder stays protected until the last one ends.
#[tokio::test]
async fn several_project_terminals_in_one_folder_each_count_until_they_end() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("native-panel"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(ResolvedEnv {
        vars: ghost_process::process_env(),
        source: EnvSource::Fallback {
            reason: "metadata-only terminal panel fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let mut plans = Vec::new();
    for _ in 0..3 {
        let (plan, _) = rig
            .manager
            .open_project_terminal(&chat.id, None)
            .await
            .unwrap();
        plans.push(plan);
    }
    assert_eq!(rig.manager.project_terminal_count(), 3);
    let key = live::normalize_path(rig.folder.clone());
    plans.pop();
    assert_eq!(rig.manager.project_terminal_count(), 2);
    assert!(
        !rig.manager
            .wait_project_terminals(Duration::from_millis(50))
            .await
    );
    plans.clear();
    assert_eq!(rig.manager.project_terminal_count(), 0);
    assert!(
        rig.manager
            .wait_project_terminals(Duration::from_millis(50))
            .await
    );
    rig.manager.stop(&chat.id).await.unwrap();
    eventually("retired", || !rig.manager.shared().live.contains(&chat.id)).await;
    assert!(rig.manager.shared().live.reserve_folder(&key));
    rig.manager.shared().live.release_folder(&key);
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}

/// A synthetic Zed install is offered, opens through its own launcher with the folder as its one
/// argument, and a launcher replaced after review is refused rather than started.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_detected_launcher_editor_opens_the_reviewed_folder_and_refuses_a_replaced_one() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("native-editor"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let install = tempfile::tempdir().unwrap();
    let mut vars = ghost_process::process_env();
    let launcher = if cfg!(windows) {
        vars.insert("LOCALAPPDATA".into(), install.path().into());
        install.path().join("Programs/Zed/Zed.exe")
    } else if cfg!(target_os = "linux") {
        vars.insert("HOME".into(), install.path().into());
        install.path().join(".local/zed.app/bin/zed")
    } else {
        vars.insert("HOME".into(), install.path().into());
        install
            .path()
            .join("Applications/Zed.app/Contents/MacOS/zed")
    };
    std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    std::fs::write(&launcher, b"first").unwrap();
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "synthetic editor install".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let review = rig.manager.project_tools_review(&chat.id).await.unwrap();
    assert!(review.editors.contains(&ProjectEditor::Zed));
    let open = rig
        .manager
        .project_editor_open(&chat.id, &review.token, ProjectEditor::Zed)
        .await
        .unwrap();
    let EditorOpen::Launch(launch) = open else {
        panic!("Zed opens through its launcher");
    };
    if cfg!(any(windows, target_os = "linux")) {
        assert_eq!(launch.program, launcher);
        assert_eq!(
            launch.args,
            [std::ffi::OsString::from(review.folder.clone())]
        );
    } else {
        assert_eq!(launch.program, PathBuf::from("/usr/bin/open"));
        assert_eq!(launch.args.last(), Some(&review.folder.clone().into()));
    }
    // The review is used up, and a launcher replaced since review is refused.
    assert!(
        rig.manager
            .project_editor_open(&chat.id, &review.token, ProjectEditor::Zed)
            .await
            .is_err()
    );
    let second = rig.manager.project_tools_review(&chat.id).await.unwrap();
    std::fs::remove_file(&launcher).unwrap();
    std::fs::write(&launcher, b"second").unwrap();
    let refused = rig
        .manager
        .project_editor_open(&chat.id, &second.token, ProjectEditor::Zed)
        .await
        .err()
        .expect("a replaced launcher is refused");
    assert!(refused.to_string().contains("changed after review"));
    // An editor the review did not offer is refused.
    let third = rig.manager.project_tools_review(&chat.id).await.unwrap();
    assert!(!third.editors.contains(&ProjectEditor::CLion));
    assert!(
        rig.manager
            .project_editor_open(&chat.id, &third.token, ProjectEditor::CLion)
            .await
            .is_err()
    );
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}

#[tokio::test]
async fn initialization_waits_for_a_working_agent_and_keeps_admission_through_completion() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("native-init"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_git(&mut rig);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let preview = rig
        .manager
        .git_initialize_preview(&chat.id, "main")
        .await
        .expect("read-only review");
    rig.manager
        .send(&chat.id, "working".into())
        .await
        .expect("send");
    handle.wait_for(CommandMatch::Prompt).await;
    let released = Arc::new(AtomicBool::new(false));
    let error = rig
        .manager
        .git_initialize_apply(
            &chat.id,
            &preview.token,
            Box::new(Admission(released.clone())),
        )
        .await
        .expect_err("a working agent's folder stays protected");
    assert!(
        error.to_string().contains("is working in this folder"),
        "{error}"
    );
    assert!(released.load(Ordering::Acquire));
    assert!(!rig.folder.join(".git").exists());
    rig.manager.stop(&chat.id).await.expect("stop");
    eventually("retired", || !rig.manager.shared().live.contains(&chat.id)).await;
    let released = Arc::new(AtomicBool::new(false));
    let result = rig
        .manager
        .git_initialize_apply(
            &chat.id,
            &preview.token,
            Box::new(Admission(released.clone())),
        )
        .await
        .expect("explicit initialization after stop");
    assert!(result.head.is_none());
    assert!(released.load(Ordering::Acquire));
    assert!(!rig.folder.join(".git/index").exists());
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}

/// Runs Git in `cwd` for a fixture.
pub(super) async fn fixture_git(cwd: &Path, args: &[&str]) {
    use ghost_process::{HelperEnv, HelperSpec, LeftoverRegistry, find_exe, run_helper};
    let vars = ghost_process::process_env();
    let git = find_exe("git", &vars).expect("Git is required for this fixture");
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            program: git,
            args: args.iter().map(Into::into).collect(),
            cwd: cwd.to_path_buf(),
            env: HelperEnv::Resolved(Arc::new(ResolvedEnv {
                vars,
                source: EnvSource::Fallback {
                    reason: "push fixture".into(),
                },
                warnings: vec![],
                resolved_at_ms: 0,
            })),
            set: vec![],
            new_session: true,
            timeout: Duration::from_secs(15),
            max_stdout: 8192,
        },
    )
    .await
    .expect("fixture Git");
    assert_eq!(
        output.exit.and_then(|exit| exit.code),
        Some(0),
        "{}",
        output.stderr_tail
    );
}

#[tokio::test]
async fn a_terminal_opens_while_another_folder_pushes() {
    let driver = |native: &str| {
        ScriptedDriver::new(vec![
            ready_step(native),
            Step::Expect(CommandMatch::Shutdown),
            Step::Exit(DriverExit::Closed),
        ])
        .0
    };
    let mut rig = rig(
        vec![driver("pushing"), driver("elsewhere")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    super::worktree_guards::enable_worktree_fixture(&mut rig).await;
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(ResolvedEnv {
        vars: ghost_process::process_env(),
        source: EnvSource::Fallback {
            reason: "metadata-only terminal fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let remote = rig._dir.path().join("remote.git");
    fixture_git(
        rig._dir.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .await;
    fixture_git(
        &rig.folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    let other_folder = rig._dir.path().join("other");
    std::fs::create_dir(&other_folder).unwrap();
    let pushing = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    let elsewhere = rig
        .manager
        .create_chat("mock", &other_folder)
        .await
        .unwrap();
    status(&rig, &pushing.id, SessionStatus::Ready).await;
    status(&rig, &elsewhere.id, SessionStatus::Ready).await;
    rig.manager.stop(&pushing.id).await.unwrap();
    eventually("stopped", || {
        !rig.manager.shared().live.contains(&pushing.id)
    })
    .await;
    let preview = rig
        .manager
        .git_push_preview(&pushing.id)
        .await
        .expect("push review");
    // The push is slow (a large upload, a slow network): it holds its repository meanwhile.
    let slow = rig
        .manager
        .worktrees()
        .unwrap()
        .hold_repository_for_test(&rig.folder)
        .await;
    let push = tokio::spawn({
        let manager = rig.manager.clone();
        let (chat, token) = (pushing.id.clone(), preview.token.clone());
        async move { manager.git_push_apply(&chat, &token).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!push.is_finished(), "the push is under way");
    let quick = Duration::from_secs(5);
    tokio::time::timeout(quick, rig.manager.project_tools_review(&elsewhere.id))
        .await
        .expect("project tools answer at once")
        .expect("review");
    let review = tokio::time::timeout(quick, rig.manager.project_terminal_review(&elsewhere.id))
        .await
        .expect("a new terminal tab's folder check answers at once")
        .expect("terminal review");
    let shell = review.shells[0].id;
    let (plan, _) = tokio::time::timeout(
        quick,
        rig.manager
            .open_project_terminal(&elsewhere.id, Some(shell)),
    )
    .await
    .expect("a terminal in another folder opens at once")
    .expect("terminal");
    assert_eq!(plan.spec.folder, other_folder.canonicalize().unwrap());
    drop(plan);
    drop(slow);
    tokio::time::timeout(quick, push)
        .await
        .expect("the push finishes")
        .unwrap()
        .expect("pushed");
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert!(rig.store.shutdown(Duration::from_secs(5)).await);
}
