// Build tooling only. Read locked package metadata and preserve upstream license texts.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const root = fileURLToPath(new URL("../../", import.meta.url));
export const targets = [
  "aarch64-apple-darwin",
  "aarch64-pc-windows-msvc",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
];
const cataloguePath = join(root, "third-party/catalogue.json");
const digest = (value) => createHash("sha256").update(value).digest("hex");
const json = (file) => JSON.parse(readFileSync(file, "utf8").replace(/^\uFEFF/, ""));
const sort = (values) => values.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));

export function runtimePackages(metadata, packageName) {
  const packages = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const start = metadata.packages.find((p) => p.name === packageName);
  if (!start) throw new Error(`Missing runtime root ${packageName}`);
  const found = new Set();
  const todo = [start.id];
  while (todo.length) {
    const id = todo.pop();
    if (found.has(id)) continue;
    found.add(id);
    for (const dependency of nodes.get(id)?.deps ?? []) {
      const pkg = packages.get(dependency.pkg);
      if (
        dependency.dep_kinds.some((kind) => kind.kind === null) &&
        !pkg.targets.every((target) => target.kind.includes("proc-macro"))
      )
        todo.push(pkg.id);
    }
  }
  return [...found].map((id) => packages.get(id)).filter((p) => p.source);
}

function inputFiles() {
  const files = [
    "Cargo.lock",
    "Cargo.toml",
    "pnpm-lock.yaml",
    "third-party/overrides.json",
    "third-party/models.json",
    "apps/desktop/scripts/dictation-models.mjs",
  ];
  for (const folder of ["apps", "crates", "packages"]) {
    for (const entry of readdirSync(join(root, folder), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      for (const file of ["Cargo.toml", "package.json", "src-tauri/Cargo.toml"]) {
        const name = `${folder}/${entry.name}/${file}`;
        if (existsSync(join(root, name))) files.push(name);
      }
    }
  }
  return Object.fromEntries(
    sort(files).map((file) => [file, digest(readFileSync(join(root, file)))]),
  );
}

function missingLicense(pkg) {
  throw new Error(
    `${pkg.name}@${pkg.version} names no license: review its license file and add it to NPM_LICENSES`,
  );
}

function licenseFiles(directory) {
  return sort(readdirSync(directory)).filter(
    (name) =>
      /^(?:licen[cs]e|notice|copying)(?:[._-]|$)/i.test(name) &&
      statSync(join(directory, name)).isFile(),
  );
}

/**
 * npm packages whose package.json names no license, with the license their own license file
 * states (reviewed). Any other package without one stops the refresh until it is reviewed.
 */
export const NPM_LICENSES = { "khroma@2.1.0": "MIT" };

/**
 * npm packages in a dependency's metadata that o7 does not ship, each with why (reviewed),
 * and what keeps it out. elkjs is EPL-2.0 or GPL-3.0; Mermaid imports it only for its optional
 * ELK layout, which apps/desktop/vite.config.ts replaces with a stub.
 */
export const NPM_EXCLUDED = {
  elkjs: {
    reason: "EPL-2.0 or GPL-3.0; Mermaid's optional ELK layout, replaced by a stub",
    guard: ["apps/desktop/vite.config.ts", "elkStub.ts"],
  },
};

export function collect() {
  const overrides = json(join(root, "third-party/overrides.json"));
  const records = new Map();
  const texts = {};
  function file(text, source) {
    if (!text.trim() || Buffer.byteLength(text) > 1024 * 1024)
      throw new Error(`Invalid license: ${source}`);
    const hash = digest(text);
    texts[hash] = text;
    return { sha256: hash, source };
  }
  function add(record) {
    const key = `${record.ecosystem}/${record.name}/${record.version}`;
    const existing = records.get(key);
    if (existing) {
      existing.distributions = sort([
        ...new Set([...existing.distributions, ...record.distributions]),
      ]);
      existing.targets = sort([...new Set([...existing.targets, ...record.targets])]);
      return;
    }
    if (!record.files.length) throw new Error(`No reviewed license text for ${key}`);
    records.set(key, record);
  }
  function installedFiles(directory, source) {
    return licenseFiles(directory).map((name) =>
      file(readFileSync(join(directory, name), "utf8"), `${source}#${name}`),
    );
  }
  function npm(distribution, directory, seen = new Set()) {
    directory = realpathSync(directory);
    if (seen.has(directory)) return;
    seen.add(directory);
    const pkg = json(join(directory, "package.json"));
    const excluded = NPM_EXCLUDED[pkg.name];
    if (excluded) {
      const [guardFile, marker] = excluded.guard;
      if (!readFileSync(join(root, guardFile), "utf8").includes(marker))
        throw new Error(
          `${pkg.name} is left out of the notices, but ${guardFile} no longer keeps it out`,
        );
      return;
    }
    if (!pkg.private) {
      const source = `https://registry.npmjs.org/${pkg.name}/-/${pkg.name.split("/").at(-1)}-${pkg.version}.tgz`;
      const files = installedFiles(directory, source);
      if (pkg.name === "ogl" && pkg.version === "1.0.11" && !files.length) {
        const readme = readFileSync(join(directory, "README.md"), "utf8");
        const marker = "## Unlicense";
        if (!readme.includes(marker))
          throw new Error("OGL no longer includes its reviewed Unlicense section");
        files.push(file(readme.slice(readme.indexOf(marker)), `${source}#README.md-Unlicense`));
      }
      add({
        ecosystem: "npm",
        name: pkg.name,
        version: pkg.version,
        license: pkg.license ?? NPM_LICENSES[`${pkg.name}@${pkg.version}`] ?? missingLicense(pkg),
        source,
        distributions: [distribution],
        targets: [],
        files,
      });
    }
    for (const name of sort(Object.keys(pkg.dependencies ?? {}))) {
      let parent = directory;
      let dependency;
      while (parent !== dirname(parent)) {
        const candidate = join(parent, "node_modules", name);
        if (existsSync(join(candidate, "package.json"))) {
          dependency = candidate;
          break;
        }
        parent = dirname(parent);
      }
      if (!dependency)
        throw new Error(`Install the locked dependency ${name} before refreshing notices`);
      npm(distribution, dependency, seen);
    }
  }
  npm("desktop", join(root, "apps/desktop"));
  npm("web", join(root, "apps/web"));
  for (const target of targets) {
    const metadata = JSON.parse(
      execFileSync(
        process.env.CARGO ?? "cargo",
        ["metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target],
        { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, windowsHide: true },
      ),
    );
    for (const pkg of runtimePackages(metadata, "o7-desktop")) {
      const directory = dirname(pkg.manifest_path);
      const source = `https://crates.io/api/v1/crates/${pkg.name}/${pkg.version}/download`;
      let files = installedFiles(directory, source);
      const override = overrides[`cargo/${pkg.name}/${pkg.version}`];
      if (!files.length && override) {
        const vcs = json(join(directory, ".cargo_vcs_info.json"));
        if (vcs.git.sha1 !== override.revision)
          throw new Error(`Upstream revision changed for ${pkg.name}`);
        files = override.files.map((entry) => {
          const text = readFileSync(join(root, "third-party", entry.file), "utf8");
          if (digest(text) !== entry.sha256)
            throw new Error(`Upstream notice changed for ${pkg.name}`);
          return file(text, entry.source);
        });
      }
      add({
        ecosystem: "cargo",
        name: pkg.name,
        version: pkg.version,
        license: pkg.license,
        source,
        distributions: ["desktop"],
        targets: [target],
        files,
      });
      if (pkg.name === "transcribe-cpp-sys") {
        // transcribe.cpp vendors and statically links these; its own MIT text is the crate's LICENSE.
        for (const [name, path] of [
          ["ggml", "ggml/LICENSE"],
          ["miniz", "src/third_party/miniz/LICENSE"],
          ["Silero VAD segmentation code", "src/third_party/silero_vad/LICENSE"],
        ])
          add({
            ecosystem: "native",
            name: `${name} (transcribe-cpp-sys source)`,
            version: pkg.version,
            license: "MIT",
            source: `${source}#${dirname(path)}`,
            distributions: ["desktop"],
            targets: [target],
            files: [file(readFileSync(join(directory, path), "utf8"), `${source}#${path}`)],
          });
      }
    }
  }
  // The speech models bundled as app resources (apps/desktop/scripts/dictation-models.mjs): model
  // weights, not code. Each keeps its license, its converter's NOTICE and an attribution.
  const models = json(join(root, "third-party/models.json"));
  const pins = readFileSync(join(root, "apps/desktop/scripts/dictation-models.mjs"), "utf8");
  const pinned = [...pins.matchAll(/file: "([^"]+)"/g)].map((match) => match[1]);
  // A record answers for the file it is named for and for the files it says it covers: a model's
  // own settings beside its weights (`covers`), or every file under a folder (`covers_prefix`,
  // the voices, which share one license and one source).
  const answered = Object.entries(models).flatMap(([name, model]) => [
    ...(model.covers_prefix ? [] : [name]),
    ...(model.covers ?? []),
    ...(model.covers_prefix ? pinned.filter((entry) => entry.startsWith(model.covers_prefix)) : []),
  ]);
  if (
    new Set(answered).size !== answered.length ||
    JSON.stringify(sort([...pinned])) !== JSON.stringify(sort([...answered]))
  )
    throw new Error("The bundled models changed: review third-party/models.json");
  for (const [name, model] of Object.entries(models)) {
    if (!pins.includes(`revision: "${model.revision}"`))
      throw new Error(`The pinned revision changed for ${name}: review third-party/models.json`);
    const source = `https://huggingface.co/${model.repo}/tree/${model.revision}`;
    add({
      ecosystem: "model",
      name: model.name,
      version: model.revision,
      license: model.license,
      source,
      distributions: ["desktop"],
      targets: [...targets],
      files: [
        file(model.attribution, `${source}#attribution`),
        ...model.files.map((entry) => {
          const text = readFileSync(
            join(root, "third-party/upstream", `${entry.sha256}.txt`),
            "utf8",
          );
          if (digest(text) !== entry.sha256) throw new Error(`Upstream notice changed for ${name}`);
          return file(
            text,
            entry.url ??
              `https://huggingface.co/${model.repo}/blob/${model.revision}/${entry.name}`,
          );
        }),
      ],
    });
  }
  // The scrcpy server bundled in the desktop app (crates/o7-devices): Genymobile's prebuilt
  // program that runs on a person's Android device to show its screen and take input.
  const scrcpy = json(join(root, "third-party/scrcpy.json"));
  if (digest(readFileSync(join(root, scrcpy.file))) !== scrcpy.sha256)
    throw new Error("The bundled scrcpy server changed: review third-party/scrcpy.json");
  const scrcpyLicense = readFileSync(
    join(root, "third-party/upstream", `${scrcpy.license_sha256}.txt`),
    "utf8",
  );
  if (digest(scrcpyLicense) !== scrcpy.license_sha256)
    throw new Error("Upstream license changed for the scrcpy server");
  add({
    ecosystem: "bundled",
    name: scrcpy.name,
    version: scrcpy.version,
    license: scrcpy.license,
    source: `https://github.com/Genymobile/scrcpy/releases/tag/v${scrcpy.version}`,
    distributions: ["desktop"],
    targets: [...targets],
    files: [
      file(scrcpyLicense, `https://github.com/Genymobile/scrcpy/blob/v${scrcpy.version}/LICENSE`),
    ],
  });
  return {
    version: 1,
    scope:
      "Locked production dependency closure, excluding dev/build dependencies and procedural macro executables. transcribe.cpp's vendored sources, the bundled dictation models and the bundled scrcpy server are explicitly included. Linker-level and installer SDK qualification is separate.",
    inputs: inputFiles(),
    records: [...records.values()].sort((a, b) =>
      `${a.ecosystem}/${a.name}/${a.version}`.localeCompare(
        `${b.ecosystem}/${b.name}/${b.version}`,
        "en",
      ),
    ),
    texts: Object.fromEntries(sort(Object.keys(texts)).map((key) => [key, texts[key]])),
  };
}

export function verifiedCatalogue() {
  const catalogue = json(cataloguePath);
  catalogue.texts = json(join(root, "third-party/texts.json"));
  if (JSON.stringify(catalogue.inputs) !== JSON.stringify(inputFiles()))
    throw new Error(
      "Third-party notices are stale. Run node .github/scripts/third-party-notices.mjs --write after reviewing dependency changes.",
    );
  for (const [hash, text] of Object.entries(catalogue.texts))
    if (digest(text) !== hash) throw new Error("Third-party license text integrity failed");
  for (const record of catalogue.records)
    for (const entry of record.files)
      if (!catalogue.texts[entry.sha256])
        throw new Error(`Missing third-party license text for ${record.name}`);
  return catalogue;
}

export function renderText(catalogue, distribution) {
  const records = catalogue.records.filter((record) => record.distributions.includes(distribution));
  const lines = [
    "Third-party software notices",
    "",
    "These notices apply to the listed third-party components. They do not grant a license to o7 itself.",
    "",
    "Component source archives are linked below, including the unmodified source for MPL-covered components.",
    "",
    "Agent marks: the logos shown beside the Claude Code (the Claude spark), Codex (the OpenAI Blossom), Pi, Antigravity, Cursor, Grok Build (the Grok mark), OpenCode, oh-my-pi and Hermes agents, and the model makers' marks (Anthropic, OpenAI, Google Gemini, xAI, DeepSeek, Mistral AI, Meta, Qwen and Moonshot AI), and the marks of the assistants that can be linked as bots (Grok Bot, Muse, Hermes and OpenClaw), and the GitHub mark beside the website's link to the repository, are trademarks of their respective owners. They only identify which agent a chat uses, whose model is working, which assistant a link is for or where the source is, imply no endorsement or partnership, and are not covered by these notices or by any license for o7. Their sources, guidelines and terms are listed in third-party/brand-marks.md.",
    "",
    ...(distribution === "desktop"
      ? [
          "Start screen aircraft: the pictures of aircraft the desktop app opens on were supplied by o7's owner. They are not covered by these notices or by any license for o7. The files and how they were prepared are listed in third-party/ascii-art.md.",
          "",
        ]
      : []),
  ];
  const used = new Map();
  for (const record of records) {
    lines.push(
      `${record.name} ${record.version} (${record.ecosystem})`,
      `License: ${record.license}`,
      `Source: ${record.source}`,
    );
    if (record.targets.length) lines.push(`Targets: ${record.targets.join(", ")}`);
    for (const entry of record.files) {
      if (!used.has(entry.sha256)) used.set(entry.sha256, used.size + 1);
      lines.push(`License text ${used.get(entry.sha256)}: ${entry.source}`);
    }
    lines.push("");
  }
  for (const [hash, number] of used)
    lines.push(
      `================ LICENSE TEXT ${number} ================`,
      "",
      catalogue.texts[hash],
      "",
    );
  return lines.join("\n");
}

export function renderHtml(text, distribution = "desktop") {
  const safe = text.replace(
    /[&<>"']/g,
    (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char],
  );
  // Each document wears its own surface's colours: the website's page is always dark, as the
  // site is (owner, 25 November 2026), and the desktop's follows System in Graphite's. Both
  // remain script-free and leave the notice text unchanged.
  const web = distribution === "web";
  const scheme = web ? "dark" : "light dark";
  const light = web
    ? "--notice-bg:#0a0a0a;--notice-fg:#8f8f8f;--notice-title:#f2f2f2;--notice-line:#1c1c1c;--notice-selection:#333"
    : "--notice-bg:#fafafa;--notice-fg:#52525b;--notice-title:#18181b;--notice-line:#e4e4e7;--notice-selection:#dde2ea";
  const dark = web
    ? ""
    : "@media(prefers-color-scheme:dark){html{--notice-bg:#101112;--notice-fg:#a1a1aa;--notice-title:#fafafa;--notice-line:#303236;--notice-selection:#333}}";
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="${scheme}"><title>Third-party notices | o7</title><style>html{color-scheme:${scheme};${light};background:var(--notice-bg);color:var(--notice-fg);font:14px/1.6 ui-sans-serif,system-ui,sans-serif;-webkit-font-smoothing:antialiased}${dark}body{max-width:72rem;margin:clamp(2rem,6vw,6rem) auto;padding:0 1.25rem}h1{margin:0 0 2rem;color:var(--notice-title);font-size:2rem;line-height:1.2;font-weight:500;letter-spacing:-0.03em}pre{margin:0;padding-top:2rem;border-top:1px solid var(--notice-line);white-space:pre-wrap;overflow-wrap:anywhere;font:13px/1.65 ui-monospace,Menlo,Consolas,monospace}::selection{background:var(--notice-selection);color:var(--notice-title)}</style><body><h1>Third-party notices</h1><pre>${safe}</pre></body></html>`;
}

const virtualNotices = "virtual:third-party-notices";

export function noticesPlugin(distribution) {
  return {
    name: "auracode-third-party-notices",
    // The desktop CSP has no same-origin connect-src, so the app lazily imports the same text
    // as a script chunk instead of fetching the emitted file.
    resolveId(id) {
      return id === virtualNotices ? `\0${virtualNotices}` : null;
    },
    load(id) {
      if (id !== `\0${virtualNotices}`) return null;
      return `export default ${JSON.stringify(renderText(verifiedCatalogue(), distribution))};`;
    },
    generateBundle() {
      const text = renderText(verifiedCatalogue(), distribution);
      this.emitFile({ type: "asset", fileName: "third-party-notices.txt", source: text });
      this.emitFile({
        type: "asset",
        // A directory index gives the website Worker a clean route. It serves HTML only as
        // prerendered index pages.
        fileName: "third-party-notices/index.html",
        source: renderHtml(text, distribution),
      });
    },
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] === "--write") {
    const { texts, ...catalogue } = collect();
    const textsPath = join(root, "third-party/texts.json");
    writeFileSync(cataloguePath, `${JSON.stringify(catalogue, null, 2)}\n`);
    writeFileSync(textsPath, `${JSON.stringify(texts, null, 2)}\n`);
    execFileSync(
      process.execPath,
      [
        join(root, "node_modules/@biomejs/biome/bin/biome"),
        "format",
        "--write",
        cataloguePath,
        textsPath,
      ],
      { cwd: root, stdio: "pipe", windowsHide: true },
    );
    console.log("Updated third-party notice catalogue from locked packages.");
  } else {
    const catalogue = verifiedCatalogue();
    console.log(
      `Verified ${catalogue.records.length} third-party runtime records and their license texts.`,
    );
  }
}
