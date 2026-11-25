//! What o7 for iPhone asks for beyond a browser's requests ([`wire::APP_FEATURES`]). Each is one
//! call a session's `perform` makes through the session's own bound backend, and reads what the
//! bot tools read: nothing here reaches further than [`Backend`].

use super::*;
use crate::{
    render::{self, Waiting},
    wire::{Asked, Asking, Inbox, MAX_INBOX, Recent, SearchHit, SearchHits, Working},
};

/// What needs the person, as the app's Inbox shows it ([`wire::Op::Inbox`]): the requests chats
/// wait on, with what each asks and its choices; the chats working, with what each does now; and
/// the chats whose last turn ended in the last day, with what it came to. The chats are the ones
/// the bot's `inbox` reads ([`render::sorted`]).
///
/// It reads the overview, then the newest page of history of each chat it lists, as the bot's
/// `inbox` does for the chats it reads into: at most [`MAX_INBOX`] chats in each part, so at most
/// 120 pages, and in practice a few, since chats ask and work a few at a time.
pub(super) async fn inbox(backend: &Arc<dyn Backend>) -> Result<Data, BackendError> {
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
        inbox.recent.push(Recent {
            chat_id: entry.chat.id.clone(),
            ended_at,
            duration_ms: turn.duration_ms,
            line: render::brief(&entry.chat, &page.items),
        });
    }
    inbox
        .recent
        .sort_by_key(|recent| std::cmp::Reverse(recent.ended_at));
    Ok(Data::Inbox(Box::new(inbox)))
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
