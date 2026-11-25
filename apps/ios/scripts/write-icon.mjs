// Writes the iPhone app icon, apps/ios/O7/Resources/Assets.xcassets/AppIcon.appiconset, from the
// wing in packages/ui/src/graphite/logoPaths.ts, rasterised as the desktop's icons are (the Tauri
// CLI, as apps/desktop/scripts/app-icons.mjs runs it):
//   AppIcon.png         1024 square and full bleed (iOS masks the corners): the wing in #ededed on
//                       black, as wide against the tile as on the desktop icon (640 of its 824px
//                       body), with no alpha channel, as the App Store requires.
//   AppIcon-Dark.png    the same.
//   AppIcon-Tinted.png  the wing in white on transparency: iOS tints it and draws the background.
//
//   node apps/ios/scripts/write-icon.mjs          write the catalog
//   node apps/ios/scripts/write-icon.mjs --check  fail when it is out of date (CI)
//
// The check renders again and compares pixels, allowing 2 of 255 per channel, so a rasteriser
// rounding an edge differently on another machine is not drift.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { crc32, deflateSync, inflateSync } from "node:zlib";
import { LOGO_PATH, LOGO_VIEWBOX } from "../../../packages/ui/src/graphite/logoPaths.ts";
import { emit, finish, repo } from "./lib.mjs";

const SCRIPT = "node apps/ios/scripts/write-icon.mjs";
const CATALOG = repo("apps/ios/O7/Resources/Assets.xcassets");
const ICONSET = join(CATALOG, "AppIcon.appiconset");
const SIZE = 1024;
/** The desktop's macOS icon draws the wing 640px wide on an 824px tile. */
const WING = (SIZE * 640) / 824;
const TOLERANCE = 2;

const [vw, vh] = LOGO_VIEWBOX;

/** The wing `width` wide, centred on the canvas, as app-icons.mjs places it. */
function wing(fill) {
  const scale = WING / Math.max(vw, vh);
  const x = (SIZE - vw * scale) / 2;
  const y = (SIZE - vh * scale) / 2;
  const r = (n) => Math.round(n * 1000) / 1000;
  return `<path transform="translate(${r(x)} ${r(y)}) scale(${r(scale)})" fill="${fill}" fill-rule="evenodd" d="${LOGO_PATH}"/>`;
}

const svg = (body) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${SIZE}" height="${SIZE}" viewBox="0 0 ${SIZE} ${SIZE}">\n${body}\n</svg>\n`;

const SOURCES = {
  opaque: svg(`  <rect width="${SIZE}" height="${SIZE}" fill="#000000"/>\n  ${wing("#ededed")}`),
  tinted: svg(`  ${wing("#ffffff")}`),
};

/** Rasterises an SVG with the desktop's Tauri CLI, as RGBA pixels. */
function rasterise(text, work, name) {
  const source = join(work, `${name}.svg`);
  const out = join(work, name);
  writeFileSync(source, text);
  const tauri = repo("apps/desktop/node_modules/@tauri-apps/cli/tauri.js");
  execFileSync(process.execPath, [tauri, "icon", source, "--output", out, "--png", String(SIZE)], {
    cwd: repo("apps/desktop"),
    stdio: ["ignore", "ignore", "inherit"],
  });
  return decode(readFileSync(join(out, `${SIZE}x${SIZE}.png`)));
}

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const CHANNELS = { 0: 1, 2: 3, 4: 2, 6: 4 };

/** Reads an 8-bit, non-interlaced PNG as RGBA pixels. */
function decode(png) {
  if (!png.subarray(0, 8).equals(SIGNATURE)) throw new Error("not a PNG");
  let at = 8;
  let header = null;
  const data = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at);
    const type = png.toString("latin1", at + 4, at + 8);
    const body = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") header = body;
    else if (type === "IDAT") data.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  if (!header) throw new Error("PNG without a header");
  const width = header.readUInt32BE(0);
  const height = header.readUInt32BE(4);
  const [depth, color, , , interlace] = header.subarray(8, 13);
  const channels = CHANNELS[color];
  if (depth !== 8 || channels === undefined || interlace !== 0) {
    throw new Error(
      `unsupported PNG: depth ${depth}, colour type ${color}, interlace ${interlace}`,
    );
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * channels;
  const rows = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? rows[y * stride + x - channels] : 0;
      const up = y > 0 ? rows[(y - 1) * stride + x] : 0;
      const corner = x >= channels && y > 0 ? rows[(y - 1) * stride + x - channels] : 0;
      let value = line[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - corner;
        const [pa, pb, pc] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - corner)];
        value += pa <= pb && pa <= pc ? left : pb <= pc ? up : corner;
      } else if (filter !== 0) throw new Error(`bad PNG filter ${filter}`);
      rows[y * stride + x] = value & 255;
    }
  }
  const rgba = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const s = rows.subarray(i * channels, (i + 1) * channels);
    const [r, g, b, a] =
      channels === 1 ? [s[0], s[0], s[0], 255] : channels === 2 ? [s[0], s[0], s[0], s[1]] : [...s];
    rgba.set([r, g, b, channels === 3 ? 255 : a], i * 4);
  }
  return { width, height, rgba };
}

function chunk(type, body) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(body.length, 0);
  head.write(type, 4, "latin1");
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])), 0);
  return Buffer.concat([head, body, sum]);
}

/** Writes RGBA pixels as an 8-bit PNG: RGB when `alpha` is false, RGBA otherwise. */
function encode({ width, height, rgba }, alpha) {
  const channels = alpha ? 4 : 3;
  const stride = width * channels;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    // Each row filtered by Sub: flat runs compress to almost nothing.
    raw[y * (stride + 1)] = 1;
    for (let x = 0; x < width; x++) {
      for (let c = 0; c < channels; c++) {
        const value = rgba[(y * width + x) * 4 + c];
        const left = x > 0 ? rgba[(y * width + x - 1) * 4 + c] : 0;
        raw[y * (stride + 1) + 1 + x * channels + c] = (value - left) & 255;
      }
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, alpha ? 6 : 2, 0, 0, 0], 8);
  return Buffer.concat([
    SIGNATURE,
    chunk("IHDR", header),
    // sRGB, perceptual: the colours are the tokens' own.
    chunk("sRGB", Buffer.from([0])),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Whether two PNGs hold the same picture, within the tolerance. */
function samePicture(a, b) {
  try {
    const [x, y] = [decode(a), decode(b)];
    if (x.width !== y.width || x.height !== y.height) return false;
    for (let i = 0; i < x.rgba.length; i++) {
      if (Math.abs(x.rgba[i] - y.rgba[i]) > TOLERANCE) return false;
    }
    return true;
  } catch {
    return false;
  }
}

/** Asset catalog JSON as Xcode writes it: sorted keys, `" : "`, two-space indents. */
function xcodeJson(value, indent = "") {
  const inner = `${indent}  `;
  if (Array.isArray(value)) {
    return `[\n${value.map((item) => `${inner}${xcodeJson(item, inner)}`).join(",\n")}\n${indent}]`;
  }
  if (value !== null && typeof value === "object") {
    const keys = Object.keys(value).sort();
    return `{\n${keys.map((key) => `${inner}${JSON.stringify(key)} : ${xcodeJson(value[key], inner)}`).join(",\n")}\n${indent}}`;
  }
  return JSON.stringify(value);
}

const INFO = { author: "xcode", version: 1 };
const image = (filename, appearance) => ({
  ...(appearance ? { appearances: [{ appearance: "luminosity", value: appearance }] } : {}),
  filename,
  idiom: "universal",
  platform: "ios",
  size: `${SIZE}x${SIZE}`,
});

const work = mkdtempSync(join(tmpdir(), "o7-ios-icon-"));
try {
  const opaque = rasterise(SOURCES.opaque, work, "opaque");
  for (let i = 3; i < opaque.rgba.length; i += 4) {
    if (opaque.rgba[i] !== 255) throw new Error("the opaque icon has a transparent pixel");
  }
  const tinted = rasterise(SOURCES.tinted, work, "tinted");
  const png = encode(opaque, false);
  emit(join(ICONSET, "AppIcon.png"), png, samePicture);
  emit(join(ICONSET, "AppIcon-Dark.png"), png, samePicture);
  emit(join(ICONSET, "AppIcon-Tinted.png"), encode(tinted, true), samePicture);
  emit(
    join(ICONSET, "Contents.json"),
    `${xcodeJson({
      images: [
        image("AppIcon.png"),
        image("AppIcon-Dark.png", "dark"),
        image("AppIcon-Tinted.png", "tinted"),
      ],
      info: INFO,
    })}\n`,
  );
  emit(join(CATALOG, "Contents.json"), `${xcodeJson({ info: INFO })}\n`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
finish(SCRIPT);
