//! The probe and every smoke scenario against a real process: this package's fake ACP agent,
//! installed as `hermes` on the test env's `PATH`, through the real spawner and leftover registry,
//! the real ACP driver with the hermes dialect, real protocol logs and the process table. Then the
//! logs the runs left are curated into fixtures, which must replay as recorded. No real agent runs.

#[cfg(test)]
mod common;

#[cfg(test)]
mod process {
    use std::path::Path;
    use std::time::Duration;

    use ghost_cli::args::{FixtureArgs, ProbeArgs, Scenario, SmokeArgs};
    use ghost_cli::fixture::{self, CurateContext};
    use ghost_cli::logs;
    use ghost_cli::out::Out;
    use ghost_cli::probe::{self, ProbeTimings};
    use ghost_cli::repo::{self, RepoKind};
    use ghost_cli::report::Outcome;
    use ghost_cli::smoke::{self, SmokeTimings};

    use crate::common::{Machine, assert_no_failures, assert_passed, named_logs};

    const PROBE: ProbeTimings = ProbeTimings {
        commands: Duration::from_millis(500),
        ..ProbeTimings::STANDARD
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_text_fallback_free_probe_checks_detection_without_creating_a_chat() {
        let machine = Machine::new();
        let name = if cfg!(windows) { "grok.exe" } else { "grok" };
        std::fs::copy(crate::common::fake_agent(), machine.bin.join(name))
            .expect("install the fake version helper");
        let headless = machine.headless(&[("GHOST_FAKE_VERSION", "1.0.70")]).await;
        let (out, captured) = Out::capture();
        let result = probe::run(
            &headless,
            &ProbeArgs {
                agent: "grok-stream".into(),
                env: vec![],
                folder: None,
            },
            &machine.path("unused-folder"),
            false,
            PROBE,
            &out,
        )
        .await
        .expect("probe");
        assert_eq!(result, Outcome::Passed, "{}", captured.text());
        assert!(captured.text().contains("No chat or prompt was started."));
        assert!(headless.manager.chats().await.expect("chats").is_empty());
        assert!(headless.shutdown().await.saved);
    }

    /// Curates `log` into a fixture named `name` and checks that it replays as recorded. The
    /// replay runs as `ghost-cli fixture` runs it: on a thread of its own, on paused time.
    fn curates_and_replays(machine: &Machine, log: &Path, name: &str) {
        let (out, captured) = Out::capture();
        let fixture_path = machine.path("fixtures").join(format!("{name}.jsonl"));
        let args = FixtureArgs {
            log: log.to_path_buf(),
            scenario: name.into(),
            out: fixture_path.clone(),
        };
        let outcome = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .start_paused(true)
                        .build()
                        .expect("a paused runtime")
                        .block_on(fixture::run(&args, &CurateContext::here(), &out))
                })
                .join()
                .expect("the curation thread")
        })
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{name}:\n{output}");
        assert_passed(
            &output,
            "fixture: the driver wrote what the fixture recorded",
        );
        let curated = std::fs::read_to_string(&fixture_path).expect("the fixture");
        assert!(curated.contains("<CWD>"), "{name}: {curated}");
        assert!(
            !curated.contains(&machine.path("repo").display().to_string()),
            "{name}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_runs_every_step_against_a_real_process() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless_setting(
                // Settings answer late, as a real agent's do: each autonomy level must still reach
                // the agent before the walk goes on.
                &[
                    ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                    ("GHOST_FAKE_SET_DELAY_MS", "150"),
                ],
                &[("GHOST_PROBE_SET", "a value that is never logged")],
            )
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: vec![(
                    "GHOST_PROBE_SET".into(),
                    "a value that is never logged".into(),
                )],
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        for needle in [
            "detect: Hermes 0.22.5 (",
            "start: ready in",
            "1 initialize: fake-hermes 0.22.5 answered in",
            "1 initialize: nothing ran the hostile chat folder's fake adapters",
            "2 session: opened session fake-",
            "4 options: every option set to its current value (2); nothing changed",
            "4 options: the model switched to fake/mini (fake:mini)",
            "4 options: the model switched back to fake/large (fake:large)",
            "4 autonomy: Full auto: mode dont_ask, answers execute itself",
            "4 autonomy: Auto-edit: mode accept_edits, Ghost does not auto-answer remaining permission requests",
            "4 autonomy: Ask: mode default, Ghost does not auto-answer remaining permission requests",
            "5 cancel: an interrupt while idle left the session ready and sent 0 session/cancel",
            "6 close: the agent exited when stdin closed: steps ClosedStdin, ExitedAfterEof",
            "7 resume: no transcript came back while the session opened",
            "7 resume: the options are back as they were",
            "9 interrupt: an interrupt after initialize stopped the start",
            "9 interrupt: the next start was ready",
        ] {
            assert_passed(&output, needle);
        }
        assert!(
            output.contains("PASS  7 resume: the session fake-") && output.contains(" continued\n"),
            "{output}"
        );
        for needle in [
            "INFO  1 initialize: capabilities: ",
            "auth methods: hermes-setup",
            "INFO  2 session: the open's reply: 2 legacy model(s) (current fake:large); modes default, accept_edits, dont_ask (current default)",
            "INFO  3 commands: 3 slash command(s) after 0.5 s; the largest commands update was",
            "INFO  4 options: while switched, a second session reports fake:large: the switch stays inside the session",
            "INFO  7 resume: opened with session/resume",
        ] {
            assert!(output.contains(needle), "{needle:?} missing:\n{output}");
        }
        if cfg!(unix) {
            for step in ["6 close", "9 interrupt", "end"] {
                assert_passed(&output, &format!("{step}: nothing left running"));
            }
        }

        let logs = named_logs(&output);
        let names: Vec<&str> = logs.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "probe-session",
                "probe-second-session",
                "probe-resume",
                "probe-interrupted-start",
                "probe-restart"
            ],
            "{output}"
        );
        // `--env` reaches every agent process by name; its value is never logged.
        let first = logs::read(&logs[0].0).expect("the session's log").records;
        let set = logs::note(&first, "spawn")
            .and_then(|spawn| spawn.get("set"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(set, serde_json::json!(["GHOST_PROBE_SET"]));
        // Full auto at the start, then the walk: Auto-edit, Ask and Full auto again, each mode
        // sent once the one before it landed (a later change replaces one still on its way).
        let modes: Vec<&str> = logs::requests_of(&first, &["session/set_mode"])
            .filter_map(logs::message)
            .filter_map(|msg| msg.pointer("/params/modeId").and_then(|id| id.as_str()))
            .collect();
        assert_eq!(
            modes,
            ["dont_ask", "accept_edits", "default", "dont_ask"],
            "{output}"
        );
        for (log, _) in &logs {
            let text = std::fs::read_to_string(log).expect("a log");
            assert!(
                !text.contains("a value that is never logged"),
                "{}",
                log.display()
            );
        }
        assert!(
            !headless.manager.has_live_sessions(),
            "the probe stopped every agent"
        );
        assert!(headless.shutdown().await.saved);
        assert!(machine.registry_files().is_empty());

        for (log, name) in logs
            .iter()
            .filter(|(_, name)| name != "probe-interrupted-start")
        {
            curates_and_replays(&machine, log, name);
        }
    }

    /// A force stop (an interrupt while the agent starts) reads `Stopped` at once, while its FORCE
    /// chain still runs: a child in the agent's group that takes a while to exit on SIGTERM (as
    /// claude-agent-acp's `claude auth status` does) is still there right then. The probe's process
    /// table check waits for the chain instead of reporting it as left running.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_waits_for_a_forced_stop_to_end_the_agents_group() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless(&[
                ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                ("GHOST_FAKE_SLOW_CHILD_MS", "1500"),
            ])
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert_passed(
            &output,
            "9 interrupt: an interrupt after initialize stopped the start",
        );
        for step in ["6 close", "9 interrupt", "end"] {
            assert_passed(&output, &format!("{step}: nothing left running"));
        }
        assert!(!headless.manager.has_live_sessions());
        assert!(headless.shutdown().await.saved);
    }

    /// claude-acp and codex-acp keep a session only from its first prompt, so a probe, which
    /// never prompts, can't continue one: the agent answers the resume as not found, and Ghost
    /// starts a new session and says so. That is the agent's way, not a failed resume.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_reads_a_refused_resume_of_a_session_never_prompted() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless(&[
                ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                ("GHOST_FAKE_FORGET_UNPROMPTED", "1"),
            ])
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert!(
            output.contains("INFO  7 resume: the agent keeps no session it was never prompted in: it answered the resume -32002 (Resource not found: fake-"),
            "{output}"
        );
        assert!(
            output.contains("Ghost started a new session (fake-")
                && output.contains("and said so: \"Could not continue the earlier session (Resource not found: fake-"),
            "{output}"
        );
        assert_passed(&output, "7 resume: the options are back as they were");
        assert_passed(&output, "9 interrupt: the next start was ready");
        assert!(headless.shutdown().await.saved);
    }

    /// Hermes 0.22.5 can't switch a session back to its configured model when that model's
    /// provider is `auto`: `auto:<model>` reads as a model of the provider in use. After a restart
    /// the session runs another model and no longer offers the configured one; Ghost says so in
    /// a notice. The probe reports that as the agent's doing, not as options Ghost lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_reads_a_model_the_agent_no_longer_offers_after_the_restart() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless(&[
                ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                ("GHOST_FAKE_HERMES_MODELS", "1"),
            ])
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert_passed(
            &output,
            "4 options: the model switched to fake · mini (fake:mini)",
        );
        assert!(
            output.contains("INFO  4 options: while switched, a second session reports auto:large: the switch stays inside the session"),
            "{output}"
        );
        assert!(
            output.contains("INFO  7 resume: model: the agent no longer offers auto:large and runs fake · auto:large (fake:auto:large); Ghost said so: \"The model you chose earlier (auto:large) isn't offered anymore; using fake · auto:large.\""),
            "{output}"
        );
        assert!(headless.shutdown().await.saved);
    }

    /// codex-acp's effort follows its model: a model that offers no `ultra` sets `medium`, and
    /// switching back keeps it. A model change hands the thinking level to the agent, so a restart
    /// brings back the level the agent showed with the model rather than the one chosen before the
    /// switch; the probe says what the switch changed and checks the values a restart restores.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_tells_an_option_the_agent_changed_with_the_model_from_a_lost_one() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless(&[
                ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                ("GHOST_FAKE_EFFORT", "1"),
            ])
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert!(
            output.contains("INFO  4 options: switching the model changed effort (High (high) -> Medium (medium)), and switching back left it"),
            "{output}"
        );
        assert_passed(&output, "7 resume: the options are back as they were");
        assert!(headless.shutdown().await.saved);
    }

    /// From its `nous` provider, which checks the model, Hermes 0.22.5 refuses the switch back to
    /// its configured `auto:` model outright. The probe says the agent refused a model it offers,
    /// and the restart brings back the model that was set last.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_reads_a_refused_switch_back_as_the_agents_doing() {
        let machine = Machine::with_fake_hermes();
        let headless = machine
            .headless(&[
                ("GHOST_FAKE_OPEN_DELAY_MS", "300"),
                ("GHOST_FAKE_HERMES_MODELS", "refuse"),
            ])
            .await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert_passed(
            &output,
            "4 options: the model switched to fake · mini (fake:mini)",
        );
        assert!(
            output.contains("INFO  4 options: the agent refused to switch back to auto · large (auto:large), a model it offers itself: Internal error"),
            "{output}"
        );
        assert_passed(&output, "7 resume: the options are back as they were");
        assert!(headless.shutdown().await.saved);
    }

    /// Step 8: an agent that needs a sign-in shows its card, and its protocol log is offered as
    /// the `probe-auth` fixture like every other probe's (spec 10.2).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_shows_the_sign_in_card_and_offers_its_log() {
        let machine = Machine::with_fake_hermes();
        let headless = machine.headless(&[("GHOST_FAKE_AUTH_REQUIRED", "1")]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "hermes".into(),
                env: Vec::new(),
                folder: None,
            },
            &folder,
            true,
            PROBE,
            &out,
        )
        .await
        .expect("the probe runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        assert!(
            output
                .contains("INFO  8 auth: the agent needs a sign-in: Hermes needs you to sign in."),
            "{output}"
        );
        let logs = named_logs(&output);
        let names: Vec<&str> = logs.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(names, ["probe-auth"], "{output}");
        assert!(headless.shutdown().await.saved);
        curates_and_replays(&machine, &logs[0].0, "probe-auth");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn smoke_runs_every_scenario_against_a_real_process() {
        let machine = Machine::with_fake_hermes();
        let headless = machine.headless(&[]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Plain, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = smoke::run(
            &headless,
            &SmokeArgs {
                agent: "hermes".into(),
                scenarios: Scenario::ALL.to_vec(),
                model: Some("fake:mini".into()),
                effort: None,
            },
            &folder,
            SmokeTimings::STANDARD,
            &out,
        )
        .await
        .expect("the smoke run runs");
        let output = captured.text();
        assert_no_failures(&output);
        assert_eq!(outcome, Outcome::Passed, "{output}");
        for needle in [
            "start: the agent is ready",
            "settings: model switched to fake/mini (fake:mini)",
            "basic: the reply streamed in",
            "basic: stop reason Some(EndTurn)",
            "scope Session (expected Session)",
            "permission: a card appeared (1 in all), rejected",
            "permission: after Reject there is no aura-perm.txt, and the agent carried on",
            "permission: a card appeared (1 in all), allowed once",
            "permission: after Allow once aura-perm.txt exists",
            "interrupt: interrupted after the first text: stop reason Some(Cancelled)",
            "interrupt: the partial answer is kept",
            "interrupt: the prompt queued before the interrupt is held, not sent",
            "interrupt: the held prompt was removed",
            "after-interrupt: the agent says its last number was",
            "full-auto: 0 card(s) waited for the user",
            "full-auto: aura-auto.txt holds \"hi\"",
            "resume: the reply names aura-perm.txt and aura-auto.txt",
            "resume: every one of the 7 turns is stored once",
        ] {
            assert_passed(&output, needle);
        }
        assert!(
            output.contains("INFO  basic: the turn ran on the model fake/mini"),
            "{output}"
        );
        assert!(
            !output.contains("opened a new session"),
            "every restart resumed: {output}"
        );
        assert!(folder.join("aura-perm.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(folder.join("aura-auto.txt")).expect("the file"),
            "hi\n"
        );
        let logs = named_logs(&output);
        let names: Vec<&str> = logs.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(
            names,
            Scenario::ALL
                .iter()
                .map(|scenario| scenario.name())
                .collect::<Vec<_>>(),
            "one log per scenario:\n{output}"
        );
        assert!(!headless.manager.has_live_sessions());
        assert!(headless.shutdown().await.saved);
        assert!(machine.registry_files().is_empty());

        for (log, name) in &logs {
            curates_and_replays(&machine, log, name);
        }
    }
}
