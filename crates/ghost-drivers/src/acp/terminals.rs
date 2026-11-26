//! Display-only terminals: Zed's ACP extension, which codex-acp speaks once a client advertises
//! `clientCapabilities._meta.terminal_output`. The agent runs a command itself and embeds
//! `{"type": "terminal", "terminalId": ..}` in the tool call (with `_meta.terminal_info`), streams
//! its output on later updates as `_meta.terminal_output {terminal_id, data}` and ends with
//! `_meta.terminal_exit {terminal_id, exit_code, signal}`. Ghost runs nothing: it shows the
//! output as the tool's terminal, a short live tail while the command runs (small enough never to
//! spill a blob per chunk) and, once it exits, the output it kept, which the session stores whole.
//! Escape sequences are dropped and a carriage return rewrites its line, as a terminal shows it.

use std::collections::HashMap;
use std::time::Duration;

use ghost_protocol::{ToolContent, Truncation};
use serde_json::Value;
use tokio::time::Instant;

/// What a running command shows: its last bytes. Escaped as JSON it stays well within an event.
pub const LIVE_TAIL: usize = 3000;
/// What a command keeps once it exits: its last 256 KiB, with a line saying how much came before.
pub const KEEP: usize = 256 * 1024;
/// A running command shows its output at most this often.
pub const FLUSH: Duration = Duration::from_millis(200);
/// Terminals tracked at once; the oldest that has ended makes room.
const MAX_TERMINALS: usize = 64;

/// One `_meta` entry about a terminal on a tool call or its update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalMeta {
    Info { id: String },
    Output { id: String, data: String },
    Exit { id: String, exit_code: Option<i32> },
}

/// The terminal entries in an update's `_meta`, in the order they are applied.
pub fn meta(update: &Value) -> Vec<TerminalMeta> {
    let meta = &update["_meta"];
    let id = |entry: &Value| {
        entry["terminal_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    let mut found = Vec::new();
    if let Some(id) = id(&meta["terminal_info"]) {
        found.push(TerminalMeta::Info { id });
    }
    if let (Some(id), Some(data)) = (
        id(&meta["terminal_output"]),
        meta["terminal_output"]["data"].as_str(),
    ) {
        found.push(TerminalMeta::Output {
            id,
            data: data.to_owned(),
        });
    }
    if let Some(id) = id(&meta["terminal_exit"]) {
        found.push(TerminalMeta::Exit {
            id,
            exit_code: meta["terminal_exit"]["exit_code"]
                .as_i64()
                .and_then(|code| i32::try_from(code).ok()),
        });
    }
    found
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Start,
    /// A control sequence (`ESC [`), until its final byte.
    Csi,
    /// An operating system command (`ESC ]`), until BEL or `ESC \`.
    Osc,
    /// ESC inside an OSC: `\` ends it.
    OscEnd,
}

/// A command's output as a terminal shows it: escape sequences dropped (even split across
/// chunks), CR LF as a newline, a lone CR rewriting its line, and only the last [`KEEP`] bytes.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Screen {
    text: String,
    /// Bytes dropped from the front to stay within [`KEEP`].
    dropped: u64,
    escape: Escape,
    /// A CR not yet followed by anything: the next character rewrites the line.
    cr: bool,
}

impl Screen {
    pub fn feed(&mut self, data: &str) {
        for c in data.chars() {
            match self.escape {
                Escape::Start => {
                    self.escape = match c {
                        '[' => Escape::Csi,
                        ']' => Escape::Osc,
                        _ => Escape::None,
                    };
                    continue;
                }
                Escape::Csi => {
                    if ('@'..='~').contains(&c) {
                        self.escape = Escape::None;
                    }
                    continue;
                }
                Escape::Osc => {
                    match c {
                        '\u{7}' => self.escape = Escape::None,
                        '\u{1b}' => self.escape = Escape::OscEnd,
                        _ => {}
                    }
                    continue;
                }
                Escape::OscEnd => {
                    self.escape = if c == '\\' { Escape::None } else { Escape::Osc };
                    continue;
                }
                Escape::None => {}
            }
            match c {
                '\u{1b}' => self.escape = Escape::Start,
                '\r' => self.cr = true,
                '\n' => {
                    self.cr = false;
                    self.text.push('\n');
                }
                c if c == '\t' || !c.is_control() => {
                    if std::mem::take(&mut self.cr) {
                        let start = self.text.rfind('\n').map_or(0, |at| at + 1);
                        self.text.truncate(start);
                    }
                    self.text.push(c);
                }
                _ => {}
            }
        }
        if self.text.len() > KEEP {
            let mut cut = self.text.len() - KEEP;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.dropped += cut as u64;
        }
    }

    /// Bytes shown so far, those dropped included.
    pub fn total(&self) -> u64 {
        self.dropped + self.text.len() as u64
    }

    /// The last [`LIVE_TAIL`] bytes, from a character boundary.
    pub fn tail(&self) -> &str {
        let mut start = self.text.len().saturating_sub(LIVE_TAIL);
        while !self.text.is_char_boundary(start) {
            start += 1;
        }
        &self.text[start..]
    }

    /// What it kept, saying first how much came before.
    pub fn kept(&self) -> String {
        if self.dropped == 0 {
            self.text.clone()
        } else {
            format!(
                "[{} earlier bytes were not kept]\n{}",
                self.dropped, self.text
            )
        }
    }
}

#[derive(Debug)]
struct Term {
    /// The ACP tool call it belongs to.
    tool: String,
    screen: Screen,
    exited: bool,
    exit_code: Option<i32>,
    /// When its output last went out, and whether more has come since.
    shown: Option<Instant>,
    dirty: bool,
    /// Order of arrival, to let the oldest ended one go first.
    order: u64,
}

/// One entry of a tool call's content: a terminal, or anything else, kept as the agent sent it.
#[derive(Debug, Clone, PartialEq)]
enum Slot {
    Terminal(String),
    Other(ToolContent),
}

/// The display terminals of a session's tool calls.
#[derive(Debug, Default)]
pub struct Terminals {
    by_id: HashMap<String, Term>,
    tools: HashMap<String, Vec<Slot>>,
    next: u64,
}

impl Terminals {
    fn ensure(&mut self, id: &str, tool: &str) -> bool {
        if let Some(term) = self.by_id.get_mut(id) {
            tool.clone_into(&mut term.tool);
            return true;
        }
        if self.by_id.len() >= MAX_TERMINALS {
            let oldest = self
                .by_id
                .iter()
                .filter(|(_, term)| term.exited)
                .min_by_key(|(_, term)| term.order)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(oldest) => {
                    self.by_id.remove(&oldest);
                }
                None => return false,
            }
        }
        self.next += 1;
        self.by_id.insert(
            id.to_owned(),
            Term {
                tool: tool.to_owned(),
                screen: Screen::default(),
                exited: false,
                exit_code: None,
                shown: None,
                dirty: false,
                order: self.next,
            },
        );
        true
    }

    fn render(&self, tool: &str) -> Option<Vec<ToolContent>> {
        let slots = self.tools.get(tool)?;
        Some(
            slots
                .iter()
                .map(|slot| match slot {
                    Slot::Other(content) => content.clone(),
                    Slot::Terminal(id) => match self.by_id.get(id) {
                        Some(term) if term.exited => ToolContent::Terminal {
                            output: term.screen.kept(),
                            exit_code: term.exit_code,
                            truncated: None,
                        },
                        Some(term) => {
                            let tail = term.screen.tail();
                            let total = term.screen.total();
                            ToolContent::Terminal {
                                output: tail.to_owned(),
                                exit_code: None,
                                truncated: (total > tail.len() as u64).then_some(Truncation {
                                    total_bytes: total,
                                    blob: None,
                                }),
                            }
                        }
                        None => ToolContent::Terminal {
                            output: String::new(),
                            exit_code: None,
                            truncated: None,
                        },
                    },
                })
                .collect(),
        )
    }

    /// A tool call's content, where `terminals` names the entries that are terminals (by their
    /// position): it is remembered, and each terminal shows its output so far.
    pub fn content(
        &mut self,
        tool: &str,
        content: Vec<ToolContent>,
        terminals: &[(usize, String)],
    ) -> Vec<ToolContent> {
        if terminals.is_empty() && !self.tools.contains_key(tool) {
            return content;
        }
        let slots = content
            .into_iter()
            .enumerate()
            .map(
                |(at, entry)| match terminals.iter().find(|(position, _)| *position == at) {
                    Some((_, id)) if self.ensure(id, tool) => Slot::Terminal(id.clone()),
                    _ => Slot::Other(entry),
                },
            )
            .collect();
        self.tools.insert(tool.to_owned(), slots);
        self.render(tool).unwrap_or_default()
    }

    /// Applies an update's terminal `_meta`. Returns the tool's content when it should show now:
    /// at once for an exit or the first output, otherwise at most every [`FLUSH`].
    pub fn apply(
        &mut self,
        tool: &str,
        metas: Vec<TerminalMeta>,
        now: Instant,
    ) -> Option<Vec<ToolContent>> {
        let mut show = false;
        for meta in metas {
            match meta {
                TerminalMeta::Info { id } => {
                    if self.ensure(&id, tool) {
                        let slots = self.tools.entry(tool.to_owned()).or_default();
                        if !slots.contains(&Slot::Terminal(id.clone())) {
                            slots.push(Slot::Terminal(id));
                            show = true;
                        }
                    }
                }
                TerminalMeta::Output { id, data } => {
                    if !self.ensure(&id, tool) {
                        continue;
                    }
                    let slots = self.tools.entry(tool.to_owned()).or_default();
                    if !slots.contains(&Slot::Terminal(id.clone())) {
                        slots.push(Slot::Terminal(id.clone()));
                    }
                    let Some(term) = self.by_id.get_mut(&id) else {
                        continue;
                    };
                    if term.exited {
                        continue;
                    }
                    term.screen.feed(&data);
                    if term.shown.is_none_or(|at| now >= at + FLUSH) {
                        term.shown = Some(now);
                        term.dirty = false;
                        show = true;
                    } else {
                        term.dirty = true;
                    }
                }
                TerminalMeta::Exit { id, exit_code } => {
                    if !self.ensure(&id, tool) {
                        continue;
                    }
                    let slots = self.tools.entry(tool.to_owned()).or_default();
                    if !slots.contains(&Slot::Terminal(id.clone())) {
                        slots.push(Slot::Terminal(id.clone()));
                    }
                    if let Some(term) = self.by_id.get_mut(&id) {
                        term.exited = true;
                        term.exit_code = exit_code;
                        term.dirty = false;
                        show = true;
                    }
                }
            }
        }
        if show { self.render(tool) } else { None }
    }

    /// When a running terminal's held output is due to show.
    pub fn due(&self) -> Option<Instant> {
        self.by_id
            .values()
            .filter(|term| term.dirty && !term.exited)
            .filter_map(|term| term.shown.map(|at| at + FLUSH))
            .min()
    }

    /// The tools whose terminals' held output is due, with their content.
    pub fn flush(&mut self, now: Instant) -> Vec<(String, Vec<ToolContent>)> {
        let mut tools: Vec<String> = Vec::new();
        for term in self.by_id.values_mut() {
            if term.dirty && !term.exited && term.shown.is_some_and(|at| now >= at + FLUSH) {
                term.dirty = false;
                term.shown = Some(now);
                if !tools.contains(&term.tool) {
                    tools.push(term.tool.clone());
                }
            }
        }
        tools
            .into_iter()
            .filter_map(|tool| self.render(&tool).map(|content| (tool, content)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn screen(chunks: &[&str]) -> Screen {
        let mut screen = Screen::default();
        for chunk in chunks {
            screen.feed(chunk);
        }
        screen
    }

    #[test]
    fn output_shows_as_a_terminal_would() {
        // Colours dropped, even split across chunks; CR LF a newline; a progress line rewritten.
        let shown = screen(&[
            "\u{1b}[32mok\u{1b}[0m line\r\n",
            "build \u{1b}[",
            "1;31mred\u{1b}]0;title\u{7} done\r\n",
            "progress 10%\rprogress 50%\r",
            "progress 100%\n",
            "bell\u{7} tab\tend",
        ]);
        assert_eq!(
            shown.kept(),
            "ok line\nbuild red done\nprogress 100%\nbell tab\tend"
        );
        // A CR at a chunk's end followed by LF in the next is still one newline.
        assert_eq!(screen(&["a\r", "\nb"]).kept(), "a\nb");
    }

    #[test]
    fn a_long_output_keeps_its_end_and_says_what_came_before() {
        let mut screen = Screen::default();
        let line = format!("{}\n", "é".repeat(100));
        for _ in 0..2000 {
            screen.feed(&line);
        }
        assert!(screen.text.len() <= KEEP);
        assert_eq!(screen.total(), (line.len() * 2000) as u64);
        assert!(screen.kept().starts_with(&format!(
            "[{} earlier bytes were not kept]\n",
            screen.dropped
        )));
        assert!(screen.tail().len() <= LIVE_TAIL && screen.tail().ends_with('\n'));
    }

    #[test]
    fn meta_reads_codex_acps_terminal_fields() {
        assert_eq!(
            meta(&json!({"_meta": {
                "terminal_info": {"terminal_id": "t1", "cwd": "/w"},
                "terminal_output": {"terminal_id": "t1", "data": "hi\n"},
                "terminal_exit": {"terminal_id": "t1", "exit_code": 2, "signal": null}
            }})),
            [
                TerminalMeta::Info { id: "t1".into() },
                TerminalMeta::Output {
                    id: "t1".into(),
                    data: "hi\n".into()
                },
                TerminalMeta::Exit {
                    id: "t1".into(),
                    exit_code: Some(2)
                },
            ]
        );
        assert!(meta(&json!({"_meta": {"terminal_output": {"data": "no id"}}})).is_empty());
        assert!(meta(&json!({})).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn live_output_shows_at_most_every_flush_and_the_end_at_once() {
        let mut terms = Terminals::default();
        let content = terms.content(
            "call",
            vec![
                ToolContent::Text {
                    text: "kept".into(),
                    truncated: None,
                },
                ToolContent::Terminal {
                    output: String::new(),
                    exit_code: None,
                    truncated: None,
                },
            ],
            &[(1, "t1".into())],
        );
        assert_eq!(content.len(), 2);
        let start = Instant::now();
        let output = |data: &str| TerminalMeta::Output {
            id: "t1".into(),
            data: data.into(),
        };
        // The first output shows at once; the next is held until FLUSH has passed.
        let first = terms.apply("call", vec![output("one\n")], start).unwrap();
        assert!(matches!(&first[1], ToolContent::Terminal { output, .. } if output == "one\n"));
        assert!(matches!(&first[0], ToolContent::Text { text, .. } if text == "kept"));
        assert_eq!(terms.apply("call", vec![output("two\n")], start), None);
        assert_eq!(terms.due(), Some(start + FLUSH));
        assert!(terms.flush(start + Duration::from_millis(50)).is_empty());
        let flushed = terms.flush(start + FLUSH);
        assert_eq!(flushed.len(), 1);
        assert!(
            matches!(&flushed[0].1[1], ToolContent::Terminal { output, .. } if output == "one\ntwo\n")
        );
        assert_eq!(terms.due(), None);
        // The exit shows at once, with its code and everything kept.
        terms.apply("call", vec![output("three\n")], start + FLUSH);
        let end = terms
            .apply(
                "call",
                vec![TerminalMeta::Exit {
                    id: "t1".into(),
                    exit_code: Some(0),
                }],
                start + FLUSH,
            )
            .unwrap();
        assert_eq!(
            end[1],
            ToolContent::Terminal {
                output: "one\ntwo\nthree\n".into(),
                exit_code: Some(0),
                truncated: None
            }
        );
        assert_eq!(terms.due(), None);
    }
}
