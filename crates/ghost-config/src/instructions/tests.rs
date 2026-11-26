//! Edge cases, each with defined behaviour. Every test works in temporary folders; none touches a
//! real agent configuration.

use std::path::{Path, PathBuf};

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = home.join("code").join("app");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    Fixture {
        _dir: dir,
        home,
        project,
        state,
    }
}

impl Fixture {
    fn project(&self, installed: &[&str]) -> Context {
        Context {
            scope: ScopeKind::Project,
            project: Some(self.project.clone()),
            user_roots: Vec::new(),
            home: Some(self.home.clone()),
            installed: installed.iter().map(|id| (*id).to_owned()).collect(),
            git: GitFacts::default(),
            state_dir: self.state.clone(),
            codex_limit: None,
            notices: Vec::new(),
        }
    }

    fn global(&self, installed: &[&str]) -> Context {
        Context {
            scope: ScopeKind::Global,
            project: None,
            user_roots: user_roots(&self.home, |_| None),
            home: Some(self.home.clone()),
            installed: installed.iter().map(|id| (*id).to_owned()).collect(),
            git: GitFacts::default(),
            state_dir: self.state.clone(),
            codex_limit: None,
            notices: Vec::new(),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn read(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.project.join(path)).unwrap()
    }

    fn home_file(&self, path: &str) -> PathBuf {
        self.home.join(path)
    }
}

const ALL: &[&str] = &[
    "claude",
    "codex",
    "pi",
    "omp",
    "opencode",
    "hermes",
    "cursor",
    "grok",
    "antigravity",
];

fn agent<'a>(snapshot: &'a Snapshot, id: &str) -> &'a AgentInfo {
    snapshot.agents.iter().find(|agent| agent.id == id).unwrap()
}

fn file<'a>(snapshot: &'a Snapshot, key: &str) -> &'a FileInfo {
    snapshot
        .files
        .iter()
        .find(|file| file.key == key)
        .unwrap_or_else(|| {
            panic!(
                "{key} missing from {:?}",
                snapshot.files.iter().map(|f| &f.key).collect::<Vec<_>>()
            )
        })
}

fn universal(source: &str, content: Content) -> Change {
    Change::Universal {
        source: source.into(),
        content,
        remove: Vec::new(),
        agents: None,
    }
}

fn run(context: &Context, change: &Change) -> ApplyResult {
    let prepared = prepare(context, change).unwrap();
    let result = apply(prepared).unwrap();
    assert!(
        !result.rolled_back && !result.recovery_required,
        "{:?}",
        result.message
    );
    result
}

/// Case-insensitive file systems (Windows, macOS by default) find `AGENTS.md` as `agents.md`.
fn case_insensitive(dir: &Path) -> bool {
    let probe = dir.join("case-probe.txt");
    std::fs::write(&probe, b"x").unwrap();
    let found = dir.join("CASE-PROBE.TXT").exists();
    std::fs::remove_file(probe).unwrap();
    found
}

fn symlink(target: impl AsRef<Path>, link: &Path) -> bool {
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    // Windows needs Developer Mode or elevation for symlinks; those parts skip without it.
    if made.is_err() {
        eprintln!(
            "skipping a symlink case: this system refused to create {}",
            link.display()
        );
    }
    made.is_ok()
}

#[test]
fn one_agents_md_is_universal_with_no_claude_file_needed() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Use pnpm.\n");
    let snapshot = snapshot(&fixture.project(ALL)).unwrap();
    assert_eq!(snapshot.mode, Mode::Universal);
    assert_eq!(snapshot.source.as_deref(), Some("project:AGENTS.md"));
    // Claude Code 2.1.277 and later reads AGENTS.md when no CLAUDE file exists.
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::Direct);
    for id in [
        "codex",
        "pi",
        "omp",
        "opencode",
        "hermes",
        "cursor",
        "grok",
        "antigravity",
    ] {
        assert_eq!(agent(&snapshot, id).status, AgentStatus::Direct, "{id}");
    }
    assert!(
        agent(&snapshot, "codex")
            .shared_with
            .contains(&"pi".to_owned())
    );
    assert!(snapshot.conflict.is_empty());
    let empty = setup();
    let none = super::snapshot(&empty.project(ALL)).unwrap();
    assert_eq!(none.mode, Mode::None);
    assert_eq!(agent(&none, "codex").status, AgentStatus::NotSet);
    assert!(
        none.files
            .iter()
            .any(|file| file.key == "project:AGENTS.md" && file.kind == FileKind::Missing)
    );
}

#[test]
fn different_claude_and_agents_text_is_a_conflict_resolved_only_by_choice() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Shared rules.\n");
    fixture.write("CLAUDE.md", b"Claude rules.\n");
    let context = fixture.project(&["claude", "codex"]);
    let before = snapshot(&context).unwrap();
    assert_eq!(before.mode, Mode::PerAgent);
    assert_eq!(
        before.conflict,
        vec!["project:AGENTS.md", "project:CLAUDE.md"]
    );
    assert_eq!(agent(&before, "claude").status, AgentStatus::Own);
    // Combining keeps both texts; CLAUDE.md becomes an import and its old text is backed up.
    let prepared = prepare(&context, &universal("project:AGENTS.md", Content::Combine)).unwrap();
    let claude = prepared
        .review()
        .files
        .iter()
        .find(|file| file.key == "project:CLAUDE.md")
        .unwrap();
    assert_eq!(claude.action, Action::Replace);
    assert_eq!(claude.before.as_deref(), Some("Claude rules.\n"));
    assert_eq!(claude.after.as_deref(), Some("@AGENTS.md\n"));
    assert!(claude.backup);
    let result = apply(prepared).unwrap();
    assert_eq!(
        fixture.read("AGENTS.md"),
        b"Shared rules.\n\nClaude rules.\n"
    );
    assert_eq!(fixture.read("CLAUDE.md"), b"@AGENTS.md\n");
    let backup = PathBuf::from(result.backup.unwrap());
    let kept = std::fs::read_dir(&backup)
        .unwrap()
        .filter_map(|entry| std::fs::read(entry.unwrap().path()).ok())
        .any(|bytes| bytes == b"Claude rules.\n");
    assert!(kept, "the replaced CLAUDE.md text is in the backup");
    let after = snapshot(&context).unwrap();
    assert_eq!(after.mode, Mode::Universal);
    assert_eq!(agent(&after, "claude").status, AgentStatus::Import);
    // Choosing CLAUDE.md's text instead moves it into AGENTS.md.
    let second = setup();
    second.write("AGENTS.md", b"Shared rules.\n");
    second.write("CLAUDE.md", b"Claude rules.\n");
    run(
        &second.project(&["claude", "codex"]),
        &universal(
            "project:AGENTS.md",
            Content::File("project:CLAUDE.md".into()),
        ),
    );
    assert_eq!(second.read("AGENTS.md"), b"Claude rules.\n");
}

#[test]
fn a_claude_md_that_imports_or_links_to_agents_md_is_already_universal() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Rules.\n");
    fixture.write("CLAUDE.md", b"<!-- shared -->\n@AGENTS.md\n");
    let snapshot = snapshot(&fixture.project(ALL)).unwrap();
    assert_eq!(snapshot.mode, Mode::Universal);
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::Import);
    assert_eq!(
        file(&snapshot, "project:CLAUDE.md").import_of.as_deref(),
        Some("project:AGENTS.md")
    );
    // Cursor and Grok Build read the import line as text; it adds no second copy of the rules.
    assert!(agent(&snapshot, "cursor").warnings.is_empty());
    let error = prepare(
        &fixture.project(ALL),
        &universal("project:AGENTS.md", Content::Source),
    )
    .unwrap_err();
    assert!(error.to_string().contains("nothing to change"), "{error}");

    let linked = setup();
    linked.write("AGENTS.md", b"Rules.\n");
    if symlink("AGENTS.md", &linked.project.join("CLAUDE.md")) {
        let snapshot = super::snapshot(&linked.project(ALL)).unwrap();
        assert_eq!(file(&snapshot, "project:CLAUDE.md").kind, FileKind::Link);
        assert_eq!(snapshot.mode, Mode::Universal);
        assert_eq!(agent(&snapshot, "claude").status, AgentStatus::Link);
        // Saving through the link writes its target; the link stays a link.
        run(
            &linked.project(ALL),
            &Change::Save {
                file: "project:CLAUDE.md".into(),
                text: "New.\n".into(),
            },
        );
        assert_eq!(linked.read("AGENTS.md"), b"New.\n");
        assert!(
            std::fs::symlink_metadata(linked.project.join("CLAUDE.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn a_case_variant_is_read_kept_on_save_and_renamed_only_on_request() {
    let fixture = setup();
    if !case_insensitive(&fixture.project) {
        return;
    }
    fixture.write("agents.md", b"Rules.\n");
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    let agents = file(&snapshot, "project:AGENTS.md");
    assert_eq!(agents.disk_name.as_deref(), Some("agents.md"));
    assert!(agents.note.as_deref().unwrap().contains("letter case"));
    assert_eq!(agent(&snapshot, "codex").status, AgentStatus::Direct);
    run(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "Changed.\n".into(),
        },
    );
    let names = std::fs::read_dir(&fixture.project)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["agents.md"],
        "a save keeps the existing spelling"
    );
    run(
        &context,
        &Change::Rename {
            file: "project:AGENTS.md".into(),
        },
    );
    let names = std::fs::read_dir(&fixture.project)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["AGENTS.md"]);
    assert_eq!(fixture.read("AGENTS.md"), b"Changed.\n");
}

#[test]
fn an_edit_outside_auracode_after_the_review_is_refused_before_any_write() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"One.\n");
    fixture.write("CLAUDE.md", b"Two.\n");
    let context = fixture.project(&["claude", "codex"]);
    let prepared = prepare(&context, &universal("project:AGENTS.md", Content::Combine)).unwrap();
    fixture.write("AGENTS.md", b"Edited in another editor.\n");
    assert!(matches!(apply(prepared), Err(crate::Error::Stale)));
    assert_eq!(fixture.read("CLAUDE.md"), b"Two.\n");
    // A file created meanwhile counts too, even one the change does not write.
    let prepared = prepare(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "Three.\n".into(),
        },
    )
    .unwrap();
    fixture.write("AGENTS.override.md", b"New override.\n");
    assert!(matches!(apply(prepared), Err(crate::Error::Stale)));
    assert_eq!(fixture.read("AGENTS.md"), b"Edited in another editor.\n");
}

#[test]
fn global_universal_imports_for_claude_and_copies_for_agents_without_imports() {
    let fixture = setup();
    let codex = fixture.home_file(".codex/AGENTS.md");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(&codex, b"Global rules.\n").unwrap();
    let context = fixture.global(&["claude", "codex", "pi", "opencode"]);
    let before = snapshot(&context).unwrap();
    assert_eq!(before.default_source.as_deref(), Some("codex:AGENTS.md"));
    assert_eq!(agent(&before, "hermes").status, AgentStatus::Unsupported);
    assert_eq!(agent(&before, "cursor").status, AgentStatus::Unsupported);
    let prepared = prepare(&context, &universal("codex:AGENTS.md", Content::Source)).unwrap();
    let keys = prepared
        .review()
        .files
        .iter()
        .map(|file| file.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec!["claude:CLAUDE.md", "pi:AGENTS.md", "opencode:AGENTS.md"]
    );
    assert!(
        prepared
            .review()
            .files
            .iter()
            .all(|file| file.action == Action::Create)
    );
    assert!(
        prepared
            .review()
            .notes
            .iter()
            .any(|note| note.contains("session starts"))
    );
    run(&context, &universal("codex:AGENTS.md", Content::Source));
    assert_eq!(
        std::fs::read(fixture.home_file(".claude/CLAUDE.md")).unwrap(),
        b"@~/.codex/AGENTS.md\n"
    );
    assert_eq!(
        std::fs::read(fixture.home_file(".pi/agent/AGENTS.md")).unwrap(),
        b"Global rules.\n"
    );
    assert_eq!(
        std::fs::read(fixture.home_file(".config/opencode/AGENTS.md")).unwrap(),
        b"Global rules.\n"
    );
    // Agents that are not installed get nothing.
    assert!(!fixture.home_file(".omp/agent/AGENTS.md").exists());
    assert!(!fixture.home_file(".gemini/AGENTS.md").exists());
    let after = snapshot(&context).unwrap();
    assert_eq!(after.mode, Mode::Universal);
    assert_eq!(agent(&after, "claude").status, AgentStatus::Import);
    assert_eq!(agent(&after, "pi").status, AgentStatus::Copy);
    assert!(file(&after, "pi:AGENTS.md").managed.is_some());
    // The agents' folders hold ordinary files only: nothing else lands in them.
    for folder in [".claude", ".codex", ".pi/agent", ".config/opencode"] {
        let names = std::fs::read_dir(fixture.home_file(folder))
            .unwrap()
            .count();
        assert_eq!(names, 1, "{folder}");
    }
    // Saving the source carries the text to every managed copy in one change.
    run(
        &context,
        &Change::Save {
            file: "codex:AGENTS.md".into(),
            text: "Updated.\n".into(),
        },
    );
    assert_eq!(
        std::fs::read(fixture.home_file(".pi/agent/AGENTS.md")).unwrap(),
        b"Updated.\n"
    );
    assert_eq!(
        std::fs::read(fixture.home_file(".claude/CLAUDE.md")).unwrap(),
        b"@~/.codex/AGENTS.md\n"
    );
}

#[test]
fn a_hand_edited_managed_copy_shows_drift_and_can_be_restored_or_adopted() {
    let fixture = setup();
    let codex = fixture.home_file(".codex/AGENTS.md");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(&codex, b"Shared.\n").unwrap();
    let context = fixture.global(&["codex", "pi", "opencode"]);
    run(&context, &universal("codex:AGENTS.md", Content::Source));
    let pi = fixture.home_file(".pi/agent/AGENTS.md");
    std::fs::write(&pi, b"Edited for Pi.\n").unwrap();
    let drift = snapshot(&context).unwrap();
    assert_eq!(drift.mode, Mode::Universal);
    assert_eq!(agent(&drift, "pi").status, AgentStatus::Drift);
    let managed = file(&drift, "pi:AGENTS.md").managed.clone().unwrap();
    assert!(managed.edited && !managed.source_changed);
    // Saving the source leaves the edited copy alone and says so.
    let save = prepare(
        &context,
        &Change::Save {
            file: "codex:AGENTS.md".into(),
            text: "Shared 2.\n".into(),
        },
    )
    .unwrap();
    assert!(
        save.review()
            .files
            .iter()
            .all(|file| file.key != "pi:AGENTS.md")
    );
    assert!(
        save.review()
            .notes
            .iter()
            .any(|note| note.contains("edited by hand"))
    );
    drop(save);
    run(
        &context,
        &Change::Restore {
            file: "pi:AGENTS.md".into(),
        },
    );
    assert_eq!(std::fs::read(&pi).unwrap(), b"Shared.\n");
    assert_eq!(
        agent(&snapshot(&context).unwrap(), "pi").status,
        AgentStatus::Copy
    );
    std::fs::write(&pi, b"Better text.\n").unwrap();
    run(
        &context,
        &Change::Adopt {
            file: "pi:AGENTS.md".into(),
        },
    );
    assert_eq!(std::fs::read(&codex).unwrap(), b"Better text.\n");
    assert_eq!(
        std::fs::read(fixture.home_file(".config/opencode/AGENTS.md")).unwrap(),
        b"Better text.\n"
    );
    let after = snapshot(&context).unwrap();
    assert!(
        after
            .agents
            .iter()
            .all(|agent| agent.status != AgentStatus::Drift)
    );
}

#[test]
fn read_only_files_are_shown_and_never_written() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Locked.\n");
    let path = fixture.project.join("AGENTS.md");
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions.clone()).unwrap();
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    let info = file(&snapshot, "project:AGENTS.md");
    assert!(info.read_only && !info.editable);
    assert_eq!(
        info.text.as_deref(),
        Some("Locked.\n"),
        "read-only text is still shown"
    );
    let error = prepare(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "x".into(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("read-only"));
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
}

#[test]
fn bom_crlf_non_utf8_and_empty_files_keep_their_bytes() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"\xEF\xBB\xBF# Rules\r\nOne.\r\n");
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    let info = file(&snapshot, "project:AGENTS.md");
    assert_eq!(info.text.as_deref(), Some("# Rules\nOne.\n"));
    let prepared = prepare(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "# Rules\nTwo.\n".into(),
        },
    )
    .unwrap();
    assert!(
        prepared.review().files[0]
            .notes
            .iter()
            .any(|note| note.contains("CRLF"))
    );
    apply(prepared).unwrap();
    assert_eq!(
        fixture.read("AGENTS.md"),
        b"\xEF\xBB\xBF# Rules\r\nTwo.\r\n"
    );

    let other = setup();
    other.write("AGENTS.md", b"caf\xE9\n");
    other.write("CLAUDE.md", b"");
    let context = other.project(ALL);
    let snapshot = super::snapshot(&context).unwrap();
    let agents = file(&snapshot, "project:AGENTS.md");
    assert_eq!(agents.kind, FileKind::NotText);
    assert!(!agents.editable && agents.text.is_none());
    let claude = file(&snapshot, "project:CLAUDE.md");
    assert!(claude.empty && claude.editable);
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::NotSet);
    assert!(agent(&snapshot, "claude").detail.contains("empty"));
    assert!(
        prepare(
            &context,
            &Change::Save {
                file: "project:AGENTS.md".into(),
                text: "x".into()
            }
        )
        .is_err()
    );
    // An empty save is allowed, and the review says agents skip it.
    let empty = prepare(
        &context,
        &Change::Save {
            file: "project:CLAUDE.md".into(),
            text: "  \n".into(),
        },
    )
    .unwrap();
    assert!(
        empty.review().files[0]
            .notes
            .iter()
            .any(|note| note.contains("skip an empty file"))
    );
}

#[test]
fn large_files_warn_per_agent_and_oversized_ones_are_not_editable() {
    let fixture = setup();
    let line = "Keep functions small and name things plainly.\n";
    fixture.write("AGENTS.md", line.repeat(900).as_bytes());
    let snapshot = snapshot(&fixture.project(ALL)).unwrap();
    let warnings = |id: &str| agent(&snapshot, id).warnings.join(" ");
    assert!(
        warnings("codex").contains("first 32 KiB"),
        "{}",
        warnings("codex")
    );
    assert!(warnings("antigravity").contains("24,000 bytes"));
    assert!(warnings("hermes").contains("20,000"));
    assert!(warnings("claude").contains("200 lines"));
    assert!(warnings("pi").is_empty());
    let mut configured = fixture.project(ALL);
    configured.codex_limit = Some(64 * 1024);
    let roomy = super::snapshot(&configured).unwrap();
    assert!(agent(&roomy, "codex").warnings.is_empty());

    let big = setup();
    big.write("AGENTS.md", &vec![b'a'; 1024 * 1024 + 1]);
    let snapshot = super::snapshot(&big.project(ALL)).unwrap();
    let info = file(&snapshot, "project:AGENTS.md");
    assert_eq!(info.kind, FileKind::TooLarge);
    assert!(!info.editable);
    assert!(
        agent(&snapshot, "codex")
            .warnings
            .join(" ")
            .contains("first 32 KiB")
    );
}

#[test]
fn untracked_and_ignored_files_are_flagged_for_worktrees_and_grok_skips_ignored() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Rules.\n");
    fixture.write("CLAUDE.md", b"Claude.\n");
    let mut context = fixture.project(ALL);
    context.git = GitFacts {
        repository: true,
        files: [
            ("AGENTS.md".to_owned(), GitState::Untracked),
            ("CLAUDE.md".to_owned(), GitState::Ignored),
        ]
        .into(),
        worktree_branch: Some("ghost/7f3a9c2e".into()),
    };
    let snapshot = snapshot(&context).unwrap();
    assert!(
        file(&snapshot, "project:AGENTS.md")
            .note
            .as_deref()
            .unwrap()
            .contains("Not committed")
    );
    assert!(
        file(&snapshot, "project:CLAUDE.md")
            .note
            .as_deref()
            .unwrap()
            .contains("Ignored by Git")
    );
    assert_eq!(snapshot.worktree_branch.as_deref(), Some("ghost/7f3a9c2e"));
    assert!(
        agent(&snapshot, "grok")
            .warnings
            .join(" ")
            .contains("skips CLAUDE.md because Git ignores it")
    );
    assert!(
        !agent(&snapshot, "grok")
            .also
            .contains(&"project:CLAUDE.md".to_owned())
    );
}

#[test]
fn nested_and_parent_files_are_listed_and_a_parent_claude_md_changes_what_claude_reads() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Rules.\n");
    fixture.write("packages/api/AGENTS.md", b"API rules.\n");
    fixture.write("node_modules/dep/AGENTS.md", b"Ignored.\n");
    std::fs::write(fixture.home.join("code/CLAUDE.md"), b"Parent.\n").unwrap();
    std::fs::create_dir_all(fixture.home.join(".claude")).unwrap();
    std::fs::write(
        fixture.home.join(".claude/CLAUDE.md"),
        b"User file, not a parent.\n",
    )
    .unwrap();
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    assert_eq!(snapshot.nested.len(), 1);
    assert_eq!(snapshot.nested[0].path, "packages/api/AGENTS.md");
    assert!(
        snapshot
            .parents
            .iter()
            .any(|parent| parent.path == "~/code/CLAUDE.md")
    );
    assert!(
        !snapshot
            .parents
            .iter()
            .any(|parent| parent.path.ends_with(".claude/CLAUDE.md"))
    );
    assert!(
        snapshot
            .notices
            .iter()
            .any(|notice| notice.contains("~/code/CLAUDE.md"))
    );
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::NotSet);
    // Universal adds the import Claude Code needs once a parent CLAUDE.md is in the way.
    run(&context, &universal("project:AGENTS.md", Content::Source));
    assert_eq!(fixture.read("CLAUDE.md"), b"@AGENTS.md\n");
    assert_eq!(
        agent(&super::snapshot(&context).unwrap(), "claude").status,
        AgentStatus::Import
    );
}

#[test]
fn unknown_keys_traversal_and_links_outside_the_folder_are_refused() {
    let fixture = setup();
    let context = fixture.project(ALL);
    for key in [
        "project:../AGENTS.md",
        "project:src/AGENTS.md",
        "codex:AGENTS.md",
        "project:C:/x.md",
    ] {
        assert!(
            prepare(
                &context,
                &Change::Save {
                    file: key.into(),
                    text: "x".into()
                }
            )
            .is_err(),
            "{key}"
        );
    }
    let outside = fixture.home.join("elsewhere.md");
    std::fs::write(&outside, b"Outside.\n").unwrap();
    if symlink(
        outside.to_str().unwrap(),
        &fixture.project.join("AGENTS.md"),
    ) {
        let snapshot = snapshot(&context).unwrap();
        let info = file(&snapshot, "project:AGENTS.md");
        assert_eq!(info.kind, FileKind::Outside);
        assert!(!info.editable && info.text.is_none());
        assert!(
            prepare(
                &context,
                &Change::Save {
                    file: "project:AGENTS.md".into(),
                    text: "x".into()
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"Outside.\n");
    }
}

#[test]
fn an_override_hides_the_shared_file_and_is_removed_only_by_explicit_choice() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Shared.\n");
    fixture.write("AGENTS.override.md", b"Codex only.\n");
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    assert_eq!(agent(&snapshot, "codex").status, AgentStatus::Override);
    assert_eq!(agent(&snapshot, "pi").status, AgentStatus::Override);
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::Own);
    let keep = prepare(&context, &universal("project:AGENTS.md", Content::Source));
    // Keeping the override is a valid choice; the review offers its removal without doing it.
    let keep = keep.unwrap();
    assert!(
        keep.review()
            .optional_removals
            .contains(&"project:AGENTS.override.md".to_owned())
    );
    assert!(
        keep.review()
            .files
            .iter()
            .all(|file| file.action != Action::Remove)
    );
    drop(keep);
    let change = Change::Universal {
        source: "project:AGENTS.md".into(),
        content: Content::Source,
        remove: vec!["project:AGENTS.override.md".into()],
        agents: None,
    };
    let result = run(&context, &change);
    assert!(!fixture.project.join("AGENTS.override.md").exists());
    let backup = PathBuf::from(result.backup.unwrap());
    assert!(
        std::fs::read_dir(backup)
            .unwrap()
            .filter_map(|entry| std::fs::read(entry.unwrap().path()).ok())
            .any(|bytes| bytes == b"Codex only.\n")
    );
    assert_eq!(
        agent(&super::snapshot(&context).unwrap(), "codex").status,
        AgentStatus::Direct
    );
    // A file that is not in the way can't be removed through this change.
    let bad = Change::Universal {
        source: "project:AGENTS.md".into(),
        content: Content::Source,
        remove: vec!["project:AGENTS.md".into()],
        agents: None,
    };
    assert!(prepare(&context, &bad).is_err());
}

#[test]
fn per_agent_inlines_imports_replaces_links_and_deletes_nothing() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Shared.\n");
    fixture.write("CLAUDE.md", b"@AGENTS.md\n");
    let context = fixture.project(ALL);
    let prepared = prepare(&context, &Change::PerAgent).unwrap();
    assert!(
        prepared
            .review()
            .files
            .iter()
            .all(|file| file.action != Action::Remove)
    );
    apply(prepared).unwrap();
    assert_eq!(fixture.read("CLAUDE.md"), b"Shared.\n");
    assert_eq!(fixture.read("AGENTS.md"), b"Shared.\n");
    run(
        &context,
        &Change::Save {
            file: "project:CLAUDE.md".into(),
            text: "Claude only.\n".into(),
        },
    );
    let snapshot = snapshot(&context).unwrap();
    assert_eq!(snapshot.mode, Mode::PerAgent);
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::Own);
    assert_eq!(agent(&snapshot, "codex").status, AgentStatus::Own);
    // Cursor reads both files in a project, so it is told it shares Claude's file.
    assert!(
        agent(&snapshot, "cursor")
            .also
            .contains(&"project:CLAUDE.md".to_owned())
    );
    // With only AGENTS.md, Per agent gives Claude Code a CLAUDE.md of its own to start from.
    let fresh = setup();
    fresh.write("AGENTS.md", b"Shared.\n");
    run(&fresh.project(ALL), &Change::PerAgent);
    assert_eq!(fresh.read("CLAUDE.md"), b"Shared.\n");
}

#[test]
fn a_failure_part_way_puts_back_every_file() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"One.\n");
    fixture.write("CLAUDE.md", b"Two.\n");
    let context = fixture.project(&["claude", "codex"]);
    let prepared = prepare(&context, &universal("project:AGENTS.md", Content::Combine)).unwrap();
    assert_eq!(prepared.mutations.len(), 2);
    let result = apply_failing_at(prepared, 1).unwrap();
    assert!(result.rolled_back);
    assert_eq!(fixture.read("AGENTS.md"), b"One.\n");
    assert_eq!(fixture.read("CLAUDE.md"), b"Two.\n");
    // The managed-copy record is untouched by a rolled back change.
    assert_eq!(snapshot(&context).unwrap().mode, Mode::PerAgent);
}

#[test]
fn hard_linked_files_are_detected_and_saved_as_separate_files() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Same file.\n");
    if std::fs::hard_link(
        fixture.project.join("AGENTS.md"),
        fixture.project.join("CLAUDE.md"),
    )
    .is_err()
    {
        return;
    }
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    assert_eq!(
        file(&snapshot, "project:CLAUDE.md").kind,
        FileKind::HardLink
    );
    assert_eq!(snapshot.mode, Mode::Universal);
    run(
        &context,
        &Change::Save {
            file: "project:AGENTS.md".into(),
            text: "Edited.\n".into(),
        },
    );
    assert_eq!(fixture.read("AGENTS.md"), b"Edited.\n");
    assert_eq!(fixture.read("CLAUDE.md"), b"Edited.\n");
    std::fs::write(fixture.project.join("AGENTS.md"), b"Only one.\n").unwrap();
    assert_eq!(
        fixture.read("CLAUDE.md"),
        b"Edited.\n",
        "they are separate files now"
    );
}

#[test]
fn several_project_folders_are_separate_scopes_with_separate_records() {
    let fixture = setup();
    let second = fixture.home.join("code").join("site");
    std::fs::create_dir_all(&second).unwrap();
    fixture.write("AGENTS.md", b"App.\n");
    std::fs::write(second.join("AGENTS.md"), b"Site.\n").unwrap();
    std::fs::write(second.join("CLAUDE.md"), b"Site Claude.\n").unwrap();
    let app = fixture.project(ALL);
    let mut site = fixture.project(ALL);
    site.project = Some(second.clone());
    assert_eq!(snapshot(&app).unwrap().mode, Mode::Universal);
    assert_eq!(snapshot(&site).unwrap().mode, Mode::PerAgent);
    run(&site, &universal("project:AGENTS.md", Content::Source));
    assert_eq!(
        std::fs::read(second.join("CLAUDE.md")).unwrap(),
        b"@AGENTS.md\n"
    );
    assert!(!fixture.project.join("CLAUDE.md").exists());
}

#[test]
fn shared_files_are_named_and_agent_specific_files_are_offered() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Rules.\n");
    let snapshot = snapshot(&fixture.project(ALL)).unwrap();
    assert_eq!(
        agent(&snapshot, "codex").override_file.as_deref(),
        Some("project:AGENTS.override.md")
    );
    assert_eq!(
        agent(&snapshot, "claude").override_file.as_deref(),
        Some("project:CLAUDE.md")
    );
    assert_eq!(
        agent(&snapshot, "antigravity").override_file.as_deref(),
        Some("project:GEMINI.md")
    );
    assert_eq!(agent(&snapshot, "cursor").override_file, None);
    let readers = &file(&snapshot, "project:AGENTS.md").readers;
    for id in ALL {
        assert!(readers.contains(&(*id).to_owned()), "{id}");
    }
}

#[test]
fn link_classification_follows_the_resolved_target_never_the_link_text() {
    // The classifier sees only the canonical root and the OS's own resolution of the link, so
    // `a/../..` text, or a linked folder inside the target, cannot disguise where it leads.
    use model::{LinkTarget, classify_link};
    let fixture = setup();
    let root = std::fs::canonicalize(&fixture.project).unwrap();
    let inside = |path: &str| LinkTarget::Inside(path.to_owned());
    assert_eq!(
        classify_link(Some(&root), Ok(root.join("AGENTS.md"))),
        inside("AGENTS.md")
    );
    assert_eq!(
        classify_link(Some(&root), Ok(root.join(".claude").join("CLAUDE.md"))),
        inside(".claude/CLAUDE.md")
    );
    // What `a/../../AGENTS.md`, or `linked-folder/../AGENTS.md` through a folder link, resolves
    // to when the OS follows it out of the folder.
    let parent = root.parent().unwrap().to_path_buf();
    assert_eq!(
        classify_link(Some(&root), Ok(parent.join("AGENTS.md"))),
        LinkTarget::Outside
    );
    assert_eq!(
        classify_link(Some(&root), Ok(parent.join(".ssh").join("config"))),
        LinkTarget::Outside
    );
    // A sibling whose name starts with the folder's name is still outside.
    let sibling = PathBuf::from(format!("{}-other", root.display())).join("AGENTS.md");
    assert_eq!(classify_link(Some(&root), Ok(sibling)), LinkTarget::Outside);
    assert_eq!(
        classify_link(Some(&root), Ok(root.clone())),
        LinkTarget::Outside
    );
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert_eq!(
        classify_link(Some(&root), Err(missing)),
        LinkTarget::Dangling
    );
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert_eq!(classify_link(Some(&root), Err(denied)), LinkTarget::Outside);
    #[cfg(windows)]
    assert_eq!(
        classify_link(Some(&root), Err(std::io::Error::from_raw_os_error(123))),
        LinkTarget::Outside
    );
    assert_eq!(
        classify_link(None, Ok(root.join("AGENTS.md"))),
        LinkTarget::Outside
    );
}

#[test]
fn a_relative_link_through_a_linked_folder_is_classified_by_where_the_os_resolves_it() {
    let fixture = setup();
    fixture.write("AGENTS.md", b"Inside.\n");
    let outside = fixture.home.join("outside").join("inner");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        fixture.home.join("outside").join("AGENTS.md"),
        b"Outside.\n",
    )
    .unwrap();
    if !folder_link(&outside, &fixture.project.join("linked")) {
        eprintln!("skipped: this system refused to create a folder link");
        return;
    }
    // By text `linked/../AGENTS.md` is the project's AGENTS.md; the OS may follow `linked` first.
    // Windows stores a relative symlink's target verbatim, so use native separators without
    // normalizing away `..` or resolving the linked folder before the OS opens the link.
    let target = Path::new("linked").join("..").join("AGENTS.md");
    if !symlink(&target, &fixture.project.join("CLAUDE.md")) {
        eprintln!("skipped: creating a file symlink needs Developer Mode or elevation here");
        return;
    }
    let snapshot = snapshot(&fixture.project(ALL)).unwrap();
    let claude = file(&snapshot, "project:CLAUDE.md");
    let real = std::fs::canonicalize(fixture.project.join("CLAUDE.md")).unwrap();
    let root = std::fs::canonicalize(&fixture.project).unwrap();
    if real.starts_with(&root) {
        assert_eq!(claude.kind, FileKind::Link);
    } else {
        assert_eq!(claude.kind, FileKind::Outside);
        assert!(!claude.editable);
    }
}

#[test]
fn a_dangling_link_reads_as_nothing_and_is_never_written() {
    let fixture = setup();
    if !symlink("missing.md", &fixture.project.join("CLAUDE.md")) {
        eprintln!("skipped: creating a file symlink needs Developer Mode or elevation here");
        return;
    }
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    let claude = file(&snapshot, "project:CLAUDE.md");
    assert_eq!(claude.kind, FileKind::Dangling);
    assert!(!claude.editable);
    assert_eq!(agent(&snapshot, "claude").status, AgentStatus::NotSet);
    let save = Change::Save {
        file: "project:CLAUDE.md".into(),
        text: "x".into(),
    };
    assert!(prepare(&context, &save).is_err());
    assert!(!fixture.project.join("missing.md").exists());
}

#[test]
fn a_configured_codex_limit_is_read_and_a_bad_config_falls_back_to_the_default() {
    let fixture = setup();
    let codex = fixture.home.join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    assert_eq!(codex_doc_limit(&codex), None);
    std::fs::write(
        codex.join("config.toml"),
        b"model = \"x\"\nproject_doc_max_bytes = 65536\n",
    )
    .unwrap();
    assert_eq!(codex_doc_limit(&codex), Some(65536));
    std::fs::write(
        codex.join("config.toml"),
        b"project_doc_max_bytes = \"big\"\n",
    )
    .unwrap();
    assert_eq!(codex_doc_limit(&codex), None);
    std::fs::write(codex.join("config.toml"), b"[unclosed\n").unwrap();
    assert_eq!(codex_doc_limit(&codex), None);
}

/// A folder link: a junction on Windows (no privileges needed), a symlink elsewhere.
fn folder_link(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|output| output.status.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

#[cfg(windows)]
#[test]
fn a_linked_agent_folder_pointing_outside_is_never_followed() {
    // A junction needs no privileges on Windows, unlike a symbolic link.
    let fixture = setup();
    let outside = fixture.home.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("CLAUDE.md"), b"Outside.\n").unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(fixture.project.join(".claude"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "a junction needs no privileges: {} {}",
        String::from_utf8_lossy(&made.stdout),
        String::from_utf8_lossy(&made.stderr)
    );
    let context = fixture.project(ALL);
    let snapshot = snapshot(&context).unwrap();
    let info = file(&snapshot, "project:.claude/CLAUDE.md");
    assert_eq!(info.kind, FileKind::Outside);
    assert!(!info.editable && info.text.is_none());
    let save = Change::Save {
        file: "project:.claude/CLAUDE.md".into(),
        text: "x".into(),
    };
    assert!(prepare(&context, &save).is_err());
    assert_eq!(
        std::fs::read(outside.join("CLAUDE.md")).unwrap(),
        b"Outside.\n"
    );
    std::fs::remove_dir(fixture.project.join(".claude")).unwrap();
}
