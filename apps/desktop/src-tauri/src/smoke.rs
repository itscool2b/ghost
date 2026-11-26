//! Smoke mode (`GHOST_SMOKE`): the UI runs a scripted check against the real shell and reports
//! here; this side prints one machine-readable line and, with `GHOST_SMOKE_EXIT=1`, exits with
//! the verdict.
//!
//! The line is `GHOST_SMOKE_RESULT <json>` on stdout. The verdict is re-checked here rather than
//! taken on trust, and a watchdog reports a failure if the UI never reports at all, so a frontend
//! that is broken badly enough to never get there still ends the run with a clear result.
//!
//! There are four scenarios ([`SmokeScenario`]). The stream check measures demo streams; the chat
//! scenarios drive the chat screens and report one entry per check they define
//! ([`expected_checks`]), so a run that skipped a check fails here even when the UI says it passed.

use std::io::Write as _;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Starts the stdout line, so automation can find it among the logs.
pub const RESULT_PREFIX: &str = "GHOST_SMOKE_RESULT";

/// Which scripted check a smoke launch runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SmokeScenario {
    /// A demo stream, a cancelled stream and a stress burst (`GHOST_SMOKE=1`).
    Stream,
    /// The chat screens against the demo agent in a new data folder, ending with the quit flow
    /// while a turn runs (`GHOST_SMOKE=chat`).
    Chat,
    /// The same data folder started again after `chat`: the stored history, the turn the quit
    /// interrupted, the prompt it held, and Resume (`GHOST_SMOKE=chat-reopen`).
    ChatReopen,
    /// One minimal chat with `GHOST_SMOKE_AGENT`'s agent (`GHOST_SMOKE=chat` with that set).
    /// For local runs only: a real agent spends the user's quota.
    Agent,
}

impl SmokeScenario {
    /// The name the report and the logs use.
    pub fn name(self) -> &'static str {
        match self {
            SmokeScenario::Stream => "stream",
            SmokeScenario::Chat => "chat",
            SmokeScenario::ChatReopen => "chat_reopen",
            SmokeScenario::Agent => "agent",
        }
    }
}

/// How long the UI has to report. The stream check takes a few seconds and covers a cold Vite dev
/// server with a wide margin; the chat run takes about a minute; the agent run may first download
/// its adapter (up to 10 minutes) before its one turn.
pub fn watchdog(scenario: SmokeScenario) -> Duration {
    match scenario {
        SmokeScenario::Stream => Duration::from_secs(120),
        SmokeScenario::Chat => Duration::from_secs(300),
        SmokeScenario::ChatReopen => Duration::from_secs(120),
        SmokeScenario::Agent => Duration::from_secs(900),
    }
}

/// The checks of the chat scenario, in the order it makes them (`lib/smokeChat.ts`).
pub const CHAT_CHECKS: [&str; 34] = [
    "chats-screen",
    "pick-folder",
    "create-chat",
    "first-message",
    "pickers",
    "slash-menu",
    "echo-turn",
    "demo-turn",
    "option-change",
    "auto-approved",
    "autonomy-ask",
    "permission-allow",
    "permission-reject",
    "permission-withdrawn",
    "question-answer",
    "question-dismiss",
    "big-output",
    "background-output",
    "cancel-turn",
    "queue-after-turn",
    "queue-held-at-stop",
    "error-card",
    "queue-held-at-error",
    "crash-card",
    "queue-held-at-crash",
    "auth-card",
    "auth-retry",
    "force-stop",
    "send-starts-agent",
    "settings-customize-light",
    "settings-customize-dark",
    "customize-instructions-scopes",
    "transcript",
    "quit-with-turn",
];

/// The checks of the chat scenario's second launch.
pub const CHAT_REOPEN_CHECKS: [&str; 8] = [
    "reopen-list",
    "reopen-no-start",
    "reopen-history",
    "reopen-interrupted",
    "reopen-held",
    "reopen-blob",
    "reopen-resume",
    "reopen-send-queued",
];

/// The checks of the minimal chat with a real agent.
pub const AGENT_CHECKS: [&str; 3] = ["agent-available", "agent-starts", "agent-replies"];

/// The checks a scenario must report, each exactly once; none for the stream check, whose verdict
/// rests on its measurements.
pub fn expected_checks(scenario: SmokeScenario) -> &'static [&'static str] {
    match scenario {
        SmokeScenario::Stream => &[],
        SmokeScenario::Chat => &CHAT_CHECKS,
        SmokeScenario::ChatReopen => &CHAT_REOPEN_CHECKS,
        SmokeScenario::Agent => &AGENT_CHECKS,
    }
}

/// What the stream check measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct StreamMeasures {
    /// Events applied across every stream of the run.
    pub events_received: u32,
    /// Frames (`EventBatch`es) received across every stream of the run.
    pub frames_received: u32,
    /// Sequence gaps the stream reducer detected. Any gap fails the run.
    pub seq_gaps: u32,
    /// Characters of streamed answer text present in the rendered DOM.
    pub rendered_chars: u32,
    /// Lowest frame rate over half-second windows of the stress burst.
    pub fps_min: f64,
    /// Average frame rate over the stress burst.
    pub fps_avg: f64,
}

/// One check a chat scenario made: its name, whether it held, and what was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SmokeCheck {
    pub name: String,
    pub ok: bool,
    /// What the check saw, or why it failed (or did not run).
    pub detail: String,
}

/// What the UI's smoke run found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SmokeReport {
    pub ok: bool,
    /// The scenario that ran; `null` only in a report made before any could (a smoke launch whose
    /// switches were not valid).
    pub scenario: Option<SmokeScenario>,
    /// The stream check's measurements; `null` for the chat scenarios.
    pub stream: Option<StreamMeasures>,
    /// Every check a chat scenario defines, in order; empty for the stream check.
    pub checks: Vec<SmokeCheck>,
    pub errors: Vec<String>,
}

/// The report as printed: `ok` holds only if the UI said so, `asked` is the scenario that ran, and
/// what it reported agrees: the stream check's numbers, or every check of a chat scenario present
/// exactly once and passing.
pub fn finalize(mut report: SmokeReport, asked: SmokeScenario) -> SmokeReport {
    let mut problems = Vec::new();
    if report.scenario != Some(asked) {
        let ran = report.scenario.map_or("no scenario", SmokeScenario::name);
        problems.push(format!(
            "the UI ran {ran}, but this launch asked for {}",
            asked.name()
        ));
    }
    match asked {
        SmokeScenario::Stream => stream_problems(report.stream.as_mut(), &mut problems),
        scenario => check_problems(&report.checks, expected_checks(scenario), &mut problems),
    }
    report.ok = report.ok && report.errors.is_empty() && problems.is_empty();
    report.errors.extend(problems);
    report
}

fn stream_problems(stream: Option<&mut StreamMeasures>, problems: &mut Vec<String>) {
    let Some(stream) = stream else {
        problems.push("the stream check reported no measurements".to_string());
        return;
    };
    if stream.events_received == 0 {
        problems.push("no stream events were received".to_string());
    }
    if stream.frames_received == 0 {
        problems.push("no stream frames were received".to_string());
    }
    if stream.seq_gaps > 0 {
        problems.push(format!(
            "the stream skipped sequence numbers {} time(s)",
            stream.seq_gaps
        ));
    }
    if stream.rendered_chars == 0 {
        problems.push("no streamed text was rendered".to_string());
    }
    for fps in [&mut stream.fps_min, &mut stream.fps_avg] {
        // JSON has no NaN or infinity; serde would write null and break consumers expecting numbers.
        if !fps.is_finite() {
            *fps = 0.0;
        }
    }
}

/// Every expected check present exactly once and passing, and nothing else.
fn check_problems(checks: &[SmokeCheck], expected: &[&str], problems: &mut Vec<String>) {
    let failed: Vec<&str> = checks
        .iter()
        .filter(|check| !check.ok)
        .map(|check| check.name.as_str())
        .collect();
    if !failed.is_empty() {
        problems.push(format!(
            "{} of {} checks failed: {}",
            failed.len(),
            expected.len(),
            failed.join(", ")
        ));
    }
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|name| !checks.iter().any(|check| check.name == *name))
        .collect();
    if missing.len() == expected.len() && !expected.is_empty() {
        problems.push(format!("none of the {} checks ran", expected.len()));
    } else if !missing.is_empty() {
        problems.push(format!(
            "{} of {} checks never reported: {}",
            missing.len(),
            expected.len(),
            missing.join(", ")
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for check in checks {
        let name = check.name.as_str();
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        if !expected.contains(&name) {
            problems.push(format!(
                "the UI reported a check this scenario lacks: {name}"
            ));
            continue;
        }
        let times = checks.iter().filter(|other| other.name == name).count();
        if times > 1 {
            problems.push(format!("check {name} was reported {times} times"));
        }
    }
}

/// The report the watchdog prints when the UI never reported.
pub fn timed_out(scenario: SmokeScenario, after: Duration) -> SmokeReport {
    SmokeReport {
        ok: false,
        scenario: Some(scenario),
        stream: None,
        checks: Vec::new(),
        errors: vec![format!(
            "the UI did not report a smoke result within {} seconds",
            after.as_secs()
        )],
    }
}

/// The report printed when the app could not create its window, so the UI never ran: the run
/// still ends with a result line rather than only an exit code.
pub fn no_window(scenario: SmokeScenario, reason: &str) -> SmokeReport {
    SmokeReport {
        errors: vec![format!("Ghost could not create its window: {reason}")],
        ..timed_out(scenario, Duration::ZERO)
    }
}

/// The report printed when the smoke switches cannot run any scenario (an unknown
/// `GHOST_SMOKE` value, a chat run without its folder, ...): the UI never starts one.
pub fn setup_failed(scenario: Option<SmokeScenario>, reason: &str) -> SmokeReport {
    SmokeReport {
        ok: false,
        scenario,
        stream: None,
        checks: Vec::new(),
        errors: vec![format!("the smoke run cannot start: {reason}")],
    }
}

/// `GHOST_SMOKE_RESULT {"ok":true,...}`
pub fn result_line(report: &SmokeReport) -> String {
    let json = serde_json::to_string(report).unwrap_or_else(|error| {
        // Unreachable for this struct (finite numbers, string keys); still a valid failing line.
        format!(r#"{{"ok":false,"errors":["could not serialize the report: {error}"]}}"#)
    });
    format!("{RESULT_PREFIX} {json}")
}

pub fn exit_code(report: &SmokeReport) -> i32 {
    if report.ok { 0 } else { 1 }
}

/// Prints the result line on stdout and flushes, so it is out before any exit.
pub fn print(report: &SmokeReport) {
    let line = result_line(report);
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = writeln!(stdout, "{line}").and_then(|()| stdout.flush()) {
        tracing::error!(%error, "could not print the smoke result");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measures() -> StreamMeasures {
        StreamMeasures {
            events_received: 120,
            frames_received: 40,
            seq_gaps: 0,
            rendered_chars: 900,
            fps_min: 55.0,
            fps_avg: 59.8,
        }
    }

    fn passing() -> SmokeReport {
        SmokeReport {
            ok: true,
            scenario: Some(SmokeScenario::Stream),
            stream: Some(measures()),
            checks: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn check(name: &str, ok: bool) -> SmokeCheck {
        SmokeCheck {
            name: name.to_string(),
            ok,
            detail: format!("{name} detail"),
        }
    }

    fn chat_passing(scenario: SmokeScenario) -> SmokeReport {
        SmokeReport {
            ok: true,
            scenario: Some(scenario),
            stream: None,
            checks: expected_checks(scenario)
                .iter()
                .map(|name| check(name, true))
                .collect(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn a_clean_report_passes_unchanged() {
        let report = finalize(passing(), SmokeScenario::Stream);
        assert_eq!(report, passing());
        assert_eq!(exit_code(&report), 0);
    }

    #[test]
    fn the_ui_cannot_claim_success_with_bad_numbers() {
        let report = finalize(
            SmokeReport {
                stream: Some(StreamMeasures {
                    seq_gaps: 2,
                    rendered_chars: 0,
                    ..measures()
                }),
                ..passing()
            },
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
        assert_eq!(exit_code(&report), 1);
        assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
    }

    #[test]
    fn a_stream_report_without_measurements_fails() {
        let report = finalize(
            SmokeReport {
                stream: None,
                ..passing()
            },
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
        assert_eq!(
            report.errors,
            vec!["the stream check reported no measurements".to_string()]
        );
    }

    #[test]
    fn reported_errors_fail_the_run() {
        let report = finalize(
            SmokeReport {
                errors: vec!["demo turn timed out".into()],
                ..passing()
            },
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
        assert_eq!(report.errors, vec!["demo turn timed out".to_string()]);
    }

    #[test]
    fn a_failing_verdict_stays_failing() {
        let report = finalize(
            SmokeReport {
                ok: false,
                ..passing()
            },
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
    }

    #[test]
    fn non_finite_frame_rates_become_zero() {
        let report = finalize(
            SmokeReport {
                stream: Some(StreamMeasures {
                    fps_min: f64::NAN,
                    fps_avg: f64::INFINITY,
                    ..measures()
                }),
                ..passing()
            },
            SmokeScenario::Stream,
        );
        let stream = report.stream.clone().expect("measures");
        assert_eq!((stream.fps_min, stream.fps_avg), (0.0, 0.0));
        assert!(result_line(&report).contains(r#""fps_min":0.0"#));
    }

    #[test]
    fn the_line_is_the_prefix_and_one_json_object() {
        for report in [passing(), chat_passing(SmokeScenario::Chat)] {
            let line = result_line(&report);
            let json = line.strip_prefix("GHOST_SMOKE_RESULT ").expect("prefix");
            assert!(!json.contains('\n'));
            let parsed: SmokeReport = serde_json::from_str(json).expect("json");
            assert_eq!(parsed, report);
        }
    }

    #[test]
    fn the_report_names_its_scenario_in_snake_case() {
        let line = result_line(&chat_passing(SmokeScenario::ChatReopen));
        assert!(line.contains(r#""scenario":"chat_reopen""#), "{line}");
        assert!(line.contains(r#""stream":null"#), "{line}");
        for scenario in [
            SmokeScenario::Stream,
            SmokeScenario::Chat,
            SmokeScenario::ChatReopen,
            SmokeScenario::Agent,
        ] {
            assert_eq!(
                serde_json::to_value(scenario).expect("serialize"),
                serde_json::json!(scenario.name())
            );
        }
    }

    #[test]
    fn a_complete_chat_report_passes() {
        for scenario in [
            SmokeScenario::Chat,
            SmokeScenario::ChatReopen,
            SmokeScenario::Agent,
        ] {
            let report = finalize(chat_passing(scenario), scenario);
            assert!(report.ok, "{scenario:?}: {:?}", report.errors);
            assert!(report.errors.is_empty());
        }
    }

    #[test]
    fn a_failed_check_fails_the_chat_run_whatever_the_ui_says() {
        let mut report = chat_passing(SmokeScenario::Chat);
        report.checks[6].ok = false;
        report.checks[10].ok = false;
        let report = finalize(report, SmokeScenario::Chat);
        assert!(!report.ok);
        assert_eq!(
            report.errors,
            vec!["2 of 34 checks failed: echo-turn, autonomy-ask".to_string()]
        );
    }

    /// A UI that stopped early and still said it passed fails here.
    #[test]
    fn a_chat_run_must_report_every_check() {
        let mut report = chat_passing(SmokeScenario::Chat);
        report.checks.truncate(CHAT_CHECKS.len() - 2);
        let report = finalize(report, SmokeScenario::Chat);
        assert!(!report.ok);
        assert_eq!(
            report.errors,
            vec!["2 of 34 checks never reported: transcript, quit-with-turn".to_string()]
        );
    }

    #[test]
    fn unknown_and_repeated_checks_fail_the_chat_run() {
        let mut report = chat_passing(SmokeScenario::ChatReopen);
        report.checks.push(check("reopen-list", true));
        report.checks.push(check("made-up", true));
        let report = finalize(report, SmokeScenario::ChatReopen);
        assert!(!report.ok);
        assert_eq!(
            report.errors,
            vec![
                "check reopen-list was reported 2 times".to_string(),
                "the UI reported a check this scenario lacks: made-up".to_string(),
            ]
        );
    }

    #[test]
    fn the_report_must_be_for_the_scenario_the_launch_asked_for() {
        let report = finalize(chat_passing(SmokeScenario::Chat), SmokeScenario::ChatReopen);
        assert!(!report.ok);
        assert_eq!(
            report.errors[0],
            "the UI ran chat, but this launch asked for chat_reopen"
        );
        let report = finalize(passing(), SmokeScenario::Agent);
        assert_eq!(
            report.errors,
            vec![
                "the UI ran stream, but this launch asked for agent".to_string(),
                "none of the 3 checks ran".to_string(),
            ]
        );
    }

    #[test]
    fn every_scenario_names_distinct_checks() {
        let mut all: Vec<&str> = Vec::new();
        for scenario in [
            SmokeScenario::Chat,
            SmokeScenario::ChatReopen,
            SmokeScenario::Agent,
        ] {
            for name in expected_checks(scenario) {
                assert!(!all.contains(name), "{name} is listed twice");
                assert!(
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
                    "{name} is not kebab-case"
                );
                all.push(name);
            }
        }
        assert!(expected_checks(SmokeScenario::Stream).is_empty());
    }

    #[test]
    fn a_missing_window_fails_with_its_reason() {
        let report = finalize(
            no_window(SmokeScenario::Stream, "WebView2 is not installed"),
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
        assert_eq!(exit_code(&report), 1);
        assert_eq!(
            report.errors[0],
            "Ghost could not create its window: WebView2 is not installed"
        );
        assert!(
            !report.errors.iter().any(|e| e.contains("seconds")),
            "not reported as a timeout: {:?}",
            report.errors
        );
    }

    #[test]
    fn the_watchdog_report_fails() {
        let after = watchdog(SmokeScenario::Stream);
        let report = finalize(
            timed_out(SmokeScenario::Stream, after),
            SmokeScenario::Stream,
        );
        assert!(!report.ok);
        assert!(report.errors[0].contains("120 seconds"));
        assert_eq!(exit_code(&report), 1);

        let report = finalize(
            timed_out(SmokeScenario::Chat, watchdog(SmokeScenario::Chat)),
            SmokeScenario::Chat,
        );
        assert_eq!(
            report.errors,
            vec![
                "the UI did not report a smoke result within 300 seconds".to_string(),
                "none of the 34 checks ran".to_string(),
            ]
        );
    }

    #[test]
    fn every_scenario_gets_the_ui_long_enough() {
        assert!(watchdog(SmokeScenario::Chat) > watchdog(SmokeScenario::Stream));
        // The agent run may download its adapter first (10 minutes at most).
        assert!(watchdog(SmokeScenario::Agent) > Duration::from_secs(600));
    }

    #[test]
    fn a_setup_failure_reports_why_without_a_scenario() {
        let report = setup_failed(None, "GHOST_SMOKE=\"x\" names no smoke run");
        assert!(!report.ok);
        assert_eq!(report.scenario, None);
        assert_eq!(
            report.errors,
            vec!["the smoke run cannot start: GHOST_SMOKE=\"x\" names no smoke run".to_string()]
        );
        assert!(result_line(&report).contains(r#""scenario":null"#));
    }
}
