//! State transitions for native shell effects. No transcript text or prompt content is retained.

use std::collections::HashMap;

use ghost_protocol::SessionStatus;
use ghost_protocol::session::ChatStatusUpdate;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub active: usize,
    pub attention: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Attention,
    Finished,
    Failed,
}

impl Notice {
    pub fn title(self) -> &'static str {
        match self {
            Self::Attention => "An agent needs your attention",
            Self::Finished => "An agent finished its turn",
            Self::Failed => "An agent could not continue",
        }
    }

    /// The sound the window plays for this notice.
    pub fn sound(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Chat {
    status: SessionStatus,
    attention: bool,
}

#[derive(Default)]
pub struct Activity {
    chats: HashMap<String, Chat>,
}

impl Activity {
    pub fn work_count(&self) -> usize {
        self.chats.len()
    }

    pub fn notice_is_current(&self, chat: &str, notice: Notice) -> bool {
        match notice {
            Notice::Attention => self.chats.get(chat).is_some_and(|chat| chat.attention),
            Notice::Finished | Notice::Failed => !self.chats.contains_key(chat),
        }
    }
    /// Retains only ongoing work and unresolved requests. Idle chat history does not grow this map.
    pub fn update(&mut self, update: &ChatStatusUpdate) -> (Counts, Option<Notice>) {
        let previous = self.chats.remove(&update.chat_id);
        let attention_rose = update.needs_attention && !previous.is_some_and(|chat| chat.attention);
        let notice = if attention_rose {
            Some(Notice::Attention)
        } else if previous.is_some_and(|chat| matches!(chat.status, SessionStatus::Running))
            && update.status == SessionStatus::Ready
        {
            Some(Notice::Finished)
        } else if previous.is_some()
            && matches!(
                update.status,
                SessionStatus::Failed | SessionStatus::AuthRequired
            )
        {
            Some(Notice::Failed)
        } else {
            None
        };
        if active(update.status) || update.needs_attention {
            self.chats.insert(
                update.chat_id.clone(),
                Chat {
                    status: update.status,
                    attention: update.needs_attention,
                },
            );
        }
        (self.counts(), notice)
    }

    pub fn counts(&self) -> Counts {
        Counts {
            active: self
                .chats
                .values()
                .filter(|chat| active(chat.status))
                .count(),
            attention: self.chats.values().filter(|chat| chat.attention).count(),
        }
    }
}

fn active(status: SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Starting
            | SessionStatus::Running
            | SessionStatus::Cancelling
            | SessionStatus::Stopping
    )
}

pub fn tooltip(counts: Counts) -> String {
    match (counts.active, counts.attention) {
        (0, 0) => "Ghost".into(),
        (active, 0) => format!(
            "Ghost: {active} active {}",
            if active == 1 { "chat" } else { "chats" }
        ),
        (0, attention) => format!(
            "Ghost: {attention} {} attention",
            if attention == 1 {
                "chat needs"
            } else {
                "chats need"
            }
        ),
        (active, attention) => format!(
            "Ghost: {active} active, {attention} {} attention",
            if attention == 1 { "needs" } else { "need" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: &str, status: SessionStatus, needs_attention: bool) -> ChatStatusUpdate {
        ChatStatusUpdate {
            chat_id: id.into(),
            status,
            needs_attention,
            title: None,
            updated_at: 1,
        }
    }

    /// The tray read "1 active, 1 need attention".
    #[test]
    fn the_tooltip_agrees_in_number() {
        let counts = |active, attention| Counts { active, attention };
        assert_eq!(tooltip(counts(0, 0)), "Ghost");
        assert_eq!(tooltip(counts(1, 0)), "Ghost: 1 active chat");
        assert_eq!(tooltip(counts(0, 2)), "Ghost: 2 chats need attention");
        assert_eq!(tooltip(counts(1, 1)), "Ghost: 1 active, 1 needs attention");
        assert_eq!(tooltip(counts(3, 2)), "Ghost: 3 active, 2 need attention");
    }

    #[test]
    fn concurrent_turns_hold_activity_until_the_last_turn_finishes() {
        let mut state = Activity::default();
        state.update(&update("a", SessionStatus::Running, false));
        state.update(&update("b", SessionStatus::Running, false));
        assert_eq!(state.counts().active, 2);
        assert_eq!(
            state.update(&update("a", SessionStatus::Ready, false)).1,
            Some(Notice::Finished)
        );
        assert_eq!(state.counts().active, 1);
        state.update(&update("b", SessionStatus::Stopping, false));
        assert_eq!(
            state.update(&update("b", SessionStatus::Stopped, false)).1,
            None
        );
        assert_eq!(state.counts(), Counts::default());
        assert!(state.chats.is_empty());
    }

    #[test]
    fn attention_notifies_once_per_request_episode_and_survives_an_idle_status() {
        let mut state = Activity::default();
        let attention = update("a", SessionStatus::Running, true);
        assert_eq!(state.update(&attention).1, Some(Notice::Attention));
        assert_eq!(state.update(&attention).1, None);
        assert_eq!(
            state
                .update(&update("a", SessionStatus::Ready, true))
                .0
                .attention,
            1
        );
        state.update(&update("a", SessionStatus::Ready, false));
        assert_eq!(state.update(&attention).1, Some(Notice::Attention));
    }

    #[test]
    fn cancellation_idle_restore_and_duplicate_failure_do_not_report_success() {
        let mut state = Activity::default();
        assert_eq!(
            state.update(&update("old", SessionStatus::Ready, false)).1,
            None
        );
        state.update(&update("a", SessionStatus::Cancelling, false));
        assert_eq!(
            state.update(&update("a", SessionStatus::Ready, false)).1,
            None
        );
        state.update(&update("b", SessionStatus::Starting, false));
        assert_eq!(
            state
                .update(&update("b", SessionStatus::AuthRequired, false))
                .1,
            Some(Notice::Failed)
        );
        assert_eq!(
            state
                .update(&update("b", SessionStatus::AuthRequired, false))
                .1,
            None
        );
    }

    #[test]
    fn delayed_notices_are_suppressed_after_attention_resolves_or_new_work_starts() {
        let mut state = Activity::default();
        state.update(&update("a", SessionStatus::Ready, true));
        assert_eq!(state.work_count(), 1, "an unresolved request guards quit");
        assert_eq!(
            state.counts().active,
            0,
            "idle requests do not prevent sleep"
        );
        assert!(state.notice_is_current("a", Notice::Attention));
        state.update(&update("a", SessionStatus::Ready, false));
        assert!(!state.notice_is_current("a", Notice::Attention));
        assert_eq!(state.work_count(), 0);
        assert!(state.notice_is_current("a", Notice::Finished));
        state.update(&update("a", SessionStatus::Starting, false));
        assert!(!state.notice_is_current("a", Notice::Finished));
        assert!(!state.notice_is_current("a", Notice::Failed));
    }
}
