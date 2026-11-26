//! `ghost-cli replay` over package B's synthetic fixtures: every one replays to its committed
//! golden, `--update` writes exactly that golden, and a golden or a fixture that no longer
//! matches fails with what differs.

#[cfg(test)]
mod common;

#[cfg(test)]
mod replaying {
    use std::path::Path;

    use ghost_cli::args::ReplayArgs;
    use ghost_cli::out::Out;
    use ghost_cli::replay;
    use ghost_cli::report::Outcome;
    use ghost_drivers::replay::golden_path;
    use serde_json::Value;

    use crate::common::{assert_no_failures, fixture_root, fixtures};

    async fn run(fixture: &Path, update: bool) -> (Outcome, String) {
        let (out, captured) = Out::capture();
        let outcome = replay::run(
            &ReplayArgs {
                fixture: fixture.to_path_buf(),
                update,
            },
            &out,
        )
        .await
        .expect("the replay runs");
        (outcome, captured.text())
    }

    #[tokio::test(start_paused = true)]
    async fn every_synthetic_fixture_replays_to_its_golden() {
        let paths = fixtures();
        assert!(paths.len() >= 32, "package B's fixtures are there");
        for path in paths {
            let (outcome, output) = run(&path, false).await;
            assert_eq!(outcome, Outcome::Passed, "{}:\n{output}", path.display());
            assert_no_failures(&output);
            assert!(output.contains("PASS  golden: matches"), "{output}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn update_writes_the_golden_the_fixture_already_has() {
        let dir = tempfile::tempdir().expect("tempdir");
        for path in fixtures() {
            let copy = dir.path().join(path.file_name().expect("a name"));
            std::fs::copy(&path, &copy).expect("copy the fixture");
            let (outcome, output) = run(&copy, true).await;
            assert_eq!(outcome, Outcome::Passed, "{output}");
            assert!(output.contains("PASS  golden: wrote"), "{output}");
            let written: Value = serde_json::from_str(
                &std::fs::read_to_string(golden_path(&copy)).expect("written"),
            )
            .expect("json");
            let committed: Value = serde_json::from_str(
                &std::fs::read_to_string(golden_path(&path)).expect("committed"),
            )
            .expect("json");
            assert_eq!(written, committed, "{}", path.display());
            let (outcome, output) = run(&copy, true).await;
            assert_eq!(outcome, Outcome::Passed);
            assert!(output.contains("is unchanged"), "{output}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_changed_golden_a_missing_one_or_a_changed_fixture_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = fixture_root().join("acp-agent/turn-basic.jsonl");
        let fixture = dir.path().join("turn-basic.jsonl");
        std::fs::copy(&source, &fixture).expect("copy");

        let (outcome, output) = run(&fixture, false).await;
        assert_eq!(outcome, Outcome::Failed);
        assert!(
            output.contains("doesn't exist; run ghost-cli replay") && output.contains("--update"),
            "{output}"
        );

        let golden = std::fs::read_to_string(golden_path(&source)).expect("golden");
        std::fs::write(
            golden_path(&fixture),
            golden.replace("I'll fix ", "I will fix "),
        )
        .expect("a changed golden");
        let (outcome, output) = run(&fixture, false).await;
        assert_eq!(outcome, Outcome::Failed);
        assert!(
            output.contains("differs from the replay: events["),
            "{output}"
        );

        std::fs::write(golden_path(&fixture), "not json").expect("a broken golden");
        let (outcome, output) = run(&fixture, false).await;
        assert_eq!(outcome, Outcome::Failed);
        assert!(output.contains("is not JSON"), "{output}");

        // The driver no longer writes what the fixture says: no golden is written, even with
        // --update.
        let text = std::fs::read_to_string(&source).expect("fixture");
        std::fs::write(
            &fixture,
            text.replace("\"Fix the build\"}]}}}", "\"Fix the tests\"}]}}}"),
        )
        .expect("a changed fixture");
        std::fs::remove_file(golden_path(&fixture)).expect("remove the golden");
        let (outcome, output) = run(&fixture, true).await;
        assert_eq!(outcome, Outcome::Failed, "{output}");
        assert!(output.contains("FAIL  fixture: record"), "{output}");
        assert!(output.contains("The golden was not written"), "{output}");
        assert!(!golden_path(&fixture).exists());

        let unreadable = replay::run(
            &ReplayArgs {
                fixture: dir.path().join("missing.jsonl"),
                update: false,
            },
            &Out::capture().0,
        )
        .await
        .expect_err("a missing fixture");
        assert!(
            unreadable.to_string().starts_with("could not read"),
            "{unreadable}"
        );
    }
}
