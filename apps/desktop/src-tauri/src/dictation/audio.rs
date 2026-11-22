//! Recording with live text: the microphone streams into a 16 kHz converter, voice activity
//! detection and [`Live`], which turns phrases into text while the person talks. There is no time
//! limit; only the phrase in progress is held, so memory stays bounded (docs/local-dictation.md).
use super::engine::Engine;
use super::live::{Live, LiveConfig, LiveText, SAMPLE_RATE};
use super::{DictationEvent, Failure, Operation};
use crate::sync::lock;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{FftFixedInOut, Resampler};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transcribe_cpp::{CancelToken, VadSession};
use zeroize::Zeroize;

/// How often the loop takes the microphone's audio and reports the level and text.
const TICK: Duration = Duration::from_millis(80);
/// Audio the capture callback holds before the loop takes it; past this it is dropped, so a
/// stalled loop cannot grow memory.
const BACKLOG_SECONDS: usize = 12;
/// A stream that delivered audio and then nothing for this long has stopped.
const STALL: Duration = Duration::from_secs(3);
/// A stream that delivers no audio at all within this long cannot be used.
const FIRST_AUDIO: Duration = Duration::from_secs(5);
const MICROPHONE_ERROR: &str = "The microphone could not be opened. Check that one is connected and set as the default input device in your system sound settings.";
const PRIVACY_ERROR: &str = "Microphone access is turned off for desktop apps. Allow it in your system privacy settings, then try again.";
const BUSY_ERROR: &str = "Another app is using the microphone exclusively. Close it, or turn off exclusive mode for the device, then try again.";
const STOPPED_ERROR: &str =
    "The microphone disconnected or stopped responding. Reconnect it and try again.";

/// The processor features this build's speech engine was compiled for. ggml-cpu alone uses them,
/// and only once dictation runs: nothing compiled for them runs at startup (see
/// docs/local-dictation.md), so checking here keeps older processors safe.
pub fn cpu_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx")
            && std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// What a stream error means for the recording. cpal reports glitches and reroutes through the
/// same callback as failures: on Windows an early capture packet routinely carries WASAPI's data
/// discontinuity flag, which cpal 0.18 reports as an `Xrun`. Treating that as fatal ended every
/// recording a tenth of a second after it began.
fn stream_fault(kind: cpal::ErrorKind) -> u8 {
    use cpal::ErrorKind as K;
    match kind {
        // The stream keeps running: an overrun drops a few samples, a reroute follows the new
        // default device, and a refused real-time priority only adds latency.
        K::Xrun | K::DeviceChanged | K::RealtimeDenied => FAULT_NONE,
        K::PermissionDenied => FAULT_PERMISSION,
        // cpal ends the capture thread after any other error.
        _ => FAULT_STOPPED,
    }
}
const FAULT_NONE: u8 = 0;
const FAULT_STOPPED: u8 = 1;
const FAULT_PERMISSION: u8 = 2;

/// A failure to open or start the stream, by what the person can do about it.
fn open_failure(kind: cpal::ErrorKind) -> Failure {
    use cpal::ErrorKind as K;
    Failure::microphone(match kind {
        K::PermissionDenied => PRIVACY_ERROR,
        K::DeviceBusy => BUSY_ERROR,
        K::DeviceNotAvailable => "No microphone was found. Connect one and try again.",
        K::UnsupportedConfig => {
            "This microphone's format is unsupported. Select another input device in your system sound settings."
        }
        _ => MICROPHONE_ERROR,
    })
}

/// What the capture callback has delivered since the loop last took it.
struct Capture {
    samples: Vec<f32>,
    limit: usize,
    /// Every sample delivered so far, taken or dropped: what the watchdog follows.
    delivered: usize,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.samples.zeroize();
    }
}
impl Capture {
    fn push<T: cpal::Sample>(&mut self, data: &[T], channels: usize)
    where
        f32: cpal::FromSample<T>,
    {
        for frame in data.chunks_exact(channels) {
            self.delivered += 1;
            if self.samples.len() >= self.limit {
                continue;
            }
            let sample = frame
                .iter()
                .map(|sample| sample.to_sample::<f32>())
                .sum::<f32>()
                / channels as f32;
            self.samples.push(if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            });
        }
    }
}

/// The root-mean-square level of `samples`, 0 to 1.
fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The default microphone, open and delivering into a [`Capture`] until dropped.
struct Microphone {
    stream: cpal::Stream,
    capture: Arc<Mutex<Capture>>,
    fault: Arc<AtomicU8>,
    rate: usize,
}

impl Microphone {
    fn open() -> Result<Self, Failure> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| {
            Failure::microphone("No microphone was found. Connect one and try again.")
        })?;
        let supported = device
            .default_input_config()
            .map_err(|error| open_failure(error.kind()))?;
        let config = supported.config();
        let rate = config.sample_rate as usize;
        if !(8_000..=192_000).contains(&rate) || !(1..=8).contains(&config.channels) {
            return Err(open_failure(cpal::ErrorKind::UnsupportedConfig));
        }
        let capture = Arc::new(Mutex::new(Capture {
            samples: Vec::with_capacity(rate),
            limit: rate * BACKLOG_SECONDS,
            delivered: 0,
        }));
        let fault = Arc::new(AtomicU8::new(FAULT_NONE));
        macro_rules! stream {
            ($sample:ty) => {
                input_stream::<$sample>(&device, config, capture.clone(), fault.clone())
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
        Ok(Self {
            stream,
            capture,
            fault,
            rate,
        })
    }

    /// The audio delivered since the last call, and how much has been delivered in all.
    fn take(&self) -> (Vec<f32>, usize) {
        let mut capture = lock(&self.capture);
        (std::mem::take(&mut capture.samples), capture.delivered)
    }

    fn check(&self) -> Result<(), Failure> {
        match self.fault.load(Ordering::Acquire) {
            FAULT_NONE => Ok(()),
            FAULT_PERMISSION => Err(Failure::microphone(PRIVACY_ERROR)),
            _ => Err(Failure::microphone(STOPPED_ERROR)),
        }
    }

    /// Closes the native stream, releasing the microphone.
    fn close(self) {
        drop(self.stream);
    }
}

fn input_stream<T: cpal::SizedSample>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    capture: Arc<Mutex<Capture>>,
    fault: Arc<AtomicU8>,
) -> Result<cpal::Stream, cpal::Error>
where
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    device.build_input_stream(
        config,
        move |samples: &[T], _| lock(&capture).push(samples, channels),
        move |error: cpal::Error| {
            let code = stream_fault(error.kind());
            if code != FAULT_NONE {
                // Keep the first failure: the capture thread ends after it.
                let _ =
                    fault.compare_exchange(FAULT_NONE, code, Ordering::AcqRel, Ordering::Acquire);
            }
        },
        Some(Duration::from_secs(5)),
    )
}

/// Notices a stream that never starts delivering audio, or stops partway: some drivers end a
/// stream without reporting an error.
struct Watchdog {
    began: Instant,
    last_count: usize,
    last_change: Instant,
}
impl Watchdog {
    fn new(began: Instant) -> Self {
        Self {
            began,
            last_count: 0,
            last_change: began,
        }
    }
    fn check(&mut self, count: usize, now: Instant) -> Option<&'static str> {
        if count != self.last_count {
            self.last_count = count;
            self.last_change = now;
            return None;
        }
        if count == 0 {
            (now.duration_since(self.began) > FIRST_AUDIO).then_some(MICROPHONE_ERROR)
        } else {
            (now.duration_since(self.last_change) > STALL).then_some(STOPPED_ERROR)
        }
    }
}

/// Anti-aliased, streaming conversion to the models' mono 16 kHz input. The FFT's delay is
/// dropped from the start and flushed at the end, so the first and last words survive.
struct Converter {
    resampler: Option<FftFixedInOut<f32>>,
    pending: Vec<f32>,
    /// Output samples still to drop: the resampler's delay.
    delay: usize,
    taken: usize,
    produced: usize,
    rate: usize,
}

const CONVERT_ERROR: &str = "The microphone audio could not be converted.";

/// Appends converted audio, dropping what remains of the resampler's delay first.
fn emit(delay: &mut usize, converted: Vec<Vec<f32>>, out: &mut Vec<f32>) {
    for mut channel in converted {
        let skip = (*delay).min(channel.len());
        *delay -= skip;
        out.extend_from_slice(&channel[skip..]);
        channel.zeroize();
    }
}

impl Converter {
    fn new(rate: usize) -> Result<Self, Failure> {
        let resampler = if rate == SAMPLE_RATE {
            None
        } else {
            Some(
                FftFixedInOut::<f32>::new(rate, SAMPLE_RATE, 1024, 1)
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
            taken: 0,
            produced: 0,
            rate,
        })
    }

    /// Converts `input`, returning every whole output sample it completes.
    fn push(&mut self, input: &[f32]) -> Result<Vec<f32>, Failure> {
        self.taken += input.len();
        let Some(resampler) = self.resampler.as_mut() else {
            self.produced += input.len();
            return Ok(input.to_vec());
        };
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        loop {
            let chunk = resampler.input_frames_next();
            if self.pending.len() < chunk {
                break;
            }
            let converted = resampler
                .process(&[&self.pending[..chunk]], None)
                .map_err(|_| Failure::other(CONVERT_ERROR))?;
            self.pending.drain(..chunk);
            emit(&mut self.delay, converted, &mut out);
        }
        self.produced += out.len();
        Ok(out)
    }

    /// The rest of the audio once the microphone has closed, trimmed to the input's length.
    fn finish(&mut self) -> Result<Vec<f32>, Failure> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let total = self.taken * SAMPLE_RATE / self.rate;
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.pending);
        let converted = resampler
            .process_partial(Some(&[&rest[..]]), None)
            .map_err(|_| Failure::other(CONVERT_ERROR))?;
        emit(&mut self.delay, converted, &mut out);
        // Flush the FFT's delay until the output covers the whole input.
        while self.produced + out.len() < total
            && let Some(resampler) = self.resampler.as_mut()
        {
            let converted = resampler
                .process_partial::<&[f32]>(None, None)
                .map_err(|_| Failure::other(CONVERT_ERROR))?;
            let before = out.len();
            emit(&mut self.delay, converted, &mut out);
            if out.len() == before {
                break;
            }
        }
        out.truncate(total.saturating_sub(self.produced));
        self.produced += out.len();
        Ok(out)
    }
}

/// Scores `samples` for voice activity and feeds every completed frame to `live`; `pending`
/// holds the samples of the frame still open.
fn score<D: super::live::Decoder>(
    vad: &mut VadSession,
    pending: &mut Vec<f32>,
    samples: &[f32],
    frame: usize,
    live: &mut Live<D>,
    text: &mut LiveText,
) -> Result<(), Failure> {
    if samples.is_empty() {
        return Ok(());
    }
    pending.extend_from_slice(samples);
    let scored = vad
        .stream_feed(samples)
        .map_err(|_| Failure::other("Voice detection stopped. Try again."))?;
    for probability in scored.probs {
        if pending.len() < frame {
            break;
        }
        let block: Vec<f32> = pending.drain(..frame).collect();
        if let Some(next) = live.feed(&block, probability)? {
            *text = next;
        }
    }
    Ok(())
}

/// Records from the default microphone until the person finishes or cancels, reporting the level
/// and the live text as it goes, and returns the finished text.
pub fn run(engine: &Engine, operation: &Operation) -> Result<DictationEvent, Failure> {
    let cancel = CancelToken::new();
    let decoder = engine.decoder(&cancel)?;
    let mut vad = engine.vad()?;
    let frame = engine.frame_samples();
    let mut live = Live::new(LiveConfig::default(), decoder);
    let microphone = Microphone::open()?;
    let mut converter = Converter::new(microphone.rate)?;
    let mut pending = Vec::new();
    let mut text = LiveText::default();
    let began = Instant::now();
    let mut watch = Watchdog::new(began);
    // Cancelling also aborts a pass in flight, so the microphone closes at once.
    let aborted = || {
        let cancelled = operation.cancelled();
        if cancelled {
            cancel.cancel();
        }
        cancelled
    };
    loop {
        if aborted() {
            return Err(Failure::cancelled());
        }
        microphone.check()?;
        let (mut chunk, delivered) = microphone.take();
        if let Some(message) = watch.check(delivered, Instant::now()) {
            return Err(Failure::microphone(message));
        }
        let loudness = level(&chunk);
        let samples = converter.push(&chunk)?;
        chunk.zeroize();
        score(
            &mut vad,
            &mut pending,
            &samples,
            frame,
            &mut live,
            &mut text,
        )?;
        if aborted() {
            return Err(Failure::cancelled());
        }
        if !operation.emit(DictationEvent::Listening {
            seconds: began.elapsed().as_secs() as u32,
            level: (loudness * 8.0).clamp(0.0, 1.0),
            speaking: live.speaking(),
            text: text.clone(),
        }) {
            return Err(Failure::cancelled());
        }
        if operation.stop.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(TICK);
    }
    // The microphone closes before the last pass: the person is done talking.
    let (mut chunk, _) = microphone.take();
    microphone.close();
    let mut samples = converter.push(&chunk)?;
    chunk.zeroize();
    samples.extend(converter.finish()?);
    score(
        &mut vad,
        &mut pending,
        &samples,
        frame,
        &mut live,
        &mut text,
    )?;
    if let Ok(flushed) = vad.stream_flush()
        && let Some(probability) = flushed.probs.first()
        && !pending.is_empty()
    {
        let block = std::mem::take(&mut pending);
        if let Some(next) = live.feed(&block, *probability)? {
            text = next;
        }
    }
    if !operation.emit(DictationEvent::Finishing { text }) {
        return Err(Failure::cancelled());
    }
    let text = live.finish()?;
    if aborted() {
        return Err(Failure::cancelled());
    }
    Ok(DictationEvent::Complete { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_normalizes_samples_and_drops_a_backlog_the_loop_never_took() {
        let mut capture = Capture {
            samples: Vec::new(),
            limit: 2,
            delivered: 0,
        };
        capture.push(&[i16::MAX, i16::MIN, i16::MAX, i16::MAX, 0, 0], 2);
        assert_eq!(capture.samples.len(), 2, "the backlog is bounded");
        assert_eq!(capture.delivered, 3, "the watchdog still sees every frame");
        assert!(capture.samples[0].abs() < 0.001);
        assert!(capture.samples[1] > 0.99);
        let mut floats = Capture {
            samples: Vec::new(),
            limit: 10,
            delivered: 0,
        };
        floats.push(&[f32::NAN, f32::INFINITY, 2.0], 1);
        assert_eq!(floats.samples, vec![0.0, 0.0, 1.0]);
    }

    #[test]
    fn glitches_and_reroutes_keep_recording_and_failures_end_it() {
        use cpal::ErrorKind as K;
        // The Windows regression: an early Xrun ended every recording at once.
        for kind in [K::Xrun, K::DeviceChanged, K::RealtimeDenied] {
            assert_eq!(stream_fault(kind), FAULT_NONE, "{kind:?}");
        }
        assert_eq!(stream_fault(K::PermissionDenied), FAULT_PERMISSION);
        for kind in [
            K::DeviceNotAvailable,
            K::StreamInvalidated,
            K::BackendError,
            K::Other,
        ] {
            assert_eq!(stream_fault(kind), FAULT_STOPPED, "{kind:?}");
        }
        assert_eq!(open_failure(K::PermissionDenied).message, PRIVACY_ERROR);
        assert_eq!(open_failure(K::DeviceBusy).message, BUSY_ERROR);
        assert_eq!(
            open_failure(K::BackendError).reason,
            super::super::DictationFailure::Microphone
        );
    }

    #[test]
    fn the_watchdog_notices_a_stream_that_never_starts_or_stalls() {
        let began = Instant::now();
        let at = |ms: u64| began + Duration::from_millis(ms);
        let mut quiet = Watchdog::new(began);
        assert_eq!(quiet.check(0, at(4_900)), None);
        assert_eq!(quiet.check(0, at(5_100)), Some(MICROPHONE_ERROR));
        let mut stalled = Watchdog::new(began);
        assert_eq!(stalled.check(4_800, at(100)), None);
        assert_eq!(stalled.check(9_600, at(200)), None);
        assert_eq!(stalled.check(9_600, at(3_100)), None);
        assert_eq!(stalled.check(9_600, at(3_300)), Some(STOPPED_ERROR));
        let mut steady = Watchdog::new(began);
        for tick in 1..100 {
            assert_eq!(steady.check(tick * 4_800, at(tick as u64 * 100)), None);
        }
    }

    #[test]
    fn streaming_conversion_keeps_duration_and_speech_band_signal_in_any_chunks() {
        for rate in [16_000, 44_100, 48_000, 96_000] {
            let input: Vec<f32> = (0..rate)
                .map(|n| (n as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.2)
                .collect();
            let mut converter = Converter::new(rate).unwrap();
            let mut output = Vec::new();
            // Uneven chunks, as the microphone delivers them between ticks.
            for chunk in input.chunks(rate / 13 + 7) {
                output.extend(converter.push(chunk).unwrap());
            }
            output.extend(converter.finish().unwrap());
            assert_eq!(output.len(), SAMPLE_RATE, "{rate}");
            let middle = &output[SAMPLE_RATE / 10..SAMPLE_RATE * 9 / 10];
            let rms = level(middle);
            assert!((0.13..0.15).contains(&rms), "{rate}: {rms}");
        }
    }

    /// 16 kHz mono signed 16-bit PCM from a fixture WAV. Not a general WAV decoder.
    fn fixture_pcm(path: &std::path::Path) -> Vec<f32> {
        let wav = std::fs::read(path).expect("fixture WAV");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let mut cursor = 12;
        let mut pcm = None;
        while cursor + 8 <= wav.len() {
            let size = u32::from_le_bytes(wav[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
            if &wav[cursor..cursor + 4] == b"data" {
                pcm = Some(&wav[cursor + 8..(cursor + 8 + size).min(wav.len())]);
                break;
            }
            cursor += 8 + size + size % 2;
        }
        pcm.expect("PCM data")
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0)
            .collect()
    }

    /// A decoder that notes how long each pass took and over how much audio.
    struct Timed<D> {
        inner: D,
        passes: std::rc::Rc<std::cell::RefCell<Vec<(usize, Duration)>>>,
    }

    impl<D: super::super::live::Decoder> super::super::live::Decoder for Timed<D> {
        fn decode(&mut self, pcm: &[f32]) -> Result<String, Failure> {
            let started = Instant::now();
            let text = self.inner.decode(pcm);
            self.passes
                .borrow_mut()
                .push((pcm.len(), started.elapsed()));
            text
        }
    }

    /// Opt-in engine acceptance with the real bundled models, separate from microphone hardware
    /// verification: streams every 16 kHz mono `*.wav` in `O7_DICTATION_FIXTURE_DIR`
    /// through voice detection and live text in 80 ms ticks, as a recording does, and prints the
    /// text, the passes' time and the share of real time they took. `jfk.wav` must come out as
    /// its famous line. The models come from `resources/dictation` (run
    /// `node apps/desktop/scripts/dictation-models.mjs` once).
    ///
    /// It is also the bench a model is chosen with: `O7_DICTATION_MODELS` names another folder,
    /// `O7_DICTATION_ASR` another speech model in it, and `O7_DICTATION_THREADS` another thread
    /// count. Each file prints one tab-separated `RESULT` line (name, seconds of audio, seconds
    /// of passes, live updates, the slowest pass and the audio it covered, text) for a script to
    /// score against the reference beside the WAV.
    #[test]
    #[ignore = "requires the bundled models and O7_DICTATION_FIXTURE_DIR with WAVs"]
    fn bundled_models_transcribe_fixture_audio_live() {
        let fixtures = std::path::PathBuf::from(
            std::env::var_os("O7_DICTATION_FIXTURE_DIR").expect("fixture directory"),
        );
        let models = std::env::var_os("O7_DICTATION_MODELS").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/dictation"),
            std::path::PathBuf::from,
        );
        let asr = std::env::var("O7_DICTATION_ASR")
            .unwrap_or_else(|_| super::super::model::ASR.file.to_owned());
        let began = Instant::now();
        let engine = Engine::shared(&models, &asr, super::super::model::VAD.file)
            .expect("the models to load");
        println!("load {asr}: {:?}", began.elapsed());
        let mut wavs: Vec<_> = std::fs::read_dir(&fixtures)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
            .collect();
        wavs.sort();
        let mut heard = 0.0;
        let mut worked = 0.0;
        for wav in wavs {
            let audio = fixture_pcm(&wav);
            let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
            let cancel = CancelToken::new();
            let passes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let decoder = Timed {
                inner: engine.decoder(&cancel).unwrap(),
                passes: passes.clone(),
            };
            let mut live = Live::new(LiveConfig::default(), decoder);
            let mut vad = engine.vad().unwrap();
            let mut pending = Vec::new();
            let mut text = LiveText::default();
            let mut updates = 0;
            for tick in audio.chunks(SAMPLE_RATE * 80 / 1000) {
                let before = text.clone();
                score(
                    &mut vad,
                    &mut pending,
                    tick,
                    engine.frame_samples(),
                    &mut live,
                    &mut text,
                )
                .unwrap();
                if text != before {
                    updates += 1;
                }
            }
            let result = live.finish().unwrap_or_default();
            let passes = passes.borrow();
            let spent: f32 = passes.iter().map(|(_, took)| took.as_secs_f32()).sum();
            let slowest = passes
                .iter()
                .max_by_key(|(_, took)| *took)
                .copied()
                .unwrap_or_default();
            heard += seconds;
            worked += spent;
            println!(
                "RESULT\t{}\t{seconds:.2}\t{spent:.3}\t{updates}\t{:.3}\t{:.2}\t{result}",
                wav.file_name().unwrap().to_string_lossy(),
                slowest.1.as_secs_f32(),
                slowest.0 as f32 / SAMPLE_RATE as f32,
            );
            if wav.file_name().is_some_and(|name| name == "jfk.wav") {
                assert!(
                    result
                        .to_lowercase()
                        .contains("ask not what your country can do for you"),
                    "{result}"
                );
            }
        }
        println!(
            "TOTAL\t{heard:.1}\t{worked:.2}\t{:.1}% of real time",
            worked / heard.max(0.001) * 100.0
        );
    }
}
