// The browser's half of the remote channel, held to the vectors the desktop's half is held to.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { Channel, decode, encode, importSecret, randomBytes } from "../app/remote/channel.ts";
import { takePairingLink } from "../app/remote/link.ts";

interface Vectors {
  secret: string;
  client_nonce: string;
  host_nonce: string;
  context: string;
  client_to_host: [string, string][];
  host_to_client: [string, string][];
}

const vectors: Vectors = JSON.parse(
  await readFile(
    new URL("../../../crates/ghost-remote/tests/vectors.json", import.meta.url),
    "utf8",
  ),
);

async function channels(context = vectors.context, secret = vectors.secret) {
  const key = await importSecret(decode(secret, 32)!);
  const client = decode(vectors.client_nonce, 16)!;
  const host = decode(vectors.host_nonce, 16)!;
  return {
    client: await Channel.create("client", key, client, host, context),
    host: await Channel.create("host", key, client, host, context),
  };
}

test("seals and opens exactly what the desktop does", async () => {
  const { client, host } = await channels();
  assert.ok(vectors.client_to_host.length >= 3 && vectors.host_to_client.length >= 3);
  for (const [plain, sealed] of vectors.client_to_host) {
    assert.equal(await client.seal(plain), sealed);
    assert.equal(await host.open(sealed), plain);
  }
  for (const [plain, sealed] of vectors.host_to_client) {
    assert.equal(await host.seal(plain), sealed);
    assert.equal(await client.open(sealed), plain);
  }
});

test("opens only the next message, sealed with the same key for the same credential", async () => {
  const { client, host } = await channels();
  const first = await client.seal("one");
  const second = await client.seal("two");
  // Out of order, replayed, echoed back or changed: all refused, and nothing is lost.
  await assert.rejects(host.open(second));
  await assert.rejects(client.open(first));
  const changed = decode(first)!;
  changed[0] ^= 1;
  await assert.rejects(host.open(encode(changed)));
  await assert.rejects(host.open("not base64!"));
  assert.equal(await host.open(first), "one");
  await assert.rejects(host.open(first));
  assert.equal(await host.open(second), "two");

  const sealed = await (await channels()).client.seal("for this device");
  await assert.rejects((await channels("device|another")).host.open(sealed));
  await assert.rejects(
    (await channels(vectors.context, encode(new Uint8Array(32)))).host.open(sealed),
  );
  assert.equal(await (await channels()).host.open(sealed), "for this device");
});

test("a stored key can be used and never read", async () => {
  const key = await importSecret(randomBytes(32));
  assert.equal(key.extractable, false);
  await assert.rejects(crypto.subtle.exportKey("raw", key));
});

test("ids and keys decode only in their one canonical spelling", () => {
  const key = randomBytes(32);
  const text = encode(key);
  assert.equal(text.length, 43);
  assert.deepEqual(decode(text, 32), key);
  assert.equal(decode(text, 16), null);
  assert.equal(decode(`${text}=`), null);
  assert.equal(decode("a+b/"), null);
  assert.equal(decode("A"), null);
  assert.deepEqual(decode("AA"), new Uint8Array([0]));
  assert.equal(decode("AB"), null);
  assert.deepEqual(decode(""), new Uint8Array(0));
});

/** An address bar and history that record what the page did to them. */
function addressOf(href: string) {
  const url = new URL(href);
  const replaced: { state: unknown; url: string }[] = [];
  const address = {
    location: { hash: url.hash, pathname: url.pathname, search: url.search },
    history: {
      state: { usr: null, key: "default", idx: 0 },
      replaceState(state: unknown, _unused: string, next?: string | URL | null) {
        replaced.push({ state, url: String(next) });
        address.location.hash = "";
      },
    },
  };
  return { address, replaced };
}

test("a pairing link leaves the address the moment it is taken, keeping the router's entry", () => {
  const { address, replaced } = addressOf(
    "https://onghost.dev/remote?relay=ws://127.0.0.1:8799#aaaa.bbbb.cccc",
  );
  assert.equal(takePairingLink(address), "#aaaa.bbbb.cccc");
  assert.deepEqual(replaced, [
    {
      state: { usr: null, key: "default", idx: 0 },
      url: "/remote?relay=ws://127.0.0.1:8799",
    },
  ]);
  // Nothing is left to take, and nothing more is rewritten.
  assert.equal(takePairingLink(address), "");
  assert.equal(replaced.length, 1);
});

test("every link is taken, not only the one the page loaded with", () => {
  const { address } = addressOf("https://onghost.dev/remote");
  assert.equal(takePairingLink(address), "");
  // A second computer's code scanned while the page is open changes only the part after `#`.
  address.location.hash = "#second.link.key";
  assert.equal(takePairingLink(address), "#second.link.key");
  address.location.hash = "#third.link.key";
  assert.equal(takePairingLink(address), "#third.link.key");
  // A bare `#` is no link.
  address.location.hash = "#";
  assert.equal(takePairingLink(address), "");
});
