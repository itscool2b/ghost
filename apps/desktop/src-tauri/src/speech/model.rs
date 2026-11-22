//! The voice model o7 bundles for reading aloud (docs/read-aloud.md): Gradium's Phonon 31M,
//! English, Apache 2.0, with its tokenizer, its own voice and a catalogue of voices. They ship in
//! the app's resources, so reading aloud needs no download. A release bundle refuses to build
//! without the pinned files (apps/desktop/scripts/dictation-models.mjs verifies their SHA-256);
//! the installer and updater signatures cover them from there, so the app checks only their sizes.
use super::Failure;
use super::engine::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    pub file: &'static str,
    pub bytes: u64,
}

pub const WEIGHTS: Bundled = Bundled {
    file: "model.q8.gguf",
    bytes: 64_512_352,
};
pub const CONFIG: Bundled = Bundled {
    file: "config.json",
    bytes: 4_634,
};
pub const TOKENIZER: Bundled = Bundled {
    file: "tokenizer.json",
    bytes: 277_967,
};
/// The voice the model ships with, registered as `default`.
pub const OWN_VOICE: Bundled = Bundled {
    file: "default-voice.safetensors",
    bytes: 256_136,
};
/// Kept in step with `SPEECH` in apps/desktop/scripts/dictation-models.mjs (tested below).
pub const BUNDLED: [Bundled; 4] = [WEIGHTS, CONFIG, TOKENIZER, OWN_VOICE];

/// Every catalogued voice is one tensor of the same shape, so one size.
pub const VOICE_BYTES: u64 = 256_136;
/// The catalogue of the voices beside the model: who each is, in words (committed to the
/// repository; the voice files themselves are fetched by the release script).
pub const CATALOG: &str = "voices.json";
pub const VOICES: &str = "voices";
/// The id of the model's own voice, and the one used when a request names none or an unknown one.
pub const OWN_VOICE_ID: &str = "default";

/// A voice the person can choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SpeechVoice {
    /// What a request names: `default`, or the catalogue's id for the voice.
    pub id: String,
    pub name: String,
    /// `en-us`, `en-gb`, or `characters` for the storybook voices.
    pub locale: String,
    pub gender: Option<String>,
    /// One line on how it sounds, from the catalogue.
    pub description: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SpeechStatus {
    /// The bundled model is here.
    pub installed: bool,
    /// This processor can run the speech engine.
    pub supported: bool,
    pub name: String,
    /// The voices that can be chosen, the model's own first.
    pub voices: Vec<SpeechVoice>,
}

/// Whether `folder` holds the model at its pinned sizes. Catalogued voices are extra: one that
/// is missing is simply not offered.
pub fn present(folder: &Path) -> bool {
    BUNDLED
        .iter()
        .all(|model| sized(&folder.join(model.file), model.bytes))
}

fn sized(path: &Path, bytes: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == bytes)
}

/// A catalogue id: the letters, digits, dashes and underscores the catalogue uses, and never a
/// path (no dot, no slash).
pub fn voice_id(id: &str) -> bool {
    (8..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The file of a catalogued voice. Only an id of the catalogue's form names one.
fn voice_path(folder: &Path, id: &str) -> Option<PathBuf> {
    voice_id(id).then(|| folder.join(VOICES).join(format!("{id}.safetensors")))
}

/// The voices `folder` can speak with: the model's own, then every catalogued voice whose file
/// is here at its size, in the catalogue's order.
pub fn voices(folder: &Path) -> Vec<SpeechVoice> {
    let mut voices = vec![SpeechVoice {
        id: OWN_VOICE_ID.into(),
        name: "Reuben".into(),
        locale: "en-us".into(),
        gender: None,
        description: "The voice the model ships with.".into(),
    }];
    let catalogue = std::fs::read(folder.join(CATALOG))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<SpeechVoice>>(&bytes).ok())
        .unwrap_or_default();
    for voice in catalogue {
        let here = voice_path(folder, &voice.id).is_some_and(|path| sized(&path, VOICE_BYTES));
        if here && voices.iter().all(|known| known.id != voice.id) {
            voices.push(voice);
        }
    }
    voices
}

/// The catalogued voices of `folder` with their files, for the engine to register.
pub(super) fn voice_files(folder: &Path) -> Vec<(String, PathBuf)> {
    voices(folder)
        .into_iter()
        .filter_map(|voice| voice_path(folder, &voice.id).map(|path| (voice.id, path)))
        .collect()
}

pub fn status(folder: Option<&Path>) -> SpeechStatus {
    let installed = folder.is_some_and(present);
    SpeechStatus {
        installed,
        supported: crate::dictation::cpu_supported(),
        name: "Phonon 31M".into(),
        voices: match folder {
            Some(folder) if installed => voices(folder),
            _ => Vec::new(),
        },
    }
}

/// The engine for the bundled model in `folder`, loaded the first time it is asked for.
pub(super) fn engine(folder: &Path) -> Result<Arc<Engine>, Failure> {
    if !present(folder) {
        return Err(Failure::model(
            "This build of o7 is missing its voice model. Reinstall o7 to restore it.",
        ));
    }
    Engine::shared(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(folder: &Path) {
        for model in BUNDLED {
            let file = std::fs::File::create(folder.join(model.file)).unwrap();
            file.set_len(model.bytes).unwrap();
        }
    }

    #[test]
    fn the_bundled_files_match_the_release_fetch_script() {
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
        // Every voice the committed catalogue names is one the script fetches, at the one size.
        let catalogue: Vec<SpeechVoice> = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/speech/voices.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(catalogue.len() >= 20, "{}", catalogue.len());
        for voice in &catalogue {
            assert!(voice_id(&voice.id), "{}", voice.id);
            assert!(
                script.contains(&format!("file: \"voices/{}.safetensors\"", voice.id)),
                "{}",
                voice.name
            );
            assert!(!voice.name.is_empty() && !voice.description.is_empty());
            assert!(
                ["en-us", "en-gb", "characters"].contains(&voice.locale.as_str()),
                "{}",
                voice.locale
            );
        }
        assert!(script.contains(&format!("size: {VOICE_BYTES},")));
    }

    #[test]
    fn a_folder_without_the_pinned_files_is_not_ready() {
        let folder = tempfile::tempdir().unwrap();
        assert!(!present(folder.path()));
        assert!(!status(Some(folder.path())).installed);
        assert!(status(Some(folder.path())).voices.is_empty());
        assert!(!status(None).installed);
        for model in BUNDLED {
            std::fs::write(folder.path().join(model.file), b"short").unwrap();
        }
        assert!(!present(folder.path()), "sizes must match");
        assert_eq!(
            engine(folder.path()).err().map(|failure| failure.reason),
            Some(super::super::SpeechFailure::Model)
        );
    }

    #[test]
    fn only_catalogued_voices_whose_files_are_here_are_offered() {
        let folder = tempfile::tempdir().unwrap();
        fill(folder.path());
        // With no catalogue there is still the model's own voice.
        let own = voices(folder.path());
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].id, OWN_VOICE_ID);
        std::fs::create_dir(folder.path().join(VOICES)).unwrap();
        let voice = |id: &str, name: &str| SpeechVoice {
            id: id.into(),
            name: name.into(),
            locale: "en-gb".into(),
            gender: None,
            description: "A voice.".into(),
        };
        let catalogue = vec![
            voice("NbpkqMVS3CJeq2j8", "Here"),
            voice("NIWIOv5vTi5eRMj8", "Missing"),
            voice("LMDIYE0Oa377Qh6Z", "Wrong size"),
            // Not ids: a catalogue cannot point outside the voices folder.
            voice("../../secret", "Escapes"),
            voice("default", "Shadows the model's own"),
            voice("NbpkqMVS3CJeq2j8", "Listed twice"),
        ];
        std::fs::write(
            folder.path().join(CATALOG),
            serde_json::to_vec(&catalogue).unwrap(),
        )
        .unwrap();
        let sized_file = |name: &str, bytes: u64| {
            let file = std::fs::File::create(folder.path().join(VOICES).join(name)).unwrap();
            file.set_len(bytes).unwrap();
        };
        sized_file("NbpkqMVS3CJeq2j8.safetensors", VOICE_BYTES);
        sized_file("LMDIYE0Oa377Qh6Z.safetensors", VOICE_BYTES - 1);
        let offered = voices(folder.path());
        assert_eq!(
            offered
                .iter()
                .map(|voice| voice.name.as_str())
                .collect::<Vec<_>>(),
            ["Reuben", "Here"]
        );
        assert_eq!(
            voice_files(folder.path()).len(),
            1,
            "the model's own is not a file here"
        );
        assert!(voice_path(folder.path(), "../../secret").is_none());
        assert!(voice_path(folder.path(), "a/b").is_none());
        assert!(!voice_id("short"));
        assert!(!voice_id(&"x".repeat(33)));
        // A catalogue that cannot be read costs the extra voices, not reading aloud.
        std::fs::write(folder.path().join(CATALOG), b"not json").unwrap();
        assert_eq!(voices(folder.path()).len(), 1);
        assert!(status(Some(folder.path())).installed);
    }
}
