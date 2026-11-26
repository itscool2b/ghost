#![allow(clippy::expect_used)]

use std::{fs, path::Path};

use ghost_config::{Error, Kind, Library, Provider, Scope, Source};

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
    fs::write(path, bytes).expect("fixture bytes");
}

fn library(a: &Path, b: &Path) -> Library {
    Library::new(vec![
        Source {
            id: "claude".into(),
            provider: Provider::Claude,
            scope: Scope::User,
            root: a.into(),
            claude_state_only: false,
        },
        Source {
            id: "pi".into(),
            provider: Provider::Pi,
            scope: Scope::User,
            root: b.into(),
            claude_state_only: false,
        },
    ])
    .expect("library")
}

#[test]
fn nested_skill_copy_reviews_the_direct_target_and_preserves_rollback() {
    let target = tempfile::tempdir().expect("target");
    let source = tempfile::tempdir().expect("source");
    write(
        source.path(),
        "skills/team/review/SKILL.md",
        b"Nested portable instructions.\n",
    );
    write(
        source.path(),
        "skills/team/review/references/check.md",
        b"Supporting context.\n",
    );
    write(
        target.path(),
        "skills/review/SKILL.md",
        b"Existing instructions.\n",
    );
    let library = library(target.path(), source.path());
    let item = library
        .inspect()
        .items
        .into_iter()
        .find(|item| item.item.key == "team/review")
        .expect("nested item");
    let prepared = library
        .prepare_copy(&item.item, &item.revision, "claude")
        .expect("review");
    assert_eq!(
        prepared.preview().before_text.as_deref(),
        Some("Existing instructions.\n")
    );
    assert!(
        prepared
            .preview()
            .files
            .iter()
            .all(|file| file.path.starts_with("skills/review/"))
    );
    let result = library.apply_copy(prepared).expect("copy");
    assert!(!result.recovery_required);
    assert_eq!(
        fs::read(target.path().join("skills/review/SKILL.md")).expect("copied"),
        b"Nested portable instructions.\n"
    );
    assert!(!target.path().join("skills/team").exists());
    library
        .rollback_copy("claude", &result.id)
        .expect("rollback");
    assert_eq!(
        fs::read(target.path().join("skills/review/SKILL.md")).expect("restored"),
        b"Existing instructions.\n"
    );
    assert!(
        !target
            .path()
            .join("skills/review/references/check.md")
            .exists()
    );
}

#[test]
fn instruction_copy_has_explicit_preview_and_persistent_idempotent_rollback() {
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "CLAUDE.md", b"New instructions.\n");
    write(b.path(), "AGENTS.md", b"Original instructions.\n");
    write(b.path(), "auth.json", b"SYNTHETIC_UNREAD_CREDENTIAL");
    let library = library(a.path(), b.path());
    let item = library.inspect().items.remove(0);
    let plan = library
        .prepare_copy(&item.item, &item.revision, "pi")
        .expect("prepare");
    assert_eq!(
        plan.preview().before_text.as_deref(),
        Some("Original instructions.\n")
    );
    assert_eq!(plan.preview().after_text, "New instructions.\n");
    assert_eq!(
        fs::read(b.path().join("AGENTS.md")).expect("read"),
        b"Original instructions.\n"
    );
    let result = library.apply_copy(plan).expect("apply");
    assert!(!result.recovery_required);
    assert_eq!(result.applied, ["AGENTS.md"]);
    assert_eq!(
        fs::read(b.path().join("AGENTS.md")).expect("read"),
        b"New instructions.\n"
    );
    drop(library);
    let restarted = self::library(a.path(), b.path());
    let receipts = restarted.copy_history().expect("history after restart");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].id, result.id);
    assert_eq!(receipts[0].files, ["AGENTS.md"]);
    assert_eq!(receipts[0].state, ghost_config::CopyState::Complete);
    assert!(
        !restarted
            .rollback_copy("pi", &result.id)
            .expect("rollback after restart")
            .recovery_required
    );
    assert_eq!(
        restarted.copy_history().expect("restored history")[0].state,
        ghost_config::CopyState::RolledBack
    );
    assert!(
        !restarted
            .rollback_copy("pi", &result.id)
            .expect("idempotent rollback")
            .recovery_required
    );
    assert_eq!(
        fs::read(b.path().join("AGENTS.md")).expect("read"),
        b"Original instructions.\n"
    );
    assert_eq!(
        fs::read(b.path().join("auth.json")).expect("fixture assertion"),
        b"SYNTHETIC_UNREAD_CREDENTIAL"
    );
}

#[test]
fn source_and_destination_changes_invalidate_the_whole_copy_before_any_write() {
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "skills/example/SKILL.md", b"Example.");
    write(a.path(), "skills/example/scripts/run.txt", b"Companion.");
    let library = library(a.path(), b.path());
    let item = library.inspect().items.remove(0);
    let plan = library
        .prepare_copy(&item.item, &item.revision, "pi")
        .expect("prepare");
    write(
        a.path(),
        "skills/example/scripts/run.txt",
        b"Changed source.",
    );
    assert!(matches!(library.apply_copy(plan), Err(Error::Stale)));
    assert!(!b.path().join("skills").exists());
    let item = library.inspect().items.remove(0);
    let plan = library
        .prepare_copy(&item.item, &item.revision, "pi")
        .expect("prepare");
    write(
        b.path(),
        "skills/example/scripts/run.txt",
        b"Concurrent destination.",
    );
    assert!(matches!(library.apply_copy(plan), Err(Error::Stale)));
    assert!(!b.path().join("skills/example/SKILL.md").exists());
    assert_eq!(
        fs::read(b.path().join("skills/example/scripts/run.txt")).expect("read"),
        b"Concurrent destination."
    );
}

#[test]
fn skill_bundle_preserves_binary_assets_and_unrelated_target_files() {
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "skills/example/SKILL.md", b"Example.");
    write(
        a.path(),
        "skills/example/assets/binary.dat",
        &[0, 255, 3, 4],
    );
    write(b.path(), "skills/example/local-note.md", b"User note.");
    let library = library(a.path(), b.path());
    let item = library
        .inspect()
        .items
        .into_iter()
        .find(|item| item.item.source_id == "claude")
        .expect("skill");
    assert_eq!(item.item.kind, Kind::Skill);
    let result = library
        .apply_copy(
            library
                .prepare_copy(&item.item, &item.revision, "pi")
                .expect("prepare"),
        )
        .expect("apply");
    assert!(!result.recovery_required);
    assert_eq!(
        fs::read(b.path().join("skills/example/assets/binary.dat")).expect("read"),
        [0, 255, 3, 4]
    );
    library.rollback_copy("pi", &result.id).expect("rollback");
    assert!(!b.path().join("skills/example/SKILL.md").exists());
    assert!(!b.path().join("skills/example/assets/binary.dat").exists());
    assert_eq!(
        fs::read(b.path().join("skills/example/local-note.md")).expect("read"),
        b"User note."
    );
}

#[test]
fn rollback_refuses_subsequent_edits_and_deletions() {
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "CLAUDE.md", b"Replacement.");
    write(b.path(), "AGENTS.md", b"Original.");
    let library = library(a.path(), b.path());
    let item = library.inspect().items.remove(0);
    let result = library
        .apply_copy(
            library
                .prepare_copy(&item.item, &item.revision, "pi")
                .expect("prepare"),
        )
        .expect("apply");
    write(b.path(), "AGENTS.md", b"New user edit.");
    assert!(matches!(
        library.rollback_copy("pi", &result.id),
        Err(Error::Stale)
    ));
    assert_eq!(
        fs::read(b.path().join("AGENTS.md")).expect("read"),
        b"New user edit."
    );
    fs::remove_file(b.path().join("AGENTS.md")).expect("fixture deletion");
    assert!(matches!(
        library.rollback_copy("pi", &result.id),
        Err(Error::Stale)
    ));
    assert!(!b.path().join("AGENTS.md").exists());
}

#[test]
fn rollback_rejects_caller_paths_and_tampered_journal_destinations() {
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "CLAUDE.md", b"Replacement.");
    let library = library(a.path(), b.path());
    let item = library.inspect().items.remove(0);
    let result = library
        .apply_copy(
            library
                .prepare_copy(&item.item, &item.revision, "pi")
                .expect("prepare"),
        )
        .expect("apply");
    assert!(matches!(
        library.rollback_copy("pi", "../../outside"),
        Err(Error::UnsafePath)
    ));
    let path = b
        .path()
        .join(".auracode-library")
        .join(&result.id)
        .join("journal.json");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("fixture journal")).expect("json");
    journal["files"][0]["path"] = "auth.json".into();
    fs::write(&path, serde_json::to_vec(&journal).expect("serialize")).expect("fixture tamper");
    assert!(matches!(
        library.rollback_copy("pi", &result.id),
        Err(Error::UnsafePath)
    ));
    assert_eq!(
        fs::read(b.path().join("AGENTS.md")).expect("read"),
        b"Replacement."
    );
}

#[cfg(unix)]
#[test]
fn executable_skill_companions_keep_execution_bit_and_mode_changes_invalidate_preview() {
    use std::os::unix::fs::PermissionsExt;
    let a = tempfile::tempdir().expect("source fixture");
    let b = tempfile::tempdir().expect("target fixture");
    write(a.path(), "skills/example/SKILL.md", b"Example.");
    write(a.path(), "skills/example/run.sh", b"#!/bin/sh\nexit 0\n");
    fs::set_permissions(
        a.path().join("skills/example/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("fixture mode");
    let library = library(a.path(), b.path());
    let item = library.inspect().items.remove(0);
    let result = library
        .apply_copy(
            library
                .prepare_copy(&item.item, &item.revision, "pi")
                .expect("prepare"),
        )
        .expect("apply");
    assert!(!result.recovery_required);
    assert_ne!(
        fs::metadata(b.path().join("skills/example/run.sh"))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o111,
        0
    );
    fs::set_permissions(
        a.path().join("skills/example/run.sh"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("fixture mode");
    assert!(matches!(
        library.preview(&item.item, &item.revision),
        Err(Error::Stale)
    ));
}
