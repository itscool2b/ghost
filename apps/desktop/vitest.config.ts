import { defineConfig } from "vitest/config";

// Node environment on purpose: the tests cover pure logic (the stream reducer, parsers, the frame
// meter, the scroll pinning) and server-rendered markup (`*.test.tsx`, through react-dom/server).
// Rendering is checked in the real WebView by the smoke run (GHOST_SMOKE=1).
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx", "scripts/**/*.test.ts"],
  },
});
