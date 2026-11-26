//! The ghost-cli binary itself: its exit statuses, its usage errors, smoke's refusal without
//! `--spend` before anything starts, and the two commands that need no agent (`replay`,
//! `fixture`) run from the command line.

#[cfg(test)]
mod common;

#[cfg(test)]
mod binary {
    use std::path::Path;
    use std::process::{Command, Output};

    use crate::common::fixture_root;

    fn cli(args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ghost-cli"));
        command.args(args).env_remove("RUST_LOG");
        configure(&mut command);
        command.output().expect("run ghost-cli")
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn usage_errors_exit_2_and_say_what_to_change() {
        for (args, needle) in [
            (&[][..], "no command given"),
            (&["launch"], "unknown command \"launch\""),
            (&["probe"], "probe needs an agent id"),
            (&["probe", "mock", "--bogus"], "unknown option --bogus"),
            (
                &["replay", "a.jsonl", "--data-dir", "/d"],
                "--data-dir doesn't apply to replay",
            ),
        ] {
            let output = cli(args, |_| {});
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            let stderr = text(&output.stderr);
            assert!(stderr.contains(needle), "{args:?}: {stderr}");
            assert!(stderr.contains("Run ghost-cli help"), "{stderr}");
            assert!(output.stdout.is_empty(), "{args:?}");
        }
    }

    #[test]
    fn help_prints_the_usage_and_exits_0() {
        let output = cli(&["help"], |_| {});
        assert_eq!(output.status.code(), Some(0));
        let stdout = text(&output.stdout);
        assert!(
            stdout.contains("Usage:") && stdout.contains("ghost-cli replay <fixture>"),
            "{stdout}"
        );
        assert_eq!(text(&cli(&["--help"], |_| {}).stdout), stdout);
    }

    /// Without `--spend`, smoke stops at the command line: no run folder, no data root, no
    /// agent.
    #[test]
    fn smoke_without_spend_is_refused_before_anything_starts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let temp = dir.path().join("tmp");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&temp).expect("temp folder");
        for args in [
            &["smoke", "claude-acp", "--scenario", "basic"][..],
            &["smoke", "mock", "--scenario", "all", "--model", "mock-fast"],
        ] {
            let output = cli(args, |command| {
                command
                    .env("TMPDIR", &temp)
                    .env("TMP", &temp)
                    .env("TEMP", &temp)
                    .env("GHOST_DATA_DIR", &data);
            });
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            let stderr = text(&output.stderr);
            assert!(
                stderr.contains("smoke sends real prompts")
                    && stderr.contains("Add --spend to run it."),
                "{stderr}"
            );
            assert!(
                !stderr.contains("Run ghost-cli help"),
                "the refusal is not a typo: {stderr}"
            );
        }
        assert_eq!(
            std::fs::read_dir(&temp).expect("temp").count(),
            0,
            "no run folder was made"
        );
        assert!(!data.exists(), "the data root was not touched");
    }

    #[test]
    fn replay_from_the_command_line_passes_then_fails_on_a_changed_golden() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = fixture_root().join("hermes/legacy-models-modes.jsonl");
        let fixture = dir.path().join("legacy-models-modes.jsonl");
        std::fs::copy(&source, &fixture).expect("copy the fixture");
        let golden = ghost_drivers::replay::golden_path(&fixture);
        std::fs::copy(ghost_drivers::replay::golden_path(&source), &golden)
            .expect("copy the golden");
        let path = fixture.to_str().expect("a UTF-8 path");

        let output = cli(&["replay", path], |_| {});
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stdout));
        assert!(text(&output.stdout).contains("PASS  golden: matches"));

        std::fs::write(&golden, "{\"events\":[],\"replies\":[],\"exit\":null}").expect("change");
        let output = cli(&["replay", path], |_| {});
        assert_eq!(output.status.code(), Some(1));
        assert!(
            text(&output.stdout).contains("FAIL  golden:"),
            "{}",
            text(&output.stdout)
        );

        let output = cli(&["replay", path, "--update"], |_| {});
        assert_eq!(output.status.code(), Some(0));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&golden).expect("golden")).expect("json");
        let committed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ghost_drivers::replay::golden_path(&source)).expect("golden"),
        )
        .expect("json");
        assert_eq!(written, committed);

        let output = cli(&["replay", &format!("{path}.missing")], |_| {});
        assert_eq!(output.status.code(), Some(1));
        assert!(
            text(&output.stderr).starts_with("ghost-cli: could not read"),
            "{}",
            text(&output.stderr)
        );
    }

    #[test]
    fn fixture_from_the_command_line_curates_and_replays() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A log as the agent wrote it: package B's fixture with a real folder and session id.
        let source = std::fs::read_to_string(fixture_root().join("acp-agent/new-session.jsonl"))
            .expect("the fixture");
        let log = dir
            .path()
            .join("019a0000-0000-7000-8000-000000000042.jsonl");
        let mut text_log = String::new();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("record"))
            .filter(|record| record.pointer("/note/text").is_none());
        for (index, mut record) in records.enumerate() {
            record["t"] = serde_json::json!(1_792_840_000_000_i64 + index as i64);
            text_log.push_str(
                &record
                    .to_string()
                    .replace("<CWD>", "/srv/work/scratch-repo")
                    .replace("<SESSION_1>", "0c1d2e3f-session"),
            );
            text_log.push('\n');
        }
        std::fs::write(&log, text_log).expect("the log");
        let out = dir.path().join("probe-session.jsonl");
        let output = cli(
            &[
                "fixture",
                log.to_str().expect("UTF-8"),
                "--scenario",
                "probe-session",
                "--out",
                out.to_str().expect("UTF-8"),
            ],
            |_| {},
        );
        let stdout = text(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{stdout}{}",
            text(&output.stderr)
        );
        assert!(
            stdout.contains("PASS  fixture: the driver wrote what the fixture recorded"),
            "{stdout}"
        );
        let curated = std::fs::read_to_string(&out).expect("curated");
        assert!(
            curated.contains("<CWD>") && curated.contains("<SESSION_1>"),
            "{curated}"
        );
        assert!(!curated.contains("/srv/work/scratch-repo"));
        assert!(Path::new(&out).is_file());
    }
}
