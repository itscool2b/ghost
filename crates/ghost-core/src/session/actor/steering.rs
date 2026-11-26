//! Native steering is never a queued prompt. Store the visible attempt before issuing control.
use super::*;
use crate::chatstore::steering::Command;
use ghost_drivers::driver::SteerResult;
use ghost_protocol::composer::{SteeringAck, SteeringState};

fn sequence_room(next: u32, text_bytes: usize) -> bool {
    // A delta fragment contains at least one source byte. This deliberately reserves far more
    // than ordinary frame packing needs, plus both statuses and incidental header events.
    u32::try_from(text_bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(16))
        .and_then(|events| next.checked_add(events))
        .is_some_and(|end| end <= SEQ_LIMIT)
}

pub(super) struct Attempt {
    turn_id: String,
    message_id: String,
    generation: u64,
    parts: Vec<PromptPart>,
    reply: oneshot::Sender<Result<SteeringAck, SessionError>>,
}

pub(super) enum Completion {
    Prepared(Attempt, Result<(), SessionError>),
    Delivered(Attempt, SteerResult),
    Saved(Attempt, SteeringAck, Result<(), SessionError>),
}

impl ChatActor {
    fn steering_allowed(&self, turn_id: &str) -> bool {
        !self.organizing
            && !self.quitting
            && !self.shared.is_shutting_down()
            && self.status == SessionStatus::Running
            && self.capabilities.as_ref().is_some_and(|caps| caps.steer)
            && self
                .turn
                .as_ref()
                .is_some_and(|turn| turn.id == turn_id && !turn.interrupt_requested)
            && self.driver.as_ref().is_some_and(|driver| {
                driver.ready && driver.events.is_some() && driver.stop == Stop::None
            })
    }

    pub(super) fn steer(
        &mut self,
        turn_id: String,
        text: String,
        parts: Vec<PromptPart>,
        reply: oneshot::Sender<Result<SteeringAck, SessionError>>,
    ) {
        self.flush();
        let invalid = if self.steering_pending {
            Some(SessionError::OptionInvalid(
                "Wait for the current steering delivery to finish.".into(),
            ))
        } else if !self.steering_allowed(&turn_id) {
            Some(SessionError::OptionInvalid(
                "This agent cannot steer the selected running turn.".into(),
            ))
        } else if self.store.is_paused() {
            Some(SessionError::StorageUnavailable(
                "Wait for saving to resume before steering.".into(),
            ))
        } else if text.trim().is_empty() {
            Some(SessionError::PromptEmpty)
        } else if text.len() > self.shared.deps.limits.max_prompt_bytes {
            Some(SessionError::PromptTooLarge {
                bytes: text.len(),
                limit: self.shared.deps.limits.max_prompt_bytes,
            })
        } else if !sequence_room(self.batcher.next_seq(), text.len()) {
            Some(SessionError::Internal(
                "This chat is too long to continue; start a new chat.".into(),
            ))
        } else if parts
            .iter()
            .any(|part| matches!(part, PromptPart::Image { .. }))
            && self.capabilities.as_ref().is_some_and(|caps| !caps.images)
        {
            Some(SessionError::OptionInvalid(
                "This agent does not support images. Remove the image or choose another agent."
                    .into(),
            ))
        } else {
            None
        };
        if let Some(error) = invalid {
            let _ = reply.send(Err(error));
            return;
        }
        let message_id = new_id();
        self.steering_pending = true;
        let seq = self.batcher.next_seq();
        self.push(
            GhostEvent::SteeringStatus {
                message_id: message_id.clone(),
                state: SteeringState::Pending,
                detail: None,
            },
            Some(&turn_id),
        );
        self.push(
            GhostEvent::UserMessageDelta {
                message_id: message_id.clone(),
                text,
            },
            Some(&turn_id),
        );
        self.flush();
        let (saved, barrier) = oneshot::channel();
        self.queue_op(Op::Barrier(saved));
        let store = Arc::clone(&self.store);
        let rt = self.shared.deps.rt.clone();
        let command = Command::Begin {
            chat_id: self.chat_id.clone(),
            turn_id: turn_id.clone(),
            message_id: message_id.clone(),
            seq,
            parts: parts.clone(),
        };
        let attempt = Attempt {
            turn_id,
            message_id,
            generation: self.generation,
            parts,
            reply,
        };
        self.in_flight.push(
            async move {
                let result = async {
                    await_reply(&rt, barrier, Duration::from_secs(5))
                        .await
                        .map_err(|_| {
                            SessionError::StorageUnavailable(
                                "The steering message is still saving. It was not sent.".into(),
                            )
                        })?;
                    store
                        .steering(command)
                        .await
                        .map_err(|error| storage_error(&error))
                }
                .await;
                Done::Steering(Box::new(Completion::Prepared(attempt, result)))
            }
            .boxed(),
        );
    }

    pub(super) fn steering_done(&mut self, completion: Completion) {
        match completion {
            Completion::Prepared(attempt, Err(error)) => {
                self.steering_pending = false;
                self.push(
                    GhostEvent::SteeringStatus {
                        message_id: attempt.message_id,
                        state: SteeringState::Rejected,
                        detail: Some("The message could not be saved and was not sent.".into()),
                    },
                    Some(&attempt.turn_id),
                );
                self.flush();
                self.queue_op(Op::CompactTurn {
                    chat_id: self.chat_id.clone(),
                    turn_id: attempt.turn_id.clone(),
                });
                let _ = attempt.reply.send(Err(error));
                self.release_if_idle();
                self.dispatch_next();
            }
            Completion::Prepared(attempt, Ok(())) => {
                if attempt.generation != self.generation
                    || !self.steering_allowed(&attempt.turn_id)
                    || attempt.reply.is_closed()
                {
                    self.steering_done(Completion::Delivered(attempt, SteerResult::Rejected { message: "The selected turn stopped or the request was canceled before delivery.".into() }));
                    return;
                }
                let (reply, answer) = oneshot::channel();
                self.send_command(DriverCommand::Steer {
                    turn_id: attempt.turn_id.clone(),
                    message_id: attempt.message_id.clone(),
                    parts: attempt.parts.clone(),
                    reply,
                });
                self.in_flight.push(async move {
                    let outcome = match tokio::time::timeout(Duration::from_secs(45), answer).await {
                        Ok(Ok(outcome)) => outcome,
                        _ => SteerResult::Unconfirmed { message: "The agent did not confirm steering delivery. It was not retried.".into() },
                    };
                    Done::Steering(Box::new(Completion::Delivered(attempt, outcome)))
                }.boxed());
            }
            Completion::Delivered(attempt, outcome) => {
                let (state, detail) = match outcome {
                    SteerResult::Accepted => (SteeringState::Accepted, None),
                    SteerResult::Rejected { message } => (
                        SteeringState::Rejected,
                        Some(self.error_text(&message, false)),
                    ),
                    SteerResult::Unconfirmed { message } => (
                        SteeringState::Unconfirmed,
                        Some(self.error_text(&message, false)),
                    ),
                };
                let ack = SteeringAck {
                    message_id: attempt.message_id.clone(),
                    state,
                    detail,
                };
                let command = Command::Finish {
                    chat_id: self.chat_id.clone(),
                    message_id: attempt.message_id.clone(),
                    state,
                    detail: ack.detail.clone(),
                };
                let store = Arc::clone(&self.store);
                self.in_flight.push(
                    async move {
                        let result = store
                            .steering(command)
                            .await
                            .map_err(|error| storage_error(&error));
                        Done::Steering(Box::new(Completion::Saved(attempt, ack, result)))
                    }
                    .boxed(),
                );
            }
            Completion::Saved(attempt, mut ack, result) => {
                self.steering_pending = false;
                if result.is_err() {
                    ack.state = SteeringState::Unconfirmed;
                    ack.detail = Some("The delivery outcome could not be saved. Do not assume this message was delivered or automatically retry it.".into());
                }
                self.push(
                    GhostEvent::SteeringStatus {
                        message_id: ack.message_id.clone(),
                        state: ack.state,
                        detail: ack.detail.clone(),
                    },
                    Some(&attempt.turn_id),
                );
                self.flush();
                self.queue_op(Op::CompactTurn {
                    chat_id: self.chat_id.clone(),
                    turn_id: attempt.turn_id.clone(),
                });
                let _ = attempt.reply.send(Ok(ack));
                self.release_if_idle();
                self.dispatch_next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steering_reserves_sequence_space_for_split_text_and_final_outcome() {
        assert!(!sequence_room(SEQ_LIMIT, 1));
        assert!(!sequence_room(u32::MAX, 1));
        assert!(!sequence_room(SEQ_LIMIT - 16, 1));
        let text = "\u{0}".repeat(20_000);
        let mut batcher = FrameBatcher::new("chat").starting_at(SEQ_LIMIT - 20_016);
        batcher.push_tagged(
            GhostEvent::UserMessageDelta {
                message_id: "steer".into(),
                text: text.clone(),
            },
            None,
        );
        let frames = batcher.flush_tagged();
        assert!(frames.len() > 1, "escaped text must split");
        assert!(sequence_room(SEQ_LIMIT - 20_016, text.len()));
        assert!(
            batcher.next_seq() + 2 < SEQ_LIMIT,
            "both outcomes still fit"
        );
    }
}
