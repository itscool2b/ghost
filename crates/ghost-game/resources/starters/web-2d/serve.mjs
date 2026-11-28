// Serves this folder to this computer only, for playing the game while it is built:
//   npm run dev      then open the address it prints
import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL(".", import.meta.url)));
const port = Number(process.env.PORT) || 5173;
// The names this server answers to. A page on another site that points its own name at this
// computer is not one of them, so it cannot read these files through a browser.
const hosts = new Set([`localhost:${port}`, `127.0.0.1:${port}`, `[::1]:${port}`]);
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".wav": "audio/wav",
  ".mp3": "audio/mpeg",
  ".ogg": "audio/ogg",
  ".glb": "model/gltf-binary",
  ".wasm": "application/wasm",
};

const refuse = (response, status, text) => {
  response.writeHead(status, { "content-type": "text/plain; charset=utf-8" });
  response.end(text);
};

createServer((request, response) => {
  if (!hosts.has(String(request.headers.host ?? "").toLowerCase())) {
    refuse(response, 403, "Forbidden");
    return;
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    refuse(response, 405, "Method not allowed");
    return;
  }
  let path;
  try {
    path = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname);
  } catch {
    refuse(response, 400, "Bad request");
    return;
  }
  // Hidden files and folders (.git, .env) are not part of the game.
  if (path.split("/").some((part) => part.startsWith("."))) {
    refuse(response, 404, "Not found");
    return;
  }
  const file = normalize(join(root, path.endsWith("/") ? `${path}index.html` : path));
  // Nothing outside this folder is served.
  if (!file.startsWith(root + sep)) {
    refuse(response, 403, "Forbidden");
    return;
  }
  try {
    if (!statSync(file).isFile()) throw new Error("not a file");
  } catch {
    refuse(response, 404, "Not found");
    return;
  }
  response.writeHead(200, {
    "content-type": types[extname(file).toLowerCase()] ?? "application/octet-stream",
    // The game's files change while it is built: never serve a stale one.
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  if (request.method === "HEAD") {
    response.end();
    return;
  }
  createReadStream(file).pipe(response);
}).listen(port, "127.0.0.1", () => {
  console.log(`http://localhost:${port}`);
});
