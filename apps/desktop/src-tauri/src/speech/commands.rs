use super::{Failure, Operation, SpeechEvent, SpeechStatus, model, playback};
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Manager, State, Webview, ipc::Channel};

/// The most text one request may read: about twenty minutes of speech. An answer longer than
/// this is cut by the page at the last whole sentence that fits.
pub const MAX_TEXT_BYTES: usize = 24_000;

fn error(message: String) -> AppError {
    AppError::new(AppErrorCode::InvalidArgument, message)
}

/// Where the bundled model is: `speech/` in the app's resources (tauri.conf.json).
fn models(webview: &Webview) -> Option<PathBuf> {
    webview
        .app_handle()
        .path()
        .resource_dir()
        .ok()
        .map(|resources| resources.join("speech"))
}

/// A voice a request may name: the model's own, or an id of the catalogue's form. Which voices
/// exist is decided from the bundled files, never from this.
fn voice_name(voice: &Option<String>) -> Result<(), AppError> {
    match voice.as_deref() {
        None | Some(model::OWN_VOICE_ID) => Ok(()),
        Some(id) if model::voice_id(id) => Ok(()),
        Some(_) => Err(error("That is not one of Ghost's voices.".into())),
    }
}

/// What may be read aloud: real text of a sane length.
fn readable(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Err(error("There is nothing to read aloud.".into()));
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(error("That is too much to read aloud at once.".into()));
    }
    if text.contains('\0') {
        return Err(error("That text cannot be read aloud.".into()));
    }
    Ok(())
}

fn operation(
    view: &Webview,
    state: &AppState,
    id: String,
    channel: Channel<SpeechEvent>,
) -> Result<Operation, AppError> {
    let page = state.pages.for_view(view.label());
    if state.quit.is_quitting() || !page.is_current() {
        return Err(error("This page is no longer active.".into()));
    }
    let profile = state.sessions()?.generation();
    state
        .speech
        .begin(
            view.label(),
            id,
            Arc::new(move |event| channel.send(event).is_ok()),
            Box::new(move || page.is_current() && profile.is_current()),
        )
        .map_err(error)
}

/// Whether reading aloud can run here, and the voices to choose from.
#[tauri::command]
#[specta::specta]
pub fn speech_status(webview: Webview) -> SpeechStatus {
    model::status(models(&webview).as_deref())
}

/// Reads `text` aloud in `voice` (the model's own when none is named) until it is all said or
/// `speech_stop`. Whatever was being read stops first. The text is spoken on this device and is
/// neither stored nor logged.
#[tauri::command]
#[specta::specta]
pub fn speech_speak(
    webview: Webview,
    state: State<'_, AppState>,
    id: String,
    text: String,
    voice: Option<String>,
    channel: Channel<SpeechEvent>,
) -> Result<(), AppError> {
    readable(&text)?;
    voice_name(&voice)?;
    let folder = models(&webview);
    let operation = operation(&webview, &state, id, channel)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !crate::dictation::cpu_supported() {
                return Err(Failure::unsupported());
            }
            let folder = folder.ok_or_else(|| {
                Failure::model("Ghost could not find its voice model. Reinstall Ghost.")
            })?;
            if !operation.emit(SpeechEvent::Preparing) {
                return Ok(());
            }
            let engine = model::engine(&folder)?;
            playback::speak(&engine, &text, voice.as_deref(), &operation)
        }))
        .unwrap_or_else(|_| {
            Err(Failure::other(
                "Reading aloud stopped unexpectedly. Try again.",
            ))
        });
        operation.finish(result);
    });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn speech_stop(webview: Webview, state: State<'_, AppState>, id: String) {
    state.speech.stop(webview.label(), &id);
}

/// Loads the model and makes `voice` ready ahead of time, so that the first request starts at
/// once. Asked for when the pointer reaches a read-aloud button; it says nothing and reports
/// nothing, and one preparation runs at a time.
#[tauri::command]
#[specta::specta]
pub fn speech_prepare(webview: Webview, voice: Option<String>) -> Result<(), AppError> {
    static BUSY: AtomicBool = AtomicBool::new(false);
    voice_name(&voice)?;
    let Some(folder) = models(&webview) else {
        return Ok(());
    };
    if !crate::dictation::cpu_supported() || BUSY.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::panic::catch_unwind(|| {
            if let Ok(engine) = model::engine(&folder) {
                engine.warm(engine.voice(voice.as_deref()));
            }
        });
        BUSY.store(false, Ordering::Release);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_text_of_a_sane_length_is_read() {
        assert!(readable("All tests pass.").is_ok());
        assert!(readable(&"a".repeat(MAX_TEXT_BYTES)).is_ok());
        for refused in [
            String::new(),
            "  \n\t ".into(),
            "a".repeat(MAX_TEXT_BYTES + 1),
            "nul\0inside".into(),
        ] {
            assert!(
                readable(&refused).is_err(),
                "{:?}",
                &refused[..refused.len().min(12)]
            );
        }
    }

    #[test]
    fn a_request_names_the_models_own_voice_or_a_catalogue_id_and_never_a_path() {
        for accepted in [None, Some("default"), Some("NbpkqMVS3CJeq2j8")] {
            assert!(
                voice_name(&accepted.map(str::to_owned)).is_ok(),
                "{accepted:?}"
            );
        }
        for refused in [
            "",
            "Zoey",
            "../voices/NbpkqMVS3CJeq2j8",
            "voices/NbpkqMVS3CJeq2j8.safetensors",
            "C:\\Windows\\win.ini",
            "NbpkqMVS3CJeq2j8\n",
        ] {
            assert!(
                voice_name(&Some(refused.to_owned())).is_err(),
                "{refused:?}"
            );
        }
    }
}
