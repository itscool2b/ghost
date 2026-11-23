//! The tools a current relay lists (`apps/relay/src/tools.json`). They do what the first tools
//! did (`super`) in fewer calls and fewer words, because a bot pays for both: every tool's
//! description is read again on each step of the bot's own turn, and so is every answer it has
//! been given. So there are eleven tools where there were twenty-one, one call sends a message
//! and waits for its result, a wait that times out says how far the turn is in two lines, and a
//! reading is short unless more is asked for.

use std::{sync::Arc, time::Duration};

use o7_protocol::{SessionStatus, UiAnswer, UiPrompt, UiQuestion, UiQuestionAnswer};
use serde::Deserialize;
use serde_json::Value;

use super::{Answer, MAX_DIFF, MAX_MEMORY, act, arguments, count, find, message};
use crate::{
    backend::{Action, Backend, ChatOptions, Choice, Outcome, Statuses},
    render::{self, Limits, Waiting},
    wire::Overview,
};

/// The tools every bot link has.
pub const CHAT_TOOLS: [&str; 8] = [
    "inbox", "chats", "projects", "read", "send", "wait", "answer", "stop",
];
/// The tools only a full-control link has.
pub const FULL_TOOLS: [&str; 4] = ["set", "changes", "memory", "new_project"];
/// What `send` takes only from a full-control link: they choose how the agent runs.
const FULL_SEND: &str = "This link has the chat tools only, so it cannot choose a chat's model, thinking or access. Its owner can create a full control link in o7: Settings, Remote access.";

const PAGE: usize = 30;
/// How long a call waits for the turn when the bot does not say: under the shortest time an
/// assistant is known to give a tool before it gives up on it (a minute).
const WAIT_SENT: u64 = 45;
/// The longest a call waits. Each time a wait runs out the bot takes another step of its own,
/// reading everything it holds again, so a bot whose tool timeout allows it waits minutes at a
/// time. The relay gives a call a little longer than this (`BOT_CALL_MS`). It was 240 until
/// 23 November 2026: one assistant ends a tool call at 240 seconds with the network's time in
/// it, so a wait of exactly that long was lost there every time.
const WAIT_MAX: u64 = 200;

/// How many chats each part of `inbox` reads into: every one costs the bot what it says.
const INBOX_ASKING: usize = 6;
const INBOX_DONE: usize = 6;
/// A chat that finished longer ago than this is not news.
const INBOX_RECENT_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chats {
    query: Option<String>,
    page: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Nothing {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    chat: String,
    turns: Option<u8>,
    full: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    chat: Option<String>,
    folder: Option<String>,
    agent: Option<String>,
    text: String,
    wait: Option<u64>,
    model: Option<String>,
    thinking: Option<String>,
    access: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    chat: String,
    seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    chat: String,
    request: String,
    option: Option<String>,
    text: Option<String>,
    wait: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatOnly {
    chat: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    chat: String,
    model: Option<String>,
    thinking: Option<String>,
    access: Option<String>,
    agent: Option<String>,
    title: Option<String>,
    archived: Option<bool>,
    start: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    chat: String,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    project: String,
    markdown: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    path: String,
    name: Option<String>,
}

fn access_level(access: &str) -> Result<(), Answer> {
    if matches!(access, "ask" | "auto_edit" | "full_auto") {
        Ok(())
    } else {
        Err(Answer::failed(
            "access is one of ask, auto_edit and full_auto.",
        ))
    }
}

/// Runs one of the current tools. `full` is the link's level: the caller has refused a tool
/// above it, and what `send` takes only from a full link is refused here.
pub(super) async fn run(
    backend: &Arc<dyn Backend>,
    owner: &str,
    tool: &str,
    args: &Value,
    full: bool,
) -> Result<Answer, Answer> {
    // Which chat the call is about, where its arguments name one (`send` says so itself: it may
    // make the chat). An answer about a chat tells the link's bot how that chat stands.
    let chat = match tool {
        "read" | "wait" | "answer" | "stop" => args.get("chat").and_then(Value::as_str),
        _ => None,
    };
    let result = run_tool(backend, owner, tool, args, full).await;
    match (chat, result) {
        (Some(chat), Ok(answer)) => Ok(answer.about(chat, tool == "answer")),
        (_, result) => result,
    }
}

async fn run_tool(
    backend: &Arc<dyn Backend>,
    owner: &str,
    tool: &str,
    args: &Value,
    full: bool,
) -> Result<Answer, Answer> {
    match tool {
        "chats" => {
            let args: Chats = arguments(args)?;
            Ok(Answer::ok(chats(
                &backend.overview().await?,
                args.query.as_deref(),
                args.page.unwrap_or(1).max(1) as usize,
            )))
        }
        "inbox" => {
            arguments::<Nothing>(args)?;
            Ok(Answer::ok(inbox(backend).await?))
        }
        "projects" => {
            arguments::<Nothing>(args)?;
            Ok(Answer::ok(projects(&backend.overview().await?)))
        }
        "read" => {
            let args: Read = arguments(args)?;
            let turns = usize::from(args.turns.unwrap_or(1).clamp(1, 10));
            let limits = if args.full == Some(true) {
                Limits::WIDE
            } else {
                Limits::LEAN
            };
            Ok(Answer::ok(read(backend, &args.chat, turns, &limits).await?))
        }
        "send" => send(backend, owner, arguments(args)?, full).await,
        "wait" => {
            let args: Wait = arguments(args)?;
            let seconds = args.seconds.unwrap_or(WAIT_SENT).clamp(1, WAIT_MAX);
            Ok(Answer::ok(
                wait(backend, owner, &args.chat, seconds, false).await?,
            ))
        }
        "answer" => answer(backend, owner, arguments(args)?).await,
        "stop" => {
            let args: ChatOnly = arguments(args)?;
            backend.interrupt(&args.chat).await?;
            Ok(Answer::ok("Stopping the turn."))
        }
        "set" => set(backend, arguments(args)?).await,
        "changes" => {
            let args: Changes = arguments(args)?;
            match args.path {
                None => act(backend, Action::Changes { chat_id: args.chat }).await,
                Some(path) => {
                    match backend
                        .act(Action::Diff {
                            chat_id: args.chat,
                            path,
                        })
                        .await?
                    {
                        Outcome::Diff { binary: true, .. } => {
                            Ok(Answer::ok("That file is binary: there is nothing to show."))
                        }
                        Outcome::Diff {
                            before,
                            after,
                            truncated,
                            ..
                        } => Ok(Answer::ok(format!(
                            "{}{}",
                            render::change(before.as_deref(), after.as_deref(), MAX_DIFF),
                            if truncated {
                                "The file is larger than what is kept; this is from its start.\n"
                            } else {
                                ""
                            }
                        ))),
                        other => Ok(Answer::ok(super::outcome(other))),
                    }
                }
            }
        }
        "memory" => {
            let args: Memory = arguments(args)?;
            match args.markdown {
                None => {
                    act(
                        backend,
                        Action::ReadMemory {
                            project_id: args.project,
                        },
                    )
                    .await
                }
                Some(markdown) => {
                    if markdown.len() > MAX_MEMORY {
                        return Err(Answer::failed(
                            "Project memory holds at most 4 KiB here, because every chat receives it. Shorten it.",
                        ));
                    }
                    act(
                        backend,
                        Action::WriteMemory {
                            project_id: args.project,
                            markdown,
                        },
                    )
                    .await
                }
            }
        }
        "new_project" => {
            let args: NewProject = arguments(args)?;
            if args.path.trim().is_empty() {
                return Err(Answer::failed(
                    "Give the folder's full path on the computer.",
                ));
            }
            match backend
                .act(Action::CreateProject {
                    path: args.path,
                    name: args.name.filter(|name| !name.trim().is_empty()),
                })
                .await?
            {
                Outcome::Project {
                    id,
                    name,
                    folder_id,
                } => Ok(Answer::ok(match folder_id {
                    Some(folder) => {
                        format!("Created project {id} \"{name}\". Its folder is {folder}.")
                    }
                    None => {
                        format!("Created project {id} \"{name}\". Call projects for its folder.")
                    }
                })),
                other => Ok(Answer::ok(super::outcome(other))),
            }
        }
        _ => Err(Answer::failed("There is no such tool.")),
    }
}

/// Sends a message to a chat, or starts one in a folder with it, and waits for the turn.
async fn send(
    backend: &Arc<dyn Backend>,
    owner: &str,
    mut args: Send,
    full: bool,
) -> Result<Answer, Answer> {
    message(&args.text)?;
    let chooses = args.model.is_some() || args.thinking.is_some() || args.access.is_some();
    if chooses && !full {
        return Err(Answer::failed(FULL_SEND));
    }
    if let Some(access) = &args.access {
        access_level(access)?;
    }
    let (chat_id, started) = match (args.chat.take(), args.folder.take()) {
        (Some(chat), None) => {
            if args.agent.is_some() {
                return Err(Answer::failed(
                    "agent is for a new chat (with folder). To hand a chat to another agent, use set.",
                ));
            }
            (chat, None)
        }
        (None, Some(folder)) => {
            let created = backend.create(&folder, args.agent.as_deref()).await?;
            (created.chat.id, Some(created.chat.agent_name))
        }
        _ => {
            return Err(Answer::failed(
                "Give chat (an existing chat) or folder (to start a new one), not both.",
            ));
        }
    };
    let touched = chat_id.clone();
    let result = sent(backend, owner, chat_id, started, args).await;
    // Whatever came of the message, the chat is this link's to hear about from here on.
    result.map(|answer| answer.about(&touched, true))
}

/// `send` once its chat exists: how the agent runs, the message, then the wait.
async fn sent(
    backend: &Arc<dyn Backend>,
    owner: &str,
    chat_id: String,
    started: Option<String>,
    args: Send,
) -> Result<Answer, Answer> {
    let begun = |text: String| match &started {
        Some(agent) => format!("Started chat {chat_id} with {agent}. {text}"),
        None => text,
    };
    // How the agent runs is settled before it is given anything to do. When that fails the
    // message is not sent: it would run with a model or an access level nobody asked for.
    for (action, what) in [
        (
            args.model.map(|model| Action::SetModel {
                chat_id: chat_id.clone(),
                model,
            }),
            "model",
        ),
        (
            args.thinking.map(|level| Action::SetThinking {
                chat_id: chat_id.clone(),
                level,
            }),
            "thinking level",
        ),
        (
            args.access.map(|access| Action::SetAccess {
                chat_id: chat_id.clone(),
                access,
            }),
            "access level",
        ),
    ] {
        let Some(action) = action else { continue };
        if let Err(error) = backend.act(action).await {
            return Err(Answer::failed(begun(format!(
                "Its {what} could not be set, so the message was not sent: {} Call set with this chat for what it offers, then send again.",
                error.message
            ))));
        }
    }
    let ack = match backend.send(&chat_id, args.text).await {
        Ok(ack) => ack,
        Err(error) => {
            return Err(Answer::failed(begun(format!(
                "The message was not sent: {}",
                error.message
            ))));
        }
    };
    let seconds = args.wait.unwrap_or(WAIT_SENT).min(WAIT_MAX);
    if ack.queued {
        return Ok(Answer::ok(begun(format!(
            "Queued in chat {chat_id}: the agent is working and gets this when its turn ends. Call wait."
        ))));
    }
    if seconds == 0 {
        return Ok(Answer::ok(begun(format!(
            "Sent to chat {chat_id}. Call wait for the result."
        ))));
    }
    match wait(backend, owner, &chat_id, seconds, true).await {
        Ok(text) => Ok(Answer::ok(begun(text))),
        // The message is with the agent; only the reading failed.
        Err(answer) => Ok(Answer::ok(begun(format!(
            "Sent to chat {chat_id}, but its result could not be read: {} Call wait.",
            answer.text
        )))),
    }
}

/// Answers a permission request with one of its options or a question with words, or declines
/// either, then waits for the turn to go on.
async fn answer(backend: &Arc<dyn Backend>, owner: &str, args: Reply) -> Result<Answer, Answer> {
    let said = match (args.option, args.text) {
        (Some(_), Some(_)) => {
            return Err(Answer::failed(
                "Give option (a permission request's option id) or text (a question's answer), not both.",
            ));
        }
        (Some(option), None) => {
            backend
                .permission(&args.chat, &args.request, Some(option))
                .await?;
            "Answered."
        }
        (None, Some(text)) => {
            if text.trim().is_empty() {
                return Err(Answer::failed("The answer is empty."));
            }
            let page = backend.history(&args.chat, u32::MAX).await?;
            let Some(Waiting::Question(request)) = render::request(&page.items, &args.request)
            else {
                return Err(Answer::failed(
                    "That request is not a question waiting for words. A permission request takes option; call read for what waits.",
                ));
            };
            let reply = in_words(&request.prompt, text.trim())?;
            backend
                .answer(&args.chat, &args.request, Some(reply))
                .await?;
            "Answered."
        }
        (None, None) => {
            // Without an answer the request is declined, whichever kind it is.
            match backend.permission(&args.chat, &args.request, None).await {
                Err(error) if error.not_pending() => {
                    backend.answer(&args.chat, &args.request, None).await?;
                }
                other => other?,
            }
            "Declined."
        }
    };
    let seconds = args.wait.unwrap_or(WAIT_SENT).min(WAIT_MAX);
    if seconds == 0 {
        return Ok(Answer::ok(format!("{said} Call wait for the result.")));
    }
    Ok(Answer::ok(
        match wait(backend, owner, &args.chat, seconds, true).await {
            Ok(text) => format!("{said} {text}"),
            Err(_) => format!("{said} Call wait for the result."),
        },
    ))
}

/// A question's answer from a bot's words: the choice they name, or the words themselves where
/// the question takes any.
fn in_words(prompt: &UiPrompt, text: &str) -> Result<UiAnswer, Answer> {
    let named = |options: &[&str]| {
        options
            .iter()
            .position(|option| option.trim().eq_ignore_ascii_case(text))
    };
    match prompt {
        UiPrompt::Input { .. } => Ok(UiAnswer::Input {
            text: text.to_owned(),
        }),
        UiPrompt::Confirm { .. } => match text.to_ascii_lowercase().as_str() {
            "yes" | "y" | "true" => Ok(UiAnswer::Confirm { yes: true }),
            "no" | "n" | "false" => Ok(UiAnswer::Confirm { yes: false }),
            _ => Err(Answer::failed("Answer this one with yes or no.")),
        },
        UiPrompt::Select { options, .. } => {
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            named(&options)
                .map(|index| UiAnswer::Select {
                    index: index as u32,
                })
                .ok_or_else(|| {
                    Answer::failed(format!(
                        "Answer with one of the choices, word for word: {}",
                        options.join("; ")
                    ))
                })
        }
        UiPrompt::Questions { questions, .. } => {
            // One line of the answer per question, in the order they were asked.
            let lines: Vec<&str> = if questions.len() == 1 {
                vec![text]
            } else {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect()
            };
            if lines.len() != questions.len() {
                return Err(Answer::failed(format!(
                    "There are {} here: answer each on its own line, in order.",
                    count(questions.len(), "question")
                )));
            }
            questions
                .iter()
                .zip(lines)
                .map(|(question, line)| one_answer(question, line))
                .collect::<Result<Vec<_>, _>>()
                .map(|answers| UiAnswer::Questions { answers })
        }
        UiPrompt::Form { .. } | UiPrompt::Link { .. } | UiPrompt::Notify { .. } => {
            Err(Answer::failed(
                "This request needs its owner at the computer. Decline it (answer with no option and no text) or leave it for them.",
            ))
        }
    }
}

fn one_answer(question: &UiQuestion, line: &str) -> Result<UiQuestionAnswer, Answer> {
    let chosen = question
        .options
        .iter()
        .position(|option| option.label.trim().eq_ignore_ascii_case(line));
    match chosen {
        Some(index) => Ok(UiQuestionAnswer {
            question_id: question.id.clone(),
            selected: vec![index as u32],
            text: None,
        }),
        None if question.allow_free_text || question.options.is_empty() => Ok(UiQuestionAnswer {
            question_id: question.id.clone(),
            selected: Vec::new(),
            text: Some(line.to_owned()),
        }),
        None => Err(Answer::failed(format!(
            "\"{}\" takes one of its choices, word for word: {}",
            question.question,
            question
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ))),
    }
}

/// Changes a chat, one thing after another, and says what each came to. With nothing to change
/// it lists what the chat offers.
async fn set(backend: &Arc<dyn Backend>, args: Set) -> Result<Answer, Answer> {
    if let Some(access) = &args.access {
        access_level(access)?;
    }
    if args
        .title
        .as_ref()
        .is_some_and(|title| title.trim().is_empty())
    {
        return Err(Answer::failed("The title is empty."));
    }
    let chat = args.chat;
    let id = || chat.clone();
    let changes: Vec<Action> = [
        args.title.map(|title| Action::Rename {
            chat_id: id(),
            title,
        }),
        args.model.map(|model| Action::SetModel {
            chat_id: id(),
            model,
        }),
        args.thinking.map(|level| Action::SetThinking {
            chat_id: id(),
            level,
        }),
        args.access.map(|access| Action::SetAccess {
            chat_id: id(),
            access,
        }),
        (args.start == Some(true)).then(|| Action::StartAgent { chat_id: id() }),
        args.archived.map(|archived| Action::Archive {
            chat_id: id(),
            archived,
        }),
        // Last: a handoff leaves the bot with another chat, which the rest was not meant for.
        args.agent.map(|agent_id| Action::SwitchAgent {
            chat_id: id(),
            agent_id,
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    if changes.is_empty() {
        return Ok(Answer::ok(
            match backend.act(Action::Options { chat_id: chat }).await? {
                Outcome::Options(options) => offered(&options),
                other => super::outcome(other),
            },
        ));
    }
    let mut done: Vec<String> = Vec::new();
    for action in changes {
        match backend.act(action).await {
            Ok(Outcome::Chat { id }) => done.push(format!(
                "The chat to use from here is {id}; its next message carries the handoff."
            )),
            Ok(outcome) => done.push(super::outcome(outcome)),
            Err(error) => {
                done.push(format!("Stopped there: {}", error.message));
                return Err(Answer::failed(done.join(" ")));
            }
        }
    }
    Ok(Answer::ok(done.join(" ")))
}

/// What a chat offers, by the values `set` and `send` take, with the current one marked.
fn offered(options: &ChatOptions) -> String {
    let list = |name: &str, choices: &[Choice], current: &Option<String>| {
        if choices.is_empty() {
            return format!("{name}: no choice.\n");
        }
        let values: Vec<String> = choices
            .iter()
            .take(40)
            .map(|choice| {
                if current.as_deref() == Some(choice.value.as_str()) {
                    format!("{} (current)", choice.value)
                } else {
                    choice.value.clone()
                }
            })
            .collect();
        format!("{name}: {}\n", values.join(", "))
    };
    format!(
        "{}{}{}",
        list("model", &options.models, &options.model),
        list("thinking", &options.thinking, &options.thinking_level),
        list("access", &options.access, &options.access_mode),
    )
}

async fn read(
    backend: &Arc<dyn Backend>,
    chat_id: &str,
    turns: usize,
    limits: &Limits,
) -> Result<String, Answer> {
    let overview = backend.overview().await?;
    let chat = find(&overview, chat_id)?;
    let page = backend.history(chat_id, u32::MAX).await?;
    Ok(render::chat_with(chat, &page.items, turns, limits))
}

/// Waits until the chat is between turns or asks for something, then reads its last turn. A wait
/// that runs out says how far the turn is and no more: the bot asks again, and what it was told
/// stays in what it reads on every later step.
/// How long a chat that was just sent a message or an answer may still look as it did before.
/// The computer acknowledges either a moment before the chat's state says so; a look in that
/// moment found the chat "stopped" or still "waiting for an answer" and reported that as the
/// turn's result.
const MOVE_GRACE: Duration = Duration::from_secs(3);

/// `acted`: this call just sent the chat a message or an answer, so the chat is about to move.
async fn wait(
    backend: &Arc<dyn Backend>,
    owner: &str,
    chat_id: &str,
    seconds: u64,
    acted: bool,
) -> Result<String, Answer> {
    let result = waited(backend, owner, chat_id, Duration::from_secs(seconds), acted).await;
    backend.release(owner);
    result
}

async fn waited(
    backend: &Arc<dyn Backend>,
    owner: &str,
    chat_id: &str,
    timeout: Duration,
    acted: bool,
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
    // Until the chat is seen working, or its state changes, or the grace runs out.
    let mut unmoved = acted.then(|| tokio::time::Instant::now() + MOVE_GRACE.min(timeout));
    let timed_out = loop {
        let overview = backend.overview().await?;
        if render::settled(find(&overview, chat_id)?) {
            let Some(until) = unmoved.filter(|until| tokio::time::Instant::now() < *until) else {
                break false;
            };
            match tokio::time::timeout_at(until, changes.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break false,
                Err(_) => unmoved = None,
            }
            continue;
        }
        unmoved = None;
        match tokio::time::timeout_at(deadline, changes.changed()).await {
            Ok(Ok(())) => {}
            // The watch ended (another profile opened): report what is known now.
            Ok(Err(_)) => break false,
            Err(_) => break true,
        }
    };
    if !timed_out {
        return read(backend, chat_id, 1, &Limits::LEAN).await;
    }
    let overview = backend.overview().await?;
    let chat = find(&overview, chat_id)?;
    let page = backend.history(chat_id, u32::MAX).await?;
    Ok(format!(
        "{} Not done after {} seconds: call wait again.",
        render::progress(chat, &page.items),
        timeout.as_secs()
    ))
}

/// What needs the bot or its person across every open chat, in one reading: the chats asking for
/// an answer with what each asks, the chats still working, and the chats that finished in the
/// last day with what each came to. A bot that is woken, or comes back after a while, starts
/// here instead of listing the chats and reading them one by one.
async fn inbox(backend: &Arc<dyn Backend>) -> Result<String, Answer> {
    let overview = backend.overview().await?;
    let project = |id: &str| {
        overview
            .projects
            .iter()
            .find(|project| project.id == id)
            .map_or("no project", |project| project.name.as_str())
    };
    let line = |entry: &o7_protocol::workspace::WorkspaceChat| {
        format!(
            "{} \"{}\" ({}, {})",
            entry.chat.id,
            render::title(&entry.chat),
            project(&entry.project_id),
            entry.chat.agent_name
        )
    };
    let open: Vec<_> = overview
        .chats
        .iter()
        .filter(|entry| !entry.archived)
        .collect();
    let busy = |status: SessionStatus| {
        matches!(
            status,
            SessionStatus::Running | SessionStatus::Starting | SessionStatus::Cancelling
        )
    };
    let asking: Vec<_> = open
        .iter()
        .filter(|entry| entry.chat.needs_attention)
        .collect();
    let working: Vec<_> = open
        .iter()
        .filter(|entry| !entry.chat.needs_attention && busy(entry.chat.status))
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    let mut done: Vec<_> = open
        .iter()
        .filter(|entry| {
            !entry.chat.needs_attention
                && !busy(entry.chat.status)
                && now.saturating_sub(entry.chat.updated_at) <= INBOX_RECENT_MS
        })
        .collect();
    done.sort_by_key(|entry| std::cmp::Reverse(entry.chat.updated_at));
    if asking.is_empty() && working.is_empty() && done.is_empty() {
        return Ok(
            "Nothing needs you: no chat is asking, working or finished in the last day.".into(),
        );
    }
    let mut out = String::new();
    if !asking.is_empty() {
        out.push_str(&format!("Asking ({}):\n", asking.len()));
        for entry in asking.iter().take(INBOX_ASKING) {
            let page = backend.history(&entry.chat.id, u32::MAX).await?;
            out.push_str(&format!("{}\n", line(entry)));
            let asks = render::asking(&page.items);
            if asks.is_empty() {
                out.push_str("  It waits for an answer; call read for what it asks.\n");
            } else {
                out.push_str(&asks);
            }
        }
        if asking.len() > INBOX_ASKING {
            out.push_str(&format!(
                "and {} more; chats lists them.\n",
                asking.len() - INBOX_ASKING
            ));
        }
    }
    if !working.is_empty() {
        out.push_str(&format!("\nWorking ({}):\n", working.len()));
        for entry in &working {
            out.push_str(&format!("{}\n", line(entry)));
        }
    }
    if !done.is_empty() {
        out.push_str(&format!("\nFinished in the last day ({}):\n", done.len()));
        for entry in done.iter().take(INBOX_DONE) {
            let page = backend.history(&entry.chat.id, u32::MAX).await?;
            out.push_str(&format!(
                "{}, {}: {}\n",
                line(entry),
                ago(now.saturating_sub(entry.chat.updated_at)),
                render::brief(&entry.chat, &page.items)
            ));
        }
        if done.len() > INBOX_DONE {
            out.push_str(&format!(
                "and {} more; chats lists them.\n",
                done.len() - INBOX_DONE
            ));
        }
    }
    Ok(out.trim_start().to_owned())
}

/// How long ago, in the largest unit that fits: "just now", "12 min ago", "3 h ago".
fn ago(millis: i64) -> String {
    let minutes = millis / 60_000;
    match minutes {
        ..=0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

/// One line a chat: its id, how it stands, its title, its project and its agent.
fn chats(overview: &Overview, query: Option<&str>, page: usize) -> String {
    let query = query.map(str::trim).filter(|query| !query.is_empty());
    let needle = query.map(str::to_lowercase);
    let project = |id: &str| {
        overview
            .projects
            .iter()
            .find(|project| project.id == id)
            .map_or("no project", |project| project.name.as_str())
    };
    let open: Vec<_> = overview
        .chats
        .iter()
        .filter(|entry| !entry.archived)
        .filter(|entry| {
            needle.as_ref().is_none_or(|needle| {
                [
                    render::title(&entry.chat),
                    project(&entry.project_id).to_owned(),
                    entry.chat.agent_name.clone(),
                ]
                .iter()
                .any(|field| field.to_lowercase().contains(needle))
            })
        })
        .collect();
    if open.is_empty() {
        return match query {
            Some(query) => format!("No open chat matches \"{query}\"."),
            None => "No chats are open. Start one with send and a folder from projects.".into(),
        };
    }
    let pages = open.len().div_ceil(PAGE);
    let page = page.min(pages);
    let mut out = format!(
        "{} on {}, newest first (id | status | title | project | agent):\n",
        count(open.len(), "chat"),
        overview.host_name
    );
    for entry in open.iter().skip((page - 1) * PAGE).take(PAGE) {
        out.push_str(&format!(
            "{} | {} | {} | {} | {}\n",
            entry.chat.id,
            render::status(&entry.chat),
            render::title(&entry.chat),
            project(&entry.project_id),
            entry.chat.agent_name
        ));
    }
    if pages > 1 {
        out.push_str(&format!("Page {page} of {pages}.\n"));
    }
    out
}

/// The projects with the ids `memory` takes, their folders with the ids `send` takes, and the
/// agents a new chat can start with.
fn projects(overview: &Overview) -> String {
    let mut out = String::new();
    if overview.projects.is_empty() {
        out.push_str("There are no projects yet.\n");
    }
    for project in &overview.projects {
        out.push_str(&format!("Project {} \"{}\"", project.id, project.name));
        if let Some(agent) = &project.default_agent {
            out.push_str(&format!(" (agent: {agent})"));
        }
        out.push('\n');
        for folder in &project.folders {
            out.push_str(&format!("  folder {} {}\n", folder.id, folder.display_name));
        }
    }
    let ready: Vec<&str> = overview
        .agents
        .iter()
        .filter(|agent| agent.available)
        .map(|agent| agent.id.as_str())
        .collect();
    out.push_str(&format!(
        "Agents ready: {}.",
        if ready.is_empty() {
            "none".to_owned()
        } else {
            ready.join(", ")
        }
    ));
    if let Some(agent) = &overview.default_agent {
        out.push_str(&format!(
            " New chats use {agent} unless the project names another."
        ));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use o7_protocol::UiQuestionOption;

    use super::*;

    fn question(id: &str, options: &[&str], free: bool) -> UiQuestion {
        UiQuestion {
            id: id.into(),
            header: String::new(),
            question: format!("Question {id}?"),
            options: options
                .iter()
                .map(|label| UiQuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                })
                .collect(),
            multi_select: false,
            allow_free_text: free,
        }
    }

    #[test]
    fn a_questions_answer_is_the_choice_its_words_name_or_the_words_where_any_are_taken() {
        let input = UiPrompt::Input {
            message: "Name?".into(),
            placeholder: None,
        };
        assert_eq!(
            in_words(&input, "main").unwrap(),
            UiAnswer::Input {
                text: "main".into()
            }
        );
        let confirm = UiPrompt::Confirm {
            message: "Sure?".into(),
        };
        assert_eq!(
            in_words(&confirm, "Yes").unwrap(),
            UiAnswer::Confirm { yes: true }
        );
        assert_eq!(
            in_words(&confirm, "no").unwrap(),
            UiAnswer::Confirm { yes: false }
        );
        assert!(in_words(&confirm, "maybe").unwrap_err().error);
        let select = UiPrompt::Select {
            message: "Which?".into(),
            options: vec!["Postgres".into(), "SQLite".into()],
        };
        assert_eq!(
            in_words(&select, "sqlite").unwrap(),
            UiAnswer::Select { index: 1 }
        );
        // A choice that is not offered is never guessed at.
        let refused = in_words(&select, "MySQL").unwrap_err();
        assert!(
            refused.text.contains("Postgres; SQLite"),
            "{}",
            refused.text
        );

        let questions = UiPrompt::Questions {
            message: "Two things".into(),
            questions: vec![
                question("a", &["Red", "Blue"], false),
                question("b", &["Small"], true),
            ],
        };
        assert_eq!(
            in_words(&questions, "blue\n\nAs large as it gets").unwrap(),
            UiAnswer::Questions {
                answers: vec![
                    UiQuestionAnswer {
                        question_id: "a".into(),
                        selected: vec![1],
                        text: None
                    },
                    UiQuestionAnswer {
                        question_id: "b".into(),
                        selected: Vec::new(),
                        text: Some("As large as it gets".into())
                    },
                ]
            }
        );
        assert!(
            in_words(&questions, "blue")
                .unwrap_err()
                .text
                .contains("2 questions")
        );
        // Words where only a choice is taken are refused, with the choices.
        assert!(
            in_words(&questions, "green\nSmall")
                .unwrap_err()
                .text
                .contains("Red; Blue")
        );
        // What needs its owner at the computer is not answered from afar.
        let link = UiPrompt::Link {
            message: "Sign in".into(),
            source: None,
            url: "https://example.com".into(),
        };
        assert!(in_words(&link, "yes").unwrap_err().error);
    }

    #[test]
    fn what_a_chat_offers_is_listed_by_value_with_the_current_one_marked() {
        let options = ChatOptions {
            models: vec![
                Choice {
                    value: "opus".into(),
                    name: "Opus".into(),
                },
                Choice {
                    value: "sonnet".into(),
                    name: "Sonnet".into(),
                },
            ],
            model: Some("sonnet".into()),
            thinking: Vec::new(),
            thinking_level: None,
            access: vec![Choice {
                value: "ask".into(),
                name: "Ask".into(),
            }],
            access_mode: None,
        };
        assert_eq!(
            offered(&options),
            "model: opus, sonnet (current)\nthinking: no choice.\naccess: ask\n"
        );
    }
}
