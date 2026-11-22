//! The dictation models o7 bundles (docs/local-dictation.md): Parakeet TDT-CTC 110M for
//! English speech, with punctuation and capitals, and Silero VAD v6.2 for voice activity. They ship
//! in the app's resources, so dictation needs no download. A release bundle refuses to build
//! without the pinned files (apps/desktop/scripts/dictation-models.mjs verifies their SHA-256);
//! the installer and updater signatures cover them from there, so the app checks only their sizes.
//!
//! A larger speech model, Parakeet Ultra, can be fetched once into the data folder (`larger.rs`):
//! it is too large to ride in the installer. Where it is present at its pinned size, dictation
//! uses it in place of the built-in one.
use super::engine::Engine;
use super::{DictationModel, Failure, LargerModel, LargerState};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    pub file: &'static str,
    pub bytes: u64,
}

pub const ASR: Bundled = Bundled {
    file: "parakeet-tdt_ctc-110m-Q6_K.gguf",
    bytes: 112_311_264,
};
pub const VAD: Bundled = Bundled {
    file: "silero-vad-v6.2-F32.gguf",
    bytes: 1_241_056,
};
/// Kept in step with `MODELS` in apps/desktop/scripts/dictation-models.mjs (tested below).
pub const BUNDLED: [Bundled; 2] = [ASR, VAD];

/// A model fetched after installation: where from, and exactly which file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Download {
    pub file: &'static str,
    pub name: &'static str,
    pub bytes: u64,
    /// Lowercase hex.
    pub sha256: &'static str,
    /// Pinned to a revision, so the bytes behind it cannot change.
    pub url: &'static str,
}

/// Parakeet Ultra, Q4_K_M: Moondream's further training of NVIDIA's Parakeet TDT 0.6B v3
/// (CC-BY-4.0), converted for transcribe.cpp by handy-computer. Kept in step with `LARGER` in
/// apps/desktop/scripts/dictation-models.mjs (tested below).
pub const LARGER: Download = Download {
    file: "parakeet-ultra-Q4_K_M.gguf",
    name: "Parakeet Ultra",
    bytes: 486_280_096,
    sha256: "c69b7a5f9071a7afd8a1818ef7ae542c386c4b01ed0cac380c19a99947646bf9",
    url: "https://huggingface.co/handy-computer/parakeet-ultra-gguf/resolve/a1539be1b849ea7463b88343e5277b373d0c4ea9/parakeet-ultra-Q4_K_M.gguf",
};

/// The larger model in `downloads`, when it is there whole. Its SHA-256 was checked before it got
/// its name (`larger.rs`); from then on its size is the check, as for the bundled files.
pub fn larger(downloads: Option<&Path>) -> Option<PathBuf> {
    let path = downloads?.join(LARGER.file);
    std::fs::metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() == LARGER.bytes)
        .then_some(path)
}

/// Whether `folder` holds every bundled model at its pinned size.
pub fn present(folder: &Path) -> bool {
    BUNDLED.iter().all(|model| {
        std::fs::metadata(folder.join(model.file))
            .is_ok_and(|meta| meta.is_file() && meta.len() == model.bytes)
    })
}

pub fn status(folder: Option<&Path>, downloads: Option<&Path>) -> DictationModel {
    let ready = larger(downloads).is_some();
    DictationModel {
        installed: folder.is_some_and(present),
        name: if ready {
            LARGER.name
        } else {
            "Parakeet TDT-CTC 110M"
        }
        .into(),
        bytes: BUNDLED.iter().map(|model| model.bytes).sum::<u64>() as u32,
        supported: super::audio::cpu_supported(),
        larger: LargerModel {
            name: LARGER.name.into(),
            bytes: LARGER.bytes as u32,
            state: if ready {
                LargerState::Ready
            } else if super::larger::downloading() {
                LargerState::Downloading
            } else {
                LargerState::Absent
            },
            // A person can only be offered it where there is somewhere to keep it.
            available: downloads.is_some(),
        },
    }
}

/// The engine for the bundled models in `folder`, loaded the first time it is asked for, with
/// the larger speech model from `downloads` in the built-in one's place when it is there.
pub fn engine(folder: &Path, downloads: Option<&Path>) -> Result<Arc<Engine>, Failure> {
    if !present(folder) {
        return Err(Failure::model(
            "This build of o7 is missing its dictation models. Reinstall o7 to restore them.",
        ));
    }
    let speech = larger(downloads).unwrap_or_else(|| folder.join(ASR.file));
    Engine::shared(&speech, &folder.join(VAD.file))
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
        assert!(!status(Some(folder.path()), None).installed);
        assert!(!status(None, None).installed);
        std::fs::write(folder.path().join(ASR.file), b"short").unwrap();
        std::fs::write(folder.path().join(VAD.file), b"short").unwrap();
        assert!(!present(folder.path()), "sizes must match");
        assert_eq!(
            engine(folder.path(), None)
                .err()
                .map(|failure| failure.reason),
            Some(super::super::DictationFailure::Model)
        );
    }

    #[test]
    fn the_larger_model_matches_the_script_and_counts_only_at_its_pinned_size() {
        let script = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/dictation-models.mjs"),
        )
        .unwrap();
        for needle in [
            format!("file: \"{}\"", LARGER.file),
            format!("size: {},", LARGER.bytes),
            format!("sha256: \"{}\"", LARGER.sha256),
        ] {
            assert!(script.contains(&needle), "{needle}");
        }
        // The address is the script's repo and revision, over HTTPS, at Hugging Face.
        assert!(LARGER.url.starts_with("https://huggingface.co/"));
        assert!(LARGER.url.ends_with(&format!("/{}", LARGER.file)));
        assert_eq!(LARGER.sha256.len(), 64);

        let downloads = tempfile::tempdir().unwrap();
        assert_eq!(larger(None), None);
        assert_eq!(larger(Some(downloads.path())), None);
        let status = status(None, Some(downloads.path()));
        assert_eq!(status.larger.state, LargerState::Absent);
        assert!(status.larger.available);
        assert_eq!(status.name, "Parakeet TDT-CTC 110M");
        // A file of another size under its name is not it, and neither is one still arriving.
        std::fs::write(downloads.path().join(LARGER.file), b"short").unwrap();
        std::fs::write(
            downloads.path().join(format!("{}.partial", LARGER.file)),
            b"arriving",
        )
        .unwrap();
        assert_eq!(larger(Some(downloads.path())), None);
        assert!(!super::super::model::status(None, None).larger.available);
    }
}
