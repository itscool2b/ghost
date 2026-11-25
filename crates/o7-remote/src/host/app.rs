//! What o7 for iPhone asks for beyond a browser's requests ([`wire::APP_FEATURES`]). Each is one
//! call a session's `perform` makes through the session's own bound backend, and reads what the
//! bot tools read: nothing here reaches further than [`Backend`].

use super::*;
use crate::{
    backend::{Action, Diff, Outcome, PictureOf, PrChecks, PullRequest},
    push::{Env, Prefs, Scope},
    registry::{ActivityToken, By, Push},
    render::{self, Waiting},
    wire::{
        ArchivedChats, Asked, Asking, Brief, ChangedFile, ChangedFiles, Facts, FileDiff, Inbox,
        MAX_INBOX, Night, PullRequestFacts, PullRequestNote, QueuedEntry, Recent,
        RecentPullRequest, SearchHit, SearchHits, TaskEntry, TaskList, TaskStarted, Usage, Working,
    },
};
use o7_protocol::composer::ChatImageSource;
use session::{Context, name_ok, text_ok, unexpected};
use uploads::{Held, Uploads, prepare};

/// What needs the person, as the app's Inbox shows it ([`wire::Op::Inbox`]): the requests chats
/// wait on, with what each asks and its choices; the chats working, with what each does now; and
/// the chats whose last turn ended in the last day, with what it came to, what its edits changed
/// and its pull request as the computer last heard of it. The chats are the ones the bot's
/// `inbox` reads ([`render::sorted`]).
///
/// It reads the overview, then the newest page of history of each chat it lists, as the bot's
/// `inbox` does for the chats it reads into: at most [`MAX_INBOX`] chats in each part, so at most
/// 120 pages, and in practice a few, since chats ask and work a few at a time. A pull request
/// costs no call to GitHub: what the computer already knows of it, or nothing.
pub(super) async fn inbox(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
) -> Result<Data, BackendError> {
    let overview = backend.overview().await?;
    let now = now();
    let sorted = render::sorted(&overview.chats, now);
    let mut inbox = Inbox::default();
    for entry in &sorted.asking {
        if inbox.asking.len() >= MAX_INBOX {
            break;
        }
        let Some(page) = page(backend, &entry.chat.id).await? else {
            continue;
        };
        let mut asking: Vec<Asking> = render::digest(&page.items)
            .waiting
            .into_iter()
            .map(|request| Asking {
                chat_id: entry.chat.id.clone(),
                request: Some(asked(request)),
            })
            .collect();
        // It waits on something its newest page does not show yet: the chat itself says.
        if asking.is_empty() {
            asking.push(Asking {
                chat_id: entry.chat.id.clone(),
                request: None,
            });
        }
        inbox.asking.extend(asking);
    }
    inbox.asking.truncate(MAX_INBOX);
    for entry in sorted.working.iter().take(MAX_INBOX) {
        let Some(page) = page(backend, &entry.chat.id).await? else {
            continue;
        };
        // The start of the turn it is in; a chat starting one its history does not show yet
        // started when it last changed.
        let turn = render::last_turn(&page.items);
        inbox.working.push(Working {
            chat_id: entry.chat.id.clone(),
            line: render::doing(&entry.chat, &page.items),
            since: turn
                .started_at
                .filter(|_| turn.ended_at.is_none())
                .unwrap_or(entry.chat.updated_at),
        });
    }
    for entry in sorted.finished.iter().take(MAX_INBOX) {
        let Some(page) = page(backend, &entry.chat.id).await? else {
            continue;
        };
        let turn = render::last_turn(&page.items);
        // A turn that ended, in the last day: a chat that never ran has nothing to tell.
        let Some(ended_at) = turn
            .ended_at
            .filter(|at| now.saturating_sub(*at) <= render::RECENT_MS)
        else {
            continue;
        };
        let edits = render::last_turn_edits(&page.items);
        inbox.recent.push(Recent {
            chat_id: entry.chat.id.clone(),
            ended_at,
            duration_ms: turn.duration_ms,
            line: render::brief(&entry.chat, &page.items),
            files: (edits.files > 0).then_some(edits.files),
            added: edits.added,
            removed: edits.removed,
            pull_request: known_pull_request(shared, backend, context, &entry.chat.id),
        });
    }
    inbox
        .recent
        .sort_by_key(|recent| std::cmp::Reverse(recent.ended_at));
    Ok(Data::Inbox(Box::new(inbox)))
}

/// A chat's pull request as the computer last heard of it: what GitHub said lately to a reading
/// of the chat ([`Backend::known_pull_request`]), else what its task's watcher last saw. Never a
/// new look.
fn known_pull_request(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
    chat_id: &str,
) -> Option<RecentPullRequest> {
    if let Some(pr) = backend.known_pull_request(chat_id) {
        return Some(RecentPullRequest {
            number: pr.number,
            state: pr.state,
            checks: pr.checks.word().to_owned(),
        });
    }
    let seen = registry_here(shared, context).ok()?.pr_seen(chat_id)?;
    Some(RecentPullRequest {
        number: seen.number,
        state: seen.state,
        checks: seen.checks,
    })
}

/// A chat's newest page of history, or none when the chat went in the moment since the overview
/// was read.
async fn page(
    backend: &Arc<dyn Backend>,
    chat_id: &str,
) -> Result<Option<HistoryPage>, BackendError> {
    match backend.history(chat_id, u32::MAX).await {
        Ok(page) => Ok(Some(page)),
        Err(error) if error.code == "not_found" => Ok(None),
        Err(error) => Err(error),
    }
}

/// A request a chat waits on, as `permission` or `answer` answers it.
fn asked(request: Waiting) -> Asked {
    match request {
        Waiting::Permission(request) => Asked {
            id: request.request_id,
            kind: "permission",
            title: request.title,
            options: Some(request.options),
            prompt: None,
        },
        Waiting::Question(request) => Asked {
            id: request.request_id,
            kind: "question",
            title: render::question(&request.prompt),
            options: None,
            prompt: Some(request.prompt),
        },
    }
}

/// The chats a search finds ([`wire::Op::Search`]), as the window's own search finds them, at
/// most `limit` ([`wire::DEFAULT_HITS`] when it names none, never more than
/// [`wire::MAX_HITS`]). Each hit is dated by its chat in the overview, and a hit the overview
/// does not list is left out. It costs one search of the computer's chat index and one overview.
pub(super) async fn search(
    backend: &Arc<dyn Backend>,
    query: &str,
    limit: Option<u32>,
) -> Result<Data, BackendError> {
    let query = query.trim();
    let length = query.chars().count();
    if length == 0 || length > wire::MAX_QUERY || query.chars().any(char::is_control) {
        return Err(BackendError::new(
            "invalid",
            format!(
                "Search for 1 to {} characters on one line.",
                wire::MAX_QUERY
            ),
        ));
    }
    let limit = limit
        .map_or(wire::DEFAULT_HITS, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        })
        .clamp(1, wire::MAX_HITS);
    let found = backend.search(query).await?;
    let overview = backend.overview().await?;
    let hits = found
        .into_iter()
        .filter(|hit| !hit.archived)
        .filter_map(|hit| {
            let at = overview
                .chats
                .iter()
                .find(|entry| entry.chat.id == hit.id)?
                .chat
                .updated_at;
            Some(SearchHit {
                chat_id: hit.id,
                title: hit.title,
                snippet: hit.snippet,
                at,
            })
        })
        .take(limit)
        .collect();
    Ok(Data::Search(SearchHits { hits }))
}

/// The access levels a chat takes, by the value [`wire::Op::SetAccess`] names.
const ACCESS: [&str; 3] = ["ask", "auto_edit", "full_auto"];
/// What a picture's or an image's base64 may take, leaving room in the answer for the rest of
/// it: the answer stays under [`wire::MAX_PICTURE`].
const PICTURE_DATA: usize = wire::MAX_PICTURE - 1024;
/// The longest path a diff is asked for, and the longest revision a brief is saved over.
const MAX_PATH: usize = 4096;
const MAX_REVISION: usize = 256;
/// How many queued tasks are listed at most: ten per link or device.
const MAX_QUEUED_LISTED: usize = 100;
/// How much of each goal a list of tasks shows, and how much when the whole list would not fit
/// one message otherwise. A queued task keeps its whole goal for when it starts.
const GOAL_SHOWN: usize = 2_000;
const GOAL_LISTED: usize = 200;
/// The most an answer's data may take once encoded, escapes and all, leaving room for the rest
/// of the reply in one sealed message.
const ANSWER_BYTES: usize = MAX_PLAINTEXT - 1024;

fn invalid(message: impl Into<String>) -> BackendError {
    BackendError::new("invalid", message)
}

fn no_chat() -> BackendError {
    BackendError::new("not_found", "There is no chat with that id here.")
}

/// How long `value` is once encoded.
fn encoded(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The registry, while the workspace this session's key was accepted under is still the one
/// open: a request that ran on past a switch of workspace reaches nothing of the next one's
/// tasks or queue. Checked under the lock the registry's change of workspace takes.
fn registry_here<'a>(
    shared: &'a Shared,
    context: &Context,
) -> Result<MutexGuard<'a, crate::registry::Registry>, BackendError> {
    let registry = lock(&shared.registry);
    if registry.scope() != context.scope {
        return Err(BackendError::new(
            "unavailable",
            "Another workspace is open on this computer.",
        ));
    }
    Ok(registry)
}

/// Photos with a message: at most [`wire::MAX_ATTACHMENTS`], and the text no longer than any.
fn photos_ok(text: &str, attachments: &[String]) -> Result<(), BackendError> {
    if attachments.len() > wire::MAX_ATTACHMENTS {
        return Err(invalid(format!(
            "A message carries at most {} photos.",
            wire::MAX_ATTACHMENTS
        )));
    }
    if text.len() > MAX_TEXT {
        return Err(BackendError::new("too_large", "The message is too long."));
    }
    Ok(())
}

/// A message to a chat ([`wire::Op::Send`]); with photos for a session offered them, which go
/// from the stage only once the message went.
pub(super) async fn send(
    backend: &Arc<dyn Backend>,
    context: &Context,
    chat_id: &str,
    text: String,
    attachments: &[String],
) -> Result<Data, BackendError> {
    if attachments.is_empty() || !context.offered(wire::ATTACHMENTS) {
        text_ok(&text)?;
        return Ok(Data::Sent(backend.send(chat_id, text).await?));
    }
    photos_ok(&text, attachments)?;
    let held = Held::take(&context.uploads, attachments)?;
    let images = prepare(backend, &held.images).await?;
    let sent = send_with(backend, chat_id, text, &images).await?;
    held.sent();
    Ok(Data::Sent(sent))
}

/// A new chat with its first message ([`wire::Op::Create`]). The photos are held and measured
/// before the chat is made, so one that is not staged, or photos too large together, make no
/// chat. For a session offered photos, a first message that fails once the chat exists is said
/// in `unsent` beside the chat; a browser is answered with the error, as before.
pub(super) async fn create(
    backend: &Arc<dyn Backend>,
    context: &Context,
    folder_id: &str,
    agent_id: Option<&str>,
    text: String,
    attachments: &[String],
) -> Result<Data, BackendError> {
    let app = context.offered(wire::ATTACHMENTS);
    let photos = app && !attachments.is_empty();
    if photos {
        photos_ok(&text, attachments)?;
    } else {
        text_ok(&text)?;
    }
    let held = if photos {
        Some(Held::take(&context.uploads, attachments)?)
    } else {
        None
    };
    let images = match &held {
        Some(held) => Some(prepare(backend, &held.images).await?),
        None => None,
    };
    let chat = backend.create(folder_id, agent_id).await?;
    let sent = match &images {
        Some(images) => send_with(backend, &chat.chat.id, text, images).await,
        None => backend.send(&chat.chat.id, text).await,
    };
    match sent {
        Ok(sent) => {
            if let Some(held) = held {
                held.sent();
            }
            Ok(Data::Created {
                chat: Box::new(chat),
                sent,
            })
        }
        Err(error) if app => Ok(Data::Unsent {
            chat: Box::new(chat),
            unsent: error.message,
        }),
        Err(error) => Err(error),
    }
}

/// A page of archived chats, newest first ([`wire::Op::Archived`]). A page never ends between
/// chats that changed in the same millisecond, so asking for those before the last one's time
/// misses none.
pub(super) async fn archived(
    backend: &Arc<dyn Backend>,
    before: Option<i64>,
    limit: Option<u32>,
) -> Result<Data, BackendError> {
    let limit = limit
        .map_or(wire::DEFAULT_ARCHIVED, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        })
        .clamp(1, wire::MAX_ARCHIVED);
    let mut chats: Vec<_> = backend
        .archived()
        .await?
        .into_iter()
        .filter(|entry| entry.archived)
        .filter(|entry| before.is_none_or(|before| entry.chat.updated_at < before))
        .collect();
    chats.sort_by(|a, b| {
        b.chat
            .updated_at
            .cmp(&a.chat.updated_at)
            .then_with(|| a.chat.id.cmp(&b.chat.id))
    });
    let mut end = limit.min(chats.len());
    while end > 0
        && end < chats.len()
        && chats[end].chat.updated_at == chats[end - 1].chat.updated_at
    {
        end += 1;
    }
    let more = end < chats.len();
    chats.truncate(end);
    Ok(Data::Archived(Box::new(ArchivedChats { chats, more })))
}

/// A chat's access level, exactly one of [`ACCESS`] ([`wire::Op::SetAccess`]).
pub(super) async fn set_access(
    backend: &Arc<dyn Backend>,
    chat_id: String,
    access: String,
) -> Result<Data, BackendError> {
    if !ACCESS.contains(&access.as_str()) {
        return Err(invalid("The access level is ask, auto_edit or full_auto."));
    }
    backend.act(Action::SetAccess { chat_id, access }).await?;
    Ok(Data::None {})
}

/// A chat's new title, trimmed: 1 to [`wire::MAX_TITLE`] characters on one line
/// ([`wire::Op::Rename`]).
pub(super) async fn rename(
    backend: &Arc<dyn Backend>,
    chat_id: String,
    title: &str,
) -> Result<Data, BackendError> {
    let title = title.trim();
    let length = title.chars().count();
    if length == 0 || length > wire::MAX_TITLE || title.chars().any(char::is_control) {
        return Err(invalid(format!(
            "A chat's title is 1 to {} characters on one line.",
            wire::MAX_TITLE
        )));
    }
    backend
        .act(Action::Rename {
            chat_id,
            title: title.to_owned(),
        })
        .await?;
    Ok(Data::None {})
}

/// `turn` (the default) or `branch`.
fn scope_of(scope: Option<&str>) -> Result<&'static str, BackendError> {
    match scope {
        None | Some("turn") => Ok("turn"),
        Some("branch") => Ok("branch"),
        Some(_) => Err(invalid("The scope is turn or branch.")),
    }
}

/// The commit a task's chat started from, when `chat_id` does a task's work in this session's
/// workspace.
fn task_base(
    shared: &Shared,
    context: &Context,
    chat_id: &str,
) -> Result<Option<String>, BackendError> {
    Ok(registry_here(shared, context)?
        .task_of(chat_id)
        .filter(|task| task.builder == chat_id)
        .and_then(|task| task.base))
}

/// The files a chat changed, in its last turn or on its own branch, at most
/// [`wire::MAX_FILES`], and fewer when their paths would not fit one answer. A branch is counted
/// from its task's base where the chat does a task's work, and otherwise from where the
/// computer finds the branch began.
async fn listed(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
    chat_id: &str,
    scope: &'static str,
) -> Result<ChangedFiles, BackendError> {
    let (files, unavailable, base) = if scope == "turn" {
        match backend
            .act(Action::Changes {
                chat_id: chat_id.to_owned(),
            })
            .await?
        {
            Outcome::Changes { files, unavailable } => (files, unavailable, None),
            _ => return Err(unexpected()),
        }
    } else {
        let base = task_base(shared, context, chat_id)?;
        let branch = backend
            .branch_changes(chat_id, base.as_deref(), wire::MAX_FILES)
            .await?;
        (branch.files, branch.unavailable, branch.base)
    };
    let mut listed = ChangedFiles {
        files: files
            .into_iter()
            .take(wire::MAX_FILES)
            .map(|(path, kind)| ChangedFile { path, kind })
            .collect(),
        unavailable,
        scope,
        base,
    };
    while !listed.files.is_empty() && encoded(&listed) > ANSWER_BYTES {
        let keep = listed.files.len() * 3 / 4;
        listed.files.truncate(keep);
    }
    Ok(listed)
}

/// [`wire::Op::Changes`].
pub(super) async fn changes(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
    chat_id: &str,
    scope: Option<&str>,
) -> Result<Data, BackendError> {
    let scope = scope_of(scope)?;
    Ok(Data::Changes(Box::new(
        listed(shared, backend, context, chat_id, scope).await?,
    )))
}

/// One changed file, before and after ([`wire::Op::Diff`]). Only a file the list of changes
/// names is read, whatever path is asked for: anything else is `not_found`.
pub(super) async fn diff(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
    chat_id: &str,
    path: &str,
    scope: Option<&str>,
) -> Result<Data, BackendError> {
    let scope = scope_of(scope)?;
    if path.is_empty() || path.len() > MAX_PATH || path.chars().any(char::is_control) {
        return Err(invalid("That is not a file of this chat's changes."));
    }
    let listed = listed(shared, backend, context, chat_id, scope).await?;
    if !listed.files.iter().any(|file| file.path == path) {
        return Err(BackendError::new(
            "not_found",
            if scope == "turn" {
                "The last turn did not change that file."
            } else {
                "This chat's branch did not change that file."
            },
        ));
    }
    let diff = if scope == "turn" {
        match backend
            .act(Action::Diff {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
            })
            .await?
        {
            Outcome::Diff {
                before,
                after,
                binary,
                truncated,
            } => Diff {
                before,
                after,
                binary,
                truncated,
            },
            _ => return Err(unexpected()),
        }
    } else {
        backend
            .branch_diff(chat_id, listed.base.as_deref(), path, wire::MAX_SIDE)
            .await?
    };
    Ok(Data::Diff(Box::new(fitted(diff))))
}

/// Each side of a file cut to [`wire::MAX_SIDE`] bytes, at a character's edge; a binary file's
/// sides are not sent.
fn fitted(diff: Diff) -> FileDiff {
    if diff.binary {
        return FileDiff {
            before: None,
            after: None,
            binary: true,
            truncated: diff.truncated,
        };
    }
    let mut truncated = diff.truncated;
    let mut cut = |side: Option<String>| {
        side.map(|mut text| {
            if text.len() > wire::MAX_SIDE {
                let mut end = wire::MAX_SIDE;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                truncated = true;
            }
            text
        })
    };
    let before = cut(diff.before);
    let after = cut(diff.after);
    let mut fitted = FileDiff {
        before,
        after,
        binary: false,
        truncated,
    };
    // Escapes can make the encoding longer than the text: the longer side is halved until the
    // answer fits one message.
    while encoded(&fitted) > ANSWER_BYTES {
        let length = |side: &Option<String>| side.as_ref().map_or(0, String::len);
        let longer = if length(&fitted.before) >= length(&fitted.after) {
            &mut fitted.before
        } else {
            &mut fitted.after
        };
        let Some(text) = longer.as_mut().filter(|text| !text.is_empty()) else {
            break;
        };
        let mut end = text.len() / 2;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        fitted.truncated = true;
    }
    fitted
}

/// What a chat stands on ([`wire::Op::Facts`]): the reading a bot's `read` ends with, and the
/// chat's pull request in parts where that reading names one. The computer keeps what GitHub
/// said of a pull request for a while, so its parts cost no second look.
pub(super) async fn facts(backend: &Arc<dyn Backend>, chat_id: &str) -> Result<Data, BackendError> {
    let overview = backend.overview().await?;
    let chat = overview
        .chats
        .iter()
        .find(|entry| entry.chat.id == chat_id)
        .ok_or_else(no_chat)?;
    let facts = backend.facts(chat_id, &chat.chat.agent_id).await?;
    let pull_request = if facts.pull_request.is_some() {
        backend
            .pull_request(chat_id)
            .await
            .ok()
            .flatten()
            .map(pull_request_facts)
    } else {
        None
    };
    Ok(Data::Facts(Box::new(Facts {
        branch: facts.branch,
        worktree: facts.worktree,
        uncommitted: facts.uncommitted,
        usage: facts
            .usage
            .into_iter()
            .map(|(window, used)| Usage {
                window,
                used: if used.is_finite() {
                    used.clamp(0.0, 100.0)
                } else {
                    0.0
                },
            })
            .collect(),
        pull_request,
    })))
}

/// A pull request in parts: its newest [`wire::MAX_NOTES`] reviews and comments, each cut to
/// [`wire::MAX_NOTE`] characters, and no words of an author who is not one of the
/// repository's own people (anyone can comment on a public repository).
fn pull_request_facts(pr: PullRequest) -> PullRequestFacts {
    let failed = match &pr.checks {
        PrChecks::Failed(names) => names
            .iter()
            .take(wire::MAX_NOTES)
            .map(|name| render::cut(name, 200))
            .collect(),
        _ => Vec::new(),
    };
    let skip = pr.notes.len().saturating_sub(wire::MAX_NOTES);
    PullRequestFacts {
        number: pr.number,
        url: pr.url,
        state: pr.state,
        checks: pr.checks.word(),
        failed,
        notes: pr
            .notes
            .into_iter()
            .skip(skip)
            .map(|note| PullRequestNote {
                body: if note.trusted {
                    note.body.chars().take(wire::MAX_NOTE).collect()
                } else {
                    String::new()
                },
                author: note.author,
                kind: note.kind,
                trusted: note.trusted,
            })
            .collect(),
    }
}

/// A project's brief and its revision ([`wire::Op::Memory`]).
pub(super) async fn memory(
    backend: &Arc<dyn Backend>,
    project_id: String,
) -> Result<Data, BackendError> {
    match backend.act(Action::ReadMemory { project_id }).await? {
        Outcome::Memory { markdown, revision } => Ok(Data::Memory(Brief { markdown, revision })),
        _ => Err(unexpected()),
    }
}

/// The person's edit of a project's brief from this device ([`wire::Op::SetMemory`]): saved as
/// theirs, "Edited from" the device's name, and refused `conflict` when `base_revision` is no
/// longer the brief's. An empty `base_revision` says there was no brief when the device read it:
/// refused `conflict` once there is one.
pub(super) async fn set_memory(
    shared: &Shared,
    backend: &Arc<dyn Backend>,
    context: &Context,
    project_id: String,
    markdown: String,
    base_revision: Option<String>,
) -> Result<Data, BackendError> {
    if markdown.len() > wire::MAX_BRIEF {
        return Err(BackendError::new(
            "too_large",
            "A brief holds at most 4 KiB, because every chat of the project receives it.",
        ));
    }
    let base_revision = match base_revision {
        None => None,
        Some(revision) if revision.is_empty() => Some(None),
        Some(revision)
            if revision.len() > MAX_REVISION || revision.chars().any(char::is_control) =>
        {
            return Err(invalid("That is not a revision of this brief."));
        }
        Some(revision) => Some(Some(revision)),
    };
    let name = registry_here(shared, context)?
        .device_name(&context.device)
        .unwrap_or_else(|| "a paired device".into());
    match backend
        .act(Action::SaveMemory {
            project_id,
            markdown,
            base_revision,
            reason: format!("Edited from {name}"),
        })
        .await?
    {
        Outcome::Memory { revision, .. } => Ok(Data::SavedMemory { revision }),
        _ => Err(unexpected()),
    }
}

/// Clears a session's picture flag however its picture ends.
struct Picturing<'a>(&'a AtomicBool);

impl Drop for Picturing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// What a chat's browser tab or emulator shows ([`wire::Op::Picture`]), small enough that the
/// answer stays under [`wire::MAX_PICTURE`]. Only a chat of this workspace's overview is
/// pictured, and one picture at a time per session: each costs the computer a capture and a
/// resize.
pub(super) async fn picture(
    backend: &Arc<dyn Backend>,
    picturing: &AtomicBool,
    chat_id: &str,
    of: &str,
) -> Result<Data, BackendError> {
    let of = match of {
        "browser" => PictureOf::Browser,
        "device" => PictureOf::Device,
        _ => return Err(invalid("A picture is of a chat's browser or its device.")),
    };
    if picturing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(BackendError::new(
            "busy",
            "A picture is being taken already. Wait for it.",
        ));
    }
    let _picturing = Picturing(picturing);
    let overview = backend.overview().await?;
    if !overview.chats.iter().any(|entry| entry.chat.id == chat_id) {
        return Err(no_chat());
    }
    let picture = backend.picture_within(chat_id, of, PICTURE_DATA).await?;
    if picture.data.len() > PICTURE_DATA {
        return Err(BackendError::new(
            "too_large",
            "The picture is too large to send to this device.",
        ));
    }
    Ok(Data::Picture(picture))
}

/// What [`wire::Op::Task`] asks for.
pub(super) struct TaskAsked {
    pub(super) goal: String,
    pub(super) project_id: Option<String>,
    pub(super) agent_id: Option<String>,
    pub(super) review: Option<bool>,
    pub(super) when: Option<String>,
    pub(super) access: Option<String>,
    pub(super) attachments: Vec<String>,
}

/// A task the person hands over from this device ([`wire::Op::Task`]): now, as a bot's task
/// starts (a chat on its own branch, its brief, then a second agent's review) but set to the
/// access the person chose before the brief is sent, with their photos; or queued for tonight.
/// The task is the device's: the person may see it and its queue in [`tasks`].
pub(super) async fn task(
    shared: &Arc<Shared>,
    backend: &Arc<dyn Backend>,
    context: &Context,
    asked: TaskAsked,
) -> Result<Data, BackendError> {
    if asked.goal.trim().is_empty() {
        return Err(BackendError::new("empty", "Say what the task is."));
    }
    if asked.goal.len() > MAX_TEXT {
        return Err(BackendError::new("too_large", "The goal is too long."));
    }
    let tonight = match asked.when.as_deref() {
        None | Some("now") => false,
        Some("tonight") => true,
        Some(_) => return Err(invalid("when is now or tonight.")),
    };
    if let Some(access) = &asked.access
        && !ACCESS.contains(&access.as_str())
    {
        return Err(invalid("The access level is ask, auto_edit or full_auto."));
    }
    for name in [&asked.project_id, &asked.agent_id].into_iter().flatten() {
        name_ok(name)?;
    }
    let photos: &[String] = if context.offered(wire::ATTACHMENTS) {
        &asked.attachments
    } else {
        &[]
    };
    if tonight && !photos.is_empty() {
        return Err(invalid("Photos go with a task that starts now."));
    }
    photos_ok("", photos)?;
    let review = Some(asked.review.unwrap_or(true));
    let failed = |answer: crate::bot::Answer| BackendError::new("failed", answer.text);
    if tonight {
        let later = super::DeviceLater {
            shared: shared.clone(),
            device: context.device.clone(),
            scope: context.scope.clone(),
        };
        let (id, _) = crate::bot::queue_task(
            backend,
            &asked.goal,
            asked.project_id.as_deref(),
            asked.agent_id.as_deref(),
            review,
            asked.access,
            &later,
        )
        .await
        .map_err(failed)?;
        return Ok(Data::Queued { queued: id });
    }
    // The photos are measured before the task's chat, its worktree and its branch are made.
    let held = Held::take(&context.uploads, photos)?;
    let images = prepare(backend, &held.images).await?;
    let started = crate::bot::start_task(
        backend,
        &asked.goal,
        asked.project_id.as_deref(),
        asked.agent_id.as_deref(),
        review,
        crate::bot::Handover {
            by: crate::bot::HandedBy::Phone,
            most: crate::bot::Most::Anything,
            access: asked.access.as_deref(),
            images: &images,
        },
    )
    .await
    .map_err(failed)?;
    held.sent();
    let mut task = started.task;
    task.bot = context.device.clone();
    task.by = By::Device;
    task.created_at = now();
    let started = TaskStarted {
        chat_id: started.chat,
        branch: task.branch.clone(),
        review_agent: task.review_agent.clone(),
    };
    // Kept for its review while this session's workspace is still the open one; the chat itself
    // was made in it.
    if let Ok(mut registry) = registry_here(shared, context) {
        registry.add_task(task);
    }
    Ok(Data::Started(started))
}

/// The tasks handed over lately in this workspace, by its links and its devices, newest first;
/// what waits for tonight, oldest first; and the night's limits ([`wire::Op::Tasks`]). Each goal
/// is shown by its first [`GOAL_SHOWN`] characters.
pub(super) fn tasks(shared: &Shared, context: &Context) -> Result<Data, BackendError> {
    let at = now();
    let registry = registry_here(shared, context)?;
    let tasks = registry
        .tasks_here()
        .into_iter()
        .rev()
        .take(wire::MAX_TASKS)
        .map(|task| TaskEntry {
            chat_id: task.builder,
            goal: task.goal.chars().take(GOAL_SHOWN).collect(),
            branch: task.branch,
            review_agent: task.review_agent,
            reviewer_chat: task.reviewer,
            created_at: task.created_at,
            by: task.by.word(),
        })
        .collect();
    let queued = registry
        .queued_here(at)
        .into_iter()
        .take(MAX_QUEUED_LISTED)
        .map(|item| QueuedEntry {
            id: item.id,
            goal: item.goal.chars().take(GOAL_SHOWN).collect(),
            project_id: item.project,
            agent_id: item.agent,
            review: item.review,
            created_at: item.created_at,
            failed: item.failed,
            failed_at: item.failed_at,
            by: item.by.word(),
        })
        .collect();
    let night = Night {
        from: crate::bot::NIGHT_HOURS.start,
        to: crate::bot::NIGHT_HOURS.end,
        budget: crate::bot::NIGHT_BUDGET,
        started: registry.started_tonight(at),
    };
    drop(registry);
    let mut list = TaskList {
        tasks,
        queued,
        night,
    };
    // When the whole list would not fit one message, each goal is listed by its start, which is
    // what a list shows.
    if encoded(&list) > ANSWER_BYTES / 2 {
        for task in &mut list.tasks {
            task.goal = render::cut(&task.goal, GOAL_LISTED);
        }
        for item in &mut list.queued {
            item.goal = render::cut(&item.goal, GOAL_LISTED);
        }
    }
    Ok(Data::Tasks(Box::new(list)))
}

/// Takes a task off the queue for tonight, whoever queued it ([`wire::Op::TaskCancel`]).
pub(super) fn task_cancel(
    shared: &Shared,
    context: &Context,
    queued_id: &str,
) -> Result<Data, BackendError> {
    if registry_here(shared, context)?.cancel_any_queued(queued_id) {
        Ok(Data::None {})
    } else {
        Err(BackendError::new(
            "not_found",
            "Nothing is queued for tonight under that id.",
        ))
    }
}

/// An image of a chat, as the window's preview reads it ([`wire::Op::Image`]). Only what the
/// chat holds: a stored image, an attachment or a message's image, never a file by its path.
pub(super) async fn image(
    backend: &Arc<dyn Backend>,
    chat_id: &str,
    source: serde_json::Value,
    full: bool,
) -> Result<Data, BackendError> {
    let source: ChatImageSource = serde_json::from_value(source)
        .map_err(|_| invalid("That is not an image of this chat."))?;
    if matches!(source, ChatImageSource::Project { .. }) {
        return Err(invalid(
            "An image comes from the chat's own messages and attachments, never a file by its path.",
        ));
    }
    let preview = backend.image(chat_id, source, full, PICTURE_DATA).await?;
    if preview.data.len() > PICTURE_DATA {
        return Err(BackendError::new(
            "too_large",
            "The image is too large to send to this device.",
        ));
    }
    Ok(Data::Image(preview))
}

/// The app on this session's device started, sent to or answered `chat_id` (`answered`: it
/// answered what the chat asked). Only for a session offered notifications: what the device
/// acted on is what "mine" means to it.
pub(super) fn acted(shared: &Shared, context: &Context, chat_id: &str, answered: bool) {
    if context.offered(wire::PUSH) {
        super::notify::acted(shared, &context.device, &context.scope, chat_id, answered);
    }
}

/// A phone's address at Apple and which of Apple's services it belongs to, as the app gives
/// them: 64 to 200 hex characters, kept in lower case, and `sandbox` or `production`.
fn address(token: &str, env: &str) -> Result<(String, Env), BackendError> {
    if !crate::push::token_ok(token) {
        return Err(invalid(
            "That is not a notification token: 64 to 200 hex characters.",
        ));
    }
    let env = Env::parse(env).ok_or_else(|| invalid("The service is sandbox or production."))?;
    Ok((token.to_ascii_lowercase(), env))
}

/// The device went in the moment between: its session ends at its next request.
fn gone() -> BackendError {
    BackendError::new("not_found", "This device is no longer paired here.")
}

fn unsaved(error: crate::registry::RegistryError) -> BackendError {
    BackendError::new("failed", error.to_string())
}

/// Where Apple delivers this device's notifications, and what it wants told
/// ([`wire::Op::PushRegister`]): everything, of every chat, unless it says otherwise. Answers
/// whether the relay in front of this computer sends notifications now.
pub(super) fn push_register(
    shared: &Shared,
    context: &Context,
    token: &str,
    env: &str,
    prefs: Option<wire::PushPrefs>,
) -> Result<Data, BackendError> {
    let (token, env) = address(token, env)?;
    let prefs = match prefs {
        None => Prefs::default(),
        Some(asked) => Prefs {
            asking: asked.asking,
            finished: asked.finished,
            failed: asked.failed,
            tasks: asked.tasks,
            scope: match asked.scope.as_deref() {
                None => Scope::All,
                Some(scope) => {
                    Scope::parse(scope).ok_or_else(|| invalid("The scope is all or mine."))?
                }
            },
        },
    };
    let push = Push {
        token,
        env,
        prefs,
        since: now(),
    };
    if !registry_here(shared, context)?
        .set_device_push(&context.device, push)
        .map_err(unsaved)?
    {
        return Err(gone());
    }
    // The call-back's watch starts now, if this is the first device it tells.
    shared.tell_wake.notify_one();
    (shared.config.changed)();
    Ok(Data::Registered {
        relay: super::notify::relays(shared),
    })
}

/// This device is told nothing more ([`wire::Op::PushUnregister`]).
pub(super) fn push_unregister(shared: &Shared, context: &Context) -> Result<Data, BackendError> {
    if !registry_here(shared, context)?
        .clear_device_push(&context.device)
        .map_err(unsaved)?
    {
        return Err(gone());
    }
    super::notify::forget(shared, &context.device);
    (shared.config.changed)();
    Ok(Data::None {})
}

/// A test notification to this device now ([`wire::Op::PushTest`]): at most one every ten
/// seconds, else `busy`.
pub(super) fn push_test(shared: &Shared, context: &Context) -> Result<Data, BackendError> {
    // Only while this session's workspace is still the open one.
    drop(registry_here(shared, context)?);
    super::notify::test(shared, &context.device)
        .map_err(|why| BackendError::new(why.code(), why.message()))?;
    Ok(Data::None {})
}

/// Where Apple delivers the updates of this computer's Live Activity on this device
/// ([`wire::Op::Activity`]).
pub(super) fn activity(
    shared: &Shared,
    context: &Context,
    token: &str,
    env: &str,
) -> Result<Data, BackendError> {
    let (token, env) = address(token, env)?;
    let activity = ActivityToken {
        token,
        env,
        since: now(),
    };
    if !registry_here(shared, context)?
        .set_device_activity(&context.device, activity)
        .map_err(unsaved)?
    {
        return Err(gone());
    }
    shared.tell_wake.notify_one();
    Ok(Data::None {})
}

/// A piece of a staged photo ([`wire::Op::UploadBegin`], [`wire::Op::UploadChunk`],
/// [`wire::Op::UploadAbort`]), taken at once and in order as it arrives; `None` for any other
/// request.
pub(super) fn upload(uploads: &Arc<Mutex<Uploads>>, op: &Op) -> Option<Result<Data, BackendError>> {
    let now = std::time::Instant::now();
    Some(match op {
        Op::UploadBegin {
            name,
            mime,
            size,
            sha256,
        } => lock(uploads)
            .begin(name, mime, *size, sha256, now)
            .map(|upload_id| Data::UploadBegun {
                upload_id,
                chunk: wire::UPLOAD_CHUNK,
            }),
        Op::UploadChunk {
            upload_id,
            index,
            data,
        } => lock(uploads)
            .chunk(upload_id, *index, data, now)
            .map(|(received, complete)| Data::UploadProgress { received, complete }),
        Op::UploadAbort { upload_id } => {
            lock(uploads).abort(upload_id);
            Ok(Data::None {})
        }
        _ => return None,
    })
}
