//! Page/profile-bound one-use vendor reviews. The terminal owns all guards through cleanup.
use crate::{
    config_library::LibraryService,
    error::{AppError, AppErrorCode},
    sessions::PageToken,
    state::{AppState, ProfileGeneration},
};
use ghost_core::{session::PreparedVendor, terminal::TerminalError};
use ghost_protocol::config_vendor::{VendorRequest, VendorReview};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::{State, Webview};

struct Pending {
    owner: String,
    page: PageToken,
    generation: ProfileGeneration,
    prepared: PreparedVendor,
    _slot: tokio::sync::OwnedSemaphorePermit,
}
pub struct VendorService {
    pending: Mutex<HashMap<String, Pending>>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Default for VendorService {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}
impl VendorService {
    pub(crate) fn reset_profile(&self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
    pub(crate) fn drop_owner(&self, owner: &str) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, value| value.owner != owner);
    }
}
fn stale() -> AppError {
    AppError::new(
        AppErrorCode::InvalidArgument,
        "This integration review expired or changed. Open a fresh review.",
    )
}

#[tauri::command]
#[specta::specta]
pub async fn config_vendor_review(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<VendorService>>,
    request: VendorRequest,
) -> Result<VendorReview, AppError> {
    if state.quit.is_quitting() {
        return Err(TerminalError::Closed.into());
    }
    let page = state.pages.for_view(webview.label());
    if !page.is_current() {
        return Err(stale());
    }
    let sessions = state.sessions()?;
    service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|_, value| {
            !value.prepared.expired() && value.page.is_current() && value.generation.is_current()
        });
    let slot = service.slots.clone().try_acquire_owned().map_err(|_| {
        AppError::new(
            AppErrorCode::TooManyStreams,
            "Close another integration review first.",
        )
    })?;
    let prepared = sessions.review_config_vendor(request).await?;
    if !page.is_current() || state.quit.is_quitting() {
        return Err(stale());
    }
    let mut pending = service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pending.retain(|_, value| {
        !value.prepared.expired() && value.page.is_current() && value.generation.is_current()
    });
    if pending.len() >= 4 {
        return Err(AppError::new(
            AppErrorCode::TooManyStreams,
            "Close another integration review first.",
        ));
    }
    let mut review = prepared.review().clone();
    review.token = uuid::Uuid::new_v4().to_string();
    pending.insert(
        review.token.clone(),
        Pending {
            owner: webview.label().into(),
            page,
            generation: sessions.generation(),
            prepared,
            _slot: slot,
        },
    );
    Ok(review)
}

#[tauri::command]
#[specta::specta]
pub fn config_vendor_cancel(
    webview: Webview,
    service: State<'_, Arc<VendorService>>,
    token: String,
) {
    let mut pending = service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending
        .get(&token)
        .is_some_and(|value| value.owner == webview.label())
    {
        pending.remove(&token);
    }
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)] // The IPC carries separate page state, token and terminal size.
pub async fn config_vendor_start(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<VendorService>>,
    library: State<'_, Arc<LibraryService>>,
    token: String,
    terminal_id: String,
    rows: u16,
    cols: u16,
) -> Result<String, AppError> {
    if state.quit.is_quitting() {
        return Err(TerminalError::Closed.into());
    }
    let pending = {
        let mut pending = service
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !pending
            .get(&token)
            .is_some_and(|value| value.owner == webview.label())
        {
            return Err(stale());
        }
        pending.remove(&token).ok_or_else(stale)?
    };
    if pending.prepared.expired() || !pending.page.is_current() || !pending.generation.is_current()
    {
        return Err(stale());
    }
    let size = ghost_process::pty::PtySize { rows, cols }
        .validate()
        .map_err(|_| TerminalError::Resize)?;
    // Claim before waiting on filesystem admission. Close/reload can cancel a queued start.
    let ticket = state.terminals.claim(webview.label(), &terminal_id)?;
    let profile = state.sessions()?;
    let mutation = library.mutation.clone().lock_owned().await;
    if !pending.page.is_current() || !pending.generation.is_current() || state.quit.is_quitting() {
        return Err(stale());
    }
    let launch = profile.start_config_vendor(pending.prepared).await?;
    if !pending.page.is_current() || state.quit.is_quitting() {
        return Err(TerminalError::Closed.into());
    }
    let chats = state.chats().ok_or(TerminalError::Closed)?;
    let spawner = ghost_process::Spawner::new(
        chats.registry.clone(),
        tauri::async_runtime::handle().inner().clone(),
    );
    Ok(ticket
        .start_guarded(
            spawner,
            launch.spec,
            size,
            Box::new((profile, mutation, launch.guard)),
        )
        .await?)
}
