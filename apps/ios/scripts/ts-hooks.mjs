// Module hooks that let Node 26 run packages/ui's TypeScript as Vite does (register-ts.mjs installs
// them). Node strips the types itself; these add the rest:
//   - relative imports without an extension resolve to .ts, .tsx or an index file;
//   - `?raw` imports load a file's text as the module's default export;
//   - .tsx files are compiled by esbuild (Node does not read JSX), with React's automatic runtime.
import { existsSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadEsbuild } from "./lib.mjs";

const RAW = "?raw";
const CANDIDATES = [".ts", ".tsx", "/index.ts", "/index.tsx"];

function isFile(url) {
  const path = fileURLToPath(url);
  return existsSync(path) && statSync(path).isFile();
}

export function resolve(specifier, context, nextResolve) {
  if (specifier.endsWith(RAW)) {
    const url = new URL(specifier.slice(0, -RAW.length), context.parentURL);
    if (!isFile(url))
      throw new Error(`Cannot find ${specifier} imported from ${context.parentURL}`);
    return { url: `${url.href}${RAW}`, format: "module", shortCircuit: true };
  }
  try {
    return nextResolve(specifier, context);
  } catch (error) {
    const relative = specifier.startsWith("./") || specifier.startsWith("../");
    if (error?.code !== "ERR_MODULE_NOT_FOUND" || !relative || !context.parentURL) throw error;
    for (const ending of CANDIDATES) {
      const url = new URL(`${specifier}${ending}`, context.parentURL);
      if (isFile(url)) return { url: url.href, shortCircuit: true };
    }
    throw error;
  }
}

let esbuild = null;

export function load(url, context, nextLoad) {
  if (url.endsWith(RAW)) {
    const text = readFileSync(fileURLToPath(url.slice(0, -RAW.length)), "utf8");
    return {
      format: "module",
      source: `export default ${JSON.stringify(text)};`,
      shortCircuit: true,
    };
  }
  if (url.startsWith("file:") && url.endsWith(".tsx")) {
    const path = fileURLToPath(url);
    esbuild ??= loadEsbuild();
    const { code } = esbuild.transformSync(readFileSync(path, "utf8"), {
      loader: "tsx",
      jsx: "automatic",
      format: "esm",
      target: "es2022",
      sourcefile: path,
    });
    return { format: "module", source: code, shortCircuit: true };
  }
  return nextLoad(url, context);
}
