import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { open } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { applyDelta, isForbiddenDotGit, PackIndex, parseCommit, parseTag, parseTagTarget, parseTree, readPackedObject } from "../worker/git-r2-pack.mjs";
import { buildR2PackStore } from "../worker/git-r2-packstore.mjs";
import { parseReceivedPack } from "../worker/git-r2-receive.mjs";
import { createHash } from "node:crypto";
import pako from "pako";

function git(cwd, ...args) {
  return execFileSync("git", ["-c", "core.autocrlf=false", ...args], { cwd, encoding: "utf8" });
}

test("reads base and delta objects from a real pack index", async (t) => {
  const dir = mkdtempSync(join(tmpdir(), "shard-pack-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  git(dir, "init", "-q");
  git(dir, "config", "user.name", "Pack Test");
  git(dir, "config", "user.email", "pack@example.test");
  const body = "The quick brown fox jumps over the lazy dog.\n".repeat(400);
  writeFileSync(join(dir, "README.md"), `# Pack test\n\n${body}`);
  writeFileSync(join(dir, "alpha.txt"), body);
  git(dir, "add", ".");
  git(dir, "commit", "-q", "-m", "first commit");
  writeFileSync(join(dir, "README.md"), `# Pack test\n\n${body}\nchanged line\n`);
  writeFileSync(join(dir, "beta.txt"), body.replaceAll("quick", "slow"));
  git(dir, "add", ".");
  git(dir, "commit", "-q", "-m", "second commit");
  writeFileSync(join(dir, "alpha.txt"), `${body}\nappended\n`);
  git(dir, "add", ".");
  git(dir, "commit", "-q", "-m", "third commit");
  git(dir, "tag", "-a", "v1", "-m", "annotated tag");
  git(dir, "repack", "-adq");

  const packDir = join(dir, ".git", "objects", "pack");
  const idxName = readdirSync(packDir).find((name) => name.endsWith(".idx"));
  assert.ok(idxName, "repack produced an index");
  const base = join(packDir, idxName.slice(0, -4));
  const indexBytes = new Uint8Array(readFileSync(`${base}.idx`));
  const index = new PackIndex(indexBytes);
  const handle = await open(`${base}.pack`, "r");
  t.after(() => handle.close());
  const readRange = async (offset, length) => {
    const buffer = Buffer.alloc(length);
    const result = await handle.read(buffer, 0, length, offset);
    return new Uint8Array(buffer.buffer, buffer.byteOffset, result.bytesRead);
  };

  const listing = git(dir, "cat-file", "--batch-all-objects", "--batch-check=%(objectname) %(objecttype) %(objectsize)")
    .trim().split("\n").filter(Boolean);
  assert.ok(listing.length > 5);
  const types = new Map();
  for (const line of listing) {
    const [oid, type, size] = line.split(" ");
    types.set(oid, type);
    const object = await readPackedObject(index, oid, { readRange, maxBytes: 4 * 1024 * 1024 });
    assert.ok(object, `object ${oid} is present in the pack`);
    assert.equal(object.type, { commit: 1, tree: 2, blob: 3, tag: 4 }[type]);
    const expected = execFileSync("git", ["-c", "core.autocrlf=false", "cat-file", type, oid], { cwd: dir });
    assert.equal(object.data.length, Number(size));
    assert.deepEqual(Buffer.from(object.data), expected);
  }

  const head = git(dir, "rev-parse", "HEAD").trim();
  const headObject = await readPackedObject(index, head, { readRange });
  const commit = parseCommit(head, headObject.data);
  assert.equal(commit.hash, head);
  assert.equal(commit.tree, git(dir, "rev-parse", "HEAD^{tree}").trim());
  assert.equal(commit.author, "Pack Test");
  assert.equal(commit.author_email, "pack@example.test");
  assert.equal(commit.message.trimEnd(), "third commit");
  assert.deepEqual(commit.parents, [git(dir, "rev-parse", "HEAD^").trim()]);

  const treeObject = await readPackedObject(index, commit.tree, { readRange });
  const entries = parseTree(treeObject.data);
  assert.deepEqual(entries.map((entry) => entry.name), ["README.md", "alpha.txt", "beta.txt"]);
  for (const entry of entries) {
    assert.equal(entry.mode, "100644");
    assert.equal(git(dir, "rev-parse", `${head}:${entry.name}`).trim(), entry.hash);
  }

  const tag = git(dir, "rev-parse", "v1").trim();
  const tagObject = await readPackedObject(index, tag, { readRange });
  assert.equal(tagObject.type, 4);
  assert.equal(parseTagTarget(tagObject.data), head);
});

test("builds reusable raw pack metadata", async () => {
  const dir = mkdtempSync(join(tmpdir(), "shard-packstore-"));
  try {
    git(dir, "init", "-q");
    git(dir, "config", "user.name", "Pack Store");
    git(dir, "config", "user.email", "store@example.test");
    const body = "abcdefghij".repeat(2000);
    writeFileSync(join(dir, "a.txt"), body);
    writeFileSync(join(dir, "b.txt"), body.split("").reverse().join(""));
    git(dir, "add", ".");
    git(dir, "commit", "-q", "-m", "store");
    git(dir, "repack", "-adq");
    const packDir = join(dir, ".git", "objects", "pack");
    const idxName = readdirSync(packDir).find((name) => name.endsWith(".idx"));
    const stem = join(packDir, idxName.slice(0, -4));
    const index = new PackIndex(new Uint8Array(readFileSync(`${stem}.idx`)));
    const pack = readFileSync(`${stem}.pack`);
    const bucket = {
      async head(key) {
        return key === `${stem}.pack` ? { size: pack.length } : null;
      },
      async get(key, options) {
        if (key !== `${stem}.pack`) return null;
        const offset = options?.range?.offset ?? 0;
        const length = options?.range?.length ?? pack.length;
        const data = pack.subarray(offset, Math.min(offset + length, pack.length));
        return { size: data.length, arrayBuffer: async () => data.buffer.slice(data.byteOffset, data.byteOffset + data.length) };
      },
    };
    const store = await buildR2PackStore(bucket, [stem], async () => index);
    assert.ok(store);
    assert.equal(store.totalPackBytes(), pack.length);
    let checked = 0;
    for (const oid of git(dir, "cat-file", "--batch-all-objects", "--batch-check=%(objectname)").trim().split("\n")) {
      const entry = store.lookup(oid);
      if (!entry) continue;
      const raw = await store.readRaw(entry.packId, entry.dataOff, entry.dataLen);
      assert.deepEqual(Buffer.from(raw), pack.subarray(entry.dataOff, entry.dataOff + entry.dataLen));
      checked++;
    }
    assert.ok(checked > 0);
    const received = await parseReceivedPack(new Uint8Array(pack));
    assert.equal(received.entries.length, index.count);
    const rebuilt = new PackIndex(received.idx);
    // [C1] Verify .idx v2 has valid 40-byte trailer (pack checksum + idx checksum) accepted by `git verify-pack`.
    const verifyPath = join(dir, "rebuilt.pack");
    writeFileSync(verifyPath, pack);
    writeFileSync(join(dir, "rebuilt.idx"), received.idx);
    git(dir, "verify-pack", "-v", verifyPath);
    for (const oid of git(dir, "cat-file", "--batch-all-objects", "--batch-check=%(objectname)").trim().split("\n")) {
      assert.notEqual(rebuilt.find(oid), null);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("parseReceivedPack resolves REF_DELTA with exact Git OID and rejects truncated deltas", async () => {
  const baseData = new TextEncoder().encode("base-content-for-ref-delta\n".repeat(10));
  const baseHeader = new TextEncoder().encode(`blob ${baseData.length}\0`);
  const baseOid = createHash("sha1").update(baseHeader).update(baseData).digest("hex");
  const extra = new TextEncoder().encode("appended-line\n");
  const resultData = new Uint8Array(baseData.length + extra.length);
  resultData.set(baseData, 0);
  resultData.set(extra, baseData.length);
  const expectedOid = createHash("sha1")
    .update(new TextEncoder().encode(`blob ${resultData.length}\0`))
    .update(resultData)
    .digest("hex");

  // Encode delta: baseLen, resultLen, copy all of baseData, insert extra.
  const varint = (n) => {
    const out = [];
    let v = n;
    do {
      let b = v & 0x7f;
      v >>= 7;
      if (v) b |= 0x80;
      out.push(b);
    } while (v);
    return out;
  };
  const deltaBytes = new Uint8Array([
    ...varint(baseData.length),
    ...varint(resultData.length),
    0x80 | 0x10 | 0x20, // copy at offset 0, size in 2 bytes
    baseData.length & 0xff,
    (baseData.length >> 8) & 0xff,
    extra.length,
    ...extra,
  ]);
  const deflated = pako.deflate(deltaBytes);
  // Build minimal 1-entry REF_DELTA pack (type = 7).
  const head = [];
  let sz = deltaBytes.length;
  head.push((7 << 4) | (sz & 0x0f) | (sz >= 16 ? 0x80 : 0));
  sz >>= 4;
  while (sz > 0) {
    head.push((sz & 0x7f) | (sz >= 128 ? 0x80 : 0));
    sz >>= 7;
  }
  const baseOidBytes = Buffer.from(baseOid, "hex");
  const body = Buffer.concat([
    Buffer.from("PACK"),
    Buffer.from([0, 0, 0, 2, 0, 0, 0, 1]),
    Buffer.from(head),
    baseOidBytes,
    Buffer.from(deflated),
  ]);
  const packHash = createHash("sha1").update(body).digest();
  const pack = new Uint8Array(Buffer.concat([body, packHash]));

  // [C2] Resolve against external base reader and verify exact blob OID.
  const parsed = await parseReceivedPack(
    pack,
    20_000,
    async (oid) => (oid === baseOid ? { type: 3, data: baseData } : null),
  );
  assert.equal(parsed.entries.length, 1);
  assert.equal(parsed.entries[0].oid, expectedOid);
  assert.equal(parsed.entries[0].type, 3);

  // [H11] Truncated copy opcode in applyDelta must throw instead of silently reading undefined.
  assert.throws(() => applyDelta(baseData, new Uint8Array([...varint(baseData.length), ...varint(4), 0x81]), 1024));

  // [L20] parseTagTarget must ignore 'object ...' lines inside the tag message body.
  const fakeTag = new TextEncoder().encode(
    `object ${baseOid}\ntype commit\ntag v2\n\nmessage mentions:\nobject ${expectedOid}\n`,
  );
  assert.equal(parseTagTarget(fakeTag), baseOid);

  // [H3] parseTree enforces canonical modes, forbids .git variants, and requires canonical sort order.
  const rawTreeEntry = (mode, name, oidHex) => Buffer.concat([
    Buffer.from(`${mode} ${name}\0`),
    Buffer.from(oidHex, "hex"),
  ]);
  assert.throws(() => parseTree(new Uint8Array(rawTreeEntry("040000", "dir", baseOid))), /Invalid Git tree mode/);
  assert.throws(() => parseTree(new Uint8Array(rawTreeEntry("100664", "file", baseOid))), /Invalid Git tree mode/);
  assert.throws(() => parseTree(new Uint8Array(rawTreeEntry("100644", ".Git", baseOid))), /Invalid Git tree name/);
  assert.throws(() => parseTree(new Uint8Array(rawTreeEntry("100644", "GIT~1", baseOid))), /Invalid Git tree name/);
  assert.throws(() => parseTree(new Uint8Array(Buffer.concat([
    rawTreeEntry("100644", "b.txt", baseOid),
    rawTreeEntry("100644", "a.txt", expectedOid),
  ]))), /Invalid Git tree entry order/);

  // [H3] parseCommit requires tree first and single tree/author/committer headers.
  assert.throws(() => parseCommit(baseOid, new TextEncoder().encode(
    `tree ${baseOid}\nauthor A <a@b.c> 100 +0000\n\nmissing committer\n`,
  )), /Invalid Git commit/);

  // [M4] parseReceivedPack resolves an OFS_DELTA chained onto a REF_DELTA in a thin pack.
  const secondExtra = new TextEncoder().encode("chained-ofs-line\n");
  const secondResult = new Uint8Array(resultData.length + secondExtra.length);
  secondResult.set(resultData, 0);
  secondResult.set(secondExtra, resultData.length);
  const secondExpectedOid = createHash("sha1")
    .update(new TextEncoder().encode(`blob ${secondResult.length}\0`))
    .update(secondResult)
    .digest("hex");
  const secondDelta = new Uint8Array([
    ...varint(resultData.length),
    ...varint(secondResult.length),
    0x80 | 0x10 | 0x20,
    resultData.length & 0xff,
    (resultData.length >> 8) & 0xff,
    secondExtra.length,
    ...secondExtra,
  ]);
  const secondDeflated = pako.deflate(secondDelta);
  const ofsHead = [];
  let ofsSz = secondDelta.length;
  ofsHead.push((6 << 4) | (ofsSz & 0x0f) | (ofsSz >= 16 ? 0x80 : 0));
  ofsSz >>= 4;
  while (ofsSz > 0) {
    ofsHead.push((ofsSz & 0x7f) | (ofsSz >= 128 ? 0x80 : 0));
    ofsSz >>= 7;
  }
  const firstEntryBytes = Buffer.concat([Buffer.from(head), baseOidBytes, Buffer.from(deflated)]);
  // Negative offset encoding from entry 2 (offset = 12 + firstEntryBytes.length) back to entry 1 (offset = 12).
  const distance = firstEntryBytes.length;
  const ofsBytes = [];
  let rem = distance;
  ofsBytes.unshift(rem & 0x7f);
  while ((rem >>= 7) > 0) {
    rem -= 1;
    ofsBytes.unshift(0x80 | (rem & 0x7f));
  }
  const chainedBody = Buffer.concat([
    Buffer.from("PACK"),
    Buffer.from([0, 0, 0, 2, 0, 0, 0, 2]),
    firstEntryBytes,
    Buffer.from(ofsHead),
    Buffer.from(ofsBytes),
    Buffer.from(secondDeflated),
  ]);
  const chainedPack = new Uint8Array(Buffer.concat([chainedBody, createHash("sha1").update(chainedBody).digest()]));
  const chainedParsed = await parseReceivedPack(
    chainedPack,
    20_000,
    async (oid) => (oid === baseOid ? { type: 3, data: baseData } : null),
  );
  assert.equal(chainedParsed.entries.length, 2);
  assert.equal(chainedParsed.entries[1].oid, secondExpectedOid);

  // Hardened HFS+/NTFS .git detection, commit header order/NUL, and tag type validation.
  for (const forbidden of [".g\u200bit", ".g\u00adit", ".git ", ".git::$DATA", "git~2"]) {
    assert.equal(isForbiddenDotGit(forbidden), true, forbidden);
    assert.throws(() => parseTree(new Uint8Array(rawTreeEntry("100644", forbidden, baseOid))), /Invalid Git tree name/);
  }
  assert.throws(() => parseCommit(baseOid, new TextEncoder().encode(
    `tree ${baseOid}\ncommitter A <a@b.c> 100 +0000\nauthor A <a@b.c> 100 +0000\n\nout of order\n`,
  )), /Invalid Git commit/);
  assert.throws(() => parseCommit(baseOid, new TextEncoder().encode(
    `tree ${baseOid}\nauthor A <a@b.c> <evil@x.y> 100 +0000\ncommitter A <a@b.c> 100 +0000\n\nangle\n`,
  )), /Invalid Git commit/);
  assert.throws(() => parseCommit(baseOid, new TextEncoder().encode(
    `tree ${baseOid}\nauthor A <a@b.c> -1 +0000\ncommitter A <a@b.c> 100 +0000\n\nneg\n`,
  )), /Invalid Git commit/);
  assert.throws(() => parseCommit(baseOid, new TextEncoder().encode(
    `tree ${baseOid}\nauthor A <a@b.c> 100 +0000\ncommitter A <a@b.c> 100 +0000\n\0\nmsg\n`,
  )), /Invalid Git commit/);
  const validTag = parseTag(new TextEncoder().encode(
    `object ${baseOid}\ntype commit\ntag v1.0\ntagger A <a@b.c> 100 +0000\n\nrelease\n`,
  ));
  assert.equal(validTag.target, baseOid);
  assert.equal(validTag.type, 1);
  assert.throws(() => parseTag(new TextEncoder().encode(`object ${baseOid}\n\nmissing type and tag\n`)), /Invalid Git tag/);
  assert.throws(() => parseTag(new TextEncoder().encode(`object ${baseOid}\ntype bogus\ntag v1\n\n`)), /Invalid Git tag/);
});
