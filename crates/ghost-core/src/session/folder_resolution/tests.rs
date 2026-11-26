use super::*;

#[test]
fn discovery_is_bounded_and_does_not_descend_into_repositories_or_hidden_folders() {
    let temp = tempfile::tempdir().unwrap();
    for relative in [
        "a/.git",
        "a/nested/.git",
        ".private/repo/.git",
        "group/b/.git",
        "group/deep/c/.git",
        "too/deep/nested/d/.git",
    ] {
        std::fs::create_dir_all(temp.path().join(relative)).unwrap();
    }
    let (mut found, truncated) = candidates(vec![temp.path().into(), temp.path().join("a")]);
    found.sort();
    let mut expected = vec![
        crate::fsutil::plain_path(&temp.path().join("a").canonicalize().unwrap()),
        crate::fsutil::plain_path(&temp.path().join("group/b").canonicalize().unwrap()),
        crate::fsutil::plain_path(&temp.path().join("group/deep/c").canonicalize().unwrap()),
    ];
    expected.sort();
    assert_eq!(found, expected);
    assert!(!truncated);
    for index in 0..=MAX_REPOSITORIES {
        std::fs::create_dir_all(temp.path().join(format!("repo{index}/.git"))).unwrap();
    }
    let (found, truncated) = candidates(vec![temp.path().into()]);
    assert_eq!(found.len(), MAX_REPOSITORIES);
    assert!(truncated);
}

#[cfg(unix)]
#[test]
fn discovery_never_follows_directory_links() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outside.path().join("repo/.git")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
    assert!(candidates(vec![root.path().into()]).0.is_empty());
}

#[test]
fn roots_and_mapping_are_device_local_and_use_identity_compare_and_swap() {
    use crate::chatstore::{folder_resolution::execute, test_support::TestDb};
    let db = TestDb::new();
    let chat = db.chat("/old");
    let folder: String = db
        .conn()
        .query_row("SELECT folder_id FROM chats WHERE id=?1", [&chat], |row| {
            row.get(0)
        })
        .unwrap();
    let map = |previous: Option<&str>, remote: Option<&str>| Command::Locate {
        folder_id: folder.clone(),
        path: "/new".into(),
        previous: previous.map(Into::into),
        kind: FolderKind::Local,
        remote: remote.map(Into::into),
    };
    assert!(execute(db.conn(), "dev", 2, map(Some("/wrong"), None)).is_err());
    assert!(
        execute(
            db.conn(),
            "dev",
            2,
            map(Some("/old"), Some("https://example.com/changed"))
        )
        .is_err()
    );
    execute(db.conn(), "other", 2, map(None, None)).unwrap();
    let original: String = db
        .conn()
        .query_row(
            "SELECT path FROM folder_locations WHERE folder_id=?1 AND device_id='dev'",
            [&folder],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original, "/old");
    let root = tempfile::tempdir()
        .unwrap()
        .path()
        .to_string_lossy()
        .into_owned();
    execute(
        db.conn(),
        "dev",
        2,
        Command::Roots(Some(vec![root.clone()])),
    )
    .unwrap();
    let Reply::Roots(saved) = execute(db.conn(), "dev", 2, Command::Roots(None)).unwrap() else {
        panic!()
    };
    assert_eq!(saved, vec![root]);
    let Reply::Roots(other) = execute(db.conn(), "other", 2, Command::Roots(None)).unwrap() else {
        panic!()
    };
    assert!(other.is_empty());
    db.conn().execute("INSERT INTO agent_sessions(id,chat_id,agent_id,cwd,created_at,updated_at,device_id) VALUES('native',?1,'mock','/old',1,1,'dev')",[&chat]).unwrap();
    assert!(execute(db.conn(), "dev", 3, map(Some("/old"), None)).is_err());
    assert_eq!(
        crate::chatstore::rows::chat(db.conn(), "dev", &chat)
            .unwrap()
            .unwrap()
            .folder,
        PathBuf::from("/old")
    );
}
