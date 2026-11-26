import { defineConfig } from "vitest/config";

// Node environment on purpose: everything under test here is pure logic or React rendered to a
// string, and a DOM shim would only pretend to run WebGL and layout. What runs them in real engines
// is the desktop app's smoke run (GHOST_SMOKE, .github/scripts/run-smoke.sh), whose stress burst
// draws live orbs: WebKit when run locally on a Mac, WebKitGTK and WebView2 in CI. It does not
// fail without WebGL2, though: the orbs fall back to their CSS posters.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
    // Vitest blanks CSS imports unless told otherwise; the token tests read stylesheets as text.
    css: { include: [/\.css\?raw$/] },
  },
});
