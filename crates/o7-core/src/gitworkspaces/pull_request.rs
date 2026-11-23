//! The pull request open from a folder's branch on GitHub, read with GitHub CLI (`gh pr view`)
//! as the person is signed in: its number, address and state, how its checks stand, and its
//! reviews and comments. A bot link that started a task is told when these change
//! (`o7_remote`), and a reading of the chat names it. Nothing here writes to GitHub.

use super::*;
use o7_process::{EnvKey, HelperEnv, HelperSpec, find_exe, run_helper};
use std::{ffi::OsString, time::Duration};

/// How a pull request's checks stand, all of them together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checks {
    /// It has none.
    None,
    /// Some have not finished, and none has failed.
    Pending,
    /// All finished, and none failed.
    Passed,
    /// The names of those that failed (or errored, timed out, were cancelled).
    Failed(Vec<String>),
}

/// A review or a comment on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub author: String,
    /// `comment`, or a review's state in lower case (`approved`, `changes requested`, ...).
    pub kind: String,
    pub body: String,
    /// When it was written, as GitHub gives it (ISO 8601, so it sorts as text).
    pub at: String,
    /// Its author owns the repository, is a member of its organization or a collaborator on it.
    /// Anyone else can comment on a public repository, so only these people's words are passed
    /// on to a bot.
    pub trusted: bool,
}

/// A pull request as GitHub CLI reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestView {
    pub number: u64,
    pub url: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub checks: Checks,
    /// Reviews and comments, oldest first.
    pub notes: Vec<Note>,
}

/// The fields asked of `gh pr view`.
const FIELDS: &str = "number,url,state,statusCheckRollup,reviews,comments";
/// The most of `gh`'s answer read: a pull request with many comments is cut, not refused.
const MAX_OUTPUT: usize = 1024 * 1024;

impl GitWorkspaces {
    /// The pull request open from `folder`'s current branch, or none: no pull request, no
    /// GitHub CLI, not signed in, or a folder GitHub does not know all read as none, since a
    /// watcher has nothing to say about any of them.
    pub async fn pull_request(&self, folder: &Path) -> Result<Option<PullRequestView>> {
        let mut env = (*self.env.get().await).clone();
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        for name in [
            "GH_REPO",
            "GH_DEBUG",
            "GH_FORCE_TTY",
            "GH_EDITOR",
            "GH_PAGER",
            "PAGER",
        ] {
            env.vars.remove(&EnvKey::from(name));
        }
        let Some(program) = find_exe("gh", &env.vars) else {
            return Ok(None);
        };
        let output = run_helper(
            &self.registry,
            HelperSpec {
                program,
                args: ["pr", "view", "--json", FIELDS]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                cwd: folder.into(),
                env: HelperEnv::Resolved(Arc::new(env)),
                set: vec![
                    ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ("NO_COLOR".into(), "1".into()),
                ],
                new_session: true,
                timeout: Duration::from_secs(30),
                max_stdout: MAX_OUTPUT,
            },
        )
        .await
        .map_err(|error| GitError::Message(error.to_string()))?;
        let exited = output.exit.as_ref().and_then(|exit| exit.code);
        if output.timed_out || output.stdout_truncated || exited != Some(0) {
            return Ok(None);
        }
        Ok(parse(&output.stdout))
    }
}

/// `gh pr view --json` as a [`PullRequestView`]; none when it is not one.
pub fn parse(json: &[u8]) -> Option<PullRequestView> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let trusted = |value: &serde_json::Value| {
        matches!(
            value
                .get("authorAssociation")
                .and_then(serde_json::Value::as_str),
            Some("OWNER" | "MEMBER" | "COLLABORATOR")
        )
    };
    let author = |value: &serde_json::Value| {
        value
            .get("author")
            .and_then(|author| author.get("login"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("someone")
            .to_owned()
    };
    let list = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut notes: Vec<Note> = list("reviews")
        .iter()
        .map(|review| Note {
            author: author(review),
            kind: text(review, "state").to_lowercase().replace('_', " "),
            body: text(review, "body"),
            at: text(review, "submittedAt"),
            trusted: trusted(review),
        })
        .chain(list("comments").iter().map(|comment| Note {
            author: author(comment),
            kind: "comment".into(),
            body: text(comment, "body"),
            at: text(comment, "createdAt"),
            trusted: trusted(comment),
        }))
        .collect();
    notes.sort_by(|a, b| a.at.cmp(&b.at));
    Some(PullRequestView {
        number: value.get("number")?.as_u64()?,
        url: text(&value, "url"),
        state: text(&value, "state").to_lowercase(),
        checks: checks(&list("statusCheckRollup")),
        notes,
    })
}

/// The checks together: failed when any did, pending while any runs, passed when all are done.
fn checks(rollup: &[serde_json::Value]) -> Checks {
    if rollup.is_empty() {
        return Checks::None;
    }
    let mut failed = Vec::new();
    let mut pending = false;
    for check in rollup {
        let field = |key: &str| {
            check
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_uppercase()
        };
        // A check run has a status and, once completed, a conclusion; a commit status has a state.
        let (name, outcome) = if check.get("__typename").and_then(serde_json::Value::as_str)
            == Some("StatusContext")
        {
            (field("context"), field("state"))
        } else if field("status") == "COMPLETED" {
            (field("name"), field("conclusion"))
        } else {
            (field("name"), "PENDING".into())
        };
        match outcome.as_str() {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => failed.push(
                check
                    .get("name")
                    .or_else(|| check.get("context"))
                    .and_then(serde_json::Value::as_str)
                    .map_or(name, str::to_owned),
            ),
            "SUCCESS" | "NEUTRAL" | "SKIPPED" | "STALE" => {}
            _ => pending = true,
        }
    }
    if !failed.is_empty() {
        Checks::Failed(failed)
    } else if pending {
        Checks::Pending
    } else {
        Checks::Passed
    }
}

#[cfg(test)]
mod pull_request_tests {
    use super::*;

    #[test]
    fn a_pull_request_reads_with_its_checks_and_notes_in_order() {
        let view = parse(
            br#"{
              "number": 12,
              "url": "https://github.com/o/r/pull/12",
              "state": "OPEN",
              "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE"},
                {"__typename": "StatusContext", "context": "deploy", "state": "PENDING"}
              ],
              "reviews": [
                {"author": {"login": "ana"}, "authorAssociation": "MEMBER", "state": "CHANGES_REQUESTED", "body": "Rename it.", "submittedAt": "2026-11-23T10:00:00Z"}
              ],
              "comments": [
                {"author": {"login": "bo"}, "body": "Looks close.", "createdAt": "2026-11-23T09:00:00Z"}
              ]
            }"#,
        )
        .expect("a pull request");
        assert_eq!(view.number, 12);
        assert_eq!(view.state, "open");
        assert_eq!(view.checks, Checks::Failed(vec!["test".into()]));
        let notes: Vec<(&str, &str)> = view
            .notes
            .iter()
            .map(|note| (note.author.as_str(), note.kind.as_str()))
            .collect();
        assert_eq!(notes, [("bo", "comment"), ("ana", "changes requested")]);
        assert!(!view.notes[0].trusted, "no association: anyone");
        assert!(view.notes[1].trusted, "a member");
    }

    #[test]
    fn checks_are_pending_until_all_finish_and_passed_when_none_failed() {
        let run = |status: &str, conclusion: &str| serde_json::json!({"__typename": "CheckRun", "name": "ci", "status": status, "conclusion": conclusion});
        assert_eq!(checks(&[]), Checks::None);
        assert_eq!(
            checks(&[run("COMPLETED", "SUCCESS"), run("IN_PROGRESS", "")]),
            Checks::Pending
        );
        assert_eq!(
            checks(&[run("COMPLETED", "SUCCESS"), run("COMPLETED", "SKIPPED")]),
            Checks::Passed
        );
        assert_eq!(
            checks(&[run("COMPLETED", "TIMED_OUT")]),
            Checks::Failed(vec!["ci".into()])
        );
        let status = serde_json::json!({"__typename": "StatusContext", "context": "netlify", "state": "ERROR"});
        assert_eq!(checks(&[status]), Checks::Failed(vec!["netlify".into()]));
    }

    #[test]
    fn what_is_not_a_pull_request_reads_as_none() {
        assert_eq!(parse(b"not json"), None);
        assert_eq!(parse(br#"{"url": "x"}"#), None);
    }
}
