import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { reactRouter } from "@react-router/dev/vite";
import { defineConfig } from "vite";
import { noticesPlugin } from "../../.github/scripts/third-party-notices.mjs";
import { changelogPlugin } from "./scripts/changelog.ts";
import { siteImagesPlugin } from "./scripts/site-images.ts";

const app = fileURLToPath(new URL(".", import.meta.url));
const stage = process.env.GHOST_RELEASE_STAGE;
if (stage) {
  // Release logic stays in Rust. Build tooling only selects an already prepared local artifact set.
  const verified = spawnSync(
    process.env.CARGO ?? "cargo",
    ["run", "--locked", "-p", "ghost-release", "--", "verify", resolve(stage)],
    { cwd: resolve(app, "../.."), stdio: "inherit", windowsHide: true },
  );
  if (verified.error || verified.status !== 0) {
    throw new Error("The release stage failed offline verification. Website build stopped.");
  }
}
export default defineConfig({
  plugins: [reactRouter(), noticesPlugin("web"), siteImagesPlugin(), changelogPlugin()],
  resolve: {
    alias: {
      "#release-catalog": stage
        ? resolve(stage, "downloads.json")
        : resolve(app, "app/release-catalog.json"),
    },
  },
  build: { target: "safari16.4" },
});
