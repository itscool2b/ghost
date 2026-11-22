//! The dictation models o7 bundles (docs/local-dictation.md): Parakeet Ultra for speech, with
//! punctuation and capitals, and Silero VAD v6.2 for voice activity. They ship
//! in the app's resources, so dictation needs no download. A release bundle refuses to build
//! without the pinned files (apps/desktop/scripts/dictation-models.mjs verifies their SHA-256);
//! the installer and updater signatures cover them from there, so the app checks only their sizes.
use super::engine::Engine;
use super::{DictationModel, Failure};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    pub file: &'static str,
    pub bytes: u64,
}

pub const ASR: Bundled = Bundled {
    file: "parakeet-ultra-Q4_K_M.gguf",
    bytes: 486_280_096,
};
pub const VAD: Bundled = Bundled {
    file: "silero-vad-v6.2-F32.gguf",
    bytes: 1_241_056,
};
/// Kept in step with `MODELS` in apps/desktop/scripts/dictation-models.mjs (tested below).
pub const BUNDLED: [Bundled; 2] = [ASR, VAD];

/// Whether `folder` holds every bundled model at its pinned size.
pub fn present(folder: &Path) -> bool {
    BUNDLED.iter().all(|model| {
        std::fs::metadata(folder.join(model.file))
            .is_ok_and(|meta| meta.is_file() && meta.len() == model.bytes)
    })
}

pub fn status(folder: Option<&Path>) -> DictationModel {
    DictationModel {
        installed: folder.is_some_and(present),
        name: "Parakeet Ultra".into(),
        bytes: BUNDLED.iter().map(|model| model.bytes).sum::<u64>() as u32,
        supported: super::audio::cpu_supported(),
    }
}

/// The engine for the bundled models in `folder`, loaded the first time it is asked for.
pub fn engine(folder: &Path) -> Result<Arc<Engine>, Failure> {
    if !present(folder) {
        return Err(Failure::model(
            "This build of o7 is missing its dictation models. Reinstall o7 to restore them.",
        ));
    }
    Engine::shared(folder, ASR.file, VAD.file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_models_match_the_release_fetch_script() {
        let script = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/dictation-models.mjs"),
        )
        .unwrap();
        for model in BUNDLED {
            assert!(
                script.contains(&format!("file: \"{}\"", model.file)),
                "{}",
                model.file
            );
            assert!(
                script.contains(&format!("size: {},", model.bytes)),
                "{}",
                model.file
            );
        }
    }

    #[test]
    fn a_folder_without_the_pinned_files_is_not_ready() {
        let folder = tempfile::tempdir().unwrap();
        assert!(!present(folder.path()));
        assert!(!status(Some(folder.path())).installed);
        assert!(!status(None).installed);
        std::fs::write(folder.path().join(ASR.file), b"short").unwrap();
        std::fs::write(folder.path().join(VAD.file), b"short").unwrap();
        assert!(!present(folder.path()), "sizes must match");
        assert_eq!(
            engine(folder.path()).err().map(|failure| failure.reason),
            Some(super::super::DictationFailure::Model)
        );
    }
}
