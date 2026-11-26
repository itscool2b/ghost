import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { noticesPlugin } from "../../.github/scripts/third-party-notices.mjs";

// The Tauri CLI sets these when it runs the dev server (beforeDevCommand) or a build
// (beforeBuildCommand). A plain `vite build` (CI's frontend step) sees neither.
const platform = process.env.TAURI_ENV_PLATFORM;
const debug = process.env.TAURI_ENV_DEBUG === "true";
// Set by `tauri dev` for mobile targets only; desktop serves on localhost.
const devHost = process.env.TAURI_DEV_HOST;

/**
 * The oldest WebKit the build supports: WKWebView on macOS and WebKitGTK on Linux. Safari 16.4 is
 * the WebKit that macOS 13.3 ships with, which is why `bundle.macOS.minimumSystemVersion` in
 * tauri.conf.json is 13.3. The two move together (build-targets.test.ts checks it): a newer target
 * than the oldest allowed macOS ships leaves that system rendering the window unstyled. The
 * stylesheets themselves need Safari 15.4 (cascade layers, `:has()`, dynamic viewport units).
 */
export const WEBKIT_TARGET = "safari16.4";

/** WebView2 is evergreen (it updates itself on Windows 10 and 11), so a recent Chromium is safe. */
export const WEBVIEW2_TARGET = "chrome130";

export default defineConfig({
  plugins: [react(), noticesPlugin("desktop")],
  resolve: {
    // Mermaid's optional ELK layout needs elkjs, which is EPL-2.0 or GPL-3.0: Ghost ships a stub
    // in its place (src/lib/elkStub.ts), and the third-party notices leave elkjs out.
    alias: [
      {
        find: /^elkjs\/lib\/elk\.bundled\.js$/,
        replacement: fileURLToPath(new URL("./src/lib/elkStub.ts", import.meta.url)),
      },
    ],
  },
  // Keep Rust's compile errors on screen instead of clearing them.
  clearScreen: false,
  server: {
    // tauri.conf.json's devUrl points here, so the port cannot drift.
    port: 1420,
    strictPort: true,
    host: devHost || false,
    hmr: devHost ? { protocol: "ws", host: devHost, port: 1421 } : undefined,
    watch: {
      // Cargo writes target/ and gen/ constantly while it builds.
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // The engines the app actually runs in: WebView2 (Chromium) on Windows, WKWebView on macOS and
    // WebKitGTK on Linux, each at the oldest version the app allows (see above).
    target: platform === "windows" ? WEBVIEW2_TARGET : WEBKIT_TARGET,
    sourcemap: debug,
    minify: debug ? false : "oxc",
  },
});
