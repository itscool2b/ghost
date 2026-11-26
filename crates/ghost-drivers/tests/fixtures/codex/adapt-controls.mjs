/** Explicit fixture migration for the session controls added in 9607df94. No native CLI runs. */
import assert from "node:assert/strict";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = new URL("./", import.meta.url);
const requestId = "aura-replay-collaboration";
const marker = {
  d: "note",
  note: {
    adaptation: "session-controls-9607df94",
    text: "Derived replay fixture: experimental API enabled; synthetic empty collaboration catalog added after startup. Original captured traffic is retained in originals/ when applicable.",
  },
};
const parse = (text) =>
  text.trim().split(/\r?\n/).map((line) => JSON.parse(line));

function migrate(original) {
  const records = structuredClone(original);
  assert.ok(!records.some((record) => record.note?.adaptation));
  const initialize = records.find(
    (record) => record.d === "out" && record.msg?.method === "initialize",
  );
  assert.equal(initialize?.msg.params.capabilities.experimentalApi, false);
  initialize.msg.params.capabilities.experimentalApi = true;
  assert.ok(!records.some((record) => record.msg?.id === requestId));
  const limits = records.findIndex(
    (record) => record.d === "out" && record.msg?.method === "account/rateLimits/read",
  );
  if (limits !== -1) {
    // The harness correlates recorded IDs to live IDs. Keep all captured IDs and timestamps.
    const time = records[limits].t;
    const stamp = time === undefined ? {} : { t: time };
    records.splice(limits, 0, {
      ...stamp,
      d: "out",
      msg: { id: requestId, method: "collaborationMode/list", params: {} },
    });
    records.splice(limits + 2, 0, {
      ...stamp,
      d: "in",
      msg: { id: requestId, result: { data: [] } },
    });
  }
  records.splice(1, 0, marker);
  return records;
}

function originalOf(records) {
  const original = structuredClone(records).filter(
    (record) => record.note?.adaptation !== marker.note.adaptation && record.msg?.id !== requestId,
  );
  const initialize = original.find(
    (record) => record.d === "out" && record.msg?.method === "initialize",
  );
  assert.equal(initialize?.msg.params.capabilities.experimentalApi, true);
  initialize.msg.params.capabilities.experimentalApi = false;
  return original;
}

const write = process.argv.includes("--write");
assert.ok(
  process.argv.slice(2).every((arg) => arg === "--write"),
  "Only --write is supported",
);
let files = 0;
let preserved = 0;
for (const name of await readdir(root)) {
  if (!name.endsWith(".jsonl")) continue;
  const path = new URL(name, root);
  const current = parse(await readFile(path, "utf8"));
  let original;
  try {
    original = parse(await readFile(new URL(`originals/${name}.original`, root), "utf8"));
    preserved += 1;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    original = current.some((record) => record.note?.adaptation)
      ? originalOf(current)
      : current;
  }
  const derived = migrate(original);
  assert.deepEqual(
    originalOf(derived), original,
    `${name}: migration must preserve all other captured fields`,
  );
  if (write) {
    await writeFile(path, `${derived.map((record) => JSON.stringify(record)).join("\n")}\n`);
  } else {
    assert.deepEqual(
      current, derived,
      `${name}: derived handshake differs from the documented migration`,
    );
  }
  files += 1;
}
assert.equal(files, 13);
assert.equal(preserved, 4);
console.log(
  `${files} Codex derived fixtures checked; ${preserved} original recordings preserved (${fileURLToPath(root)})`,
);
