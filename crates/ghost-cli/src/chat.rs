//! One chat a probe or a smoke run drives through the session manager, the way the app's buttons
//! do: created in its folder (which starts the agent), started, stopped, interrupted and prompted,
//! with every frame and status update seen through a [`Watch`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ghost_agents::AgentSpec;
use ghost_core::session::{FrameSink, SessionError, SinkGone};
use ghost_protocol::session::{ChatSnapshot, PromptAck, SessionLists};
use ghost_protocol::{
    EventBatch, PermissionOptionKind, PermissionRequest, SessionInfo, SessionStatus,
};

use crate::headless::Headless;
use crate::logs;
use crate::watch::{TurnView, Watch};

/// How a start ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartEnd {
    Ready,
    Failed,
    AuthRequired,
    /// Stopped before it was ready (an interrupt, or a crash while starting).
    Stopped,
    TimedOut,
}

/// What a prompted turn did next.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnNext {
    /// A permission card waits for an answer.
    Card(PermissionRequest),
    Ended(TurnView),
    TimedOut,
}

/// A chat and its view.
pub struct ChatRun<'a> {
    pub headless: &'a Headless,
    pub watch: Arc<Watch>,
    pub chat_id: String,
    pub spec: AgentSpec,
    pub folder: PathBuf,
}

impl<'a> ChatRun<'a> {
    /// Creates the chat in `folder`, which starts its agent, and attaches its view. The status
    /// channel must already be subscribed, so the start's first update is seen.
    pub async fn create(
        headless: &'a Headless,
        watch: &Arc<Watch>,
        spec: &AgentSpec,
        folder: &Path,
    ) -> Result<ChatRun<'a>, SessionError> {
        let chat = headless.manager.create_chat(spec.id, folder).await?;
        watch.attach(&headless.manager, &chat.id).await?;
        Ok(ChatRun {
            headless,
            watch: Arc::clone(watch),
            chat_id: chat.id,
            spec: spec.clone(),
            folder: folder.to_path_buf(),
        })
    }

    pub fn status(&self) -> Option<SessionStatus> {
        self.watch.with(|state| state.status(&self.chat_id))
    }

    pub fn info(&self) -> Option<SessionInfo> {
        self.watch.with(|state| state.info(&self.chat_id).cloned())
    }

    /// The header state once it reads `Ready`: the status channel can report a start's end
    /// before the frame with the new header state (and its native session id) arrives.
    pub async fn ready_info(&self, limit: Duration) -> Option<SessionInfo> {
        self.watch
            .until(limit, |state| {
                state
                    .info(&self.chat_id)
                    .filter(|info| info.status == SessionStatus::Ready)
                    .cloned()
            })
            .await
    }

    /// A mark in the events seen so far.
    pub fn mark(&self) -> usize {
        self.watch.with(|state| state.mark())
    }

    /// Waits until the start in progress ends. A permission card that shows before the agent is
    /// ready (a question about the folder, say) is answered "cancelled", so it can never write a
    /// lasting rule, and returned.
    pub async fn settle(&self, limit: Duration) -> (StartEnd, Vec<PermissionRequest>) {
        let deadline = tokio::time::Instant::now() + limit;
        let mut answered: Vec<PermissionRequest> = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let next = self
                .watch
                .until(left, |state| {
                    match state.status(&self.chat_id) {
                        Some(SessionStatus::Ready) => return Some(Ok(StartEnd::Ready)),
                        Some(SessionStatus::Failed) => return Some(Ok(StartEnd::Failed)),
                        Some(SessionStatus::AuthRequired) => {
                            return Some(Ok(StartEnd::AuthRequired));
                        }
                        Some(SessionStatus::Stopped) => return Some(Ok(StartEnd::Stopped)),
                        _ => {}
                    }
                    state
                        .open_cards(&self.chat_id)
                        .into_iter()
                        .find(|card| {
                            !answered
                                .iter()
                                .any(|done| done.request_id == card.request_id)
                        })
                        .map(Err)
                })
                .await;
            match next {
                Some(Ok(end)) => return (end, answered),
                Some(Err(card)) => {
                    let _ = self
                        .headless
                        .manager
                        .respond_permission(&self.chat_id, &card.request_id, None)
                        .await;
                    answered.push(card);
                }
                None => return (StartEnd::TimedOut, answered),
            }
        }
    }

    /// Starts the agent again (resuming its native session) and waits for the start to end.
    pub async fn start(
        &self,
        limit: Duration,
    ) -> Result<(StartEnd, Vec<PermissionRequest>), SessionError> {
        self.headless.manager.start(&self.chat_id, false).await?;
        Ok(self.settle(limit).await)
    }

    /// Stops the agent and waits until the chat reads `Stopped`. A chat that could not start or
    /// that waits for a sign-in has no process (spec 6.5.1) and counts as stopped as it is.
    pub async fn stop(&self, limit: Duration) -> Result<bool, SessionError> {
        if matches!(
            self.status(),
            Some(SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired)
        ) {
            return Ok(true);
        }
        self.headless.manager.stop(&self.chat_id).await?;
        Ok(self.wait_status(SessionStatus::Stopped, limit).await)
    }

    /// Waits until the chat reads `status`.
    pub async fn wait_status(&self, status: SessionStatus, limit: Duration) -> bool {
        self.watch
            .until(limit, |state| {
                (state.status(&self.chat_id) == Some(status)).then_some(())
            })
            .await
            .is_some()
    }

    pub async fn lists(&self) -> Result<SessionLists, SessionError> {
        self.headless.manager.lists(&self.chat_id).await
    }

    /// The chat's snapshot as a newly opened view would get it: the last error, the auth card,
    /// the queue. Read through a second, short-lived view.
    pub async fn snapshot(&self) -> Result<ChatSnapshot, SessionError> {
        let attach = self
            .headless
            .manager
            .attach(&self.chat_id, Arc::new(Peek))
            .await?;
        self.headless.manager.detach(&attach.subscription_id);
        Ok(attach.snapshot)
    }

    /// The chat's protocol logs, oldest spawn first.
    pub fn logs(&self) -> Vec<PathBuf> {
        logs::list(&self.headless.log_dir(&self.chat_id))
    }

    pub async fn send(&self, text: &str) -> Result<PromptAck, SessionError> {
        self.headless
            .manager
            .send(&self.chat_id, text.to_owned())
            .await
    }

    /// Waits for the turn a prompt started.
    pub async fn turn_started(&self, prompt_id: &str, limit: Duration) -> Option<String> {
        self.watch
            .until(limit, |state| {
                state.turn_of_prompt(&self.chat_id, prompt_id)
            })
            .await
    }

    /// Waits until the turn ended.
    pub async fn turn_ended(&self, turn_id: &str, limit: Duration) -> Option<TurnView> {
        self.watch
            .until(limit, |state| {
                state
                    .turn(&self.chat_id, turn_id)
                    .filter(|turn| turn.ended.is_some())
            })
            .await
    }

    /// Waits for whichever comes first: a card of the turn that nobody answered (`answered`
    /// lists the ones answered already, whose resolution may still be on its way), or the turn's
    /// end.
    pub async fn turn_next(&self, turn_id: &str, answered: &[String], limit: Duration) -> TurnNext {
        self.watch
            .until(limit, |state| {
                let turn = state.turn(&self.chat_id, turn_id)?;
                if turn.ended.is_some() {
                    return Some(TurnNext::Ended(turn));
                }
                turn.requests
                    .iter()
                    .find(|request| {
                        turn.resolved_by(&request.request_id).is_none()
                            && !answered.contains(&request.request_id)
                    })
                    .cloned()
                    .map(TurnNext::Card)
            })
            .await
            .unwrap_or(TurnNext::TimedOut)
    }

    /// Runs `work` (an option or autonomy change) while answering every card that shows meanwhile
    /// (Hermes asks to confirm a guarded model while `session/set_model` waits): "allow once" when
    /// the card offers it, else cancelled. Returns the work's result and the cards answered.
    pub async fn answering<T>(&self, work: impl Future<Output = T>) -> (T, Vec<PermissionRequest>) {
        tokio::pin!(work);
        let mut answered: Vec<PermissionRequest> = Vec::new();
        loop {
            let known: Vec<String> = answered
                .iter()
                .map(|card| card.request_id.clone())
                .collect();
            let card = self
                .watch
                .until(Duration::from_secs(24 * 60 * 60), move |state| {
                    state
                        .open_cards(&self.chat_id)
                        .into_iter()
                        .find(|card| !known.contains(&card.request_id))
                });
            tokio::select! {
                result = &mut work => return (result, answered),
                Some(card) = card => {
                    let allow = card
                        .options
                        .iter()
                        .find(|option| option.kind == PermissionOptionKind::AllowOnce)
                        .map(|option| option.option_id.clone());
                    let _ = self
                        .headless
                        .manager
                        .respond_permission(&self.chat_id, &card.request_id, allow)
                        .await;
                    answered.push(card);
                }
            }
        }
    }

    /// Waits until the turn's reply has begun.
    pub async fn first_delta(&self, turn_id: &str, limit: Duration) -> Option<TurnView> {
        self.watch
            .until(limit, |state| {
                state
                    .turn(&self.chat_id, turn_id)
                    .filter(|turn| !turn.reply.is_empty() || turn.ended.is_some())
            })
            .await
    }
}

/// A view that only reads a snapshot.
struct Peek;

impl FrameSink for Peek {
    fn send(&self, _frame: &EventBatch) -> Result<(), SinkGone> {
        Err(SinkGone)
    }

    fn owner(&self) -> &str {
        "ghost-cli-peek"
    }
}
