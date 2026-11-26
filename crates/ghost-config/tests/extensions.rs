//! Names-only extension inventory over temporary agent homes. Nothing real is read.
use ghost_config::Provider;
use ghost_config::extensions::{inspect, inspect_opencode};
use std::fs;

#[test]
fn claude_codex_and_pi_extension_names_come_from_their_folders() {
    let temp = tempfile::tempdir().unwrap();
    let claude = temp.path().join(".claude");
    fs::create_dir_all(claude.join("commands/team")).unwrap();
    fs::create_dir_all(claude.join("agents")).unwrap();
    fs::write(claude.join("commands/review.md"), "Review").unwrap();
    fs::write(claude.join("commands/team/ship.md"), "Ship").unwrap();
    fs::write(claude.join("commands/notes.txt"), "not a command").unwrap();
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\n---",
    )
    .unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"SYNTHETIC_SECRET"}]}]},
            "extraKnownMarketplaces":{"team":{"source":{"source":"github","repo":"owner/plugins"}}},
            "env":{"TOKEN":"SYNTHETIC_SECRET"}}"#,
    )
    .unwrap();
    let found = inspect(Provider::Claude, &claude);
    assert_eq!(found.commands, ["review", "team:ship"]);
    assert_eq!(found.subagents, ["reviewer"]);
    assert_eq!(found.hooks, ["PreToolUse"]);
    assert_eq!(found.marketplaces, ["team"]);
    assert!(!format!("{found:?}").contains("SYNTHETIC_SECRET"));

    let codex = temp.path().join(".codex");
    fs::create_dir_all(codex.join("agents")).unwrap();
    fs::write(codex.join("agents/fixer.toml"), "name = 'fixer'").unwrap();
    fs::write(
        codex.join("hooks.json"),
        r#"{"hooks":{"SessionStart":[],"Stop":[]}}"#,
    )
    .unwrap();
    fs::write(
        codex.join("config.toml"),
        "[marketplaces.local-team]\nsource_type = 'local'\nsource = 'C:/m'\n",
    )
    .unwrap();
    let found = inspect(Provider::Codex, &codex);
    assert_eq!(found.subagents, ["fixer"]);
    assert_eq!(found.hooks, ["SessionStart", "Stop"]);
    assert_eq!(found.marketplaces, ["local-team"]);

    let pi = temp.path().join(".pi/agent");
    fs::create_dir_all(pi.join("prompts")).unwrap();
    fs::write(pi.join("prompts/release.md"), "Release").unwrap();
    assert_eq!(inspect(Provider::Pi, &pi).commands, ["release"]);
    // A missing folder lists nothing and raises nothing.
    assert_eq!(
        inspect(Provider::Claude, &temp.path().join("absent")),
        Default::default()
    );
}

#[test]
fn a_malformed_settings_file_is_a_notice_and_the_rest_still_lists() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("commands")).unwrap();
    fs::write(temp.path().join("commands/review.md"), "Review").unwrap();
    fs::write(temp.path().join("settings.json"), "{broken").unwrap();
    let found = inspect(Provider::Claude, temp.path());
    assert_eq!(found.commands, ["review"]);
    assert_eq!(found.notices.len(), 1);
}

#[test]
fn opencode_reads_its_jsonc_config_and_folders_without_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("plugins")).unwrap();
    fs::create_dir_all(root.join("skill/hello")).unwrap();
    fs::create_dir_all(root.join("commands")).unwrap();
    fs::write(root.join("plugins/notify.ts"), "export default {}").unwrap();
    fs::write(root.join("skill/hello/SKILL.md"), "---\nname: hello\n---").unwrap();
    fs::write(root.join("commands/test.md"), "Run tests").unwrap();
    fs::write(
        root.join("opencode.jsonc"),
        r#"{
          // Plugins load at startup.
          "plugin": ["opencode-wakatime", "@team/helper@1.2.0", "./local/path.js", "https://x.test/p"],
          "mcp": {
            "docs": {"type": "remote", "url": "https://mcp.example.test", "headers": {"Authorization": "Bearer SYNTHETIC_SECRET"}},
            "local": {"type": "local", "command": ["node", "s.js"], "environment": {"TOKEN": "SYNTHETIC_SECRET"}, "enabled": false},
          },
        }"#,
    )
    .unwrap();
    let found = inspect_opencode(root);
    assert_eq!(
        found.plugins,
        ["opencode-wakatime", "@team/helper@1.2.0", "notify"]
    );
    assert_eq!(found.skills, ["hello"]);
    assert_eq!(found.commands, ["test"]);
    assert_eq!(found.servers.len(), 2);
    assert_eq!(found.servers[0].local_fields, ["headers/Authorization"]);
    assert_eq!(found.servers[1].local_fields, ["environment/TOKEN"]);
    assert_eq!(found.servers[1].enabled, Some(false));
    assert!(!format!("{found:?}").contains("SYNTHETIC_SECRET"));
}
