//! Remote access: paired browsers and linked bots reaching this computer's chats through the
//! relay (`o7-remote`, `docs/remote-sessions.md`). Off until the person turns it on in Settings.
//!
//! The service belongs to the installation: one connection to the relay, whichever profile is
//! open. A device or a bot link belongs to the profile it was made in and reaches no other
//! (`o7_remote::Registry`): when another profile opens, browsers are told to start over, the
//! other profile's devices are ended, and the relay is given only this profile's bot links.

use std::sync::Arc;

use o7_core::session::{FrameSink, SinkGone, StatusSink};
use o7_protocol::{
    EventBatch, UiAnswer,
    remote::{RemoteBotLink, RemotePairing, RemoteStatus},
    session::{Availability, ChatAttach, ChatStatusUpdate, HistoryPage, PromptAck},
    workspace::{ProjectChatCreate, WorkspaceChat, WorkspaceFilter},
};
use o7_remote::{
    Backend, BackendError, BoxFuture, Frames, Overview, RegistryError, RemoteAgent, Service,
    Statuses,
};
use tauri::{Emitter, Manager, State};

use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

/// Sent with a [`RemoteStatus`] whenever what Settings shows about remote access changed.
pub const REMOTE_CHANGED: &str = "auracode-remote-changed";
/// History pages a browser asks for after the first: small enough to seal into one message.
const PAGE_BYTES: u32 = 256 * 1024;

/// The relay and the website it belongs to. Release builds reach only o7's own; a
/// development build may name a relay on this machine. The site was auracode.si until
/// 19 November 2026: the relay still answers there for copies from before the move
/// (docs/remote-sessions.md), and a copy from after it never goes there.
fn origins() -> (String, String) {
    if cfg!(debug_assertions)
        && let (Ok(relay), Ok(site)) = (
            std::env::var("O7_REMOTE_RELAY"),
            std::env::var("O7_REMOTE_SITE"),
        )
    {
        return (relay, site);
    }
    let site = match option_env!("O7_API_ORIGIN") {
        Some("https://preview.o7.codes") => "https://preview.o7.codes",
        _ => "https://o7.codes",
    };
    (site.replacen("https://", "wss://", 1), site.to_owned())
}

/// Starts the service once the app's state exists. Smoke runs never get one.
pub(crate) fn setup(app: &tauri::AppHandle, runtime: &tokio::runtime::Handle) {
    let state = app.state::<AppState>();
    if state.launch.is_smoke() {
        return;
    }
    let Some(folder) = state.data_dir() else {
        return;
    };
    let (relay, site) = origins();
    let host_name = state
        .app_info()
        .device
        .map(|device| device.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "This computer".into());
    let notify = app.clone();
    let config = o7_remote::Config {
        path: folder.join("remote.json"),
        relay,
        site,
        host_name,
        profile: open_profile(&state),
        backend: Arc::new(AppBackend {
            app: app.clone(),
            profile: String::new(),
        }),
        changed: Arc::new(move || {
            if let Some(service) = notify.try_state::<Service>() {
                let _ = notify.emit(REMOTE_CHANGED, service.status());
            }
        }),
    };
    match Service::start(config, runtime) {
        Ok(service) => {
            app.manage(service);
        }
        // Settings shows why; nothing else depends on remote access.
        Err(error) => {
            tracing::warn!(%error, "remote access is unavailable");
            app.manage(Unavailable(error.to_string()));
        }
    }
}

/// Why the service could not start: its saved settings could not be read.
struct Unavailable(String);

/// The open profile's id, which a device or a bot link is bound to; empty while none is open.
fn open_profile(state: &AppState) -> String {
    state
        .app_info()
        .profile
        .map(|profile| profile.id)
        .unwrap_or_default()
}

pub(crate) fn profile_changed(app: &tauri::AppHandle) {
    if let Some(service) = app.try_state::<Service>() {
        service.profile_changed(&open_profile(&app.state::<AppState>()));
    }
}

pub(crate) fn shutdown(app: &tauri::AppHandle) {
    if let Some(service) = app.try_state::<Service>() {
        service.shutdown();
    }
}

fn service(app: &tauri::AppHandle) -> Result<Service, AppError> {
    if let Some(service) = app.try_state::<Service>() {
        return Ok(service.inner().clone());
    }
    Err(AppError::new(
        AppErrorCode::StorageUnavailable,
        match app.try_state::<Unavailable>() {
            Some(reason) => reason.0.clone(),
            None => "Remote access is not available in this run.".into(),
        },
    ))
}

fn refused(error: RegistryError) -> AppError {
    let code = match error {
        RegistryError::TooManyDevices | RegistryError::TooManyBots | RegistryError::NoWorkspace => {
            AppErrorCode::InvalidArgument
        }
        _ => AppErrorCode::StorageUnavailable,
    };
    AppError::new(code, error.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn remote_status(app: tauri::AppHandle) -> Result<RemoteStatus, AppError> {
    Ok(service(&app)?.status())
}

/// Turns remote access on or off. Off closes the connection at once; paired devices and bot
/// links stay saved, and work again when it is back on.
#[tauri::command]
#[specta::specta]
pub fn remote_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.set_enabled(enabled).map_err(refused)?;
    Ok(service.status())
}

/// Opens a pairing link, good once and for ten minutes. It replaces the link before it.
#[tauri::command]
#[specta::specta]
pub fn remote_pair(app: tauri::AppHandle) -> Result<RemotePairing, AppError> {
    let service = service(&app)?;
    if !service.status().enabled {
        return Err(AppError::invalid_argument(
            "Turn remote access on before pairing a device.",
        ));
    }
    service.pair().map_err(refused)
}

#[tauri::command]
#[specta::specta]
pub fn remote_cancel_pairing(app: tauri::AppHandle) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.cancel_pairing();
    Ok(service.status())
}

/// Removes a paired device. Its open sessions end and its key stops working.
#[tauri::command]
#[specta::specta]
pub fn remote_remove_device(app: tauri::AppHandle, id: String) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.remove_device(&id).map_err(refused)?;
    Ok(service.status())
}

/// Creates a bot link. Its address carries the bot's token and is returned only here.
#[tauri::command]
#[specta::specta]
pub fn remote_create_bot(
    app: tauri::AppHandle,
    name: String,
    full: bool,
) -> Result<RemoteBotLink, AppError> {
    service(&app)?.create_bot(&name, full).map_err(refused)
}

#[tauri::command]
#[specta::specta]
pub fn remote_remove_bot(app: tauri::AppHandle, id: String) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.remove_bot(&id).map_err(refused)?;
    Ok(service.status())
}

/// One profile's chats, as remote sessions reach them. Every call takes the same lease a window's
/// command does, and takes it only while `profile` is the open profile, both read under one lock:
/// a session accepted for one profile is never served another's chats, wherever a switch lands.
/// The service binds a profile before every call; unbound (`profile` empty), nothing is served.
struct AppBackend {
    app: tauri::AppHandle,
    profile: String,
}

fn failed(error: AppError) -> BackendError {
    // The same codes the window gets, in the same words.
    let code = match error.code {
        AppErrorCode::ChatNotFound => "not_found",
        AppErrorCode::RequestNotPending => "request_not_pending",
        AppErrorCode::OptionInvalid => "option_invalid",
        AppErrorCode::PromptEmpty => "empty",
        AppErrorCode::PromptTooLarge => "too_large",
        AppErrorCode::QueueFull => "queue_full",
        AppErrorCode::UnknownAgent | AppErrorCode::AgentUnavailable => "agent_unavailable",
        AppErrorCode::InvalidFolder | AppErrorCode::InvalidArgument => "invalid",
        AppErrorCode::TooManyAgents
        | AppErrorCode::TooManySubscriptions
        | AppErrorCode::FolderBusy => "busy",
        AppErrorCode::StorageUnavailable | AppErrorCode::ShuttingDown => "unavailable",
        _ => "failed",
    };
    BackendError::new(code, error.message)
}

struct FrameAdapter {
    owner: String,
    frames: Frames,
}

impl FrameSink for FrameAdapter {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        if (self.frames)(frame) {
            Ok(())
        } else {
            Err(SinkGone)
        }
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

struct StatusAdapter {
    owner: String,
    statuses: Statuses,
}

impl StatusSink for StatusAdapter {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        if (self.statuses)(update) {
            Ok(())
        } else {
            Err(SinkGone)
        }
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

impl AppBackend {
    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// The bound profile's session manager, under a command lease.
    fn sessions(
        &self,
    ) -> Result<crate::state::ProfileLease<o7_core::session::SessionManager>, AppError> {
        self.state().sessions_of(&self.profile)
    }

    /// The agent a new chat starts with: the one asked for, else the project's, else the one
    /// Settings names, else the first that is ready.
    async fn agent_for(&self, folder_id: &str, asked: Option<&str>) -> Result<String, AppError> {
        if let Some(agent) = asked {
            return Ok(agent.to_owned());
        }
        let sessions = self.sessions()?;
        let snapshot = sessions
            .workspace_snapshot(WorkspaceFilter {
                include_archived: false,
                project_id: None,
            })
            .await?;
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.folders.iter().any(|folder| folder.id == folder_id))
            .ok_or_else(|| AppError::invalid_argument("That folder is no longer here."))?;
        if let Some(agent) = &project.default_agent {
            return Ok(agent.clone());
        }
        if let Some(agent) = default_agent(&self.state()) {
            return Ok(agent);
        }
        sessions
            .agents(false)
            .await
            .into_iter()
            .find(|agent| matches!(agent.availability, Availability::Ready { .. }))
            .map(|agent| agent.id)
            .ok_or_else(|| {
                AppError::new(
                    AppErrorCode::AgentUnavailable,
                    "No agent is ready on this computer.",
                )
            })
    }
}

/// The open profile's theme and appearance, by their saved names, for a paired page to wear.
fn look(state: &AppState) -> Option<o7_remote::Look> {
    let preferences = state.setup_service()?.preferences().ok()?;
    let name = |value: serde_json::Value| value.as_str().map(str::to_owned);
    Some(o7_remote::Look {
        theme: name(serde_json::to_value(preferences.theme).ok()?)?,
        appearance: name(serde_json::to_value(preferences.appearance).ok()?)?,
    })
}

fn default_agent(state: &AppState) -> Option<String> {
    state
        .setup_service()
        .and_then(|setup| setup.preferences().ok())
        .and_then(|preferences| preferences.general.default_agent)
}

/// Whether `path` names a folder by a full path on this computer's own drives. A network share
/// in any spelling (`\\server\share`, `//server/share`, mixed slashes, the `\\?\UNC\` and device
/// forms) is not: opening one makes Windows sign in to that machine. The path is parsed as the
/// system parses it, after its separators are made one kind, so no spelling slips past.
///
/// A drive letter mapped to a network share is the person's own setup and passes: it reaches
/// nothing they have not already connected.
fn on_this_computer(path: &str) -> bool {
    use std::path::{Component, Path, Prefix};
    if cfg!(windows) {
        let unified = path.replace('/', "\\");
        if unified.starts_with("\\\\") {
            return false;
        }
        let mut parts = Path::new(&unified).components();
        matches!(
            (parts.next(), parts.next()),
            (Some(Component::Prefix(prefix)), Some(Component::RootDir))
                if matches!(prefix.kind(), Prefix::Disk(_))
        )
    } else {
        !path.starts_with("//") && Path::new(path).is_absolute()
    }
}

/// The words a bot uses for the three access levels.
fn access_word(autonomy: o7_protocol::Autonomy) -> &'static str {
    match autonomy {
        o7_protocol::Autonomy::Ask => "ask",
        o7_protocol::Autonomy::AutoEdit => "auto_edit",
        o7_protocol::Autonomy::FullAuto => "full_auto",
    }
}

/// A chat's model and thinking choices as its pickers show them, and its access levels.
fn chat_options(lists: &o7_protocol::session::SessionLists) -> o7_remote::ChatOptions {
    use o7_protocol::session::{ConfigCategory, ConfigKind};
    use o7_remote::Choice;
    let select = |category: ConfigCategory| {
        lists
            .options
            .iter()
            .filter(|option| !matches!(option.id.as_str(), "aura.access" | "aura.workflow"))
            .find(|option| option.category == category)
            .and_then(|option| match &option.kind {
                ConfigKind::Select { current, choices } => Some((
                    choices
                        .iter()
                        .map(|choice| Choice {
                            value: choice.value.clone(),
                            name: choice.name.clone(),
                        })
                        .collect::<Vec<_>>(),
                    Some(current.clone()),
                )),
                ConfigKind::Toggle { .. } => None,
            })
            .unwrap_or_default()
    };
    let (models, model) = select(ConfigCategory::Model);
    let (thinking, thinking_level) = select(ConfigCategory::Effort);
    let native = &lists.controls.access.choices;
    let access = if native.is_empty() {
        [
            o7_protocol::Autonomy::Ask,
            o7_protocol::Autonomy::AutoEdit,
            o7_protocol::Autonomy::FullAuto,
        ]
        .into_iter()
        .map(|level| Choice {
            value: access_word(level).into(),
            name: access_word(level).replace('_', " "),
        })
        .collect()
    } else {
        native
            .iter()
            .filter_map(|choice| {
                choice.legacy_autonomy.map(|level| Choice {
                    value: access_word(level).into(),
                    name: choice.name.clone(),
                })
            })
            .collect()
    };
    o7_remote::ChatOptions {
        models,
        model,
        thinking,
        thinking_level,
        access,
        access_mode: Some(access_word(lists.autonomy.requested).into()),
    }
}

impl Backend for AppBackend {
    fn bound(self: Arc<Self>, profile: &str) -> Arc<dyn Backend> {
        Arc::new(Self {
            app: self.app.clone(),
            profile: profile.to_owned(),
        })
    }

    fn look(&self) -> Option<o7_remote::Look> {
        look(&self.state())
    }

    /// A full-control link's actions, each the call the window itself makes, under the same
    /// lease and only while this backend's profile is the open one.
    fn act(
        &self,
        action: o7_remote::Action,
    ) -> BoxFuture<'_, Result<o7_remote::Outcome, BackendError>> {
        use o7_protocol::session::{ConfigCategory, ConfigValue, SessionControlChange};
        use o7_remote::{Action, Outcome};
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            // A model or a thinking level, by the value its picker would send.
            let choose = |chat_id: String,
                          category: ConfigCategory,
                          what: &'static str,
                          value: String| {
                let sessions = &sessions;
                async move {
                    let lists = sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let offered = chat_options(&lists);
                    let choices = if category == ConfigCategory::Model {
                        &offered.models
                    } else {
                        &offered.thinking
                    };
                    if !choices.iter().any(|choice| choice.value == value) {
                        return Err(BackendError::new(
                            "invalid",
                            format!(
                                "This chat offers no such {what}. Call chat_options for the list."
                            ),
                        ));
                    }
                    let option = lists
                        .options
                        .iter()
                        .filter(|option| {
                            !matches!(option.id.as_str(), "aura.access" | "aura.workflow")
                        })
                        .find(|option| option.category == category)
                        .map(|option| option.id.clone())
                        .ok_or_else(|| {
                            BackendError::new(
                                "invalid",
                                format!("This agent has no {what} to choose."),
                            )
                        })?;
                    sessions
                        .set_option(
                            &chat_id,
                            &option,
                            ConfigValue::Choice {
                                value: value.clone(),
                            },
                        )
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(format!("The {what} is now {value}.")))
                }
            };
            match action {
                Action::CreateProject { path, name } => {
                    // A folder on this computer, by its full path: not a network share, whose
                    // name alone makes Windows reach out to another machine.
                    if !on_this_computer(&path) {
                        return Err(BackendError::new(
                            "invalid",
                            "Give a folder on the computer itself, not a network path.",
                        ));
                    }
                    let name = name.unwrap_or_else(|| {
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| "Project".into())
                    });
                    let project = sessions
                        .project_create(o7_protocol::workspace::ProjectCreate {
                            project: o7_protocol::workspace::ProjectInput {
                                name,
                                color: None,
                                default_agent: None,
                                default_model: None,
                            },
                            folder: path,
                        })
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Project {
                        folder_id: project.folders.first().map(|folder| folder.id.clone()),
                        id: project.id,
                        name: project.name,
                    })
                }
                Action::Options { chat_id } => Ok(Outcome::Options(chat_options(
                    &sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?,
                ))),
                Action::SetModel { chat_id, model } => {
                    choose(chat_id, ConfigCategory::Model, "model", model).await
                }
                Action::SetThinking { chat_id, level } => {
                    choose(chat_id, ConfigCategory::Effort, "thinking level", level).await
                }
                Action::SetAccess { chat_id, access } => {
                    let wanted = match access.as_str() {
                        "ask" => o7_protocol::Autonomy::Ask,
                        "auto_edit" => o7_protocol::Autonomy::AutoEdit,
                        "full_auto" => o7_protocol::Autonomy::FullAuto,
                        _ => return Err(BackendError::new("invalid", "No such access level.")),
                    };
                    let lists = sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let native = &lists.controls.access.choices;
                    // As the window's picker does: the agent's own level where it has them.
                    if native.is_empty() {
                        sessions
                            .set_autonomy(&chat_id, wanted)
                            .await
                            .map_err(|error| failed(error.into()))?;
                    } else {
                        let choice = native
                            .iter()
                            .find(|choice| choice.legacy_autonomy == Some(wanted))
                            .ok_or_else(|| {
                                BackendError::new(
                                    "invalid",
                                    "This agent does not offer that access level. Call chat_options.",
                                )
                            })?;
                        sessions
                            .set_control(
                                &chat_id,
                                SessionControlChange::Access {
                                    value: choice.value.clone(),
                                },
                            )
                            .await
                            .map_err(|error| failed(error.into()))?;
                    }
                    Ok(Outcome::Done(format!("Access is now {access}.")))
                }
                Action::SwitchAgent { chat_id, agent_id } => {
                    // As the window does: the agent stops, then the app's own handoff is prepared
                    // and applied. A chat that is already stopped answers the stop with an error
                    // that does not matter here; the handoff itself refuses a chat still working.
                    let _ = sessions.stop(&chat_id).await;
                    let preview = sessions
                        .continuation_preview(
                            &chat_id,
                            o7_protocol::conversation::ContinuationAction::SwitchAgent { agent_id },
                        )
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let result = sessions
                        .continuation_apply(&chat_id, &preview.token)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Chat { id: result.chat_id })
                }
                Action::Rename { chat_id, title } => {
                    sessions
                        .chat_rename(&chat_id, &title)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done("Renamed.".into()))
                }
                Action::Archive { chat_id, archived } => {
                    sessions
                        .chat_archive(&chat_id, archived)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(
                        if archived {
                            "Archived."
                        } else {
                            "Restored from the archive."
                        }
                        .into(),
                    ))
                }
                Action::StartAgent { chat_id } => {
                    sessions
                        .start(&chat_id, false)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(
                        "The agent is starting. Call wait_for_chat or send_message.".into(),
                    ))
                }
                Action::Changes { chat_id } => {
                    let changes = sessions
                        .chat_changes(&chat_id, None)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Changes {
                        files: changes
                            .files
                            .iter()
                            .map(|file| {
                                let kind = match file.kind {
                                    o7_protocol::changes::ChangeKind::Added => "added",
                                    o7_protocol::changes::ChangeKind::Modified => "modified",
                                    o7_protocol::changes::ChangeKind::Deleted => "deleted",
                                };
                                (file.path.clone(), kind.to_owned())
                            })
                            .collect(),
                        unavailable: changes.unavailable_reason,
                    })
                }
                Action::Diff { chat_id, path } => {
                    let changes = sessions
                        .chat_changes(&chat_id, None)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let checkpoint = changes.checkpoint.ok_or_else(|| {
                        BackendError::new("not_found", "This chat has no saved changes to read.")
                    })?;
                    // Only a file the last turn changed: the path is looked up among them, never
                    // opened from the disk by name.
                    if !changes.files.iter().any(|file| file.path == path) {
                        return Err(BackendError::new(
                            "not_found",
                            "The last turn did not change that file. Call list_changes.",
                        ));
                    }
                    let diff = sessions
                        .chat_change_diff(&chat_id, &checkpoint.id, &path)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Diff {
                        before: diff.before,
                        after: diff.after,
                        binary: diff.binary,
                        truncated: diff.truncated,
                    })
                }
                Action::ReadMemory { project_id } => Ok(Outcome::Memory {
                    markdown: sessions
                        .read_project_memory(&project_id)
                        .await
                        .map_err(|error| failed(error.into()))?
                        .markdown,
                }),
                Action::WriteMemory {
                    project_id,
                    markdown,
                } => {
                    // Memory reaches every later chat of the project: the person's own switch
                    // for agents' edits covers a bot's as well.
                    if !sessions.agent_memory_writes() {
                        return Err(BackendError::new(
                            "refused",
                            "The owner has turned off edits to project memory. Tell them what you would have saved.",
                        ));
                    }
                    let current = sessions
                        .read_project_memory(&project_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    // Saved as a bot's, not as the person's: the window shows the edit and
                    // offers to undo it, as it does for an agent's.
                    sessions
                        .bot_save_project_memory(o7_protocol::memory::SaveProjectMemory {
                            project_id,
                            expected_revision: current.revision,
                            markdown,
                            reason: "Edited by a linked bot".into(),
                        })
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done("Saved the project's memory.".into()))
                }
            }
        })
    }

    fn overview(&self) -> BoxFuture<'_, Result<Overview, BackendError>> {
        Box::pin(async move {
            let state = self.state();
            let sessions = self.sessions().map_err(failed)?;
            let snapshot = sessions
                .workspace_snapshot(WorkspaceFilter {
                    include_archived: false,
                    project_id: None,
                })
                .await
                .map_err(|error| failed(error.into()))?;
            let agents = sessions
                .agents(false)
                .await
                .into_iter()
                .map(|agent| RemoteAgent {
                    available: matches!(agent.availability, Availability::Ready { .. }),
                    id: agent.id,
                    name: agent.name,
                })
                .collect();
            Ok(Overview {
                host_name: state
                    .app_info()
                    .device
                    .map(|device| device.name)
                    .unwrap_or_default(),
                projects: snapshot.projects,
                chats: snapshot.chats,
                agents,
                default_agent: default_agent(&state),
            })
        })
    }

    fn watch(&self, owner: &str, statuses: Statuses) -> Result<(), BackendError> {
        // No lease: the subscription belongs to the manager, and ends with it.
        let chats = self.state().chats_of(&self.profile).map_err(failed)?;
        chats.manager.status_subscribe(Arc::new(StatusAdapter {
            owner: owner.to_owned(),
            statuses,
        }));
        Ok(())
    }

    fn attach<'a>(
        &'a self,
        chat_id: &'a str,
        owner: &'a str,
        frames: Frames,
    ) -> BoxFuture<'a, Result<ChatAttach, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            // One chat per browser: what it followed before is let go first.
            sessions.drop_owner(owner);
            sessions
                .attach(
                    chat_id,
                    Arc::new(FrameAdapter {
                        owner: owner.to_owned(),
                        frames,
                    }),
                )
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn release(&self, owner: &str) {
        if let Some(chats) = self.state().chats() {
            chats.manager.drop_owner(owner);
        }
    }

    fn history<'a>(
        &'a self,
        chat_id: &'a str,
        before_seq: u32,
    ) -> BoxFuture<'a, Result<HistoryPage, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .history(chat_id, before_seq, Some(PAGE_BYTES))
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .send(chat_id, text)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn interrupt<'a>(&'a self, chat_id: &'a str) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .interrupt(chat_id)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn permission<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        option_id: Option<String>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .respond_permission(chat_id, request_id, option_id)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn answer<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        answer: Option<UiAnswer>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .respond_ui(chat_id, request_id, answer)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn create<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<WorkspaceChat, BackendError>> {
        Box::pin(async move {
            let agent_id = self.agent_for(folder_id, agent_id).await.map_err(failed)?;
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .chat_project_create(ProjectChatCreate {
                    agent_id,
                    folder_id: folder_id.to_owned(),
                    // Asked for from afar, there is nobody to show the busy-folder warning to.
                    allow_busy_folder: true,
                })
                .await
                .map_err(|error| failed(error.into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::on_this_computer;

    #[test]
    fn a_project_folder_is_a_full_path_on_this_computers_own_drives() {
        #[cfg(windows)]
        {
            for local in [
                r"C:\work\site",
                "C:/work/site",
                r"d:\",
                r"C:\work/mixed\path",
            ] {
                assert!(on_this_computer(local), "{local}");
            }
            // A network share, however it is spelled, and the forms that reach one.
            for remote in [
                r"\server\share",
                "//server/share",
                r"\/server\share",
                r"/\server/share",
                r"\?\UNC\server\share",
                r"\.\pipe\name",
                r"\?\C:\work",
                "//?/UNC/server/share",
            ] {
                assert!(!on_this_computer(remote), "{remote}");
            }
            // Not a full path: relative, drive-relative, or rooted with no drive.
            for partial in ["work", r"C:work", r"\work", "/work", "", "C:"] {
                assert!(!on_this_computer(partial), "{partial}");
            }
        }
        #[cfg(not(windows))]
        {
            assert!(on_this_computer("/home/someone/site"));
            for other in ["//server/share", "site", ""] {
                assert!(!on_this_computer(other), "{other}");
            }
        }
    }
}
