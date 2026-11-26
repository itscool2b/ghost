//! The demo agent end to end through `probe` and `smoke`: the real headless runtime (chat store,
//! leftover registry, detector, drivers) on temporary roots, with a fixed env whose `PATH` finds no
//! agent, so nothing reaches a real one.

#[cfg(test)]
mod common;

#[cfg(test)]
mod demo {
    use std::time::Duration;

    use ghost_cli::args::{ProbeArgs, Scenario, SmokeArgs};
    use ghost_cli::chat::{ChatRun, StartEnd};
    use ghost_cli::out::Out;
    use ghost_cli::probe::{self, ProbeTimings};
    use ghost_cli::repo::{self, RepoKind};
    use ghost_cli::report::Outcome;
    use ghost_cli::smoke::{self, SmokeTimings};
    use ghost_cli::watch::Watch;
    use ghost_protocol::SessionStatus;

    use crate::common::{Machine, assert_no_failures, assert_passed, lines};

    /// The standard waits, but short where the demo agent needs no time.
    const PROBE: ProbeTimings = ProbeTimings {
        commands: Duration::from_millis(300),
        ..ProbeTimings::STANDARD
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn probe_runs_every_free_step_with_the_demo_agent() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Hostile, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = probe::run(
            &headless,
            &ProbeArgs {
                agent: "mock".into(),
                env: vec![("GHOST_PROBE_TEST".into(), "1".into())],
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
            "detect: ready (built in)",
            "start: ready in",
            "2 session: opened session mock-",
            "4 options: every option set to its current value (4); nothing changed",
            "4 options: the model switched to Mock Smart (mock-smart)",
            "4 options: the model switched back to Mock Fast (mock-fast)",
            "4 autonomy: Full auto: mode full_auto, answers every tool kind itself",
            "4 autonomy: Auto-edit: mode auto_edit",
            "4 autonomy: Ask: mode ask, Ghost does not auto-answer remaining permission requests",
            "5 cancel: an interrupt while idle left the session ready and sent 0 session/cancel",
            "6 close: the demo agent stopped",
            "9 interrupt: an interrupt during the start stopped it",
            "9 interrupt: the next start was ready",
        ] {
            assert_passed(&output, needle);
        }
        assert!(
            output.contains(
                "INFO  env: set on every agent process, after the scrub: GHOST_PROBE_TEST"
            ),
            "{output}"
        );
        assert!(
            output.contains("INFO  3 commands: 40 slash command(s) after 0.3 s"),
            "{output}"
        );
        // The demo agent never continues a session (spec 5.7: a resume quietly starts new).
        assert!(
            output.contains("INFO  7 resume: a new session (mock-")
                && output.contains("this agent can't continue earlier sessions"),
            "{output}"
        );
        assert_passed(
            &output,
            "7 resume: no transcript came back while the session opened",
        );
        assert_passed(&output, "7 resume: the options are back as they were");
        assert!(
            output.contains("SKIP  0 install: Demo agent (scripted, no AI) is not an npm adapter"),
            "{output}"
        );
        assert!(
            output.contains("SKIP  1 initialize: the demo agent runs inside Ghost"),
            "{output}"
        );
        assert!(
            output.contains("The demo agent writes no protocol log."),
            "{output}"
        );
        assert!(lines(&output, "PASS").len() >= 15, "{output}");
        assert!(
            !headless.manager.has_live_sessions(),
            "the probe stopped its agent"
        );
        let ended = headless.shutdown().await;
        assert!(ended.saved);
        assert!(machine.registry_files().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn smoke_runs_every_scenario_with_the_demo_agent() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Plain, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = smoke::run(
            &headless,
            &SmokeArgs {
                agent: "mock".into(),
                scenarios: Scenario::ALL.to_vec(),
                model: Some("mock-smart".into()),
                effort: Some("Low".into()),
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
            "settings: model switched to Mock Smart (mock-smart)",
            "settings: effort switched to Low (low)",
            "basic: the reply streamed in",
            "basic: stop reason Some(EndTurn)",
            "basic: usage: 8 in",
            "scope Turn (expected Turn)",
            "permission: a card appeared (1 in all), rejected",
            "permission: after Reject the demo tool failed and the agent carried on",
            "permission: a card appeared (1 in all), allowed once",
            "permission: after Allow once the demo tool completed",
            "interrupt: interrupted after the first text: stop reason Some(Cancelled)",
            "interrupt: the partial answer is kept",
            "interrupt: the prompt queued before the interrupt is held, not sent",
            "interrupt: the held prompt was removed",
            "after-interrupt: the turn after the interrupt ended normally",
            "full-auto: 0 card(s) waited for the user",
            "full-auto: the demo tool ran without a card",
            "resume: the turn after the restart ended normally",
            "resume: every one of the 7 turns is stored once",
        ] {
            assert_passed(&output, needle);
        }
        assert!(
            output.contains("INFO  basic: the turn ran on the model Mock Smart"),
            "{output}"
        );
        assert!(
            output.contains("INFO  full-auto: auto-approved: \"Run `ls -la`\""),
            "{output}"
        );
        assert!(!headless.manager.has_live_sessions());
        assert!(headless.shutdown().await.saved);
        assert!(machine.registry_files().is_empty());
    }

    /// A chat waiting for a sign-in keeps no process (spec 6.5.1), so stopping it has nothing to
    /// wait for: the probe of an agent that needs one (codex-acp with an empty `CODEX_HOME`) once
    /// waited out the stop limit twice at its end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stopping_a_chat_that_waits_for_a_sign_in_ends_at_once() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Plain, &headless.registry)
            .await
            .expect("the chat folder");
        let watch = Watch::new();
        headless.manager.status_subscribe(watch.status_sink());
        let spec = headless.detector.spec("mock").expect("the demo agent");
        let chat = ChatRun::create(&headless, &watch, &spec, &folder)
            .await
            .expect("the chat");
        assert_eq!(
            chat.settle(Duration::from_secs(30)).await.0,
            StartEnd::Ready
        );
        chat.send("/auth").await.expect("the prompt is sent");
        assert!(
            chat.wait_status(SessionStatus::AuthRequired, Duration::from_secs(30))
                .await
        );
        let started = std::time::Instant::now();
        assert!(
            chat.stop(Duration::from_secs(3)).await.expect("stop"),
            "a chat with no process counts as stopped"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(chat.status(), Some(SessionStatus::AuthRequired));
        assert!(!headless.manager.has_live_sessions());
        assert!(headless.shutdown().await.saved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn smoke_stops_before_any_prompt_when_a_setting_is_not_offered() {
        let machine = Machine::new();
        let headless = machine.headless(&[]).await;
        let folder = repo::create(&machine.path("repo"), RepoKind::Plain, &headless.registry)
            .await
            .expect("the chat folder");
        let (out, captured) = Out::capture();
        let outcome = smoke::run(
            &headless,
            &SmokeArgs {
                agent: "mock".into(),
                scenarios: vec![Scenario::Basic],
                model: Some("mock-huge".into()),
                effort: None,
            },
            &folder,
            SmokeTimings::STANDARD,
            &out,
        )
        .await
        .expect("the smoke run runs");
        let output = captured.text();
        assert_eq!(outcome, Outcome::Failed, "{output}");
        assert!(
            output.contains("FAIL  settings: --model mock-huge is not offered; the choices are mock-fast, mock-smart"),
            "{output}"
        );
        assert!(!output.contains("basic:"), "no scenario ran: {output}");
        assert!(!headless.manager.has_live_sessions());
        headless.shutdown().await;
    }
}
