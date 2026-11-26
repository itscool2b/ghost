//! Keep-awake assertions belong to one dedicated thread. Windows' underlying execution state is
//! thread-local, so creating on a Tokio worker and dropping on another would leave the wrong state.

use std::sync::mpsc::{Sender, channel};

pub struct Power {
    sender: Option<Sender<bool>>,
}

impl Power {
    pub fn start() -> Self {
        Self::start_with(|| {
            match keepawake::Builder::default()
                .idle(true)
                .display(false)
                .sleep(false)
                .reason("A Ghost agent is working, or remote access keeps the computer reachable")
                .app_name("Ghost")
                .app_reverse_domain("dev.onghost.desktop")
                .create()
            {
                Ok(assertion) => Some(assertion),
                Err(error) => {
                    tracing::warn!(%error, "could not prevent idle sleep");
                    None
                }
            }
        })
    }

    pub(super) fn start_with<G: 'static>(
        mut create: impl FnMut() -> Option<G> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = channel();
        let thread = std::thread::Builder::new()
            .name("auracode-awake".into())
            .spawn(move || {
                let mut held = None;
                while let Ok(mut active) = receiver.recv() {
                    // Collapse rapid transitions, retaining the most recent request including false.
                    for latest in receiver.try_iter() {
                        active = latest;
                    }
                    if active && held.is_none() {
                        held = create();
                    } else if !active {
                        held = None;
                    }
                }
                // The assertion drops on the same thread even if the shell disappears unexpectedly.
                drop(held);
            });
        match thread {
            Ok(_) => Self {
                sender: Some(sender),
            },
            Err(error) => {
                tracing::warn!(%error, "could not start the keep-awake service");
                Self { sender: None }
            }
        }
    }

    pub fn set(&self, active: bool) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(active);
        }
    }

    pub fn stop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(false);
        }
    }
}

impl Drop for Power {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::ThreadId;
    use std::time::Duration;

    struct Assertion(Sender<(&'static str, ThreadId)>);
    impl Drop for Assertion {
        fn drop(&mut self) {
            self.0
                .send(("drop", std::thread::current().id()))
                .expect("record drop");
        }
    }

    #[test]
    fn assertions_start_and_end_on_one_thread_and_release_on_service_drop() {
        let (record, events) = channel();
        let power = Power::start_with(move || {
            record
                .send(("create", std::thread::current().id()))
                .expect("record create");
            Some(Assertion(record.clone()))
        });
        let next = || {
            events
                .recv_timeout(Duration::from_secs(5))
                .expect("power event")
        };
        power.set(true);
        let (event, thread) = next();
        assert_eq!(event, "create");
        assert_ne!(thread, std::thread::current().id());
        power.set(false);
        assert_eq!(next(), ("drop", thread));
        power.set(true);
        assert_eq!(next(), ("create", thread));
        drop(power);
        assert_eq!(next(), ("drop", thread));
    }
}
