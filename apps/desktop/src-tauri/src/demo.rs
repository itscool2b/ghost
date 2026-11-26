//! Demo streams: the scripted mock agent from ghost-drivers, played into a Tauri channel.
//!
//! This is the M0 stand-in for a real agent session and the load generator for the streaming stress
//! test. Each stream is one session with its own id; the registry only tracks how to cancel the
//! ones still running, and every stream removes itself when it ends, however it ends (completed,
//! cancelled, receiver gone, or a panic unwinding through it).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_drivers::mock::{MockScript, RunOutcome, run_script};
use ghost_protocol::EventBatch;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppErrorCode};
use crate::sync::lock;

/// One frame at 60 Hz: deltas that fall due within it reach the UI as a single batch.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Streams allowed at once. The UI runs one at a time; the cap keeps a buggy (or hostile) page from
/// piling up tasks.
pub const MAX_CONCURRENT_STREAMS: usize = 8;

/// Largest stress turn. The mock builds the whole answer up front, so this bounds memory (a few MB
/// of text) whatever the page asks for.
pub const MAX_STRESS_TOKENS: u32 = 500_000;

/// Every demo session plays exactly one turn.
const TURN_ID: &str = "turn-1";

/// Which scripted turn to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DemoKind {
    /// A realistic turn: a thought, a file read, an edit, a plan, a markdown answer, usage.
    Demo,
    /// A long, fast markdown answer of exactly `total_tokens` tokens, for the stress view.
    Stress { total_tokens: u32 },
}

impl DemoKind {
    /// The script for this kind at `tokens_per_sec` (clamped by the mock to 1..=5000).
    pub fn script(self, tokens_per_sec: u32) -> Result<MockScript, AppError> {
        match self {
            DemoKind::Demo => Ok(MockScript::demo_turn(TURN_ID, tokens_per_sec)),
            DemoKind::Stress { total_tokens } => {
                if total_tokens == 0 || total_tokens > MAX_STRESS_TOKENS {
                    return Err(AppError::invalid_argument(format!(
                        "A stress turn must have between 1 and {MAX_STRESS_TOKENS} tokens, not {total_tokens}."
                    )));
                }
                Ok(MockScript::stress(TURN_ID, tokens_per_sec, total_tokens))
            }
        }
    }
}

type Running = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// The demo streams that are still running, by session id.
#[derive(Debug, Clone, Default)]
pub struct DemoSessions {
    running: Running,
}

impl DemoSessions {
    /// Registers a new session and returns its id with the future that plays it. The caller spawns
    /// the future on a Tokio runtime with time enabled; `sink` gets every frame and returns `false`
    /// once its receiver is gone, which stops the stream.
    pub fn start<F>(
        &self,
        kind: DemoKind,
        tokens_per_sec: u32,
        sink: F,
    ) -> Result<(String, impl Future<Output = RunOutcome> + Send + 'static), AppError>
    where
        F: FnMut(EventBatch) -> bool + Send + 'static,
    {
        let script = kind.script(tokens_per_sec)?;
        let session_id = ghost_core::ids::new_id();
        let cancel = CancellationToken::new();
        {
            let mut running = lock(&self.running);
            if running.len() >= MAX_CONCURRENT_STREAMS {
                return Err(AppError::new(
                    AppErrorCode::TooManyStreams,
                    format!(
                        "{MAX_CONCURRENT_STREAMS} demo streams are already running. Stop one first."
                    ),
                ));
            }
            running.insert(session_id.clone(), cancel.clone());
        }
        let registration = Registration {
            running: Arc::clone(&self.running),
            session_id: session_id.clone(),
        };
        let id = session_id.clone();
        let run = async move {
            // Dropped when the run ends, including by unwinding, so the entry never leaks.
            let _registration = registration;
            let outcome = run_script(script, &id, FRAME_INTERVAL, cancel, sink).await;
            tracing::debug!(session_id = %id, ?outcome, "demo stream ended");
            outcome
        };
        Ok((session_id, run))
    }

    /// Cancels the session if it is still running. Unknown or finished ids are fine (the UI may
    /// cancel after the turn ended on its own), so this is idempotent.
    pub fn cancel(&self, session_id: &str) -> bool {
        match lock(&self.running).get(session_id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every running session (page reload, app exit). Returns how many were running.
    pub fn cancel_all(&self) -> usize {
        let running = lock(&self.running);
        for token in running.values() {
            token.cancel();
        }
        running.len()
    }

    pub fn running_count(&self) -> usize {
        lock(&self.running).len()
    }
}

/// Removes its session from the registry when dropped.
struct Registration {
    running: Running,
    session_id: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        lock(&self.running).remove(&self.session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::{GhostEvent, StopReason};
    use std::sync::Mutex as StdMutex;

    fn collector() -> (
        Arc<StdMutex<Vec<EventBatch>>>,
        impl FnMut(EventBatch) -> bool + Send + 'static,
    ) {
        let frames = Arc::new(StdMutex::new(Vec::new()));
        let sink_frames = Arc::clone(&frames);
        let sink = move |batch: EventBatch| {
            lock(&sink_frames).push(batch);
            true
        };
        (frames, sink)
    }

    fn events(frames: &[EventBatch]) -> Vec<GhostEvent> {
        frames.iter().flat_map(|f| f.events.clone()).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_demo_turn_plays_to_the_end_and_unregisters() {
        let sessions = DemoSessions::default();
        let (frames, sink) = collector();
        let (id, run) = sessions.start(DemoKind::Demo, 400, sink).expect("start");
        assert_eq!(sessions.running_count(), 1);

        assert_eq!(run.await, RunOutcome::Completed);
        assert_eq!(
            sessions.running_count(),
            0,
            "a finished stream removes itself"
        );

        let frames = lock(&frames).clone();
        assert!(frames.iter().all(|f| f.chat_id == id));
        let mut next = 0;
        for frame in &frames {
            assert_eq!(frame.seq_start, next, "sequence numbers are contiguous");
            next += u32::try_from(frame.events.len()).expect("small");
        }
        let all = events(&frames);
        assert!(matches!(all.first(), Some(GhostEvent::TurnStarted { .. })));
        assert!(matches!(
            all.last(),
            Some(GhostEvent::TurnEnded {
                stop_reason: StopReason::EndTurn,
                ..
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_ends_the_turn_as_cancelled() {
        let sessions = DemoSessions::default();
        let (frames, sink) = collector();
        let (id, run) = sessions.start(DemoKind::Demo, 10, sink).expect("start");
        let handle = tokio::spawn(run);
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(sessions.cancel(&id));
        assert_eq!(handle.await.expect("join"), RunOutcome::Cancelled);
        assert_eq!(sessions.running_count(), 0);
        assert!(matches!(
            events(&lock(&frames)).last(),
            Some(GhostEvent::TurnEnded {
                stop_reason: StopReason::Cancelled,
                ..
            })
        ));
        // A second cancel, or one for an id that never existed, is a harmless no-op.
        assert!(!sessions.cancel(&id));
        assert!(!sessions.cancel("not-a-session"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_gone_receiver_stops_the_stream() {
        let sessions = DemoSessions::default();
        let mut calls = 0;
        let (_, run) = sessions
            .start(DemoKind::Demo, 400, move |_| {
                calls += 1;
                calls < 2
            })
            .expect("start");
        assert_eq!(run.await, RunOutcome::ReceiverGone);
        assert_eq!(sessions.running_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_all_stops_every_stream() {
        let sessions = DemoSessions::default();
        let mut handles = Vec::new();
        for _ in 0..3 {
            let (_, sink) = collector();
            let (_, run) = sessions
                .start(
                    DemoKind::Stress {
                        total_tokens: 5_000,
                    },
                    50,
                    sink,
                )
                .expect("start");
            handles.push(tokio::spawn(run));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(sessions.cancel_all(), 3);
        for handle in handles {
            assert_eq!(handle.await.expect("join"), RunOutcome::Cancelled);
        }
        assert_eq!(sessions.running_count(), 0);
        assert_eq!(sessions.cancel_all(), 0);
    }

    #[test]
    fn too_many_streams_are_refused() {
        let sessions = DemoSessions::default();
        let mut pending = Vec::new();
        for _ in 0..MAX_CONCURRENT_STREAMS {
            let (_, sink) = collector();
            pending.push(sessions.start(DemoKind::Demo, 100, sink).expect("start"));
        }
        let (_, sink) = collector();
        let error = sessions
            .start(DemoKind::Demo, 100, sink)
            .err()
            .expect("refused");
        assert_eq!(error.code, AppErrorCode::TooManyStreams);

        // Dropping a never-started run unregisters it, which frees a slot.
        pending.pop();
        let (_, sink) = collector();
        assert!(sessions.start(DemoKind::Demo, 100, sink).is_ok());
    }

    #[test]
    fn stress_token_counts_are_bounded() {
        for total_tokens in [0, MAX_STRESS_TOKENS + 1, u32::MAX] {
            let error = DemoKind::Stress { total_tokens }
                .script(200)
                .expect_err("rejected");
            assert_eq!(error.code, AppErrorCode::InvalidArgument, "{total_tokens}");
        }
        let script = DemoKind::Stress { total_tokens: 600 }
            .script(200)
            .expect("valid");
        assert_eq!(script.streamed_token_count(), 600);
    }

    #[test]
    fn kinds_serialize_with_a_type_tag() {
        assert_eq!(
            serde_json::to_value(DemoKind::Stress { total_tokens: 600 }).expect("serialize"),
            serde_json::json!({ "type": "stress", "total_tokens": 600 })
        );
        let demo: DemoKind =
            serde_json::from_value(serde_json::json!({ "type": "demo" })).expect("deserialize");
        assert_eq!(demo, DemoKind::Demo);
    }
}
