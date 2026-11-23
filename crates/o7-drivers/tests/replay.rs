//! Every fixture under `tests/fixtures` replays to its golden, keeps the driver contract, and
//! holds no personal data. `O7_UPDATE_GOLDEN=1` rewrites the goldens.
#![cfg(feature = "replay")]

#[cfg(test)]
mod replay {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use o7_drivers::driver::{DriverEvent, LoggedCommand};
    use o7_drivers::replay::{
        Fixture, FixtureRecord, ReplayReport, assert_golden, golden_path, run, scan_clean,
    };
    use o7_protocol::O7Event;
    use serde_json::Value;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Every `.jsonl` fixture, sorted.
    fn fixtures() -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut folders = vec![fixture_root()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("a fixture folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    fn events_of(report: &ReplayReport) -> Vec<DriverEvent> {
        report
            .events
            .iter()
            .map(|event| serde_json::from_value(event.clone()).expect("a driver event"))
            .collect()
    }

    /// The driver contract (spec 5.1) over one replay: what may come before `Ready`, exactly one
    /// `TurnEnded` per prompt after every event tagged with its turn, tags that name prompted
    /// turns, and the events a driver never emits.
    fn contract_violations(fixture: &Fixture, report: &ReplayReport) -> Vec<String> {
        let events = events_of(report);
        let mut violations = Vec::new();
        let prompts: Vec<String> = fixture
            .records
            .iter()
            .filter_map(|record| match record {
                FixtureRecord::Cmd(LoggedCommand::Prompt { turn_id, .. }) => Some(turn_id.clone()),
                _ => None,
            })
            .collect();
        let readies = events
            .iter()
            .filter(|event| matches!(event, DriverEvent::Ready(_)))
            .count();
        let exit_type = report
            .exit
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if readies > 1 {
            violations.push(format!("{readies} Ready events"));
        }
        let cancelled_start = readies == 0
            && exit_type == "closed"
            && prompts.is_empty()
            && fixture
                .records
                .iter()
                .any(|record| matches!(record, FixtureRecord::Cmd(LoggedCommand::Shutdown)));
        if (readies == 0) != (exit_type == "start_failed" || cancelled_start) {
            violations.push(format!("{readies} Ready events and the exit {exit_type}"));
        }

        // Rule 1.
        let before_ready = events
            .iter()
            .take_while(|event| !matches!(event, DriverEvent::Ready(_)));
        for event in before_ready {
            let allowed = match event {
                DriverEvent::Status(_)
                | DriverEvent::RunState { .. }
                | DriverEvent::BackgroundWork { .. }
                | DriverEvent::Options(_)
                | DriverEvent::Commands(_)
                | DriverEvent::Title(_)
                | DriverEvent::Autonomy(_)
                | DriverEvent::AuthRequired(_) => true,
                DriverEvent::Event { turn: None, event } => {
                    matches!(event, O7Event::Notice { .. } | O7Event::UsageUpdated(_))
                }
                DriverEvent::Permission { turn: None, .. }
                | DriverEvent::UiRequest { turn: None, .. } => true,
                _ => false,
            };
            if !allowed {
                violations.push(format!("before Ready: {event:?}"));
            }
        }

        // Rules 2 and 8.
        let mut ended: BTreeMap<String, usize> = BTreeMap::new();
        for (index, event) in events.iter().enumerate() {
            let tag = match event {
                DriverEvent::Event { turn, .. }
                | DriverEvent::Permission { turn, .. }
                | DriverEvent::UiRequest { turn, .. } => turn.clone(),
                DriverEvent::TurnEnded { turn_id, .. } => {
                    if !prompts.contains(turn_id) {
                        violations
                            .push(format!("TurnEnded for {turn_id}, which no prompt started"));
                    }
                    if ended.insert(turn_id.clone(), index).is_some() {
                        violations.push(format!("a second TurnEnded for {turn_id}"));
                    }
                    None
                }
                _ => None,
            };
            if let Some(tag) = tag {
                if !prompts.contains(&tag) {
                    violations.push(format!("an event tagged {tag}, which no prompt started"));
                }
                if ended.contains_key(&tag) {
                    violations.push(format!(
                        "an event tagged {tag} after its TurnEnded: {event:?}"
                    ));
                }
            }
            if let DriverEvent::Event { event, .. } = event
                && matches!(
                    event,
                    O7Event::TurnStarted { .. }
                        | O7Event::TurnEnded { .. }
                        | O7Event::UserMessageDelta { .. }
                        | O7Event::SessionUpdated(_)
                        | O7Event::PromptLifecycle { .. }
                        | O7Event::PermissionRequested(_)
                        | O7Event::PermissionResolved { .. }
                        | O7Event::UiRequest(_)
                        | O7Event::UiResolved { .. }
                )
            {
                violations.push(format!("a driver never emits {event:?}"));
            }
        }
        for (position, turn_id) in prompts.iter().enumerate() {
            let last = position + 1 == prompts.len();
            let run_ended_first = matches!(exit_type, "crashed" | "closed");
            if !ended.contains_key(turn_id) && !(last && run_ended_first) {
                violations.push(format!("no TurnEnded for {turn_id}"));
            }
        }
        if fixture
            .start()
            .is_some_and(|start| start["agent_id"] == "claude")
        {
            violations.extend(native_contract_violations(fixture, &events, report));
        }
        if fixture
            .start()
            .is_some_and(|start| start["agent_id"] == "codex")
        {
            if fixture.name == "approval-available-decisions" {
                for event in &events {
                    if let DriverEvent::Permission { request, .. } = event
                        && request
                            .options
                            .iter()
                            .map(|option| option.option_id.as_str())
                            .collect::<Vec<_>>()
                            != ["accept", "cancel"]
                    {
                        violations.push("Codex approval offered an unadvertised decision".into());
                    }
                }
            }
            if fixture.name == "recorded-basic" {
                let text: String = events
                    .iter()
                    .filter_map(|event| match event {
                        DriverEvent::Event {
                            event: O7Event::MessageDelta { text, .. },
                            ..
                        } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if text != "ok" || !events.iter().any(|event| matches!(event, DriverEvent::TurnEnded { usage: Some(usage), .. } if usage.scope == o7_protocol::UsageScope::Turn && usage.input_tokens == 25745 && usage.output_tokens == 5)) {
                    violations.push("recorded Codex basic content or native turn usage changed".into());
                }
            }
        }
        violations
    }

    fn native_contract_violations(
        fixture: &Fixture,
        events: &[DriverEvent],
        report: &ReplayReport,
    ) -> Vec<String> {
        use o7_protocol::StopReason;
        let mut violations = Vec::new();
        if !events
            .iter()
            .take_while(|event| !matches!(event, DriverEvent::Ready(_)))
            .any(|event| matches!(event, DriverEvent::RunState { .. }))
        {
            violations.push("native driver did not opt in to run state before Ready".into());
        }
        let ended = |turn: &str, reason| {
            events.iter().any(|event| matches!(event, DriverEvent::TurnEnded { turn_id, stop_reason, .. } if turn_id == turn && *stop_reason == reason))
        };
        let stale_model = events.iter().any(|event| matches!(event,
            DriverEvent::Options(options) if options.iter().any(|option| matches!(
                &option.kind, o7_protocol::session::ConfigKind::Select { current, .. } if current == "late-model"
            ))
        ));
        match fixture.name.as_str() {
            "telemetry-merge" | "telemetry-coalesced" => {
                if fixture.name == "telemetry-coalesced" && events.iter().any(|event| matches!(event, DriverEvent::Event { event: O7Event::UsageUpdated(usage), .. } if usage.context_used == Some(100))) {
                    violations.push("stale pending context reply was emitted after the result requested a refresh".into());
                }
                let usage = events.iter().rev().find_map(|event| match event {
                    DriverEvent::Event { event: O7Event::UsageUpdated(usage), .. } => Some(usage),
                    _ => None,
                });
                if !usage.is_some_and(|usage| usage.context_used == Some(250) && usage.context_size == Some(1000) && usage.cost.as_ref().is_some_and(|cost| cost.amount == 0.03)) {
                    violations.push("telemetry refresh lost result spend or context occupancy".into());
                }
                let limits = events.iter().rev().find_map(|event| match event {
                    DriverEvent::Event { event: O7Event::LimitsUpdated { windows }, .. } => Some(windows),
                    _ => None,
                });
                if !limits.is_some_and(|windows| windows.iter().any(|window| window.used_percent == 25.0 && window.resets_at == Some(1792834799749))) {
                    violations.push("control telemetry quota percentages or reset times were translated incorrectly".into());
                }
                if !events.iter().any(|event| matches!(event, DriverEvent::TurnEnded { usage: Some(usage), .. } if usage.input_tokens == 3 && usage.output_tokens == 4 && usage.cache_read_tokens == Some(2))) {
                    violations.push("control telemetry replaced the result's per-turn token counters".into());
                }
            }
            "basic" if !events.iter().any(|event| matches!(event, DriverEvent::Event { event: O7Event::MessageDelta { text, .. }, .. } if text == "hello back")) => violations.push("native assistant content was not replayed".into()),
            "result-before-idle" => {
                // An open prompt refuses another; after its result, Claude Code queues a prompt
                // even while it still reports background work (the session sends one only after
                // its idle grace or the person's Send now).
                if !ended("TOO-EARLY", StopReason::Error) || !ended("T2", StopReason::EndTurn) {
                    violations.push("an open prompt accepted another, or a finished one refused the next while Claude still worked".into());
                }
            }
            "permission-withdrawal" => {
                let permission = events.iter().position(|event| matches!(event, DriverEvent::Permission { turn: None, request } if request.request_id == "permission-1"));
                let withdrawal = events.iter().position(|event| matches!(event, DriverEvent::PermissionWithdrawn { request_id } if request_id == "permission-1"));
                if !matches!((permission, withdrawal), (Some(request), Some(withdrawn)) if request < withdrawn) {
                    violations.push("background permission did not remain untagged and withdraw".into());
                }
            }
            "interrupt" if !ended("T1", StopReason::Cancelled) => violations.push("native interruption did not end as cancelled".into()),
            "bad-late-controls" if report.replies.len() != 2 || report.replies.iter().any(|reply| reply.pointer("/reply/ok").is_none()) || stale_model => {
                violations.push("late control reply affected the next setting request".into());
            }
            _ => {}
        }
        if fixture.name.starts_with("recorded-") {
            violations.extend(recorded_native_violations(fixture, events));
        }
        violations
    }

    fn recorded_native_violations(fixture: &Fixture, events: &[DriverEvent]) -> Vec<String> {
        use o7_protocol::{StopReason, ToolStatus, session::ConfigKind};
        let mut violations = Vec::new();
        let endings: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::TurnEnded {
                    stop_reason, usage, ..
                } => Some((stop_reason, usage)),
                _ => None,
            })
            .collect();
        let expected_turns = if fixture.name == "recorded-permission" {
            2
        } else {
            1
        };
        let expected_reason = if fixture.name == "recorded-interrupt" {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };
        if endings.len() != expected_turns
            || endings
                .iter()
                .any(|(reason, _)| **reason != expected_reason)
        {
            violations.push(
                "recorded prompts did not end exactly once with their recorded outcome".into(),
            );
        }
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::Event {
                    event: O7Event::MessageDelta { text, .. },
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let committed: String = fixture
            .records
            .iter()
            .filter_map(|record| match record {
                FixtureRecord::In(message)
                    if message["type"] == "assistant"
                        && message["parent_tool_use_id"].is_null() =>
                {
                    message
                        .pointer("/message/content")
                        .and_then(Value::as_array)
                }
                _ => None,
            })
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect();
        if fixture.name != "recorded-interrupt" && text != committed {
            violations.push(
                "streamed and committed assistant text duplicated or lost recorded content".into(),
            );
        }
        if events.iter().any(|event| matches!(event, DriverEvent::Options(options) if options.iter().any(|option|
            matches!(&option.kind, ConfigKind::Select { current, choices } if !choices.iter().any(|choice| choice.value == *current))))) {
            violations.push("recorded native metadata advertised a current value outside its choices".into());
        }
        let tools: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::Event {
                    event: O7Event::ToolCall(call),
                    ..
                } => Some(call),
                _ => None,
            })
            .collect();
        let tool_finished = |index: usize, status| {
            tools.get(index).is_some_and(|tool| {
                events.iter().any(|event| {
                    matches!(event, DriverEvent::Event { event: O7Event::ToolUpdate(update), .. }
                if update.tool_call_id == tool.tool_call_id && update.status == Some(status))
                })
            })
        };
        match fixture.name.as_str() {
            "recorded-basic" if text != "ok" => violations.push("basic response changed".into()),
            "recorded-permission" => {
                let decisions: Vec<_> = fixture
                    .records
                    .iter()
                    .filter_map(|record| match record {
                        FixtureRecord::Out(message) if message["type"] == "control_response" => {
                            message
                                .pointer("/response/response/behavior")
                                .and_then(Value::as_str)
                        }
                        _ => None,
                    })
                    .collect();
                let permissions = events
                    .iter()
                    .filter(|event| matches!(event, DriverEvent::Permission { .. }))
                    .count();
                if decisions != ["deny", "allow"]
                    || permissions != 2
                    || tools.len() != 2
                    || tools[0].tool_name.as_deref() != Some("Bash")
                    || tools[1].tool_name.as_deref() != Some("Write")
                    || !tool_finished(0, ToolStatus::Failed)
                    || !tool_finished(1, ToolStatus::Completed)
                {
                    violations.push(
                        "recorded deny then allow did not preserve both tool lifecycles".into(),
                    );
                }
            }
            "recorded-interrupt" | "recorded-after-interrupt" if text != "1" => {
                violations
                    .push("recorded interrupt partial text or next-turn memory changed".into());
            }
            "recorded-full-auto" => {
                if events
                    .iter()
                    .any(|event| matches!(event, DriverEvent::Permission { .. }))
                    || tools.len() != 1
                    || tools[0].tool_name.as_deref() != Some("Write")
                    || !tool_finished(0, ToolStatus::Completed)
                {
                    violations.push("native bypassPermissions no longer completed its recorded Write without a callback".into());
                }
            }
            "recorded-resume"
                if text != "aura-perm.txt\naura-auto.txt"
                    || !fixture
                        .start()
                        .is_some_and(|start| start["resume"].is_string()) =>
            {
                violations
                    .push("recorded resume did not preserve the saved file-name response".into());
            }
            "recorded-thinking" => {
                let thoughts: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        DriverEvent::Event {
                            event: O7Event::ThoughtDelta { thought_id, text },
                            ..
                        } => Some((thought_id, text)),
                        _ => None,
                    })
                    .collect();
                let completion = thoughts.first().is_some_and(|(id, _)| events.iter().any(|event|
                    matches!(event, DriverEvent::Event { event: O7Event::ThoughtCompleted { thought_id, .. }, .. } if thought_id == *id)));
                if thoughts.len() != 1
                    || thoughts.iter().any(|(_, text)| !text.is_empty())
                    || !completion
                    || !endings.iter().any(|(_, usage)| {
                        usage
                            .as_ref()
                            .is_some_and(|usage| usage.reasoning_tokens == Some(302))
                    })
                {
                    violations.push("withheld reasoning must retain one empty completed thought and the recorded 302 reasoning tokens".into());
                }
            }
            _ => {}
        }
        violations
    }

    #[tokio::test(start_paused = true)]
    async fn every_fixture_replays_to_its_golden_and_keeps_the_contract() {
        let paths = fixtures();
        assert!(
            paths.len() >= 32,
            "the synthetic fixtures are there: {}",
            paths.len()
        );
        let mut failures = Vec::new();
        for path in &paths {
            let fixture = Fixture::load(path).expect("the fixture parses");
            let report = run(&fixture).await;
            let violations = contract_violations(&fixture, &report);
            if !violations.is_empty() {
                failures.push(format!("{}: {}", fixture.name, violations.join("; ")));
            }
            let golden = golden_path(path);
            if let Err(panic) = std::panic::catch_unwind(|| assert_golden(&report, &golden)) {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                    .unwrap_or_default();
                failures.push(message);
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    }

    #[test]
    fn committed_fixtures_and_goldens_hold_no_personal_data() {
        for path in fixtures() {
            for file in [path.clone(), golden_path(&path)] {
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let findings = scan_clean(&text);
                assert!(
                    findings.is_empty(),
                    "{}: {}",
                    file.display(),
                    findings.join("; ")
                );
            }
        }
    }

    #[test]
    fn every_spec_scenario_has_a_fixture() {
        let native_names: BTreeSet<String> = fixtures()
            .iter()
            .filter(|path| {
                path.parent()
                    .is_some_and(|parent| parent.ends_with("claude"))
            })
            .filter_map(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .collect();
        for scenario in [
            "basic",
            "result-before-idle",
            "permission-withdrawal",
            "interrupt",
            "bad-late-controls",
            "telemetry-merge",
            "telemetry-coalesced",
            "recorded-basic",
            "recorded-permission",
            "recorded-interrupt",
            "recorded-after-interrupt",
            "recorded-full-auto",
            "recorded-resume",
            "recorded-thinking",
        ] {
            assert!(
                native_names.contains(scenario),
                "no native Claude fixture for {scenario}"
            );
        }
        let names: BTreeSet<String> = fixtures()
            .iter()
            .filter_map(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .collect();
        for scenario in [
            "new-session",
            "new-session-early-update",
            "legacy-models-modes",
            "load-replay",
            "open-replay-long",
            "resume-fallback",
            "resume-fallback-method",
            "resume-error-keeps-id",
            "hermes-continued-after-quit",
            "hermes-replay-matches",
            "auth-required",
            "protocol-mismatch",
            "turn-basic",
            "unknown-update",
            "prompt-auth-error",
            "prompt-error",
            "permission-ask",
            "permission-full-auto",
            "permission-withdrawn",
            "permission-outside-turn",
            "permission-during-setup",
            "question-kind-other",
            "updates-after-reply",
            "hand-picked-mode",
            "hand-picked-mode-full-auto",
            "cancel",
            "agent-requests",
            "garbage",
            "oversized",
            "eof-mid-turn",
            "exit-with-buffered-output",
            "mode-idle-only",
            "claude-no-bypass",
            "hermes-usage-session",
        ] {
            assert!(names.contains(scenario), "no fixture for {scenario}");
        }
    }
}
