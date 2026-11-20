import assert from "node:assert/strict";
import test from "node:test";
import { ArchiveLimitError, ArchiveUnsupportedError, buildSourceTar } from "../worker/git-r2-archive.mjs";

const oid = (digit) => digit.repeat(40).slice(0, 40);

function tree(entries) {
  const chunks = [];
  for (const [mode, name, hash] of entries) {
    chunks.push(Buffer.from(`${mode} ${name}\0`), Buffer.from(hash, "hex"));
  }
  return new Uint8Array(Buffer.concat(chunks));
}

function parse(tar) {
  const entries = [];
  for (let offset = 0; offset + 512 <= tar.length;) {
    if (tar.subarray(offset, offset + 512).every((byte) => byte === 0)) break;
    const text = (start, length) => new TextDecoder().decode(tar.subarray(offset + start, offset + start + length)).replace(/\0.*$/, "");
    const prefix = text(345, 155);
    const name = text(0, 100);
    const size = Number.parseInt(text(124, 12).trim() || "0", 8);
    entries.push({ path: prefix ? `${prefix}/${name}` : name, mode: text(100, 8).trim(), type: String.fromCharCode(tar[offset + 156]), data: Buffer.from(tar.subarray(offset + 512, offset + 512 + size)).toString() });
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return entries;
}

test("builds a bounded ustar archive from Git trees", async () => {
  const root = oid("1");
  const directory = oid("2");
  const readme = oid("3");
  const script = oid("4");
  const objects = new Map([
    [root, { type: 2, data: tree([["100644", "README.md", readme], ["40000", "src", directory]]) }],
    [directory, { type: 2, data: tree([["100755", "run.sh", script]]) }],
    [readme, { type: 3, data: new TextEncoder().encode("hello\n") }],
    [script, { type: 3, data: new TextEncoder().encode("#!/bin/sh\nexit 0\n") }],
  ]);
  const tar = await buildSourceTar({ hash: oid("a"), tree: root }, async (hash) => objects.get(hash));
  const entries = parse(tar);
  assert.deepEqual(entries.map((entry) => entry.path), [
    `source-${oid("a")}`, `source-${oid("a")}/README.md`, `source-${oid("a")}/src`, `source-${oid("a")}/src/run.sh`,
  ]);
  assert.equal(entries[0].type, "5");
  assert.equal(entries[1].data, "hello\n");
  assert.equal(entries[3].mode, "0000755");
  assert.equal(entries[3].data, "#!/bin/sh\nexit 0\n");
  assert.ok(tar.length % 512 === 0);
});

test("rejects unsupported modes and duplicate names", async () => {
  const root = oid("1");
  await assert.rejects(
    buildSourceTar({ hash: oid("a"), tree: root }, async () => ({ type: 2, data: tree([["160000", "module", oid("2")]]) })),
    ArchiveUnsupportedError,
  );
  await assert.rejects(
    buildSourceTar({ hash: oid("a"), tree: root }, async () => ({ type: 2, data: tree([["100644", "same", oid("2")], ["100644", "same", oid("3")]]) })),
    ArchiveUnsupportedError,
  );
});

test("enforces the entry limit", async () => {
  const root = oid("1");
  const entries = Array.from({ length: 2_001 }, (_, index) => ["100644", `file-${index}`, oid("2")]);
  await assert.rejects(
    buildSourceTar({ hash: oid("a"), tree: root }, async () => ({ type: 2, data: tree(entries) })),
    ArchiveLimitError,
  );
});