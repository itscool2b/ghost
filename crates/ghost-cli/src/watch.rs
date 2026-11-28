//! What the CLI sees of its chats, through the same channels the app's views use: one chat view
//! per chat (its history page, then every frame) and the status updates of every chat. Everything
//! is kept in arrival order, and steps wait on it with a time limit.
//!
//! Turns are read the way the UI reads them (spec 7.4): by position, from a `TurnStarted` to its
//! `TurnEnded`, since live frames carry no turn tags.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ghost_core::session::{FrameSink, SessionError, SessionManager, SinkGone, StatusSink};
use ghost_protocol::session::{ChatAttach, ChatStatusUpdate};
use ghost_protocol::{
    ErrorKind, EventBatch, GhostEvent, NoticeLevel, PermissionDecision, PermissionRequest,
    ResolvedBy, SessionInfo, SessionStatus, StopReason, TokenUsage, ToolStatus,
};
use tokio::sync::Notify;

/// The webview label the CLI's subscriptions carry.
pub const OWNER: &str = "ghost-cli";

/// One event the CLI saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub chat_id: String,
    pub seq: u32,
    /// The frame it came in, counted from 1 in arrival order; 0 for the attach's history page.
    pub frame: u64,
    pub at: Instant,
    pub event: GhostEvent,
}

/// Everything seen so far.
#[derive(Debug, Default)]
pub struct WatchState {
    pub events: Vec<Seen>,
    statuses: HashMap<String, ChatStatusUpdate>,
    infos: HashMap<String, SessionInfo>,
    /// Every status detail a chat showed, and when it showed.
    pub details: Vec<(String, Instant, Option<String>)>,
    frames: u64,
}

/// A chat's view and status channel.
#[derive(Debug, Default)]
pub struct Watch {
    state: Mutex<WatchState>,
    changed: Notify,
}

impl Watch {
    pub fn new() -> Arc<Watch> {
        Arc::new(Watch::default())
    }

    fn state(&self) -> MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads the state.
    pub fn with<T>(&self, read: impl FnOnce(&WatchState) -> T) -> T {
        read(&self.state())
    }

    /// The status channel to subscribe (`SessionManager::status_subscribe`).
    pub fn status_sink(self: &Arc<Self>) -> Arc<dyn StatusSink> {
        Arc::new(StatusTap(Arc::clone(self)))
    }

    /// Attaches a view of `chat_id`: its history page goes in first, then every frame.
    pub async fn attach(
        self: &Arc<Self>,
        manager: &SessionManager,
        chat_id: &str,
    ) -> Result<ChatAttach, SessionError> {
        let attach = manager
            .attach(
                chat_id,
                Arc::new(FrameTap {
                    watch: Arc::clone(self),
                }),
            )
            .await?;
        {
            let mut state = self.state();
            let now = Instant::now();
            // Frames that arrived before the reply carry seqs above the page's; the page goes
            // before them.
            let page: Vec<Seen> = attach
                .history
                .items
                .iter()
                .map(|item| Seen {
                    chat_id: chat_id.to_owned(),
                    seq: item.seq,
                    frame: 0,
                    at: now,
                    event: item.event.clone(),
                })
                .collect();
            let first_live = state
                .events
                .iter()
                .position(|seen| seen.chat_id == chat_id)
                .unwrap_or(state.events.len());
            state.events.splice(first_live..first_live, page);
            state.attach_info(
                chat_id,
                attach.next_seq,
                attach.snapshot.session.clone(),
                now,
            );
        }
        self.changed.notify_waiters();
        Ok(attach)
    }

    /// Waits up to `limit` for `check` to find something in the state; `None` when the time ran
    /// out first.
    pub async fn until<T>(
        &self,
        limit: Duration,
        mut check: impl FnMut(&WatchState) -> Option<T>,
    ) -> Option<T> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(found) = check(&self.state()) {
                return Some(found);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return check(&self.state());
            }
        }
    }

    fn push_frame(&self, frame: &EventBatch) {
        {
            let mut state = self.state();
            state.frames += 1;
            let number = state.frames;
            let now = Instant::now();
            for (seq, event) in (frame.seq_start..).zip(&frame.events) {
                if let GhostEvent::SessionUpdated(info) = event {
                    state.set_info(&frame.chat_id, info.clone(), now);
                }
                state.events.push(Seen {
                    chat_id: frame.chat_id.clone(),
                    seq,
                    frame: number,
                    at: now,
                    event: event.clone(),
                });
            }
        }
        self.changed.notify_waiters();
    }

    fn push_status(&self, update: &ChatStatusUpdate) {
        self.state()
            .statuses
            .insert(update.chat_id.clone(), update.clone());
        self.changed.notify_waiters();
    }
}

impl WatchState {
    /// A live header can arrive while the attach's saved history page is being read. The
    /// snapshot describes the subscription boundary, so preserve a header at or after it.
    fn attach_info(&mut self, chat_id: &str, next_seq: u32, snapshot: SessionInfo, at: Instant) {
        let later = self.events.iter().rev().find_map(|seen| {
            if seen.chat_id == chat_id
                && seen.frame > 0
                && seen.seq >= next_seq
                && let GhostEvent::SessionUpdated(info) = &seen.event
            {
                return Some(info.clone());
            }
            None
        });
        self.set_info(chat_id, later.unwrap_or(snapshot), at);
    }

    fn set_info(&mut self, chat_id: &str, info: SessionInfo, at: Instant) {
        let previous = self.infos.get(chat_id).map(|known| known.detail.clone());
        if previous.as_ref() != Some(&info.detail) {
            self.details
                .push((chat_id.to_owned(), at, info.detail.clone()));
        }
        self.infos.insert(chat_id.to_owned(), info);
    }

    /// The chat's status, as its last status update said.
    pub fn status(&self, chat_id: &str) -> Option<SessionStatus> {
        self.statuses.get(chat_id).map(|update| update.status)
    }

    /// The chat's header state, as its last `SessionUpdated` (or the attach's snapshot) said.
    pub fn info(&self, chat_id: &str) -> Option<&SessionInfo> {
        self.infos.get(chat_id)
    }

    /// How many events were seen so far: a mark for [`WatchState::since`].
    pub fn mark(&self) -> usize {
        self.events.len()
    }

    /// The chat's events seen after `mark`.
    pub fn since<'a>(&'a self, chat_id: &'a str, mark: usize) -> impl Iterator<Item = &'a Seen> {
        self.events
            .get(mark..)
            .unwrap_or_default()
            .iter()
            .filter(move |seen| seen.chat_id == chat_id)
    }

    /// The chat's events.
    pub fn of<'a>(&'a self, chat_id: &'a str) -> impl Iterator<Item = &'a Seen> {
        self.since(chat_id, 0)
    }

    /// The turn a prompt started.
    pub fn turn_of_prompt(&self, chat_id: &str, prompt_id: &str) -> Option<String> {
        self.of(chat_id).find_map(|seen| match &seen.event {
            GhostEvent::TurnStarted {
                turn_id,
                prompt_id: Some(started),
            } if started == prompt_id => Some(turn_id.clone()),
            _ => None,
        })
    }

    /// A turn as seen so far: its events from `TurnStarted` to `TurnEnded`.
    pub fn turn(&self, chat_id: &str, turn_id: &str) -> Option<TurnView> {
        let mut events = self.of(chat_id).skip_while(|seen| {
            !matches!(&seen.event, GhostEvent::TurnStarted { turn_id: started, .. } if started == turn_id)
        });
        let first = events.next()?;
        let mut view = TurnView {
            turn_id: turn_id.to_owned(),
            ..TurnView::default()
        };
        if let GhostEvent::TurnStarted { prompt_id, .. } = &first.event {
            view.prompt_id = prompt_id.clone();
        }
        let mut delta_frames = Vec::new();
        for seen in events {
            match &seen.event {
                GhostEvent::UserMessageDelta { text, .. } => view.user.push_str(text),
                GhostEvent::MessageDelta { text, .. } => {
                    view.reply.push_str(text);
                    if !delta_frames.contains(&seen.frame) {
                        delta_frames.push(seen.frame);
                    }
                    view.first_delta.get_or_insert(seen.at);
                }
                GhostEvent::ToolCall(call) => {
                    view.tools.insert(call.tool_call_id.clone(), call.status);
                }
                GhostEvent::ToolUpdate(update) => {
                    if let Some(status) = update.status {
                        view.tools.insert(update.tool_call_id.clone(), status);
                    }
                }
                GhostEvent::PermissionRequested(request) => view.requests.push(request.clone()),
                GhostEvent::PermissionResolved {
                    request_id,
                    decision,
                    by,
                } => view
                    .resolutions
                    .push((request_id.clone(), decision.clone(), *by)),
                GhostEvent::Notice { level, message } => {
                    view.notices.push((*level, message.clone()))
                }
                GhostEvent::Error { kind, message, .. } => {
                    view.errors.push((*kind, message.clone()))
                }
                GhostEvent::TurnEnded {
                    turn_id: ended,
                    stop_reason,
                    usage,
                    ..
                } if ended == turn_id => {
                    view.ended = Some((*stop_reason, usage.clone()));
                    break;
                }
                _ => {}
            }
        }
        view.delta_frames = delta_frames.len();
        Some(view)
    }

    /// The chat's permission cards nobody answered yet, oldest first.
    pub fn open_cards(&self, chat_id: &str) -> Vec<PermissionRequest> {
        let mut open: Vec<PermissionRequest> = Vec::new();
        for seen in self.of(chat_id) {
            match &seen.event {
                GhostEvent::PermissionRequested(request) => open.push(request.clone()),
                GhostEvent::PermissionResolved { request_id, .. } => {
                    open.retain(|request| &request.request_id != request_id);
                }
                _ => {}
            }
        }
        open
    }

    /// The chat's notices seen after `mark`.
    pub fn notices_since(&self, chat_id: &str, mark: usize) -> Vec<String> {
        self.since(chat_id, mark)
            .filter_map(|seen| match &seen.event {
                GhostEvent::Notice { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    /// The chat's error cards seen after `mark`.
    pub fn errors_since(
        &self,
        chat_id: &str,
        mark: usize,
    ) -> Vec<(ErrorKind, String, Option<String>)> {
        self.since(chat_id, mark)
            .filter_map(|seen| match &seen.event {
                GhostEvent::Error {
                    kind,
                    message,
                    detail,
                    ..
                } => Some((*kind, message.clone(), detail.clone())),
                _ => None,
            })
            .collect()
    }
}

/// A turn reduced to what the checks read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnView {
    pub turn_id: String,
    pub prompt_id: Option<String>,
    pub user: String,
    pub reply: String,
    /// How many frames carried the reply: more than one is a reply that streamed.
    pub delta_frames: usize,
    /// When the first piece of the reply arrived.
    pub first_delta: Option<Instant>,
    /// Each tool's last status.
    pub tools: BTreeMap<String, ToolStatus>,
    pub requests: Vec<PermissionRequest>,
    pub resolutions: Vec<(String, PermissionDecision, ResolvedBy)>,
    pub notices: Vec<(NoticeLevel, String)>,
    pub errors: Vec<(ErrorKind, String)>,
    /// How the turn ended, once it did.
    pub ended: Option<(StopReason, Option<TokenUsage>)>,
}

impl TurnView {
    /// Who answered the request, if anyone did.
    pub fn resolved_by(&self, request_id: &str) -> Option<ResolvedBy> {
        self.resolutions
            .iter()
            .find(|(id, _, _)| id == request_id)
            .map(|(_, _, by)| *by)
    }
}

struct FrameTap {
    watch: Arc<Watch>,
}

impl FrameSink for FrameTap {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        self.watch.push_frame(frame);
        Ok(())
    }

    fn owner(&self) -> &str {
        OWNER
    }
}

struct StatusTap(Arc<Watch>);

impl StatusSink for StatusTap {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        self.0.push_status(update);
        Ok(())
    }

    fn owner(&self) -> &str {
        OWNER
    }
}

#[cfg(test)]
mod tests {
    use ghost_protocol::{PermissionOption, PermissionOptionKind, ToolCall, ToolKind, UsageScope};

    use super::*;

    fn frame(watch: &Watch, seq: u32, events: Vec<GhostEvent>) {
        watch.push_frame(&EventBatch {
            chat_id: "c".into(),
            seq_start: seq,
            events,
        });
    }

    fn request(id: &str) -> PermissionRequest {
        PermissionRequest {
            request_id: id.into(),
            tool_call_id: None,
            title: "Write a file".into(),
            kind: Some(ToolKind::Edit),
            raw_input: None,
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![PermissionOption {
                option_id: "allow".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
            truncated: None,
        }
    }

    #[test]
    fn attach_snapshot_does_not_replace_a_newer_live_ready_header() {
        let watch = Watch::default();
        let ready = SessionInfo {
            status: SessionStatus::Ready,
            native_session_id: Some("native-session".into()),
            ..SessionInfo::default()
        };
        frame(&watch, 10, vec![GhostEvent::SessionUpdated(ready.clone())]);
        watch.state().attach_info(
            "c",
            10,
            SessionInfo {
                status: SessionStatus::Starting,
                ..SessionInfo::default()
            },
            Instant::now(),
        );
        assert_eq!(watch.with(|state| state.info("c").cloned()), Some(ready));
    }

    #[test]
    fn a_new_attach_snapshot_replaces_headers_before_its_boundary() {
        let watch = Watch::default();
        frame(
            &watch,
            10,
            vec![GhostEvent::SessionUpdated(SessionInfo {
                status: SessionStatus::Ready,
                ..SessionInfo::default()
            })],
        );
        let stopped = SessionInfo::default();
        watch
            .state()
            .attach_info("c", 11, stopped.clone(), Instant::now());
        assert_eq!(watch.with(|state| state.info("c").cloned()), Some(stopped));
    }

    #[test]
    fn a_turn_reads_from_its_start_to_its_end_across_frames() {
        let watch = Watch::default();
        frame(
            &watch,
            0,
            vec![
                GhostEvent::TurnStarted {
                    turn_id: "T".into(),
                    prompt_id: Some("P".into()),
                },
                GhostEvent::UserMessageDelta {
                    message_id: "T.u".into(),
                    text: "hi".into(),
                },
                GhostEvent::MessageDelta {
                    message_id: "T.m1".into(),
                    text: "o".into(),
                },
            ],
        );
        frame(
            &watch,
            3,
            vec![
                GhostEvent::MessageDelta {
                    message_id: "T.m1".into(),
                    text: "k".into(),
                },
                GhostEvent::ToolCall(ToolCall {
                    tool_call_id: "T.t".into(),
                    kind: ToolKind::Edit,
                    title: "Edit".into(),
                    status: ToolStatus::Pending,
                    locations: Vec::new(),
                    raw_input: None,
                    tool_name: None,
                    content: Vec::new(),
                    truncated: None,
                }),
                GhostEvent::PermissionRequested(request("T.p1")),
            ],
        );
        let state_open = watch.with(|state| state.open_cards("c"));
        assert_eq!(state_open.len(), 1);
        frame(
            &watch,
            6,
            vec![
                GhostEvent::PermissionResolved {
                    request_id: "T.p1".into(),
                    decision: PermissionDecision::Selected {
                        option_id: "allow".into(),
                        plan: None,
                        note: None,
                    },
                    by: ResolvedBy::User,
                },
                GhostEvent::TurnEnded {
                    turn_id: "T".into(),
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                        reasoning_tokens: None,
                        scope: UsageScope::Turn,
                    }),
                    duration_ms: Some(5),
                    recovered: false,
                },
                GhostEvent::MessageDelta {
                    message_id: "late".into(),
                    text: "after the end".into(),
                },
            ],
        );
        let turn = watch
            .with(|state| {
                assert_eq!(state.turn_of_prompt("c", "P").as_deref(), Some("T"));
                assert!(state.open_cards("c").is_empty());
                state.turn("c", "T")
            })
            .expect("the turn");
        assert_eq!(turn.user, "hi");
        assert_eq!(
            turn.reply, "ok",
            "what came after its end is not the turn's"
        );
        assert_eq!(turn.delta_frames, 2);
        assert_eq!(turn.tools.get("T.t"), Some(&ToolStatus::Pending));
        assert_eq!(turn.resolved_by("T.p1"), Some(ResolvedBy::User));
        assert!(matches!(turn.ended, Some((StopReason::EndTurn, Some(_)))));
        assert_eq!(watch.with(|state| state.turn("c", "other")), None);
    }

    #[tokio::test]
    async fn until_wakes_on_a_change_and_gives_up_at_its_limit() {
        let watch = Watch::new();
        let waiter = Arc::clone(&watch);
        let waiting = tokio::spawn(async move {
            waiter
                .until(Duration::from_secs(10), |state| state.status("c"))
                .await
        });
        tokio::task::yield_now().await;
        watch.push_status(&ChatStatusUpdate {
            chat_id: "c".into(),
            status: SessionStatus::Ready,
            needs_attention: false,
            title: None,
            updated_at: 1,
        });
        assert_eq!(waiting.await.expect("join"), Some(SessionStatus::Ready));
        let none = watch
            .until(Duration::from_millis(20), |state| state.status("other"))
            .await;
        assert_eq!(none, None);
    }
}
