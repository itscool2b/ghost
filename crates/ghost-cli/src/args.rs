//! The command line, read by a small hand-written parser.
//!
//! ```text
//! ghost-cli [--data-dir <dir>] agents
//! ghost-cli [--data-dir <dir>] probe <agent> [--env NAME=VALUE]... [--folder <dir>]
//! ghost-cli [--data-dir <dir>] smoke <agent> --scenario <names> --spend [--model <id>] [--effort <id>]
//! ghost-cli fixture <log> --scenario <name> --out <file>
//! ghost-cli replay <fixture> [--update]
//! ghost-cli help
//! ```
//!
//! Options may come before or after the command and its arguments, as `--name value` or
//! `--name=value`; `--` ends the options. Nothing here touches the file system: paths are checked
//! where they are used. `smoke` without `--spend` is refused here, before anything starts.

use std::ffi::OsString;
use std::path::PathBuf;

/// What to run, and the data root to run it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `--data-dir`: run on this data root's own profile (taking its owner lock), register in its
    /// `run/` and install adapters into its `adapters/`. Without it, the profile is a temporary one
    /// and `run/` and `adapters/` are the real data root's.
    pub data_dir: Option<PathBuf>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Agents,
    AgentAuth(String),
    AgentUpdate(String),
    Probe(ProbeArgs),
    Smoke(SmokeArgs),
    Fixture(FixtureArgs),
    Replay(ReplayArgs),
}

/// `probe <agent> [--env NAME=VALUE]... [--folder <dir>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeArgs {
    pub agent: String,
    /// Set on every agent process the probe starts, after the scrub, so a value here wins over the
    /// user's own (`LaunchSpec::set_env`).
    pub env: Vec<(String, String)>,
    /// The chat folder, instead of a fresh temporary git repository.
    pub folder: Option<PathBuf>,
}

/// `smoke <agent> --scenario <names> --spend [--model <id>] [--effort <id>]`. `--spend` is not
/// kept: a smoke run without it never parses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeArgs {
    pub agent: String,
    /// In the order given; every scenario a later one needs comes earlier ([`Scenario::needs`]).
    pub scenarios: Vec<Scenario>,
    /// A Model option choice to switch to before the first prompt.
    pub model: Option<String>,
    /// An Effort option choice to switch to before the first prompt.
    pub effort: Option<String>,
}

/// `fixture <log> --scenario <name> --out <file>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureArgs {
    pub log: PathBuf,
    /// The fixture's name: lowercase letters, digits and inner hyphens.
    pub scenario: String,
    pub out: PathBuf,
}

/// `replay <fixture> [--update]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayArgs {
    pub fixture: PathBuf,
    /// Write the golden instead of comparing with it.
    pub update: bool,
}

/// The paid scenarios of spec 10.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scenario {
    Basic,
    Permission,
    Interrupt,
    AfterInterrupt,
    FullAuto,
    Resume,
}

impl Scenario {
    /// Every scenario, in the spec's order: what `--scenario all` runs.
    pub const ALL: [Scenario; 6] = [
        Scenario::Basic,
        Scenario::Permission,
        Scenario::Interrupt,
        Scenario::AfterInterrupt,
        Scenario::FullAuto,
        Scenario::Resume,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Basic => "basic",
            Scenario::Permission => "permission",
            Scenario::Interrupt => "interrupt",
            Scenario::AfterInterrupt => "after-interrupt",
            Scenario::FullAuto => "full-auto",
            Scenario::Resume => "resume",
        }
    }

    pub fn from_name(name: &str) -> Option<Scenario> {
        Scenario::ALL
            .into_iter()
            .find(|scenario| scenario.name() == name)
    }

    /// The scenarios that must run earlier in the same invocation, in the same chat:
    /// `after-interrupt` asks about the interrupted answer, `resume` about the files the other two
    /// created.
    pub fn needs(self) -> &'static [Scenario] {
        match self {
            Scenario::AfterInterrupt => &[Scenario::Interrupt],
            Scenario::Resume => &[Scenario::Permission, Scenario::FullAuto],
            Scenario::Basic | Scenario::Permission | Scenario::Interrupt | Scenario::FullAuto => {
                &[]
            }
        }
    }
}

/// Why the command line could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    #[error("{0}")]
    Invalid(String),
    #[error(
        "smoke sends real prompts to {agent}, which uses your quota or costs money. Add --spend to run it."
    )]
    SpendRequired { agent: String },
}

impl UsageError {
    fn invalid(message: impl Into<String>) -> UsageError {
        UsageError::Invalid(message.into())
    }
}

/// The help text `ghost-cli help` prints.
pub const USAGE: &str = "\
ghost-cli: Ghost's headless dev tool. It runs agents through a headless session manager on a
temporary profile, with a fresh temporary git repository as the chat folder.

Usage:
  ghost-cli [--data-dir <dir>] agents
  ghost-cli [--data-dir <dir>] agent-auth <agent>
  ghost-cli [--data-dir <dir>] agent-update <agent>
  ghost-cli [--data-dir <dir>] probe <agent> [--env NAME=VALUE]... [--folder <dir>]
  ghost-cli [--data-dir <dir>] smoke <agent> --scenario <names> --spend
           [--model <id>] [--effort <id>]
  ghost-cli fixture <log> --scenario <name> --out <file>
  ghost-cli replay <fixture> [--update]
  ghost-cli help

Commands:
  agents    The environment agents start with, the variable names the scrub removes, and whether
            each agent can start here.
  agent-auth    Check local authentication using the vendor's own command.
  agent-update  Explicitly run the vendor update for the detected installation channel.
  probe     Starts the agent and runs the free probe steps of the M1a spec (10.2): install,
            initialize, the session's options and commands, option and autonomy changes, a cancel
            while idle, closing stdin, a resume, and an interrupted start. Sends no prompt.
  smoke     Runs paid scenarios (spec 10.3) in one chat, one agent process per scenario, and stops
            at the first failure. Refused without --spend.
  fixture   Curates a protocol log into a replay fixture: placeholders, the redaction scan, a
            replay check, and a checklist for the review.
  replay    Replays a fixture through its real agent driver and compares the result with its golden
            (<fixture>.events.json); --update writes the golden instead.

Options:
  --data-dir <dir>      Use this data root for everything: its own profile (refused while the app
                        runs on it), its run/ folder and its adapters/ folder. Without it the
                        profile is temporary, and run/ and adapters/ are the real data root's.
  --env NAME=VALUE      (probe) Set on every agent process the probe starts; wins over your own
                        value. Repeat it for more variables.
  --folder <dir>        (probe) The chat folder, instead of a fresh temporary git repository.
  --scenario <names>    (smoke) basic, permission, interrupt, after-interrupt, full-auto, resume,
                        a comma-separated list of them, or all. after-interrupt needs interrupt
                        earlier in the list, and resume needs permission and full-auto.
                        (fixture) The fixture's name, such as basic or probe-session.
  --spend               (smoke) Confirms that the scenarios may send real prompts.
  --model <id>          (smoke) Switch the model to this choice before the first prompt.
  --effort <id>         (smoke) Switch the effort to this choice before the first prompt.
  --out <file>          (fixture) Where to write the fixture.
  --update              (replay) Write the golden instead of comparing with it.

Exit status: 0 when everything passed, 1 when a check or the command failed, 2 for a usage error.
";

/// Options that take no value.
const FLAGS: [&str; 3] = ["--spend", "--update", "--help"];

/// Options that take a value.
const VALUED: [&str; 7] = [
    "--data-dir",
    "--env",
    "--folder",
    "--scenario",
    "--model",
    "--effort",
    "--out",
];

/// The command line split into its parts, before any command's rules apply.
#[derive(Debug, Default)]
struct Parts {
    positionals: Vec<OsString>,
    flags: Vec<&'static str>,
    valued: Vec<(&'static str, OsString)>,
}

/// Reads a command line, without the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, UsageError> {
    let parts = split(args)?;
    if parts.flags.contains(&"--help") {
        return Ok(Invocation {
            data_dir: None,
            command: Command::Help,
        });
    }
    let mut positionals = parts.positionals.into_iter();
    let Some(name) = positionals.next() else {
        return Err(UsageError::invalid(
            "no command given; the commands are agents, probe, smoke, fixture, replay and help",
        ));
    };
    let name = text(name, "the command")?;
    let rest: Vec<OsString> = positionals.collect();
    let options = Options {
        command: name.clone(),
        flags: parts.flags,
        valued: parts.valued,
    };
    let data_dir = match name.as_str() {
        "agents" | "agent-auth" | "agent-update" | "probe" | "smoke" => {
            options.path("--data-dir")?
        }
        _ => None,
    };
    let command = match name.as_str() {
        "help" => {
            options.allow(&[], &[])?;
            Command::Help
        }
        "agents" => {
            options.allow(&["--data-dir"], &[])?;
            no_more(&rest)?;
            Command::Agents
        }
        "agent-auth" | "agent-update" => {
            options.allow(&["--data-dir"], &[])?;
            let id = agent_id(one(rest, "this command needs an agent id")?)?;
            if name == "agent-auth" {
                Command::AgentAuth(id)
            } else {
                Command::AgentUpdate(id)
            }
        }
        "probe" => {
            options.allow(&["--data-dir", "--env", "--folder"], &[])?;
            let agent = one(
                rest,
                "probe needs an agent id, such as: ghost-cli probe hermes",
            )?;
            let agent = agent_id(agent)?;
            let mut env = Vec::new();
            for value in options.all("--env") {
                env.push(env_pair(value)?);
            }
            let folder = options.path("--folder")?;
            Command::Probe(ProbeArgs { agent, env, folder })
        }
        "smoke" => {
            options.allow(
                &["--data-dir", "--scenario", "--model", "--effort"],
                &["--spend"],
            )?;
            let agent = one(
                rest,
                "smoke needs an agent id, such as: ghost-cli smoke hermes --scenario basic --spend",
            )?;
            let agent = agent_id(agent)?;
            let scenarios = match options.single("--scenario")? {
                Some(value) => scenarios(text(value, "--scenario")?)?,
                None => {
                    return Err(UsageError::invalid(
                        "smoke needs --scenario: basic, permission, interrupt, after-interrupt, full-auto, resume, a comma-separated list of them, or all",
                    ));
                }
            };
            let model = options.word("--model")?;
            let effort = options.word("--effort")?;
            if !options.flags.contains(&"--spend") {
                return Err(UsageError::SpendRequired { agent });
            }
            Command::Smoke(SmokeArgs {
                agent,
                scenarios,
                model,
                effort,
            })
        }
        "fixture" => {
            options.allow(&["--scenario", "--out"], &[])?;
            let log = one(
                rest,
                "fixture needs a protocol log, such as: ghost-cli fixture <log> --scenario basic --out <file>",
            )?;
            let scenario = match options.single("--scenario")? {
                Some(value) => fixture_name(text(value, "--scenario")?)?,
                None => {
                    return Err(UsageError::invalid(
                        "fixture needs --scenario <name>: the fixture's name",
                    ));
                }
            };
            let Some(out) = options.path("--out")? else {
                return Err(UsageError::invalid(
                    "fixture needs --out <file>: where to write the fixture",
                ));
            };
            Command::Fixture(FixtureArgs {
                log: non_empty_path(log, "the protocol log")?,
                scenario,
                out,
            })
        }
        "replay" => {
            options.allow(&[], &["--update"])?;
            let fixture = one(
                rest,
                "replay needs a fixture, such as: ghost-cli replay crates/ghost-drivers/tests/fixtures/acp/hermes/basic.jsonl",
            )?;
            Command::Replay(ReplayArgs {
                fixture: non_empty_path(fixture, "the fixture")?,
                update: options.flags.contains(&"--update"),
            })
        }
        other => {
            return Err(UsageError::invalid(format!(
                "unknown command \"{other}\"; the commands are agents, probe, smoke, fixture, replay and help"
            )));
        }
    };
    Ok(Invocation { data_dir, command })
}

/// Splits the arguments into positionals, flags and options with their values.
fn split(args: impl IntoIterator<Item = OsString>) -> Result<Parts, UsageError> {
    let mut parts = Parts::default();
    let mut args = args.into_iter();
    let mut options_ended = false;
    while let Some(arg) = args.next() {
        if options_ended {
            parts.positionals.push(arg);
            continue;
        }
        let Some(raw) = arg.to_str() else {
            if arg.to_string_lossy().starts_with('-') {
                return Err(UsageError::invalid(format!(
                    "the option {} isn't valid UTF-8",
                    arg.to_string_lossy()
                )));
            }
            parts.positionals.push(arg);
            continue;
        };
        if raw == "--" {
            options_ended = true;
            continue;
        }
        if raw == "-h" {
            parts.flags.push("--help");
            continue;
        }
        if !raw.starts_with('-') || raw == "-" {
            parts.positionals.push(arg);
            continue;
        }
        let (name, inline) = match raw.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (raw, None),
        };
        if let Some(flag) = FLAGS.iter().find(|flag| **flag == name) {
            if inline.is_some() {
                return Err(UsageError::invalid(format!("{flag} takes no value")));
            }
            parts.flags.push(flag);
        } else if let Some(option) = VALUED.iter().find(|option| **option == name) {
            let value = match inline {
                Some(value) => OsString::from(value),
                None => match args.next() {
                    Some(value) if !value.to_string_lossy().starts_with("--") => value,
                    _ => return Err(UsageError::invalid(format!("{option} needs a value"))),
                },
            };
            parts.valued.push((option, value));
        } else {
            return Err(UsageError::invalid(format!(
                "unknown option {name}; run ghost-cli help for the options"
            )));
        }
    }
    Ok(parts)
}

/// The options of one command.
struct Options {
    command: String,
    flags: Vec<&'static str>,
    valued: Vec<(&'static str, OsString)>,
}

impl Options {
    /// Refuses every option the command does not take.
    fn allow(&self, valued: &[&str], flags: &[&str]) -> Result<(), UsageError> {
        let given = self
            .valued
            .iter()
            .map(|(name, _)| *name)
            .chain(self.flags.iter().copied());
        for name in given {
            if !valued.contains(&name) && !flags.contains(&name) {
                return Err(UsageError::invalid(format!(
                    "{name} doesn't apply to {}",
                    self.command
                )));
            }
        }
        Ok(())
    }

    /// Every value of a repeatable option, in order.
    fn all(&self, name: &str) -> Vec<OsString> {
        self.valued
            .iter()
            .filter(|(option, _)| *option == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// The value of an option given at most once.
    fn single(&self, name: &str) -> Result<Option<OsString>, UsageError> {
        let mut values = self.all(name).into_iter();
        let first = values.next();
        if values.next().is_some() {
            return Err(UsageError::invalid(format!(
                "{name} is given more than once"
            )));
        }
        Ok(first)
    }

    fn path(&self, name: &str) -> Result<Option<PathBuf>, UsageError> {
        self.single(name)?
            .map(|value| non_empty_path(value, name))
            .transpose()
    }

    /// A one-word value: valid UTF-8, not empty, no whitespace.
    fn word(&self, name: &str) -> Result<Option<String>, UsageError> {
        let Some(value) = self.single(name)? else {
            return Ok(None);
        };
        let value = text(value, name)?;
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(UsageError::invalid(format!(
                "{name} needs one word, got \"{value}\""
            )));
        }
        Ok(Some(value))
    }
}

fn text(value: OsString, what: &str) -> Result<String, UsageError> {
    value.into_string().map_err(|value| {
        UsageError::invalid(format!(
            "{what} isn't valid UTF-8: {}",
            value.to_string_lossy()
        ))
    })
}

fn non_empty_path(value: OsString, what: &str) -> Result<PathBuf, UsageError> {
    if value.is_empty() {
        return Err(UsageError::invalid(format!("{what} is empty")));
    }
    Ok(PathBuf::from(value))
}

/// Exactly one positional argument.
fn one(rest: Vec<OsString>, missing: &str) -> Result<OsString, UsageError> {
    let mut rest = rest.into_iter();
    let Some(first) = rest.next() else {
        return Err(UsageError::invalid(missing));
    };
    no_more(&rest.collect::<Vec<_>>())?;
    Ok(first)
}

fn no_more(rest: &[OsString]) -> Result<(), UsageError> {
    match rest.first() {
        Some(extra) => Err(UsageError::invalid(format!(
            "unexpected argument \"{}\"",
            extra.to_string_lossy()
        ))),
        None => Ok(()),
    }
}

/// An agent id: checked against the detector's agents when the command runs.
fn agent_id(value: OsString) -> Result<String, UsageError> {
    let id = text(value, "the agent id")?;
    if id.is_empty() || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(UsageError::invalid(format!(
            "\"{id}\" isn't an agent id; run ghost-cli agents for the list"
        )));
    }
    Ok(id)
}

/// `NAME=VALUE`; the value may be empty and may hold `=`.
fn env_pair(value: OsString) -> Result<(String, String), UsageError> {
    let pair = text(value, "--env")?;
    match pair.split_once('=') {
        Some((name, value))
            if !name.is_empty()
                && !name.chars().any(|c| c.is_whitespace() || c == '\0')
                && !value.contains('\0') =>
        {
            Ok((name.to_owned(), value.to_owned()))
        }
        _ => Err(UsageError::invalid(format!(
            "--env needs NAME=VALUE, got \"{pair}\""
        ))),
    }
}

/// `all`, or scenario names separated by commas, each once, every scenario's needs earlier.
fn scenarios(value: String) -> Result<Vec<Scenario>, UsageError> {
    if value == "all" {
        return Ok(Scenario::ALL.to_vec());
    }
    let mut picked: Vec<Scenario> = Vec::new();
    for name in value.split(',').map(str::trim) {
        let Some(scenario) = Scenario::from_name(name) else {
            return Err(UsageError::invalid(format!(
                "unknown scenario \"{name}\"; the scenarios are basic, permission, interrupt, after-interrupt, full-auto, resume and all"
            )));
        };
        if picked.contains(&scenario) {
            return Err(UsageError::invalid(format!(
                "the scenario {name} is listed twice"
            )));
        }
        for need in scenario.needs() {
            if !picked.contains(need) {
                return Err(UsageError::invalid(format!(
                    "{name} continues the chat of {}, so list {} before it (for example --scenario {})",
                    need.name(),
                    need.name(),
                    example_for(scenario)
                )));
            }
        }
        picked.push(scenario);
    }
    Ok(picked)
}

/// The shortest list that runs `scenario` with everything it needs.
fn example_for(scenario: Scenario) -> String {
    let mut names: Vec<&str> = scenario.needs().iter().map(|need| need.name()).collect();
    names.push(scenario.name());
    names.join(",")
}

/// A fixture name: a file stem that reads the same everywhere.
fn fixture_name(name: String) -> Result<String, UsageError> {
    let valid = !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(name)
    } else {
        Err(UsageError::invalid(format!(
            "\"{name}\" can't name a fixture: use lowercase letters, digits and inner hyphens, such as basic or probe-session"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &[&str]) -> Vec<OsString> {
        line.iter().map(OsString::from).collect()
    }

    fn parsed(line: &[&str]) -> Invocation {
        parse(args(line)).unwrap_or_else(|error| panic!("{line:?}: {error}"))
    }

    fn refused(line: &[&str]) -> String {
        match parse(args(line)) {
            Ok(invocation) => panic!("{line:?} parsed: {invocation:?}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn every_command_parses_in_its_documented_form() {
        assert_eq!(parsed(&["agents"]).command, Command::Agents);
        assert_eq!(
            parsed(&["agent-auth", "codex"]).command,
            Command::AgentAuth("codex".into())
        );
        assert_eq!(
            parsed(&["agent-update", "claude"]).command,
            Command::AgentUpdate("claude".into())
        );
        assert_eq!(
            parsed(&[
                "probe",
                "codex-acp",
                "--env",
                "CODEX_HOME=/tmp/empty",
                "--env=A=b=c",
                "--folder",
                "/tmp/repo"
            ])
            .command,
            Command::Probe(ProbeArgs {
                agent: "codex-acp".into(),
                env: vec![
                    ("CODEX_HOME".into(), "/tmp/empty".into()),
                    ("A".into(), "b=c".into())
                ],
                folder: Some(PathBuf::from("/tmp/repo")),
            })
        );
        assert_eq!(
            parsed(&[
                "smoke",
                "hermes",
                "--scenario",
                "basic",
                "--spend",
                "--model",
                "p:cheap",
                "--effort=low"
            ])
            .command,
            Command::Smoke(SmokeArgs {
                agent: "hermes".into(),
                scenarios: vec![Scenario::Basic],
                model: Some("p:cheap".into()),
                effort: Some("low".into()),
            })
        );
        assert_eq!(
            parsed(&[
                "fixture",
                "/logs/x.jsonl",
                "--scenario",
                "probe-session",
                "--out",
                "f.jsonl"
            ])
            .command,
            Command::Fixture(FixtureArgs {
                log: PathBuf::from("/logs/x.jsonl"),
                scenario: "probe-session".into(),
                out: PathBuf::from("f.jsonl"),
            })
        );
        assert_eq!(
            parsed(&["replay", "a.jsonl"]).command,
            Command::Replay(ReplayArgs {
                fixture: PathBuf::from("a.jsonl"),
                update: false
            })
        );
        assert_eq!(
            parsed(&["replay", "--update", "a.jsonl"]).command,
            Command::Replay(ReplayArgs {
                fixture: PathBuf::from("a.jsonl"),
                update: true
            })
        );
    }

    #[test]
    fn help_wins_wherever_it_is() {
        for line in [
            &["help"][..],
            &["--help"],
            &["-h"],
            &["probe", "hermes", "--help"],
            &["smoke", "-h"],
            &["--bogus", "--help"],
        ] {
            // An unknown option is refused before help is looked at.
            if line.contains(&"--bogus") {
                assert!(refused(line).contains("unknown option --bogus"));
                continue;
            }
            assert_eq!(parsed(line).command, Command::Help, "{line:?}");
        }
    }

    #[test]
    fn the_data_dir_goes_anywhere_but_only_where_it_applies() {
        let before = parsed(&["--data-dir", "/d", "probe", "mock"]);
        let after = parsed(&["probe", "mock", "--data-dir=/d"]);
        assert_eq!(before, after);
        assert_eq!(before.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(parsed(&["agents"]).data_dir, None);
        assert_eq!(
            parsed(&[
                "smoke",
                "mock",
                "--data-dir",
                "/d",
                "--scenario",
                "all",
                "--spend"
            ])
            .data_dir,
            Some(PathBuf::from("/d"))
        );
        assert_eq!(
            refused(&["--data-dir", "/d", "replay", "x.jsonl"]),
            "--data-dir doesn't apply to replay"
        );
        assert_eq!(
            refused(&[
                "fixture",
                "l",
                "--scenario",
                "s",
                "--out",
                "o",
                "--data-dir",
                "/d"
            ]),
            "--data-dir doesn't apply to fixture"
        );
    }

    #[test]
    fn smoke_without_spend_is_refused_once_everything_else_is_right() {
        assert_eq!(
            parse(args(&["smoke", "claude-acp", "--scenario", "basic"])),
            Err(UsageError::SpendRequired {
                agent: "claude-acp".into()
            })
        );
        let message = refused(&["smoke", "mock", "--scenario", "all"]);
        assert!(message.contains("Add --spend"), "{message}");
        assert!(message.contains("costs money"), "{message}");
        // The other problems come first, so fixing them never leads to a run that spends.
        assert!(refused(&["smoke", "mock"]).starts_with("smoke needs --scenario"));
        assert!(refused(&["smoke", "--scenario", "basic"]).starts_with("smoke needs an agent id"));
        assert!(refused(&["smoke", "mock", "--scenario", "nope"]).starts_with("unknown scenario"));
        assert_eq!(
            refused(&["smoke", "mock", "--scenario", "basic", "--spend=yes"]),
            "--spend takes no value"
        );
    }

    #[test]
    fn scenarios_need_their_earlier_scenarios() {
        let list = |value: &str| match parsed(&["smoke", "mock", "--scenario", value, "--spend"])
            .command
        {
            Command::Smoke(smoke) => smoke.scenarios,
            other => panic!("{other:?}"),
        };
        assert_eq!(list("all"), Scenario::ALL.to_vec());
        assert_eq!(
            list("interrupt,after-interrupt"),
            [Scenario::Interrupt, Scenario::AfterInterrupt]
        );
        assert_eq!(
            list("full-auto, permission ,resume"),
            [Scenario::FullAuto, Scenario::Permission, Scenario::Resume]
        );
        let alone = refused(&["smoke", "mock", "--scenario", "after-interrupt", "--spend"]);
        assert!(
            alone.contains("list interrupt before it")
                && alone.contains("--scenario interrupt,after-interrupt"),
            "{alone}"
        );
        let resume = refused(&[
            "smoke",
            "mock",
            "--scenario",
            "permission,resume",
            "--spend",
        ]);
        assert!(
            resume.contains("list full-auto before it")
                && resume.contains("--scenario permission,full-auto,resume"),
            "{resume}"
        );
        assert!(
            refused(&["smoke", "mock", "--scenario", "basic,basic", "--spend"])
                .contains("listed twice")
        );
        for scenario in Scenario::ALL {
            assert_eq!(Scenario::from_name(scenario.name()), Some(scenario));
            for need in scenario.needs() {
                assert!(need < &scenario, "{scenario:?} needs a later scenario");
            }
        }
    }

    #[test]
    fn options_are_checked_for_values_repeats_and_the_command() {
        assert_eq!(
            refused(&["probe", "mock", "--folder"]),
            "--folder needs a value"
        );
        assert_eq!(
            refused(&["probe", "mock", "--folder", "--env", "A=1"]),
            "--folder needs a value"
        );
        assert_eq!(
            refused(&["probe", "mock", "--folder", "/a", "--folder", "/b"]),
            "--folder is given more than once"
        );
        assert_eq!(
            refused(&["probe", "mock", "--model", "x"]),
            "--model doesn't apply to probe"
        );
        assert_eq!(
            refused(&["agents", "--update"]),
            "--update doesn't apply to agents"
        );
        assert!(refused(&["probe", "mock", "-x"]).starts_with("unknown option -x"));
        assert!(
            refused(&["probe", "mock", "--env", "NOEQUALS"]).starts_with("--env needs NAME=VALUE")
        );
        assert!(refused(&["probe", "mock", "--env", "=v"]).starts_with("--env needs NAME=VALUE"));
        assert!(
            refused(&[
                "smoke",
                "mock",
                "--scenario",
                "basic",
                "--spend",
                "--model",
                "two words"
            ])
            .starts_with("--model needs one word")
        );
        assert_eq!(
            refused(&["probe", "mock", "--folder="]),
            "--folder is empty"
        );
    }

    #[test]
    fn positionals_are_counted() {
        assert!(refused(&[]).starts_with("no command given"));
        assert!(refused(&["launch"]).starts_with("unknown command \"launch\""));
        assert_eq!(
            refused(&["agents", "extra"]),
            "unexpected argument \"extra\""
        );
        assert!(refused(&["probe"]).starts_with("probe needs an agent id"));
        assert_eq!(refused(&["probe", "a", "b"]), "unexpected argument \"b\"");
        assert!(refused(&["replay"]).starts_with("replay needs a fixture"));
        assert!(
            refused(&["fixture", "--scenario", "s", "--out", "o"])
                .starts_with("fixture needs a protocol log")
        );
        assert!(refused(&["fixture", "l", "--out", "o"]).starts_with("fixture needs --scenario"));
        assert!(refused(&["fixture", "l", "--scenario", "s"]).starts_with("fixture needs --out"));
        // `--` ends the options, so a file named like one can still be passed.
        assert_eq!(
            parsed(&["replay", "--", "--odd.jsonl"]).command,
            Command::Replay(ReplayArgs {
                fixture: PathBuf::from("--odd.jsonl"),
                update: false
            })
        );
    }

    #[test]
    fn fixture_names_are_file_stems_that_read_the_same_everywhere() {
        for good in ["basic", "probe-session", "after-interrupt", "turn2"] {
            assert_eq!(fixture_name(good.into()).as_deref(), Ok(good));
        }
        for bad in ["", "Basic", "-x", "x-", "a b", "a/b", "../x", "a_b", "a.b"] {
            assert!(fixture_name(bad.into()).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn paths_need_not_be_utf8_but_options_and_ids_must_be() {
        use std::os::unix::ffi::OsStringExt as _;
        let odd = OsString::from_vec(b"/tmp/caf\xe9".to_vec());
        let invocation = parse(vec![
            OsString::from("probe"),
            OsString::from("mock"),
            OsString::from("--folder"),
            odd.clone(),
        ])
        .expect("parses");
        assert_eq!(
            invocation.command,
            Command::Probe(ProbeArgs {
                agent: "mock".into(),
                env: Vec::new(),
                folder: Some(PathBuf::from(odd.clone())),
            })
        );
        let error = parse(vec![OsString::from("probe"), odd]).expect_err("an id must be UTF-8");
        assert!(
            error
                .to_string()
                .starts_with("the agent id isn't valid UTF-8")
        );
    }

    #[test]
    fn the_usage_names_every_command_and_option() {
        for word in [
            "agents",
            "probe",
            "smoke",
            "fixture",
            "replay",
            "help",
            "--data-dir",
            "--env",
            "--folder",
            "--scenario",
            "--spend",
            "--model",
            "--effort",
            "--out",
            "--update",
        ] {
            assert!(USAGE.contains(word), "{word}");
        }
        for scenario in Scenario::ALL {
            assert!(USAGE.contains(scenario.name()), "{scenario:?}");
        }
        assert!(
            USAGE.lines().all(|line| line.chars().count() <= 100),
            "the usage fits a terminal"
        );
    }
}
