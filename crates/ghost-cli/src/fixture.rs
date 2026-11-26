//! `ghost-cli fixture <log> --scenario <name> --out <file>`: a recorded protocol log into a replay
//! fixture (spec 5.8).
//!
//! The log (with its rotated parts) goes through package B's `curate`: the log's redaction again,
//! `<HOME>`, `<CWD>` (the chat folder the agent got), `<ADAPTER>` (an npm adapter's install folder),
//! `<USER>` and `<HOST>`, native session ids as `<SESSION_n>`, timestamps as `<TIME>`, times
//! relative; then the scan, which must find nothing.
//! A log that capped a record or dropped records cannot replay, so it is refused, as is one whose
//! beginning (the start note) was rotated away. The fixture gets a first note naming the
//! scenario, is written, is replayed at once, and a checklist for the review follows.

use std::fmt::Write as _;
use std::path::Path;

use ghost_drivers::replay::{CurateError, Fixture, curate};
use serde_json::{Value, json};

use crate::CliError;
use crate::args::FixtureArgs;
use crate::logs;
use crate::out::Out;
use crate::report::{Outcome, Report};

/// What curation replaces with `<HOME>`, `<USER>` and `<HOST>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurateContext {
    pub home: String,
    pub user: String,
    pub host: String,
}

impl CurateContext {
    /// This machine's: the home folder, the user's name, and the host name (without the `.local`
    /// macOS adds, which then stays next to `<HOST>`).
    pub fn here() -> CurateContext {
        let home = dirs::home_dir();
        let user = ["USER", "USERNAME"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
            .or_else(|| {
                home.as_deref()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let host = hostname::get()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let host = host
            .strip_suffix(".local")
            .map_or_else(|| host.clone(), str::to_owned);
        CurateContext {
            home: home
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_default(),
            user,
            host,
        }
    }
}

pub async fn run(
    args: &FixtureArgs,
    context: &CurateContext,
    out: &Out,
) -> Result<Outcome, CliError> {
    let parts = logs::parts(&args.log).map_err(CliError::Message)?;
    if parts.iter().any(|part| same_file(part, &args.out)) {
        return Err(CliError::Message(format!(
            "--out {} names the log itself; write the fixture somewhere else",
            args.out.display()
        )));
    }
    let mut text = String::new();
    for part in &parts {
        let part_text = std::fs::read_to_string(part)
            .map_err(|error| CliError::io(format!("read {}", part.display()), error))?;
        text.push_str(&part_text);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    let raw = logs::parse(&text);
    if raw.unreadable > 0 {
        return Err(CliError::Message(format!(
            "{} has {} line(s) that are not protocol log records, so it is not a whole protocol log",
            args.log.display(),
            raw.unreadable
        )));
    }
    let dropped: u64 = logs::notes(&raw.records, "dropped")
        .filter_map(Value::as_u64)
        .sum();
    if dropped > 0 {
        return Err(CliError::Message(format!(
            "the log dropped {dropped} record(s) while it was written (its queue was full), so a fixture made from it would not replay; record it again"
        )));
    }
    let Some(start) = logs::note(&raw.records, "start") else {
        return Err(CliError::Message(format!(
            "{} has no start note: its beginning was rotated away, or it is not an agent's protocol log",
            args.log.display()
        )));
    };
    let agent = start
        .get("agent_id")
        .and_then(Value::as_str)
        .unwrap_or("an agent")
        .to_owned();
    let cwd = logs::session_cwd(&raw.records).unwrap_or_default();
    let curated = curate(&text, &context.home, cwd, &context.user, &context.host).map_err(
        |error| match error {
            CurateError::Unclean { findings } => CliError::Message(format!(
                "the curated fixture still holds personal data, so nothing was written:\n  {}\nFind where it comes from in the log; when it is the agent's own output, record the scenario again without it.",
                findings.join("\n  ")
            )),
            other => CliError::Message(format!("{}: {other}", args.log.display())),
        },
    )?;
    let curated_records = logs::parse(&curated).records;
    let note = json!({"d": "note", "note": {"text": scenario_note(&args.scenario, &agent, &curated_records)}});
    let fixture_text = format!("{note}\n{curated}");
    let fixture = Fixture::parse(&args.scenario, &fixture_text).map_err(|error| {
        CliError::Message(format!("the curated fixture does not parse: {error}"))
    })?;
    if let Some(parent) = args.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| CliError::io(format!("create {}", parent.display()), error))?;
    }
    let replaced = args.out.exists();
    std::fs::write(&args.out, &fixture_text)
        .map_err(|error| CliError::io(format!("write {}", args.out.display()), error))?;

    let mut report = Report::new(out);
    report.pass(
        "curate",
        format!(
            "{} {} ({} records) from {}",
            if replaced { "replaced" } else { "wrote" },
            args.out.display(),
            fixture.records.len(),
            parts
                .iter()
                .map(|part| part.display().to_string())
                .collect::<Vec<_>>()
                .join(" + ")
        ),
    );
    report.pass(
        "scan",
        "no email address, token shape or home folder path is left",
    );
    let replayed = ghost_drivers::replay::run(&fixture).await;
    crate::replay::check(
        &mut report,
        &args.out,
        &replayed,
        crate::replay::Golden::CompareIfPresent,
    )?;
    report.note(checklist(&args.out, &fixture_text, &curated_records));
    report.note(report.summary());
    Ok(report.outcome())
}

/// Whether two paths name one existing file (through links and `..`).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The fixture's first line: which scenario it records and the agent that played it.
fn scenario_note(scenario: &str, agent: &str, records: &[Value]) -> String {
    let info = logs::exchange(records, "initialize")
        .and_then(|(_, answer)| logs::message(answer)?.pointer("/result/agentInfo"))
        .map(|info| {
            let name = info.get("name").and_then(Value::as_str).unwrap_or(agent);
            match info.get("version").and_then(Value::as_str) {
                Some(version) => format!("{name} {version}"),
                None => name.to_owned(),
            }
        });
    match info {
        Some(info) => {
            format!("Recorded scenario {scenario}: {agent} ({info}), curated by ghost-cli.")
        }
        None => format!("Recorded scenario {scenario}: {agent}, curated by ghost-cli."),
    }
}

/// What the reviewer reads before committing the fixture: what was replaced, what the agent
/// wrote that no scan can judge, and what to run next.
fn checklist(out: &Path, text: &str, records: &[Value]) -> String {
    let count = |needle: &str| text.matches(needle).count();
    let sessions = (1..)
        .take_while(|n| text.contains(&format!("<SESSION_{n}>")))
        .count();
    let by_direction = |direction: &str| {
        records
            .iter()
            .filter(|record| logs::direction(record) == Some(direction))
            .count()
    };
    let raw = records
        .iter()
        .filter(|record| logs::direction(record) == Some("in") && record.get("raw").is_some())
        .count();
    let mut commands: Vec<(String, usize)> = Vec::new();
    for command in logs::commands(records) {
        let kind = command
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned();
        match commands.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, n)) => *n += 1,
            None => commands.push((kind, 1)),
        }
    }
    let commands = if commands.is_empty() {
        "none".to_owned()
    } else {
        commands
            .iter()
            .map(|(kind, n)| format!("{kind} x{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let start = logs::note(records, "start").cloned().unwrap_or(Value::Null);
    let field = |key: &str| {
        start.get(key).map_or_else(
            || "?".to_owned(),
            |value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            },
        )
    };
    let golden = ghost_drivers::replay::golden_path(out);
    let mut list = String::from("Review before committing:\n");
    let mut item = |line: String| {
        let _ = writeln!(list, "  [ ] {line}");
    };
    item(format!(
        "Placeholders: <HOME> x{}, <CWD> x{}, <ADAPTER> x{}, <USER> x{}, <HOST> x{}, <TIME> x{}, {sessions} native session id(s) as <SESSION_n>.",
        count("<HOME>") + count("<HOME_ENC>"),
        count("<CWD>") + count("<CWD_ENC>"),
        count("<ADAPTER>"),
        count("<USER>"),
        count("<HOST>"),
        count("<TIME>"),
    ));
    item(format!(
        "Redacted: <REDACTED> x{}, <EMAIL> x{}, <TOKEN> x{}, <SECRET> x{}. Check that nothing was redacted that the replay needs, such as an id.",
        count("<REDACTED>"),
        count("<EMAIL>"),
        count("<TOKEN>"),
        count("<SECRET>"),
    ));
    item(format!(
        "Read all {} stderr line(s) and {raw} raw stdout line(s): agents log account names, paths and prompts there.",
        by_direction("err")
    ));
    item(format!(
        "Read the {} message(s) from the agent: replies, tool output and file names show the folder's contents.",
        by_direction("in") - raw
    ));
    item(format!(
        "The session: agent {}, dialect {}, resume {}, autonomy {}; commands {commands}.",
        field("agent_id"),
        field("dialect"),
        field("resume"),
        field("autonomy"),
    ));
    item(format!(
        "Write the golden with ghost-cli replay {} --update, review {}, and commit both files.",
        out.display(),
        golden.display()
    ));
    list.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_names_the_scenario_and_the_agent_that_played_it() {
        let records = vec![
            json!({"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}}),
            json!({"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"hermes-agent","version":"0.22.5"}}}}),
        ];
        assert_eq!(
            scenario_note("basic", "hermes", &records),
            "Recorded scenario basic: hermes (hermes-agent 0.22.5), curated by ghost-cli."
        );
        assert_eq!(
            scenario_note("probe-session", "codex-acp", &[]),
            "Recorded scenario probe-session: codex-acp, curated by ghost-cli."
        );
    }

    #[test]
    fn the_context_of_this_machine_has_a_home() {
        let context = CurateContext::here();
        assert!(!context.home.is_empty());
        assert!(!context.host.ends_with(".local"));
    }
}
