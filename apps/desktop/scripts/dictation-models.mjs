/**
 * The speech models o7 bundles: for dictation (docs/local-dictation.md) and for reading aloud
 * (docs/read-aloud.md). Each is pinned to a Hugging Face revision, a size and a SHA-256, fetched
 * into src-tauri/resources/dictation or resources/speech before a release bundle, and verified
 * there. The files are model weights and their settings only: nothing runs from them.
 *
 *   node scripts/dictation-models.mjs          fetch what is missing or wrong, then verify
 *   node scripts/dictation-models.mjs --check  verify only; fails when a file is missing or wrong
 *
 * Debug builds and tests run without them: the resource folder keeps a README so Tauri's resource
 * glob always matches, and dictation then says the models are missing.
 */
import { createHash } from "node:crypto";
import {
  createReadStream,
  createWriteStream,
  existsSync,
  mkdirSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

export const MODEL_DIR = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src-tauri/resources/dictation",
);

/** Kept in step with `dictation::model::BUNDLED` in the Rust crate (a test compares them). */
export const MODELS = [
  {
    // English speech to text with punctuation and capitals, CC-BY-4.0 (NVIDIA; GGUF conversion by
    // handy-computer for transcribe.cpp). Q6_K: 2.44% WER on LibriSpeech test-clean, F32 2.43%.
    // Small on purpose: an update package must stay under the 512 MiB every installed copy
    // accepts (docs/local-dictation.md, "Why the built-in model is small").
    file: "parakeet-tdt_ctc-110m-Q6_K.gguf",
    repo: "handy-computer/parakeet-tdt_ctc-110m-gguf",
    revision: "1c3f0d5378fdd358905cc410a93ed9e64f52d170",
    size: 112311264,
    sha256: "c20520c245adf82e5166005f599cb3b95e7cf5192117e845be4bbcd39226d483",
  },
  {
    // Voice activity detection, MIT (Silero; GGUF conversion by handy-computer).
    file: "silero-vad-v6.2-F32.gguf",
    repo: "handy-computer/silero-vad-v6.2-gguf",
    revision: "a9b2d61c20ae4ddf3644edb34969e8185a485448",
    size: 1241056,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
  },
];

/**
 * Not bundled. The larger speech model the app fetches when a person asks for it in Settings >
 * Voice (src-tauri/src/dictation/larger.rs): Moondream's Parakeet Ultra, a further-trained NVIDIA
 * parakeet-tdt-0.6b-v3, CC-BY-4.0, in handy-computer's GGUF conversion. Q4_K_M: 1.82% WER on
 * LibriSpeech test-clean, F32 1.80%. With it inside, a macOS or Linux package would pass the
 * 512 MiB an installed copy accepts as an update (docs/local-dictation.md). It is pinned here so
 * the notices record it, and a test keeps `dictation::model::LARGER` in step.
 */
export const LARGER = {
  file: "parakeet-ultra-Q4_K_M.gguf",
  repo: "handy-computer/parakeet-ultra-gguf",
  revision: "a1539be1b849ea7463b88343e5277b373d0c4ea9",
  size: 486280096,
  sha256: "c69b7a5f9071a7afd8a1818ef7ae542c386c4b01ed0cac380c19a99947646bf9",
};

export const SPEECH_DIR = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src-tauri/resources/speech",
);

/**
 * The voice model o7 bundles for reading aloud (docs/read-aloud.md), kept in step with
 * `speech::model::BUNDLED` and resources/speech/voices.json in the Rust crate (a test compares
 * them). Phonon 31M English, 8-bit, with its config, its tokenizer and its own voice: Apache 2.0
 * (Gradium). Then the English voices of Gradium's catalogue, Apache 2.0, one small tensor each.
 */
export const SPEECH = [
  {
    file: "model.q8.gguf",
    repo: "gradium/phonon-31m-en",
    revision: "8f7a37c94a4c58b04c8287300c53f11d2e208eae",
    size: 64512352,
    sha256: "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae",
  },
  {
    file: "config.json",
    repo: "gradium/phonon-31m-en",
    revision: "8f7a37c94a4c58b04c8287300c53f11d2e208eae",
    size: 4634,
    sha256: "b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85",
  },
  {
    file: "tokenizer.json",
    repo: "gradium/phonon-31m-en",
    revision: "8f7a37c94a4c58b04c8287300c53f11d2e208eae",
    size: 277967,
    sha256: "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936",
  },
  {
    file: "default-voice.safetensors",
    repo: "gradium/phonon-31m-en",
    revision: "8f7a37c94a4c58b04c8287300c53f11d2e208eae",
    size: 256136,
    sha256: "dc537478eaadc1a5257fcda2e3501ab81e14c111a5d95a5838f350db2afd0f1a",
  },
  // Zoey
  {
    file: "voices/NbpkqMVS3CJeq2j8.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "97c7f8c253413574406ab80dd23f49c3ebb2517163b00a9bf86b9c4cf43e7e23",
  },
  // Juniper
  {
    file: "voices/NIWIOv5vTi5eRMj8.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "444378eb06b7c492b86e8fc7d13e4db1a3c0cf3e68891a960bf6f0a41a13ef35",
  },
  // Everly
  {
    file: "voices/vjFyp2BcCyTlUmYb.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "958ea94da14f46dd48c7f64adac11ed2ad2aa45f7ccd165dfebab5272664402e",
  },
  // Harper
  {
    file: "voices/4SZHfMpw-p46Ywgs.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "6b3da6e2d796a20c51af892c482bd6509fb88267a221aff83c870843dee2a123",
  },
  // Hadley
  {
    file: "voices/bXwrkHhURL1VFJdG.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a879938aa9a2df0a8220bdadef88137929f71e887bd91e7db205bd5fa11661d2",
  },
  // Delia
  {
    file: "voices/nGzMHMPAWGaJFHOo.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a0c3dcb91b2247eca56536be9ae8e8becc1c807b1136c5fe101552d6a155b24a",
  },
  // Easton
  {
    file: "voices/LMDIYE0Oa377Qh6Z.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "45c582673cb20edb04ddcca2b5a8865ddfb0a82599efadb269021e87d20fbdd4",
  },
  // Micah
  {
    file: "voices/40QrMVgAxFiUnP9f.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "0de1098d1051686a138d068e83ff09deb5053328bc74ec9541a1fdb90bfc24da",
  },
  // Trevor
  {
    file: "voices/0zLApsZfVLLNaEGl.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "dc1341c8937af5534e6bbcc5c0fc5e7c10312229b59061228074f93683fe2261",
  },
  // Russell
  {
    file: "voices/_6Aslh2DxfmnRLmP.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "beae9e4030efce0a935fcb0f807dc0784d598bcaa87ad4aacb047436b1eaa411",
  },
  // Preston
  {
    file: "voices/qIxlZ7Ei8KvWlyQH.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "6e04b84d3ae8751b22bca0a4f63936f9cb648f4f2182b2d0788071ea1c0b68a1",
  },
  // Miles
  {
    file: "voices/d9XCyhjo4JhkHF1d.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "eab20e4f159b48cfbc01b1885804e24f681f80e7a64f909182763a4e3361bee8",
  },
  // Maeve
  {
    file: "voices/6PWnV0Nq4wu7RVBT.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "56494ee67d533ad227ff0dfd0b1c13ff5aa91f1ad6e596e66c2986ba3005e844",
  },
  // Freya
  {
    file: "voices/GgfEkEJtxZR7gnpy.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "eb3cd47418349e618408c16bea7153988b3ad04b41a6033039dd474a9a4dcd23",
  },
  // Pippa
  {
    file: "voices/uem82D50GRv2Dwma.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "d41a8f01d6e33450f1ce062d332914da10beee99e585dab531750706f187c48a",
  },
  // Elodie-Rose
  {
    file: "voices/y9bQqIDnyxwqK01t.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "21eade9d83b2e66b9364e1b6cd999a3a6704c000a71f27f34e05ace5cdfd58bb",
  },
  // Imogen
  {
    file: "voices/gDT1nz7Ie36ZhL-C.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "1980ab3c35fa98a25b5f3b5e643fd852e14ca8a12a4a26b06aaf1abf2a8c68b1",
  },
  // Lottie
  {
    file: "voices/m9MxpY6IUGXtWp76.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a906ebf17e43fded12b3d6bd077abb2df33bc86eff51c1beeb82e5d416527db3",
  },
  // Alfie
  {
    file: "voices/X6NceqF285Gr04gb.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "4de75497fff3030168e8aa6dca23ffeb332b1bc8c25d0bf3507ceadac953ae9c",
  },
  // Hugh
  {
    file: "voices/YyKcxPG7iIgS3jsK.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "3169d74b47a54d1eb15e4c243f56393abe9fe90c7e1c308ecf96ee67618b152a",
  },
  // Edwin
  {
    file: "voices/zUCxzrOrugTSis03.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "ff1d0735b8b45710462b1d1f846da040f970c6eb62c7c2f36dbfb8e5bde74f1e",
  },
  // Spencer
  {
    file: "voices/JOW0vchyswkQUMDS.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "cc9ac1107b0ffce282374a19beb7bfa95847724e6b7afbac991b4ae4277d5e6d",
  },
  // Toby
  {
    file: "voices/dME3IWyZBvmh1n1q.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "09ab315c7f4b72b4d71e66cb756184cedfb1f14d222d096e115bf95bfe7b29dc",
  },
  // Teddy
  {
    file: "voices/Zn7g6NxLqXlJb7Qi.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "baea0ea9506f3237a5ebfc61afb7c72dbd189c5603b199ef42fbb00dbda379e0",
  },
  // Bryony
  {
    file: "voices/gfP44RlHwr2kEwk7.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "bcc207a73b7885e45b49ab633d3377a10158292e3a7cef9cd6764ad107f1cb9b",
  },
  // Captain Silas Blackwater
  {
    file: "voices/xPDpsV3F592srkjA.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a09f615b0084980ebc4076d71550bf73ccd8d4df076bbbee4a9beb523a1379d9",
  },
  // Morwenna of the Deep
  {
    file: "voices/nOykTmcH9ydsGGIc.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "2ff2db91e70648623df8d42c0dcd7c336b2f14cb4362d602a20f292a4d862154",
  },
  // Old Mortlake
  {
    file: "voices/lX3LavMNdlIbgEHH.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "d3f9429205e5906dcd3c78edaf7c9d5148c7e389e34c82c86f72f154adb5d923",
  },
  // Azgorath
  {
    file: "voices/dGyGDKo2osj7h3s7.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "83900e020c14946426614c6553c5a138ba6277e1aaa69832223bd0f0f8b73893",
  },
  // Fennick the Young Elf
  {
    file: "voices/cXoIXJ2llS8S0WOW.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a05944e4c69d61d10d66f689175ba0b081930df2d77b22f04d00ac40d3d69ecb",
  },
  // The Thing Under the Stairs
  {
    file: "voices/GXaSqMEmOg12OIYb.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "bfa37bac98aefbb9de80f792c47d01a0f9e3b94e73776169040c249c8b722759",
  },
  // Vyrmathrax the Ancient
  {
    file: "voices/xO8xQo3BrB8FuI6J.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "63ab4b15427844a804480a1603774262e8903dee33cb279e2749860dbfaedd10",
  },
  // Grub the Ogre
  {
    file: "voices/cYtsRSxV5q9IMehn.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "b30fa847686ec1eb6be8c5bea4b6a5c5076ea66c3ff053cf98210a155d8aad1e",
  },
  // Master Aldric
  {
    file: "voices/gSgG5Tor0GPEnqzC.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "a83430cc022b85c83753f859f9e44f406651646dab994fced8ef7a9a88005971",
  },
  // The Snow Queen
  {
    file: "voices/NIWoEurJHMJjyUTe.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "bc8a8fcb95bb4f32d2d4376aa69d81b186b3ba2f8e9b6cb6a403b2b194590e85",
  },
  // Sir Bramwell
  {
    file: "voices/y2eiKxtQo2B67deU.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "b3cea6789aeb58822af16ba08c4e5105d7febb840dcb67b47f426c19d1a8f209",
  },
  // Fairy Godmother
  {
    file: "voices/QXNr0SvkukLB4wPq.safetensors",
    repo: "gradium/phonon-voices",
    revision: "0ee17205f254b5abc43b266c8a0f215565c352f7",
    size: 256136,
    sha256: "f390c09f5e279c0f2171995a534b92f5ca5c83443906879da68fc27445f89d89",
  },
];

async function digest(path) {
  const hash = createHash("sha256");
  await pipeline(createReadStream(path), hash);
  return hash.digest("hex");
}

/** Whether `path` holds exactly the pinned file. */
export async function verified(model, directory = MODEL_DIR) {
  const path = join(directory, model.file);
  if (!existsSync(path) || statSync(path).size !== model.size) return false;
  return (await digest(path)) === model.sha256;
}

async function fetchModel(model, directory) {
  const url = `https://huggingface.co/${model.repo}/resolve/${model.revision}/${model.file}`;
  const partial = join(directory, `${model.file}.partial`);
  // A voice sits in a folder of its own beside the model.
  mkdirSync(dirname(partial), { recursive: true });
  rmSync(partial, { force: true });
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) throw new Error(`${model.file}: HTTP ${response.status}`);
  await pipeline(Readable.fromWeb(response.body), createWriteStream(partial));
  const size = statSync(partial).size;
  const sha256 = await digest(partial);
  if (size !== model.size || sha256 !== model.sha256) {
    rmSync(partial, { force: true });
    throw new Error(`${model.file}: got ${size} bytes, sha256 ${sha256}; expected the pinned file`);
  }
  renameSync(partial, join(directory, model.file));
}

export async function ensureModels({
  check = false,
  directory = MODEL_DIR,
  speechDirectory = SPEECH_DIR,
} = {}) {
  for (const [models, folder] of [
    [MODELS, directory],
    [SPEECH, speechDirectory],
  ]) {
    mkdirSync(folder, { recursive: true });
    for (const model of models) {
      if (await verified(model, folder)) continue;
      if (check)
        throw new Error(
          `${model.file} is missing or not the pinned file. Run node scripts/dictation-models.mjs.`,
        );
      process.stdout.write(`Fetching ${model.file} (${(model.size / 1e6).toFixed(1)} MB)\n`);
      await fetchModel(model, folder);
    }
    // A model a later release replaced would ride along in the bundle, since the whole folder
    // is packaged: it is removed here, and a check fails on it.
    const pinned = new Set(models.map((model) => model.file));
    for (const name of readdirSync(folder)) {
      if (!name.endsWith(".gguf") || pinned.has(name)) continue;
      if (check)
        throw new Error(
          `${name} is not a pinned model and would be bundled. Run node scripts/dictation-models.mjs.`,
        );
      rmSync(join(folder, name));
      process.stdout.write(`Removed ${name}: no longer bundled\n`);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  ensureModels({ check: process.argv.includes("--check") }).then(
    () => process.stdout.write("Dictation and voice models verified.\n"),
    (error) => {
      process.stderr.write(`${error.message}\n`);
      process.exitCode = 1;
    },
  );
}
