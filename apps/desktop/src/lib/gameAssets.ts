/**
 * A game's pictures, sounds and models as the pane's Assets view reads them (docs/game.md,
 * "Assets"): what the list says of each, how a sprite sheet is cut into frames, a sound's
 * shape, and the bytes of each asset kept for as long as something shows them.
 */
import type { AssetKind, GameAsset, GameAssetData } from "../bindings";

/** `run.png` of `art/player/run.png`. */
export function assetName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** `art/player` of `art/player/run.png`; nothing for a file at the top. */
export function assetFolder(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut < 0 ? "" : path.slice(0, cut);
}

/** A file's size as a person reads it. */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export const KINDS: readonly { readonly kind: AssetKind; readonly many: string }[] = [
  { kind: "picture", many: "Pictures" },
  { kind: "sound", many: "Sounds" },
  { kind: "model", many: "Models" },
];

/**
 * Makers of art and sound that publish tools an agent can use, each with the page where the
 * maker itself says how to connect them. Read from those pages on 28 November 2026: Meshy's
 * (an npm server, `@meshy-ai/meshy-mcp-server`, with the person's own key), ElevenLabs' (a
 * hosted server the person signs in to) and PixelLab's. Ghost adds none of them itself: each
 * needs an account of the person's own with the maker.
 */
export const MAKERS: readonly {
  readonly name: string;
  readonly makes: string;
  readonly guide: string;
}[] = [
  {
    name: "Meshy",
    makes: "3D models, with textures, rigs and animation",
    guide: "https://docs.meshy.ai/api/ai",
  },
  { name: "PixelLab", makes: "Pixel art", guide: "https://www.pixellab.ai/vibe-coding" },
  {
    name: "ElevenLabs",
    makes: "Sound effects, music and voices",
    guide: "https://elevenlabs.io/docs/eleven-api/resources/mcp-server",
  },
];

/** How many assets of each kind a list holds. */
export function counts(assets: readonly GameAsset[]): Readonly<Record<AssetKind, number>> {
  const found: Record<AssetKind, number> = { picture: 0, sound: 0, model: 0 };
  for (const asset of assets) found[asset.kind] += 1;
  return found;
}

/** The assets of one kind (or every kind) whose path holds every word of what was typed. */
export function matching(
  assets: readonly GameAsset[],
  kind: AssetKind | null,
  typed: string,
): readonly GameAsset[] {
  const words = typed.toLowerCase().split(/\s+/).filter(Boolean);
  return assets.filter((asset) => {
    if (kind !== null && asset.kind !== kind) return false;
    const path = asset.path.toLowerCase();
    return words.every((word) => path.includes(word));
  });
}

/**
 * How a picture is most likely cut into frames: a strip of squares across or down is a sprite
 * sheet of that many frames; anything else is one picture. A guess the person can change.
 */
export function guessFrames(
  width: number,
  height: number,
): { readonly across: number; readonly down: number } {
  if (width > 0 && height > 0) {
    const across = width / height;
    if (Number.isInteger(across) && across >= 2 && across <= 64) return { across, down: 1 };
    const down = height / width;
    if (Number.isInteger(down) && down >= 2 && down <= 64) return { across: 1, down };
  }
  return { across: 1, down: 1 };
}

/** A count of frames a person typed, kept to what a sheet can hold. */
export function frameCount(typed: string | number): number {
  const value = Math.round(Number(typed));
  return Number.isFinite(value) ? Math.min(64, Math.max(1, value)) : 1;
}

/** Where frame `index` of a sheet is, reading across and then down. */
export function frameRect(
  index: number,
  across: number,
  down: number,
  width: number,
  height: number,
): { readonly x: number; readonly y: number; readonly width: number; readonly height: number } {
  const [w, h] = [width / across, height / down];
  const at = ((index % (across * down)) + across * down) % (across * down);
  return { x: (at % across) * w, y: Math.floor(at / across) * h, width: w, height: h };
}

/**
 * A sound's shape: for each of `buckets` slices of its samples, the quietest and the loudest,
 * each from -1 to 1.
 */
export function peaks(
  samples: ArrayLike<number>,
  buckets: number,
): readonly (readonly [number, number])[] {
  const out: [number, number][] = [];
  const count = Math.max(1, Math.floor(buckets));
  if (samples.length === 0) return Array.from({ length: count }, () => [0, 0] as const);
  const per = samples.length / count;
  for (let bucket = 0; bucket < count; bucket += 1) {
    const from = Math.floor(bucket * per);
    const to = Math.max(from + 1, Math.min(samples.length, Math.floor((bucket + 1) * per)));
    let [low, high] = [1, -1];
    for (let at = from; at < to; at += 1) {
      const value = samples[at] ?? 0;
      if (value < low) low = value;
      if (value > high) high = value;
    }
    out.push([Math.max(-1, low), Math.min(1, high)]);
  }
  return out;
}

/** A length of time as `0:07` or `1:23`. */
export function clock(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/** How long a sound is: a short one to the hundredth of a second, a long one by the clock. */
export function soundLength(seconds: number): string {
  if (!(seconds >= 0)) return "";
  return seconds < 10 ? `${Math.round(seconds * 100) / 100} s` : clock(seconds);
}

/** What changed between two lists of a folder: the assets that are new, or changed since. */
export function freshSince(
  before: ReadonlyMap<string, number>,
  now: readonly GameAsset[],
): readonly GameAsset[] {
  return now.filter((asset) => before.get(asset.path) !== asset.modified);
}

interface Kept {
  readonly promise: Promise<{
    readonly url: string;
    readonly bytes: Uint8Array;
    readonly mime: string;
  }>;
  url: string | null;
  holders: number;
  used: number;
}

/**
 * The bytes of the assets a view shows, each read once and kept as an object URL while
 * something shows it. What nothing shows any more is let go, the longest unused first, once
 * more than a few dozen are kept: a folder of a thousand pictures is not held in memory.
 */
export class AssetBytes {
  readonly #read: (path: string) => Promise<GameAssetData>;
  readonly #kept = new Map<string, Kept>();
  readonly #most: number;
  #tick = 0;

  constructor(read: (path: string) => Promise<GameAssetData>, most = 48) {
    this.#read = read;
    this.#most = most;
  }

  /** The asset's bytes, and a call that says its user is done with them. */
  acquire(asset: Pick<GameAsset, "path" | "modified">): {
    readonly promise: Kept["promise"];
    readonly release: () => void;
  } {
    // A file that changed is another thing to show.
    const key = `${asset.modified}:${asset.path}`;
    let kept = this.#kept.get(key);
    if (!kept) {
      const made: Kept = {
        promise: this.#read(asset.path).then(async (data) => {
          const bytes = Uint8Array.from(atob(data.base64), (char) => char.charCodeAt(0));
          const url = URL.createObjectURL(new Blob([bytes], { type: data.mime }));
          made.url = url;
          return { url, bytes, mime: data.mime };
        }),
        url: null,
        holders: 0,
        used: 0,
      };
      // A read that failed is asked again the next time.
      made.promise.catch(() => {
        if (this.#kept.get(key) === made) this.#kept.delete(key);
      });
      kept = made;
      this.#kept.set(key, kept);
    }
    const mine = kept;
    mine.holders += 1;
    this.#tick += 1;
    mine.used = this.#tick;
    let released = false;
    return {
      promise: mine.promise,
      release: () => {
        if (released) return;
        released = true;
        mine.holders -= 1;
        this.#trim();
      },
    };
  }

  #trim(): void {
    if (this.#kept.size <= this.#most) return;
    const idle = [...this.#kept.entries()]
      .filter(([, kept]) => kept.holders <= 0)
      .sort(([, a], [, b]) => a.used - b.used);
    for (const [key, kept] of idle) {
      if (this.#kept.size <= this.#most) break;
      if (kept.url) URL.revokeObjectURL(kept.url);
      this.#kept.delete(key);
    }
  }

  /** How many assets are kept now. */
  size(): number {
    return this.#kept.size;
  }

  /** The view is gone: everything is let go. */
  clear(): void {
    for (const kept of this.#kept.values()) if (kept.url) URL.revokeObjectURL(kept.url);
    this.#kept.clear();
  }
}
