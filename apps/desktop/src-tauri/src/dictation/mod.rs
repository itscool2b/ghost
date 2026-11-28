//! App-owned, explicitly started dictation (docs/local-dictation.md). The bundled models run
//! offline: audio lives only in memory, and nothing makes a network request.
mod audio;
pub mod commands;
mod engine;
mod larger;
mod live;
mod model;

use crate::sync::lock;
pub(crate) use audio::cpu_supported;
pub use live::LiveText;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub(crate) type Sink = Arc<dyn Fn(DictationEvent) -> bool + Send + Sync>;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DictationEvent {
    /// The engine is loading, the first time a recording starts.
    Preparing,
    /// The microphone is open: the time so far, its level (0 to 1), whether a phrase is in
    /// progress, and the text so far.
    Listening {
        seconds: u32,
        level: f32,
        speaking: bool,
        text: LiveText,
    },
    /// The microphone has closed and the last phrase is settling.
    Finishing {
        text: LiveText,
    },
    Complete {
        text: String,
    },
    Cancelled,
    Failed {
        message: String,
        reason: DictationFailure,
    },
}

/// Why an operation failed, so the panel offers the recovery that can help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DictationFailure {
    /// The bundled models are missing or failed to load: reinstalling restores them.
    Model,
    /// The microphone is missing, blocked, busy or stopped.
    Microphone,
    /// The recording held no speech.
    NoSpeech,
    /// This processor lacks the instructions the speech engine needs.
    Unsupported,
    Other,
}

/// A failed operation's reason and its one-line explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub reason: DictationFailure,
    pub message: String,
    /// Cancellation travels as a failure internally and ends as `Cancelled`.
    pub cancelled: bool,
}
impl Failure {
    fn new(reason: DictationFailure, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            cancelled: false,
        }
    }
    pub fn model(message: impl Into<String>) -> Self {
        Self::new(DictationFailure::Model, message)
    }
    pub fn microphone(message: impl Into<String>) -> Self {
        Self::new(DictationFailure::Microphone, message)
    }
    pub fn no_speech(message: impl Into<String>) -> Self {
        Self::new(DictationFailure::NoSpeech, message)
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(DictationFailure::Other, message)
    }
    pub fn unsupported() -> Self {
        Self::new(
            DictationFailure::Unsupported,
            "Dictation needs a processor with AVX2 (Intel from 2013 or AMD from 2015 onward). The rest of Ghost works as usual.",
        )
    }
    pub fn cancelled() -> Self {
        Self {
            cancelled: true,
            ..Self::other("Dictation cancelled.")
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct DictationModel {
    pub installed: bool,
    pub name: String,
    pub bytes: u32,
    /// This processor can run the speech engine.
    pub supported: bool,
    /// The larger speech model a person can fetch (`larger.rs`). `name` above is the model in
    /// use: this one while it is here and chosen.
    pub larger: LargerModel,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct LargerModel {
    pub name: String,
    pub bytes: u32,
    pub state: LargerState,
    /// Whether this copy can keep a downloaded model at all.
    pub available: bool,
    /// Bytes of it here so far, while it downloads: a page that did not start the download
    /// shows how far it has come from this.
    pub received: u32,
    /// The person chose it (`larger::chosen`): dictation uses it whenever it is here.
    pub chosen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LargerState {
    Absent,
    Downloading,
    Ready,
}

#[derive(Default)]
pub struct Dictation {
    active: Mutex<Option<Active>>,
}
struct Active {
    id: String,
    owner: String,
    cancel: CancellationToken,
    stop: Arc<AtomicBool>,
}

pub(crate) struct Operation {
    service: Arc<Dictation>,
    id: String,
    pub cancel: CancellationToken,
    pub stop: Arc<AtomicBool>,
    pub sink: Sink,
    current: Box<dyn Fn() -> bool + Send + Sync>,
}
impl Operation {
    pub fn cancelled(&self) -> bool {
        if !(self.current)() {
            self.cancel.cancel();
        }
        self.cancel.is_cancelled()
    }
    pub fn emit(&self, event: DictationEvent) -> bool {
        if self.cancelled() || !(self.sink)(event) {
            self.cancel.cancel();
            return false;
        }
        true
    }
    pub fn finish(&self, result: Result<DictationEvent, Failure>) {
        let event = match result {
            _ if self.cancelled() => DictationEvent::Cancelled,
            Err(failure) if failure.cancelled => DictationEvent::Cancelled,
            Err(Failure {
                reason, message, ..
            }) => DictationEvent::Failed { message, reason },
            Ok(event) => event,
        };
        // Cancellation still ends the UI's busy state. Stale page channels simply reject it.
        (self.sink)(event);
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        let mut active = lock(&self.service.active);
        if active.as_ref().is_some_and(|op| op.id == self.id) {
            *active = None;
        }
    }
}
impl Dictation {
    pub(crate) fn begin(
        self: &Arc<Self>,
        owner: &str,
        id: String,
        sink: Sink,
        current: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Operation, String> {
        uuid::Uuid::parse_str(&id).map_err(|_| "Invalid dictation request.".to_string())?;
        let mut active = lock(&self.active);
        if active.is_some() {
            return Err("Dictation is still finishing. Try again in a moment.".into());
        }
        let cancel = CancellationToken::new();
        let stop = Arc::new(AtomicBool::new(false));
        *active = Some(Active {
            id: id.clone(),
            owner: owner.into(),
            cancel: cancel.clone(),
            stop: stop.clone(),
        });
        Ok(Operation {
            service: self.clone(),
            id,
            cancel,
            stop,
            sink,
            current,
        })
    }
    pub fn cancel(&self, owner: &str, id: &str) {
        if let Some(op) = lock(&self.active)
            .as_ref()
            .filter(|op| op.owner == owner && op.id == id)
        {
            op.cancel.cancel();
        }
    }
    pub fn stop(&self, owner: &str, id: &str) {
        if let Some(op) = lock(&self.active)
            .as_ref()
            .filter(|op| op.owner == owner && op.id == id)
        {
            op.stop.store(true, Ordering::Release);
        }
    }
    pub fn drop_owner(&self, owner: &str) {
        if let Some(op) = lock(&self.active).as_ref().filter(|op| op.owner == owner) {
            op.cancel.cancel();
        }
    }
    pub fn cancel_all(&self) {
        if let Some(op) = lock(&self.active).as_ref() {
            op.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(service: &Arc<Dictation>, owner: &str) -> Operation {
        service
            .begin(
                owner,
                uuid::Uuid::new_v4().to_string(),
                Arc::new(|_| true),
                Box::new(|| true),
            )
            .unwrap()
    }
    #[test]
    fn ownership_cancellation_and_exclusivity() {
        let service = Arc::new(Dictation::default());
        let operation = begin(&service, "main");
        assert!(
            service
                .begin(
                    "main",
                    uuid::Uuid::new_v4().to_string(),
                    Arc::new(|_| true),
                    Box::new(|| true)
                )
                .is_err()
        );
        service.cancel("other", &operation.id);
        assert!(!operation.cancelled());
        service.stop("main", &operation.id);
        assert!(operation.stop.load(Ordering::Acquire));
        service.drop_owner("main");
        assert!(operation.cancelled());
        drop(operation);
        assert!(!begin(&service, "main").cancelled());
    }
    #[test]
    fn a_failure_reaches_the_page_with_its_reason_and_cancellation_wins() {
        let service = Arc::new(Dictation::default());
        let sent = Arc::new(Mutex::new(Vec::new()));
        let begin = |service: &Arc<Dictation>| {
            let sent = sent.clone();
            service
                .begin(
                    "main",
                    uuid::Uuid::new_v4().to_string(),
                    Arc::new(move |event| {
                        lock(&sent).push(event);
                        true
                    }),
                    Box::new(|| true),
                )
                .unwrap()
        };
        begin(&service).finish(Err(Failure::model("damaged")));
        begin(&service).finish(Err(Failure::cancelled()));
        let operation = begin(&service);
        operation.cancel.cancel();
        operation.finish(Err(Failure::microphone("late")));
        drop(operation);
        let sent = lock(&sent);
        assert!(matches!(
            &sent[0],
            DictationEvent::Failed { reason: DictationFailure::Model, message } if message == "damaged"
        ));
        assert!(matches!(sent[1], DictationEvent::Cancelled));
        assert!(matches!(sent[2], DictationEvent::Cancelled));
    }

    #[test]
    fn stale_page_or_disconnected_channel_cancels() {
        let service = Arc::new(Dictation::default());
        let operation = service
            .begin(
                "main",
                uuid::Uuid::new_v4().to_string(),
                Arc::new(|_| true),
                Box::new(|| false),
            )
            .unwrap();
        assert!(operation.cancelled());
        drop(operation);
        let operation = service
            .begin(
                "main",
                uuid::Uuid::new_v4().to_string(),
                Arc::new(|_| false),
                Box::new(|| true),
            )
            .unwrap();
        assert!(!operation.emit(DictationEvent::Preparing));
        assert!(operation.cancelled());
    }
}
