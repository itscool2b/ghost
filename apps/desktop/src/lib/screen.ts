/**
 * A live picture in a pane: the decoder that turns a stream of packets into frames on a canvas,
 * and the mapping from a point of the pane to a point of the picture. The Device pane shows a
 * phone's video with it and the Game pane a game's frames (docs/mobile.md, docs/game.md).
 */

/** The kinds of packet a device's video is sent as (`ghost_devices::manager::Viewer`). */
const PACKET_SIZE = 1;
const PACKET_CONFIG = 2;
const PACKET_KEY = 3;
const PACKET_FRAME = 4;
const PACKET_ENDED = 5;
/** A whole picture as a JPEG: an iPhone simulator's screen. */
const PACKET_PICTURE = 6;

/**
 * The WebCodecs codec string for an H.264 configuration in Annex B form: `avc1.` and the three
 * bytes after the sequence parameter set's header (profile, constraints, level). `null` when
 * the bytes hold no such set.
 */
export function avcCodec(config: Uint8Array): string | null {
  for (let at = 0; at + 4 < config.length; at += 1) {
    const three = config[at] === 0 && config[at + 1] === 0 && config[at + 2] === 1;
    if (!three) continue;
    const header = config[at + 3];
    // A sequence parameter set is unit type 7.
    if (header !== undefined && (header & 0x1f) === 7 && at + 6 < config.length) {
      const hex = (value: number | undefined) => (value ?? 0).toString(16).padStart(2, "0");
      return `avc1.${hex(config[at + 4])}${hex(config[at + 5])}${hex(config[at + 6])}`;
    }
  }
  return null;
}

/** What the screen is doing, for the pane to say. */
export type ScreenState =
  | { readonly kind: "waiting" }
  | { readonly kind: "live"; readonly width: number; readonly height: number }
  | { readonly kind: "ended" }
  /** This system's webview cannot decode the video. */
  | { readonly kind: "unsupported" };

/**
 * Draws a device's video on a canvas. Packets go in as they arrive; frames come out on the canvas
 * as soon as they are decoded. A configuration packet starts the decoder again, and the key frame
 * after it carries the configuration in front of it, as Annex B decoding needs. An iPhone's screen
 * arrives as whole pictures instead, which every web view can draw.
 */
export class ScreenPlayer {
  readonly #canvas: HTMLCanvasElement;
  readonly #onState: (state: ScreenState) => void;
  #decoder: VideoDecoder | null = null;
  #config: Uint8Array | null = null;
  #configured = false;
  #size: { width: number; height: number } | null = null;
  #closed = false;
  /** A picture is being drawn; the newest one that came meanwhile waits here. */
  #drawing = false;
  #waiting: Uint8Array | null = null;

  constructor(canvas: HTMLCanvasElement, onState: (state: ScreenState) => void) {
    this.#canvas = canvas;
    this.#onState = onState;
    onState({ kind: "waiting" });
  }

  /** The video's size now, in its own pixels: what a touch's coordinates are measured in. */
  size(): { width: number; height: number } | null {
    return this.#size;
  }

  push(packet: Uint8Array): void {
    if (this.#closed || packet.length === 0) return;
    const kind = packet[0];
    const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
    if (kind === PACKET_SIZE && packet.length >= 5) {
      this.#size = { width: view.getUint16(1), height: view.getUint16(3) };
      return;
    }
    if (kind === PACKET_ENDED) {
      this.#onState({ kind: "ended" });
      return;
    }
    if (kind === PACKET_PICTURE) {
      this.#picture(packet.subarray(1));
      return;
    }
    // The rest is H.264, which needs the web view's video decoder.
    if (typeof VideoDecoder === "undefined") {
      if (kind === PACKET_CONFIG || kind === PACKET_KEY) this.#onState({ kind: "unsupported" });
      return;
    }
    if (kind === PACKET_CONFIG) {
      this.#config = packet.slice(1);
      this.#configured = false;
      return;
    }
    if ((kind !== PACKET_KEY && kind !== PACKET_FRAME) || packet.length < 9) return;
    const key = kind === PACKET_KEY;
    const timestamp = Number(view.getBigUint64(1));
    let data = packet.subarray(9);
    if (key && !this.#configured) {
      if (!this.#config || !this.#start(this.#config)) return;
      // The first key frame after a configuration carries it.
      const joined = new Uint8Array(this.#config.length + data.length);
      joined.set(this.#config, 0);
      joined.set(data, this.#config.length);
      data = joined;
      this.#configured = true;
    }
    if (!this.#configured || !this.#decoder || this.#decoder.state !== "configured") return;
    try {
      this.#decoder.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp, data }));
    } catch {
      // A frame the decoder refuses: wait for the next key frame.
      this.#configured = false;
    }
  }

  #start(config: Uint8Array): boolean {
    const codec = avcCodec(config);
    if (!codec) return false;
    this.#decoder?.close();
    const decoder = new VideoDecoder({
      output: (frame) => this.#draw(frame),
      error: () => {
        // The stream starts again from its next configuration and key frame.
        this.#configured = false;
      },
    });
    try {
      decoder.configure({ codec, optimizeForLatency: true });
    } catch {
      this.#onState({ kind: "unsupported" });
      return false;
    }
    this.#decoder = decoder;
    return true;
  }

  #draw(frame: VideoFrame): void {
    if (this.#closed) {
      frame.close();
      return;
    }
    const { displayWidth: width, displayHeight: height } = frame;
    if (this.#canvas.width !== width || this.#canvas.height !== height) {
      this.#canvas.width = width;
      this.#canvas.height = height;
    }
    this.#canvas.getContext("2d")?.drawImage(frame, 0, 0, width, height);
    frame.close();
    this.#onState({ kind: "live", width, height });
  }

  /** Draws one JPEG. Pictures can come faster than a slow machine draws them: only the newest
   * one waits, and the rest are dropped. */
  #picture(jpeg: Uint8Array): void {
    if (this.#drawing) {
      this.#waiting = jpeg;
      return;
    }
    this.#drawing = true;
    createImageBitmap(new Blob([jpeg.slice()], { type: "image/jpeg" }))
      .then(
        (bitmap) => {
          if (this.#closed) {
            bitmap.close();
            return;
          }
          const { width, height } = bitmap;
          if (this.#canvas.width !== width || this.#canvas.height !== height) {
            this.#canvas.width = width;
            this.#canvas.height = height;
          }
          this.#canvas.getContext("2d")?.drawImage(bitmap, 0, 0, width, height);
          bitmap.close();
          this.#size = { width, height };
          this.#onState({ kind: "live", width, height });
        },
        () => {
          // A picture that would not decode: the next one replaces it.
        },
      )
      .finally(() => {
        this.#drawing = false;
        const next = this.#waiting;
        this.#waiting = null;
        if (next && !this.#closed) this.#picture(next);
      });
  }

  close(): void {
    this.#closed = true;
    if (this.#decoder && this.#decoder.state !== "closed") this.#decoder.close();
    this.#decoder = null;
  }
}

/**
 * A point of the pane's canvas as a point of the video: the canvas shows the whole video, scaled
 * to fit, so the two differ only by that scale. `null` outside the picture.
 */
export function videoPoint(
  clientX: number,
  clientY: number,
  box: { left: number; top: number; width: number; height: number },
  video: { width: number; height: number },
): { x: number; y: number } | null {
  if (box.width <= 0 || box.height <= 0) return null;
  const x = ((clientX - box.left) / box.width) * video.width;
  const y = ((clientY - box.top) / box.height) * video.height;
  if (x < 0 || y < 0 || x >= video.width || y >= video.height) return null;
  return { x: Math.floor(x), y: Math.floor(y) };
}
