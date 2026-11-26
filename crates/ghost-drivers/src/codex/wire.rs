//! The subset of the Codex 0.162.1 generated schema used by this driver.
//! Unknown fields and enum variants are retained or ignored without assuming success.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub user_agent: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadStatus {
    Idle,
    Active,
    NotLoaded,
    SystemError,
    #[serde(other)]
    Unknown,
}
impl ThreadStatus {
    pub fn idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub status: ThreadStatus,
    #[serde(default)]
    pub turns: Vec<Turn>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenResponse {
    pub thread: Thread,
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    pub approval_policy: Value,
    pub approvals_reviewer: String,
    pub sandbox: Value,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Interrupted,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub struct Turn {
    pub id: String,
    pub status: TurnStatus,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct TurnResponse {
    pub turn: Turn,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[serde(default)]
    pub is_default: bool,
    pub id: String,
    pub model: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub default_reasoning_effort: String,
    pub supported_reasoning_efforts: Vec<ReasoningEffortOption>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOption {
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResponse {
    pub data: Vec<Model>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_statuses_do_not_mean_ready_or_success() {
        let status: ThreadStatus =
            serde_json::from_value(json!({"type":"future","detail":123})).unwrap();
        assert!(!status.idle());
        assert_eq!(
            serde_json::from_value::<TurnStatus>(json!("future")).unwrap(),
            TurnStatus::Unknown
        );
    }

    #[test]
    fn open_requires_thread_identity_and_effective_settings() {
        assert!(
            serde_json::from_value::<OpenResponse>(
                json!({"thread":{"id":"t","status":{"type":"idle"}}})
            )
            .is_err()
        );
        let open: OpenResponse = serde_json::from_value(json!({
            "thread":{"id":"thread-1","sessionId":"different-session-tree","status":{"type":"idle"},"future":true},
            "model":"model-a","approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"}
        })).unwrap();
        assert_eq!(open.thread.id, "thread-1");
        assert!(open.thread.status.idle());
    }
}
