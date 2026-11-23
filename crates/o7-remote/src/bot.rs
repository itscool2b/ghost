//! The tools a linked bot calls. Each call is one answer in plain text; a failure is text too,
//! marked as an error, so the bot can read why and try something else.
//!
//! There are two sets. The current one is in [`lean`] and `apps/relay/src/tools.json` describes
//! it to the bot. The first one, here, is what a relay from before it lists
//! (`apps/relay/src/tools-first.json`): a copy of the app and the relay are updated apart, so
//! each answers the other's set.

mod lean;

use std::{sync::Arc, time::Duration};

use o7_protocol::session::ChatSummary;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    backend::{Action, Backend, BackendError, ChatOptions, Choice, Outcome, Statuses},
    render,
    wire::Overview,
};

/// The tools every bot link has: following chats and working in them.
pub const CHAT_TOOLS: [&str; 8] = [
    "list_chats",
    "list_projects",
    "read_chat",
    "send_message",
    "wait_for_chat",
    "respond_to_request",
    "stop_chat",
    "start_chat",
];

/// The tools only a full-control link has: the rest of what a person does with chats and
/// projects in the app's window.
pub const FULL_TOOLS: [&str; 13] = [
    "create_project",
    "chat_options",
    "set_model",
    "set_thinking",
    "set_access",
    "switch_agent",
    "rename_chat",
    "archive_chat",
    "start_agent",
    "list_changes",
    "read_change",
    "read_memory",
    "write_memory",
];

/// What a link without full control is told when it asks for more.
pub const NOT_ALLOWED: &str = "This link has the chat tools only. Its owner can create a full control link in o7: Settings, Remote access.";

/// The current tools every bot link has, and the ones only a full-control link has.
pub use lean::{CHAT_TOOLS as LEAN_CHAT_TOOLS, FULL_TOOLS as LEAN_FULL_TOOLS};

/// The set of tools this copy of the app answers, which it tells the relay with its bot links.
/// A relay that knows the number lists that set; one that does not lists the first.
pub const TOOLS_VERSION: u32 = 2;

fn chat_tool(tool: &str) -> bool {
    CHAT_TOOLS.contains(&tool) || lean::CHAT_TOOLS.contains(&tool)
}

fn full_tool(tool: &str) -> bool {
    FULL_TOOLS.contains(&tool) || lean::FULL_TOOLS.contains(&tool)
}

/// Whether `tool` is one the relay offers at all. A name outside is refused before anything runs.
pub fn known(tool: &str) -> bool {
    chat_tool(tool) || full_tool(tool)
}

/// Whether a link of this level may call `tool`.
pub fn allowed(tool: &str, full: bool) -> bool {
    chat_tool(tool) || (full && full_tool(tool))
}

/// The longest project memory a bot may write, in bytes: the app's own limit for the brief, which
/// is the part of memory a bot reads and writes (every chat of the project receives it).
const MAX_MEMORY: usize = 4 * 1024;
/// How much of one side of a change is shown.
const MAX_DIFF: usize = 24 * 1024;

const LISTED_CHATS: usize = 60;
/// The longest message a bot may send, in bytes.
const MAX_TEXT: usize = 64 * 1024;
const WAIT_DEFAULT: u64 = 30;
const WAIT_MAX: u64 = 45;

#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub error: bool,
}

impl Answer {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn failed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

impl From<BackendError> for Answer {
    fn from(error: BackendError) -> Self {
        Self::failed(error.message)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Nothing {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatOnly {
    chat_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    chat_id: String,
    turns: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    chat_id: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    chat_id: String,
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Respond {
    chat_id: String,
    request_id: String,
    option_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    folder_id: String,
    agent_id: Option<String>,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    path: String,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    chat_id: String,
    model: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Thinking {
    chat_id: String,
    level: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    chat_id: String,
    access: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    chat_id: String,
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename {
    chat_id: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    chat_id: String,
    archived: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    chat_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectOnly {
    project_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    project_id: String,
    markdown: String,
}

fn arguments<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, Answer> {
    // A bot may send no arguments at all for a tool that takes none.
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args.clone()
    };
    serde_json::from_value(args)
        .map_err(|error| Answer::failed(format!("The tool's arguments are not valid: {error}.")))
}

fn message(text: &str) -> Result<(), Answer> {
    if text.trim().is_empty() {
        Err(Answer::failed("The message is empty."))
    } else if text.len() > MAX_TEXT {
        Err(Answer::failed(
            "The message is too long. Keep it under 64 KiB.",
        ))
    } else {
        Ok(())
    }
}

/// Runs one tool call. `owner` is unique to the call: whatever it watches is released at the end.
/// `full` is the link's level; a tool above it is refused here too, whoever let the call through.
pub async fn call(
    backend: &Arc<dyn Backend>,
    owner: &str,
    tool: &str,
    args: &Value,
    full: bool,
) -> Answer {
    if known(tool) && !allowed(tool, full) {
        return Answer::failed(NOT_ALLOWED);
    }
    let answer = if lean::CHAT_TOOLS.contains(&tool) || lean::FULL_TOOLS.contains(&tool) {
        lean::run(backend, owner, tool, args, full).await
    } else {
        run(backend, owner, tool, args).await
    };
    match answer {
        Ok(answer) | Err(answer) => answer,
    }
}

/// One full-control action, answered in words.
async fn act(backend: &Arc<dyn Backend>, action: Action) -> Result<Answer, Answer> {
    Ok(Answer::ok(outcome(backend.act(action).await?)))
}

fn outcome(outcome: Outcome) -> String {
    match outcome {
        Outcome::Done(text) => text,
        Outcome::Project {
            id,
            name,
            folder_id,
        } => match folder_id {
            Some(folder) => format!(
                "Created project \"{name}\" ({id}). Its folder id is {folder}: start a chat there with start_chat."
            ),
            None => {
                format!("Created project \"{name}\" ({id}). Call list_projects for its folder.")
            }
        },
        Outcome::Chat { id } => format!(
            "Done. The chat to use from here is {id}. Send its next message with send_message."
        ),
        Outcome::Options(options) => chat_options(&options),
        Outcome::Changes { files, unavailable } => {
            if let Some(reason) = unavailable {
                return format!("The changes are not available: {reason}");
            }
            if files.is_empty() {
                return "The last turn changed no files.".into();
            }
            let mut text = format!("{} changed in the last turn:\n", count(files.len(), "file"));
            for (path, kind) in files.iter().take(200) {
                text.push_str(&format!("- {path} ({kind})\n"));
            }
            if files.len() > 200 {
                text.push_str(&format!("and {} more.\n", files.len() - 200));
            }
            text.push_str("Read one by its path.");
            text
        }
        Outcome::Diff {
            before,
            after,
            binary,
            truncated,
        } => {
            if binary {
                return "That file is binary: there is nothing to show.".into();
            }
            let side = |name: &str, text: Option<String>| match text {
                None => format!("{name}: the file did not exist.\n"),
                Some(text) => format!("{name}:\n{}\n", render::cut(&text, MAX_DIFF)),
            };
            format!(
                "{}{}{}",
                side("Before", before),
                side("After", after),
                if truncated {
                    "The file is larger than what is kept; this is its start.\n"
                } else {
                    ""
                }
            )
        }
        Outcome::Memory { markdown } => {
            if markdown.trim().is_empty() {
                "This project's memory is empty.".into()
            } else {
                format!("This project's memory:\n\n{markdown}")
            }
        }
    }
}

fn count(number: usize, noun: &str) -> String {
    format!("{number} {noun}{}", if number == 1 { "" } else { "s" })
}

fn chat_options(options: &ChatOptions) -> String {
    let list = |title: &str, tool: &str, choices: &[Choice], current: &Option<String>| {
        if choices.is_empty() {
            return format!("{title}: this agent offers no choice here.\n");
        }
        let mut text = format!("{title} (change with {tool}):\n");
        for choice in choices.iter().take(60) {
            let chosen = current.as_deref() == Some(choice.value.as_str());
            text.push_str(&format!(
                "- {}{}{}\n",
                choice.value,
                if choice.name == choice.value {
                    String::new()
                } else {
                    format!(" ({})", choice.name)
                },
                if chosen { " [current]" } else { "" }
            ));
        }
        text
    };
    format!(
        "{}{}{}",
        list("Models", "set_model", &options.models, &options.model),
        list(
            "Thinking levels",
            "set_thinking",
            &options.thinking,
            &options.thinking_level
        ),
        list(
            "Access",
            "set_access",
            &options.access,
            &options.access_mode
        ),
    )
}

async fn run(
    backend: &Arc<dyn Backend>,
    owner: &str,
    tool: &str,
    args: &Value,
) -> Result<Answer, Answer> {
    match tool {
        "list_chats" => {
            arguments::<Nothing>(args)?;
            Ok(Answer::ok(chats(&backend.overview().await?)))
        }
        "list_projects" => {
            arguments::<Nothing>(args)?;
            Ok(Answer::ok(projects(&backend.overview().await?)))
        }
        "read_chat" => {
            let args: Read = arguments(args)?;
            let turns = usize::from(args.turns.unwrap_or(2).clamp(1, 10));
            Ok(Answer::ok(read(backend, &args.chat_id, turns).await?))
        }
        "send_message" => {
            let args: Send = arguments(args)?;
            message(&args.text)?;
            let ack = backend.send(&args.chat_id, args.text).await?;
            Ok(Answer::ok(if ack.queued {
                "Queued: the agent is working, and gets this message when its turn ends. Call wait_for_chat for the result."
            } else {
                "Sent: the agent is working on it. Call wait_for_chat for the result."
            }))
        }
        "wait_for_chat" => {
            let args: Wait = arguments(args)?;
            let seconds = args
                .timeout_seconds
                .unwrap_or(WAIT_DEFAULT)
                .clamp(1, WAIT_MAX);
            let result = wait(backend, owner, &args.chat_id, Duration::from_secs(seconds)).await;
            backend.release(owner);
            Ok(Answer::ok(result?))
        }
        "respond_to_request" => {
            let args: Respond = arguments(args)?;
            match args.option_id {
                Some(option) => {
                    backend
                        .permission(&args.chat_id, &args.request_id, Some(option))
                        .await?;
                    Ok(Answer::ok(
                        "Answered. The agent continues; call wait_for_chat.",
                    ))
                }
                None => {
                    // Without an option the request is declined, whichever kind it is.
                    match backend
                        .permission(&args.chat_id, &args.request_id, None)
                        .await
                    {
                        Err(error) if error.not_pending() => {
                            backend
                                .answer(&args.chat_id, &args.request_id, None)
                                .await?;
                        }
                        other => other?,
                    }
                    Ok(Answer::ok(
                        "Declined. Send a message to tell the agent what to do instead.",
                    ))
                }
            }
        }
        "stop_chat" => {
            let args: ChatOnly = arguments(args)?;
            backend.interrupt(&args.chat_id).await?;
            Ok(Answer::ok("Stopping the turn."))
        }
        "start_chat" => {
            let args: Start = arguments(args)?;
            message(&args.text)?;
            let created = backend
                .create(&args.folder_id, args.agent_id.as_deref())
                .await?;
            let id = created.chat.id;
            match backend.send(&id, args.text).await {
                Ok(_) => Ok(Answer::ok(format!(
                    "Started chat {id} with {}. The agent is working on the message; call wait_for_chat with this chat id.",
                    created.chat.agent_name
                ))),
                Err(error) => Err(Answer::failed(format!(
                    "Chat {id} was created, but its first message was not sent: {} Send it with send_message.",
                    error.message
                ))),
            }
        }
        "create_project" => {
            let args: NewProject = arguments(args)?;
            if args.path.trim().is_empty() {
                return Err(Answer::failed(
                    "Give the folder's full path on the computer.",
                ));
            }
            act(
                backend,
                Action::CreateProject {
                    path: args.path,
                    name: args.name.filter(|name| !name.trim().is_empty()),
                },
            )
            .await
        }
        "chat_options" => {
            let args: ChatOnly = arguments(args)?;
            act(
                backend,
                Action::Options {
                    chat_id: args.chat_id,
                },
            )
            .await
        }
        "set_model" => {
            let args: Model = arguments(args)?;
            act(
                backend,
                Action::SetModel {
                    chat_id: args.chat_id,
                    model: args.model,
                },
            )
            .await
        }
        "set_thinking" => {
            let args: Thinking = arguments(args)?;
            act(
                backend,
                Action::SetThinking {
                    chat_id: args.chat_id,
                    level: args.level,
                },
            )
            .await
        }
        "set_access" => {
            let args: Access = arguments(args)?;
            if !matches!(args.access.as_str(), "ask" | "auto_edit" | "full_auto") {
                return Err(Answer::failed(
                    "access is one of ask, auto_edit and full_auto.",
                ));
            }
            act(
                backend,
                Action::SetAccess {
                    chat_id: args.chat_id,
                    access: args.access,
                },
            )
            .await
        }
        "switch_agent" => {
            let args: Switch = arguments(args)?;
            act(
                backend,
                Action::SwitchAgent {
                    chat_id: args.chat_id,
                    agent_id: args.agent_id,
                },
            )
            .await
        }
        "rename_chat" => {
            let args: Rename = arguments(args)?;
            if args.title.trim().is_empty() {
                return Err(Answer::failed("The title is empty."));
            }
            act(
                backend,
                Action::Rename {
                    chat_id: args.chat_id,
                    title: args.title,
                },
            )
            .await
        }
        "archive_chat" => {
            let args: Archive = arguments(args)?;
            act(
                backend,
                Action::Archive {
                    chat_id: args.chat_id,
                    archived: args.archived.unwrap_or(true),
                },
            )
            .await
        }
        "start_agent" => {
            let args: ChatOnly = arguments(args)?;
            act(
                backend,
                Action::StartAgent {
                    chat_id: args.chat_id,
                },
            )
            .await
        }
        "list_changes" => {
            let args: ChatOnly = arguments(args)?;
            act(
                backend,
                Action::Changes {
                    chat_id: args.chat_id,
                },
            )
            .await
        }
        "read_change" => {
            let args: Change = arguments(args)?;
            act(
                backend,
                Action::Diff {
                    chat_id: args.chat_id,
                    path: args.path,
                },
            )
            .await
        }
        "read_memory" => {
            let args: ProjectOnly = arguments(args)?;
            act(
                backend,
                Action::ReadMemory {
                    project_id: args.project_id,
                },
            )
            .await
        }
        "write_memory" => {
            let args: Memory = arguments(args)?;
            if args.markdown.len() > MAX_MEMORY {
                return Err(Answer::failed(
                    "Project memory holds at most 4 KiB here, because every chat receives it. Shorten it.",
                ));
            }
            act(
                backend,
                Action::WriteMemory {
                    project_id: args.project_id,
                    markdown: args.markdown,
                },
            )
            .await
        }
        _ => Err(Answer::failed("There is no such tool.")),
    }
}

fn find<'a>(overview: &'a Overview, chat_id: &str) -> Result<&'a ChatSummary, Answer> {
    overview
        .chats
        .iter()
        .map(|entry| &entry.chat)
        .find(|chat| chat.id == chat_id)
        .ok_or_else(|| {
            Answer::failed("There is no chat with that id. List the chats for their ids.")
        })
}

async fn read(backend: &Arc<dyn Backend>, chat_id: &str, turns: usize) -> Result<String, Answer> {
    let overview = backend.overview().await?;
    let chat = find(&overview, chat_id)?;
    let page = backend.history(chat_id, u32::MAX).await?;
    Ok(render::chat(chat, &page.items, turns))
}

/// Waits until the chat is between turns or asks for something, then reads its last turn.
async fn wait(
    backend: &Arc<dyn Backend>,
    owner: &str,
    chat_id: &str,
    timeout: Duration,
) -> Result<String, Answer> {
    // Watch before looking, so a change between the two is not missed.
    let (changed, mut changes) = tokio::sync::watch::channel(());
    let watched = chat_id.to_owned();
    let statuses: Statuses = Arc::new(move |update| {
        if update.chat_id == watched {
            let _ = changed.send(());
        }
        !changed.is_closed()
    });
    backend.watch(owner, statuses)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let timed_out = loop {
        let overview = backend.overview().await?;
        if render::settled(find(&overview, chat_id)?) {
            break false;
        }
        match tokio::time::timeout_at(deadline, changes.changed()).await {
            Ok(Ok(())) => {}
            // The watch ended (another profile opened): report what is known now.
            Ok(Err(_)) => break false,
            Err(_) => break true,
        }
    };
    let text = read(backend, chat_id, 1).await?;
    Ok(if timed_out {
        format!(
            "Still working after {} seconds. Call wait_for_chat again to keep waiting.\n\n{text}",
            timeout.as_secs()
        )
    } else {
        text
    })
}

fn chats(overview: &Overview) -> String {
    let open: Vec<_> = overview
        .chats
        .iter()
        .filter(|entry| !entry.archived)
        .collect();
    if open.is_empty() {
        return format!(
            "No chats are open on {}. Use list_projects and start_chat to start one.",
            overview.host_name
        );
    }
    let mut out = format!(
        "Chats on {}, most recently active first:\n",
        overview.host_name
    );
    for entry in open.iter().take(LISTED_CHATS) {
        let project = overview
            .projects
            .iter()
            .find(|project| project.id == entry.project_id)
            .map_or("no project", |project| project.name.as_str());
        out.push_str(&format!(
            "- {} \"{}\" in {} with {}: {}\n",
            entry.chat.id,
            render::title(&entry.chat),
            project,
            entry.chat.agent_name,
            render::status(&entry.chat)
        ));
    }
    if open.len() > LISTED_CHATS {
        out.push_str(&format!("And {} older chats.\n", open.len() - LISTED_CHATS));
    }
    out
}

fn projects(overview: &Overview) -> String {
    let mut out = String::new();
    if overview.projects.is_empty() {
        out.push_str("There are no projects yet. Its owner adds one in o7.\n");
    }
    for project in &overview.projects {
        out.push_str(&format!("Project \"{}\"", project.name));
        if let Some(agent) = &project.default_agent {
            out.push_str(&format!(" (usual agent: {agent})"));
        }
        out.push('\n');
        for folder in &project.folders {
            out.push_str(&format!(
                "- folder {}: {}\n",
                folder.id, folder.display_name
            ));
        }
    }
    out.push_str("\nAgents:\n");
    for agent in &overview.agents {
        out.push_str(&format!(
            "- {}: {}{}\n",
            agent.id,
            agent.name,
            if agent.available {
                ""
            } else {
                " (not ready on this computer)"
            }
        ));
    }
    if let Some(agent) = &overview.default_agent {
        out.push_str(&format!(
            "New chats use {agent} unless the project names another.\n"
        ));
    }
    out
}
