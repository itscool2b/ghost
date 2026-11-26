//! Shared MCP commands retain the profile admission lease through writer completion.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_core::mcp::Command;
use ghost_protocol::mcp::*;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_snapshot(state: State<'_, AppState>) -> Result<McpSnapshot, AppError> {
    Ok(state.sessions()?.shared_mcp(Command::Snapshot).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_save(
    state: State<'_, AppState>,
    expected: u32,
    definition: McpDefinition,
) -> Result<McpSnapshot, AppError> {
    Ok(state
        .sessions()?
        .shared_mcp(Command::Save {
            expected,
            definition,
        })
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_delete(
    state: State<'_, AppState>,
    expected: u32,
    id: String,
    project_id: Option<String>,
) -> Result<McpSnapshot, AppError> {
    Ok(state
        .sessions()?
        .shared_mcp(Command::Delete {
            expected,
            id,
            project_id,
        })
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_credentials(
    state: State<'_, AppState>,
    expected: u32,
    id: String,
    project_id: Option<String>,
    credentials: McpCredentials,
) -> Result<McpSnapshot, AppError> {
    Ok(state
        .sessions()?
        .shared_mcp(Command::Credentials {
            expected,
            id,
            project_id,
            credentials,
        })
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_import(
    state: State<'_, AppState>,
    expected: u32,
    document: String,
) -> Result<McpSnapshot, AppError> {
    if document.len() > 256 * 1024 {
        return Err(AppError::new(
            AppErrorCode::InvalidArgument,
            "The shared MCP import exceeds 256 KB.",
        ));
    }
    let document: McpExport=serde_json::from_str(&document).map_err(|_|AppError::new(AppErrorCode::InvalidArgument,"Use a Ghost shared MCP JSON export. Native imports need a reviewed portable definition."))?;
    Ok(state
        .sessions()?
        .shared_mcp(Command::Import { expected, document })
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_export(state: State<'_, AppState>) -> Result<String, AppError> {
    let snapshot = state.sessions()?.shared_mcp(Command::Snapshot).await?;
    serde_json::to_string_pretty(&McpExport {
        version: 1,
        definitions: snapshot.servers.into_iter().map(|s| s.definition).collect(),
    })
    .map_err(|_| AppError::new(AppErrorCode::Internal, "Shared MCP export failed."))
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_test(
    state: State<'_, AppState>,
    expected: u32,
    id: String,
    project_id: Option<String>,
    chat_id: Option<String>,
) -> Result<McpTestResult, AppError> {
    Ok(state
        .sessions()?
        .test_shared_mcp(expected, id, project_id, chat_id)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_session_action(
    state: State<'_, AppState>,
    chat_id: String,
    server_name: String,
    action: McpSessionAction,
) -> Result<McpActionResult, AppError> {
    Ok(state
        .sessions()?
        .mcp_session_action(&chat_id, server_name, action)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn shared_mcp_import_native(
    state: State<'_, AppState>,
    expected: u32,
    project_id: Option<String>,
    document: String,
) -> Result<McpSnapshot, AppError> {
    Ok(state
        .sessions()?
        .shared_mcp(Command::ImportNative {
            expected,
            project_id,
            document,
        })
        .await?)
}
