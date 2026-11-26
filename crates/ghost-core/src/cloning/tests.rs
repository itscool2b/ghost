//! Clone checks with a synthetic Git, and real clones from a local bare repository. The real-Git
//! cases use an isolated, empty Git configuration and are skipped when Git is not installed.
use super::*;
use ghost_process::{EnvMap, EnvSource, LeftoverRegistry, Spawner};
use std::process::Command;

fn env_with(vars: EnvMap) -> Arc<ResolvedEnv> {
    Arc::new(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "test".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    })
}

/// A PATH holding only a file named git that is never run.
fn synthetic_git(folder: &Path) -> Arc<ResolvedEnv> {
    let bin = folder.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let git = bin.join(if cfg!(windows) { "git.exe" } else { "git" });
    std::fs::write(&git, "synthetic executable, never run").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut vars = EnvMap::new();
    vars.insert("PATH".into(), bin.as_os_str().to_owned());
    env_with(vars)
}

fn input(remote: &str, parent: Option<&Path>, name: Option<&str>) -> CloneInput {
    CloneInput {
        remote: remote.into(),
        parent: parent.map(|path| path.to_string_lossy().into_owned()),
        name: name.map(str::to_owned),
    }
}

#[tokio::test]
async fn lifecycle_survives_restart_without_output_and_dismissal_keeps_the_clone() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let parent = temp.path().join("code");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let env = synthetic_git(temp.path());
    let service = Clones::new(Some(data.clone()));
    let review = service
        .check(
            "window",
            input("https://github.com/owner/repo", Some(&parent), None),
            env,
        )
        .await
        .unwrap()
        .review
        .unwrap();
    let (prepared, job) = service.take("window", &review.id).unwrap();
    let restarted = Clones::new(Some(data.clone()));
    // The planned record exists before a destination. Recovery never creates that folder.
    let before = restarted.recovery_list().unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].state, CloneState::Failed);
    assert!(!parent.join("repo").exists());
    let destination = files::begin(prepared).unwrap();
    job.journal.as_ref().unwrap().created(&destination).unwrap();
    std::fs::write(parent.join("repo/user-work"), "keep my changes").unwrap();
    job.update(|record| record.output = vec!["private raw Git output".into()]);
    let interrupted = restarted.recovery_list().unwrap();
    assert_eq!(interrupted[0].state, CloneState::Interrupted);
    assert!(interrupted[0].can_open);
    assert!(!interrupted[0].can_add);
    assert!(
        service.recovery_list().unwrap().is_empty(),
        "active attempt is not an interrupted notice"
    );
    std::fs::create_dir(parent.join("repo/.git")).unwrap();
    job.journal
        .as_ref()
        .unwrap()
        .git_finished(&destination)
        .unwrap();
    // Git exited but the app stopped before project addition or its final Ready update.
    let completed = restarted.recovery_get(&review.id).unwrap();
    assert!(completed.can_add && completed.can_open);
    assert_eq!(completed.state, CloneState::Ready);
    let journal = data.join("clones").join(format!("{}.json", review.id));
    assert!(
        !std::fs::read_to_string(&journal)
            .unwrap()
            .contains("private raw Git output")
    );
    drop(destination);
    // Ending the old instance is independent of the restart's read-only view.
    drop(service);
    drop(job);
    assert_eq!(restarted.recovery_get(&review.id).unwrap(), completed);
    // Keeping the destination identity does not authorize a substituted Git directory.
    std::fs::rename(parent.join("repo/.git"), parent.join("repo/.git-original")).unwrap();
    std::fs::create_dir(parent.join("repo/.git")).unwrap();
    let changed_git = restarted.recovery_get(&review.id).unwrap();
    assert!(changed_git.can_open && !changed_git.can_add);
    assert_eq!(changed_git.state, CloneState::Interrupted);
    restarted.recovery_dismiss(&review.id).unwrap();
    restarted.recovery_dismiss(&review.id).unwrap();
    assert!(restarted.recovery_list().unwrap().is_empty());
    assert!(parent.join("repo/.git").is_dir());
    assert!(parent.join("repo/.git-original").is_dir());
    assert_eq!(
        std::fs::read_to_string(parent.join("repo/user-work")).unwrap(),
        "keep my changes"
    );
}

#[tokio::test]
async fn replaced_and_linked_destinations_are_preserved_and_never_offered_as_completed() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let parent = temp.path().join("code");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let service = Clones::new(Some(data.clone()));
    let review = service
        .check(
            "window",
            input("https://github.com/owner/repo", Some(&parent), None),
            synthetic_git(temp.path()),
        )
        .await
        .unwrap()
        .review
        .unwrap();
    let (prepared, job) = service.take("window", &review.id).unwrap();
    let destination = files::begin(prepared).unwrap();
    job.journal.as_ref().unwrap().created(&destination).unwrap();
    std::fs::create_dir(parent.join("repo/.git")).unwrap();
    job.journal
        .as_ref()
        .unwrap()
        .git_finished(&destination)
        .unwrap();
    drop(destination);
    std::fs::rename(parent.join("repo"), parent.join("moved")).unwrap();
    std::fs::create_dir_all(parent.join("repo/.git")).unwrap();
    std::fs::write(parent.join("repo/foreign.txt"), "foreign").unwrap();
    let restarted = Clones::new(Some(data));
    let record = restarted.recovery_get(&review.id).unwrap();
    assert!(!record.can_open && !record.can_add);
    assert_eq!(
        std::fs::read_to_string(parent.join("repo/foreign.txt")).unwrap(),
        "foreign"
    );
    // A redirect to the original folder still fails the no-follow boundary.
    std::fs::rename(parent.join("repo"), parent.join("foreign")).unwrap();
    #[cfg(windows)]
    {
        let result = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(parent.join("repo"))
            .arg(parent.join("moved"))
            .output()
            .unwrap();
        assert!(result.status.success(), "fixture junction creation failed");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(parent.join("moved"), parent.join("repo")).unwrap();
    let linked = restarted.recovery_get(&review.id).unwrap();
    assert!(!linked.can_open && !linked.can_add);
    restarted.recovery_dismiss(&review.id).unwrap();
    assert!(parent.join("repo").exists());
    assert!(parent.join("foreign/foreign.txt").exists());
}

#[tokio::test]
async fn corrupt_records_and_failed_publication_preserve_folders_and_unknown_files() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let service = Clones::new(Some(data.clone()));
    let review = service
        .check(
            "window",
            input("https://github.com/owner/repo", Some(temp.path()), None),
            synthetic_git(temp.path()),
        )
        .await
        .unwrap()
        .review
        .unwrap();
    // A journal root occupied by a file prevents destination creation.
    std::fs::write(data.join("clones"), "unknown data").unwrap();
    assert!(service.take("window", &review.id).is_err());
    assert!(!temp.path().join("repo").exists());
    std::fs::rename(data.join("clones"), data.join("saved-unknown")).unwrap();
    std::fs::create_dir(data.join("clones")).unwrap();
    let id = uuid::Uuid::now_v7().to_string();
    let record = data.join("clones").join(format!("{id}.json"));
    std::fs::write(&record, "{broken").unwrap();
    std::fs::write(data.join("clones/foreign.txt"), "keep").unwrap();
    let records = service.recovery_list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].destination.is_none() && records[0].remote.is_none());
    assert!(!records[0].can_open && !records[0].can_add);
    assert!(records[0].detail.contains(&id));
    assert_eq!(service.recovery_list().unwrap(), records);
    assert_eq!(std::fs::read_to_string(&record).unwrap(), "{broken");
    assert!(service.recovery_dismiss("../saved-unknown").is_err());
    service.recovery_dismiss(&id).unwrap();
    assert_eq!(
        std::fs::read_to_string(data.join("saved-unknown")).unwrap(),
        "unknown data"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("clones/foreign.txt")).unwrap(),
        "keep"
    );
}

fn entries(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_check_is_inert_and_puts_each_problem_beside_its_field() {
    let temp = tempfile::tempdir().unwrap();
    let env = synthetic_git(temp.path());
    let parent = temp.path().join("code");
    std::fs::create_dir(&parent).unwrap();
    let service = Clones::default();
    let check = |value: CloneInput, env: Arc<ResolvedEnv>| service.check("main", value, env);

    let found = check(
        input("file:///srv/repo.git", Some(&parent), None),
        Arc::clone(&env),
    )
    .await
    .unwrap();
    assert_eq!(found.problem.unwrap().field, CloneField::Remote);
    assert!(found.review.is_none() && found.repository.is_none());

    let found = check(
        input("https://github.com/owner/repo", None, None),
        Arc::clone(&env),
    )
    .await
    .unwrap();
    assert_eq!(found.problem.unwrap().field, CloneField::Parent);
    assert_eq!(found.repository.as_deref(), Some("owner/repo"));
    assert_eq!(found.name.as_deref(), Some("repo"));

    std::fs::create_dir(parent.join("taken")).unwrap();
    let found = check(
        input(
            "https://github.com/owner/repo",
            Some(&parent),
            Some("taken"),
        ),
        Arc::clone(&env),
    )
    .await
    .unwrap();
    let problem = found.problem.unwrap();
    assert_eq!(problem.field, CloneField::Name);
    assert!(problem.message.contains("already has a taken folder"));

    let found = check(
        input("https://github.com/owner/repo", Some(&parent), Some("..")),
        Arc::clone(&env),
    )
    .await
    .unwrap();
    assert_eq!(found.problem.unwrap().field, CloneField::Name);

    let found = check(
        input("https://github.com/owner/repo", Some(&parent), None),
        env_with(EnvMap::new()),
    )
    .await
    .unwrap();
    assert_eq!(found.problem.unwrap().field, CloneField::Git);

    let found = check(
        input("git@github.com:owner/repo.git", Some(&parent), None),
        Arc::clone(&env),
    )
    .await
    .unwrap();
    assert!(found.problem.is_none());
    let review = found.review.unwrap();
    let destination = plain_path(&std::fs::canonicalize(&parent).unwrap()).join("repo");
    let destination = destination.to_string_lossy().into_owned();
    assert_eq!(review.destination, destination);
    assert_eq!(
        review.arguments,
        [
            "clone",
            "--progress",
            "--",
            "git@github.com:owner/repo.git",
            destination.as_str()
        ]
    );
    assert_eq!(
        review.command,
        format!("git clone --progress -- git@github.com:owner/repo.git {destination}")
    );
    assert_eq!(
        entries(&parent),
        ["taken"],
        "checking never creates a folder or runs Git"
    );

    // A typed name with a space is quoted for display only.
    let spaced = check(
        input(
            "https://github.com/owner/repo",
            Some(&parent),
            Some("My repo"),
        ),
        Arc::clone(&env),
    )
    .await
    .unwrap()
    .review
    .unwrap();
    assert!(spaced.command.ends_with("My repo\""));
    assert!(spaced.arguments[4].ends_with("My repo"));

    // Reviews belong to their window and start once.
    assert!(service.take("other", &review.id).is_err());
    service.cancel("main", &review.id);
    assert!(service.take("main", &review.id).is_err());
    let review = check(
        input("https://github.com/owner/repo", Some(&parent), None),
        Arc::clone(&env),
    )
    .await
    .unwrap()
    .review
    .unwrap();
    let (prepared, job) = service.take("main", &review.id).unwrap();
    assert!(service.take("main", &review.id).is_err());
    // Canceled before Git starts: nothing is created.
    service.drop_owner("main");
    execute(
        prepared,
        Arc::clone(&job),
        Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        ),
    )
    .await;
    assert_eq!(job.snapshot().state, CloneState::Canceled);
    assert_eq!(entries(&parent), ["taken"]);
}

#[tokio::test]
async fn typing_keeps_only_the_newest_reviews_per_window() {
    let temp = tempfile::tempdir().unwrap();
    let env = synthetic_git(temp.path());
    let service = Clones::default();
    let mut ids = Vec::new();
    for index in 0..12 {
        let review = service
            .check(
                "main",
                input(
                    &format!("https://github.com/owner/repo{index}"),
                    Some(temp.path()),
                    None,
                ),
                Arc::clone(&env),
            )
            .await
            .unwrap()
            .review
            .unwrap();
        ids.push(review.id);
    }
    assert_eq!(lock(&service.pending).len(), 8);
    assert!(!lock(&service.pending).contains_key(&ids[0]));
    assert!(lock(&service.pending).contains_key(&ids[11]));
}

#[test]
fn the_owned_folder_is_removed_with_read_only_files_and_a_replaced_one_is_kept() {
    let temp = tempfile::tempdir().unwrap();
    let env = synthetic_git(temp.path());
    let remote = validate::remote("https://github.com/owner/repo").unwrap();
    let prepared = plan(remote.clone(), "repo".into(), temp.path().to_str(), &env).unwrap();
    let destination = files::begin(prepared).unwrap();
    let pack = temp.path().join("repo/.git/objects/pack");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("pack-1.pack"), "read-only like Git's packs").unwrap();
    let mut permissions = std::fs::metadata(pack.join("pack-1.pack"))
        .unwrap()
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(pack.join("pack-1.pack"), permissions).unwrap();
    assert!(files::remove(destination));
    assert!(!temp.path().join("repo").exists());

    // A folder put in its place is someone else's: it stays.
    let prepared = plan(remote, "repo".into(), temp.path().to_str(), &env).unwrap();
    let destination = files::begin(prepared).unwrap();
    if std::fs::rename(temp.path().join("repo"), temp.path().join("moved")).is_ok() {
        std::fs::create_dir(temp.path().join("repo")).unwrap();
        std::fs::write(temp.path().join("repo/user.txt"), "keep").unwrap();
        assert!(!files::remove(destination));
        assert_eq!(
            std::fs::read(temp.path().join("repo/user.txt")).unwrap(),
            b"keep"
        );
    }
}

/// Git with an empty configuration of its own, so this computer's settings play no part.
fn real_git(temp: &Path) -> Option<Arc<ResolvedEnv>> {
    let mut vars = ghost_process::process_env();
    for key in REPOSITORY_VARIABLES {
        vars.remove(&EnvKey::from(*key));
    }
    let config = temp.join("empty-gitconfig");
    std::fs::write(&config, "").unwrap();
    vars.insert("GIT_CONFIG_GLOBAL".into(), config.into_os_string());
    vars.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    find_exe("git", &vars)?;
    Some(env_with(vars))
}

fn git(env: &ResolvedEnv, folder: &Path, args: &[&str]) {
    let program = find_exe("git", &env.vars).unwrap();
    let status = Command::new(program)
        .args(args)
        .current_dir(folder)
        .env_clear()
        .envs(env.vars.iter().map(|(key, value)| (key.as_os_str(), value)))
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A bare repository with one commit, reachable by a file:// URL.
fn bare_repository(temp: &Path, env: &ResolvedEnv) -> String {
    let work = temp.join("work");
    std::fs::create_dir(&work).unwrap();
    git(env, &work, &["init", "-q"]);
    std::fs::write(work.join("README.md"), "hello from the fixture\n").unwrap();
    git(env, &work, &["add", "README.md"]);
    git(
        env,
        &work,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-q",
            "-m",
            "First",
        ],
    );
    git(env, temp, &["clone", "-q", "--bare", "work", "source.git"]);
    file_url(&temp.join("source.git"))
}

fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// A review for an address the form would refuse (file://, http://), as only a test can make.
fn review_for(service: &Clones, url: &str, parent: &Path, env: &ResolvedEnv) -> String {
    let remote = validate::Remote {
        url: url.into(),
        repository: "fixture/source".into(),
        name: "source".into(),
    };
    let prepared = plan(remote, "clone".into(), parent.to_str(), env).unwrap();
    let id = prepared.view.id.clone();
    lock(&service.pending).insert(
        id.clone(),
        Pending {
            owner: "main".into(),
            prepared,
            created: Instant::now(),
        },
    );
    id
}

fn spawner() -> Arc<Spawner> {
    Spawner::new(
        LeftoverRegistry::disabled(),
        tokio::runtime::Handle::current(),
    )
}

#[tokio::test]
async fn a_real_clone_from_a_local_bare_repository_becomes_a_folder_with_its_files() {
    let temp = tempfile::tempdir().unwrap();
    let Some(env) = real_git(temp.path()) else {
        eprintln!("Git is not installed; skipped.");
        return;
    };
    let url = bare_repository(temp.path(), &env);
    let parent = temp.path().join("projects");
    std::fs::create_dir(&parent).unwrap();
    let service = Clones::default();
    let id = review_for(&service, &url, &parent, &env);
    let (prepared, job) = service.take("main", &id).unwrap();
    execute(prepared, Arc::clone(&job), spawner()).await;
    let record = service.status("main", &id, None).await.unwrap();
    assert_eq!(record.state, CloneState::Ready, "{}", record.detail);
    assert!(!record.removed);
    assert_eq!(
        std::fs::read_to_string(parent.join("clone/README.md")).unwrap(),
        "hello from the fixture\n"
    );
    assert!(parent.join("clone/.git").is_dir());
    if let Some(progress) = record.progress {
        assert_eq!(progress.percent, 100);
    }
    assert!(record.revision > 1, "progress and states were published");
}

#[tokio::test]
async fn a_failed_clone_removes_only_the_folder_it_created() {
    let temp = tempfile::tempdir().unwrap();
    let Some(env) = real_git(temp.path()) else {
        eprintln!("Git is not installed; skipped.");
        return;
    };
    let parent = temp.path().join("projects");
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(parent.join("notes.txt"), "the person's file").unwrap();
    let service = Clones::default();
    let missing = file_url(&temp.path().join("missing.git"));
    let id = review_for(&service, &missing, &parent, &env);
    let (prepared, job) = service.take("main", &id).unwrap();
    execute(prepared, Arc::clone(&job), spawner()).await;
    let record = job.snapshot();
    assert_eq!(record.state, CloneState::Failed);
    assert!(record.removed, "{}", record.detail);
    assert!(
        record
            .detail
            .ends_with("The unfinished folder was removed.")
    );
    assert!(
        !record.output.is_empty(),
        "Git's own message is kept for the person"
    );
    assert_eq!(entries(&parent), ["notes.txt"]);

    // A folder someone else made after the check is never reused or removed.
    let id = review_for(&service, &missing, &parent, &env);
    let (prepared, job) = service.take("main", &id).unwrap();
    std::fs::create_dir(parent.join("clone")).unwrap();
    std::fs::write(parent.join("clone/mine.txt"), "keep").unwrap();
    execute(prepared, Arc::clone(&job), spawner()).await;
    let record = job.snapshot();
    assert_eq!(record.state, CloneState::Failed);
    assert!(!record.removed);
    assert!(record.detail.contains("already has a clone folder"));
    assert_eq!(
        std::fs::read(parent.join("clone/mine.txt")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn cancel_stops_git_and_removes_the_unfinished_folder() {
    let temp = tempfile::tempdir().unwrap();
    let Some(env) = real_git(temp.path()) else {
        eprintln!("Git is not installed; skipped.");
        return;
    };
    // A server that accepts Git's request and never answers, so the clone waits until canceled.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (connected, accepted) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
            let _ = connected.send(());
        }
    });
    let parent = temp.path().join("projects");
    std::fs::create_dir(&parent).unwrap();
    let service = Arc::new(Clones::default());
    let id = review_for(
        &service,
        &format!("http://127.0.0.1:{port}/repo.git"),
        &parent,
        &env,
    );
    let (prepared, job) = service.take("main", &id).unwrap();
    let running = tokio::spawn(execute(prepared, Arc::clone(&job), spawner()));
    tokio::task::spawn_blocking(move || accepted.recv_timeout(Duration::from_secs(30)))
        .await
        .unwrap()
        .expect("Git connected");
    assert!(
        parent.join("clone").is_dir(),
        "the owned folder exists while Git runs"
    );
    service.cancel("main", &id);
    tokio::time::timeout(Duration::from_secs(60), running)
        .await
        .expect("cancel stops the clone")
        .unwrap();
    let record = job.snapshot();
    assert_eq!(record.state, CloneState::Canceled);
    assert!(record.removed, "{}", record.detail);
    assert_eq!(
        record.detail,
        "Clone canceled. The unfinished folder was removed."
    );
    assert!(entries(&parent).is_empty());
}
