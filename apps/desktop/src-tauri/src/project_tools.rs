//! Project utilities accept saved chat identities and reviewed choices, never arbitrary commands.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_core::session::{EditorOpen, UrlTarget};
use ghost_core::terminal::TerminalError;
use ghost_process::{Spawner, pty::PtySize};
use ghost_protocol::git::{GitInitializePreview, GitWorkspace};
use ghost_protocol::project_tools::{
    DetectedEditors, ProjectEditor, ProjectShell, ProjectTerminalOpened, ProjectTerminalReview,
    ProjectToolsReview,
};
use tauri::{AppHandle, State, Url, Webview};
use tauri_plugin_opener::OpenerExt;

fn current(webview: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(webview.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::ShuttingDown,
            "This page is closing.",
        ));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn project_tools_review(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<ProjectToolsReview, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.project_tools_review(&chat_id).await?)
}

/// The chat's folder and the shells this device offers, for a new terminal tab. It holds nothing,
/// so a tab never carries anything that expires; `project_terminal_open` checks both again.
#[tauri::command]
#[specta::specta]
pub async fn project_terminal_review(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<ProjectTerminalReview, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.project_terminal_review(&chat_id).await?)
}

/// Reserves a project shell for the docked panel or the reviewed sheet. A window may hold several;
/// the app holds at most eight. The shell itself starts only through `project_terminal_open`.
#[tauri::command]
#[specta::specta]
pub fn project_terminal_reserve(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<String, AppError> {
    current(&webview, &state)?;
    Ok(state.terminals.reserve_project(webview.label())?.pending())
}

/// Opens a project shell in a reserved terminal (`project_terminal_reserve`) in one step: Rust
/// checks the chat's folder and the shell, prepares it and starts it, with no review token in
/// between, so a tab first shown long after it was made still opens. `shell: null` takes the
/// first shell this device offers.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn project_terminal_open(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    shell: Option<ProjectShell>,
    terminal_id: String,
    rows: u16,
    cols: u16,
) -> Result<ProjectTerminalOpened, AppError> {
    current(&webview, &state)?;
    let size = PtySize { rows, cols }
        .validate()
        .map_err(|_| TerminalError::Resize)?;
    let ticket = state.terminals.claim(webview.label(), &terminal_id)?;
    let admission = state.sessions()?;
    let (plan, shell) = admission.open_project_terminal(&chat_id, shell).await?;
    current(&webview, &state)?;
    let chats = state.chats().ok_or(TerminalError::Closed)?;
    let spawner = Spawner::new(
        chats.registry.clone(),
        tauri::async_runtime::handle().inner().clone(),
    );
    let terminal_id = ticket
        .start_project(spawner, plan.spec, size, Box::new((plan.guard, admission)))
        .await?;
    Ok(ProjectTerminalOpened { terminal_id, shell })
}

/// The URL that opens `path` in `editor`: a folder as `scheme://file/<folder>/`, a file as
/// `scheme://file/<file>` with `:line` and `:line:column` when known, the form the VS Code
/// builds and Cursor read.
/// A worktree chat's Run: its saved command, if any, and its worktree's first port; `null` for a
/// chat that does not work in a worktree of its own.
#[tauri::command]
#[specta::specta]
pub async fn worktree_run(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<Option<ghost_protocol::worktree_setup::WorktreeRun>, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.worktree_run(&chat_id).await?)
}

/// Starts Run in a reserved project terminal (`project_terminal_reserve`): the saved Run command
/// in the chat's worktree, with `GHOST_PORT` and `PORT`. Closing the terminal stops it.
#[tauri::command]
#[specta::specta]
pub async fn worktree_run_start(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    terminal_id: String,
    rows: u16,
    cols: u16,
) -> Result<String, AppError> {
    current(&webview, &state)?;
    let size = PtySize { rows, cols }
        .validate()
        .map_err(|_| TerminalError::Resize)?;
    let ticket = state.terminals.claim(webview.label(), &terminal_id)?;
    let admission = state.sessions()?;
    let plan = admission.prepare_worktree_run(&chat_id).await?;
    current(&webview, &state)?;
    let chats = state.chats().ok_or(TerminalError::Closed)?;
    let spawner = Spawner::new(
        chats.registry.clone(),
        tauri::async_runtime::handle().inner().clone(),
    );
    Ok(ticket
        .start_project(spawner, plan.spec, size, Box::new((plan.guard, admission)))
        .await?)
}

fn editor_url(editor: ProjectEditor, path: &str, target: UrlTarget) -> Result<Url, AppError> {
    let scheme = match editor {
        ProjectEditor::Cursor => "cursor",
        ProjectEditor::VsCode => "vscode",
        ProjectEditor::VsCodeInsiders => "vscode-insiders",
        // These open through their detected launcher, never a URL.
        ProjectEditor::Zed
        | ProjectEditor::IntellijIdea
        | ProjectEditor::WebStorm
        | ProjectEditor::PyCharm
        | ProjectEditor::RustRover
        | ProjectEditor::GoLand
        | ProjectEditor::CLion => {
            return Err(AppError::new(
                AppErrorCode::InvalidArgument,
                "This editor opens through its own launcher.",
            ));
        }
    };
    let mut url = Url::parse(&format!("{scheme}://file/")).map_err(|_| {
        AppError::new(
            AppErrorCode::InvalidArgument,
            "This editor's link handler is unavailable.",
        )
    })?;
    // Preserve a native path as one URI path, never an authority, query, fragment or command.
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.into()
    };
    if path.is_empty() || path.contains('\0') {
        return Err(AppError::new(
            AppErrorCode::InvalidArgument,
            "This path cannot be opened in an editor.",
        ));
    }
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    let path = path.trim_end_matches('/').replace('%', "%25");
    let place = match target {
        UrlTarget::Folder => "/".to_owned(),
        UrlTarget::File { line: None, .. } => String::new(),
        UrlTarget::File {
            line: Some(line),
            column: None,
        } => format!(":{line}"),
        UrlTarget::File {
            line: Some(line),
            column: Some(column),
        } => format!(":{line}:{column}"),
    };
    url.set_path(&format!("{path}{place}"));
    Ok(url)
}

/// Hands `open` to the operating system: a URL to its handler, or the detected launcher started
/// detached. The editor is the person's, never an owned process.
async fn dispatch(
    app: &AppHandle,
    editor: ProjectEditor,
    open: EditorOpen,
) -> Result<(), AppError> {
    match open {
        EditorOpen::Url { path, target } => {
            let url = editor_url(editor, &path, target)?;
            app.opener()
                .open_url(url.as_str(), None::<&str>)
                .map_err(|_| {
                    AppError::new(
                        AppErrorCode::AgentUnavailable,
                        "The operating system could not open this editor. Install or repair its URL handler, then try again.",
                    )
                })
        }
        EditorOpen::Launch(launch) => tauri::async_runtime::spawn_blocking(move || launch.spawn())
            .await
            .map_err(|_| {
                AppError::new(
                    AppErrorCode::AgentUnavailable,
                    "The editor launch stopped unexpectedly.",
                )
            })?
            .map_err(|_| {
                AppError::new(
                    AppErrorCode::AgentUnavailable,
                    "The operating system could not start this editor. Check its installation, then try again.",
                )
            }),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn project_editor_open(
    app: AppHandle,
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
    editor: ProjectEditor,
) -> Result<(), AppError> {
    current(&webview, &state)?;
    let admission = state.sessions()?;
    let open = admission
        .project_editor_open(&chat_id, &token, editor)
        .await?;
    current(&webview, &state)?;
    dispatch(&app, editor, open).await?;
    // The profile lease stays alive through the OS dispatch receipt, not the editor's lifetime.
    drop(admission);
    Ok(())
}

/// Settings' Editor, when the profile's preferences can be read.
fn preferred_editor(state: &AppState) -> Option<ProjectEditor> {
    state
        .setup_service()
        .and_then(|service| service.preferences().ok())
        .and_then(|preferences| preferences.general.editor)
}

/// The editors installed in their standard locations, for Settings' Editor, and the one a chat's
/// files open in.
#[tauri::command]
#[specta::specta]
pub async fn editors_detected(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<DetectedEditors, AppError> {
    current(&webview, &state)?;
    let editors = state.sessions()?.detected_editors().await;
    let opens_in = preferred_editor(&state)
        .filter(|editor| editors.contains(editor))
        .or_else(|| editors.first().copied());
    Ok(DetectedEditors { editors, opens_in })
}

/// For each of a chat's file references (from answers, tool rows and Changes), the
/// folder-relative path of the file it names in the chat's folder, or `null`. Up to 200 at once.
#[tauri::command]
#[specta::specta]
pub async fn chat_file_check(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    references: Vec<String>,
) -> Result<Vec<Option<String>>, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .chat_file_check(&chat_id, references)
        .await?)
}

/// Opens the file a chat's reference names, at its line and column when given, in Settings'
/// editor (or the first one detected), and says which editor it was.
#[tauri::command]
#[specta::specta]
pub async fn chat_file_open(
    app: AppHandle,
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    reference: String,
    line: Option<u32>,
    column: Option<u32>,
) -> Result<ProjectEditor, AppError> {
    current(&webview, &state)?;
    let preferred = preferred_editor(&state);
    let admission = state.sessions()?;
    let (editor, open) = admission
        .chat_file_open(&chat_id, reference, line, column, preferred)
        .await?;
    current(&webview, &state)?;
    dispatch(&app, editor, open).await?;
    drop(admission);
    Ok(editor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_uri_cannot_inject_commands_queries_or_fragments() {
        for editor in [
            ProjectEditor::Cursor,
            ProjectEditor::VsCode,
            ProjectEditor::VsCodeInsiders,
        ] {
            let url = editor_url(editor, "/project/a # ? % folder", UrlTarget::Folder).unwrap();
            assert_eq!(url.host_str(), Some("file"));
            assert!(url.query().is_none());
            assert!(url.fragment().is_none());
            assert!(url.as_str().contains("%23"));
            assert!(url.as_str().contains("%3F"));
            assert!(url.as_str().contains("%25"));
        }
        assert!(
            editor_url(
                ProjectEditor::Cursor,
                "//server/share/a%2Fb",
                UrlTarget::Folder
            )
            .unwrap()
            .as_str()
            .ends_with("//server/share/a%252Fb/")
        );
        // A file has no trailing slash and takes its place after a colon.
        let file = |line, column| {
            editor_url(
                ProjectEditor::VsCode,
                "/project/src/a #1.ts",
                UrlTarget::File { line, column },
            )
            .unwrap()
        };
        assert_eq!(
            file(None, None).as_str(),
            "vscode://file/project/src/a%20%231.ts"
        );
        assert_eq!(
            file(Some(42), None).as_str(),
            "vscode://file/project/src/a%20%231.ts:42"
        );
        let place = file(Some(42), Some(7));
        assert_eq!(place.as_str(), "vscode://file/project/src/a%20%231.ts:42:7");
        assert!(place.query().is_none() && place.fragment().is_none());
        // Zed and the JetBrains IDEs never open through a URL, only their detected launcher.
        for editor in [
            ProjectEditor::Zed,
            ProjectEditor::IntellijIdea,
            ProjectEditor::WebStorm,
            ProjectEditor::PyCharm,
            ProjectEditor::RustRover,
            ProjectEditor::GoLand,
            ProjectEditor::CLion,
        ] {
            assert!(editor_url(editor, "/project", UrlTarget::Folder).is_err());
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn git_initialize_preview(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    initial_branch: String,
) -> Result<GitInitializePreview, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .git_initialize_preview(&chat_id, &initial_branch)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn git_initialize_apply(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<GitWorkspace, AppError> {
    current(&webview, &state)?;
    let admission = state.sessions()?;
    let manager = (*admission).clone();
    Ok(manager
        .git_initialize_apply(&chat_id, &token, Box::new(admission))
        .await?)
}
