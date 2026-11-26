//! `ghost-cli replay <fixture> [--update]`: the real ACP driver against a fake agent playing the
//! fixture (spec 5.8), then the collected events and replies against `<fixture>.events.json`.
//!
//! Run it on a current-thread runtime with paused time (the binary builds one), so a fixture that
//! waits minutes replays in no time. With `--update` the golden is written instead of compared,
//! but only when the driver wrote exactly what the fixture recorded.

use std::path::Path;

use ghost_drivers::replay::golden::{first_difference, golden_text};
use ghost_drivers::replay::{Fixture, ReplayReport, golden_path};
use serde_json::Value;

use crate::CliError;
use crate::args::ReplayArgs;
use crate::out::Out;
use crate::report::{Outcome, Report};

pub async fn run(args: &ReplayArgs, out: &Out) -> Result<Outcome, CliError> {
    let fixture = Fixture::load(&args.fixture)
        .map_err(|error| CliError::io(format!("read {}", args.fixture.display()), error))?;
    let replayed = ghost_drivers::replay::run(&fixture).await;
    let mut report = Report::new(out);
    let golden = if args.update {
        Golden::Write
    } else {
        Golden::Compare
    };
    check(&mut report, &args.fixture, &replayed, golden)?;
    report.note(report.summary());
    Ok(report.outcome())
}

/// What a replay does with the golden beside its fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Golden {
    /// Compare; a missing golden fails.
    Compare,
    /// Write it (when the driver matched the fixture).
    Write,
    /// Compare when there is one (`fixture`, which may replace a fixture that has a golden).
    CompareIfPresent,
}

/// The checks of one replay: the driver against the fixture, then the result against the golden.
/// Shared with `fixture`, which replays what it curated.
pub(crate) fn check(
    report: &mut Report,
    fixture: &Path,
    replayed: &ReplayReport,
    golden_mode: Golden,
) -> Result<(), CliError> {
    report.info(
        "replay",
        format!(
            "{} events, {} replies, exit {}",
            replayed.events.len(),
            replayed.replies.len(),
            exit_kind(&replayed.exit)
        ),
    );
    if !replayed.mismatches.is_empty() {
        for mismatch in &replayed.mismatches {
            report.fail("fixture", mismatch);
        }
        if golden_mode == Golden::Write {
            report
                .note("The golden was not written: the driver's output differs from the fixture.");
        }
        return Ok(());
    }
    report.pass("fixture", "the driver wrote what the fixture recorded");
    let golden = golden_path(fixture);
    let text = golden_text(replayed);
    let stored = match std::fs::read_to_string(&golden) {
        Ok(stored) => Some(stored),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CliError::io(format!("read {}", golden.display()), error));
        }
    };
    if golden_mode == Golden::Write {
        if stored.as_deref() == Some(text.as_str()) {
            report.pass("golden", format!("{} is unchanged", golden.display()));
        } else {
            std::fs::write(&golden, &text)
                .map_err(|error| CliError::io(format!("write {}", golden.display()), error))?;
            report.pass("golden", format!("wrote {}", golden.display()));
        }
        return Ok(());
    }
    let Some(stored) = stored else {
        if golden_mode == Golden::CompareIfPresent {
            report.info(
                "golden",
                format!(
                    "no golden yet: ghost-cli replay {} --update writes {}",
                    fixture.display(),
                    golden.display()
                ),
            );
            return Ok(());
        }
        report.fail(
            "golden",
            format!(
                "{} doesn't exist; run ghost-cli replay {} --update to write it, then review it",
                golden.display(),
                fixture.display()
            ),
        );
        return Ok(());
    };
    let current: Value = serde_json::from_str(&text)
        .map_err(|error| CliError::Message(format!("the replay's golden is not JSON: {error}")))?;
    match serde_json::from_str::<Value>(&stored) {
        Err(error) => report.fail("golden", format!("{} is not JSON: {error}", golden.display())),
        Ok(stored) if stored == current => {
            report.pass("golden", format!("matches {}", golden.display()));
        }
        Ok(stored) => report.fail(
            "golden",
            format!(
                "{} differs from the replay: {}\nRun ghost-cli replay {} --update to accept the new output, then review the diff.",
                golden.display(),
                first_difference(&stored, &current),
                fixture.display()
            ),
        ),
    }
    Ok(())
}

/// `closed`, `crashed`, `start_failed`, ... from a replay's exit.
fn exit_kind(exit: &Value) -> String {
    exit.get("type")
        .and_then(Value::as_str)
        .map_or_else(|| exit.to_string(), str::to_owned)
}
