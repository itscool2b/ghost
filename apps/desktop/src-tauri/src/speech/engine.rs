//! The speech engine: Phonon 31M through Gradium's own Rust runtime (`ptts`, MIT or Apache 2.0),
//! on the CPU. The model loads once, in about a tenth of a second, and stays loaded; each voice
//! is made ready the first time it speaks (most of a second), after which speech starts in a
//! few hundredths.
use super::Failure;
use super::model::{self, OWN_VOICE_ID, WEIGHTS};
use crate::sync::lock;
use ptts::checkpoint::{Checkpoint, ResolveOptions};
use ptts::preprocess::Lang;
use ptts::synth::{DeviceKind, Quant, SpeechOptions, SpeechStream, Synth};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static LOADED: Mutex<Option<(PathBuf, Arc<Engine>)>> = Mutex::new(None);

pub struct Engine {
    synth: Synth,
    /// The voices registered with the model, by the id a request names.
    voices: Vec<String>,
}

/// How many threads the engine may use. Measured on an 8-core desktop: 3.5 times faster than
/// speech on one, 6 on two, 9 on four, and under 3 when left to take all sixteen, where the
/// threads get in each other's way. Two is plenty and leaves the machine to the agents.
fn threads() -> usize {
    match std::thread::available_parallelism().map_or(2, usize::from) {
        0..=2 => 1,
        3..=11 => 2,
        _ => 4,
    }
}

impl Engine {
    /// The engine for the model in `folder`, loading it the first time.
    pub fn shared(folder: &Path) -> Result<Arc<Self>, Failure> {
        let mut loaded = lock(&LOADED);
        if let Some((path, engine)) = loaded.as_ref()
            && path == folder
        {
            return Ok(engine.clone());
        }
        let engine = Arc::new(Self::load(folder)?);
        *loaded = Some((folder.to_path_buf(), engine.clone()));
        Ok(engine)
    }

    fn load(folder: &Path) -> Result<Self, Failure> {
        // The runtime sizes its pool the first time it works, so this comes before any model.
        static THREADS: std::sync::Once = std::sync::Once::new();
        THREADS.call_once(|| xn::set_num_threads(threads()));
        let unreadable =
            |_| Failure::model("The voice model could not be loaded. Reinstall o7 to repair it.");
        let checkpoint = Checkpoint::resolve(
            folder,
            ResolveOptions {
                quant: Quant::Q80,
                weights: Some(WEIGHTS.file),
            },
        )
        .map_err(unreadable)?;
        let mut synth = checkpoint
            .builder(Lang::En)
            .device(DeviceKind::Cpu)
            .build()
            .map_err(unreadable)?;
        // The model's own voice, as `default`, and every voice file in the folder beside it,
        // each under its file's name: the catalogue's id.
        checkpoint.register_voices(&mut synth);
        // A catalogued voice the runtime did not pick up is added by its id; one that will not
        // load is left out, and the rest still speak.
        for (id, path) in model::voice_files(folder) {
            if !synth.voices().contains(&id) {
                let _ = synth.add_voice_file(&id, &path);
            }
        }
        let voices = synth.voices();
        if !voices.iter().any(|voice| voice == OWN_VOICE_ID) {
            return Err(Failure::model(
                "The voice model is missing its voice. Reinstall o7 to repair it.",
            ));
        }
        Ok(Self { synth, voices })
    }

    /// The voice a request gets: the one it names when the model has it, else the model's own.
    pub fn voice<'a>(&'a self, wanted: Option<&'a str>) -> &'a str {
        wanted
            .filter(|wanted| self.voices.iter().any(|voice| voice == wanted))
            .unwrap_or(OWN_VOICE_ID)
    }

    /// The sample rate of what [`Self::stream`] yields.
    pub fn sample_rate(&self) -> usize {
        self.synth.sample_rate() as usize
    }

    /// Starts speaking `text`: mono samples arrive a fraction of a second at a time, as they are
    /// made. Dropping the stream stops the work.
    pub fn stream(&self, text: &str, voice: &str) -> Result<SpeechStream, Failure> {
        self.synth
            .stream_with(text, &SpeechOptions::default().voice(voice))
            .map_err(|_| Failure::other("That could not be read aloud. Try again."))
    }

    /// Makes `voice` ready, so the first thing it is asked to say starts at once. The sound of
    /// the few words it takes is discarded.
    pub fn warm(&self, voice: &str) {
        if let Ok(stream) = self.stream("Ready.", voice) {
            for chunk in stream {
                if chunk.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_takes_a_few_threads_and_never_all_of_a_large_machine() {
        let threads = threads();
        assert!((1..=4).contains(&threads), "{threads}");
    }

    /// With the bundled model fetched (`node scripts/dictation-models.mjs`), the engine loads,
    /// offers its voices and speaks: real samples, several times faster than they play.
    /// `cargo test -p o7-desktop --lib speech -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the bundled voice model in resources/speech"]
    fn the_bundled_model_speaks_faster_than_real_time() {
        // `O7_SPEECH_MODELS` names another folder holding the fetched files.
        let folder = std::env::var_os("O7_SPEECH_MODELS").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/speech"),
            PathBuf::from,
        );
        assert!(
            model::present(&folder),
            "run node scripts/dictation-models.mjs"
        );
        let engine = Engine::shared(&folder).unwrap();
        assert_eq!(engine.voice(None), OWN_VOICE_ID);
        assert_eq!(engine.voice(Some("nobody")), OWN_VOICE_ID);
        let offered = model::voices(&folder);
        println!(
            "{} voices offered, {} registered",
            offered.len(),
            engine.voices.len()
        );
        assert!(offered.len() >= 30, "the catalogue's voices are here");
        for voice in &offered {
            assert_eq!(
                engine.voice(Some(&voice.id)),
                voice.id,
                "{} loads",
                voice.name
            );
        }
        for voice in [OWN_VOICE_ID, offered.last().unwrap().id.as_str()] {
            let voice = engine.voice(Some(voice));
            engine.warm(voice);
            let started = std::time::Instant::now();
            let mut first = None;
            let mut samples = 0usize;
            let mut energy = 0f64;
            for chunk in engine
                .stream("All 42 tests pass, and nothing else changed.", voice)
                .unwrap()
            {
                let chunk = chunk.unwrap();
                first.get_or_insert_with(|| started.elapsed());
                samples += chunk.len();
                energy += chunk
                    .iter()
                    .map(|s| f64::from(*s) * f64::from(*s))
                    .sum::<f64>();
            }
            let seconds = samples as f32 / engine.sample_rate() as f32;
            let took = started.elapsed().as_secs_f32();
            let rms = (energy / samples.max(1) as f64).sqrt();
            println!(
                "{voice}: first audio {:.0} ms, {seconds:.2} s in {took:.2} s ({:.1}x), rms {rms:.3}",
                first.unwrap().as_secs_f32() * 1000.0,
                seconds / took
            );
            assert!(seconds > 1.5, "a sentence of speech");
            assert!(rms > 0.01, "not silence");
            assert!(seconds / took > 1.5, "faster than it plays");
        }
    }
}
