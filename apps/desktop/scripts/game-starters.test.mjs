// The web starters Ghost writes for "Start a game" (crates/ghost-game/resources/starters): the
// small server of the plain canvas one serves its own folder to this computer and nothing else,
// and the files parse as what they are.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { request } from "node:http";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const starters = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../crates/ghost-game/resources/starters",
);

/** A port nothing listens on now. */
const freePort = () =>
  new Promise((done, fail) => {
    const probe = createServer();
    probe.once("error", fail);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => done(port));
    });
  });

/** One request to the server, with the Host header as given: its status, type and body. */
const ask = (port, path, { host = `localhost:${port}`, method = "GET" } = {}) =>
  new Promise((done, fail) => {
    const call = request(
      { host: "127.0.0.1", port, path, method, headers: { host }, setHost: false },
      (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (chunk) => {
          body += chunk;
        });
        response.on("end", () =>
          done({ status: response.statusCode, type: response.headers["content-type"], body }),
        );
      },
    );
    call.once("error", fail);
    call.end();
  });

test("the canvas starter's server serves its folder to this computer and nothing else", async () => {
  const folder = await mkdtemp(join(tmpdir(), "ghost-starter-"));
  await cp(join(starters, "web-2d"), folder, { recursive: true });
  // What a project folder holds beside the game, and must not be handed to a browser.
  await mkdir(join(folder, ".git"));
  await writeFile(join(folder, ".git", "config"), "[core]\n");
  await writeFile(join(folder, ".env"), "NOT=served\n");
  await writeFile(join(folder, "..", "ghost-starter-outside.txt"), "outside\n");
  await mkdir(join(folder, "art"));
  await writeFile(join(folder, "art", "ship.png"), "png");
  // A link inside the folder that leads out of it, where this system lets one be made.
  const linked = await symlink(
    join(folder, "..", "ghost-starter-outside.txt"),
    join(folder, "art", "link.txt"),
  ).then(
    () => true,
    () => false,
  );
  const port = await freePort();
  const server = spawn(process.execPath, [join(folder, "serve.mjs")], {
    env: { ...process.env, PORT: String(port) },
    stdio: ["ignore", "pipe", "inherit"],
  });
  try {
    const said = await new Promise((done, fail) => {
      server.stdout.once("data", (chunk) => done(String(chunk).trim()));
      server.once("exit", (code) => fail(new Error(`the server stopped (${code})`)));
    });
    assert.equal(said, `http://localhost:${port}`);

    const page = await ask(port, "/");
    assert.equal(page.status, 200);
    assert.match(page.type, /^text\/html/);
    assert.match(page.body, /<canvas id="game"/);
    const script = await ask(port, "/game.js");
    assert.equal(script.status, 200);
    assert.match(script.type, /^text\/javascript/);
    assert.equal((await ask(port, "/", { host: `127.0.0.1:${port}` })).status, 200);
    assert.equal((await ask(port, "/", { method: "HEAD" })).body, "");

    // A page elsewhere that points a name of its own at this computer is refused.
    for (const host of ["evil.example", `evil.example:${port}`, `localhost:${port + 1}`, ""]) {
      const refused = await ask(port, "/game.js", { host });
      assert.equal(refused.status, 403, `Host: ${host}`);
      assert.doesNotMatch(refused.body, /canvas|requestAnimationFrame/);
    }
    // A file in a folder of the game's own is served.
    assert.equal((await ask(port, "/art/ship.png")).status, 200);
    // Hidden files, files outside the folder, and what is not there, however the address
    // spells them: with a backslash, in another case, by a Windows short name, through a link.
    for (const path of [
      "/.env",
      "/.git/config",
      "/sub/.hidden/x",
      "/art%5C..%5C.env",
      "/%5C.env",
      "/art/..%5C.git%5Cconfig",
      "/.GIT/config",
      "/GIT~1/config",
      "/ENV~1",
      "/../ghost-starter-outside.txt",
      "/%2e%2e/ghost-starter-outside.txt",
      "/art/%2e%2e/%2e%2e/ghost-starter-outside.txt",
      "/art/..%5C..%5Cghost-starter-outside.txt",
      "/art",
      "/game.js%00.png",
    ]) {
      const answer = await ask(port, path);
      assert.ok([400, 403, 404].includes(answer.status), `${path}: ${answer.status}`);
      assert.doesNotMatch(answer.body, /outside|NOT=served|\[core\]/, path);
    }
    if (linked) {
      const through = await ask(port, "/art/link.txt");
      assert.equal(through.status, 404, "a link that leads out of the folder");
      assert.doesNotMatch(through.body, /outside/);
    }
    assert.equal((await ask(port, "/missing.png")).status, 404);
    assert.equal((await ask(port, "/%E0%A4%A")).status, 400);
    assert.equal((await ask(port, "/game.js", { method: "POST" })).status, 405);
  } finally {
    server.kill();
    await rm(folder, { recursive: true, force: true });
    await rm(join(folder, "..", "ghost-starter-outside.txt"), { force: true });
  }
});

test("every web starter's package file is valid once its name is filled in", async () => {
  for (const starter of ["web-2d", "web-3d"]) {
    const text = (await readFile(join(starters, starter, "package.json"), "utf8")).replaceAll(
      "__PACKAGE__",
      "moon-hop",
    );
    const parsed = JSON.parse(text);
    assert.equal(parsed.name, "moon-hop");
    assert.equal(parsed.private, true, `${starter} is never published by accident`);
    assert.ok(parsed.keywords.includes("game"), `${starter} says it is a game`);
    assert.match(parsed.scripts.dev, /serve\.mjs|vite/);
  }
});
