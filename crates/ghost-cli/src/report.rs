//! Check results as the commands print them, one line each:
//!
//! ```text
//! PASS  1 initialize: the agent answered in 412 ms: hermes-agent 0.22.5, protocol version 1
//! FAIL  6 close: 1 process is still in the agent's process group: 5120 node ...
//! INFO  2 session: 3 options: mode (mode, 3 choices), model (model, 2 choices), ...
//! SKIP  0 install: Hermes is not an npm adapter
//! ```
//!
//! A command fails when any check failed.

use std::fmt;

use crate::out::Out;

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Info,
    Skip,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Info => "INFO",
            Verdict::Skip => "SKIP",
        }
    }
}

/// How a whole command came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// A signal stopped the command before it finished.
    Interrupted,
}

/// One check: what it says and how it came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub verdict: Verdict,
    pub step: String,
    pub text: String,
}

impl fmt::Display for Check {
    /// Continuation lines are indented under the text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut lines = self.text.lines();
        write!(
            f,
            "{}  {}: {}",
            self.verdict.label(),
            self.step,
            lines.next().unwrap_or_default()
        )?;
        for line in lines {
            write!(f, "\n      {line}")?;
        }
        Ok(())
    }
}

/// The checks of one command, printed as they are made.
#[derive(Debug)]
pub struct Report {
    out: Out,
    checks: Vec<Check>,
}

impl Report {
    pub fn new(out: &Out) -> Report {
        Report {
            out: out.clone(),
            checks: Vec::new(),
        }
    }

    pub fn add(&mut self, verdict: Verdict, step: &str, text: impl fmt::Display) {
        let check = Check {
            verdict,
            step: step.to_owned(),
            text: text.to_string(),
        };
        self.out.line(check.to_string());
        self.checks.push(check);
    }

    pub fn pass(&mut self, step: &str, text: impl fmt::Display) {
        self.add(Verdict::Pass, step, text);
    }

    pub fn fail(&mut self, step: &str, text: impl fmt::Display) {
        self.add(Verdict::Fail, step, text);
    }

    pub fn info(&mut self, step: &str, text: impl fmt::Display) {
        self.add(Verdict::Info, step, text);
    }

    pub fn skip(&mut self, step: &str, text: impl fmt::Display) {
        self.add(Verdict::Skip, step, text);
    }

    /// `pass` when `ok`, else `fail`, with the same text.
    pub fn check(&mut self, ok: bool, step: &str, text: impl fmt::Display) {
        self.add(if ok { Verdict::Pass } else { Verdict::Fail }, step, text);
    }

    /// A plain line, not a check.
    pub fn note(&self, text: impl AsRef<str>) {
        self.out.line(text);
    }

    pub fn checks(&self) -> &[Check] {
        &self.checks
    }

    pub fn failures(&self) -> usize {
        self.count(Verdict::Fail)
    }

    fn count(&self, verdict: Verdict) -> usize {
        self.checks
            .iter()
            .filter(|check| check.verdict == verdict)
            .count()
    }

    /// "12 passed, 1 failed, 2 skipped".
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("{} passed", self.count(Verdict::Pass)),
            format!("{} failed", self.failures()),
        ];
        let skipped = self.count(Verdict::Skip);
        if skipped > 0 {
            parts.push(format!("{skipped} skipped"));
        }
        parts.join(", ")
    }

    pub fn outcome(&self) -> Outcome {
        if self.failures() == 0 {
            Outcome::Passed
        } else {
            Outcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_print_as_they_come_and_any_failure_fails() {
        let (out, captured) = Out::capture();
        let mut report = Report::new(&out);
        report.pass("1 initialize", "answered");
        report.info("2 session", "two lines\nsecond line");
        assert_eq!(report.outcome(), Outcome::Passed);
        report.check(false, "3 commands", "none");
        report.skip("0 install", "built in");
        assert_eq!(report.outcome(), Outcome::Failed);
        assert_eq!(report.summary(), "1 passed, 1 failed, 1 skipped");
        assert_eq!(
            captured.text(),
            "PASS  1 initialize: answered\nINFO  2 session: two lines\n      second line\nFAIL  3 commands: none\nSKIP  0 install: built in\n"
        );
    }
}
