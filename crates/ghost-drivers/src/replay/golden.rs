//! Goldens: the collected driver events and replies compared with `<fixture>.events.json`;
//! `GHOST_UPDATE_GOLDEN=1` rewrites them.
//!
//! A golden is pretty-printed JSON, `{"events": [...], "replies": [...], "exit": ...}`, so a change
//! in what the driver emits shows as a reviewable diff.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::ReplayReport;

/// Set to `1` to rewrite goldens instead of comparing.
pub const UPDATE_ENV: &str = "GHOST_UPDATE_GOLDEN";

/// The golden beside a fixture: `<scenario>.events.json`.
pub fn golden_path(fixture: &Path) -> PathBuf {
    fixture.with_extension("events.json")
}

/// The golden's text for a report.
pub fn golden_text(report: &ReplayReport) -> String {
    let golden = json!({
        "events": report.events,
        "replies": report.replies,
        "exit": report.exit,
    });
    let mut text = serde_json::to_string_pretty(&golden).unwrap_or_default();
    text.push('\n');
    text
}

/// Panics when the replay found mismatches, or when the report differs from the golden; with
/// `GHOST_UPDATE_GOLDEN=1` it writes the golden instead (after checking the mismatches).
pub fn assert_golden(report: &ReplayReport, golden: &Path) {
    assert!(
        report.mismatches.is_empty(),
        "the replay for {} did not match its fixture:\n  {}",
        golden.display(),
        report.mismatches.join("\n  ")
    );
    let text = golden_text(report);
    if std::env::var(UPDATE_ENV).is_ok_and(|value| value == "1") {
        if let Err(error) = std::fs::write(golden, &text) {
            panic!("could not write {}: {error}", golden.display());
        }
        return;
    }
    let stored = match std::fs::read_to_string(golden) {
        Ok(stored) => stored,
        Err(error) => panic!(
            "{} could not be read ({error}); run with {UPDATE_ENV}=1 to create it",
            golden.display()
        ),
    };
    let (Ok(stored), Ok(current)) = (
        serde_json::from_str::<Value>(&stored),
        serde_json::from_str::<Value>(&text),
    ) else {
        panic!("{} is not JSON", golden.display());
    };
    if stored != current {
        panic!(
            "{} differs from the replay: {}\nRun with {UPDATE_ENV}=1 to accept the new output, then review the diff.",
            golden.display(),
            first_difference(&stored, &current)
        );
    }
}

/// Where two goldens first differ, for the failure message (and `ghost-cli replay`'s).
pub fn first_difference(stored: &Value, current: &Value) -> String {
    for key in ["events", "replies"] {
        let empty = Vec::new();
        let old = stored.get(key).and_then(Value::as_array).unwrap_or(&empty);
        let new = current.get(key).and_then(Value::as_array).unwrap_or(&empty);
        for index in 0..old.len().max(new.len()) {
            match (old.get(index), new.get(index)) {
                (Some(a), Some(b)) if a == b => {}
                (a, b) => {
                    return format!(
                        "{key}[{index}]: the golden has {}, the replay has {}",
                        a.map_or_else(|| "nothing".into(), Value::to_string),
                        b.map_or_else(|| "nothing".into(), Value::to_string)
                    );
                }
            }
        }
    }
    format!(
        "exit: the golden has {}, the replay has {}",
        stored
            .get("exit")
            .map_or_else(|| "nothing".into(), Value::to_string),
        current
            .get("exit")
            .map_or_else(|| "nothing".into(), Value::to_string)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_of(events: Vec<Value>) -> ReplayReport {
        ReplayReport {
            events,
            replies: Vec::new(),
            exit: json!({"type": "closed"}),
            mismatches: Vec::new(),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aura-golden-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join(name)
    }

    #[test]
    fn a_matching_golden_passes_and_a_different_one_names_the_first_difference() {
        let path = scratch("x.events.json");
        let report = report_of(vec![json!({"type": "status", "data": "ready"})]);
        std::fs::write(&path, golden_text(&report)).expect("write");
        assert_golden(&report, &path);
        let changed = report_of(vec![json!({"type": "status", "data": "closing"})]);
        let panic =
            std::panic::catch_unwind(|| assert_golden(&changed, &path)).expect_err("differs");
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.contains("events[0]"), "{message}");
        assert_eq!(
            golden_path(Path::new("/f/turn-basic.jsonl")),
            Path::new("/f/turn-basic.events.json")
        );
    }

    #[test]
    fn mismatches_fail_before_any_comparison() {
        let mut report = report_of(Vec::new());
        report
            .mismatches
            .push("record 3: expected a request".into());
        let path = scratch("y.events.json");
        let panic = std::panic::catch_unwind(|| assert_golden(&report, &path)).expect_err("fails");
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.contains("record 3"), "{message}");
    }

    #[test]
    fn differences_in_replies_and_the_exit_are_named() {
        let mut old = report_of(Vec::new());
        old.replies.push(json!({"ok": null}));
        let mut new = report_of(Vec::new());
        new.replies.push(json!({"err": "x"}));
        let to_value = |report: &ReplayReport| {
            serde_json::from_str::<Value>(&golden_text(report)).expect("json")
        };
        assert!(first_difference(&to_value(&old), &to_value(&new)).starts_with("replies[0]"));
        new.replies = old.replies.clone();
        new.exit = json!({"type": "crashed"});
        assert!(first_difference(&to_value(&old), &to_value(&new)).starts_with("exit"));
    }
}
