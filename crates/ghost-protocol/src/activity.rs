//! The Dashboard's activity calendar: the lines agents' edit tools changed, over time.

use crate::Millis;
use serde::{Deserialize, Serialize};
use specta::Type;

/// Lines one agent's edits changed in one quarter hour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LineEditBucket {
    /// The agent's id (`claude`, `codex`), or empty when the turn does not say.
    pub agent: String,
    /// When the quarter hour began, in Unix milliseconds. Every time zone's days start on a
    /// quarter hour, so the window can place it in its own day.
    pub at: Millis,
    pub lines: u32,
}

/// The line edits since a moment, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LineEdits {
    pub buckets: Vec<LineEditBucket>,
    /// False while ended turns are still waiting to be counted: the figures are not final yet.
    pub complete: bool,
}
