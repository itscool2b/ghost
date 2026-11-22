# Read aloud

The speaker under an answer reads it aloud (owner, 22 November 2026: "find the absolute best TTS
model which is reasonably fast for machines for daily use"). The voice is Gradium's Phonon 31M. It
ships inside the app and runs on this device, on the processor, so there is nothing to install,
no account and no network request.

## Using it

Every finished answer has a speaker in its row of quiet actions, after Copy. Press it and the
answer is read; the button becomes a stop square, and pressing it again stops at once. Pressing another answer's speaker moves the voice there: one thing is read at a time.
Reaching the button with the pointer or the keyboard readies the voice, so the click speaks at
once.

Reading belongs to the chat it was started in. Opening another chat, closing the chat, going to
Settings, switching profiles and quitting all end it, so a voice is never left talking with its
stop button out of reach. Scrolling, typing and sending in the same chat do not.

What is said is the answer's words, not its marks (`speakable` in `apps/desktop/src/lib/speech.ts`):

- Code blocks are not read. Each is named in its place ("Code block."), because code cannot be
  followed by ear. Inline code is read as its words.
- A link says its words, a bare address says "link", an image says its description.
- Headings and list items end as sentences, a table is read a row at a time, and rules and other
  marks say nothing.
- Only the answer's prose is read: not the steps, the thinking or the tool output.

Settings > Voice > Read aloud names the model, lists the voices (American, British and
Characters) and plays a sample of a choice. The choice is saved with the profile's other
preferences (`general.speech_voice`); until one is made, the model's own voice speaks. A saved
voice a later build no longer has falls back to the model's own.

A build without the voice model, and a processor the engine cannot run on (x86 without AVX2),
show no speaker at all; Settings says which. A failure (no output device, a device another program
holds, a device that stops) says why beside the button that asked.

One request reads at most 24,000 bytes of text, about twenty minutes of speech. A longer answer is
read up to the last whole sentence that fits.

## Why this model

The requirement was the best voice that is still fast on an ordinary machine. What was compared
(November 2026):

| Model | Size | Why or why not |
| --- | ---: | --- |
| **Gradium Phonon 31M** (English) | 31M parameters, 64.5 MB | Chosen. Apache 2.0 weights and voices. Runs several times faster than speech on a few processor threads, streams as it goes, needs no phonemizer, and has an official Rust runtime. |
| Kokoro 82M | 82M | Lower in blind listening tests, needs a phonemizer (the usual one, espeak-ng, is GPL), and does not stream. |
| Chatterbox | about 500M | Lower in listening tests and far heavier. |
| Supertonic 3 | 99M | Repository archived in September 2026; its OpenRAIL-M licence passes use restrictions on to everybody downstream. |
| Pocket TTS | 100M | Slower and rated lower than Phonon. |
| ZONOS3-Mini | about 800M | No processor-only numbers, C++ only. |
| Phonon 40M (multilingual) | 40M | CC BY-NC: not usable in an app people may use for work. |
| Fish Audio Drama 3, Breeze TTS 2, Ming-3.0 | billions | Rated higher, but each needs a graphics card. |

In Artificial Analysis's blind arena of open-weights models, Phonon 31M stood at Elo 1108 (fifth),
above Kokoro 82M (1060) and Chatterbox (1019); everything above it is a large model for a graphics
card. Those ratings move; the choice rests on it being the best-rated model that also meets the
other needs: permissive licence, no copyleft dependency, streaming, and real-time speed on two
threads.

What it costs: installers grow by about 72 MB (the model is 64.5 MB, the 37 catalogue voices
256 KB each); it speaks English only; and once something has been read the model stays loaded
until o7 quits, about 230 MB of memory.

## Privacy and what may ask

- The text goes from the page to the speech engine in this process and out of the default output
  device. It is not stored, not logged and not sent anywhere; the engine's own logging is held at
  warnings (`DEFAULT_LOG_FILTER`). At most two seconds of sound wait in memory ahead of the
  speaker, and the queue is dropped when the request ends.
- Only the app's own page can ask, through four commands (`speech_status`, `speech_speak`,
  `speech_stop`, `speech_prepare`). Agents, linked assistants and remote sessions have no tool that
  speaks.
- A request is tied to its webview and profile generation, as dictation's is: a page that is
  replaced, a profile switch and quitting all silence it, and only the page that started a request
  can stop it.
- The text must be real text of at most 24,000 bytes with no NUL. A voice is the model's own or an
  id of the catalogue's form (8 to 32 of `A-Z a-z 0-9 _ -`): it is never used as a path. Which
  voices exist is decided from the bundled files beside `voices.json`, each of the pinned size.
- The saved preference is a bounded single-line name; anything else is refused by the core.

Each refusal has a test (`speech::commands::tests`, `speech::tests`,
`onboarding::tests::the_read_aloud_voice_is_saved_and_refuses_anything_but_a_name`).

## How it works

`apps/desktop/src-tauri/src/speech` owns it:

- **Model** (`model.rs`). The pinned files, the catalogue (`resources/speech/voices.json`, in the
  catalogue's own words), and the status the page asks for. A voice is offered only when its file
  is present at the pinned size.
- **Engine** (`engine.rs`). [ptts](https://crates.io/crates/ptts) 0.7.1, Gradium's own Rust
  runtime (MIT or Apache 2.0), over its tensor library `xn`, on the processor. The model loads
  once and stays loaded. It is given one thread on a machine with one or two, two on most, and
  four on machines with twelve or more: left to take every thread it gets slower, not faster.
- **Playback** (`playback.rs`). CPAL opens the default output; the engine's 24 kHz mono samples
  pass through a chunked rubato FFT resampler to the device's rate and are copied to every
  channel. The engine may run two seconds ahead of the speaker and no further. Glitch reports
  (`Xrun`, `DeviceChanged`, `RealtimeDenied`) do not end the speech, as in dictation's capture.
  The output callback reads the request's stop flag itself, so a stopped or replaced request is
  silent from the next buffer and two voices are never heard together.
- **Requests** (`mod.rs`, `commands.rs`). One request at a time across the app: a new one stops
  the one before it. Events reach the page over a typed channel: `preparing`, `speaking`, then
  `done`, `stopped`, or `failed` with a reason (`model`, `output`, `unsupported`, `other`).

`apps/desktop/src/lib/speech.ts` holds the page's one voice (`SpeechPlayer`): which button is
being read, and late events from a stopped request cannot touch the next.
`components/chat/ReadAloud.tsx` is the button, `components/settings/ReadAloudSettings.tsx` the
Settings rows.

### Measurements

On a Ryzen 7 5800X3D, with the bundled q8 model:

- Loading the model: about 80 ms.
- Speed against real time: 3.5 times on one thread, 6.1 on two, 9.1 on four, and 2.6 when left
  all sixteen. On two threads in the test below: 7.8 to 8.1 times.
- In the Windows debug app: the first request after launch began to sound 570 to 690 ms after
  the click (model load and the voice's first use); the next began after 55 ms; the first use of
  another voice, about 390 ms. A stop silences the output at the device's next buffer; the
  request's `stopped` event followed 140 to 270 ms after the press, once the device was closed.
- Memory: the app grew from 72 MB to 305 MB once the model had spoken.

## Bundling and builds

The files are pinned in `apps/desktop/scripts/dictation-models.mjs` (`SPEECH`) to a Hugging Face
revision, a size and a SHA-256, like dictation's:

| Files | Source | Size | License |
| --- | --- | ---: | --- |
| `model.q8.gguf`, `config.json`, `tokenizer.json`, `default-voice.safetensors` | [gradium/phonon-31m-en](https://huggingface.co/gradium/phonon-31m-en/tree/8f7a37c94a4c58b04c8287300c53f11d2e208eae) | 64,512,352 B and three small files | Apache-2.0 |
| `voices/*.safetensors` (37) | [gradium/phonon-voices](https://huggingface.co/gradium/phonon-voices/tree/0ee17205f254b5abc43b266c8a0f215565c352f7) | 256,136 B each | Apache-2.0 |

`pnpm bundle` fetches any that are missing and verifies every one before `tauri build` packages
`src-tauri/resources/speech/` as the app's `speech/` resources. The files are not committed;
`voices.json` is. For a debug build run `node scripts/dictation-models.mjs` from `apps/desktop`
once. A test keeps the Rust pins and the script's in step, and the notices generator fails when
`third-party/models.json` disagrees with the script. The notices carry the Apache 2.0 text for
the model and the voices.

The engine is Rust throughout: no CMake, no C++ and no phonemizer. `ptts` is built with its `hf`
feature alone (the tokenizer), and `xn` with none: no graphics backend, and not the vendored
KleidiAI kernels.

## Verification

- `cargo test -p o7-desktop --lib speech`: the request registry, the refusals, the model's pins,
  the catalogue, the thread choice, and the resampler (a tone keeps its length, pitch and level at
  16, 44.1, 48 and 96 kHz).
- With the model fetched, `-- --ignored --nocapture` also runs
  `the_bundled_model_speaks_faster_than_real_time` (every catalogued voice loads; real samples,
  faster than they play) and, with `O7_SPEECH_WAV_DIR`, writes one sentence in four voices as
  16 kHz WAVs. Dictation's own model then transcribes them
  (`O7_DICTATION_FIXTURE_DIR=<that folder>`, `bundled_models_transcribe`). On 22 November 2026 all
  four came back word for word, "42" as "forty two": "I fixed the login flow, and all forty two
  tests pass. Nothing else changed."
- `vitest run src/lib/speech.test.ts`: what is said for Markdown, the cut at a sentence, and the
  player (one voice, stop, takeover, late events, failures, readying).
- `node scripts/check-read-aloud.mjs` (port 1466), Chromium and WebKit, light and dark, with a
  stand-in engine that makes no sound: the button beside Copy, reading and stopping by pointer and
  keyboard, takeover, a failure, opening another chat, no button without the model or on an
  unsupported processor, Settings' groups, sample and saved choice, axe, and no pulse under
  reduced motion.
  `check-tour.mjs` flies the Read aloud stop.
- Natively, on Windows x64 (debug app, scratch profile, driven over CDP): the model and all 38
  voices were found; speech played through the default output for as long as the sentence lasts;
  stop and takeover worked; a path as a voice, empty text and oversized text were refused;
  Settings listed the voices, played a sample and saved the choice.

Not verified: how it sounds to a person (the test machine cannot be listened to; intelligibility
rests on the round trip above and quality on the published listening tests), macOS, Linux and
Windows ARM64 builds and any on-device run of them.

## Not built

- Reading every answer automatically as it arrives. The speaker is pressed, never automatic.
- Languages other than English: the multilingual Phonon is non-commercial.
- Voice cloning. The runtime can, the app does not offer it.
- Releasing the model's memory after a while without use.
