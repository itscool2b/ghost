//! Plugin library listings run fake CLIs pointed at temporary agent homes through each agent's
//! documented variable (CLAUDE_CONFIG_DIR, CODEX_HOME, PI_CODING_AGENT_DIR). The fakes answer
//! only from `fake-listing.json` in that home, so a passing test proves the home was used.
#![cfg(test)]
mod common;
use common::Machine;
use ghost_protocol::plugins::{ListingState, PluginFeature};
use std::path::{Path, PathBuf};

fn home(machine: &Machine, name: &str, answers: serde_json::Value) -> PathBuf {
    let dir = machine.root.path().join("agent-homes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fake-listing.json"), answers.to_string()).unwrap();
    dir
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn claude_answers(version: &str) -> serde_json::Value {
    serde_json::json!({
        "plugin list --json --available": {"stdout": serde_json::json!({
            "installed": [{"id": format!("tools@team-{version}"), "version": "1.0.0", "scope": "user", "enabled": true}],
            "available": [{"pluginId": "docs@team", "name": "docs", "marketplaceName": "team"}]
        }).to_string()},
        "plugin marketplace list --json": {"stdout": "[{\"name\":\"team\",\"source\":\"github\",\"repo\":\"owner/plugins\"}]"}
    })
}

#[tokio::test]
async fn claude_listings_use_the_given_home_cache_by_version_and_reread_after_an_update() {
    let machine = Machine::with_tools(&[]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    let claude = home(&machine, "claude", claude_answers("a"));
    let env = machine.env_with_path(&[&bin], &[("CLAUDE_CONFIG_DIR", &text(&claude))]);
    let detector = machine.detector(env);
    let first = detector
        .plugin_listings("claude", false, 1_000)
        .await
        .unwrap();
    assert_eq!(first.version, "2.1.310");
    assert_eq!(first.version_change, None);
    let plugins = &first.listings[0];
    assert_eq!(plugins.feature, PluginFeature::Plugins);
    assert_eq!(plugins.state, ListingState::Live);
    assert_eq!(plugins.entries[0].key, "tools@team-a");
    assert_eq!(plugins.available_total, 1);
    assert_eq!(
        first.listings[1].entries[0].description.as_deref(),
        Some("owner/plugins")
    );
    let runs = std::fs::read_to_string(machine.fake.join("claude.listing.log")).unwrap();
    assert_eq!(runs.lines().count(), 2);
    for run in runs.lines() {
        let run: serde_json::Value = serde_json::from_str(run).unwrap();
        assert_eq!(
            run["home"],
            text(&claude),
            "the temporary home, never the real one"
        );
        assert_eq!(run["no_color"], "1");
    }
    // Same version: the cache answers and nothing runs.
    let cached = detector
        .plugin_listings("claude", false, 2_000)
        .await
        .unwrap();
    assert_eq!(cached.updated_at, 1_000);
    assert_eq!(
        std::fs::read_to_string(machine.fake.join("claude.listing.log"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    // The harness was updated outside Ghost: detection sees the new version and the listing
    // is read again, reporting the change.
    machine.version("claude", "2.1.312 (Claude Code)");
    std::fs::write(
        claude.join("fake-listing.json"),
        claude_answers("b").to_string(),
    )
    .unwrap();
    detector.detect_all(true).await;
    let after = detector
        .plugin_listings("claude", false, 3_000)
        .await
        .unwrap();
    assert_eq!(after.updated_at, 3_000);
    assert_eq!(after.listings[0].entries[0].key, "tools@team-b");
    let change = after.version_change.unwrap();
    assert_eq!(
        (change.from.as_str(), change.to.as_str()),
        ("2.1.310", "2.1.312")
    );
    // An explicit refresh always runs again.
    let refreshed = detector
        .plugin_listings("claude", true, 4_000)
        .await
        .unwrap();
    assert_eq!(refreshed.updated_at, 4_000);
}

#[tokio::test]
async fn one_failing_codex_command_fails_only_its_feature_and_secrets_never_survive() {
    let machine = Machine::with_tools(&["codex"]);
    let codex = home(
        &machine,
        "codex",
        serde_json::json!({
            "plugin list --json --available": {"stdout": "", "stderr": "Error: marketplace cache is corrupt\n", "code": 1},
            "plugin marketplace list --json": {"stdout": "{\"marketplaces\":[{\"name\":\"local-team\",\"root\":\"C:\\\\x\"}]}"},
            "mcp list --json": {"stdout": serde_json::json!([{"name":"private","enabled":true,
                "transport":{"type":"stdio","command":"node","args":["s.js"],"env":{"TOKEN":"SYNTHETIC_CODEX_SECRET"}}}]).to_string()}
        }),
    );
    let env = machine.env(&[("CODEX_HOME", &text(&codex))]);
    let detector = machine.detector(env);
    let listed = detector.plugin_listings("codex", false, 1).await.unwrap();
    let state = |feature| {
        listed
            .listings
            .iter()
            .find(|listing| listing.feature == feature)
            .unwrap()
            .state
            .clone()
    };
    let ListingState::Failed { reason } = state(PluginFeature::Plugins) else {
        panic!("plugins failed");
    };
    assert!(reason.contains("marketplace cache is corrupt"), "{reason}");
    assert_eq!(state(PluginFeature::Marketplaces), ListingState::Live);
    assert_eq!(state(PluginFeature::McpServers), ListingState::Live);
    assert!(!format!("{listed:?}").contains("SYNTHETIC_CODEX_SECRET"));
}

#[tokio::test]
async fn pi_lists_offline_and_an_agent_without_a_home_variable_fails_plainly() {
    let machine = Machine::with_tools(&[]);
    machine.install_tool(&machine.bin, "pi");
    machine.version("pi", "1.0.2");
    let pi = home(
        &machine,
        "pi",
        serde_json::json!({"list --no-approve": {"stdout": "User packages:\n  npm:pi-tools@1.0.0\n"}}),
    );
    let detector = machine.detector(machine.env(&[("PI_CODING_AGENT_DIR", &text(&pi))]));
    let listed = detector.plugin_listings("pi", false, 1).await.unwrap();
    assert_eq!(listed.listings[0].entries[0].key, "npm:pi-tools@1.0.0");
    let run: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(machine.fake.join("pi.listing.log"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        run["pi_offline"], "1",
        "a listing never reaches the network"
    );
    assert_eq!(run["home"], text(&pi));
    // The same fake without its variable cannot find answers: the listing fails, not guesses.
    let bare = machine.detector(machine.env(&[]));
    let failed = bare.plugin_listings("pi", true, 2).await.unwrap();
    assert!(matches!(
        failed.listings[0].state,
        ListingState::Failed { .. }
    ));
    // Agents without reviewed listing commands are not run at all.
    assert!(bare.plugin_listings("hermes", false, 3).await.is_none());
}

#[tokio::test]
async fn a_missing_installation_invalidates_its_warm_listing_cache() {
    let machine = Machine::with_tools(&[]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    let claude = home(&machine, "claude", claude_answers("warm"));
    let detector =
        machine.detector(machine.env_with_path(&[&bin], &[("CLAUDE_CONFIG_DIR", &text(&claude))]));
    assert_eq!(
        detector
            .plugin_listings("claude", false, 1)
            .await
            .unwrap()
            .listings[0]
            .state,
        ListingState::Live
    );
    assert!(detector.cached_plugin_listings().contains_key("claude"));
    // Move only this fixture's executable out of its isolated PATH. The separate listing-failure
    // test keeps an installed provider and proves cached rows survive with a Failed warning.
    std::fs::rename(&bin, machine.root.path().join("removed-claude")).unwrap();
    detector.prepare_plugin_refresh().await;
    detector.refresh_plugin_provider("claude").await;
    assert!(matches!(
        detector
            .cached()
            .into_iter()
            .find(|agent| agent.id == "claude")
            .unwrap()
            .availability,
        ghost_protocol::session::Availability::Missing { .. }
    ));
    assert!(detector.plugin_listings("claude", true, 2).await.is_none());
    assert!(
        !detector.cached_plugin_listings().contains_key("claude"),
        "a missing installation must not retain live cache data"
    );
    assert!(detector.plugin_listings("claude", false, 3).await.is_none());
}

async fn wait_for_listing(machine: &Machine, provider: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !machine
            .fake
            .join(format!("{provider}.listing.log"))
            .exists()
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn provider_singleflight_shares_refreshes_and_failure_keeps_cached_rows() {
    let machine = Machine::with_tools(&[]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    let mut answers = claude_answers("retained");
    answers["plugin list --json --available"]["wait_for"] = "release".into();
    let claude = home(&machine, "claude", answers);
    let detector =
        machine.detector(machine.env_with_path(&[&bin], &[("CLAUDE_CONFIG_DIR", &text(&claude))]));
    let first = tokio::spawn({
        let detector = detector.clone();
        async move { detector.plugin_listings("claude", true, 1).await.unwrap() }
    });
    wait_for_listing(&machine, "claude").await;
    let second = tokio::spawn({
        let detector = detector.clone();
        async move { detector.plugin_listings("claude", true, 2).await.unwrap() }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        std::fs::read_to_string(machine.fake.join("claude.listing.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::write(claude.join("release"), "ready").unwrap();
    let a = first.await.unwrap();
    assert_eq!(a, second.await.unwrap());
    assert_eq!(
        std::fs::read_to_string(machine.fake.join("claude.listing.log"))
            .unwrap()
            .lines()
            .count(),
        2,
        "each recipe runs once, sequentially"
    );
    std::fs::write(
        claude.join("fake-listing.json"),
        serde_json::json!({
            "plugin list --json --available": {"code": 1, "stderr": "fixture failure"},
            "plugin marketplace list --json": {"code": 1}
        })
        .to_string(),
    )
    .unwrap();
    let failed = detector.plugin_listings("claude", true, 3).await.unwrap();
    assert!(matches!(
        failed.listings[0].state,
        ListingState::Failed { .. }
    ));
    assert_eq!(failed.listings[0].entries, a.listings[0].entries);
    assert_eq!(failed.listings[0].available, a.listings[0].available);
    assert_eq!(
        failed.listings[0].available_total,
        a.listings[0].available_total
    );
    assert_eq!(failed.listings[1].entries, a.listings[1].entries);
}

#[tokio::test]
async fn at_most_two_providers_list_concurrently_and_a_fast_provider_overtakes_a_blocked_one() {
    let machine = Machine::with_tools(&["codex", "pi"]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    machine.version("pi", "1.0.2");
    let mut answers = claude_answers("slow");
    answers["plugin list --json --available"]["wait_for"] = "release".into();
    let claude = home(&machine, "claude", answers);
    let codex = home(
        &machine,
        "codex",
        serde_json::json!({
            "plugin list --json --available": {"wait_for": "release", "stdout": "{\"installed\":[],\"available\":[]}"},
            "plugin marketplace list --json": {"stdout": "{\"marketplaces\":[]}"},
            "mcp list --json": {"stdout": "[]"}
        }),
    );
    let pi = home(
        &machine,
        "pi",
        serde_json::json!({"list --no-approve": {"stdout": ""}}),
    );
    let detector = machine.detector(machine.env_with_path(
        &[&bin, &machine.bin],
        &[
            ("CLAUDE_CONFIG_DIR", &text(&claude)),
            ("CODEX_HOME", &text(&codex)),
            ("PI_CODING_AGENT_DIR", &text(&pi)),
        ],
    ));
    let slow = tokio::spawn({
        let d = detector.clone();
        async move { d.plugin_listings("claude", true, 1).await }
    });
    let fast = tokio::spawn({
        let d = detector.clone();
        async move { d.plugin_listings("codex", true, 1).await }
    });
    wait_for_listing(&machine, "claude").await;
    wait_for_listing(&machine, "codex").await;
    let queued = tokio::spawn({
        let d = detector.clone();
        async move { d.plugin_listings("pi", true, 1).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !machine.fake.join("pi.listing.log").exists(),
        "the third provider cannot enter until a slot is free"
    );
    std::fs::write(codex.join("release"), "ready").unwrap();
    assert!(fast.await.unwrap().is_some());
    assert!(
        queued.await.unwrap().is_some(),
        "a queued provider proceeds while Claude is still blocked"
    );
    assert!(!slow.is_finished());
    std::fs::write(claude.join("release"), "ready").unwrap();
    assert!(slow.await.unwrap().is_some());
}

#[tokio::test]
async fn an_older_listing_cannot_replace_the_newer_installation_generation() {
    let machine = Machine::with_tools(&[]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    let mut answers = claude_answers("old");
    answers["plugin list --json --available"]["wait_for"] = "release".into();
    let claude = home(&machine, "claude", answers);
    let detector =
        machine.detector(machine.env_with_path(&[&bin], &[("CLAUDE_CONFIG_DIR", &text(&claude))]));
    let old = tokio::spawn({
        let d = detector.clone();
        async move { d.plugin_listings("claude", true, 1).await }
    });
    wait_for_listing(&machine, "claude").await;
    machine.version("claude", "2.1.312 (Claude Code)");
    std::fs::write(
        claude.join("fake-listing.json"),
        claude_answers("new").to_string(),
    )
    .unwrap();
    detector.detect_all(true).await;
    let new = detector.plugin_listings("claude", true, 2).await.unwrap();
    assert!(
        old.await.unwrap().is_none(),
        "the superseded provider job is cancelled"
    );
    assert_eq!(new.version, "2.1.312");
    assert_eq!(new.listings[0].entries[0].key, "tools@team-new");
    let cached = detector.cached_plugin_listings();
    assert_eq!(
        cached["claude"], new,
        "no stale result overwrites the provider cache"
    );
}
