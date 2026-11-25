// Installs ts-hooks.mjs in this process, once. Either preload it
// (`node --import ./apps/ios/scripts/register-ts.mjs script.mts`) or import it first and load the
// TypeScript sources with `await import()` after it, since static imports resolve before any code
// runs.
import { registerHooks } from "node:module";
import { load, resolve } from "./ts-hooks.mjs";

const KEY = Symbol.for("o7.ios.ts-hooks");
if (!globalThis[KEY]) {
  globalThis[KEY] = true;
  registerHooks({ resolve, load });
}
