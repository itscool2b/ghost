#![allow(clippy::expect_used)]
use ghost_config::{CopyState, Error, Kind, Library, Provider, Scope, Source};
use std::{fs, path::Path};

fn source(id: &str, provider: Provider, root: &Path) -> Source {
    Source {
        id: id.into(),
        provider,
        scope: Scope::User,
        root: root.into(),
        claude_state_only: false,
    }
}

fn server(library: &Library, source: &str) -> ghost_config::Item {
    library
        .inspect()
        .items
        .into_iter()
        .find(|item| {
            item.item.source_id == source
                && item.item.kind == Kind::McpServer
                && item.item.key == "docs"
        })
        .expect("synthetic docs server")
}

#[test]
fn copy_and_restart_restore_only_the_selected_entry_preserving_other_edits_and_private_values() {
    let a = tempfile::tempdir().expect("source");
    let b = tempfile::tempdir().expect("target");
    fs::write(
        a.path().join("mcp.json"),
        r#"{"mcpServers":{"docs":{"command":"node","args":["docs.js"]}}}"#,
    )
    .expect("source");
    let original = "# unchanged comment\nmodel='old'\n[mcp_servers.docs]\ncommand='old-command'\n[mcp_servers.private]\ncommand='other'\nenv={API_KEY='SYNTHETIC_PRIVATE'}\n[projects.'private-path']\ntrust_level='trusted'\n";
    fs::write(b.path().join("config.toml"), original).expect("target");
    let sources = vec![
        source("pi", Provider::Pi, a.path()),
        source("codex", Provider::Codex, b.path()),
    ];
    let library = Library::new(sources.clone()).expect("library");
    let item = server(&library, "pi");
    let plan = library
        .prepare_copy(&item.item, &item.revision, "codex")
        .expect("prepare");
    let preview = serde_json::to_string(plan.preview()).expect("preview");
    for excluded in ["PRIVATE", "private-path", "trust_level", "model"] {
        assert!(!preview.contains(excluded));
    }
    let result = library.apply_copy(plan).expect("apply");
    assert!(!result.recovery_required, "{:?}", result.message);
    let changed = fs::read_to_string(b.path().join("config.toml")).expect("changed");
    assert!(changed.contains("command = \"node\""));
    assert!(changed.ends_with("[projects.'private-path']\ntrust_level='trusted'\n"));
    let journal = b.path().join(".auracode-library").join(&result.id);
    for file in fs::read_dir(&journal).expect("retained files") {
        let file = file.expect("file");
        let bytes = fs::read(file.path()).expect("retained content");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("SYNTHETIC_PRIVATE"));
        assert!(!text.contains("private-path"));
        assert!(!file.file_name().to_string_lossy().contains("-0"));
    }
    fs::write(
        b.path().join("config.toml"),
        changed.replace("model='old'", "model='newer-user-choice'"),
    )
    .expect("unrelated edit");
    drop(library);
    let restarted = Library::new(sources).expect("restart");
    let restored = restarted
        .rollback_copy("codex", &result.id)
        .expect("restore");
    assert!(!restored.recovery_required, "{:?}", restored.message);
    let content = fs::read_to_string(b.path().join("config.toml")).expect("restored");
    assert!(content.contains("model='newer-user-choice'"));
    assert!(content.contains("command = \"old-command\""));
    assert!(content.contains("API_KEY='SYNTHETIC_PRIVATE'"));
    let receipts = restarted.copy_history().expect("history");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].state, CopyState::RolledBack);
}

#[test]
fn claude_state_edit_preserves_private_project_state_and_adds_required_http_type() {
    let a = tempfile::tempdir().expect("source");
    let b = tempfile::tempdir().expect("target");
    fs::write(
        a.path().join("mcp.json"),
        r#"{"mcpServers":{"docs":{"url":"https://example.test/mcp"}}}"#,
    )
    .expect("source");
    let original = r#"{"projects":{"private-path":{"hasTrustDialogAccepted":true}},"mcpServers":{},"other":1e2}"#;
    fs::write(b.path().join(".claude.json"), original).expect("state");
    let mut claude = source("claude-mcp", Provider::Claude, b.path());
    claude.claude_state_only = true;
    let library =
        Library::new(vec![source("pi", Provider::Pi, a.path()), claude]).expect("library");
    let item = server(&library, "pi");
    let plan = library
        .prepare_copy(&item.item, &item.revision, "claude-mcp")
        .expect("prepare");
    assert!(plan.preview().after_text.contains("\"type\": \"http\""));
    let result = library.apply_copy(plan).expect("apply");
    assert!(!result.recovery_required);
    let content = fs::read_to_string(b.path().join(".claude.json")).expect("state");
    assert!(
        content.starts_with(
            r#"{"projects":{"private-path":{"hasTrustDialogAccepted":true}},"mcpServers":{
"#
            .trim_end()
        )
    );
    assert!(content.ends_with(",\"other\":1e2}"));
    assert!(
        !library
            .rollback_copy("claude-mcp", &result.id)
            .expect("rollback")
            .recovery_required
    );
    let doc: serde_json::Value =
        serde_json::from_slice(&fs::read(b.path().join(".claude.json")).expect("state"))
            .expect("json");
    assert!(doc["mcpServers"].as_object().expect("servers").is_empty());
    assert_eq!(
        doc["projects"]["private-path"]["hasTrustDialogAccepted"],
        true
    );
}

#[test]
fn credential_bearing_destination_and_stale_whole_document_fail_before_any_write() {
    let a = tempfile::tempdir().expect("source");
    let b = tempfile::tempdir().expect("target");
    fs::write(
        a.path().join("mcp.json"),
        r#"{"mcpServers":{"docs":{"command":"node"}}}"#,
    )
    .expect("source");
    let private = r#"{"mcpServers":{"docs":{"command":"old","env":{"KEY":"SYNTHETIC_PRIVATE"}}}}"#;
    fs::write(b.path().join("mcp.json"), private).expect("target");
    let library = Library::new(vec![
        source("a", Provider::Pi, a.path()),
        source("b", Provider::Pi, b.path()),
    ])
    .expect("library");
    let item = server(&library, "a");
    assert!(matches!(
        library.prepare_copy(&item.item, &item.revision, "b"),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(
        fs::read_to_string(b.path().join("mcp.json")).expect("unchanged"),
        private
    );
    fs::write(b.path().join("mcp.json"), "{}").expect("safe target");
    let plan = library
        .prepare_copy(&item.item, &item.revision, "b")
        .expect("prepare");
    fs::write(b.path().join("mcp.json"), r#"{"unrelated":"newer"}"#).expect("external edit");
    assert!(matches!(library.apply_copy(plan), Err(Error::Stale)));
    assert!(!b.path().join(".auracode-library").exists());
}
