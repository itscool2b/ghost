//! Read aloud (docs/read-aloud.md): o7 speaks an answer in a voice that ships inside the app and
//! runs on this device. The text never leaves the computer, nothing makes a network request,
//! and no audio is written to disk: samples go from the model to the output device and are gone.
pub mod commands;
mod engine;
mod model;
mod playback;

use crate::sync::lock;
pub use model::{SpeechStatus, SpeechVoice};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) type Sink = Arc<dyn Fn(SpeechEvent) -> bool + Send + Sync>;

/// What one request to read aloud is doing. A request ends with exactly one of `Done`,
/// `Stopped` or `Failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SpeechEvent {
    /// The voice model is loading, or the voice is being made ready: the first time, and the
    /// first time for each voice.
    Preparing,
    /// Sound is coming out.
    Speaking,
    /// All of it was said.
    Done,
    /// It was stopped: by the person, by another request, or because its page went away.
    Stopped,
    Failed {
        message: String,
        reason: SpeechFailure,
    },
}

/// Why reading aloud failed, so the interface offers what can help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SpeechFailure {
    /// The bundled voice model is missing or failed to load: reinstalling restores it.
    Model,
    /// There is no output device, or it stopped.
    Output,
    /// This processor lacks the instructions the speech engine needs.
    Unsupported,
    Other,
}

/// A failed request's reason and its one-line explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub reason: SpeechFailure,
    pub message: String,
}
impl Failure {
    fn new(reason: SpeechFailure, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
    pub fn model(message: impl Into<String>) -> Self {
        Self::new(SpeechFailure::Model, message)
    }
    pub fn output(message: impl Into<String>) -> Self {
        Self::new(SpeechFailure::Output, message)
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(SpeechFailure::Other, message)
    }
    pub fn unsupported() -> Self {
        Self::new(
            SpeechFailure::Unsupported,
            "Reading aloud needs a processor with AVX2 (Intel from 2013 or AMD from 2015 onward). The rest of o7 works as usual.",
        )
    }
}

/// The one voice that may be speaking. A new request stops the one before it, so two answers
/// are never read over each other.
#[derive(Default)]
pub struct Speech {
    active: Mutex<Option<Active>>,
}
struct Active {
    id: String,
    owner: String,
    stop: Arc<AtomicBool>,
}

pub(crate) struct Operation {
    service: Arc<Speech>,
    id: String,
    stop: Arc<AtomicBool>,
    sink: Sink,
    current: Box<dyn Fn() -> bool + Send + Sync>,
}
impl Operation {
    /// Whether this request should fall silent: it was stopped, replaced, or its page is gone.
    pub fn stopped(&self) -> bool {
        if !(self.current)() {
            self.stop.store(true, Ordering::Release);
        }
        self.stop.load(Ordering::Acquire)
    }
    /// The flag a stop raises, for the output callback: it goes silent the moment the request
    /// is stopped or replaced, without waiting for the speaking loop to look.
    pub fn silenced(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
    pub fn emit(&self, event: SpeechEvent) -> bool {
        if self.stopped() || !(self.sink)(event) {
            self.stop.store(true, Ordering::Release);
            return false;
        }
        true
    }
    pub fn finish(&self, result: Result<(), Failure>) {
        let event = match result {
            _ if self.stopped() => SpeechEvent::Stopped,
            Err(Failure { reason, message }) => SpeechEvent::Failed { message, reason },
            Ok(()) => SpeechEvent::Done,
        };
        // A stop still ends the interface's speaking state. A page that left rejects it.
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
impl Speech {
    /// Starts a request, stopping the one in progress.
    pub(crate) fn begin(
        self: &Arc<Self>,
        owner: &str,
        id: String,
        sink: Sink,
        current: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Operation, String> {
        uuid::Uuid::parse_str(&id).map_err(|_| "Invalid request to read aloud.".to_string())?;
        let mut active = lock(&self.active);
        if let Some(previous) = active.as_ref() {
            previous.stop.store(true, Ordering::Release);
        }
        let stop = Arc::new(AtomicBool::new(false));
        *active = Some(Active {
            id: id.clone(),
            owner: owner.into(),
            stop: stop.clone(),
        });
        Ok(Operation {
            service: self.clone(),
            id,
            stop,
            sink,
            current,
        })
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
            op.stop.store(true, Ordering::Release);
        }
    }
    pub fn stop_all(&self) {
        if let Some(op) = lock(&self.active).as_ref() {
            op.stop.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(service: &Arc<Speech>, owner: &str) -> (Operation, Arc<Mutex<Vec<SpeechEvent>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let operation = service
            .begin(
                owner,
                uuid::Uuid::new_v4().to_string(),
                Arc::new(move |event| {
                    lock(&sink).push(event);
                    true
                }),
                Box::new(|| true),
            )
            .unwrap();
        (operation, seen)
    }

    #[test]
    fn a_new_request_stops_the_one_before_it_and_each_ends_once() {
        let service = Arc::new(Speech::default());
        let (first, first_seen) = begin(&service, "main");
        assert!(first.emit(SpeechEvent::Speaking));
        let (second, second_seen) = begin(&service, "main");
        assert!(first.stopped(), "replaced");
        assert!(
            !first.emit(SpeechEvent::Speaking),
            "a stopped request says no more"
        );
        first.finish(Ok(()));
        assert_eq!(
            *lock(&first_seen),
            [SpeechEvent::Speaking, SpeechEvent::Stopped]
        );
        drop(first);
        // The first request ending does not clear the second's place.
        assert!(!second.stopped());
        second.finish(Ok(()));
        assert_eq!(*lock(&second_seen), [SpeechEvent::Done]);
    }

    #[test]
    fn only_its_own_page_can_stop_a_request_and_a_page_that_leaves_silences_it() {
        let service = Arc::new(Speech::default());
        let (operation, seen) = begin(&service, "main");
        service.stop("other", &operation.id);
        service.stop("main", "not-this-one");
        assert!(!operation.stopped());
        service.drop_owner("other");
        assert!(!operation.stopped());
        service.stop("main", &operation.id);
        assert!(operation.stopped());
        operation.finish(Err(Failure::other("ignored once stopped")));
        assert_eq!(*lock(&seen), [SpeechEvent::Stopped]);
        drop(operation);
        let (leaving, _) = begin(&service, "main");
        service.drop_owner("main");
        assert!(leaving.stopped());
        drop(leaving);
        let (quitting, _) = begin(&service, "main");
        service.stop_all();
        assert!(quitting.stopped());
    }

    #[test]
    fn a_request_whose_page_is_no_longer_current_is_stopped() {
        let service = Arc::new(Speech::default());
        let current = Arc::new(AtomicBool::new(true));
        let flag = current.clone();
        let operation = service
            .begin(
                "main",
                uuid::Uuid::new_v4().to_string(),
                Arc::new(|_| true),
                Box::new(move || flag.load(Ordering::Acquire)),
            )
            .unwrap();
        assert!(!operation.stopped());
        current.store(false, Ordering::Release);
        assert!(operation.stopped());
        assert!(
            service
                .begin(
                    "main",
                    "not-a-uuid".into(),
                    Arc::new(|_| true),
                    Box::new(|| true)
                )
                .is_err()
        );
    }

    #[test]
    fn a_failure_is_reported_with_its_reason() {
        let service = Arc::new(Speech::default());
        let (operation, seen) = begin(&service, "main");
        operation.finish(Err(Failure::output("No speaker was found.")));
        assert_eq!(
            *lock(&seen),
            [SpeechEvent::Failed {
                message: "No speaker was found.".into(),
                reason: SpeechFailure::Output
            }]
        );
    }
}
