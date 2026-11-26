//! `ghost-cli fixture` over package B's synthetic fixtures: each is turned back into the protocol
//! log an agent would have left (real paths and session ids, Unix times, a spawn note), curated,
//! and must come back as the same fixture, replaying to the committed golden. Then every way a log
//! can be unfit: a capped record, dropped records, a missing start, personal data the scan finds,
//! and a rotated part passed instead of the log.

#[cfg(test)]
mod common;

#[cfg(test)]
mod curation {
    use std::path::{Path, PathBuf};

    use ghost_cli::args::FixtureArgs;
    use ghost_cli::fixture::{self, CurateContext};
    use ghost_cli::out::Out;
    use ghost_cli::report::Outcome;
    use serde_json::{Value, json};

    use crate::common::{assert_no_failures, assert_passed, fixtures};

    /// The chat folder and home the made-up logs name.
    const CWD: &str = "/srv/work/repo-a";
    const HOME: &str = "/srv/home/tester";

    fn context() -> CurateContext {
        CurateContext {
            home: HOME.into(),
            user: "tester".into(),
            host: "tester-box".into(),
        }
    }

    /// The real session id each placeholder stands for.
    fn session_id(placeholder: &str) -> String {
        format!(
            "4f7c-{}-session",
            placeholder
                .trim_start_matches("<SESSION_")
                .trim_end_matches('>')
                .to_lowercase()
        )
    }

    /// A fixture as an agent's protocol log would hold it.
    fn as_log(fixture: &str) -> String {
        let mut t = 1_792_840_001_000_i64;
        let mut log = format!(
            "{}\n",
            json!({"t": t, "d": "note", "note": {"spawn": {"label": "agent acp",
                "program": format!("{HOME}/.local/bin/agent"), "args": ["acp"], "cwd": CWD,
                "removed": ["CLAUDECODE"], "set": [], "pid": 4242}}})
        );
        for line in fixture.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: Value = serde_json::from_str(line).expect("a fixture record");
            if record.pointer("/note/text").is_some() {
                continue;
            }
            t += 7;
            record["t"] = json!(t);
            let mut text = record.to_string().replace("<CWD>", CWD);
            while let Some(start) = text.find("<SESSION_") {
                let end = start + text[start..].find('>').expect("a closed placeholder") + 1;
                let placeholder = text[start..end].to_owned();
                text = text.replace(&placeholder, &session_id(&placeholder));
            }
            log.push_str(&text);
            log.push('\n');
        }
        log
    }

    /// A fixture's records as the log writer redacts them (a hand-written fixture can hold a value
    /// under a credential key that a real log never would), without comments, spawn notes and
    /// times, with session placeholders renamed in order of first appearance, as curation numbers
    /// sessions.
    fn normalized(text: &str) -> Vec<Value> {
        let redactor = ghost_process::Redactor::standard();
        let mut names: Vec<String> = Vec::new();
        let mut renamed = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("<SESSION_") {
            let end = start + rest[start..].find('>').expect("a closed placeholder") + 1;
            let placeholder = &rest[start..end];
            let index = match names.iter().position(|known| known == placeholder) {
                Some(index) => index,
                None => {
                    names.push(placeholder.to_owned());
                    names.len() - 1
                }
            };
            renamed.push_str(&rest[..start]);
            renamed.push_str(&format!("<S{index}>"));
            rest = &rest[end..];
        }
        renamed.push_str(rest);
        renamed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Value>(line).expect("a record"))
            .filter(|record| {
                record.pointer("/note/text").is_none() && record.pointer("/note/spawn").is_none()
            })
            .map(|mut record| {
                redactor.redact_value(&mut record);
                if let Some(object) = record.as_object_mut() {
                    object.remove("t");
                }
                record
            })
            .collect()
    }

    async fn curate(
        log: &Path,
        scenario: &str,
        out: &Path,
    ) -> (Result<Outcome, ghost_cli::CliError>, String) {
        let (writer, captured) = Out::capture();
        let result = fixture::run(
            &FixtureArgs {
                log: log.to_path_buf(),
                scenario: scenario.into(),
                out: out.to_path_buf(),
            },
            &context(),
            &writer,
        )
        .await;
        (result, captured.text())
    }

    #[tokio::test(start_paused = true)]
    async fn every_synthetic_fixture_survives_the_trip_through_a_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = fixtures();
        assert!(paths.len() >= 32, "package B's fixtures are there");
        for path in &paths {
            let name = path
                .file_stem()
                .expect("a stem")
                .to_string_lossy()
                .into_owned();
            let original = std::fs::read_to_string(path).expect("the fixture");
            let log = dir.path().join(format!("{name}.log.jsonl"));
            std::fs::write(&log, as_log(&original)).expect("the log");
            let out = dir.path().join("out").join(format!("{name}.jsonl"));
            // The committed golden beside the curated fixture: the replay must match it.
            std::fs::create_dir_all(out.parent().expect("parent")).expect("out folder");
            std::fs::copy(
                ghost_drivers::replay::golden_path(path),
                ghost_drivers::replay::golden_path(&out),
            )
            .expect("the golden");
            let (result, output) = curate(&log, &name, &out).await;
            assert_eq!(result.expect(&name), Outcome::Passed, "{name}:\n{output}");
            assert_no_failures(&output);
            assert_passed(&output, "matches");
            let curated = std::fs::read_to_string(&out).expect("the curated fixture");
            let first: Value =
                serde_json::from_str(curated.lines().next().expect("a first line")).expect("json");
            assert_eq!(
                first["note"]["text"],
                format!(
                    "Recorded scenario {name}: {}, curated by ghost-cli.",
                    agent_of(&original, &curated)
                ),
                "{name}"
            );
            assert_eq!(normalized(&curated), normalized(&original), "{name}");
            assert!(
                ghost_drivers::replay::scan_clean(&curated).is_empty(),
                "{name}: {curated}"
            );
        }
    }

    /// `agent_id (name version)`, as the note names the agent that played the fixture.
    fn agent_of(original: &str, curated: &str) -> String {
        let records = ghost_cli::logs::parse(original).records;
        let agent = ghost_cli::logs::note(&records, "start")
            .and_then(|start| start.get("agent_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let curated = ghost_cli::logs::parse(curated).records;
        let info = ghost_cli::logs::exchange(&curated, "initialize").and_then(|(_, answer)| {
            ghost_cli::logs::message(answer)?
                .pointer("/result/agentInfo")
                .cloned()
        });
        match info {
            Some(info) => {
                let name = info
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&agent)
                    .to_owned();
                match info.get("version").and_then(Value::as_str) {
                    Some(version) => format!("{agent} ({name} {version})"),
                    None => format!("{agent} ({name})"),
                }
            }
            None => agent,
        }
    }

    fn turn_basic_log(dir: &Path) -> PathBuf {
        let original = std::fs::read_to_string(
            crate::common::fixture_root().join("acp-agent/turn-basic.jsonl"),
        )
        .expect("turn-basic");
        let log = dir.join("019a0000-0000-7000-8000-0000000000a1.jsonl");
        std::fs::write(&log, as_log(&original)).expect("the log");
        log
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_fixture_without_a_golden_says_how_to_write_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = turn_basic_log(dir.path());
        let out = dir
            .path()
            .join("fixtures")
            .join("hermes")
            .join("basic.jsonl");
        let (result, output) = curate(&log, "basic", &out).await;
        assert_eq!(result.expect("curated"), Outcome::Passed, "{output}");
        assert!(out.is_file(), "the parent folders are made");
        assert!(
            output.contains("PASS  fixture: the driver wrote what the fixture recorded"),
            "{output}"
        );
        assert!(
            output.contains("INFO  golden: no golden yet: ghost-cli replay"),
            "{output}"
        );
        for item in [
            "Review before committing:",
            "Placeholders: <HOME> x1, <CWD> x",
            "1 native session id(s) as <SESSION_n>",
            "The session: agent acp-agent, dialect generic, resume null, autonomy full_auto; commands prompt x1.",
            "Write the golden with ghost-cli replay",
        ] {
            assert!(output.contains(item), "{item:?} missing:\n{output}");
        }
        // Curating again replaces the fixture and says so.
        let (result, output) = curate(&log, "basic", &out).await;
        assert_eq!(result.expect("curated again"), Outcome::Passed);
        assert!(output.contains("PASS  curate: replaced"), "{output}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rotated_log_is_read_with_its_older_parts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = turn_basic_log(dir.path());
        let whole = std::fs::read_to_string(&log).expect("log");
        let lines: Vec<&str> = whole.lines().collect();
        let (older, newer) = lines.split_at(lines.len() / 2);
        let part = dir
            .path()
            .join("019a0000-0000-7000-8000-0000000000a1.1.jsonl");
        std::fs::write(&part, older.join("\n") + "\n").expect("the older part");
        std::fs::write(&log, newer.join("\n") + "\n").expect("the current part");
        let out = dir.path().join("rotated.jsonl");
        let (result, output) = curate(&log, "rotated", &out).await;
        assert_eq!(result.expect("curated"), Outcome::Passed, "{output}");
        assert!(
            output.contains(".1.jsonl + "),
            "both parts are named: {output}"
        );
        let refused = curate(&part, "rotated", &out)
            .await
            .0
            .expect_err("a part alone");
        assert!(
            refused.to_string().contains("older part of the log"),
            "{refused}"
        );
        // A fixture never replaces the recording it comes from, whichever part --out names.
        for target in [&log, &part] {
            let before = std::fs::read_to_string(target).expect("the part");
            let refused = curate(&log, "rotated", target)
                .await
                .0
                .expect_err("--out names the log");
            assert!(
                refused.to_string().contains("names the log itself"),
                "{refused}"
            );
            assert_eq!(std::fs::read_to_string(target).expect("the part"), before);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn logs_that_cannot_become_fixtures_are_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = turn_basic_log(dir.path());
        let good = std::fs::read_to_string(&log).expect("log");
        let out = dir.path().join("never.jsonl");
        let cases = [
            (
                "capped",
                format!(
                    "{good}{}\n",
                    json!({"t": 9, "d": "in", "raw": "{\"jsonrpc\":\"2.0\",\"method\":\"session/upd", "truncated": 131_072})
                ),
                "capped",
            ),
            (
                "dropped",
                format!(
                    "{good}{}\n",
                    json!({"t": 9, "d": "note", "note": {"dropped": 12}})
                ),
                "dropped 12 record(s)",
            ),
            (
                "no start",
                good.lines()
                    .filter(|line| !line.contains("\"start\""))
                    .map(|line| format!("{line}\n"))
                    .collect(),
                "no start note",
            ),
            (
                "personal",
                format!(
                    "{good}{}\n",
                    json!({"t": 9, "d": "err", "line": "reading /home/someone-else/.config/agent.toml"})
                ),
                "still holds personal data",
            ),
            (
                "not a log",
                format!("{good}this is not JSON\n"),
                "not protocol log records",
            ),
        ];
        for (case, text, needle) in cases {
            std::fs::write(&log, text).expect("write the log");
            let error = curate(&log, "never", &out).await.0.expect_err(case);
            assert!(error.to_string().contains(needle), "{case}: {error}");
            assert!(!out.exists(), "{case}: nothing is written");
        }
        let missing = curate(&dir.path().join("missing.jsonl"), "never", &out)
            .await
            .0
            .expect_err("a missing log");
        assert!(
            missing.to_string().starts_with("could not read"),
            "{missing}"
        );
    }
}
