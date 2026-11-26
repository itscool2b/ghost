import assert from "node:assert/strict";
import test from "node:test";
import {
  noticesPlugin,
  renderHtml,
  renderText,
  runtimePackages,
  verifiedCatalogue,
} from "./third-party-notices.mjs";

test("native inventory follows only normal runtime edges, through workspace crates", () => {
  const pkg = (id, source = "registry", kind = "lib") => ({
    id,
    name: id,
    source,
    targets: [{ kind: [kind] }],
  });
  const dep = (name, kind = null) => ({ pkg: name, dep_kinds: [{ kind }] });
  const metadata = {
    packages: [
      pkg("app", null),
      pkg("workspace", null),
      pkg("runtime"),
      pkg("build"),
      pkg("test"),
      pkg("macro", "registry", "proc-macro"),
      pkg("macro-helper"),
    ],
    resolve: {
      nodes: [
        {
          id: "app",
          deps: [dep("workspace"), dep("build", "build"), dep("test", "dev"), dep("macro")],
        },
        { id: "workspace", deps: [dep("runtime")] },
        { id: "runtime", deps: [] },
        { id: "macro", deps: [dep("macro-helper")] },
      ],
    },
  };
  assert.deepEqual(
    runtimePackages(metadata, "app").map((p) => p.name),
    ["runtime"],
  );
});

test("locked catalogue includes complete font notices and explicit native sources without build tools", () => {
  const catalogue = verifiedCatalogue();
  for (const name of [
    "@fontsource-variable/geist",
    "@fontsource-variable/geist-mono",
    "@fontsource/instrument-serif",
  ]) {
    const record = catalogue.records.find((r) => r.name === name);
    assert.equal(record.license, "OFL-1.1");
    assert.match(catalogue.texts[record.files[0].sha256], /SIL OPEN FONT LICENSE/);
    assert.match(catalogue.texts[record.files[0].sha256], /Copyright/i);
  }
  assert.deepEqual(
    catalogue.records.filter((r) => r.ecosystem === "native").map((r) => r.name),
    [
      "ggml (transcribe-cpp-sys source)",
      "miniz (transcribe-cpp-sys source)",
      "Silero VAD segmentation code (transcribe-cpp-sys source)",
    ],
  );
  for (const name of [
    "@axe-core/playwright",
    "@biomejs/biome",
    "sharp",
    "@img/sharp-win32-x64",
    "tauri-build",
    "cc",
  ])
    assert.ok(!catalogue.records.some((r) => r.name === name), name);
});

test("the bundled speech models keep their licenses, conversion notices and attribution", () => {
  const catalogue = verifiedCatalogue();
  const models = catalogue.records.filter((r) => r.ecosystem === "model");
  assert.deepEqual(
    models.map((r) => [r.name, r.license, r.distributions]),
    [
      [
        "Parakeet TDT-CTC 110M speech model (parakeet-tdt_ctc-110m-Q6_K.gguf)",
        "CC-BY-4.0",
        ["desktop"],
      ],
      [
        "Parakeet Ultra speech model (parakeet-ultra-Q4_K_M.gguf, fetched on request)",
        "CC-BY-4.0",
        ["desktop"],
      ],
      [
        "Phonon 31M English voice model (model.q8.gguf, with its config.json, tokenizer.json and default-voice.safetensors)",
        "Apache-2.0",
        ["desktop"],
      ],
      ["Phonon English voices (37 voices, voices/*.safetensors)", "Apache-2.0", ["desktop"]],
      ["Silero VAD v6.2 voice activity model (silero-vad-v6.2-F32.gguf)", "MIT", ["desktop"]],
    ],
  );
  // The licence texts and attribution each record carries, as the notices print them.
  const texts = (record) =>
    [record.attribution ?? "", ...record.files.map((entry) => catalogue.texts[entry.sha256])].join(
      "\n",
    );
  const [parakeet, ultra, phonon, voices, silero] = models;
  assert.match(texts(parakeet), /Attribution 4\.0 International/);
  assert.match(texts(parakeet), /Modifications: handy-computer converted/);
  // The larger model is not carried, only fetched when asked for: it is credited all the same.
  assert.match(texts(ultra), /Attribution 4\.0 International/);
  assert.match(texts(ultra), /moondream\/parakeet-ultra at commit 73175eb/);
  assert.match(texts(silero), /Copyright \(c\) 2020-present Silero Team/);
  assert.match(texts(silero), /Modifications: handy-computer converted/);
  // Read aloud's voice model and its voices: under Apache 2.0.
  for (const voice of [phonon, voices]) assert.match(texts(voice), /Apache License/);
});

test("every desktop npm record names its license, and elkjs stays out of what ships", () => {
  const catalogue = verifiedCatalogue();
  const npm = catalogue.records.filter((r) => r.ecosystem === "npm");
  assert.deepEqual(
    npm.filter((r) => typeof r.license !== "string" || !r.license).map((r) => r.name),
    [],
  );
  assert.equal(npm.find((r) => r.name === "khroma")?.license, "MIT");
  assert.ok(!catalogue.records.some((r) => r.name === "elkjs"));
  assert.ok(!npm.some((r) => /GPL|EPL/.test(r.license)), "no copyleft npm package ships");
});

test("both outputs say the agent marks are their owners' trademarks and point at their sources", () => {
  for (const distribution of ["web", "desktop"]) {
    const text = renderText(verifiedCatalogue(), distribution);
    assert.match(text, /Agent marks: .*trademarks of their respective owners/);
    assert.match(text, /third-party\/brand-marks\.md/);
  }
});

test("the desktop output says where the start screen's aircraft pictures come from; the website, which has none, does not", () => {
  const desktop = renderText(verifiedCatalogue(), "desktop");
  assert.match(desktop, /Start screen aircraft: .*supplied by o7's owner/);
  assert.doesNotMatch(desktop, /Wikimedia|public-domain/);
  assert.match(desktop, /third-party\/ascii-art\.md/);
  assert.doesNotMatch(renderText(verifiedCatalogue(), "web"), /Start screen aircraft/);
});

test("website output contains frontend notices, not desktop native dependencies", () => {
  const text = renderText(verifiedCatalogue(), "web");
  assert.match(text, /@fontsource-variable\/geist/);
  assert.doesNotMatch(text, /@fontsource-variable\/fraunces/);
  assert.match(text, /react-router/);
  assert.doesNotMatch(text, /libsqlite3-sys|tauri-plugin|@xterm|transcribe-cpp|Parakeet/);
});

test("packaging emits readable plain text and escaped script-free HTML", () => {
  const assets = [];
  noticesPlugin("desktop").generateBundle.call({ emitFile: (asset) => assets.push(asset) });
  assert.deepEqual(
    assets.map((asset) => asset.fileName),
    ["third-party-notices.txt", "third-party-notices/index.html"],
  );
  assert.match(assets[0].source, /transcribe-cpp-sys/);
  assert.match(assets[1].source, /color-scheme:light dark/);
  const html = renderHtml('<script>alert("bad")</script>&');
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /&lt;script&gt;/);
  assert.match(html, /color-scheme:light dark/);
  assert.match(html, /@media\(prefers-color-scheme:dark\)/);
});

test("desktop imports the notice text as a virtual module without fetching", () => {
  const plugin = noticesPlugin("desktop");
  const id = plugin.resolveId("virtual:third-party-notices");
  assert.equal(plugin.resolveId("react"), null);
  assert.equal(plugin.load("react"), null);
  const source = plugin.load(id);
  assert.match(source, /^export default "Third-party software notices/);
  assert.match(source, /transcribe-cpp-sys/);
});

test("the website's notices are always dark and the desktop's follow System", () => {
  const assets = [];
  noticesPlugin("web").generateBundle.call({ emitFile: (asset) => assets.push(asset) });
  const html = assets.find((asset) => asset.fileName.endsWith("index.html")).source;
  // The website's plain page: near-black, as the site is, whatever the system asks for.
  assert.match(html, /color-scheme:dark;/);
  assert.match(html, /--notice-bg:#0a0a0a/);
  assert.doesNotMatch(html, /prefers-color-scheme/);
  assert.doesNotMatch(html, /<script/);
  // The desktop keeps Graphite's surfaces, light and dark.
  const desktop = renderHtml("Desktop");
  assert.match(desktop, /color-scheme:light dark;/);
  assert.match(desktop, /--notice-bg:#fafafa/);
  assert.match(desktop, /@media\(prefers-color-scheme:dark\)\{html\{--notice-bg:#101112/);
});
