//! A stand-in desktop for trying remote sessions end to end without the app: the real connection
//! task and sealed channel, in front of a made-up project whose "agent" follows a script.
//!
//!     cargo run -p o7-remote --example demo_host -- ws://127.0.0.1:8799 http://localhost:1437 <folder> [<name>]
//!
//! `<name>` is the computer's name (default "Demo computer"), for a test that runs two side by side.
//!
//! It turns remote access on, prints `PAIR <link>` and `BOT <address>`, and prints a new pairing
//! link for every line read from standard input. Each message sent to a chat starts a turn that
//! reads a file, asks permission to run the tests, and finishes once that is answered.
//!
//! o7 for iPhone reaches it as it reaches a real computer, on the relay's app path: its Inbox
//! reads these chats (what asks, what works, what finished), and its Search finds them by title
//! and words. Everything else the app asks is answered too: access levels, titles, archiving and
//! the archived chats; a turn's and a branch's changes (one file) and their diffs; what a chat
//! stands on (a branch and a pull request whose checks passed); the project's brief, with
//! revisions; a picture of a chat's browser; tasks now (each on a branch of its own, reviewed by
//! the other agent) and for tonight; and photos in a message.
//!
//! Notifications too, as the real service sends them: a phone that turned them on
//! (`push_register`) is sent a sealed `asking`, with Allow once and Deny, a moment after a turn
//! asks for its permission, and `finished` when the turn ends. They go only on a socket whose
//! relay said it sends notifications (`f`), so an end-to-end test that mocks Apple at the relay
//! can open what it captured with the phone's key and answer from it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::{
    collections::HashMap,
    io::BufRead,
    sync::{Arc, Mutex},
    time::Duration,
};

use o7_protocol::{
    EventBatch, O7Event, PermissionDecision, PermissionOption, PermissionOptionKind,
    PermissionRequest, ResolvedBy, SessionStatus, StopReason, ToolCall, ToolKind, ToolStatus,
    UiAnswer,
    search::ChatSearchHit,
    session::{ChatAttach, ChatStatusUpdate, HistoryItem, HistoryPage, PromptAck},
    workspace::WorkspaceChat,
};
use o7_remote::{
    Action, Backend, BackendError, BoxFuture, ChatOptions, Choice, Config, Frames, Outcome,
    Overview, RemoteAgent, Service, Statuses,
};
use serde_json::json;

struct Chat {
    title: String,
    status: SessionStatus,
    needs_attention: bool,
    /// What happened in it, oldest first, each with when.
    events: Vec<(i64, O7Event)>,
    updated_at: i64,
    /// The permission request the turn waits on, and what wakes it.
    waiting: Option<(String, Arc<tokio::sync::Notify>)>,
    stop: Arc<tokio::sync::Notify>,
    turns: u32,
    /// The agent the chat runs on, and what the pickers set.
    agent: &'static str,
    model: String,
    thinking: String,
    /// Its access level, by the value `set_access` takes.
    access: String,
    archived: bool,
    /// The branch of its own it works on, in a worktree: the first chat's, and a task's.
    branch: Option<String>,
}

/// The demo computer's agents: two that can run and one that is not installed.
const AGENTS: [(&str, &str, bool); 3] = [
    ("demo", "Demo agent", true),
    ("other", "Other agent", true),
    ("absent", "Absent agent", false),
];
const MODELS: [(&str, &str); 2] = [("fast", "Fast"), ("smart", "Smart")];
const LEVELS: [(&str, &str); 2] = [("low", "Low"), ("high", "High")];

fn choices(list: &[(&str, &str)]) -> Vec<Choice> {
    list.iter()
        .map(|(value, name)| Choice {
            value: (*value).into(),
            name: (*name).into(),
        })
        .collect()
}

#[derive(Default)]
struct Demo {
    chats: Mutex<Vec<(String, Chat)>>,
    frames: Mutex<HashMap<String, (String, Frames)>>,
    statuses: Mutex<HashMap<String, Statuses>>,
    /// Each project's brief and the number of its revision.
    briefs: Mutex<HashMap<String, (String, u32)>>,
    /// Photos added to message boxes, by attachment id.
    attached: Mutex<Vec<(String, Attached)>>,
    /// The photos each turn's message carried, by turn.
    turn_images: Mutex<HashMap<String, Vec<String>>>,
}

/// A photo added to a chat's message box.
#[derive(Clone)]
struct Attached {
    chat: String,
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

/// The commit the demo's branches start from.
const BASE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const ACCESS: [(&str, &str); 3] = [
    ("ask", "Ask first"),
    ("auto_edit", "Edit files"),
    ("full_auto", "Full access"),
];
/// The demo's one change: the login form checks the email first.
const LOGIN_BEFORE: &str =
    "export function submit(email: string) {\n  post(email);\n  return true;\n}\n";
const LOGIN_AFTER: &str = "export function submit(email: string) {\n  if (!email.includes(\"@\")) {\n    setError(\"Enter your email address.\");\n    return false;\n  }\n  post(email);\n  return true;\n}\n";
const LOGIN_TEST: &str =
    "test(\"refuses an empty email\", () => {\n  expect(submit(\"\")).toBe(false);\n});\n";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl Demo {
    fn summary(id: &str, chat: &Chat) -> WorkspaceChat {
        serde_json::from_value(json!({
            "chat": {
                "id": id, "title": chat.title, "agent_id": chat.agent, "agent_host": { "type": "native" },
                "agent_name": AGENTS.iter().find(|(id, ..)| *id == chat.agent).map_or("Demo agent", |agent| agent.1),
                "folder": "/demo", "folder_state": "ok",
                "status": chat.status, "needs_attention": chat.needs_attention,
                "created_at": 0, "updated_at": chat.updated_at,
            },
            "project_id": "p1", "folder_id": "f1", "pinned": false, "archived": chat.archived,
        }))
        .expect("chat")
    }

    /// Tells whoever watches that a chat's title or place changed.
    fn changed(&self, chat_id: &str) {
        let (status, needs_attention) = {
            let chats = self.chats.lock().unwrap();
            let Some((_, chat)) = chats.iter().find(|(id, _)| id == chat_id) else {
                return;
            };
            (chat.status, chat.needs_attention)
        };
        self.status(chat_id, status, needs_attention);
    }

    /// Records events in a chat and sends them to whoever follows it.
    fn emit(&self, chat_id: &str, events: Vec<O7Event>) {
        let seq_start = {
            let mut chats = self.chats.lock().unwrap();
            let Some((_, chat)) = chats.iter_mut().find(|(id, _)| id == chat_id) else {
                return;
            };
            let start = chat.events.len() as u32;
            let at = now();
            chat.events
                .extend(events.iter().cloned().map(|event| (at, event)));
            chat.updated_at = at;
            start
        };
        let batch = EventBatch {
            chat_id: chat_id.to_owned(),
            seq_start,
            events,
        };
        self.frames
            .lock()
            .unwrap()
            .retain(|_, (following, frames)| following != chat_id || frames(&batch));
    }

    fn status(&self, chat_id: &str, status: SessionStatus, needs_attention: bool) {
        let update = {
            let mut chats = self.chats.lock().unwrap();
            let Some((_, chat)) = chats.iter_mut().find(|(id, _)| id == chat_id) else {
                return;
            };
            chat.status = status;
            chat.needs_attention = needs_attention;
            chat.updated_at = now();
            ChatStatusUpdate {
                chat_id: chat_id.to_owned(),
                status,
                needs_attention,
                title: Some(chat.title.clone()),
                updated_at: chat.updated_at,
            }
        };
        self.statuses
            .lock()
            .unwrap()
            .retain(|_, statuses| statuses(&update));
    }

    /// One scripted turn: read a file, ask to run the tests, finish when that is answered.
    /// `images` are the photos its message carried.
    async fn turn(self: Arc<Self>, chat_id: String, text: String, images: Vec<String>) {
        let (turn, stop) = {
            let mut chats = self.chats.lock().unwrap();
            let (_, chat) = chats.iter_mut().find(|(id, _)| *id == chat_id).unwrap();
            chat.turns += 1;
            (format!("{chat_id}-t{}", chat.turns), chat.stop.clone())
        };
        if !images.is_empty() {
            self.turn_images
                .lock()
                .unwrap()
                .insert(turn.clone(), images);
        }
        let request = format!("{turn}-p");
        let asked = Arc::new(tokio::sync::Notify::new());
        self.status(&chat_id, SessionStatus::Running, false);
        self.emit(
            &chat_id,
            vec![
                O7Event::TurnStarted {
                    turn_id: turn.clone(),
                    prompt_id: None,
                },
                O7Event::UserMessageDelta {
                    message_id: format!("{turn}.u"),
                    text,
                },
            ],
        );
        let work = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            self.emit(
                &chat_id,
                vec![
                    O7Event::MessageDelta {
                        message_id: format!("{turn}.m1"),
                        text: "I'll look at the **login form** first.".into(),
                    },
                    O7Event::ToolCall(ToolCall {
                        tool_call_id: format!("{turn}.x1"),
                        kind: ToolKind::Read,
                        title: "Read src/login.ts".into(),
                        status: ToolStatus::Completed,
                        locations: vec![],
                        raw_input: None,
                        tool_name: Some("Read".into()),
                        content: vec![],
                        truncated: None,
                    }),
                ],
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
            self.chats
                .lock()
                .unwrap()
                .iter_mut()
                .find(|(id, _)| *id == chat_id)
                .unwrap()
                .1
                .waiting = Some((request.clone(), asked.clone()));
            self.emit(
                &chat_id,
                vec![O7Event::PermissionRequested(PermissionRequest {
                    request_id: request.clone(),
                    tool_call_id: None,
                    title: "Run `npm test`".into(),
                    kind: Some(ToolKind::Execute),
                    raw_input: None,
                    content: vec![],
                    locations: vec![],
                    options: vec![
                        PermissionOption {
                            option_id: "allow".into(),
                            kind: PermissionOptionKind::AllowOnce,
                            label: "Allow once".into(),
                        },
                        PermissionOption {
                            option_id: "reject".into(),
                            kind: PermissionOptionKind::RejectOnce,
                            label: "Reject".into(),
                        },
                    ],
                    truncated: None,
                })],
            );
            self.status(&chat_id, SessionStatus::Running, true);
            asked.notified().await;
            self.status(&chat_id, SessionStatus::Running, false);
            tokio::time::sleep(Duration::from_millis(300)).await;
            // What it was allowed to run, and one command that fails: a reading of the turn
            // says how each ended.
            let ran = |id: &str, command: &str, exit: i32, output: &str| {
                O7Event::ToolCall(ToolCall {
                    tool_call_id: format!("{turn}.{id}"),
                    kind: ToolKind::Execute,
                    title: command.into(),
                    status: if exit == 0 {
                        ToolStatus::Completed
                    } else {
                        ToolStatus::Failed
                    },
                    locations: vec![],
                    raw_input: Some(json!({ "command": command })),
                    tool_name: Some("Bash".into()),
                    content: vec![o7_protocol::ToolContent::Terminal {
                        output: output.into(),
                        exit_code: Some(exit),
                        truncated: None,
                    }],
                    truncated: None,
                })
            };
            self.emit(
                &chat_id,
                vec![
                    ran("x2", "npm test", 0, "PASS login.test.ts"),
                    ran(
                        "x3",
                        "npm run lint",
                        1,
                        "src/login.ts\n  3:7  error  'old' is assigned a value but never used\n",
                    ),
                ],
            );
            self.emit(
                &chat_id,
                vec![O7Event::MessageDelta {
                    message_id: format!("{turn}.m2"),
                    text: "Done. The form now checks the email before it submits:\n\n- an empty address is refused\n- `a@b` is accepted\n\n```ts\nif (!email.includes(\"@\")) return;\n```".into(),
                }],
            );
            StopReason::EndTurn
        };
        let stop_reason = tokio::select! {
            reason = work => reason,
            () = stop.notified() => StopReason::Cancelled,
        };
        // A turn that ends takes its unanswered request with it.
        let open = self
            .chats
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(id, _)| *id == chat_id)
            .and_then(|(_, chat)| chat.waiting.take());
        let mut last = Vec::new();
        if let Some((request_id, _)) = open {
            last.push(O7Event::PermissionResolved {
                request_id,
                decision: PermissionDecision::Cancelled,
                by: ResolvedBy::Turn,
            });
        }
        last.push(O7Event::TurnEnded {
            turn_id: turn,
            stop_reason,
            usage: None,
            duration_ms: Some(900),
            recovered: false,
        });
        self.emit(&chat_id, last);
        self.status(&chat_id, SessionStatus::Ready, false);
    }
}

fn missing() -> BackendError {
    BackendError::new("not_found", "That chat is no longer here.")
}

/// A PNG of one pixel: what the demo computer's browser shows a full-control bot.
const DEMO_PICTURE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// The backend the service drives. It holds the demo behind an `Arc` so a turn can outlive a call.
struct Shared(Arc<Demo>);

impl Shared {
    /// Starts a turn with `text` and the photos `images` names, unless one is running.
    fn start(
        &self,
        chat_id: &str,
        text: String,
        images: Vec<String>,
    ) -> Result<PromptAck, BackendError> {
        let busy = {
            let chats = self.0.chats.lock().unwrap();
            let (_, chat) = chats
                .iter()
                .find(|(id, _)| id == chat_id)
                .ok_or_else(missing)?;
            chat.status == SessionStatus::Running
        };
        if busy {
            return Err(BackendError::new(
                "queue_full",
                "The demo agent takes one message at a time.",
            ));
        }
        tokio::spawn(self.0.clone().turn(chat_id.to_owned(), text, images));
        Ok(PromptAck {
            prompt_id: "prompt".into(),
            queued: false,
        })
    }
}

impl Demo {
    /// The branch of its own `chat_id` works on, if any.
    fn branch_of(&self, chat_id: &str) -> Result<Option<String>, BackendError> {
        let chats = self.chats.lock().unwrap();
        let (_, chat) = chats
            .iter()
            .find(|(id, _)| id == chat_id)
            .ok_or_else(missing)?;
        Ok(chat.branch.clone())
    }
}

/// The pull request of every branch the demo has: open, its checks passed, approved.
fn demo_pull_request() -> o7_remote::PullRequest {
    o7_remote::PullRequest {
        number: 12,
        url: "https://github.com/demo/app/pull/12".into(),
        state: "open".into(),
        checks: o7_remote::PrChecks::Passed,
        notes: vec![o7_remote::PrNote {
            author: "you".into(),
            kind: "approved".into(),
            body: "Looks right. Ship it.".into(),
            trusted: true,
        }],
    }
}

/// A PNG's or a JPEG's width and height, read from its header; nothing for anything else.
fn dimensions(bytes: &[u8]) -> (u32, u32) {
    let be32 = |at: usize| {
        bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return (be32(16), be32(20));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xFF {
                break;
            }
            let marker = bytes[at + 1];
            let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            // A start of frame: its height, then its width.
            if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
                let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
                return (u32::from(width), u32::from(height));
            }
            at += 2 + length;
        }
    }
    (0, 0)
}

impl Backend for Shared {
    // The demo computer has one workspace.
    fn bound(self: Arc<Self>, _profile: &str) -> Arc<dyn Backend> {
        self
    }

    // It looks as a new o7 does: Claude Code's theme, following the system.
    fn look(&self) -> Option<o7_remote::Look> {
        Some(o7_remote::Look {
            theme: "claude".into(),
            appearance: "system".into(),
        })
    }

    // The chats whose title or words hold the query, as the window's own search finds them.
    fn search<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ChatSearchHit>, BackendError>> {
        Box::pin(async move {
            let needle = query.to_lowercase();
            let chats = self.0.chats.lock().unwrap();
            let mut hits: Vec<(i64, ChatSearchHit)> = chats
                .iter()
                .filter_map(|(id, chat)| {
                    let said = chat.events.iter().find_map(|(_, event)| match event {
                        O7Event::MessageDelta { text, .. }
                        | O7Event::UserMessageDelta { text, .. }
                            if text.to_lowercase().contains(&needle) =>
                        {
                            Some(snippet(text, &needle))
                        }
                        _ => None,
                    });
                    (said.is_some() || chat.title.to_lowercase().contains(&needle)).then(|| {
                        (
                            chat.updated_at,
                            ChatSearchHit {
                                id: id.clone(),
                                title: chat.title.clone(),
                                project: "demo".into(),
                                snippet: said,
                                archived: false,
                            },
                        )
                    })
                })
                .collect();
            hits.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
            Ok(hits.into_iter().map(|(_, hit)| hit).collect())
        })
    }

    // A chat's browser shows a page of one pixel; there is no device.
    fn picture<'a>(
        &'a self,
        _chat_id: &'a str,
        of: o7_remote::PictureOf,
    ) -> BoxFuture<'a, Result<o7_remote::Picture, BackendError>> {
        Box::pin(async move {
            match of {
                o7_remote::PictureOf::Browser => Ok(o7_remote::Picture {
                    mime: "image/png".into(),
                    data: DEMO_PICTURE.into(),
                }),
                o7_remote::PictureOf::Device => Err(BackendError::new(
                    "invalid",
                    "The demo computer has no device.",
                )),
            }
        })
    }

    fn overview(&self) -> BoxFuture<'_, Result<Overview, BackendError>> {
        Box::pin(async move {
            let mut chats: Vec<WorkspaceChat> = self
                .0
                .chats
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, chat)| !chat.archived)
                .map(|(id, chat)| Demo::summary(id, chat))
                .collect();
            chats.sort_by_key(|entry| std::cmp::Reverse(entry.chat.updated_at));
            Ok(Overview {
                host_name: host_name().into(),
                projects: vec![
                    serde_json::from_value(json!({
                        "id": "p1", "name": "demo", "color": null, "default_agent": "demo",
                        "default_model": null, "created_at": 0, "updated_at": 0,
                        "folders": [{
                            "id": "f1", "project_id": "p1", "display_name": "demo", "path": "/demo",
                            "folder_state": "ok", "unavailable_reason": null, "kind": "git",
                            "git_remote_url": null, "git_default_branch": "main",
                        }],
                    }))
                    .expect("project"),
                ],
                chats,
                agents: AGENTS
                    .iter()
                    .map(|(id, name, available)| RemoteAgent {
                        id: (*id).into(),
                        name: (*name).into(),
                        available: *available,
                    })
                    .collect(),
                default_agent: Some("demo".into()),
            })
        })
    }

    /// The pickers (what a chat offers, a model, a thinking level, a handoff to another agent),
    /// and what else a chat or a project changes: its access, title and place, its agent, its
    /// changes and the project's brief.
    fn act(&self, action: Action) -> BoxFuture<'_, Result<Outcome, BackendError>> {
        Box::pin(async move {
            let invalid = |what: &str| {
                BackendError::new("invalid", format!("This agent has no such {what}."))
            };
            let done = || Ok(Outcome::Done("Done.".into()));
            let mut chats = self.0.chats.lock().unwrap();
            match action {
                Action::Options { chat_id } => {
                    let (_, chat) = chats
                        .iter()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    Ok(Outcome::Options(ChatOptions {
                        models: choices(&MODELS),
                        model: Some(chat.model.clone()),
                        thinking: choices(&LEVELS),
                        thinking_level: Some(chat.thinking.clone()),
                        access: choices(&ACCESS),
                        access_mode: Some(chat.access.clone()),
                    }))
                }
                Action::SetModel { chat_id, model } => {
                    let (_, chat) = chats
                        .iter_mut()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    if !MODELS.iter().any(|(value, _)| *value == model) {
                        return Err(invalid("model"));
                    }
                    chat.model = model;
                    done()
                }
                Action::SetThinking { chat_id, level } => {
                    let (_, chat) = chats
                        .iter_mut()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    if !LEVELS.iter().any(|(value, _)| *value == level) {
                        return Err(invalid("thinking level"));
                    }
                    chat.thinking = level;
                    done()
                }
                Action::SetAccess { chat_id, access } => {
                    let (_, chat) = chats
                        .iter_mut()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    if !ACCESS.iter().any(|(value, _)| *value == access) {
                        return Err(invalid("access level"));
                    }
                    chat.access = access;
                    done()
                }
                Action::SwitchAgent { chat_id, agent_id } => {
                    let agent = AGENTS
                        .iter()
                        .find(|(id, _, available)| *id == agent_id && *available)
                        .ok_or_else(|| BackendError::new("invalid", "That agent cannot run here."))?
                        .0;
                    let (_, from) = chats
                        .iter()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    if from.status == SessionStatus::Running {
                        return Err(BackendError::new(
                            "busy",
                            "Stop the turn before handing the chat to another agent.",
                        ));
                    }
                    // The app's handoff: a new chat that starts from this one's history.
                    let mut next = chat(&from.title);
                    next.agent = agent;
                    next.events = from.events.clone();
                    let id = format!("c{}", chats.len() + 1);
                    chats.push((id.clone(), next));
                    Ok(Outcome::Chat { id })
                }
                Action::Rename { chat_id, title } => {
                    let (_, chat) = chats
                        .iter_mut()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    chat.title = title;
                    drop(chats);
                    self.0.changed(&chat_id);
                    done()
                }
                Action::Archive { chat_id, archived } => {
                    let (_, chat) = chats
                        .iter_mut()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    chat.archived = archived;
                    drop(chats);
                    self.0.changed(&chat_id);
                    done()
                }
                Action::StartAgent { chat_id } => {
                    let (_, chat) = chats
                        .iter()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    let stopped = chat.status == SessionStatus::Stopped;
                    drop(chats);
                    if stopped {
                        self.0.status(&chat_id, SessionStatus::Ready, false);
                    }
                    done()
                }
                // The last turn changed the login form, in every chat that ran one.
                Action::Changes { chat_id } => {
                    let (_, chat) = chats
                        .iter()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    Ok(Outcome::Changes {
                        files: if chat.turns > 0 {
                            vec![("src/login.ts".into(), "modified".into())]
                        } else {
                            Vec::new()
                        },
                        unavailable: None,
                    })
                }
                Action::Diff { chat_id, path } => {
                    let (_, chat) = chats
                        .iter()
                        .find(|(id, _)| *id == chat_id)
                        .ok_or_else(missing)?;
                    if chat.turns == 0 || path != "src/login.ts" {
                        return Err(BackendError::new(
                            "not_found",
                            "The last turn did not change that file.",
                        ));
                    }
                    Ok(Outcome::Diff {
                        before: Some(LOGIN_BEFORE.into()),
                        after: Some(LOGIN_AFTER.into()),
                        binary: false,
                        truncated: false,
                    })
                }
                Action::ReadMemory { project_id } => {
                    drop(chats);
                    let briefs = self.0.briefs.lock().unwrap();
                    let (markdown, revision) = briefs.get(&project_id).cloned().unwrap_or_default();
                    Ok(Outcome::Memory {
                        markdown,
                        revision: (revision > 0).then(|| format!("r{revision}")),
                    })
                }
                Action::WriteMemory {
                    project_id,
                    markdown,
                } => {
                    drop(chats);
                    let mut briefs = self.0.briefs.lock().unwrap();
                    let brief = briefs.entry(project_id).or_default();
                    *brief = (markdown, brief.1 + 1);
                    Ok(Outcome::Done("Saved the project's memory.".into()))
                }
                // The person's own edit, refused when the brief moved on since it was read.
                Action::SaveMemory {
                    project_id,
                    markdown,
                    base_revision,
                    ..
                } => {
                    drop(chats);
                    let mut briefs = self.0.briefs.lock().unwrap();
                    let brief = briefs.entry(project_id).or_default();
                    let current = (brief.1 > 0).then(|| format!("r{}", brief.1));
                    if base_revision.is_some_and(|base| base != current) {
                        return Err(BackendError::new(
                            "conflict",
                            "The brief changed on the computer since it was read. Read it again.",
                        ));
                    }
                    *brief = (markdown.clone(), brief.1 + 1);
                    Ok(Outcome::Memory {
                        markdown,
                        revision: Some(format!("r{}", brief.1)),
                    })
                }
                Action::CreateProject { .. } => Err(BackendError::new(
                    "unavailable",
                    "The demo computer does not offer that.",
                )),
            }
        })
    }

    // Archived chats are left out of the overview and listed here.
    fn archived(&self) -> BoxFuture<'_, Result<Vec<WorkspaceChat>, BackendError>> {
        Box::pin(async move {
            Ok(self
                .0
                .chats
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, chat)| chat.archived)
                .map(|(id, chat)| Demo::summary(id, chat))
                .collect())
        })
    }

    // A chat on a branch of its own has a pull request whose checks passed.
    fn facts<'a>(
        &'a self,
        chat_id: &'a str,
        _agent_id: &'a str,
    ) -> BoxFuture<'a, Result<o7_remote::ChatFacts, BackendError>> {
        Box::pin(async move {
            let branch = self.0.branch_of(chat_id)?;
            Ok(o7_remote::ChatFacts {
                pull_request: branch
                    .as_ref()
                    .map(|_| o7_remote::render::pull_request(&demo_pull_request())),
                worktree: branch.is_some(),
                branch: Some(branch.unwrap_or_else(|| "main".into())),
                uncommitted: Some(0),
                usage: vec![("5 hours".into(), 42.0), ("Week".into(), 18.0)],
            })
        })
    }

    fn pull_request<'a>(
        &'a self,
        chat_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<o7_remote::PullRequest>, BackendError>> {
        Box::pin(async move { Ok(self.0.branch_of(chat_id)?.map(|_| demo_pull_request())) })
    }

    // A chat on a branch of its own changed the login form and added its test.
    fn branch_changes<'a>(
        &'a self,
        chat_id: &'a str,
        base: Option<&'a str>,
        _max_files: usize,
    ) -> BoxFuture<'a, Result<o7_remote::BranchChanges, BackendError>> {
        Box::pin(async move {
            Ok(match self.0.branch_of(chat_id)? {
                Some(_) => o7_remote::BranchChanges {
                    files: vec![
                        ("src/login.test.ts".into(), "added".into()),
                        ("src/login.ts".into(), "modified".into()),
                    ],
                    base: Some(base.unwrap_or(BASE).to_owned()),
                    unavailable: None,
                },
                None => o7_remote::BranchChanges {
                    files: Vec::new(),
                    base: None,
                    unavailable: Some(
                        "This chat has no branch of its own: it works in the project folder."
                            .into(),
                    ),
                },
            })
        })
    }

    fn branch_diff<'a>(
        &'a self,
        chat_id: &'a str,
        _base: Option<&'a str>,
        path: &'a str,
        _max_bytes: usize,
    ) -> BoxFuture<'a, Result<o7_remote::Diff, BackendError>> {
        Box::pin(async move {
            let not_found =
                || BackendError::new("not_found", "This chat's branch did not change that file.");
            self.0.branch_of(chat_id)?.ok_or_else(not_found)?;
            match path {
                "src/login.ts" => Ok(o7_remote::Diff {
                    before: Some(LOGIN_BEFORE.into()),
                    after: Some(LOGIN_AFTER.into()),
                    binary: false,
                    truncated: false,
                }),
                "src/login.test.ts" => Ok(o7_remote::Diff {
                    before: None,
                    after: Some(LOGIN_TEST.into()),
                    binary: false,
                    truncated: false,
                }),
                _ => Err(not_found()),
            }
        })
    }

    // A task's chat works in a worktree of its own, on a branch named for its goal.
    fn create_task_chat<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
        title: &'a str,
    ) -> BoxFuture<'a, Result<o7_remote::TaskChat, BackendError>> {
        Box::pin(async move {
            let made = self.create(folder_id, agent_id).await?;
            let slug: String = title
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .take(5)
                .collect::<Vec<_>>()
                .join("-");
            let branch = format!("o7/{}", if slug.is_empty() { "task" } else { &slug });
            {
                let mut chats = self.0.chats.lock().unwrap();
                if let Some((_, chat)) = chats.iter_mut().find(|(id, _)| *id == made.chat.id) {
                    chat.title = title.to_owned();
                    chat.branch = Some(branch.clone());
                }
            }
            Ok(o7_remote::TaskChat {
                chat: made,
                path: Some(format!("/demo-worktrees/{}", &branch[3..])),
                branch: Some(branch),
                base: Some(BASE.into()),
            })
        })
    }

    // A photo goes into the chat's message box; the next message carries it.
    fn attach_image<'a>(
        &'a self,
        chat_id: &'a str,
        bytes: Vec<u8>,
        name: &'a str,
        mime: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>> {
        Box::pin(async move {
            let mut attached = self.0.attached.lock().unwrap();
            let id = format!("a{}", attached.len() + 1);
            attached.push((
                id.clone(),
                Attached {
                    chat: chat_id.to_owned(),
                    name: name.to_owned(),
                    mime: mime.to_owned(),
                    bytes,
                },
            ));
            Ok(id)
        })
    }

    fn send_input<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
        attachments: Vec<String>,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async move {
            let names: Vec<String> = {
                let attached = self.0.attached.lock().unwrap();
                attachments
                    .iter()
                    .map(|id| {
                        attached
                            .iter()
                            .find(|(known, photo)| known == id && photo.chat == chat_id)
                            .map(|(_, photo)| photo.name.clone())
                            .ok_or_else(|| {
                                BackendError::new("invalid", "That attachment is not this chat's.")
                            })
                    })
                    .collect::<Result<_, _>>()?
            };
            // As the window shows a message with images: its text, then each image named.
            let mut shown = text;
            for name in &names {
                shown.push_str(&format!("\n\n[Image: {name}]"));
            }
            self.start(chat_id, shown.trim_start().to_owned(), attachments)
        })
    }

    // The photos a chat holds: one in its message box, one a message carried, or one by its
    // SHA-256. Never larger than the device asked for.
    fn image<'a>(
        &'a self,
        chat_id: &'a str,
        source: o7_protocol::composer::ChatImageSource,
        _full: bool,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<o7_remote::ImagePreview, BackendError>> {
        use base64::Engine as _;
        use o7_protocol::composer::{ChatImageContext, ChatImageSource};
        Box::pin(async move {
            let gone = || BackendError::new("invalid", "This image is unavailable in this chat.");
            let id = match source {
                ChatImageSource::Attachment { id } => id,
                ChatImageSource::Message {
                    context: ChatImageContext::Turn { turn_id },
                    index,
                } => self
                    .0
                    .turn_images
                    .lock()
                    .unwrap()
                    .get(&turn_id)
                    .and_then(|ids| ids.get(index as usize).cloned())
                    .ok_or_else(gone)?,
                ChatImageSource::Stored { sha256 } => {
                    use sha2::Digest as _;
                    self.0
                        .attached
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(_, photo)| {
                            sha2::Sha256::digest(&photo.bytes)
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>()
                                == sha256
                        })
                        .map(|(id, _)| id.clone())
                        .ok_or_else(gone)?
                }
                _ => return Err(gone()),
            };
            let photo = self
                .0
                .attached
                .lock()
                .unwrap()
                .iter()
                .find(|(known, photo)| *known == id && photo.chat == chat_id)
                .map(|(_, photo)| photo.clone())
                .ok_or_else(gone)?;
            let data = base64::engine::general_purpose::STANDARD.encode(&photo.bytes);
            if data.len() > max_bytes {
                return Err(BackendError::new(
                    "too_large",
                    "The demo computer cannot make this image smaller.",
                ));
            }
            let (width, height) = dimensions(&photo.bytes);
            Ok(o7_remote::ImagePreview {
                mime: photo.mime,
                data,
                width,
                height,
            })
        })
    }

    fn watch(&self, owner: &str, statuses: Statuses) -> Result<(), BackendError> {
        self.0
            .statuses
            .lock()
            .unwrap()
            .insert(owner.into(), statuses);
        Ok(())
    }

    fn attach<'a>(
        &'a self,
        chat_id: &'a str,
        owner: &'a str,
        frames: Frames,
    ) -> BoxFuture<'a, Result<ChatAttach, BackendError>> {
        Box::pin(async move {
            let chats = self.0.chats.lock().unwrap();
            let (id, chat) = chats
                .iter()
                .find(|(id, _)| id == chat_id)
                .ok_or_else(missing)?;
            self.0
                .frames
                .lock()
                .unwrap()
                .insert(owner.into(), (chat_id.into(), frames));
            let items: Vec<HistoryItem> = chat
                .events
                .iter()
                .enumerate()
                .map(|(seq, (at, event))| HistoryItem {
                    seq: seq as u32,
                    turn_id: None,
                    at: *at,
                    event: event.clone(),
                })
                .collect();
            Ok(serde_json::from_value(json!({
                "subscription_id": owner,
                "next_seq": items.len(),
                "snapshot": {
                    "chat": Demo::summary(id, chat).chat,
                    "session": o7_protocol::SessionInfo { status: chat.status, ..Default::default() },
                    "queue": [], "open_turn_id": null, "usage": null, "limits": null,
                    "last_error": null, "auth": null,
                    "storage": o7_protocol::session::StorageStatus::default(),
                },
                "history": HistoryPage {
                    chat_id: chat_id.into(),
                    items,
                    has_more: false,
                    before_seq: None,
                    cut_turn_id: None,
                    unsaved_from: None,
                    skipped: 0,
                    turn_agents: vec![],
                },
            }))
            .expect("attach"))
        })
    }

    fn release(&self, owner: &str) {
        self.0.frames.lock().unwrap().remove(owner);
        self.0.statuses.lock().unwrap().remove(owner);
    }

    fn history<'a>(
        &'a self,
        chat_id: &'a str,
        before_seq: u32,
    ) -> BoxFuture<'a, Result<HistoryPage, BackendError>> {
        Box::pin(async move {
            let chats = self.0.chats.lock().unwrap();
            let (_, chat) = chats
                .iter()
                .find(|(id, _)| id == chat_id)
                .ok_or_else(missing)?;
            Ok(HistoryPage {
                chat_id: chat_id.into(),
                items: chat
                    .events
                    .iter()
                    .enumerate()
                    .filter(|(seq, _)| (*seq as u32) < before_seq)
                    .map(|(seq, (at, event))| HistoryItem {
                        seq: seq as u32,
                        turn_id: None,
                        at: *at,
                        event: event.clone(),
                    })
                    .collect(),
                has_more: false,
                before_seq: None,
                cut_turn_id: None,
                unsaved_from: None,
                skipped: 0,
                turn_agents: vec![],
            })
        })
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async move { self.start(chat_id, text, Vec::new()) })
    }

    fn interrupt<'a>(&'a self, chat_id: &'a str) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let chats = self.0.chats.lock().unwrap();
            let (_, chat) = chats
                .iter()
                .find(|(id, _)| id == chat_id)
                .ok_or_else(missing)?;
            chat.stop.notify_one();
            Ok(())
        })
    }

    fn permission<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        option_id: Option<String>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let asked = {
                let mut chats = self.0.chats.lock().unwrap();
                let (_, chat) = chats
                    .iter_mut()
                    .find(|(id, _)| id == chat_id)
                    .ok_or_else(missing)?;
                match chat.waiting.take() {
                    Some((id, asked)) if id == request_id => asked,
                    other => {
                        chat.waiting = other;
                        return Err(BackendError::new(
                            "request_not_pending",
                            "That request is not waiting any more.",
                        ));
                    }
                }
            };
            self.0.emit(
                chat_id,
                vec![O7Event::PermissionResolved {
                    request_id: request_id.to_owned(),
                    decision: match option_id {
                        Some(option_id) => PermissionDecision::Selected {
                            option_id,
                            plan: None,
                        },
                        None => PermissionDecision::Cancelled,
                    },
                    by: ResolvedBy::User,
                }],
            );
            asked.notify_one();
            Ok(())
        })
    }

    fn answer<'a>(
        &'a self,
        _chat_id: &'a str,
        _request_id: &'a str,
        _answer: Option<UiAnswer>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            Err(BackendError::new(
                "request_not_pending",
                "That request is not waiting any more.",
            ))
        })
    }

    fn create<'a>(
        &'a self,
        folder_id: &'a str,
        _agent_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<WorkspaceChat, BackendError>> {
        Box::pin(async move {
            if folder_id != "f1" {
                return Err(BackendError::new(
                    "invalid",
                    "That folder is no longer here.",
                ));
            }
            let mut chats = self.0.chats.lock().unwrap();
            let id = format!("c{}", chats.len() + 1);
            chats.push((id.clone(), chat("New chat")));
            Ok(Demo::summary(&id, &chats.last().unwrap().1))
        })
    }
}

/// The words around the first word of `needle` in `text`, as a search result shows them.
fn snippet(text: &str, needle: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let first = needle.split_whitespace().next().unwrap_or_default();
    let at = words
        .iter()
        .position(|word| word.to_lowercase().contains(first))
        .unwrap_or(0);
    let (from, to) = (at.saturating_sub(4), (at + 8).min(words.len()));
    format!(
        "{}{}{}",
        if from > 0 { "…" } else { "" },
        words[from..to].join(" "),
        if to < words.len() { "…" } else { "" }
    )
}

fn chat(title: &str) -> Chat {
    Chat {
        title: title.into(),
        status: SessionStatus::Stopped,
        needs_attention: false,
        events: Vec::new(),
        updated_at: now(),
        waiting: None,
        stop: Arc::new(tokio::sync::Notify::new()),
        turns: 0,
        agent: "demo",
        model: "fast".into(),
        thinking: "low".into(),
        access: "ask".into(),
        archived: false,
        branch: None,
    }
}

/// The computer's name, as its pairings and overview give it: the fourth argument, so a test can
/// run two computers side by side, else "Demo computer".
static HOST_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn host_name() -> &'static str {
    HOST_NAME.get().map_or("Demo computer", String::as_str)
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (relay, site, folder) = (
        args.next().expect("the relay's ws:// origin"),
        args.next().expect("the site's origin"),
        args.next().expect("a folder for remote.json"),
    );
    if let Some(name) = args.next() {
        let _ = HOST_NAME.set(name);
    }
    std::fs::create_dir_all(&folder).expect("folder");
    let demo = Arc::new(Demo::default());
    {
        let mut first = chat("Fix the login form");
        first.status = SessionStatus::Ready;
        // Its turn finished an hour ago: the app's Inbox lists it among what finished today.
        let ended = now() - 60 * 60 * 1000;
        first.updated_at = ended;
        first.events = vec![
            O7Event::TurnStarted {
                turn_id: "c1-t0".into(),
                prompt_id: None,
            },
            O7Event::UserMessageDelta {
                message_id: "c1-t0.u".into(),
                text: "Why does the login form submit empty emails?".into(),
            },
            O7Event::MessageDelta {
                message_id: "c1-t0.m".into(),
                text: "The submit handler never checks the field. I can add a check.".into(),
            },
            O7Event::TurnEnded {
                turn_id: "c1-t0".into(),
                stop_reason: StopReason::EndTurn,
                usage: None,
                duration_ms: Some(1200),
                recovered: false,
            },
        ]
        .into_iter()
        .enumerate()
        .map(|(seq, event)| (ended - 1200 + 400 * seq as i64, event))
        .collect();
        first.turns = 1;
        // It works in a worktree of its own, whose pull request passed its checks.
        first.branch = Some("o7/fix-the-login-form".into());
        demo.chats.lock().unwrap().push(("c1".into(), first));
        demo.briefs.lock().unwrap().insert(
            "p1".into(),
            (
                "# demo\n\nShort sentences. Run `npm test` before a commit.".into(),
                1,
            ),
        );
    }
    let service = Service::start(
        Config {
            path: std::path::Path::new(&folder).join("remote.json"),
            relay,
            site,
            host_name: host_name().into(),
            profile: "demo".into(),
            backend: Arc::new(Shared(demo)),
            changed: Arc::new(|| {}),
            deliver: Arc::new(|_| {}),
        },
        &tokio::runtime::Handle::current(),
    )
    .expect("service");
    service.set_enabled(true).expect("enable");
    println!("PAIR {}", service.pair().expect("pairing").url);
    println!(
        "BOT {}",
        service
            .create_bot(
                "Demo bot",
                // It answers the demo agent's request, as the end-to-end test does.
                o7_remote::bot::Rights {
                    approve: true,
                    ..o7_remote::bot::Rights::CHATS
                },
            )
            .expect("bot")
            .url
    );
    // A full-control link too, for what only such a link has (`look`).
    println!(
        "FULLBOT {}",
        service
            .create_bot("Demo full bot", o7_remote::bot::Rights::FULL)
            .expect("bot")
            .url
    );
    // An automation in the demo project, for what is sent to an address (`/hook/`).
    println!(
        "HOOK {}",
        service
            .create_automation("Demo automation", "p1", "Look into what was sent.")
            .expect("automation")
            .url
    );
    // Each line on standard input asks for a fresh pairing link; its end stops the demo.
    let lines = tokio::task::spawn_blocking(move || {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() {
                break;
            }
            println!("PAIR {}", service.pair().expect("pairing").url);
        }
        service.shutdown();
    });
    let _ = lines.await;
}
