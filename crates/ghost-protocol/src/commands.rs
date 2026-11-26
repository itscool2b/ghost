//! A shell command an agent ran, read from its tool call: what it was, how it ended and the first
//! lines of what went wrong. A handoff to another agent (ghost-core) and a bot's reading of a turn
//! (ghost-remote) both tell it this way, so it lives with the events it is read from.

use serde_json::Value;

use crate::{ToolCall, ToolContent, ToolKind, ToolStatus};

/// A shell command an agent ran in a turn, as the next agent is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRun {
    pub command: String,
    /// Where it ran, when the agent said and it is not the chat's folder.
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub status: ToolStatus,
    /// What it printed, as far as the transcript kept it inline.
    pub output: String,
}

impl CommandRun {
    /// The command a tool call ran, when it ran one: an `execute` call, or any call whose input
    /// names a command (some agents leave the kind for a later update).
    pub fn of(call: &ToolCall) -> Option<CommandRun> {
        let input = call.raw_input.as_ref();
        let named = input.and_then(|input| {
            let value = input.get("command").or_else(|| input.get("cmd"))?;
            match value {
                Value::String(text) => Some(text.clone()),
                // ["bash", "-lc", "cargo test"]: the script is what was run.
                Value::Array(parts) => {
                    let words: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                    match words.as_slice() {
                        [shell, flag, script]
                            if flag.starts_with('-')
                                && flag.ends_with('c')
                                && !shell.is_empty() =>
                        {
                            Some((*script).to_owned())
                        }
                        _ => Some(words.join(" ")),
                    }
                }
                _ => None,
            }
        });
        let command = match named {
            Some(command) => command,
            None if call.kind == ToolKind::Execute => call.title.clone(),
            None => return None,
        };
        let command = command.trim().to_owned();
        if command.is_empty() {
            return None;
        }
        let cwd = input
            .and_then(|input| input.get("cwd").or_else(|| input.get("workdir")))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut output = String::new();
        let mut exit_code = None;
        for content in &call.content {
            match content {
                ToolContent::Terminal {
                    output: text,
                    exit_code: code,
                    ..
                } => {
                    output.push_str(text);
                    exit_code = exit_code.or(*code);
                }
                ToolContent::Text { text, .. } => output.push_str(text),
                ToolContent::Diff { .. } => {}
            }
        }
        Some(CommandRun {
            command,
            cwd,
            exit_code,
            status: call.status,
            output: plain(&output),
        })
    }

    pub fn failed(&self) -> bool {
        self.status == ToolStatus::Failed || self.exit_code.is_some_and(|code| code != 0)
    }

    pub fn succeeded(&self) -> bool {
        !self.failed() && (self.status == ToolStatus::Completed || self.exit_code == Some(0))
    }

    /// `exit 101`, `failed`, `ok` or `running`: how it ended, in one word or two.
    pub fn outcome(&self) -> String {
        match (self.exit_code, self.status) {
            (Some(code), _) => format!("exit {code}"),
            (None, ToolStatus::Failed) => "failed".into(),
            (None, ToolStatus::Completed) => "ok".into(),
            (None, _) => "running".into(),
        }
    }

    /// The command on one line: a script of several lines shows its first, marked as cut.
    pub fn line(&self) -> String {
        let mut lines = self.command.lines().filter(|line| !line.trim().is_empty());
        let first = lines.next().unwrap_or_default().trim();
        let more = lines.next().is_some();
        let short = cut(first, COMMAND_BYTES);
        if more || short.len() < first.len() {
            format!("{short} …")
        } else {
            short.to_owned()
        }
    }
}

const COMMAND_BYTES: usize = 300;
pub const ERROR_BYTES: usize = 1200;
const ERROR_LINES: usize = 12;

/// Terminal output without its colour codes and carriage-return redraws.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ parameters final-byte. Anything else: drop the next character.
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// The first lines that look like the error, and a few after them; or the output's last lines.
pub fn first_error(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let looks_wrong = |line: &&str| {
        let lower = line.to_ascii_lowercase();
        let calm = [
            "0 failed",
            "0 errors",
            "no errors",
            "0 error(s)",
            "error: 0",
        ]
        .iter()
        .any(|ok| lower.contains(ok));
        !calm
            && [
                "error",
                "failed",
                "failure",
                "panicked",
                "fatal",
                "traceback",
                "exception",
                "not found",
                "cannot find",
                "could not",
                "unable to",
                "no such",
                "refused",
                "timed out",
                "denied",
            ]
            .iter()
            .any(|word| lower.contains(word))
    };
    let chosen: Vec<&str> = match lines.iter().position(looks_wrong) {
        Some(start) => lines[start..].iter().take(ERROR_LINES).copied().collect(),
        None => lines[lines.len().saturating_sub(ERROR_LINES)..].to_vec(),
    };
    let joined = chosen.join("\n");
    let trimmed = joined.trim_matches('\n');
    let short = cut(trimmed, ERROR_BYTES);
    if short.len() < trimmed.len() {
        format!("{short}\n…")
    } else {
        short.to_owned()
    }
}

/// What a secret in a command line becomes in a handoff.
const HIDDEN: &str = "<SECRET>";

/// A command line with the values of secret-looking flags and variables hidden: `--token X`,
/// `--password=X`, `-p X` is left (too common), `API_KEY=X`, `GITHUB_TOKEN=X`. Token shapes and
/// the agents' own credentials are the session redactor's (`ghost_process` log redaction); this
/// catches what a command names only by its flag or variable.
pub fn hide_secret_flags(command: &str) -> String {
    const NAMES: [&str; 6] = [
        "token",
        "password",
        "passwd",
        "secret",
        "apikey",
        "credential",
    ];
    let secret_name = |name: &str| {
        let plain: String = name
            .trim_start_matches('-')
            .chars()
            .filter(|c| *c != '-' && *c != '_')
            .collect::<String>()
            .to_ascii_lowercase();
        !plain.is_empty() && NAMES.iter().any(|word| plain.ends_with(word))
    };
    let mut out = Vec::new();
    let mut hide_next = false;
    for word in command.split(' ') {
        if hide_next && !word.is_empty() {
            out.push(HIDDEN.to_owned());
            hide_next = false;
            continue;
        }
        match word.split_once('=') {
            // --token=X, API_KEY=X (a flag or an assignment, never a URL's query).
            Some((name, value))
                if !value.is_empty()
                    && !name.contains('/')
                    && !name.contains(':')
                    && (name.starts_with('-')
                        || name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    && secret_name(name) =>
            {
                out.push(format!("{name}={HIDDEN}"));
            }
            _ => {
                hide_next = word.starts_with("--") && secret_name(word);
                out.push(word.to_owned());
            }
        }
    }
    out.join(" ")
}

pub fn cut(value: &str, bytes: usize) -> &str {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
