//! The headless runtime and a run's folders: the temporary profile and the machine root are set
//! up and cleaned up, a profile another process owns is refused, the chat folder is a committed git
//! repository (hostile or plain), the run folder is kept or deleted, and `agents` lists every
//! catalog agent with only the demo one ready on a `PATH` that finds nothing.

#[cfg(test)]
mod common;

#[cfg(test)]
mod setup {
    use std::path::Path;
    use std::process::Command;

    use ghost_cli::CliError;
    use ghost_cli::agents;
    use ghost_cli::headless::{EnvChoice, Headless, HeadlessConfig, Roots};
    use ghost_cli::out::Out;
    use ghost_cli::repo::{self, HOSTILE_MARK, HOSTILE_REGISTRY, RepoKind, RunFolder};
    use ghost_core::CoreError;
    use ghost_core::owner::ProfileOwner;
    use ghost_core::paths::DataRoot;
    use ghost_core::profile::Profiles;
    use ghost_process::LeftoverRegistry;

    use crate::common::Machine;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn automatic_checkpoints_follow_the_owned_profile_without_crossing_roots() {
        use ghost_core::onboarding::{OnboardingStore, SetupAction};

        for (saved, services) in [
            (None, true),
            (Some(true), true),
            (Some(false), true),
            (Some(true), false),
        ] {
            let machine = Machine::new();
            let profile = Profiles::new(DataRoot::at(&machine.profile_root))
                .open_active()
                .unwrap();
            let owner = ProfileOwner::acquire(&profile).unwrap();
            if let Some(enabled) = saved {
                OnboardingStore::open(&profile.dir)
                    .unwrap()
                    .apply(0, SetupAction::AutomaticCheckpoints { enabled })
                    .unwrap();
            }
            drop(owner);
            let headless = Headless::start(HeadlessConfig {
                roots: Roots {
                    machine: machine.machine_root.clone(),
                    profile: machine.profile_root.clone(),
                },
                env: EnvChoice::Fixed(machine.env(&[])),
                set_env: Vec::new(),
                app_version: "0.1.0".into(),
                checkpoints: services,
            })
            .await
            .unwrap();
            assert_eq!(
                headless.manager.automatic_checkpoints(),
                services && saved.unwrap_or(false)
            );
            assert!(!machine.machine_root.join("profiles").exists());
            assert!(headless.shutdown().await.saved);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_profile_is_temporary_and_the_agents_register_in_the_machine_root() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        assert!(machine.profile_root.join("device.json").is_file());
        assert_eq!(
            headless.profile_dir,
            machine.profile_root.join("profiles").join("local")
        );
        assert!(headless.profile_dir.join("aura.db").is_file());
        assert!(
            !machine.machine_root.join("profiles").exists(),
            "the machine root holds no profile"
        );
        assert_eq!(
            headless.adapters_dir(),
            machine.machine_root.join("adapters")
        );
        if cfg!(unix) {
            assert_eq!(
                machine.registry_files().len(),
                1,
                "this process's registry file"
            );
        }
        assert_eq!(headless.sweep.killed, Vec::<(u32, String)>::new());
        let ended = headless.shutdown().await;
        assert!(ended.saved, "the store saved everything");
        assert_eq!(ended.sessions.map(|summary| summary.sessions), Some(0));
        assert!(
            machine.registry_files().is_empty(),
            "the registry file is gone"
        );
    }

    /// `--data-dir` on a profile another process owns (the app) is refused before anything runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_profile_another_owner_holds_is_refused() {
        let machine = Machine::new();
        let data = machine.path("app-data");
        let root = DataRoot::at(&data);
        root.ensure().expect("ensure");
        let profile = Profiles::new(root).open_active().expect("profile");
        let app = ProfileOwner::acquire(&profile).expect("the app owns the profile");
        let refused = Headless::start(HeadlessConfig {
            roots: Roots {
                machine: data.clone(),
                profile: data.clone(),
            },
            env: EnvChoice::Fixed(machine.env(&[])),
            set_env: Vec::new(),
            app_version: "0.1.0".into(),
            checkpoints: false,
        })
        .await
        .expect_err("refused");
        assert!(
            matches!(&refused, CliError::Core(CoreError::ProfileInUse { .. })),
            "{refused:?}"
        );
        assert!(
            refused
                .to_string()
                .starts_with("Ghost is already running with the profile in "),
            "{refused}"
        );
        assert!(!data.join("run").exists(), "nothing registered");
        drop(app);
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_chat_folder_is_a_committed_repository() {
        let machine = Machine::new();
        let registry = LeftoverRegistry::disabled();
        let plain = repo::create(&machine.path("plain"), RepoKind::Plain, &registry)
            .await
            .expect("a plain repository");
        assert!(plain.join(".git").is_dir());
        assert!(plain.join("README.md").is_file());
        assert_eq!(
            git(&plain, &["status", "--porcelain"]),
            "",
            "everything is committed"
        );
        assert_eq!(git(&plain, &["log", "--format=%an"]).trim(), "Ghost");

        let hostile = repo::create(&machine.path("hostile"), RepoKind::Hostile, &registry)
            .await
            .expect("a hostile repository");
        assert_eq!(git(&hostile, &["status", "--porcelain"]), "");
        let npmrc = std::fs::read_to_string(hostile.join(".npmrc")).expect(".npmrc");
        assert!(
            npmrc.contains(&format!("registry={HOSTILE_REGISTRY}")),
            "{npmrc}"
        );
        assert!(
            npmrc.contains(&format!("@agentclientprotocol:registry={HOSTILE_REGISTRY}")),
            "{npmrc}"
        );
        // A fake copy of every catalog adapter at its catalog version, committed.
        let tracked = git(&hostile, &["ls-files"]);
        for spec in ghost_agents::catalog() {
            if let ghost_agents::Recipe::NpmAdapter {
                package,
                version,
                bin,
                ..
            } = &spec.recipe
            {
                let folder = hostile.join("node_modules").join(package);
                let manifest: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(folder.join("package.json")).expect("manifest"),
                )
                .expect("json");
                assert_eq!(manifest["version"], *version);
                assert_eq!(manifest["bin"][bin], "bin.js");
                let script = std::fs::read_to_string(folder.join("bin.js")).expect("bin");
                assert!(script.contains(HOSTILE_MARK));
                assert!(
                    tracked.contains(&format!("node_modules/{package}/bin.js")),
                    "{tracked}"
                );
                assert!(hostile.join("node_modules/.bin").join(bin).is_file());
            }
        }
    }

    #[test]
    fn a_run_folder_is_deleted_unless_kept() {
        let dropped = RunFolder::create("test").expect("a run folder");
        let path = dropped.path().to_path_buf();
        assert!(path.is_dir());
        assert!(
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("ghost-cli-test-"))
        );
        drop(dropped);
        assert!(!path.exists(), "deleted on drop");
        let kept = RunFolder::create("test").expect("a run folder").keep();
        assert!(kept.is_dir(), "kept");
        std::fs::remove_dir_all(&kept).expect("clean up");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agents_lists_the_catalog_with_only_the_demo_agent_ready_here() {
        let machine = Machine::new();
        let headless = machine.headless(&[("CLAUDECODE", "1")]).await;
        let (out, captured) = Out::capture();
        let outcome = agents::run(&headless, &out).await.expect("agents");
        assert_eq!(outcome, ghost_cli::report::Outcome::Passed);
        let text = captured.text();
        assert!(
            text.starts_with("Environment: fallback, because test\n"),
            "{text}"
        );
        assert!(
            text.contains("Removed by the scrub: CLAUDECODE\n"),
            "{text}"
        );
        // One entry per agent: an adapter or a text fallback is a connection, not an agent.
        let mut agents: Vec<&ghost_agents::AgentSpec> = Vec::new();
        for spec in ghost_agents::catalog() {
            if !agents.iter().any(|agent| agent.agent == spec.agent) {
                agents.push(spec);
            }
        }
        for spec in &agents {
            assert!(
                text.contains(&format!("{} ({})\n", spec.agent, spec.name)),
                "{}: {text}",
                spec.agent
            );
        }
        assert!(!text.contains("claude-acp ("), "{text}");
        assert!(
            text.contains(
                "claude (Claude Code)\n  missing: claude was not found on your PATH.\n  connection: Automatic; new sessions use none (none can run here)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("mock (Demo agent (scripted, no AI))\n  ready (built in)"),
            "{text}"
        );
        let external_agents = agents
            .iter()
            .filter(|spec| !matches!(spec.recipe, ghost_agents::Recipe::Builtin))
            .count();
        assert_eq!(
            text.matches("  missing: ").count(),
            external_agents,
            "{text}"
        );
        assert!(!text.contains("checking"), "detection finished: {text}");
        headless.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unknown_agent_is_refused_with_the_known_ones() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        let folder = machine.path("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        let error = ghost_cli::probe::run(
            &headless,
            &ghost_cli::args::ProbeArgs {
                agent: "gemini".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            false,
            ghost_cli::probe::ProbeTimings::STANDARD,
            &Out::capture().0,
        )
        .await
        .expect_err("unknown");
        assert_eq!(
            error.to_string(),
            format!(
                "Ghost doesn't know an agent called \"gemini\"; the agents are {}",
                ghost_agents::catalog()
                    .iter()
                    .map(|spec| spec.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        );
        headless.shutdown().await;
    }
}
