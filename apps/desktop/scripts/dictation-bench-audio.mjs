/**
 * Dictation's bench, part one (docs/local-dictation.md, "Why this model"): a small real-speech
 * test set from public corpora, through the Hugging Face datasets server. LibriSpeech test-other
 * is hard read speech; AMI headset meetings are spontaneous speech. Each utterance becomes a
 * 16 kHz mono WAV with its reference text beside it. Needs ffmpeg on the path.
 *
 *   node scripts/dictation-bench-audio.mjs <out dir> [librispeech count] [ami count]
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [out, wantLibri = "150", wantAmi = "150"] = process.argv.slice(2);
const server = "https://datasets-server.huggingface.co/rows";
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

async function rows(dataset, config, split, offset, length) {
  const url = `${server}?dataset=${encodeURIComponent(dataset)}&config=${config}&split=${split}&offset=${offset}&length=${length}`;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const response = await fetch(url);
    if (response.ok) return (await response.json()).rows.map((entry) => entry.row);
    await pause(4000 * (attempt + 1));
  }
  throw new Error(`rows failed: ${url}`);
}

async function save(folder, name, src, text) {
  const wav = join(folder, `${name}.wav`);
  if (!existsSync(wav)) {
    const response = await fetch(src);
    if (!response.ok) return false;
    const raw = join(folder, `${name}.src`);
    writeFileSync(raw, Buffer.from(await response.arrayBuffer()));
    execFileSync("ffmpeg", [
      "-loglevel",
      "error",
      "-y",
      "-i",
      raw,
      "-ar",
      "16000",
      "-ac",
      "1",
      "-c:a",
      "pcm_s16le",
      wav,
    ]);
    execFileSync("rm", ["-f", raw]);
  }
  writeFileSync(join(folder, `${name}.txt`), `${text}\n`);
  return true;
}

async function librispeech(count) {
  const folder = join(out, "librispeech-other");
  mkdirSync(folder, { recursive: true });
  let saved = 0;
  // Spread over the split, a few utterances at each stop, so many speakers are heard.
  for (let offset = 0; offset < 2930 && saved < count; offset += 59) {
    for (const row of await rows("openslr/librispeech_asr", "other", "test", offset, 3)) {
      if (saved >= count) break;
      if (await save(folder, row.id, row.audio[0].src, row.text)) saved += 1;
    }
    await pause(250);
  }
  console.log(`librispeech test-other: ${saved}`);
}

async function ami(count) {
  const folder = join(out, "ami-ihm");
  mkdirSync(folder, { recursive: true });
  let saved = 0;
  // Meeting segments are often one word: keep those long enough to be a sentence.
  for (let offset = 0; offset < 12600 && saved < count; offset += 397) {
    let kept = 0;
    for (const row of await rows("edinburghcstr/ami", "ihm", "test", offset, 100)) {
      if (saved >= count || kept >= 6) break;
      const seconds = row.end_time - row.begin_time;
      if (seconds < 4 || seconds > 20 || row.text.trim().split(/\s+/).length < 8) continue;
      if (await save(folder, row.audio_id, row.audio[0].src, row.text)) {
        saved += 1;
        kept += 1;
      }
    }
    await pause(250);
  }
  console.log(`ami ihm: ${saved}`);
}

await librispeech(Number(wantLibri));
await ami(Number(wantAmi));
