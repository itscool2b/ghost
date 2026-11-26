use super::*;
use ghost_process::{LeftoverRegistry, find_exe};
use ghost_protocol::setup_actions::{CloneInput, CloneRecord, CloneState};

const REMOTE: &str = "https://clone-fixture.invalid/source.git";

fn fixture() -> (Rig, Arc<crate::cloning::Clones>, Arc<ResolvedEnv>) {
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let home = rig._dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let config = home.join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let mut vars = ghost_process::process_env();
    vars.retain(|key, _| !key.as_os_str().to_string_lossy().starts_with("GIT_"));
    for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
        vars.insert(key.into(), home.as_os_str().to_owned());
    }
    vars.insert("GIT_CONFIG_GLOBAL".into(), config.into_os_string());
    vars.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    vars.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    vars.insert("NO_PROXY".into(), "127.0.0.1".into());
    find_exe("git", &vars).expect("Git is required for session clone tests");
    let source = rig._dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut env = ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "clone fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    };
    git(&env, &source, &["init", "-q"]);
    std::fs::write(source.join("README.md"), "session clone fixture\n").unwrap();
    git(&env, &source, &["add", "README.md"]);
    git(
        &env,
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    );
    let source = source.to_string_lossy().replace('\\', "/");
    let local = if source.starts_with('/') {
        format!("file://{source}")
    } else {
        format!("file:///{source}")
    };
    env.vars.insert("GIT_CONFIG_COUNT".into(), "1".into());
    env.vars.insert(
        "GIT_CONFIG_KEY_0".into(),
        format!("url.{local}.insteadOf").into(),
    );
    env.vars.insert("GIT_CONFIG_VALUE_0".into(), REMOTE.into());
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(env.clone());
    let data = rig._dir.path().join("clone-data");
    std::fs::create_dir(&data).unwrap();
    let clones = Arc::new(crate::cloning::Clones::new(Some(data)));
    (rig, clones, Arc::new(env))
}

fn git(env: &ResolvedEnv, folder: &Path, args: &[&str]) {
    let output = std::process::Command::new(find_exe("git", &env.vars).unwrap())
        .args(args)
        .current_dir(folder)
        .env_clear()
        .envs(env.vars.iter().map(|(key, value)| (key.as_os_str(), value)))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn start(rig: &Rig, clones: &crate::cloning::Clones, name: &str) -> String {
    let review = rig
        .manager
        .clone_check(
            clones,
            "main",
            CloneInput {
                remote: REMOTE.into(),
                parent: Some(rig.folder.to_string_lossy().into_owned()),
                name: Some(name.into()),
            },
        )
        .await
        .unwrap()
        .review
        .expect("valid clone review");
    rig.manager
        .clone_start(
            clones,
            "main",
            &review.id,
            LeftoverRegistry::disabled(),
            Box::new(()),
        )
        .unwrap();
    review.id
}

async fn settled(rig: &Rig, clones: &crate::cloning::Clones, id: &str) -> CloneRecord {
    let record = tokio::time::timeout(Duration::from_secs(30), async {
        let mut after = None;
        loop {
            let record = clones.status("main", id, after).await.unwrap();
            if !matches!(
                record.state,
                CloneState::Running | CloneState::CleanupPending
            ) {
                break record;
            }
            after = Some(record.revision);
        }
    })
    .await
    .expect("clone settled");
    eventually("clone reservations released", || {
        rig.manager.shared().live.owned_work_count() == 0
    })
    .await;
    record
}

#[tokio::test]
async fn session_clone_start_reserves_its_parent_and_destination_and_clones_real_files() {
    let (rig, clones, _) = fixture();
    let id = start(&rig, &clones, "complete").await;
    let record = settled(&rig, &clones, &id).await;
    assert_eq!(record.state, CloneState::Ready, "{}", record.detail);
    assert_eq!(
        std::fs::read_to_string(rig.folder.join("complete/README.md")).unwrap(),
        "session clone fixture\n"
    );
    assert!(rig.folder.join("complete/.git").is_dir());
    let recovered = crate::cloning::Clones::new(Some(rig._dir.path().join("clone-data")))
        .recovery_get(&id)
        .unwrap();
    assert!(recovered.can_add && recovered.can_open);
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), rig.folder.join("complete")])
        .await
        .unwrap();
    drop(reservation);
}

#[tokio::test]
async fn session_clone_start_rejects_external_overlapping_owners_and_releases_failed_admission() {
    let (rig, clones, _) = fixture();
    let parent = live::normalize_path(rig.folder.clone());
    for owner in ["terminal", "chat", "mutation"] {
        let held = parent.join("existing");
        match owner {
            "terminal" => assert!(rig.manager.shared().live.begin_folder_use(held.clone())),
            "chat" => {
                assert!(rig.manager.shared().live.acquire("other"));
                assert!(rig.manager.shared().live.set_folder("other", held.clone()));
            }
            _ => assert!(rig.manager.shared().live.reserve_folder(&held)),
        }
        let id = start(&rig, &clones, owner).await;
        let record = tokio::time::timeout(Duration::from_secs(30), async {
            let mut after = None;
            loop {
                let record = clones.status("main", &id, after).await.unwrap();
                if record.state == CloneState::Failed {
                    break record;
                }
                after = Some(record.revision);
            }
        })
        .await
        .unwrap();
        // The refusal names what is in the way.
        let named = match owner {
            "terminal" => "A project terminal is open in this folder",
            "chat" => "is working in this folder",
            _ => "Another change to this folder is in progress",
        };
        assert!(record.detail.contains(named), "{}", record.detail);
        assert!(!rig.folder.join(owner).exists());
        match owner {
            "terminal" => rig.manager.shared().live.end_folder_use(&held),
            "chat" => rig.manager.shared().live.release("other"),
            _ => rig.manager.shared().live.release_folder(&held),
        }
        let reservation = rig
            .manager
            .reserve_git_folders(vec![rig.folder.clone()])
            .await
            .expect("failed admission released all paths");
        drop(reservation);
    }
}

#[tokio::test]
async fn session_clone_failure_releases_parent_and_destination() {
    let (rig, clones, _) = fixture();
    std::fs::rename(
        rig._dir.path().join("source"),
        rig._dir.path().join("retained-source"),
    )
    .unwrap();
    let id = start(&rig, &clones, "failure").await;
    let record = settled(&rig, &clones, &id).await;
    assert_eq!(record.state, CloneState::Failed, "{}", record.detail);
    assert!(record.removed, "{}", record.detail);
    assert!(!rig.folder.join("failure").exists());
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), rig.folder.join("failure")])
        .await
        .unwrap();
    drop(reservation);
}

#[tokio::test]
async fn session_clone_cancel_stops_real_git_before_releasing_its_folders() {
    let (mut rig, clones, env) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut env = (*env).clone();
    env.vars.insert(
        "GIT_CONFIG_KEY_0".into(),
        format!(
            "url.http://127.0.0.1:{}/repo.git.insteadOf",
            listener.local_addr().unwrap().port()
        )
        .into(),
    );
    let inner = Arc::get_mut(&mut rig.manager.inner).unwrap();
    Arc::get_mut(&mut inner.shared).unwrap().deps.env = EnvResolver::fixed(env);
    let id = start(&rig, &clones, "cancel").await;
    let (_connection, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
        .await
        .expect("Git connected to fixture")
        .unwrap();
    assert!(rig.folder.join("cancel").is_dir());
    assert!(
        rig.manager
            .reserve_git_folders(vec![rig.folder.clone()])
            .await
            .is_err()
    );
    clones.cancel("main", &id);
    let record = settled(&rig, &clones, &id).await;
    assert_eq!(record.state, CloneState::Canceled, "{}", record.detail);
    assert!(record.removed, "{}", record.detail);
    assert!(!rig.folder.join("cancel").exists());
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), rig.folder.join("cancel")])
        .await
        .unwrap();
    drop(reservation);
}

#[tokio::test]
async fn nested_git_reservations_keep_both_requested_chat_actors_reserved() {
    let (rig, _, _) = fixture();
    let child = rig.folder.join("child");
    std::fs::create_dir(&child).unwrap();
    let mut chats = Vec::new();
    for folder in [rig.folder.clone(), child.clone()] {
        chats.push(
            rig.store
                .create_chat(NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder,
                    device_id: "device".into(),
                    now: now_millis(),
                })
                .await
                .unwrap()
                .id,
        );
    }
    let reservation = rig
        .manager
        .reserve_git_folders(vec![child.clone(), rig.folder.clone(), child.clone()])
        .await
        .unwrap();
    for chat in &chats {
        let (reply, answer) = oneshot::channel();
        rig.manager
            .actor(chat)
            .send(ChatMsg::ReserveGit {
                id: "competing".into(),
                reply,
            })
            .unwrap();
        assert!(
            answer.await.unwrap().is_err(),
            "each requested actor stays reserved"
        );
    }
    drop(reservation);
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), child])
        .await
        .expect("dropping the guard releases actors and folders");
    drop(reservation);
}

#[tokio::test]
async fn refused_actor_releases_the_whole_nested_folder_reservation() {
    let (rig, _, _) = fixture();
    let child = rig.folder.join("child");
    std::fs::create_dir(&child).unwrap();
    let chat = rig
        .store
        .create_chat(NewChat {
            agent_host: Default::default(),
            agent_id: "mock".into(),
            folder: child.clone(),
            device_id: "device".into(),
            now: now_millis(),
        })
        .await
        .unwrap();
    let mailbox = rig.manager.actor(&chat.id);
    let (reply, answer) = oneshot::channel();
    mailbox
        .send(ChatMsg::ReserveGit {
            id: "existing".into(),
            reply,
        })
        .unwrap();
    answer.await.unwrap().unwrap();
    assert!(
        rig.manager
            .reserve_git_folders(vec![rig.folder.clone(), child.clone()])
            .await
            .is_err()
    );
    assert_eq!(rig.manager.shared().live.owned_work_count(), 0);
    mailbox
        .send(ChatMsg::ReleaseGit {
            id: "existing".into(),
        })
        .unwrap();
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), child])
        .await
        .unwrap();
    drop(reservation);
}

#[tokio::test]
async fn canceled_actor_acquisition_releases_the_whole_nested_folder_reservation() {
    let (rig, _, _) = fixture();
    let child = rig.folder.join("child");
    std::fs::create_dir(&child).unwrap();
    rig.store
        .create_chat(NewChat {
            agent_host: Default::default(),
            agent_id: "mock".into(),
            folder: child.clone(),
            device_id: "device".into(),
            now: now_millis(),
        })
        .await
        .unwrap();
    let mut acquisition = Box::pin(
        rig.manager
            .reserve_git_folders(vec![rig.folder.clone(), child.clone()]),
    );
    // On this single-thread runtime, stop polling as soon as folders are held and the
    // real actor's reservation reply is pending, before it can process its mailbox.
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(acquisition.as_mut(), cx).is_pending());
        if rig.manager.shared().live.owned_work_count() > 0 {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    drop(acquisition);
    assert_eq!(rig.manager.shared().live.owned_work_count(), 0);
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone(), child])
        .await
        .expect("cancel releases the pending real actor too");
    drop(reservation);
}
