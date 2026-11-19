import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { experimental_readRawConfig, unstable_dev } from "wrangler";

const root = fileURLToPath(new URL("../", import.meta.url));
const wrangler = join(root, "node_modules/wrangler/bin/wrangler.js");
const adapter = "shard-r2-multipart";
const media = "application/vnd.git-lfs+json";
const repo = "/owner/private";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const specFor = (bytes) => ({ oid: sha256(bytes), size: bytes.length });
const exec = promisify(execFile);

async function status(response, expected) {
  assert.equal(response.status, expected, await response.text());
}

async function json(response) {
  const text = await response.text();
  assert.equal(response.status, 200, text);
  return JSON.parse(text);
}

async function forge(mode, exercise, initialVars) {
  const temporary = await mkdtemp(join(tmpdir(), "shard-remote-lfs-"));
  const token = `shard_${randomBytes(32).toString("hex")}`;
  const readToken = `shard_${randomBytes(32).toString("hex")}`;
  const auth = `Bearer ${token}`;
  const persistence = join(temporary, "state");
  const configPath = join(temporary, "wrangler.json");
  const signedRequests = [];
  let worker;
  // Different origins exercise Git LFS's cross-origin credentials. The relay
  // preserves the signed Host header and uses Miniflare's actual S3 endpoint.
  const relay = createServer((req, res) => {
    signedRequests.push({ url: new URL(req.url, "http://localhost"), authorization: req.headers.authorization });
    const upstream = request({ hostname: "127.0.0.1", port: worker.port, path: req.url,
      headers: req.headers, method: req.method }, (reply) => {
      res.writeHead(reply.statusCode, reply.headers);
      reply.pipe(res);
    });
    upstream.on("error", () => res.destroy());
    res.on("close", () => upstream.destroy());
    req.pipe(upstream);
  });
  relay.listen(0, "127.0.0.1");
  await once(relay, "listening");
  try {
    const r2 = `http://127.0.0.1:${relay.address().port}`;
    const { rawConfig: { env: environments, ...baseConfig } } = experimental_readRawConfig({ config: join(root, "wrangler.jsonc") });
    const config = { ...baseConfig, ...environments.artifacts };
    delete config.build;
    delete config.$schema;
    delete config.durable_objects;
    delete config.migrations;
    config.name = "shard-lfs-remote-tests";
    config.main = resolve(root, config.main);
    config.assets.directory = resolve(root, config.assets.directory);
    config.vars = { ...config.vars, SHARD_LFS_UPLOAD_MODE: mode, SHARD_LFS_MAX_BYTES: "70000000",
      SHARD_LFS_R2_BUCKET: "shard-lfs", SHARD_LFS_R2_ENDPOINT: `${r2}/cdn-cgi/local/r2/s3` };
    config.r2_buckets[0].local_dev = { experimental_s3_credentials: {
      accessKeyId: "test-r2-access-key", secretAccessKey: "test-r2-secret-key",
    } };
    config.d1_databases = config.d1_databases.map((binding) => ({ ...binding,
      migrations_dir: resolve(root, binding.migrations_dir), remote: false }));
    const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false",
      GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(temporary, "gitconfig"), GIT_TERMINAL_PROMPT: "0" };
    const cli = (...args) => exec(process.execPath, [wrangler, "d1", ...args, "--local",
      "--persist-to", persistence, "--config", configPath], { cwd: temporary, env, timeout: 30_000 });
    await writeFile(join(temporary, ".dev.vars"), [
      "SHARD_LFS_R2_ACCESS_KEY_ID=test-r2-access-key", "SHARD_LFS_R2_SECRET_ACCESS_KEY=test-r2-secret-key", "",
    ].join("\n"));
    await writeFile(configPath, JSON.stringify(config));
    await cli("migrations", "apply", "DB");
    await cli("execute", "DB", "--command", `
      INSERT INTO users (id, username, email, password_hash, created_at)
      VALUES (73, 'owner', 'owner@example.test', 'fixture', 1);
      INSERT INTO repos (id, owner_id, name, artifacts_repo, visibility, created_at, updated_at)
      VALUES (81, 73, 'private', 'shard-lfs-fixture', 'private', 1, 1);
      INSERT INTO access_tokens (user_id, name, token_hash, scope, created_at)
      VALUES (73, 'upload', '${sha256(token)}', 'write', 1), (73, 'download', '${sha256(readToken)}', 'read', 1);
    `);
    let base;
    const startWorker = async (vars = {}) => {
      if (worker) {
        await worker.stop();
        worker = undefined;
      }
      await writeFile(configPath, JSON.stringify({ ...config, vars: { ...config.vars, ...vars } }));
      worker = await unstable_dev(config.main, { config: configPath, local: true, ip: "127.0.0.1", port: 0,
        persist: true, persistTo: persistence, logLevel: "error", experimental: {
          disableExperimentalWarning: true, disableDevRegistry: true, forceLocal: true, watch: false,
          showInteractiveDevSession: false,
        },
      });
      base = `http://127.0.0.1:${worker.port}`;
    };
    await startWorker(initialVars);
    const post = (href, body, authorization = auth) => fetch(href, { method: "POST", redirect: "error",
      headers: { "Content-Type": media, Authorization: authorization }, body: JSON.stringify(body),
      signal: AbortSignal.timeout(30_000) });
    const batch = (operation, spec, transfers, authorization) => post(`${base}${repo}.git/info/lfs/objects/batch`,
      { operation, objects: [spec], transfers }, authorization);
    const download = async (spec) => {
      const batchResult = await json(await batch("download", spec, ["basic"], `Bearer ${readToken}`));
      return batchResult.objects[0];
    };
    const gitLfs = async (spec, bytes, multipart) => {
      const checkout = join(temporary, `git-${spec.oid.slice(0, 8)}`);
      await mkdir(checkout);
      const git = (...args) => exec("git", args, {
        cwd: checkout, env: { ...env, SHARD_LFS_TEST_TOKEN: token }, timeout: 55_000,
      });
      await git("init", "-b", "main");
      await git("remote", "add", "origin", `${base}${repo}.git`);
      await git("config", "--local", "credential.helper",
        '!f() { if test "$1" = get; then printf "username=shard\\npassword=%s\\n" "$SHARD_LFS_TEST_TOKEN"; fi; }; f');
      if (multipart) {
        const script = join(root, "scripts/git-lfs-r2-multipart.mjs").replaceAll("\\", "/");
        await git("config", "--local", `lfs.customtransfer.${adapter}.path`, process.execPath);
        await git("config", "--local", `lfs.customtransfer.${adapter}.args`, `"${script}"`);
        await git("config", "--local", `lfs.customtransfer.${adapter}.direction`, "upload");
      }
      const objects = join(checkout, ".git", "lfs", "objects", spec.oid.slice(0, 2), spec.oid.slice(2, 4));
      await mkdir(objects, { recursive: true });
      await writeFile(join(objects, spec.oid), bytes);
      await git("lfs", "push", "--object-id", "origin", spec.oid);
    };
    await exercise({ r2, post, batch, download, gitLfs, signedRequests, readToken, startWorker,
      currentUrl: (href) => new URL(new URL(href).pathname, base) });
    assert.ok(signedRequests.length > 0);
    assert.ok(signedRequests.every(({ authorization }) => authorization === undefined));
  } finally {
    relay.closeAllConnections();
    await new Promise((done) => relay.close(done));
    await worker?.stop();
    await rm(temporary, { recursive: true, force: true });
  }
}

const put = (action, bytes) => fetch(action.href, { method: "PUT", headers: action.header,
  body: bytes, redirect: "error", signal: AbortSignal.timeout(35_000) });

test("standard Git LFS uses R2 presigned PUT and keeps staging private", { timeout: 120_000 }, async () => {
  await forge("r2-direct", async ({ r2, post, batch, download, gitLfs, signedRequests, readToken }) => {
    const bytes = Buffer.from("The staged object must be verified before publication.\n");
    const spec = specFor(bytes);
    await status(await batch("upload", spec, ["basic"], `Bearer ${readToken}`), 403);
    const over = await json(await batch("upload", { ...spec, size: 70_000_001 }, ["basic"]));
    assert.equal(over.objects[0].error.code, 413);
    const result = await json(await batch("upload", spec, ["basic"]));
    assert.equal(result.transfer, "basic");
    const { upload, verify } = result.objects[0].actions;
    assert.equal(new URL(upload.href).origin, r2);
    assert.equal(upload.header["x-amz-checksum-sha256"], Buffer.from(spec.oid, "hex").toString("base64"));
    const wrong = await put(upload, Buffer.alloc(bytes.length));
    if (wrong.ok) await status(await post(verify.href, spec), 422);
    await wrong.arrayBuffer();
    assert.equal((await download(spec)).error.code, 404);
    const stagedBytes = Buffer.from("A valid object outside the repository namespace.\n");
    const stagedSpec = specFor(stagedBytes);
    const staged = (await json(await batch("upload", stagedSpec, ["basic"]))).objects[0].actions;
    await status(await put(staged.upload, stagedBytes), 200);
    assert.equal((await download(stagedSpec)).error.code, 404);
    await status(await post(staged.verify.href, stagedSpec), 200);
    assert.ok((await download(stagedSpec)).actions.download);
    await gitLfs(spec, bytes, false);
    const { actions } = await download(spec);
    const data = await fetch(actions.download.href, { headers: { Authorization: `Bearer ${readToken}` } });
    assert.equal(data.status, 200);
    assert.deepEqual(Buffer.from(await data.arrayBuffer()), bytes);
    assert.equal(signedRequests.filter(({ url }) => url.searchParams.has("partNumber")).length, 0);
  });
});

test("multipart Git LFS resumes recorded parts and rejects wrong full SHA-256", { timeout: 150_000 }, async () => {
  await forge("r2-multipart", async ({ r2, post, batch, download, gitLfs, signedRequests, readToken }) => {
    const bytes = Buffer.alloc(32 * 1024 ** 2 + 4099, 37);
    const spec = specFor(bytes);
    await status(await batch("upload", spec, ["basic"]), 422);
    async function uploadAction(spec) {
      const result = await json(await batch("upload", spec, ["basic", adapter]));
      assert.equal(result.transfer, adapter);
      return result.objects[0].actions.upload;
    }
    async function submitPart(action, uploadId, partNumber, bytes) {
      const signed = await json(await post(action.href, { action: "part", uploadId, partNumber }));
      assert.equal(new URL(signed.href).origin, r2);
      const response = await put(signed, bytes);
      const etag = response.headers.get("ETag");
      await status(response, 200);
      assert.ok(etag);
      await status(await post(action.href, { action: "record", uploadId, partNumber, etag }), 200);
    }
    const action = await uploadAction(spec);
    const plan = await json(await post(action.href, { action: "start" }));
    await submitPart(action, plan.uploadId, 1, bytes.subarray(0, plan.partBytes));
    const resumed = await json(await post(action.href, { action: "start" }));
    assert.equal(resumed.uploadId, plan.uploadId);
    assert.deepEqual(resumed.uploaded, [1]);
    await status(await post(action.href, { action: "finish", uploadId: plan.uploadId }), 409);
    assert.equal((await download(spec)).error.code, 404);
    await gitLfs(spec, bytes, true);
    const { actions } = await download(spec);
    const data = await fetch(actions.download.href, { headers: { Authorization: `Bearer ${readToken}` } });
    assert.equal(data.status, 200);
    assert.equal(sha256(Buffer.from(await data.arrayBuffer())), spec.oid);
    assert.deepEqual(signedRequests.map(({ url }) => url.searchParams.get("partNumber")), ["1", "2"]);

    const invalid = specFor(Buffer.from("correct"));
    const badAction = await uploadAction(invalid);
    const failed = await json(await post(badAction.href, { action: "start" }));
    await submitPart(badAction, failed.uploadId, 1, Buffer.from("invalid"));
    await status(await post(badAction.href, { action: "finish", uploadId: failed.uploadId }), 422);
    assert.equal((await download(invalid)).error.code, 404);
    const retry = await json(await post(badAction.href, { action: "start" }));
    assert.notEqual(retry.uploadId, failed.uploadId);
    assert.deepEqual(retry.uploaded, []);
  });
});

test("Paid multipart defaults to one R2 PUT and preserves stored objects after lowering the cap", { timeout: 150_000 }, async () => {
  const singlePut = (5 * 1024 - 5) * 1024 ** 2;
  const r2Limit = singlePut * 1024;
  const explicitLimit = { SHARD_LFS_MAX_BYTES: String(r2Limit) };
  await forge("r2-multipart", async ({ post, batch, download, readToken, startWorker, currentUrl }) => {
    const object = async (spec) => (await json(await batch("upload", spec, [adapter]))).objects[0];
    const large = { ...specFor("large upload"), size: r2Limit };
    const largeUpload = (await object(large)).actions.upload;
    const largePlan = await json(await post(largeUpload.href, { action: "start" }));
    assert.ok(largePlan.uploadId);
    assert.equal((await object({ ...large, size: r2Limit + 1 })).error.code, 413);

    await startWorker({ SHARD_LFS_MAX_BYTES: undefined });
    const boundary = { ...specFor("boundary upload"), size: singlePut };
    const boundaryUpload = (await object(boundary)).actions.upload;
    assert.ok((await json(await post(boundaryUpload.href, { action: "start" }))).uploadId);
    assert.equal((await object({ ...boundary, size: singlePut + 1 })).error.code, 413);
    const forged = boundaryUpload.href.replace(`/${singlePut}/multipart`, `/${singlePut + 1}/multipart`);
    assert.notEqual(forged, boundaryUpload.href);
    await status(await post(forged, { action: "start" }), 413);
    await status(await post(currentUrl(largeUpload.href), { action: "start" }), 413);
    await status(await post(currentUrl(largeUpload.href), { action: "part", uploadId: largePlan.uploadId, partNumber: 1 }), 413);

    const bytes = Buffer.from("Stored before the cap was lowered.\n");
    const stored = specFor(bytes);
    const storedUpload = (await object(stored)).actions.upload;
    const { uploadId } = await json(await post(storedUpload.href, { action: "start" }));
    const part = await json(await post(storedUpload.href, { action: "part", uploadId, partNumber: 1 }));
    const uploaded = await put(part, bytes);
    const etag = uploaded.headers.get("ETag");
    await status(uploaded, 200);
    assert.ok(etag);
    await status(await post(storedUpload.href, { action: "record", uploadId, partNumber: 1, etag }), 200);
    await status(await post(storedUpload.href, { action: "finish", uploadId }), 200);

    await startWorker({ SHARD_LFS_MAX_BYTES: "1" });
    assert.equal((await object({ ...specFor("missing object"), size: stored.size })).error.code, 413);
    assert.deepEqual(await object(stored), stored);
    const { actions } = await download(stored);
    const data = await fetch(actions.download.href, { headers: { Authorization: `Bearer ${readToken}` } });
    assert.equal(data.status, 200);
    assert.deepEqual(Buffer.from(await data.arrayBuffer()), bytes);
  }, explicitLimit);
});
