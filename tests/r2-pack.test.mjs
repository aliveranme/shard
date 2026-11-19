import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { open } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { PackIndex, parseCommit, parseTagTarget, parseTree, readPackedObject } from "../worker/git-r2-pack.mjs";
import { buildR2PackStore } from "../worker/git-r2-packstore.mjs";

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
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
