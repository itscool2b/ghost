/**
 * Dictation's bench, part two (docs/local-dictation.md, "Why this model"): the word error rate of
 * each model's `RESULT` lines (from `bundled_models_transcribe_fixture_audio_live`) against the
 * reference beside each WAV, with the passes' timing. Outputs are named
 * `<set>.<model>.t<threads>.txt`. The normalizer follows the Whisper English normalizer's main
 * rules (case, punctuation, fillers, contractions, numbers as digits) and is applied to both sides,
 * so models are compared on words, not on formatting.
 *
 *   node scripts/dictation-bench-score.mjs <out dir> <audio dir> [--errors <model substring>]
 */
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [outDir, audioDir, flag, wanted] = process.argv.slice(2);

const UNITS = {
  zero: 0,
  oh: 0,
  one: 1,
  two: 2,
  three: 3,
  four: 4,
  five: 5,
  six: 6,
  seven: 7,
  eight: 8,
  nine: 9,
};
const TEENS = {
  ten: 10,
  eleven: 11,
  twelve: 12,
  thirteen: 13,
  fourteen: 14,
  fifteen: 15,
  sixteen: 16,
  seventeen: 17,
  eighteen: 18,
  nineteen: 19,
};
const TENS = {
  twenty: 20,
  thirty: 30,
  forty: 40,
  fifty: 50,
  sixty: 60,
  seventy: 70,
  eighty: 80,
  ninety: 90,
};
const SCALES = { hundred: 100, thousand: 1000, million: 1000000 };
const CONTRACTIONS = [
  [/\bwon't\b/g, "will not"],
  [/\bcan't\b/g, "can not"],
  [/\bcannot\b/g, "can not"],
  [/\blet's\b/g, "let us"],
  [/\bain't\b/g, "aint"],
  [/\by'all\b/g, "you all"],
  [/\bwanna\b/g, "want to"],
  [/\bgotta\b/g, "got to"],
  [/\bgonna\b/g, "going to"],
  [/\b'cause\b/g, "because"],
  [/\bcuz\b/g, "because"],
  [/n't\b/g, " not"],
  [/'re\b/g, " are"],
  [/'s\b/g, " is"],
  [/'d\b/g, " would"],
  [/'ll\b/g, " will"],
  [/'t\b/g, " not"],
  [/'ve\b/g, " have"],
  [/'m\b/g, " am"],
  [/\bokay\b/g, "ok"],
  [/\bmister\b/g, "mr"],
  [/\bmissus\b/g, "mrs"],
  [/\bdoctor\b/g, "dr"],
  [/\bsaint\b/g, "st"],
];

/** Runs of number words become one string of digits, as "eighteen sixty" and "1860" both mean. */
function numbers(words) {
  const out = [];
  let run = [];
  const flush = () => {
    if (run.length === 0) return;
    // A lone "oh" or "a hundred" style word is a word, not a number.
    if (run.length === 1 && (run[0] === "oh" || run[0] in SCALES)) {
      out.push(run[0]);
      run = [];
      return;
    }
    let digits = "";
    let total = 0;
    let group = 0;
    let last = "none";
    const close = () => {
      if (last !== "none") digits += String(total + group);
      total = 0;
      group = 0;
      last = "none";
    };
    for (const word of run) {
      if (word in UNITS) {
        if (last === "tens" || last === "scale") group += UNITS[word];
        else {
          close();
          group = UNITS[word];
        }
        last = "unit";
      } else if (word in TEENS) {
        if (last === "scale") group += TEENS[word];
        else {
          close();
          group = TEENS[word];
        }
        last = "teen";
      } else if (word in TENS) {
        if (last === "scale") group += TENS[word];
        else {
          close();
          group = TENS[word];
        }
        last = "tens";
      } else if (word === "hundred") {
        group = (group || 1) * 100;
        last = "scale";
      } else {
        total += (group || 1) * SCALES[word];
        group = 0;
        last = "scale";
      }
    }
    close();
    out.push(digits);
    run = [];
  };
  for (let at = 0; at < words.length; at += 1) {
    const word = words[at];
    const numeric = word in UNITS || word in TEENS || word in TENS || word in SCALES;
    if (numeric) run.push(word);
    else if (
      word === "and" &&
      run.length > 0 &&
      run.at(-1) in SCALES &&
      (words[at + 1] in UNITS || words[at + 1] in TEENS || words[at + 1] in TENS)
    )
      continue;
    else {
      flush();
      out.push(word);
    }
  }
  flush();
  // Digits a model wrote apart ("1 2 3") read as one number, as the run of words does.
  const merged = [];
  for (const word of out) {
    if (/^\d+$/.test(word) && /^\d+$/.test(merged.at(-1) ?? "")) merged[merged.length - 1] += word;
    else merged.push(word);
  }
  return merged;
}

export function normalize(text) {
  let line = text
    .toLowerCase()
    .replace(/[[(<][^\])>]*[\])>]/g, " ")
    .replace(/’/g, "'");
  for (const [from, to] of CONTRACTIONS) line = line.replace(from, to);
  line = line
    .replace(/(\d),(\d)/g, "$1$2")
    .replace(/[^\p{L}\p{N}' ]+/gu, " ")
    .replace(/(^|\s)'+|'+(\s|$)/g, " ")
    .replace(/\b(hmm|mm|mhm|mmm|uh|um|uhm|erm|er|ah|eh|huh)\b/g, " ");
  return numbers(line.split(/\s+/).filter(Boolean)).join(" ").split(" ").filter(Boolean);
}

function distance(a, b) {
  let previous = Array.from({ length: b.length + 1 }, (_, index) => index);
  for (let i = 1; i <= a.length; i += 1) {
    const row = [i];
    for (let j = 1; j <= b.length; j += 1)
      row[j] = Math.min(
        previous[j] + 1,
        row[j - 1] + 1,
        previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1),
      );
    previous = row;
  }
  return previous[b.length];
}

const rows = [];
for (const file of readdirSync(outDir)
  .filter((name) => name.endsWith(".txt"))
  .sort()) {
  const [set, ...rest] = file.replace(/\.txt$/, "").split(".");
  const threads = rest.pop();
  const model = rest.join(".");
  const lines = readFileSync(join(outDir, file), "utf8")
    .split("\n")
    .filter((line) => line.startsWith("RESULT\t"));
  let errors = 0;
  let words = 0;
  let audio = 0;
  let spent = 0;
  let slowest = 0;
  let slowestAudio = 0;
  let punctuated = 0;
  let capital = 0;
  let empty = 0;
  const worst = [];
  for (const line of lines) {
    const [, name, seconds, passes, , slow, slowClip, ...text] = line.split("\t");
    const heard = text.join("\t").trim();
    const reference = join(audioDir, set, name.replace(/\.wav$/, ".txt"));
    if (!existsSync(reference)) continue;
    const expected = normalize(readFileSync(reference, "utf8"));
    const got = normalize(heard);
    const wrong = distance(expected, got);
    errors += wrong;
    words += expected.length;
    audio += Number(seconds);
    spent += Number(passes);
    if (Number(slow) > slowest) {
      slowest = Number(slow);
      slowestAudio = Number(slowClip);
    }
    if (/[.?!]$/.test(heard)) punctuated += 1;
    if (/^[A-Z]/.test(heard)) capital += 1;
    if (heard === "") empty += 1;
    worst.push({ name, wrong, expected: expected.join(" "), got: got.join(" "), heard });
  }
  if (words === 0) continue;
  rows.push({
    set,
    model,
    threads,
    files: lines.length,
    wer: ((errors / words) * 100).toFixed(2),
    "passes % of real time": ((spent / audio) * 100).toFixed(1),
    "slowest pass s": slowest.toFixed(2),
    "over s of audio": slowestAudio.toFixed(1),
    "ends in .?! %": Math.round((punctuated / lines.length) * 100),
    "capital first %": Math.round((capital / lines.length) * 100),
    empty,
  });
  if (flag === "--errors" && model.includes(wanted)) {
    for (const entry of worst.sort((a, b) => b.wrong - a.wrong).slice(0, 8))
      console.log(
        `${set} ${entry.name} (${entry.wrong}):\n  ref: ${entry.expected}\n  hyp: ${entry.got}\n  raw: ${entry.heard}`,
      );
  }
}
console.table(rows);
