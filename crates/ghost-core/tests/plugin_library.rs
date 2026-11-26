//! Controlled fake CLIs and temporary homes only. Measure the actual core streaming path.
#![cfg(test)]

#[path = "../../ghost-agents/tests/common/mod.rs"]
mod machine;
mod support;

use std::{path::PathBuf, sync::Arc, time::Duration};

use ghost_core::session::{ManagerDeps, ManagerLimits, SessionManager, SessionTimings};
use ghost_process::{EnvResolver, LogFactory};
use ghost_protocol::plugins::{ListingState, PluginFeature, PluginLibrary};
use machine::Machine;
use tokio::{runtime::Handle, sync::mpsc, time::Instant};

type Refresh = tokio::task::JoinHandle<Result<PluginLibrary, ghost_core::session::SessionError>>;
type Updates = mpsc::UnboundedReceiver<(Duration, PluginLibrary)>;

struct Fixture {
    machine: Machine,
    harness: support::Harness,
    manager: SessionManager,
    detector: Arc<ghost_agents::Detector>,
    claude: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let machine = Machine::with_tools(&["codex", "pi"]);
        let claude_bin = machine.home.join(".local/share/claude/versions/fixture");
        machine.install_tool(&claude_bin, "claude");
        machine.version("pi", "1.0.2");
        let claude = machine.home.join(".claude");
        let codex = machine.home.join(".codex");
        let pi = machine.home.join(".pi/agent");
        for home in [&claude, &codex, &pi] {
            std::fs::create_dir_all(home).unwrap();
        }
        std::fs::create_dir_all(claude.join("skills/local-skill")).unwrap();
        std::fs::write(
            claude.join("skills/local-skill/SKILL.md"),
            "# Local skill\n",
        )
        .unwrap();
        std::fs::write(claude.join("fake-listing.json"), serde_json::json!({
            "plugin list --json --available": {"wait_for": "release", "stdout": "{\"installed\":[{\"id\":\"slow@team\"}],\"available\":[]}"},
            "plugin marketplace list --json": {"stdout": "[]"}
        }).to_string()).unwrap();
        std::fs::write(codex.join("fake-listing.json"), serde_json::json!({
            "plugin list --json --available": {"delay_ms": 100, "stdout": "{\"installed\":[{\"pluginId\":\"fast@team\",\"name\":\"fast\"}],\"available\":[]}"},
            "plugin marketplace list --json": {"stdout": "{\"marketplaces\":[]}"},
            "mcp list --json": {"stdout": "[]"}
        }).to_string()).unwrap();
        std::fs::write(
            pi.join("fake-listing.json"),
            serde_json::json!({
                "list --no-approve": {"stdout": "User packages:\n  npm:fixture-tools@1.0.0\n"}
            })
            .to_string(),
        )
        .unwrap();
        let env = machine.env_with_path(
            &[&claude_bin, &machine.bin],
            &[
                ("CLAUDE_CONFIG_DIR", &claude.to_string_lossy()),
                ("CODEX_HOME", &codex.to_string_lossy()),
                ("PI_CODING_AGENT_DIR", &pi.to_string_lossy()),
                (
                    "XDG_CONFIG_HOME",
                    &machine.home.join(".config").to_string_lossy(),
                ),
            ],
        );
        let detector = machine.detector(env.clone());
        let harness = support::Harness::new(Vec::new());
        let rt = Handle::current();
        let manager = SessionManager::new(ManagerDeps {
            worktrees: None,
            checkpoints: None,
            store: harness.store.clone(),
            env: EnvResolver::fixed(env),
            spawner: harness.spawner.clone(),
            detector: detector.clone(),
            drivers: harness.factory.clone(),
            logs: LogFactory::new(
                harness.profile.logs_dir().join("plugins"),
                Arc::clone(&harness.redactor),
                rt.clone(),
            ),
            redactor: harness.redactor.clone(),
            device_id: "fixture".into(),
            app_version: "0.1.0".into(),
            limits: ManagerLimits::STANDARD,
            timings: SessionTimings::STANDARD,
            rt,
        });
        Self {
            machine,
            harness,
            manager,
            detector,
            claude,
        }
    }

    fn start(&self, refresh: bool) -> (Refresh, Updates) {
        let manager = self.manager.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let started = Instant::now();
        let task = tokio::spawn(async move {
            manager
                .plugin_library_progressive(refresh, |library| {
                    tx.send((started.elapsed(), library)).is_ok()
                })
                .await
        });
        (task, rx)
    }

    async fn finish(&self) {
        self.manager.shutdown_all(Duration::from_secs(2)).await;
        self.harness
            .manager
            .shutdown_all(Duration::from_secs(2))
            .await;
        assert!(self.harness.store.shutdown(Duration::from_secs(2)).await);
    }
}

async fn next(
    rx: &mut mpsc::UnboundedReceiver<(Duration, PluginLibrary)>,
) -> (Duration, PluginLibrary) {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

fn has(library: &PluginLibrary, agent: &str, key: &str) -> bool {
    library
        .agents
        .iter()
        .find(|entry| entry.agent_id == agent)
        .unwrap()
        .features
        .iter()
        .any(|feature| feature.entries.iter().any(|entry| entry.key == key))
}

#[tokio::test]
async fn cold_and_warm_sections_arrive_before_blocked_cli_and_fast_provider_publishes_first() {
    let fixture = Fixture::new();
    let manager = fixture.manager.clone();
    let listing_log = fixture.machine.fake.join("claude.listing.log");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cold = tokio::spawn(async move {
        let started = Instant::now();
        let mut first = true;
        manager
            .plugin_library_progressive(false, |library| {
                if first {
                    assert!(
                        !listing_log.exists(),
                        "all initial sections publish before any CLI listing starts"
                    );
                    first = false;
                }
                tx.send((started.elapsed(), library)).is_ok()
            })
            .await
    });
    let (first_usable, initial) = next(&mut rx).await;
    assert_eq!(
        initial
            .agents
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>(),
        [
            "claude",
            "codex",
            "pi",
            "opencode",
            "hermes",
            "grok",
            "cursor",
            "antigravity",
            "omp"
        ]
    );
    assert!(has(&initial, "claude", "local-skill"));
    let fast_at = loop {
        let (elapsed, update) = next(&mut rx).await;
        if has(&update, "codex", "fast@team") {
            assert!(!has(&update, "claude", "slow@team"));
            break elapsed;
        }
    };
    assert!(
        !cold.is_finished(),
        "the fast provider publishes while Claude is blocked"
    );
    std::fs::write(fixture.claude.join("release"), "ready").unwrap();
    let final_cold = cold.await.unwrap().unwrap();
    assert!(has(&final_cold, "claude", "slow@team"));
    let mut cold_finished = fast_at;
    while let Ok((elapsed, _)) = rx.try_recv() {
        cold_finished = elapsed;
    }
    let runs = fixture.machine.runs("claude").len();
    let (warm, mut rx) = fixture.start(false);
    let (warm_usable, initial) = next(&mut rx).await;
    assert!(has(&initial, "claude", "slow@team"));
    assert!(has(&initial, "codex", "fast@team"));
    warm.await.unwrap().unwrap();
    let mut warm_finished = warm_usable;
    while let Ok((elapsed, _)) = rx.try_recv() {
        warm_finished = elapsed;
    }
    assert_eq!(
        fixture.machine.runs("claude").len(),
        runs,
        "warm cache starts no CLI"
    );
    eprintln!(
        "plugin latency cold first={first_usable:?}, fast={fast_at:?}, finished={cold_finished:?}; warm first={warm_usable:?}, finished={warm_finished:?}"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn newer_refresh_and_profile_drain_cancel_old_updates_and_shutdown_cancels_owned_work() {
    let fixture = Fixture::new();
    let (older, mut old_rx) = fixture.start(false);
    next(&mut old_rx).await;
    let (newer, mut new_rx) = fixture.start(false);
    next(&mut new_rx).await;
    assert!(older.await.unwrap().is_err());
    while old_rx.try_recv().is_ok() {}
    fixture.manager.cancel_plugin_refresh();
    assert!(
        newer.await.unwrap().is_err(),
        "profile draining cancels its admitted refresh"
    );
    assert!(
        old_rx.recv().await.is_none(),
        "no stale callbacks survive replacement"
    );
    let (closing, mut rx) = fixture.start(false);
    next(&mut rx).await;
    fixture.manager.shutdown_all(Duration::from_secs(2)).await;
    assert!(closing.await.unwrap().is_err());
    assert!(fixture.manager.plugin_library(false).await.is_err());
    fixture.finish().await;
}

#[tokio::test]
async fn profile_drain_rejects_late_registration_and_abandoned_drain_reopens_admission() {
    let fixture = Fixture::new();
    // This manager reference represents a profile command admitted before draining, but whose
    // library request has not registered yet. It resumes only after the drain closes admission.
    let admitted = fixture.manager.clone();
    let pause = fixture.manager.pause_plugin_refresh();
    let nested = fixture.manager.pause_plugin_refresh();
    let read = || {
        admitted.plugin_library_progressive(true, |_| {
            panic!("a paused request must not publish or start helper work")
        })
    };
    assert!(read().await.is_err());
    drop(nested);
    assert!(
        read().await.is_err(),
        "one remaining drain still closes admission"
    );
    assert!(!fixture.machine.fake.join("claude.listing.log").exists());
    drop(pause);
    let reopened = admitted
        .plugin_library_progressive(false, |_| false)
        .await
        .unwrap();
    assert_eq!(
        reopened.agents.len(),
        9,
        "an abandoned switch leaves its current profile usable"
    );
    assert!(!fixture.machine.fake.join("claude.listing.log").exists());
    fixture.finish().await;
}

#[tokio::test]
async fn refresh_reprobes_providers_progressively_without_waiting_for_slow_detection() {
    let fixture = Fixture::new();
    std::fs::write(fixture.claude.join("release"), "ready").unwrap();
    let cached = fixture.manager.plugin_library(false).await.unwrap();
    let previous = cached
        .agents
        .iter()
        .find(|agent| agent.agent_id == "codex")
        .unwrap()
        .updated_at;
    fixture.machine.mode("claude", "gated-version");
    let (refresh, mut rx) = fixture.start(true);
    let (_, initial) = next(&mut rx).await;
    assert!(
        has(&initial, "claude", "slow@team"),
        "cached rows remain available throughout Refresh"
    );
    loop {
        let (_, update) = next(&mut rx).await;
        if update
            .agents
            .iter()
            .find(|agent| agent.agent_id == "codex")
            .unwrap()
            .updated_at
            != previous
        {
            break;
        }
    }
    assert!(
        !refresh.is_finished(),
        "Codex publishes while Claude's version check is blocked"
    );
    std::fs::write(fixture.machine.fake.join("verification-release"), "ready").unwrap();
    assert!(refresh.await.unwrap().is_ok());
    fixture.finish().await;
}

#[tokio::test]
async fn a_warm_live_provider_becoming_missing_clears_final_and_reopened_live_facts() {
    let fixture = Fixture::new();
    std::fs::write(fixture.claude.join("release"), "ready").unwrap();
    let warm = fixture.manager.plugin_library(false).await.unwrap();
    let claude = warm
        .agents
        .iter()
        .find(|agent| agent.agent_id == "claude")
        .unwrap();
    assert!(
        claude
            .features
            .iter()
            .any(|feature| matches!(feature.state, ListingState::Live))
    );
    assert!(has(&warm, "claude", "slow@team"));
    assert!(
        fixture
            .detector
            .cached_plugin_listings()
            .contains_key("claude")
    );
    std::fs::rename(
        fixture
            .machine
            .home
            .join(".local/share/claude/versions/fixture"),
        fixture.machine.root.path().join("removed-claude"),
    )
    .unwrap();
    let missing = fixture.manager.plugin_library(true).await.unwrap();
    let assert_missing = |library: &PluginLibrary| {
        let claude = library
            .agents
            .iter()
            .find(|agent| agent.agent_id == "claude")
            .unwrap();
        assert!(!claude.installed);
        assert!(
            claude.updated_at.is_none(),
            "a missing installation cannot keep the old live timestamp"
        );
        assert!(
            claude
                .features
                .iter()
                .all(|feature| !matches!(feature.state, ListingState::Live))
        );
        assert!(!has(library, "claude", "slow@team"));
        assert!(
            has(library, "claude", "local-skill"),
            "the real file inventory remains available"
        );
    };
    assert_missing(&missing);
    assert!(
        !fixture
            .detector
            .cached_plugin_listings()
            .contains_key("claude")
    );
    let (reopen, mut updates) = fixture.start(false);
    assert_missing(&next(&mut updates).await.1);
    assert_missing(&reopen.await.unwrap().unwrap());
    fixture.finish().await;
}

#[tokio::test]
async fn unsupported_agents_are_explicitly_unavailable_and_opencode_sync_is_not_actionable() {
    let fixture = Fixture::new();
    let (task, mut rx) = fixture.start(false);
    let (_, initial) = next(&mut rx).await;
    for id in ["cursor", "antigravity"] {
        let agent = initial
            .agents
            .iter()
            .find(|agent| agent.agent_id == id)
            .unwrap();
        assert!(
            agent.features.iter().any(|feature| feature.feature
                == ghost_protocol::plugins::PluginFeature::Plugins
                && matches!(
                    feature.state,
                    ghost_protocol::plugins::ListingState::Unavailable { .. }
                )),
            "unsupported native plugin APIs must be explicit"
        );
    }
    // Grok Build and Hermes list their plugins and their marketplaces (Hermes: skill sources).
    for id in ["grok", "hermes"] {
        let agent = initial
            .agents
            .iter()
            .find(|agent| agent.agent_id == id)
            .unwrap();
        for wanted in [
            ghost_protocol::plugins::PluginFeature::Plugins,
            ghost_protocol::plugins::PluginFeature::Marketplaces,
        ] {
            let feature = agent
                .features
                .iter()
                .find(|feature| feature.feature == wanted)
                .unwrap_or_else(|| panic!("{id} lists {wanted:?}"));
            assert!(
                !matches!(
                    feature.state,
                    ghost_protocol::plugins::ListingState::Unavailable { .. }
                ),
                "{id} {wanted:?} is supported now"
            );
        }
    }
    assert_eq!(
        initial.agents.len(),
        9,
        "unsupported inventory is never represented as a successful empty listing"
    );
    let open = initial
        .agents
        .iter()
        .find(|agent| agent.agent_id == "opencode")
        .unwrap();
    assert!(
        open.features
            .iter()
            .all(|feature| matches!(feature.state, ListingState::Files { .. }))
    );
    assert!(
        initial.agents[0]
            .features
            .iter()
            .any(|feature| feature.feature == PluginFeature::Skills)
    );
    fixture.manager.cancel_plugin_refresh();
    assert!(task.await.unwrap().is_err());
    fixture.finish().await;
}
