//! Cache reuse must preserve the same private objects and bytes as a cold capture.

use super::*;

async fn capture(rig: &Rig) -> Manifest {
    let summary = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("capture");
    rig.service
        .load(&rig.chat, &rig.folder, &rig.work, &summary.id)
        .expect("manifest")
}

fn repository(rig: &Rig) -> PathBuf {
    rig.service.root.join(format!("{}.git", rig.folder))
}

/// Read objects independently of the cache and manifest readers under test.
async fn private_git(rig: &Rig, args: &[&str]) -> Vec<u8> {
    let program = ghost_process::find_exe("git", &process_env()).expect("test Git");
    let mut command = vec!["--git-dir".into(), repository(rig).into_os_string()];
    command.extend(args.iter().map(|arg| (*arg).into()));
    let output = ghost_process::run_helper(
        &LeftoverRegistry::disabled(),
        ghost_process::HelperSpec {
            program,
            args: command,
            cwd: rig.work.clone(),
            env: ghost_process::HelperEnv::ProcessEnv,
            set: vec![],
            new_session: true,
            timeout: std::time::Duration::from_secs(10),
            max_stdout: 1024 * 1024,
        },
    )
    .await
    .expect("read private Git object");
    assert_eq!(output.exit.and_then(|exit| exit.code), Some(0));
    output.stdout
}

async fn tree(rig: &Rig, manifest: &Manifest) -> String {
    let revision = format!("{}^{{tree}}", manifest.commit);
    git::oid(&private_git(rig, &["rev-parse", &revision]).await).expect("tree id")
}

async fn bytes(rig: &Rig, manifest: &Manifest, path: &str) -> Vec<u8> {
    private_git(rig, &["cat-file", "blob", &manifest.files[path].oid]).await
}

fn loose_object(rig: &Rig, oid: &str) -> PathBuf {
    repository(rig)
        .join("objects")
        .join(&oid[..2])
        .join(&oid[2..])
}

fn remove_loose_object(rig: &Rig, oid: &str) -> PathBuf {
    let path = loose_object(rig, oid);
    #[cfg(windows)]
    {
        let mut permissions = std::fs::metadata(&path).expect("object").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // Windows readonly attribute, not Unix mode bits.
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).expect("writable fixture object");
    }
    std::fs::remove_file(&path).expect("remove fixture object");
    path
}

#[tokio::test]
async fn warm_capture_preserves_binary_and_crlf_bytes_with_a_distinct_checkpoint() {
    let rig = rig();
    let text = b"first\r\nsecond\r\n";
    let binary = b"a\0b\nc\xff";
    std::fs::write(rig.work.join("text.txt"), text).expect("text");
    std::fs::write(rig.work.join("binary.dat"), binary).expect("binary");
    let cold = capture(&rig).await;
    let warm = capture(&rig).await;

    assert_ne!(cold.summary.id, warm.summary.id);
    assert_ne!(cold.summary.turn_id, warm.summary.turn_id);
    assert_eq!(cold.files, warm.files);
    assert_eq!(tree(&rig, &cold).await, tree(&rig, &warm).await);
    assert_eq!(warm.summary.file_count, 2);
    assert_eq!(warm.summary.total_bytes, (text.len() + binary.len()) as u32);
    for manifest in [&cold, &warm] {
        assert_eq!(bytes(&rig, manifest, "text.txt").await, text);
        assert_eq!(bytes(&rig, manifest, "binary.dat").await, binary);
    }
    assert!(!rig.work.join(".git").exists());
}

#[tokio::test]
async fn same_length_edits_are_hashed_even_when_the_original_mtime_is_restored() {
    let rig = rig();
    let path = rig.work.join("same-size.txt");
    std::fs::write(&path, b"before\r\n").expect("before");
    let modified = std::fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    let cold = capture(&rig).await;
    std::fs::write(&path, b"edited\r\n").expect("edit with the same length");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open edited file");
    match file.set_modified(modified) {
        Ok(()) => assert_eq!(
            file.metadata().expect("metadata").modified().unwrap(),
            modified
        ),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::Unsupported),
    }
    drop(file);
    let warm = capture(&rig).await;

    assert_eq!(cold.summary.total_bytes, warm.summary.total_bytes);
    assert_ne!(
        cold.files["same-size.txt"].oid,
        warm.files["same-size.txt"].oid
    );
    assert_ne!(tree(&rig, &cold).await, tree(&rig, &warm).await);
    assert_eq!(bytes(&rig, &cold, "same-size.txt").await, b"before\r\n");
    assert_eq!(bytes(&rig, &warm, "same-size.txt").await, b"edited\r\n");
}

#[tokio::test]
async fn warm_capture_observes_added_deleted_and_newly_ignored_paths() {
    let rig = rig();
    std::fs::write(rig.work.join(".gitignore"), "").expect("ignore");
    for path in ["kept.txt", "deleted.txt", "excluded.txt"] {
        std::fs::write(rig.work.join(path), path).expect("fixture");
    }
    let cold = capture(&rig).await;
    std::fs::remove_file(rig.work.join("deleted.txt")).expect("delete");
    std::fs::write(rig.work.join("added.txt"), b"new\r\n").expect("add");
    std::fs::write(rig.work.join(".gitignore"), "excluded.txt\nignored/\n").expect("ignore edit");
    std::fs::create_dir(rig.work.join("ignored")).expect("ignored directory");
    std::fs::write(rig.work.join("ignored/private.txt"), "excluded").expect("ignored file");
    let warm = capture(&rig).await;

    assert_eq!(cold.files.len(), 4);
    assert_eq!(
        warm.files.keys().map(String::as_str).collect::<Vec<_>>(),
        [".gitignore", "added.txt", "kept.txt"]
    );
    assert_eq!(cold.files["kept.txt"], warm.files["kept.txt"]);
    assert_eq!(bytes(&rig, &warm, "added.txt").await, b"new\r\n");
    assert_eq!(bytes(&rig, &cold, "deleted.txt").await, b"deleted.txt");
    assert!(rig.work.join("excluded.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn warm_capture_updates_executable_mode_without_changing_the_blob() {
    use std::os::unix::fs::PermissionsExt;

    let rig = rig();
    let path = rig.work.join("run.sh");
    std::fs::write(&path, b"#!/bin/sh\necho test\n").expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
    let cold = capture(&rig).await;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let warm = capture(&rig).await;

    assert_eq!(cold.files["run.sh"].oid, warm.files["run.sh"].oid);
    assert_eq!(cold.files["run.sh"].fingerprint.mode, 0o100644);
    assert_eq!(warm.files["run.sh"].fingerprint.mode, 0o100755);
    assert_ne!(tree(&rig, &cold).await, tree(&rig, &warm).await);
    assert_eq!(
        private_git(&rig, &["ls-tree", &warm.commit, "--", "run.sh"]).await,
        format!("100755 blob {}\trun.sh\n", warm.files["run.sh"].oid).as_bytes()
    );
}

async fn missing_object_recovers(remove_tree: bool) {
    let rig = rig();
    std::fs::write(rig.work.join("saved.txt"), b"unchanged\r\n").expect("fixture");
    let cold = capture(&rig).await;
    let original_tree = tree(&rig, &cold).await;
    let oid = if remove_tree {
        original_tree.clone()
    } else {
        cold.files["saved.txt"].oid.clone()
    };
    let object = remove_loose_object(&rig, &oid);
    assert!(!object.exists());
    let recovered = capture(&rig).await;

    assert!(object.exists());
    assert_eq!(cold.files, recovered.files);
    assert_eq!(tree(&rig, &recovered).await, original_tree);
    assert_eq!(bytes(&rig, &recovered, "saved.txt").await, b"unchanged\r\n");
    assert_ne!(cold.summary.id, recovered.summary.id);
}

#[tokio::test]
async fn missing_cached_blob_falls_back_to_a_complete_cold_capture() {
    missing_object_recovers(false).await;
}

#[tokio::test]
async fn missing_cached_tree_falls_back_to_a_complete_cold_capture() {
    missing_object_recovers(true).await;
}

#[tokio::test]
async fn missing_cached_nested_tree_falls_back_to_a_complete_cold_capture() {
    let rig = rig();
    std::fs::create_dir_all(rig.work.join("nested/deeper")).expect("nested directory");
    std::fs::write(rig.work.join("nested/deeper/saved.txt"), b"nested\r\n").expect("fixture");
    let cold = capture(&rig).await;
    let root_tree = tree(&rig, &cold).await;
    let subtree_revision = format!("{}:nested/deeper", cold.commit);
    let subtree =
        git::oid(&private_git(&rig, &["rev-parse", &subtree_revision]).await).expect("subtree id");
    let object = remove_loose_object(&rig, &subtree);
    let recovered = capture(&rig).await;

    assert!(object.exists(), "a missing subtree must be rebuilt");
    assert_eq!(cold.files, recovered.files);
    assert_eq!(tree(&rig, &recovered).await, root_tree);
    assert_eq!(
        bytes(&rig, &recovered, "nested/deeper/saved.txt").await,
        b"nested\r\n"
    );
}

async fn corrupt_object_is_rejected(corrupt_tree: bool) {
    let rig = rig();
    std::fs::write(rig.work.join("saved.txt"), b"must survive\r\n").expect("fixture");
    let cold = capture(&rig).await;
    let oid = if corrupt_tree {
        tree(&rig, &cold).await
    } else {
        cold.files["saved.txt"].oid.clone()
    };
    let object = remove_loose_object(&rig, &oid);
    std::fs::write(&object, b"not a valid loose Git object").expect("corrupt fixture object");
    let manifest = repository(&rig)
        .join("auracode")
        .join(format!("{}.json", cold.summary.id));
    let saved_manifest = std::fs::read(&manifest).expect("original manifest");

    for attempt in 0..2 {
        assert!(
            rig.service
                .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
                .await
                .is_err(),
            "corruption must fail both warm capture and a cold retry after cache invalidation (attempt {attempt})"
        );
        assert_eq!(
            std::fs::read_dir(repository(&rig).join("auracode"))
                .expect("manifests")
                .count(),
            1,
            "neither the initial failure nor its cold retry may publish a checkpoint"
        );
    }
    assert_eq!(std::fs::read(&manifest).expect("manifest"), saved_manifest);
    assert_eq!(
        std::fs::read_dir(repository(&rig).join("auracode"))
            .expect("manifests")
            .count(),
        1,
        "a failed capture must not publish another checkpoint"
    );
    assert_eq!(
        std::fs::read(rig.work.join("saved.txt")).unwrap(),
        b"must survive\r\n"
    );
    assert!(
        rig.service
            .cache
            .lock()
            .unwrap()
            .take(&rig.folder, &std::fs::canonicalize(&rig.work).unwrap())
            .is_none(),
        "a failed capture must not republish its cached hint"
    );
}

#[tokio::test]
async fn corrupt_cached_blob_remains_a_capture_error() {
    corrupt_object_is_rejected(false).await;
}

#[tokio::test]
async fn corrupt_cached_tree_remains_a_capture_error() {
    corrupt_object_is_rejected(true).await;
}

#[tokio::test]
async fn a_restarted_service_captures_the_same_exact_bytes_without_live_cache_state() {
    let mut rig = rig();
    std::fs::write(rig.work.join("saved.txt"), b"restart\r\n\0\xff").expect("fixture");
    let first = capture(&rig).await;
    rig.service = Checkpoints::new(
        &rig.profile,
        Arc::clone(&rig.service.env),
        LeftoverRegistry::disabled(),
    );
    assert!(
        rig.service
            .cache
            .lock()
            .unwrap()
            .take(&rig.folder, &std::fs::canonicalize(&rig.work).unwrap())
            .is_none(),
        "a new service starts without hints from the previous process"
    );
    let restarted = capture(&rig).await;

    assert_ne!(first.summary.id, restarted.summary.id);
    assert_eq!(first.files, restarted.files);
    assert_eq!(tree(&rig, &first).await, tree(&rig, &restarted).await);
    assert_eq!(
        bytes(&rig, &restarted, "saved.txt").await,
        b"restart\r\n\0\xff"
    );
}

#[test]
fn cache_keeps_only_eight_recent_folder_root_pairs() {
    let mut cache = super::super::cache::Cache::default();
    let root = Path::new("workspace");
    let tree = "a".repeat(40);
    for index in 0..8 {
        cache.publish(
            &format!("folder-{index}"),
            root,
            BTreeMap::new(),
            tree.clone(),
            Arc::new(BTreeMap::new()),
        );
    }
    // Successful reuse republishes its entry at the front, without creating a duplicate.
    let reused = cache.take("folder-0", root).expect("first folder");
    cache.publish(
        "folder-0",
        root,
        reused.files.clone(),
        reused.tree.clone(),
        Arc::clone(&reused.tree_ids),
    );
    cache.publish(
        "folder-8",
        root,
        BTreeMap::new(),
        tree,
        Arc::new(BTreeMap::new()),
    );

    assert!(
        cache.take("folder-1", root).is_none(),
        "least recent entry was evicted"
    );
    for index in [0, 2, 3, 4, 5, 6, 7, 8] {
        assert!(cache.take(&format!("folder-{index}"), root).is_some());
    }
}

#[test]
fn cancelled_scan_keeps_its_scratch_and_lease_until_the_blocking_worker_exits() {
    use futures::FutureExt;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let rig = rig();
        std::fs::write(rig.work.join("saved.txt"), b"unchanged").expect("fixture");
        let root = std::fs::canonicalize(&rig.work).expect("workspace");
        let owner = coordination::acquire(&rig.service.root, &rig.folder, &root)
            .await
            .expect("lease");
        let operation = rig
            .service
            .operation(&rig.folder, &root, true, owner.context())
            .await
            .expect("operation");
        let scratch = operation.scratch.path().to_owned();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(10))
                .expect("release blocking worker");
        });
        started.await.expect("blocking worker started");

        let paths = vec!["saved.txt".into()];
        let mut scan = Box::pin(Checkpoints::capture_scan(
            &operation,
            &root,
            &paths,
            None,
            "blocked-scan",
        ));
        assert!(scan.as_mut().now_or_never().is_none());
        drop(scan);
        drop(operation);
        drop(owner);

        assert!(scratch.exists(), "the queued worker still owns its scratch");
        let mut next = Box::pin(coordination::acquire(&rig.service.root, &rig.folder, &root));
        assert!(
            next.as_mut().now_or_never().is_none(),
            "the worker still owns its lease"
        );
        release.send(()).expect("release worker");
        blocker.await.expect("blocking worker exited");
        let admitted = tokio::time::timeout(std::time::Duration::from_secs(5), next)
            .await
            .expect("cancelled scan released the lease")
            .expect("next lease");
        assert!(!scratch.exists(), "worker exit removed its scratch");
        assert_eq!(
            std::fs::read(rig.work.join("saved.txt")).unwrap(),
            b"unchanged"
        );
        drop(admitted);
    });
}
