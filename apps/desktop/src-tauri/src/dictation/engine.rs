//! The speech engine: transcribe.cpp running the bundled Parakeet Ultra (speech to text with
//! punctuation and capitals) and Silero VAD v6.2. Both load once, memory-mapped, and stay loaded
//! for the app's life, so a recording starts at once; each recording gets its own sessions.
use super::Failure;
use super::live::{Decoder, SAMPLE_RATE};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use transcribe_cpp::{CancelToken, Model, RunOptions, Session, SessionOptions, VadSession};

/// Passes run on a few threads, never all of them: fewer keep the processor cool while a person
/// talks (owner, 4 November 2026: it "burned up" the machine before). The large model needs four
/// to feel live on a big machine (measured on a 16-thread desktop: a pass over 18 s of audio takes
/// 2.1 s on two threads and 1.3 s on four), and `live.rs` paces the passes to what they cost, so
/// more threads shorten the wait for words without raising the load much.
fn threads_for(logical: usize) -> usize {
    match logical {
        0..=2 => 1,
        3..=11 => 2,
        _ => 4,
    }
}

pub struct Engine {
    asr: Model,
    vad: Model,
    frame: usize,
}

/// The engine loaded from one folder; loading again from another replaces it.
static LOADED: Mutex<Option<(PathBuf, Arc<Engine>)>> = Mutex::new(None);

fn threads() -> i32 {
    // The opt-in bench (`bundled_models_transcribe_fixture_audio_live`) tries other counts.
    #[cfg(test)]
    if let Some(count) = std::env::var("O7_DICTATION_THREADS")
        .ok()
        .and_then(|count| count.parse().ok())
    {
        return count;
    }
    threads_for(std::thread::available_parallelism().map_or(4, usize::from)) as i32
}

impl Engine {
    /// The engine for the models in `folder`, loading them the first time.
    pub fn shared(folder: &Path, asr: &str, vad: &str) -> Result<Arc<Self>, Failure> {
        let mut loaded = crate::sync::lock(&LOADED);
        if let Some((path, engine)) = loaded.as_ref()
            && path == folder
        {
            return Ok(engine.clone());
        }
        let engine = Arc::new(Self::load(&folder.join(asr), &folder.join(vad))?);
        *loaded = Some((folder.to_path_buf(), engine.clone()));
        Ok(engine)
    }

    fn load(asr: &Path, vad: &Path) -> Result<Self, Failure> {
        // No native or ggml message reaches stderr, so no transcript text can either. The library
        // takes this once, before its first model.
        static QUIET: std::sync::Once = std::sync::Once::new();
        QUIET.call_once(transcribe_cpp::disable_logging);
        let unreadable = |_| {
            Failure::model("The dictation model could not be loaded. Reinstall o7 to repair it.")
        };
        let asr = Model::load(asr).map_err(unreadable)?;
        let vad = Model::load(vad).map_err(unreadable)?;
        let info = vad.vad_info().map_err(unreadable)?;
        if info.sample_rate != SAMPLE_RATE as i32 || info.frame_samples <= 0 {
            return Err(Failure::model(
                "The voice detection model is not the one this build expects. Reinstall o7.",
            ));
        }
        Ok(Self {
            asr,
            vad,
            frame: info.frame_samples as usize,
        })
    }

    /// Samples per voice-activity frame (512, 32 ms).
    pub fn frame_samples(&self) -> usize {
        self.frame
    }

    pub fn vad(&self) -> Result<VadSession, Failure> {
        self.vad
            .vad_session()
            .map_err(|_| Failure::other("Voice detection could not start. Try again."))
    }

    /// A decoder for one recording, aborted through `cancel`.
    pub fn decoder(&self, cancel: &CancelToken) -> Result<Parakeet, Failure> {
        let mut session = self
            .asr
            .session_with(&SessionOptions {
                n_threads: threads(),
                ..SessionOptions::default()
            })
            .map_err(|_| {
                Failure::other(
                    "Not enough memory to transcribe. Close other applications and retry.",
                )
            })?;
        session.set_cancel_token(cancel);
        Ok(Parakeet { session })
    }
}

pub struct Parakeet {
    session: Session,
}

impl Decoder for Parakeet {
    fn decode(&mut self, pcm: &[f32]) -> Result<String, Failure> {
        self.session
            .run(pcm, &RunOptions::default())
            .map(|transcript| transcript.text.trim().to_owned())
            .map_err(|_| {
                Failure::other("Dictation could not transcribe this recording. Try again.")
            })
    }
}
