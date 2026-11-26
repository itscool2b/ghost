use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    state: PathBuf,
    root: PathBuf,
    backup: PathBuf,
    dir: Dir,
    journal: Journal,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let root = temp.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let backup = state.join("backups").join(&id);
        std::fs::create_dir_all(&backup).unwrap();
        let dir = open_root(&root).unwrap().unwrap();
        let journal = Journal {
            version: 2,
            id,
            roots: vec![root.to_string_lossy().into_owned()],
            canonical_roots: vec![Some(
                std::fs::canonicalize(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            )],
            files: vec![],
            metadata: None,
        };
        Self {
            _temp: temp,
            state,
            root,
            backup,
            dir,
            journal,
        }
    }

    fn add(&mut self, name: &str, before: Option<&[u8]>, after: Option<&[u8]>) -> Mutation {
        let index = self.journal.files.len();
        if let Some(bytes) = before {
            self.dir.write(name, bytes).unwrap();
            write_backup(&self.backup, &format!("before-{index}"), bytes).unwrap();
        }
        self.journal.files.push(JournalFile {
            root: 0,
            path: name.into(),
            before: before.map(safe_fs::hash),
            link: None,
            after: after.map(safe_fs::hash),
            rename_from: None,
        });
        Mutation {
            root: 0,
            path: name.into(),
            expect: before
                .map(|bytes| Expect::File(bytes.to_vec()))
                .unwrap_or(Expect::Missing),
            after: after.map(<[u8]>::to_vec),
            rename_from: None,
        }
    }

    fn save(&self, ready: bool) {
        std::fs::write(
            self.backup.join("journal.json"),
            serde_json::to_vec(&self.journal).unwrap(),
        )
        .unwrap();
        if ready {
            mark(&self.backup, "backups-ready").unwrap();
        }
    }

    fn publish(&self, mutation: &Mutation) {
        assert!(publish(&self.dir, mutation, &self.journal.id).is_ok());
    }

    fn recover(&self) -> Vec<Pending> {
        recover(&self.state)
    }
}

#[test]
fn interruption_before_backups_complete_settles_without_needing_missing_backup() {
    let mut f = Fixture::new();
    f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    std::fs::remove_file(f.backup.join("before-0")).unwrap();
    f.save(false);
    assert!(f.recover().is_empty());
    assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"before");
    assert!(marked(&f.backup, "rolled-back"));
}

#[test]
fn interruption_after_original_move_restores_original_and_cleans_only_exact_siblings() {
    let mut f = Fixture::new();
    f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    f.save(true);
    let old = temp_name("AGENTS.md", &f.journal.id, "old");
    let tmp = temp_name("AGENTS.md", &f.journal.id, "tmp");
    write_new(&f.dir, &tmp, b"after").unwrap();
    f.dir.rename("AGENTS.md", &f.dir, &old).unwrap();
    f.dir.write(".unrelated.auracode.tmp", b"keep").unwrap();
    assert!(f.recover().is_empty());
    assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"before");
    assert_eq!(f.dir.read(".unrelated.auracode.tmp").unwrap(), b"keep");
    assert!(f.dir.symlink_metadata(tmp).is_err());
    assert!(f.recover().is_empty());
}

#[test]
fn interruption_after_each_publication_rolls_back_create_replace_and_remove() {
    for count in 0..=3 {
        let mut f = Fixture::new();
        let mutations = [
            f.add("AGENTS.md", Some(b"before"), Some(b"after")),
            f.add("CLAUDE.md", None, Some(b"new")),
            f.add("GEMINI.md", Some(b"removed"), None),
        ];
        f.save(true);
        for mutation in &mutations[..count] {
            f.publish(mutation);
        }
        assert!(f.recover().is_empty(), "{count}");
        assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"before");
        assert_eq!(f.dir.read("GEMINI.md").unwrap(), b"removed");
        assert!(f.dir.symlink_metadata("CLAUDE.md").is_err());
    }
}

#[test]
fn external_destination_and_sibling_edits_are_preserved_and_remain_pending() {
    for sibling in [false, true] {
        let mut f = Fixture::new();
        let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
        f.save(true);
        f.publish(&mutation);
        let name = if sibling {
            temp_name("AGENTS.md", &f.journal.id, "old")
        } else {
            "AGENTS.md".into()
        };
        f.dir.write(&name, b"external").unwrap();
        assert_eq!(f.recover().len(), 1);
        assert_eq!(f.dir.read(&name).unwrap(), b"external");
        assert!(!prunable(&f.backup));
        assert_eq!(f.recover().len(), 1);
        assert!(notices(&f.state)[0].contains(&f.backup.to_string_lossy().to_string()));
    }
}

fn entry(source: &str) -> crate::instructions::manifest::Entry {
    crate::instructions::manifest::Entry {
        source: source.into(),
        source_hash: "hash".into(),
        copies: Default::default(),
    }
}

#[test]
fn committed_files_finish_metadata_across_cleanup_and_metadata_boundaries() {
    for boundary in 0..3 {
        let mut f = Fixture::new();
        let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
        f.journal.metadata = Some(MetadataIntent {
            identity: "user".into(),
            before: None,
            after: Some(entry("AGENTS.md")),
        });
        f.save(true);
        f.publish(&mutation);
        mark(&f.backup, "files-committed").unwrap();
        if boundary >= 1 {
            cleanup(
                &f.dir,
                &temp_name("AGENTS.md", &f.journal.id, "old"),
                f.journal.files[0].before.as_ref(),
                None,
            )
            .unwrap();
        }
        if boundary == 2 {
            let mut manifest = crate::instructions::manifest::Manifest::default();
            manifest.roots.insert("user".into(), entry("AGENTS.md"));
            manifest.save(&f.state).unwrap();
        }
        assert!(f.recover().is_empty());
        assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"after");
        let manifest = crate::instructions::manifest::Manifest::load_checked(&f.state).unwrap();
        assert_eq!(manifest.roots["user"], entry("AGENTS.md"));
        assert!(marked(&f.backup, "metadata-committed"));
        assert!(prunable(&f.backup));
        assert!(f.recover().is_empty());
    }
}

#[test]
fn external_or_malformed_metadata_is_not_overwritten() {
    for malformed in [false, true] {
        let mut f = Fixture::new();
        f.journal.metadata = Some(MetadataIntent {
            identity: "user".into(),
            before: None,
            after: Some(entry("ours")),
        });
        f.save(true);
        mark(&f.backup, "files-committed").unwrap();
        let bytes = if malformed {
            b"{broken".to_vec()
        } else {
            let mut manifest = crate::instructions::manifest::Manifest {
                version: 1,
                ..Default::default()
            };
            manifest.roots.insert("user".into(), entry("external"));
            serde_json::to_vec(&manifest).unwrap()
        };
        std::fs::write(f.state.join("managed.json"), &bytes).unwrap();
        assert_eq!(f.recover().len(), 1);
        assert_eq!(std::fs::read(f.state.join("managed.json")).unwrap(), bytes);
    }
}

#[test]
fn missing_root_and_damaged_backup_keep_evidence() {
    for missing_root in [false, true] {
        let mut f = Fixture::new();
        let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
        f.save(true);
        f.publish(&mutation);
        if missing_root {
            // Windows cannot move a directory while this fixture holds its capability open.
            f.dir = open_root(&f.state).unwrap().unwrap();
            std::fs::rename(&f.root, f.root.with_file_name("moved")).unwrap();
        } else {
            std::fs::write(f.backup.join("before-0"), b"damaged").unwrap();
        }
        assert_eq!(f.recover().len(), 1);
        assert!(!prunable(&f.backup));
    }
}

#[test]
fn malformed_and_legacy_completed_journals_are_never_pruned() {
    let mut f = Fixture::new();
    f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    f.journal.version = 1;
    f.save(true);
    mark(&f.backup, "complete").unwrap();
    assert_eq!(f.recover().len(), 1);
    assert!(!prunable(&f.backup));
    std::fs::write(f.backup.join("journal.json"), b"{broken").unwrap();
    mark(&f.backup, "settled").unwrap();
    assert_eq!(f.recover().len(), 1);
    assert!(!prunable(&f.backup));
}

#[test]
fn malformed_journal_with_target_hints_still_blocks_only_its_target() {
    let mut f = Fixture::new();
    f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    f.journal.version = 99;
    f.save(false);
    let pending = f.recover();
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0]
            .targets
            .iter()
            .any(|target| overlaps(target, &f.root.join("AGENTS.md")))
    );
    assert!(
        !pending[0]
            .targets
            .iter()
            .any(|target| overlaps(target, &f.root.join("CLAUDE.md")))
    );
}

fn metadata_review(state: &Path, identity: &str) -> crate::instructions::Prepared {
    crate::instructions::Prepared {
        review: crate::instructions::Review {
            id: uuid::Uuid::now_v7().to_string(),
            title: "Metadata fixture".into(),
            summary: String::new(),
            files: vec![],
            notes: vec![],
            optional_removals: vec![],
        },
        roots: vec![],
        mutations: vec![],
        dependencies: vec![],
        manifest: Some(MetadataIntent {
            identity: identity.into(),
            before: None,
            after: Some(entry("new")),
        }),
        state_dir: state.to_path_buf(),
        expires: std::time::Instant::now() + std::time::Duration::from_secs(600),
    }
}

#[test]
fn malformed_metadata_only_journal_blocks_same_entry_but_allows_unrelated_entry() {
    for bad_version in [false, true] {
        let mut f = Fixture::new();
        f.journal.metadata = Some(MetadataIntent {
            identity: "user".into(),
            before: None,
            after: Some(entry("pending")),
        });
        if bad_version {
            f.journal.version = 99;
        } else {
            f.journal.canonical_roots.clear();
        }
        f.save(true);
        mark(&f.backup, "files-committed").unwrap();
        let journal_before = std::fs::read(f.backup.join("journal.json")).unwrap();
        let pending = f.recover();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].targets.is_empty());
        assert_eq!(pending[0].identity.as_deref(), Some("user"));
        let blocked = crate::instructions::apply(metadata_review(&f.state, "user")).unwrap();
        assert!(blocked.recovery_required);
        assert!(!f.state.join("managed.json").exists());
        let unrelated = std::fs::canonicalize(&f.root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let result = crate::instructions::apply(metadata_review(&f.state, &unrelated)).unwrap();
        assert!(!result.recovery_required);
        let manifest = crate::instructions::manifest::Manifest::load_checked(&f.state).unwrap();
        assert_eq!(manifest.roots[&unrelated], entry("new"));
        assert!(!manifest.roots.contains_key("user"));
        assert_eq!(
            std::fs::read(f.backup.join("journal.json")).unwrap(),
            journal_before
        );
        assert_eq!(f.recover().len(), 1);
        assert!(!prunable(&f.backup));
    }
}

#[test]
fn invalid_metadata_identity_hints_are_not_accepted() {
    let f = Fixture::new();
    for identity in [
        serde_json::json!(5),
        serde_json::json!("../relative"),
        serde_json::json!("user\0"),
    ] {
        let value = serde_json::json!({ "metadata": { "identity": identity } });
        std::fs::write(
            f.backup.join("journal.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(identity_hint(&f.backup), None);
    }
}

#[test]
fn contradictory_commit_and_rollback_markers_preserve_evidence_and_block_overlap() {
    for settled in [false, true] {
        let mut f = Fixture::new();
        let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
        f.journal.metadata = Some(MetadataIntent {
            identity: "user".into(),
            before: None,
            after: Some(entry("pending")),
        });
        f.save(true);
        f.publish(&mutation);
        mark(&f.backup, "files-committed").unwrap();
        mark(&f.backup, "rolled-back").unwrap();
        if settled {
            mark(&f.backup, "settled").unwrap();
        }
        let evidence = |path: &Path| {
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect();
            entries.sort();
            entries
        };
        let journal_before = evidence(&f.backup);
        let files_before = evidence(&f.root);
        assert!(resolve(&f.backup, &f.journal).is_err());
        for _ in 0..2 {
            assert_eq!(f.recover().len(), 1);
            let blocked = crate::instructions::apply(metadata_review(&f.state, "user")).unwrap();
            assert!(blocked.recovery_required);
            let mut file_review = metadata_review(&f.state, "user");
            file_review.manifest = None;
            file_review.roots = vec![f.root.clone()];
            file_review.mutations = vec![Mutation {
                root: 0,
                path: "AGENTS.md".into(),
                expect: Expect::File(b"after".to_vec()),
                after: Some(b"later".to_vec()),
                rename_from: None,
            }];
            assert!(
                crate::instructions::apply(file_review)
                    .unwrap()
                    .recovery_required
            );
            assert!(!f.state.join("managed.json").exists());
            assert_eq!(evidence(&f.backup), journal_before);
            assert_eq!(evidence(&f.root), files_before);
            assert!(!prunable(&f.backup));
        }
    }
}

#[test]
fn malformed_phase_marker_is_retained_without_recovery_writes() {
    let f = Fixture::new();
    f.save(true);
    std::fs::write(f.backup.join("files-committed"), b"truncated").unwrap();
    assert!(resolve(&f.backup, &f.journal).is_err());
    assert_eq!(f.recover().len(), 1);
    assert!(!f.backup.join("recovered").exists());
    assert!(!prunable(&f.backup));
    assert_eq!(
        std::fs::read(f.backup.join("files-committed")).unwrap(),
        b"truncated"
    );
}

#[test]
fn unresolved_target_blocks_overlap_but_allows_a_separate_reviewed_write() {
    use crate::instructions::{Change, Context, GitFacts, ScopeKind};
    let mut f = Fixture::new();
    let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    f.save(true);
    f.publish(&mutation);
    f.dir.remove_file("AGENTS.md").unwrap();
    f.dir.write("AGENTS.md", b"external").unwrap();
    let context = Context {
        scope: ScopeKind::Project,
        project: Some(f.root.clone()),
        user_roots: vec![],
        home: None,
        installed: vec!["codex".into(), "claude".into()],
        git: GitFacts::default(),
        state_dir: f.state.clone(),
        codex_limit: None,
        notices: vec![],
    };
    let review = crate::instructions::prepare(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "wanted".into(),
        },
    )
    .unwrap();
    let result = crate::instructions::apply(review).unwrap();
    assert!(result.recovery_required);
    assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"external");
    let review = crate::instructions::prepare(
        &context,
        &Change::Save {
            file: "project:CLAUDE.md".into(),
            text: "separate".into(),
        },
    )
    .unwrap();
    let result = crate::instructions::apply(review).unwrap();
    assert!(!result.recovery_required);
    assert_eq!(f.dir.read("CLAUDE.md").unwrap(), b"separate");
    assert_eq!(f.recover().len(), 1);
}

#[test]
fn retention_above_forty_keeps_every_unresolved_and_malformed_journal() {
    let f = Fixture::new();
    f.save(false);
    let backups = f.state.join("backups");
    let mut retained = vec![f.backup.clone()];
    for index in 0..48 {
        let id = uuid::Uuid::now_v7().to_string();
        let path = backups.join(&id);
        std::fs::create_dir(&path).unwrap();
        if index % 3 == 0 {
            write_backup(&path, "journal.json", b"{broken").unwrap();
            retained.push(path);
        } else {
            let journal = Journal {
                version: 2,
                id,
                roots: vec![],
                canonical_roots: vec![],
                files: vec![],
                metadata: None,
            };
            write_backup(
                &path,
                "journal.json",
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
            mark(&path, "rolled-back").unwrap();
            mark(&path, "settled").unwrap();
        }
    }
    for _ in 0..45 {
        let id = uuid::Uuid::now_v7().to_string();
        let path = backups.join(&id);
        std::fs::create_dir(&path).unwrap();
        let journal = Journal {
            version: 2,
            id,
            roots: vec![],
            canonical_roots: vec![],
            files: vec![],
            metadata: None,
        };
        write_backup(
            &path,
            "journal.json",
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        mark(&path, "rolled-back").unwrap();
        mark(&path, "settled").unwrap();
    }
    prune(&backups, "none");
    assert!(retained.iter().all(|path| path.exists()));
    assert_eq!(
        std::fs::read_dir(&backups).unwrap().count(),
        retained.len() + KEEP_BACKUPS
    );
}

#[test]
fn legacy_staged_replacement_rolls_back_only_with_exact_evidence() {
    let mut f = Fixture::new();
    let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    f.journal.version = 1;
    f.save(false);
    f.publish(&mutation);
    assert!(f.recover().is_empty());
    assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"before");
    assert!(prunable(&f.backup));
}

#[test]
fn case_only_rename_restores_original_spelling() {
    let mut f = Fixture::new();
    let mut mutation = f.add("agents.md", Some(b"before"), Some(b"before"));
    mutation.path = "AGENTS.md".into();
    mutation.rename_from = Some("agents.md".into());
    f.journal.files[0].path = mutation.path.clone();
    f.journal.files[0].rename_from = mutation.rename_from.clone();
    f.save(true);
    f.publish(&mutation);
    assert!(f.recover().is_empty());
    let names = safe_fs::children(&f.dir, "").unwrap();
    assert!(names.contains(&"agents.md".into()));
    assert!(!names.contains(&"AGENTS.md".into()));
}

#[test]
fn publication_never_overwrites_an_existing_original_sibling() {
    let mut f = Fixture::new();
    let mutation = f.add("AGENTS.md", Some(b"before"), Some(b"after"));
    let old = temp_name("AGENTS.md", &f.journal.id, "old");
    f.dir.write(&old, b"external").unwrap();
    assert!(publish(&f.dir, &mutation, &f.journal.id).is_err());
    assert_eq!(f.dir.read(&old).unwrap(), b"external");
    assert_eq!(f.dir.read("AGENTS.md").unwrap(), b"before");
}

#[cfg(unix)]
#[test]
fn interrupted_link_replacement_restores_link_without_touching_target() {
    let mut f = Fixture::new();
    f.dir.write("target.md", b"target").unwrap();
    std::os::unix::fs::symlink("target.md", f.root.join("AGENTS.md")).unwrap();
    f.journal.files.push(JournalFile {
        root: 0,
        path: "AGENTS.md".into(),
        before: None,
        link: Some("target.md".into()),
        after: Some(safe_fs::hash(b"after")),
        rename_from: None,
    });
    f.save(true);
    f.publish(&Mutation {
        root: 0,
        path: "AGENTS.md".into(),
        expect: Expect::Link("target.md".into()),
        after: Some(b"after".to_vec()),
        rename_from: None,
    });
    assert!(f.recover().is_empty());
    assert_eq!(
        current(&f.dir, "AGENTS.md").unwrap(),
        Expect::Link("target.md".into())
    );
    assert_eq!(f.dir.read("target.md").unwrap(), b"target");
}
