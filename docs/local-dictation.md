# Local dictation

The composer's microphone dictates into the draft. It never sends a message. Recognition runs on
this device with two models that ship inside the app, so there is nothing to install or download:
Parakeet Ultra (speech to text, with punctuation and capitals) and Silero VAD v6.2 (voice
activity). There is no time limit (owner, 4 November 2026: "don't have a limit for it", "don't make
them install it").

The speech model was chosen on 22 November 2026 (owner: "do hella research on the best voice to
text model and change it"); [Why this model](#why-this-model) has the comparison. It hears English
and 24 other European languages and writes whichever it hears, so dictation is no longer English
only.

## Using it

Click the microphone. The composer's controls give way to the bar: Cancel, the live waveform of the
microphone's level (dark where speech was heard), the time, and Finish where Send was. Finish holds
focus, so Enter finishes; Enter in the message box finishes too, and Esc cancels.

While the microphone listens, the words heard so far appear at the cursor as ghost text, lighter
than the draft. Phrases already settled, and words two passes agreed on, are fg-2; words still
forming are fg-3 and may change. The theme's mark is the caret, and an empty draft reads
"Listening". The message box is read-only meanwhile, and the ghost draws the draft itself with the
box's own metrics, so dictated words sit exactly where they will land.

- **Finish** closes the microphone. The last phrase settles, the final text goes into the draft at
  the selection captured when dictation began (or at the end, if the draft changed since), and one
  sweep sets it in ink, its leading edge in the theme's mark. The caret follows the new words.
- **Cancel** (or Esc) dissolves the words, newest first, and the draft is as it was.
- **Failures** show inside the bar, with Try again where it can help. If the microphone stops
  mid-recording, the words already heard stay in the draft and the bar says so. Recording with no
  speech says to check the microphone. Typing in the draft closes a failure.

Closing the chat, hiding the window, switching profiles or quitting cancels a recording. Reduced
motion (the system's or the app's) keeps every state and drops the movement: no sweep, dissolve,
word fade, caret pulse or waveform slide.

## Privacy

Audio lives only in memory and is zeroed when the recording ends: at most a 12 s capture backlog,
the phrase in progress (at most 20 s) and the last 30 s of settled phrases. No audio or transcript
is written to disk or logged; transcribe.cpp's and ggml's native logging is turned off before the
first model loads. Nothing makes a network request, and no account, API key, cloud speech service,
Python or external executable is involved. The finished text becomes a normal draft and follows
the draft's usual persistence.

## Why this model

The requirement: the most accurate speech to text that is still quick on an ordinary machine's
processor, with punctuation and capitals, under a licence o7 can ship, in the engine o7 already
has (transcribe.cpp runs about seventy models, so the engine was not the limit).

Measured here on 22 November 2026, each model through o7's own live pipeline (voice detection,
live passes, the finishing pass): 100 utterances of LibriSpeech test-other (hard read speech), 100
of AMI headset meetings (spontaneous speech, 4 to 20 s, 8 words or more), and 30 prompts a
developer would dictate, spoken by read aloud's voices. Word error rate after one normalizer for
both sides (case, punctuation, fillers, contractions, numbers as digits):

| Model | Size shipped | LibriSpeech test-other | AMI meetings | Developer prompts | Ends sentences |
| --- | ---: | ---: | ---: | ---: | --- |
| Parakeet TDT-CTC 110M (before) | 112 MB | 5.34% | 14.22% | 4.95% | yes |
| **Parakeet Ultra** (Moondream) | 486 MB | **3.18%** | 8.81% | 2.48% | yes |
| Parakeet TDT 0.6B v2 (NVIDIA) | 475 MB | 3.34% | **8.16%** | **1.24%** | yes |
| Parakeet Unified EN 0.6B (NVIDIA) | 477 MB | 3.40% | 8.26% | 2.23% | often not: 56 to 77% of sentences |
| Granite Speech 5.0 470M TurboCTC (IBM) | 279 MB | 3.67% | 8.46% | 2.72% | never: no punctuation or capitals |

Every 0.6B model makes about 40% fewer errors than the model o7 had, and on developers' words the
difference is plain to read: "WebSocket", "TypeScript", "async await", "a 401" and "30 seconds",
where the old model wrote "web socket", "type script", "a sync a wait" and "four o one". Among the
three Parakeets the differences on English are inside what 100 utterances can tell apart. What
decided it:

- **Granite** writes no punctuation and no capitals, which dictation into a message needs.
- **Unified** leaves the full stop off a quarter to almost half of its sentences, and comes under
  NVIDIA's own model licence instead of CC-BY.
- **v2** is English only. **Ultra** is the same size and speed, as accurate on English here, and
  hears 24 more languages; its maker reports it ahead of v3 on every set (5.80% against 6.26% on
  the seven English sets of the Open ASR Leaderboard, and ahead in noise and on long recordings).
  Those figures are Moondream's own and nobody independent has published a check, which is why the
  table above was measured here. A few sentences each of Spanish, French and German (Multilingual
  LibriSpeech) came out at 4.0%, 3.9% and 9.0% through the same pipeline, with nothing set.

Not chosen, without a run: Whisper large-v3-turbo (slower on a processor and less accurate on
English than the Parakeets), Canary-Qwen 2.5B, Cohere Transcribe, Qwen3-ASR 1.7B and Granite Speech
2B (more accurate on paper, but four to ten times the size and far too slow without a graphics
card), Moonshine (small, less accurate), Nemotron streaming (less accurate than the model o7 had on
clean English), and Moondream's Parakeet Redux (178 MB, but only its own runtime reads its packed
weights, and it is the weakest of the family in noise).

What it costs: installers grow by about 375 MB, and since an update downloads the whole installer,
so does every update. The model holds about half a gigabyte of memory from the first dictation
until o7 quits. Each pass costs about four times what it did, which is what the pacing below is
for.

To run the comparison again: `scripts/dictation-bench-audio.mjs` builds the two real-speech sets
(it needs ffmpeg), `the_voices_speak_into_fixture_audio_for_dictation_to_hear` with
`O7_SPEECH_WAV_TEXTS=scripts/dictation-bench-prompts.txt` speaks the prompts,
`bundled_models_transcribe_fixture_audio_live` runs a model over a set (`O7_DICTATION_MODELS`,
`O7_DICTATION_ASR`, `O7_DICTATION_THREADS`), and `scripts/dictation-bench-score.mjs` scores its
output.

## How it works

`apps/desktop/src-tauri/src/dictation` owns recording and recognition:

- **Engine** (`engine.rs`). [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) 0.4.1
  (MIT, through `transcribe-cpp`) statically links ggml. The two models load once, memory-mapped,
  and stay loaded, so later recordings start at once; each recording gets its own sessions. Passes
  run on one thread on a machine with one or two, two on most, and four on machines with twelve or
  more: never all of them. macOS builds also enable ggml's Metal backend, the crate's default. No
  language is set, so the model writes the language it hears.
- **Capture** (`audio.rs`). CPAL opens the default input; mono audio passes through a chunked rubato
  FFT resampler to 16 kHz, and Silero VAD scores every 32 ms frame. The loop reports every 80 ms.
  The callback holds at most 12 s of audio for the loop, which a pass keeps waiting.
  Watchdogs end a stream that delivers nothing within 5 s or stops for 3 s; glitch reports (`Xrun`,
  `DeviceChanged`, `RealtimeDenied`) leave the recording running, since cpal reports them through
  the same callback as failures (the November 2026 Windows fix, #68).
- **Pacing** (`live.rs`). The model is large and its speed depends on the machine, so every pass
  is timed and the next ones are planned from that. The phrase in progress is read again only once
  the new audio is two and a half times what the pass is expected to take, which keeps the live
  passes to about two fifths of real time. A phrase settles early, at 6 s or more, once one pass
  over it would take longer than 2.5 s. Finishing reads the last half minute again only when that
  takes 4 s or less, and never when it holds a single phrase, which was read whole already. On a
  fast machine none of this is felt; on a slow one the words arrive in larger steps and the
  microphone is never outrun.
- **Live text** (`live.rs`). Parakeet is an offline model, so live text comes from phrases. A phrase
  opens when the speech probability reaches 0.5, keeping 0.25 s of audio before it, and closes after
  0.7 s below 0.35, or at 20 s. While it runs it is transcribed again every 0.8 s, and the words two
  consecutive passes agree on become stable: the local-agreement policy of streaming Whisper. A
  closed phrase is transcribed once more and settles. Silence costs only voice detection.
- **Finishing** transcribes the last 30 s of phrases again as one clip, so a pause in the middle of
  a sentence cannot split its words; a failed or empty pass keeps the phrases' own text.
- **Events** reach the webview over a typed channel: `preparing`, then `listening` every 80 ms with
  the time, the level, whether a phrase is open and the text so far (`settled`, `stable`,
  `tentative`), then `finishing` and `complete`, or `cancelled`, or `failed` with a reason (`model`,
  `microphone`, `no_speech`, `unsupported`, `other`). One recording runs at a time, tied to its
  webview and profile generation.

`apps/desktop/src/lib/dictation.ts` owns one recording per composer: late events and command
completions cannot reach another chat, and cancelling discards a late result.
`components/chat/Dictation.tsx` draws the bar, the waveform and the ghost; `dictation.css` holds
their motion, timed by `--g-dur-wave` (80 ms), `--g-dur-ink` (560 ms) and `--g-dur-dissolve`
(320 ms).

### Measurements

On a Ryzen 7 5800X3D (16 threads, Windows 11 build 26300, measured in WSL), with the bundled
models, over the 100 LibriSpeech test-other utterances (11 minutes of speech):

- Four threads, as this machine gets: the passes took 28% of real time; the slowest, over a
  19.8 s phrase, took 1.1 s. Two threads, as most laptops get: 35%, and 1.8 s.
- Without the pacing the same runs took 45% and 76% of real time, and on two threads a single pass
  reached 2.1 s. The model o7 had took 20% on two threads.
- Accuracy is the same with and without pacing (3.18%).
- Loading both models takes 0.3 to 0.9 s the first time; they stay loaded.
- A machine half as fast per thread reads a 12 s phrase in about 2.5 s on two threads: there the
  early settle and the longer wait between live passes apply. That case is covered by the unit
  tests with a decoder of that speed, not by a run on such a machine.

## Bundling and builds

The models are pinned in `apps/desktop/scripts/dictation-models.mjs` to a Hugging Face revision, a
size and a SHA-256:

| File | Source | Size | License |
| --- | --- | ---: | --- |
| `parakeet-ultra-Q4_K_M.gguf` | [handy-computer/parakeet-ultra-gguf](https://huggingface.co/handy-computer/parakeet-ultra-gguf/tree/a1539be1b849ea7463b88343e5277b373d0c4ea9) | 486,280,096 B | CC-BY-4.0 |
| `silero-vad-v6.2-F32.gguf` | [handy-computer/silero-vad-v6.2-gguf](https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/tree/a9b2d61c20ae4ddf3644edb34969e8185a485448) | 1,241,056 B | MIT |

Q4_K_M, the smallest published, keeps Parakeet Ultra at 1.82% WER on LibriSpeech test-clean (F32:
1.80%), so the larger files buy nothing. Parakeet Ultra is Moondream's further training of NVIDIA's
Parakeet TDT 0.6B v3; both are CC-BY-4.0, and the notices name both. The script also removes a
model file it no longer pins, since the whole folder is packaged. `pnpm bundle` fetches any
model that is missing and verifies every one before `tauri build` packages
`src-tauri/resources/dictation/` as the app's `dictation/` resources; the Build workflow runs the
same script. The files are not committed. For a debug build, run `node scripts/dictation-models.mjs`
from `apps/desktop` once (`--check` only verifies); without them dictation says the build is missing
its models. The app checks their sizes at runtime; the installer and updater signatures cover their
contents. A test keeps the Rust pins and the script's in step, and the notices generator fails when
`third-party/models.json` disagrees with the script. The notices carry each model's license, its
GGUF conversion's NOTICE and an attribution.

Building needs CMake 3.15 or newer with the usual C/C++ toolchain (Visual Studio's C++ workload on
Windows, the Xcode command line tools on macOS). transcribe-cpp-sys ships its bindings, so libclang
is no longer needed.

- **Instruction set.** `.cargo/config.toml` passes `TRANSCRIBE_CMAKE_ARGS`, which the crate forwards
  to CMake after its own defaults and rebuilds on: AVX, AVX2, FMA, F16C and SSE4.2 on, BMI2 and
  host tuning (`GGML_NATIVE`) off, so a release runs on every Mac that runs macOS 13.3 and on Intel
  (2013) and AMD (2015) processors onward. `dictation::audio::cpu_supported()` checks AVX, AVX2,
  FMA and F16C before any engine call, and the bar explains, without a retry, on older processors.
  ggml applies the flags to its CPU backend only, so the rest of the app starts and works there.
  ARM builds ignore the x86 flags.
- **Runtime.** The same arguments select CMake's `CMP0091=NEW` and
  `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`, so the engine's C/C++ runtime is in the static
  library and the installer needs no Visual C++ Redistributable. `dumpbin /DEPENDENTS` on the
  November 2026 debug app listed only Windows system and Universal CRT API-set DLLs: no `MSVCP*`,
  `VCRUNTIME*`, `VCOMP*` or ggml DLL. Repeat it on the bundled executable after changing the engine
  or the toolchain.
- **Windows ARM64.** ggml accepts MSVC's `cl.exe` on ARM only when it tunes to the build machine, so
  the Build workflow selects Ninja and `.github/cmake/windows-arm64-clang.cmake` (clang-cl with the
  runner's MSVC ABI, headers and libraries, and C++ `/EHsc`) for that row only.
- **macOS** bundles keep `NSMicrophoneUsageDescription` and the hardened-runtime audio-input
  entitlement. Windows uses the system's desktop-app microphone permission.

## Verification

- `cargo test -p o7-desktop --lib dictation`: capture, conversion, voice detection, live
  agreement, settling, the finishing window, the pacing (a slow model is read less often and in
  shorter phrases, finishing skips a pass that would be slow or redundant), zeroing, operation
  ownership and the model pins (19 tests). The opt-in `bundled_models_transcribe_fixture_audio_live` runs the bundled models over
  every 16 kHz mono WAV in `O7_DICTATION_FIXTURE_DIR` as the microphone would deliver it, and
  prints the live updates, timing and final text (`-- --ignored --nocapture`).
- `vitest run src/lib/dictation.test.ts`: the controller (live text, the bounded waveform, finishing,
  cancelling, late results, kept words, failures) and insertion at the selection.
- `node scripts/check-dictation.mjs` against the `?preview=dictation` fixture on Vite (port 1448):
  Chromium and WebKit, light and dark, axe in every state, focus, the read-only draft, insertion at
  the caret, Esc and Cancel, chat switching, every failure, a microphone that drops, and, with full
  motion, the sweep and the dissolve ending and typing ending the sweep. It also compares the draft
  as the ghost draws it with the message box pixel for pixel. The fixture never opens a microphone.
- Natively, on Windows x64 (debug app, scratch profile, driven over CDP), 22 November 2026 with
  Parakeet Ultra: the bundled models were found and loaded, the real microphone reported every
  80 ms, and read aloud spoke "I fixed the login flow, and all 42 tests pass. Nothing else
  changed." through the speakers while dictation listened: the microphone heard it (level peak
  0.50), the words appeared live in eight updates, and the finished text was the sentence, word for
  word. Finishing took 80 ms, since the one phrase had settled already. Earlier, with the model o7
  had: finishing with no speech said so, and cancelling ended the recording.

Not yet verified: a person dictating at length into the native microphone (accuracy comes from
the recorded sets above; the native run heard a synthetic voice over the air), a machine slower
than the test desktop, the macOS build and its Metal backend, the Windows ARM64 build, and any
on-device run of either. Linux is deferred with the rest of the Linux port.

The live text's word agreement and the waveform do not depend on the language, but only English,
Spanish, French and German have been run here; the other languages rest on the model's published
numbers. A language cannot be chosen: the model writes what it hears.
