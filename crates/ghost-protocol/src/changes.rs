//! Checkpoint-backed Changes and explicit revert previews.

use crate::Millis;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CheckpointSummary {
    pub id: String,
    pub turn_id: String,
    pub created_at: Millis,
    pub file_count: u32,
    pub total_bytes: u32,
    /// Exclusions and limits are visible. Excluded paths are never removed by a revert.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChangesSnapshot {
    pub checkpoint: Option<CheckpointSummary>,
    pub files: Vec<ChangedFile>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChangedFile {
    pub path: String,
    pub kind: ChangeKind,
    pub before_bytes: Option<u32>,
    pub after_bytes: Option<u32>,
    pub binary: bool,
}

/// Files an agent's own checkpoints restore (Claude's file rewind), to before a sent message.
/// A preview changes nothing. They cover only the files the agent's edit tools changed; a real
/// rewind can skip links for safety, and says how many.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NativeRewind {
    pub files: Vec<String>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
    pub skipped_links: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChangeDiff {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    /// The SHA-256 of the current file `after` was read from, `None` when it is absent. Reverting
    /// one hunk names it, so its line numbers count lines of this very text.
    #[serde(default)]
    pub after_sha256: Option<String>,
}

/// A block of changed lines, by offsets: `old_start` lines of the checkpoint's text come before
/// it and `old_lines` belong to it; `new_start` and `new_lines` count the same in the current
/// text. Reverting it puts the checkpoint's lines back in place of the current ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LineHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

/// Hunks of one modified text file to revert, counted in the texts its `ChangeDiff` gave, in
/// order and apart. The rest of the file stays as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HunkRevert {
    pub path: String,
    pub after_sha256: String,
    pub hunks: Vec<LineHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RevertPreview {
    pub token: String,
    pub checkpoint_id: String,
    pub files: Vec<ChangedFile>,
    pub expires_at: Millis,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RevertResult {
    pub applied: Vec<String>,
    /// A partial failure is explicit; the caller must not report full success.
    pub error: Option<String>,
    /// Durable recovery groups in the profile's data folder.
    /// A failed finalization also names any transient working-folder copy that was preserved.
    pub recovery_paths: Vec<String>,
}
