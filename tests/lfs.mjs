import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const exec = promisify(execFile);
const media = "application/vnd.git-lfs+json";
const oid = (bytes) => createHash("sha256").update(bytes).digest("hex");

function auth(token) {
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function status(response, expected) {
  const body = await response.text();
  assert.equal(response.status, expected, body);
}

function postBatch(base, repo, operation, token, objects) {
  return fetch(`${base}${repo}.git/info/lfs/objects/batch`, {
    method: "POST",
    headers: { Accept: media, "Content-Type": `${media}; charset=utf-8`, ...auth(token) },
    body: JSON.stringify({ operation, objects }),
  });
}

async function result(response) {
  assert.equal(response.status, 200, await response.clone().text());
  assert.equal(response.headers.get("content-type"), media);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const json = await response.json();
  assert.equal(json.transfer, "basic");
  return json.objects[0];
}

export async function prepareLfsObject({ base, repo, writeToken, bytes = randomBytes(4096) }) {
  const spec = { oid: oid(bytes), size: bytes.length };
  const batch = ({ repo, token, operation = "download" }) => postBatch(base, repo, operation, token, [spec]);
  const { actions } = await result(await batch({ repo, token: writeToken, operation: "upload" }));
  if (actions?.upload) {
    await status(await fetch(actions.upload.href, {
      method: "PUT", headers: { ...actions.upload.header, ...auth(writeToken) }, body: bytes,
    }), 200);
  }
  return {
    bytes, spec, batch,
    async download({ repo, token }) {
      const entry = await result(await batch({ repo, token }));
      assert.ok(entry.actions?.download, JSON.stringify(entry));
      return fetch(entry.actions.download.href, { headers: auth(token) });
    },
  };
}

export async function exerciseLfs({ base, repo, writeToken, readToken, canRunGitLfs = true, maxObjectBytes = 100_000_000 }) {
  const bytes = randomBytes(4096);
  const spec = { oid: oid(bytes), size: bytes.length };
  const batch = (operation, token, objects = [spec]) => postBatch(base, repo, operation, token, objects);
  const unauthenticated = await batch("download");
  assert.match(unauthenticated.headers.get("lfs-authenticate"), /^Basic /);
  await status(unauthenticated, 401);
  if (readToken) await status(await batch("upload", readToken), 403);
  assert.equal((await result(await batch("download", writeToken))).error.code, 404);
  const boundary = { ...spec, size: maxObjectBytes };
  assert.ok((await result(await batch("upload", writeToken, [boundary]))).actions?.upload);
  assert.equal((await result(await batch("upload", writeToken, [{ ...boundary, size: maxObjectBytes + 1 }]))).error.code, 413);
  assert.equal((await result(await batch("upload", writeToken, [{ ...spec, oid: spec.oid.toUpperCase() }]))).error.code, 422);
  await status(await batch("upload", writeToken, Array(101).fill(spec)), 413);
  await status(await batch("upload", writeToken, [{ ...spec, oid: "x".repeat(65_536) }]), 413);
  const { actions } = await result(await batch("upload", writeToken));
  for (const { href, header } of Object.values(actions)) {
    const action = new URL(href);
    assert.equal(action.origin, base);
    assert.equal(action.username + action.password + action.search, "");
    assert.equal(header?.Authorization, undefined);
  }
  const put = (body, token = writeToken, href = actions.upload.href) => fetch(href, {
    method: "PUT", headers: { ...actions.upload.header, ...auth(token) }, body,
  });
  const verify = (object) => fetch(actions.verify.href, {
    method: "POST", headers: { "Content-Type": media, ...auth(writeToken) },
    body: JSON.stringify(object),
  });
  await status(await verify(spec), 404);
  await status(await put(Buffer.alloc(bytes.length)), 422);
  await status(await put(bytes.subarray(1)), 422);
  const otherId = actions.upload.href.replace(/\/objects\/(\d+)\//, (_, id) => `/objects/${Number(id) + 1}/`);
  await status(await put(bytes, writeToken, otherId), 404);
  assert.equal((await result(await batch("download", writeToken))).error.code, 404);
  await status(await put(bytes), 200);
  await status(await put(bytes), 200);
  await status(await verify({ ...spec, size: spec.size - 1 }), 422);
  await status(await verify(spec), 200);
  assert.equal((await result(await batch("upload", writeToken))).actions, undefined);
  const download = (await result(await batch("download", readToken ?? writeToken))).actions.download.href;
  await status(await fetch(download, { headers: auth("invalid-token") }), 401);
  const response = await fetch(download, { headers: auth(readToken ?? writeToken) });
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "application/octet-stream");
  assert.equal(response.headers.get("content-length"), String(bytes.length));
  assert.deepEqual(Buffer.from(await response.arrayBuffer()), bytes);
  if (!canRunGitLfs) return;

  const workspace = await mkdtemp(join(tmpdir(), "shard-lfs-"));
  try {
    const env = {
      ...process.env, GIT_CONFIG_GLOBAL: join(workspace, "gitconfig"), GIT_CONFIG_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0", SHARD_LFS_TEST_TOKEN: writeToken,
    };
    const git = (cwd, ...args) => exec("git", args, { cwd, env, timeout: 45_000 });
    await git(workspace, "config", "--global", "credential.helper",
      '!f() { if test "$1" = get; then printf "username=shard\\npassword=%s\\n" "$SHARD_LFS_TEST_TOKEN"; fi; }; f');
    await git(workspace, "init", "-b", "lfs-regression", "source");
    const source = join(workspace, "source");
    await git(source, "lfs", "install", "--local");
    await git(workspace, "lfs", "install", "--skip-repo");
    await git(source, "lfs", "track", "*.bin");
    const committed = randomBytes(65_537);
    await writeFile(join(source, "payload.bin"), committed);
    await git(source, "add", ".gitattributes", "payload.bin");
    await git(source, "-c", "user.name=Shard LFS", "-c", "user.email=lfs@example.test", "commit", "-m", "Track LFS test payload");
    const remote = `${base}${repo}.git`;
    await git(source, "remote", "add", "origin", remote);
    await git(source, "push", "origin", "HEAD:refs/heads/lfs-regression");
    await git(workspace, "clone", "--branch", "lfs-regression", remote, "restored");
    assert.deepEqual(await readFile(join(workspace, "restored", "payload.bin")), committed);
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}
