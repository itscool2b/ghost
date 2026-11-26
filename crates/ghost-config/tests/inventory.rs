#![allow(clippy::expect_used)]

use std::fs;

use ghost_config::{Error, Kind, Library, Provider, Scope, Source};

fn source(id: &str, provider: Provider, root: &std::path::Path) -> Source {
    Source {
        id: id.into(),
        provider,
        scope: Scope::User,
        root: root.into(),
        claude_state_only: false,
    }
}

fn write(root: &std::path::Path, path: &str, value: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directories");
    fs::write(path, value).expect("fixture write");
}

#[test]
fn nested_portable_skills_keep_distinct_source_keys_and_exclude_private_subtrees() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(
        fixture.path(),
        "skills/team-one/review/SKILL.md",
        "First instructions.",
    );
    write(
        fixture.path(),
        "skills/team-one/review/references/input.md",
        "First support.",
    );
    write(
        fixture.path(),
        "skills/team-two/review/SKILL.md",
        "Second instructions.",
    );
    write(
        fixture.path(),
        "skills/sessions/hidden/SKILL.md",
        "SYNTHETIC_UNREAD",
    );
    write(
        fixture.path(),
        "skills/cache/hidden/SKILL.md",
        "SYNTHETIC_UNREAD",
    );
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert!(inventory.notices.is_empty(), "{:?}", inventory.notices);
    assert_eq!(inventory.items.len(), 2);
    let first = inventory
        .items
        .iter()
        .find(|item| item.item.key == "team-one/review")
        .expect("nested skill");
    let preview = library
        .preview(&first.item, &first.revision)
        .expect("preview");
    assert_eq!(preview.files, ["SKILL.md", "references/input.md"]);
    assert!(
        preview
            .notices
            .iter()
            .any(|notice| notice.contains("team-two/review"))
    );
    assert!(
        preview
            .notices
            .iter()
            .any(|notice| notice.contains("first discovered"))
    );
    assert!(
        !serde_json::to_string(&preview)
            .expect("json")
            .contains("SYNTHETIC_UNREAD")
    );
    // Shared locations use the same bounded recursive portable discovery, without pretending
    // that the Library's order establishes a vendor's active skill.
    let shared = Library::new(vec![source(
        "shared",
        Provider::Shared,
        &fixture.path().join("skills"),
    )])
    .expect("shared");
    assert_eq!(shared.inspect().items.len(), 2);
}

#[test]
fn duplicate_personal_and_project_skills_are_separate_with_precedence_explained() {
    let personal = tempfile::tempdir().expect("personal");
    let project = tempfile::tempdir().expect("project");
    write(
        personal.path(),
        "skills/review/SKILL.md",
        "Personal instructions.",
    );
    write(
        project.path(),
        ".claude/skills/review/SKILL.md",
        "Project instructions.",
    );
    let mut project_source = source("project", Provider::Claude, project.path());
    project_source.scope = Scope::Project;
    let library = Library::new(vec![
        source("personal", Provider::Claude, personal.path()),
        project_source,
    ])
    .expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.items.len(), 2);
    let project_item = inventory
        .items
        .iter()
        .find(|item| item.scope == Scope::Project)
        .expect("project item");
    let preview = library
        .preview(&project_item.item, &project_item.revision)
        .expect("preview");
    assert_eq!(preview.text, "Project instructions.");
    assert!(
        preview
            .notices
            .iter()
            .any(|notice| notice.contains("personal: skills/review"))
    );
    assert!(
        preview
            .notices
            .iter()
            .any(|notice| notice.contains("personal skill takes precedence"))
    );
}

#[test]
fn recursive_discovery_reports_its_depth_limit_without_hiding_a_partial_result() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(
        fixture.path(),
        "skills/one/two/three/four/five/six/seven/eight/nine/SKILL.md",
        "Too deep.",
    );
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert!(inventory.items.is_empty());
    assert_eq!(inventory.notices.len(), 1);
    assert!(inventory.notices[0].contains("bounded"));
}

#[test]
fn allowlisted_inventory_never_opens_auth_sessions_or_vendor_synced_content() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(fixture.path(), "CLAUDE.md", "Be precise.");
    write(
        fixture.path(),
        "settings.json",
        r#"{"enabledPlugins":{"example@official":true},"env":{"API_KEY":"SYNTHETIC_PRIVATE"}}"#,
    );
    write(
        fixture.path(),
        "skills/local/SKILL.md",
        "---\nname: local\ndescription: Example\n---\nLocal text.",
    );
    write(
        fixture.path(),
        "skills/local/references/info.md",
        "Reference.",
    );
    write(fixture.path(), "skills/synced/SKILL.md", "MUST_NOT_APPEAR");
    write(fixture.path(), "auth.json", "invalid JSON MUST_NOT_APPEAR");
    write(fixture.path(), "sessions/private.json", "MUST_NOT_APPEAR");
    let library =
        Library::new(vec![source("claude", Provider::Claude, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert!(inventory.notices.is_empty(), "{:?}", inventory.notices);
    assert_eq!(inventory.items.len(), 3);
    let serialized = serde_json::to_string(&inventory).expect("inventory");
    assert!(!serialized.contains("PRIVATE"));
    assert!(!serialized.contains("MUST_NOT"));
    let skill = inventory
        .items
        .iter()
        .find(|item| item.item.kind == Kind::Skill)
        .expect("skill");
    let preview = library
        .preview(&skill.item, &skill.revision)
        .expect("preview");
    assert_eq!(preview.files, ["SKILL.md", "references/info.md"]);
    write(
        fixture.path(),
        "skills/local/references/info.md",
        "Changed companion.",
    );
    assert!(matches!(
        library.preview(&skill.item, &skill.revision),
        Err(Error::Stale)
    ));
}

#[test]
fn mcp_secrets_and_unknown_fields_never_reach_previews_or_errors() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(
        fixture.path(),
        "mcp.json",
        r#"{"mcpServers":{
      "public":{"url":"https://example.invalid/mcp"},
      "private":{"url":"https://example.invalid/mcp","headers":{"Authorization":"SYNTHETIC_SECRET"}},
      "future":{"command":"runner","futureField":"SYNTHETIC_SECRET"},
      "query":{"url":"https://example.invalid/mcp?key=SYNTHETIC_SECRET"},
      "arg":{"command":"runner","args":["--api-key","SYNTHETIC_SECRET"]}}}"#,
    );
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.items.len(), 5);
    for item in &inventory.items {
        let preview = library
            .preview(&item.item, &item.revision)
            .expect("preview");
        assert!(
            !serde_json::to_string(&preview)
                .expect("preview json")
                .contains("SYNTHETIC_SECRET")
        );
        assert_eq!(item.copy_blocked.is_none(), item.label == "public");
    }
}

#[test]
fn claude_state_projects_and_codex_trust_never_become_items() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(
        fixture.path(),
        ".claude.json",
        r#"{"mcpServers":{"public":{"command":"runner"}},"projects":{"SYNTHETIC_PRIVATE":{"trust":true}},"oauthAccount":"SYNTHETIC_PRIVATE"}"#,
    );
    write(
        fixture.path(),
        "config.toml",
        "# Preserve this later\n[projects.\"SYNTHETIC_PRIVATE\"]\ntrust_level = 'trusted'\n[mcp_servers.docs]\nurl = 'https://example.invalid/mcp'\n[plugins.\"sample@official\"]\nenabled = false\n[plugins.\"sample@official\".userConfig]\napi_key = 'SYNTHETIC_PRIVATE'\n",
    );
    let mut claude = source("claude-mcp", Provider::Claude, fixture.path());
    claude.claude_state_only = true;
    let library = Library::new(vec![
        claude,
        source("codex", Provider::Codex, fixture.path()),
    ])
    .expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.items.len(), 3, "{:?}", inventory.notices);
    for item in inventory.items {
        assert!(
            !serde_json::to_string(
                &library
                    .preview(&item.item, &item.revision)
                    .expect("preview")
            )
            .expect("json")
            .contains("SYNTHETIC_PRIVATE")
        );
    }
}

#[test]
fn incomplete_skill_is_visible_but_never_copied_without_its_dependencies() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(fixture.path(), "skills/example/SKILL.md", "Example.");
    write(fixture.path(), "skills/example/.env", "SYNTHETIC_SECRET");
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.items.len(), 1);
    assert!(inventory.items[0].copy_blocked.is_some());
    assert!(
        !library
            .preview(&inventory.items[0].item, &inventory.items[0].revision)
            .expect("preview")
            .text
            .contains("SECRET")
    );
}

#[test]
fn malformed_config_errors_do_not_quote_private_data_and_limits_are_enforced() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(fixture.path(), "mcp.json", "{invalid SYNTHETIC_SECRET");
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.notices.len(), 1);
    assert!(!inventory.notices[0].contains("SECRET"));
    fs::remove_file(fixture.path().join("mcp.json")).expect("owned fixture");
    write(fixture.path(), "AGENTS.md", &"x".repeat(1024 * 1024 + 1));
    assert!(library.inspect().notices[0].contains("limit"));
}

#[test]
fn project_layout_and_shared_skills_are_distinct_sources() {
    let fixture = tempfile::tempdir().expect("fixture");
    write(
        fixture.path(),
        ".pi/mcp.json",
        r#"{"mcpServers":{"docs":{"url":"https://example.invalid/mcp"}}}"#,
    );
    write(fixture.path(), "AGENTS.md", "Project instructions.");
    write(fixture.path(), ".agents/skills/shared/SKILL.md", "Shared.");
    let mut project = source("project-pi", Provider::Pi, fixture.path());
    project.scope = Scope::Project;
    let library = Library::new(vec![
        project,
        source(
            "shared",
            Provider::Shared,
            &fixture.path().join(".agents/skills"),
        ),
    ])
    .expect("library");
    let inventory = library.inspect();
    assert_eq!(inventory.items.len(), 3, "{:?}", inventory.notices);
    assert_eq!(
        inventory
            .items
            .iter()
            .filter(|item| item.item.kind == Kind::Skill)
            .count(),
        1
    );
    let mut forged = inventory.items[0].item.clone();
    forged.key = "../../auth.json".into();
    assert!(matches!(
        library.preview(&forged, &inventory.items[0].revision),
        Err(Error::Stale)
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_skill_support_file_is_refused_without_following_it() {
    let fixture = tempfile::tempdir().expect("fixture");
    let outside = tempfile::tempdir().expect("outside fixture");
    write(fixture.path(), "skills/example/SKILL.md", "Example.");
    write(outside.path(), "secret", "SYNTHETIC_SECRET");
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        fixture.path().join("skills/example/support.md"),
    )
    .expect("fixture link");
    let library = Library::new(vec![source("pi", Provider::Pi, fixture.path())]).expect("library");
    assert!(library.inspect().items[0].copy_blocked.is_some());
}
