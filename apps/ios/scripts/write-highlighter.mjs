/**
 * Bundles highlight.js for the iPhone app exactly as the desktop uses it, and writes the tests'
 * expectations from Node running that same bundle:
 *   - apps/ios/GhostKit/Sources/GhostHighlight/Resources/highlight.js: one IIFE built by esbuild from
 *     the desktop's own apps/desktop/src/lib/highlight.ts (its languages, aliases and limit, never
 *     copied), exposing `globalThis.GhostHighlight`.
 *   - apps/ios/GhostKit/Tests/GhostHighlightTests/HighlightGoldens.generated.swift: sample code in seven
 *     languages, the HTML the bundle writes for each and the runs a strict span parser reads from it.
 *
 *   node apps/ios/scripts/write-highlighter.mjs          write both files
 *   node apps/ios/scripts/write-highlighter.mjs --check  fail when either is out of date (CI)
 */
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import vm from "node:vm";
import { emit, finish, loadEsbuild, repo, swiftHeader, swiftRawString } from "./lib.mjs";

const SCRIPT = "node apps/ios/scripts/write-highlighter.mjs";
const SOURCE = "apps/desktop/src/lib/highlight.ts";
const BUNDLE = repo("apps/ios/GhostKit/Sources/GhostHighlight/Resources/highlight.js");
const GOLDENS = repo(
  "apps/ios/GhostKit/Tests/GhostHighlightTests/HighlightGoldens.generated.swift",
);
const VERSION = "11.12.0";

const desktop = createRequire(repo("apps/desktop/package.json"));
const packagePath = desktop.resolve("highlight.js/package.json");
const { version } = JSON.parse(readFileSync(packagePath, "utf8"));
if (version !== VERSION) {
  throw new Error(
    `highlight.js is ${version} in node_modules; the desktop and this bundle pin ${VERSION}`,
  );
}
const license = readFileSync(join(dirname(packagePath), "LICENSE"), "utf8").trim();

const source = readFileSync(repo(SOURCE), "utf8");
const limit = /^const MAX_CHARS = ([\d_]+);$/m.exec(source)?.[1];
if (!limit) throw new Error(`${SOURCE}: no \`const MAX_CHARS = <number>;\``);
const maxChars = Number(limit.replaceAll("_", ""));

const esbuild = loadEsbuild();
const built = esbuild.buildSync({
  stdin: {
    contents: [
      'import { highlightCode, languageOf } from "./highlight.ts";',
      "globalThis.GhostHighlight = Object.freeze({",
      `  version: ${JSON.stringify(version)},`,
      `  maxChars: ${maxChars},`,
      "  languageOf,",
      "  highlight: highlightCode,",
      "});",
    ].join("\n"),
    resolveDir: repo("apps/desktop/src/lib"),
    sourcefile: "ghost-highlight-entry.js",
    loader: "js",
  },
  bundle: true,
  write: false,
  format: "iife",
  platform: "neutral",
  mainFields: ["module", "main"],
  conditions: ["import"],
  target: "es2022",
  minify: true,
  legalComments: "none",
  charset: "ascii",
  logLevel: "silent",
});
const code = built.outputFiles?.[0]?.text;
if (!code) throw new Error("esbuild wrote nothing");

const header = [
  "/*",
  ` * highlight.js ${version} for the iPhone app, bundled by \`${SCRIPT}\` with esbuild`,
  ` * ${esbuild.version} from ${SOURCE}: the desktop's own languages, aliases and`,
  ` * ${maxChars.toLocaleString("en-US")} character limit. Do not edit: run the script again (\`--check\` fails on drift).`,
  " *",
  " * highlight.js is BSD-3-Clause licensed:",
  " *",
  ...license.split("\n").map((line) => (line ? ` * ${line}` : " *")),
  " */",
  "",
].join("\n");
const bundle = `${header}${code}`;
if (bundle.includes(String.fromCharCode(0x2014))) throw new Error("the bundle holds an em dash");

// The samples: each language the tests check, with nesting, entities and non-ASCII text.
const SAMPLES = [
  {
    fence: "swift",
    code: `import Foundation

/// Greets everyone in the café.
struct Greeter<Name: StringProtocol> {
    let names: [Name]
    var isLoud = false

    func greet() -> String {
        let line = names.map { "Hello, \\($0)!" }.joined(separator: " & ")
        return isLoud ? line.uppercased() : line
    }
}

@MainActor
final class Counter {
    private(set) var count = 0x1F
    func bump(by step: Int = 1) { count += step } // it's fine
}

let greeter = Greeter(names: ["Zoë", "Ana"])
print(greeter.greet(), 3.14, count < 2 && count > 0)
`,
  },
  {
    fence: "ts",
    code: `import { readFile } from "node:fs/promises";

type Result<T> = { ok: true; value: T } | { ok: false; error: string };

interface Options {
  readonly retries?: number;
  label: string;
}

export async function load<T>(path: string, options: Options): Promise<Result<T>> {
  const attempts = options.retries ?? 3;
  for (let i = 0; i < attempts && i >= 0; i++) {
    try {
      const text = await readFile(path, "utf8");
      return { ok: true, value: JSON.parse(text) as T };
    } catch (error) {
      console.warn(\`attempt \${i + 1} failed: \${String(error)}\`);
    }
  }
  return { ok: false, error: \`couldn't read \${path} (café)\` };
}

class Cache<K, V> extends Map<K, V> {}
`,
  },
  {
    fence: "rust",
    code: `use std::collections::HashMap;

/// A tally of words, café included.
#[derive(Debug, Default)]
pub struct Tally<'a> {
    counts: HashMap<&'a str, usize>,
}

#[cfg(feature = "tally")]
impl<'a> Tally<'a> {
    pub fn add(&mut self, word: &'a str) -> usize {
        let entry = self.counts.entry(word).or_insert(0);
        *entry += 1;
        *entry
    }
}

fn main() {
    let mut tally = Tally::default();
    for word in "the cat & the hat".split_whitespace() {
        tally.add(word);
    }
    println!("{:?} <{}>", tally, 0xff_u8 as char);
}
`,
  },
  {
    fence: "python",
    code: `from dataclasses import dataclass, field
import re

PATTERN = re.compile(r"<(?P<tag>\\w+)>")


@dataclass
class Note:
    """A note with tags, like "café"."""

    title: str
    tags: list[str] = field(default_factory=list)

    def render(self) -> str:
        if not self.tags and self.title != "":
            return f"{self.title!r} & nothing else"
        return " ".join(f"<{tag}>" for tag in self.tags)


async def main(limit: int = 10) -> None:
    for i in range(limit):
        print(Note(title=f"n{i}", tags=["a", 'b']).render())  # it's 1.5e3 lines
`,
  },
  {
    fence: "sh",
    code: `#!/usr/bin/env bash
set -euo pipefail

# Build every target, café or not.
TARGETS=("app" "widget")
for target in "\${TARGETS[@]}"; do
  if [[ -n "\${CI:-}" && "$target" != 'widget' ]]; then
    echo "building $target" >&2
  fi
  make "$target" 2>/dev/null || exit 1
done

count=$(ls -1 build | wc -l)
echo "built \${count} files & done" > build/log.txt
`,
  },
  {
    fence: "json",
    code: `{
  "name": "Ghost",
  "version": "0.5.1",
  "private": true,
  "tags": ["café", "<remote>", "a & b"],
  "limits": { "chars": 100000, "ratio": 4.5, "enabled": null },
  "quote": "she said \\"hi\\" and it's fine",
  "mood": "ok \u{1F642}"
}
`,
  },
  {
    fence: "diff",
    // The diff grammar never nests one span in another.
    flat: true,
    code: `diff --git a/src/greet.ts b/src/greet.ts
index 3b18e51..a9c4f2d 100644
--- a/src/greet.ts
+++ b/src/greet.ts
@@ -1,7 +1,8 @@
 export function greet(name: string): string {
-  return "Hello, " + name;
+  const place = "café";
+  return \`Hello, \${name} & welcome to the \${place}\`;
 }

-// TODO: <remove>
+// It's done.
 export const VERSION = 2;
`,
  },
];

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', "#x27": "'", "#39": "'" };
const CLASS = /^[A-Za-z0-9_ -]+$/;

/** A span's class as a highlight.js scope: `hljs-title class_` is `title.class`. */
function scopeOf(className) {
  if (!CLASS.test(className)) throw new Error(`unexpected class ${className}`);
  const [first = "", ...rest] = className.split(" ");
  if (first.startsWith("language-") && rest.length === 0 && first.length > 9) {
    return `language:${first.slice(9)}`;
  }
  if (!first.startsWith("hljs-") || first.length === 5)
    throw new Error(`unexpected class ${className}`);
  const pieces = [first.slice(5)];
  rest.forEach((piece, index) => {
    const tail = "_".repeat(index + 1);
    if (!piece.endsWith(tail) || piece.length === tail.length) {
      throw new Error(`unexpected class ${className}`);
    }
    pieces.push(piece.slice(0, -tail.length));
  });
  return pieces.join(".");
}

/** The decoded text and its runs, read strictly from highlight.js's HTML. */
function parse(html) {
  let text = "";
  const stack = [];
  const runs = [];
  let open = null;
  const push = (piece) => {
    const scopes = [...stack];
    if (open && open.scopes.join("\n") === scopes.join("\n")) open.end += piece.length;
    else {
      open = { start: text.length, end: text.length + piece.length, scopes };
      runs.push(open);
    }
    text += piece;
  };
  let at = 0;
  while (at < html.length) {
    const char = html[at];
    if (char === "<") {
      if (html.startsWith("</span>", at)) {
        if (stack.length === 0) throw new Error(`stray </span> at ${at}`);
        stack.pop();
        at += 7;
        continue;
      }
      const tag = /^<span class="([^"]*)">/.exec(html.slice(at, at + 200));
      if (!tag) throw new Error(`unexpected markup at ${at}: ${html.slice(at, at + 20)}`);
      stack.push(scopeOf(tag[1] ?? ""));
      at += tag[0].length;
    } else if (char === "&") {
      const entity = /^&(#x27|#39|amp|lt|gt|quot);/.exec(html.slice(at, at + 8));
      if (!entity) throw new Error(`unexpected entity at ${at}: ${html.slice(at, at + 10)}`);
      push(ENTITIES[entity[1] ?? ""]);
      at += entity[0].length;
    } else {
      const next = html.slice(at).search(/[<&]/);
      const piece = next < 0 ? html.slice(at) : html.slice(at, at + next);
      push(piece);
      at += piece.length;
    }
  }
  if (stack.length > 0) throw new Error("unclosed span");
  return { text, runs };
}

const sandbox = { console: { log() {}, warn() {}, error() {}, info() {}, debug() {} } };
vm.runInNewContext(bundle, sandbox, { filename: "highlight.js" });
const api = sandbox.GhostHighlight;
if (!api || api.version !== version || api.maxChars !== maxChars) {
  throw new Error("the bundle does not expose GhostHighlight as expected");
}

const samples = SAMPLES.map(({ fence, code: sample, flat = false }) => {
  const language = api.languageOf(fence);
  const html = api.highlight(sample, fence);
  if (!language || typeof html !== "string") throw new Error(`${fence}: not highlighted`);
  const { text, runs } = parse(html);
  if (text !== sample) throw new Error(`${fence}: the decoded text differs from the code`);
  if (!flat && !runs.some((run) => run.scopes.length > 1)) throw new Error(`${fence}: no nesting`);
  for (const run of runs) {
    const high = text.charCodeAt(run.end - 1);
    if (high >= 0xd800 && high <= 0xdbff)
      throw new Error(`${fence}: a run splits a surrogate pair`);
  }
  return { fence, language, code: sample, html, runs };
});

/** JSON with every non-ASCII character escaped, so the Swift file stays ASCII. */
function asciiJson(value) {
  return JSON.stringify(value, null, 2).replace(
    /[\u007f-￿]/g,
    (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
}

const goldens = `${swiftHeader(SCRIPT, `${SOURCE} through the bundle it writes`, `${SCRIPT} --check`)}
/// What Node running the bundled highlight.js writes for each sample, and the runs a strict span
/// parser reads from it. The tests decode this and compare JavaScriptCore and the Swift parser.
enum HighlightGoldens {
    static let json = ${swiftRawString(asciiJson({ version, maxChars, samples }))}
}
`;

emit(BUNDLE, bundle);
emit(GOLDENS, goldens);
finish(SCRIPT);
