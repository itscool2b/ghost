//! Speaking: the engine's samples go to the default output device as they are made. Nothing
//! is kept: the queue holds at most a couple of seconds ahead of the speaker, and it is dropped
//! when the request ends.
use super::engine::Engine;
use super::{Failure, Operation, SpeechEvent};
use crate::sync::lock;
use cpal::Sample as _;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{FftFixedInOut, Resampler};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How far ahead of the speaker the engine may run, in seconds of sound. It makes speech several
/// times faster than it plays; without a limit a long answer would sit in memory whole.
const AHEAD_SECONDS: usize = 2;
const POLL: Duration = Duration::from_millis(20);
/// The device holds a buffer of its own after the queue empties: let it play out.
const TAIL: Duration = Duration::from_millis(180);
const CONVERT_ERROR: &str = "The sound could not be prepared for this output device. Try again.";
const STOPPED_ERROR: &str =
    "The output device stopped. Check your speakers or headphones, then try again.";

/// A failure to open or start the output, by what the person can do about it.
fn open_failure(kind: cpal::ErrorKind) -> Failure {
    use cpal::ErrorKind as K;
    Failure::output(match kind {
        K::DeviceBusy => {
            "Another program has the output device to itself. Close it, then try again."
        }
        K::UnsupportedConfig => {
            "This output device uses a format o7 cannot play to. Choose another in your system's sound settings."
        }
        _ => {
            "No speaker or headphones could be opened. Check your system's sound output, then try again."
        }
    })
}

/// Whether a stream error ends the speech. cpal reports glitches and reroutes through the same
/// callback as failures: an underrun drops a moment of sound, a reroute follows the new default
/// device, and a refused real-time priority only adds latency (see dictation's capture).
fn fatal(kind: cpal::ErrorKind) -> bool {
    use cpal::ErrorKind as K;
    !matches!(kind, K::Xrun | K::DeviceChanged | K::RealtimeDenied)
}

fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    queue: Arc<Mutex<VecDeque<f32>>>,
    fault: Arc<AtomicBool>,
    silenced: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            // A stopped request is silent from this buffer on: what is still queued is not
            // played, and a request that replaced it is never heard over it.
            if silenced.load(Ordering::Acquire) {
                out.fill(0.0f32.to_sample::<T>());
                return;
            }
            let mut queue = lock(&queue);
            // One voice on every channel; silence once the queue runs dry.
            for frame in out.chunks_mut(channels) {
                let sample = queue.pop_front().unwrap_or(0.0).to_sample::<T>();
                frame.fill(sample);
            }
        },
        move |error: cpal::Error| {
            if fatal(error.kind()) {
                fault.store(true, Ordering::Release);
            }
        },
        Some(Duration::from_secs(5)),
    )
}

/// The engine's rate to the device's, a block at a time.
struct Converter {
    resampler: Option<FftFixedInOut<f32>>,
    pending: Vec<f32>,
    /// Samples of the resampler's own delay still to drop from the start.
    delay: usize,
}
impl Converter {
    fn new(from: usize, to: usize) -> Result<Self, Failure> {
        let resampler = if from == to {
            None
        } else {
            Some(
                FftFixedInOut::<f32>::new(from, to, 1024, 1)
                    .map_err(|_| Failure::other(CONVERT_ERROR))?,
            )
        };
        let delay = resampler
            .as_ref()
            .map_or(0, |resampler| resampler.output_delay());
        Ok(Self {
            resampler,
            pending: Vec::new(),
            delay,
        })
    }

    /// Converts what has arrived, keeping back less than one block.
    fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, Failure> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(samples.to_vec());
        };
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        loop {
            let block = resampler.input_frames_next();
            if self.pending.len() < block {
                break;
            }
            let converted = resampler
                .process(&[&self.pending[..block]], None)
                .map_err(|_| Failure::other(CONVERT_ERROR))?;
            self.pending.drain(..block);
            if let Some(channel) = converted.first() {
                let skip = self.delay.min(channel.len());
                self.delay -= skip;
                out.extend_from_slice(&channel[skip..]);
            }
        }
        Ok(out)
    }

    /// The rest: the last partial block, and the delay's worth the resampler still holds.
    fn finish(&mut self) -> Result<Vec<f32>, Failure> {
        let Some(resampler) = self.resampler.as_ref() else {
            return Ok(Vec::new());
        };
        let block = resampler.input_frames_next();
        let held = resampler.output_delay();
        // Silence pushes the last real samples through.
        let padding = block - self.pending.len() % block + block * held.div_ceil(block.max(1));
        let silence = vec![0.0; padding];
        self.push(&silence)
    }
}

/// Speaks `text` in `voice` until it is all said or the request is stopped. Returns once the
/// sound has played out.
pub(super) fn speak(
    engine: &Engine,
    text: &str,
    voice: Option<&str>,
    operation: &Operation,
) -> Result<(), Failure> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| {
        Failure::output("No speaker or headphones were found. Connect one and try again.")
    })?;
    let supported = device
        .default_output_config()
        .map_err(|error| open_failure(error.kind()))?;
    let config = supported.config();
    let rate = config.sample_rate as usize;
    if rate == 0 || config.channels == 0 {
        return Err(open_failure(cpal::ErrorKind::UnsupportedConfig));
    }
    let queue = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(rate)));
    let fault = Arc::new(AtomicBool::new(false));
    let silenced = operation.silenced();
    macro_rules! stream {
        ($sample:ty) => {
            output_stream::<$sample>(
                &device,
                config.clone(),
                queue.clone(),
                fault.clone(),
                silenced.clone(),
            )
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::F64 => stream!(f64),
        cpal::SampleFormat::I8 => stream!(i8),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::I64 => stream!(i64),
        cpal::SampleFormat::U8 => stream!(u8),
        cpal::SampleFormat::U16 => stream!(u16),
        cpal::SampleFormat::U32 => stream!(u32),
        cpal::SampleFormat::U64 => stream!(u64),
        _ => return Err(open_failure(cpal::ErrorKind::UnsupportedConfig)),
    }
    .map_err(|error| open_failure(error.kind()))?;
    stream.play().map_err(|error| open_failure(error.kind()))?;

    let mut convert = Converter::new(engine.sample_rate(), rate)?;
    let ahead = rate * AHEAD_SECONDS;
    let healthy = |fault: &AtomicBool| {
        if fault.load(Ordering::Acquire) {
            Err(Failure::output(STOPPED_ERROR))
        } else {
            Ok(())
        }
    };
    let mut speaking = false;
    for chunk in engine.stream(text, engine.voice(voice))? {
        if operation.stopped() {
            return Ok(());
        }
        let chunk =
            chunk.map_err(|_| Failure::other("The voice stopped unexpectedly. Try again."))?;
        let converted = convert.push(&chunk)?;
        if !converted.is_empty() {
            lock(&queue).extend(converted.iter().map(|sample| {
                if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }));
            if !speaking {
                speaking = true;
                if !operation.emit(SpeechEvent::Speaking) {
                    return Ok(());
                }
            }
        }
        // The engine waits here while the speaker catches up.
        while lock(&queue).len() > ahead {
            if operation.stopped() {
                return Ok(());
            }
            healthy(&fault)?;
            std::thread::sleep(POLL);
        }
        healthy(&fault)?;
    }
    let rest = convert.finish()?;
    lock(&queue).extend(rest);
    while !lock(&queue).is_empty() {
        if operation.stopped() {
            return Ok(());
        }
        healthy(&fault)?;
        std::thread::sleep(POLL);
    }
    std::thread::sleep(TAIL);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glitches_and_reroutes_do_not_end_the_speech() {
        use cpal::ErrorKind as K;
        for kind in [K::Xrun, K::DeviceChanged, K::RealtimeDenied] {
            assert!(!fatal(kind), "{kind:?}");
        }
        assert!(fatal(K::DeviceNotAvailable));
    }

    #[test]
    fn the_same_rate_passes_samples_through_untouched() {
        let mut convert = Converter::new(24_000, 24_000).unwrap();
        assert_eq!(convert.push(&[0.1, -0.2, 0.3]).unwrap(), [0.1, -0.2, 0.3]);
        assert!(convert.finish().unwrap().is_empty());
    }

    /// A tone resampled to a device's rate comes out as long as it went in, at the same pitch
    /// and level, with nothing lost at either end.
    #[test]
    fn a_resampled_tone_keeps_its_length_pitch_and_level() {
        for device in [48_000usize, 44_100, 16_000, 96_000] {
            let from = 24_000usize;
            let tone: Vec<f32> = (0..from * 2)
                .map(|at| (at as f32 * 440.0 * std::f32::consts::TAU / from as f32).sin() * 0.5)
                .collect();
            let mut convert = Converter::new(from, device).unwrap();
            let mut out = Vec::new();
            // In uneven pieces, as the engine delivers them.
            for piece in tone.chunks(1_920 + 37) {
                out.extend(convert.push(piece).unwrap());
            }
            out.extend(convert.finish().unwrap());
            let expected = tone.len() * device / from;
            assert!(
                out.len() >= expected && out.len() <= expected + device / 4,
                "{device}: {} samples for {expected}",
                out.len()
            );
            let body = &out[device / 10..expected - device / 10];
            let rms = (body.iter().map(|s| s * s).sum::<f32>() / body.len() as f32).sqrt();
            assert!(
                (rms - 0.5 / 2f32.sqrt()).abs() < 0.02,
                "{device}: rms {rms}"
            );
            let crossings = body
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count() as f32;
            let hertz = crossings / (body.len() as f32 / device as f32);
            assert!((hertz - 440.0).abs() < 4.0, "{device}: {hertz} Hz");
            // The sound starts at once: the resampler's own delay was dropped.
            let onset = out.iter().position(|sample| sample.abs() > 0.05).unwrap();
            assert!(onset < device / 100, "{device}: starts at sample {onset}");
        }
    }

    /// Opt-in, with the bundled voice model: one sentence in three voices, through the converter
    /// the speakers get, written to `O7_SPEECH_WAV_DIR` as 16 kHz WAVs. Dictation's own model
    /// then says what it heard
    /// (`O7_DICTATION_FIXTURE_DIR=<the same folder> cargo test -p o7-desktop --lib
    /// bundled_models_transcribe -- --ignored --nocapture`): the nearest thing to a listener a
    /// test has.
    #[test]
    #[ignore = "needs the bundled voice model and O7_SPEECH_WAV_DIR"]
    fn the_voices_speak_into_fixture_audio_for_dictation_to_hear() {
        use std::path::{Path, PathBuf};
        const HEARD_AT: usize = 16_000;
        let out = PathBuf::from(std::env::var_os("O7_SPEECH_WAV_DIR").expect("a folder for WAVs"));
        let folder = std::env::var_os("O7_SPEECH_MODELS").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/speech"),
            PathBuf::from,
        );
        let engine = Engine::shared(&folder).unwrap();
        let offered = super::super::model::voices(&folder);
        let pick = |locale: &str| {
            offered
                .iter()
                .find(|voice| voice.locale == locale && voice.id != "default")
                .expect("a voice of each kind")
        };
        let said = "I fixed the login flow, and all 42 tests pass. Nothing else changed.";
        for voice in [
            &offered[0],
            pick("en-us"),
            pick("en-gb"),
            pick("characters"),
        ] {
            let mut convert = Converter::new(engine.sample_rate(), HEARD_AT).unwrap();
            let mut pcm = Vec::new();
            for chunk in engine.stream(said, engine.voice(Some(&voice.id))).unwrap() {
                pcm.extend(convert.push(&chunk.unwrap()).unwrap());
            }
            pcm.extend(convert.finish().unwrap());
            let data: Vec<u8> = pcm
                .iter()
                .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                .collect();
            let mut wav = Vec::with_capacity(44 + data.len());
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&(HEARD_AT as u32).to_le_bytes());
            wav.extend_from_slice(&(HEARD_AT as u32 * 2).to_le_bytes());
            wav.extend_from_slice(&2u16.to_le_bytes());
            wav.extend_from_slice(&16u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
            wav.extend_from_slice(&data);
            let name = format!("{}-{}.wav", voice.locale, voice.name.replace(' ', "-"));
            std::fs::write(out.join(&name), wav).unwrap();
            println!("{name}: {:.2} s", pcm.len() as f32 / HEARD_AT as f32);
            assert!(pcm.len() > HEARD_AT * 2, "a sentence of speech");
        }
    }
}
