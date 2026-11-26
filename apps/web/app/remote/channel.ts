/**
 * The browser's half of a remote session's sealed channel. `crates/ghost-remote/src/crypto.rs` is
 * the desktop's half and says what it guarantees; both are held to
 * `crates/ghost-remote/tests/vectors.json`.
 *
 * Everything here is WebCrypto. A device's key is imported as a key that cannot be read back out,
 * so a script on this origin can use a stored key but never copy it.
 */

const TO_HOST = "auracode remote v1 client to host";
const TO_CLIENT = "auracode remote v1 host to client";
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

/** Unpadded base64url, the encoding of every id, key and sealed message. */
export function encode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

/** The bytes of unpadded base64url text, or `null` when it is anything else. */
export function decode(text: string, length?: number): Uint8Array<ArrayBuffer> | null {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) return null;
  let binary: string;
  try {
    binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
  } catch {
    return null;
  }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  if (length !== undefined && bytes.length !== length) return null;
  // One spelling per value, as the desktop requires.
  return encode(bytes) === text ? bytes : null;
}

export function randomBytes(length: number): Uint8Array<ArrayBuffer> {
  return crypto.getRandomValues(new Uint8Array(length));
}

/** A pairing or device key, held so it can derive session keys and nothing can read it. */
export function importSecret(raw: Uint8Array<ArrayBuffer>): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", raw, "HKDF", false, ["deriveKey"]);
}

export type Side = "client" | "host";

/** One session's two keys and message counters. Seal and open each run strictly in order. */
export class Channel {
  #sent = 0;
  #received = 0;
  readonly #sealKey: CryptoKey;
  readonly #openKey: CryptoKey;
  readonly #context: Uint8Array<ArrayBuffer>;

  private constructor(sealKey: CryptoKey, openKey: CryptoKey, context: Uint8Array<ArrayBuffer>) {
    this.#sealKey = sealKey;
    this.#openKey = openKey;
    this.#context = context;
  }

  /**
   * `context` names what the secret is (`pair` or `device`) and its id, so a message sealed for
   * one credential never opens under another.
   */
  static async create(
    side: Side,
    secret: CryptoKey,
    clientNonce: Uint8Array,
    hostNonce: Uint8Array,
    context: string,
  ): Promise<Channel> {
    const salt = new Uint8Array(clientNonce.length + hostNonce.length);
    salt.set(clientNonce);
    salt.set(hostNonce, clientNonce.length);
    const key = (info: string, use: KeyUsage) =>
      crypto.subtle.deriveKey(
        { name: "HKDF", hash: "SHA-256", salt, info: encoder.encode(info) },
        secret,
        { name: "AES-GCM", length: 256 },
        false,
        [use],
      );
    const [toHost, toClient] = side === "client" ? ["encrypt", "decrypt"] : ["decrypt", "encrypt"];
    const hostKey = await key(TO_HOST, toHost as KeyUsage);
    const clientKey = await key(TO_CLIENT, toClient as KeyUsage);
    return new Channel(
      side === "client" ? hostKey : clientKey,
      side === "client" ? clientKey : hostKey,
      encoder.encode(`auracode remote v1|${context}`),
    );
  }

  /** Seals the next message this side sends. */
  async seal(message: string): Promise<string> {
    const sealed = await crypto.subtle.encrypt(
      { name: "AES-GCM", iv: counter(this.#sent), additionalData: this.#context },
      this.#sealKey,
      encoder.encode(message),
    );
    this.#sent += 1;
    return encode(new Uint8Array(sealed));
  }

  /**
   * Opens the next message the other side sent. Anything else, in any other order, throws, and
   * the session is over.
   */
  async open(sealed: string): Promise<string> {
    const bytes = decode(sealed);
    if (!bytes) throw new Error("The message could not be opened.");
    let opened: ArrayBuffer;
    try {
      opened = await crypto.subtle.decrypt(
        { name: "AES-GCM", iv: counter(this.#received), additionalData: this.#context },
        this.#openKey,
        bytes,
      );
    } catch {
      throw new Error("The message could not be opened.");
    }
    this.#received += 1;
    return decoder.decode(opened);
  }
}

/** The 96-bit nonce of a direction's `n`th message: the count, big-endian, after four zero bytes. */
function counter(n: number): Uint8Array<ArrayBuffer> {
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, BigInt(n));
  return nonce;
}
