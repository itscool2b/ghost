//! The Processes page: every chat's agent processes and what they started, from one process-table
//! snapshot, and stopping one of them. Rust decides what may be stopped: a process inside a chat's
//! agent tree. Never the agent itself (the chat's own Stop keeps its session consistent) and never
//! anything else on the system.

use ghost_process::tree::{Snapshot, StopError, TreeProcess};
use serde::Serialize;
use tauri::State;

use crate::error::{AppError, AppErrorCode};
use crate::state::AppState;

/// Every running agent process, by chat, as the snapshot found them.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProcessesSnapshot {
    /// When the table was read, in milliseconds since the Unix epoch; the page works out processor
    /// use between two snapshots from it.
    pub taken_at_ms: f64,
    pub chats: Vec<ChatProcesses>,
}

/// One chat's agent processes, and the tasks its agent reported. Subagents run inside the
/// agent's own process, so the tasks are how they show.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ChatProcesses {
    pub chat_id: String,
    pub agents: Vec<AgentProcesses>,
    /// The latest record of each task, oldest first (at most 64).
    pub tasks: Vec<ghost_protocol::tasks::BackgroundTask>,
}

/// One agent process and everything it started, the agent first, then nearest first.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentProcesses {
    pub pid: u32,
    pub processes: Vec<ProcessRow>,
}

/// One process. Numbers that do not fit a JavaScript integer exactly never occur here: times are
/// milliseconds and sizes bytes.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProcessRow {
    pub pid: u32,
    /// Its parent, when that parent is in the same agent's tree.
    pub parent: Option<u32>,
    pub name: String,
    /// Masked and cut short ([`ghost_process::tree::mask_command`]).
    pub command: Option<String>,
    pub started_ms: Option<f64>,
    pub cpu_ms: Option<f64>,
    pub memory_bytes: Option<f64>,
}

impl From<TreeProcess> for ProcessRow {
    fn from(process: TreeProcess) -> Self {
        let number = |value: Option<u64>| value.map(|value| value as f64);
        ProcessRow {
            pid: process.pid,
            parent: process.parent,
            name: process.name,
            command: process.command,
            started_ms: number(process.started_ms),
            cpu_ms: number(process.cpu_ms),
            memory_bytes: number(process.memory_bytes),
        }
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_millis() as f64)
}

fn unavailable(error: impl std::fmt::Display) -> AppError {
    AppError::new(
        AppErrorCode::InvalidArgument,
        format!("The process list could not be read: {error}"),
    )
}

/// Every chat's agent processes and what each started.
#[tauri::command]
#[specta::specta]
pub async fn processes_snapshot(state: State<'_, AppState>) -> Result<ProcessesSnapshot, AppError> {
    let agents = state.sessions()?.agent_processes();
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = Snapshot::take().map_err(unavailable)?;
        let taken_at_ms = now_ms();
        let chats = agents
            .into_iter()
            .map(|chat| ChatProcesses {
                chat_id: chat.chat_id,
                tasks: chat.tasks,
                agents: chat
                    .processes
                    .iter()
                    .map(|control| AgentProcesses {
                        pid: control.pid(),
                        processes: snapshot
                            .tree(control)
                            .into_iter()
                            .map(ProcessRow::from)
                            .collect(),
                    })
                    .filter(|agent| !agent.processes.is_empty())
                    .collect(),
            })
            .filter(|chat: &ChatProcesses| !chat.agents.is_empty())
            .collect();
        Ok(ProcessesSnapshot { taken_at_ms, chats })
    })
    .await
    .map_err(unavailable)?
}

/// Stops `pid` and everything under it, if it runs under one of `chat_id`'s agents.
#[tauri::command]
#[specta::specta]
pub async fn process_stop(
    state: State<'_, AppState>,
    chat_id: String,
    pid: u32,
) -> Result<(), AppError> {
    let controls = state
        .sessions()?
        .agent_processes()
        .into_iter()
        .find(|chat| chat.chat_id == chat_id)
        .map(|chat| chat.processes)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let mut last = StopError::NotInTree;
        for control in &controls {
            match ghost_process::tree::stop(control, pid) {
                Ok(_) => return Ok(()),
                Err(StopError::NotInTree) => {}
                Err(error) => last = error,
            }
        }
        Err(AppError::new(
            AppErrorCode::InvalidArgument,
            last.to_string(),
        ))
    })
    .await
    .map_err(unavailable)?
}
