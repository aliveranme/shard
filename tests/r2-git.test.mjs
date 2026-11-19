import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import { chmod, copyFile, cp, mkdir, mkdtemp, readdir, rm, stat, writeFile } from "node:fs/promises";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { experimental_readRawConfig, unstable_startWorker } from "wrangler";
import recoveryWorker from "../worker/git-r2-recovery.mjs";

const exec = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const wrangler = join(root, "node_modules/wrangler/bin/wrangler.js");
const MiB = 1024 ** 2;
const responseType = "application/x-git-receive-pack-result";

async function expect(response, status) {
  const body = await response.text();
  assert.equal(response.status, status, body.slice(0, 1024));
  return body;
}

async function waitFor(operation, timeout = 60_000) {
  const deadline = Date.now() + timeout;
  do {
    const value = await operation();
    if (value) return value;
    await delay(400);
  } while (Date.now() < deadline);
  assert.fail("Timed out waiting for Git storage maintenance");
}

// Miniflare implements signed UploadPart and completion but returns 501 for
// ListParts. Keep the listing in this relay from successful PUTs only.
function s3Relay(workerPort) {
  const received = new Map();
  const incomingPuts = [];
  const expired = new Set();
  const errors = [];
  let cut = true;
  let hideFirstReceipt = true;
  let afterIndex;
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (!url.pathname.startsWith("/cdn-cgi/local/r2/s3/")) {
      res.writeHead(404).end();
      return;
    }
    const id = url.searchParams.get("uploadId");
    const incoming = url.pathname.includes("/git-incoming/") && id;
    const pack = url.pathname.includes("/git-packs/");
    const part = Number(url.searchParams.get("partNumber"));
    if (incoming && req.method === "GET") {
      if (expired.has(id)) {
        res.writeHead(404, { "Content-Type": "application/xml" });
        res.end("<Error><Code>NoSuchUpload</Code></Error>");
        return;
      }
      const entries = received.get(id) ?? new Map();
      const items = [...entries].sort(([a], [b]) => a - b).map(([number, { tag, size }]) =>
        `<Part><PartNumber>${number}</PartNumber><ETag>${tag}</ETag><Size>${size}</Size></Part>`);
      res.writeHead(200, { "Content-Type": "application/xml" });
      res.end(`<ListPartsResult><IsTruncated>false</IsTruncated>${items.join("")}</ListPartsResult>`);
      return;
    }
    if (incoming && req.method === "PUT" && part > 1 && cut) {
      req.resume();
      res.writeHead(403).end();
      return;
    }
    const upstream = request({ hostname: "127.0.0.1", port: workerPort(), path: req.url,
      headers: req.headers, method: req.method }, (reply) => {
      if (reply.statusCode >= 400 && reply.statusCode !== 404) {
        const action = `${req.method} ${pack ? "pack" : "bucket"}`;
        errors.push(`${action} HTTP ${reply.statusCode}`);
      }
      if (incoming && req.method === "PUT" && reply.statusCode === 200) {
        if (!received.has(id)) received.set(id, new Map());
        received.get(id).set(part, { tag: reply.headers.etag, size: Number(req.headers["content-length"]) });
        incomingPuts.push(part);
        if (part === 1 && hideFirstReceipt) {
          hideFirstReceipt = false;
          reply.resume();
          res.destroy();
          return;
        }
      }
      if (pack && req.method === "PUT" && part && reply.statusCode === 200) {
        // Miniflare's completion parser leaves rclone's numeric quote entities in ETags.
        reply.headers.etag = reply.headers.etag.replace(/^"|"$/g, "");
      }
      const forward = () => {
        res.writeHead(reply.statusCode, reply.headers);
        reply.pipe(res);
      };
      if (afterIndex && pack && req.method === "PUT" && reply.statusCode === 200 && url.pathname.endsWith(".idx")) {
        const apply = afterIndex;
        afterIndex = undefined;
        void apply().then(forward, error => {
          errors.push(error.message);
          reply.resume();
          res.destroy();
        });
      } else {
        forward();
      }
    });
    upstream.on("error", () => res.destroy());
    res.on("close", () => upstream.destroy());
    req.pipe(upstream);
  });
  return { server, errors, incomingPuts,
    continue: () => { cut = false; },
    expireUpload: id => expired.add(id),
    afterNextIndex: apply => { afterIndex = apply; },
  };
}

function browser(base) {
  const jar = new Map();
  let csrf;
  return async (path, fields) => {
    const response = await fetch(base + path, {
      method: fields ? "POST" : "GET", redirect: "manual",
      headers: { Cookie: [...jar].map(([key, value]) => `${key}=${value}`).join("; "),
        ...(fields && { Origin: base }) },
      body: fields ? new URLSearchParams({ csrf_token: csrf, ...fields }) : undefined,
      signal: AbortSignal.timeout(30_000),
    }).catch(error => { throw new Error(`${fields ? "POST" : "GET"} ${path} failed`, { cause: error }); });
    for (const cookie of response.headers.getSetCookie()) {
      const pair = cookie.split(";", 1)[0];
      const at = pair.indexOf("=");
      jar.set(pair.slice(0, at), pair.slice(at + 1));
    }
    const text = await response.text();
    csrf = text.match(/name="csrf_token" value="([a-f0-9]{64})"/)?.[1] ?? csrf;
    return { status: response.status, text, location: response.headers.get("location") };
  };
}

test("R2 Git resumes multipart, compacts live packs, and restores historical reviews and forks", {
  skip: !process.env.SHARD_TEST_R2_CONTAINER && "requires a local FUSE-enabled Docker engine",
  timeout: 420_000,
}, async (t) => {
  assert.notEqual(process.platform, "win32", "Wrangler container development requires Linux, WSL2 or macOS");
  const temporary = await mkdtemp(join(tmpdir(), "shard-r2-test-"));
  const persistence = join(temporary, "state");
  const configPath = join(temporary, "wrangler.json");
  const bin = join(temporary, "bin");
  const work = join(temporary, "work");
  let s3Port;
  const relay = s3Relay(() => s3Port);
  let worker;
  let mf;
  const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false",
    PATH: `${bin}${delimiter}${process.env.PATH}`, GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: join(temporary, "gitconfig"), GIT_TERMINAL_PROMPT: "0" };
  const git = (...args) => exec("git", args, { cwd: work, env, timeout: 180_000 });
  try {
    relay.server.listen(Number(process.env.SHARD_TEST_R2_RELAY_PORT ?? 0), "0.0.0.0");
    await once(relay.server, "listening");
    const relayPort = relay.server.address().port;
    const relayHost = process.env.SHARD_TEST_R2_RELAY_HOST ?? "host.docker.internal";
    const { rawConfig: { env: environments, ...base } } = experimental_readRawConfig({ config: join(root, "wrangler.jsonc") });
    const config = { ...base, ...environments.r2 };
    delete config.build;
    delete config.$schema;
    // Allow the container's signed Host on the local S3 endpoint.
    config.routes = [`${relayHost}/*`];
    config.main = resolve(root, config.main);
    config.assets.directory = resolve(root, config.assets.directory);
    config.d1_databases[0].migrations_dir = join(root, "migrations");
    config.containers[0].images.git = {
      dockerfile: join(root, "containers", "git-r2.Dockerfile"), build_context: root,
    };
    const gitBucket = config.r2_buckets.find(bucket => bucket.binding === "GIT_PACKS");
    const accessKeyId = "local-shard-git-key";
    const secretAccessKey = "local-shard-git-secret";
    gitBucket.local_dev = { experimental_s3_credentials: { accessKeyId, secretAccessKey } };
    config.vars = { ...config.vars, SHARD_GIT_R2_BUCKET: gitBucket.bucket_name,
      SHARD_GIT_R2_ENDPOINT: `http://127.0.0.1:${relayPort}/cdn-cgi/local/r2/s3`,
      SHARD_GIT_R2_CONTAINER_ENDPOINT: `http://${relayHost}:${relayPort}/cdn-cgi/local/r2/s3`,
    };
    await writeFile(configPath, JSON.stringify(config));
    await writeFile(join(temporary, ".dev.vars"), [
      `SHARD_GIT_R2_ACCESS_KEY_ID=${accessKeyId}`, `SHARD_GIT_R2_SECRET_ACCESS_KEY=${secretAccessKey}`, "",
    ].join("\n"));
    await exec(process.execPath, [wrangler, "d1", "migrations", "apply", "DB", "--local", "--persist-to", persistence,
      "--config", configPath], { cwd: root, env, timeout: 60_000 });
    const start = async (port = 0, data = persistence) => {
      worker = await unstable_startWorker({ config: configPath, dev: {
        remote: false, watch: false, enableContainers: true, persist: data,
        containerBuildId: randomBytes(4).toString("hex"),
        routeRequestsByRoutes: true, inferOriginFromRoutes: false,
        server: { hostname: "127.0.0.1", port }, inspector: false, logLevel: "log",
      } });
      const runtime = worker.raw.runtimes[0];
      if (!runtime.mf) {
        await once(worker.raw, "reloadComplete", { signal: AbortSignal.timeout(90_000) });
      }
      // The public Wrangler proxy checks the Host before forwarding to Miniflare.
      mf = runtime.mf;
      s3Port = Number((await mf.ready).port);
      return await worker.url;
    };
    const address = await start();
    const port = Number(address.port);
    const baseUrl = address.origin;
    const view = browser(baseUrl);
    await view("/user/sign_up");
    const password = "local-r2-test-password";
    const signup = await view("/user/sign_up", { username: "owner", email: "owner@example.test", password });
    assert.equal(signup.status, 302, signup.text);
    await view("/user/settings/tokens");
    const token = (await view("/user/settings/tokens", {
      name: "R2 test push", scope: "write", expires_in: "7", current_password: password,
    })).text
      .match(/\bshard_[a-f0-9]{64}\b/)?.[0];
    assert.ok(token);
    const readToken = (await view("/user/settings/tokens", {
      name: "R2 test read", scope: "read", expires_in: "7", current_password: password,
    })).text
      .match(/\bshard_[a-f0-9]{64}\b/)?.[0];
    assert.ok(readToken);
    env.SHARD_TEST_GIT_AUTH = `Authorization: Bearer ${token}`;
    const created = await view("/repo/create", { name: "large", private: "on", description: "R2 Git integration" });
    assert.equal(created.location, "/owner/large", created.text);
    const remote = `${baseUrl}/owner/large.git`;
    await expect(await fetch(`${remote}/info/refs?service=git-upload-pack`), 401);
    const post = (payload, credential = token) => fetch(`${remote}/shard-upload`, { method: "POST", redirect: "error",
      body: JSON.stringify(payload), headers: { Authorization: `Bearer ${credential}`, "Content-Type": "application/json" },
      signal: AbortSignal.timeout(180_000) });

    await mkdir(bin);
    await mkdir(work);
    const helper = join(bin, "git-remote-shard");
    await copyFile(join(root, "scripts", "git-remote-shard.mjs"), helper);
    await chmod(helper, 0o755);
    await git("init", "-b", "main");
    await git("config", "user.name", "Shard R2 Test");
    await git("config", "user.email", "owner@example.test");
    await git("config", "http.extraHeader", `Authorization: Bearer ${token}`);
    await git("remote", "add", "origin", remote);
    await writeFile(join(work, "README.md"), "Cloudflare-hosted Git objects\n");
    await git("add", "README.md");
    await git("commit", "-m", "Push with stock HTTPS Git");
    await git("push", "origin", "main");
    const initial = (await git("rev-parse", "HEAD")).stdout.trim();
    assert.match((await git("ls-remote", "origin", "refs/heads/main")).stdout, new RegExp(initial));

    const large = join(work, "large.bin");
    await writeFile(large, randomBytes(129 * MiB));
    await git("add", "large.bin");
    await git("commit", "-m", "Push an incompressible object with resumable transport");
    const tip = (await git("rev-parse", "HEAD")).stdout.trim();
    await git("remote", "set-url", "--push", "origin", `shard::${remote}`);
    await assert.rejects(git("push", "origin", "main"), /R2 upload part 2 returned HTTP 403/);
    assert.deepEqual(relay.incomingPuts, [1]);
    assert.match((await git("ls-remote", "origin", "refs/heads/main")).stdout, new RegExp(initial));
    const directory = join(work, ".git", "shard-uploads");
    const [spool] = await readdir(directory);
    const spoolId = spool.match(/-([a-f0-9]{64}-[a-f0-9]{32})\.receive$/)?.[1];
    assert.ok(spoolId);
    const startUpload = { action: "start", sha256: spoolId.slice(0, 64), nonce: spoolId.slice(65),
      size: (await stat(join(directory, spool))).size };
    const denied = await post(startUpload, readToken);
    assert.equal(denied.headers.get("www-authenticate"), 'Basic realm="shard"');
    await expect(denied, 401);
    const partial = JSON.parse(await expect(await post(startUpload), 200));
    assert.deepEqual(partial.uploaded, [1]);

    await worker.dispose();
    relay.continue();
    await start(port);
    const current = JSON.parse(await expect(await post(startUpload), 200));
    assert.equal(current.id, partial.id);
    assert.deepEqual(current.uploaded, [1]);
    const resume = () => exec(process.execPath, [helper, "--resume", spoolId, remote], { cwd: work, env, timeout: 180_000 });
    const rules = `${created.location}/settings/branches`;
    assert.equal((await view(rules)).status, 200);
    relay.afterNextIndex(async () => {
      const response = await view(rules, { action: "save", branch: "main", approvals: "1" });
      assert.equal(response.status, 302);
    });
    await assert.rejects(resume(), /Shard multipart finish returned HTTP 403/);
    assert.deepEqual(await readdir(directory), [spool]);
    assert.equal((await git("ls-remote", "origin", "refs/heads/main")).stdout.trim(), `${initial}\trefs/heads/main`);
    assert.equal((await view(rules, { action: "remove", branch: "main", confirm: "main" })).status, 302);
    const resumed = await resume();
    assert.match(resumed.stderr, /refs\/heads\/main/);
    const expectedParts = Math.ceil(startUpload.size / partial.partBytes);
    assert.deepEqual(relay.incomingPuts, Array.from({ length: expectedParts }, (_, i) => i + 1));
    assert.deepEqual(await readdir(directory), []);
    const complete = JSON.parse(await expect(await post(startUpload), 200));
    assert.equal(complete.complete, true);
    assert.equal(complete.id, partial.id);
    const finish = async () => {
      const reply = await post({ action: "finish", id: partial.id });
      assert.equal(reply.status, 200);
      assert.equal(reply.headers.get("content-type"), responseType);
      return Buffer.from(await reply.arrayBuffer());
    };
    const report = await finish();
    assert.ok(report.includes(Buffer.from("ok refs/heads/main")));
    assert.deepEqual(await finish(), report);
    const clone = join(temporary, "cold-clone");
    await worker.dispose();
    await start(port);
    assert.deepEqual(await finish(), report);
    await git("--config-env=http.extraHeader=SHARD_TEST_GIT_AUTH", "clone", "--no-single-branch", remote, clone);
    assert.equal((await git("-C", clone, "rev-parse", "HEAD")).stdout.trim(), tip);
    await git("-C", clone, "fsck", "--full");
    assert.equal((await stat(join(clone, "large.bin"))).size, 129 * MiB);
    const oid = (await git("rev-parse", "HEAD:large.bin")).stdout;
    assert.equal((await git("-C", clone, "rev-parse", "HEAD:large.bin")).stdout, oid);

    const branch = "refs/heads/topic\u00a0branch";
    const tag = "refs/tags/r2-audit";
    await git("tag", "-a", "r2-audit", initial, "-m", "Annotated R2 tag");
    const tagOid = (await git("rev-parse", tag)).stdout.trim();
    const branchTip = (await git("commit-tree", `${initial}^{tree}`, "-p", initial, "-m", "Non-main R2 branch")).stdout.trim();
    await git("push", remote, `${branchTip}:${branch}`, tag);
    const advertised = (await git("ls-remote", `shard::${remote}`, branch, tag, `${tag}^{}`)).stdout.trimEnd().split("\n");
    assert.ok(advertised.includes(`${branchTip}\t${branch}`));
    assert.ok(advertised.includes(`${tagOid}\t${tag}`));
    assert.ok(advertised.includes(`${initial}\t${tag}^{}`));
    await git("--config-env=http.extraHeader=SHARD_TEST_GIT_AUTH", "-C", clone, "fetch", `shard::${remote}`,
      `${branch}:refs/remotes/audit/topic`);
    assert.equal((await git("-C", clone, "rev-parse", "refs/remotes/audit/topic")).stdout.trim(), branchTip);

    const db = await mf.getD1Database("DB");
    const { id, actor } = await db.prepare("SELECT artifacts_repo AS id, CAST(owner_id AS TEXT) AS actor FROM repos WHERE name = 'large'").first();
    const namespace = await mf.getDurableObjectNamespace("GIT_R2_REPOS");
    const repo = namespace.get(namespace.idFromName(id));
    const internal = (action, body) => repo.fetch(`https://internal/v1/r2/${id}/${action}`, {
      method: "POST", body: JSON.stringify({ actor, ...body }),
    });
    const line = `${tagOid} ${"0".repeat(40)} ${tag}\0report-status\n`;
    const deletion = Buffer.from(`${(Buffer.byteLength(line) + 4).toString(16).padStart(4, "0")}${line}0000`);
    const sha256 = createHash("sha256").update(deletion).digest("hex");
    const deleteStart = { action: "start", sha256, nonce: randomBytes(16).toString("hex"), size: deletion.length };
    const signPart = async id => JSON.parse(await expect(await post({ action: "part", id, number: 1 }), 200));
    const putPart = async ({ href, header }) => expect(await fetch(href, { method: "PUT", headers: header, body: deletion }), 200);

    const original = JSON.parse(await expect(await post(deleteStart), 200));
    const expiredPart = await signPart(original.id);
    const expiredUpload = new URL(expiredPart.href).searchParams.get("uploadId");
    assert.ok(expiredUpload);
    const bucket = await mf.getR2Bucket("GIT_PACKS");
    await bucket.resumeMultipartUpload(`git-incoming/${id}/${original.id}`, expiredUpload).abort();
    relay.expireUpload(expiredUpload);
    const finalize = { action: "finish", id: original.id };
    await expect(await post(finalize), 409);
    const restarted = JSON.parse(await expect(await post(deleteStart), 200));
    assert.equal(restarted.id, original.id);
    assert.deepEqual(restarted.uploaded, []);
    const replacementPart = await signPart(original.id);
    assert.notEqual(new URL(replacementPart.href).searchParams.get("uploadId"), expiredUpload);
    await putPart(replacementPart);

    const prepared = JSON.parse(await expect(await internal("multipart", { command: finalize }), 202));
    await git("push", remote, `${initial}:refs/heads/other`);
    const stale = await internal("publish", { prepared: prepared.prepared });
    assert.equal(stale.headers.get("Retry-After"), "1");
    await expect(stale, 409);
    assert.equal((await git("ls-remote", "origin", tag)).stdout.trim(), `${tagOid}\t${tag}`);
    const rebased = JSON.parse(await expect(await internal("multipart", { command: finalize }), 202));
    assert.notEqual(rebased.prepared, prepared.prepared);
    const publish = () => internal("publish", { prepared: rebased.prepared });
    const publications = await Promise.all([publish(), publish()]);
    assert.equal(publications.filter(reply => reply.headers.has("X-Shard-Replayed-Push")).length, 1);
    const [published, replayed] = await Promise.all(publications.map(reply => expect(reply, 200)));
    assert.match(published, /ok refs\/tags\/r2-audit/);
    assert.equal(replayed, published);
    assert.equal(await expect(await post(finalize), 200), published);

    // A newly generated deletion can have precisely the same receive-pack bytes.
    const deletionDigests = [];
    for (let attempt = 0; attempt < 2; attempt++) {
      await git("push", remote, tag);
      const { stderr } = await git("push", "origin", `:${tag}`);
      const digest = stderr.match(/Staged Git push ([0-9a-f]{64})-/)?.[1];
      assert.ok(digest);
      deletionDigests.push(digest);
      assert.equal((await git("ls-remote", "origin", tag)).stdout, "");
    }
    assert.equal(deletionDigests[0], deletionDigests[1]);

    await git("push", remote, tag);
    const badStart = { ...deleteStart, sha256: "0".repeat(64) };
    const bad = JSON.parse(await expect(await post(badStart), 200));
    await putPart(await signPart(bad.id));
    await expect(await post({ action: "finish", id: bad.id }), 422);
    const failedSpool = `${badStart.sha256}-${badStart.nonce}`;
    const remoteKey = createHash("sha256").update(remote).digest("hex");
    const failedSpoolPath = join(directory, `${remoteKey}-${failedSpool}.receive`);
    await writeFile(failedSpoolPath, deletion);
    await exec(process.execPath, [helper, "--abort", failedSpool, remote], { cwd: work, env, timeout: 30_000 });
    await assert.rejects(stat(failedSpoolPath), { code: "ENOENT" });
    assert.equal(JSON.parse(await expect(await post({ ...badStart, lookup: true }), 200)), null);
    assert.equal((await git("ls-remote", "origin", tag)).stdout.trim(), `${tagOid}\t${tag}`);

    await git("switch", "-c", "discarded-review");
    await writeFile(join(work, "recovery.txt"), "A historical line retained after compaction.\n");
    await git("add", "recovery.txt");
    await git("commit", "-m", "Review this distinct Git object");
    const oldCommit = (await git("rev-parse", "HEAD")).stdout.trim();
    await git("push", remote, "discarded-review");
    const pull = await view(`${created.location}/pulls/new`, { title: "Preserve review history",
      base_branch: "main", head_branch: "discarded-review" });
    assert.equal(pull.status, 302, pull.text);
    const review = await view(`${pull.location}/reviews`, { decision: "line_comment", comparison_version: "1",
      path: "recovery.txt", line: "new:1", body: "Pin the object outside all current refs." });
    assert.equal(review.status, 302, review.text);
    const reviewBody = (await view(review.location)).text;
    const historical = [...reviewBody.matchAll(/href="([^\"]+)"/g)]
      .map(([, link]) => link.replaceAll("&amp;", "&"))
      .find(link => link.includes("review=") && link.includes("file=recovery.txt"));
    assert.ok(historical);
    const historyPath = new URL(historical, baseUrl + pull.location);
    const checkHistory = async () => {
      const contents = await view(historyPath.pathname + historyPath.search);
      assert.equal(contents.status, 200);
      assert.match(contents.text, /A historical line retained after compaction/);
      assert.match(contents.text, new RegExp(oldCommit));
    };
    await git("switch", "main");
    await git("push", remote, ":refs/heads/discarded-review");

    const operatorToken = randomBytes(32).toString("hex");
    const recovery = async (path, options = {}, credential = operatorToken) => recoveryWorker.fetch(
      new Request(`http://127.0.0.1${path}`, { ...options, headers: { Authorization: `Bearer ${credential}` } }), {
        DB: await mf.getD1Database("DB"), GIT_R2_REPOS: await mf.getDurableObjectNamespace("GIT_R2_REPOS"),
        SHARD_R2_RECOVERY_TOKEN: operatorToken,
      });
    const manifestPath = `/repositories/${id}`;
    const readManifest = async () => JSON.parse(await expect(await recovery(manifestPath), 200));
    await expect(await recovery(manifestPath, {}, "invalid-token"), 401);
    await expect(await recovery("/repositories/shard-missing"), 404);
    let beforeCompact = await readManifest();
    for (let index = 0; beforeCompact.packs.length < 8 && index < 10; index++) {
      await writeFile(join(work, "notes.txt"), `Pack generation ${index}\n`);
      await git("add", "notes.txt");
      await git("commit", "-m", `Small push ${index}`);
      await git("push", remote, "main");
      beforeCompact = await readManifest();
    }
    assert.ok(beforeCompact.packs.length >= 8);
    const latest = (await git("rev-parse", "HEAD")).stdout.trim();
    const concurrentRef = "refs/heads/during-compaction";
    const raced = Promise.withResolvers();
    relay.afterNextIndex(async () => {
      try {
        assert.deepEqual(await readManifest(), beforeCompact);
        await git("push", remote, `${latest}:${concurrentRef}`);
        raced.resolve();
      } catch (error) {
        raced.reject(error);
        throw error;
      }
    });
    let raceTimer;
    try {
      await Promise.race([raced.promise, new Promise((_, reject) => {
        raceTimer = setTimeout(() => reject(new Error("Pack compaction did not start")), 60_000);
      })]);
    } finally {
      clearTimeout(raceTimer);
    }
    const compacted = await waitFor(async () => {
      const current = await readManifest();
      return current.packs.length < beforeCompact.packs.length ? current : null;
    });
    assert.ok(compacted.refs.some(ref => ref.name === concurrentRef && ref.oid === latest));
    assert.ok(!compacted.refs.some(ref => ref.oid === oldCommit));
    assert.ok(compacted.packs.some(key => !beforeCompact.packs.includes(key)));
    for (const key of beforeCompact.packs) assert.ok(await bucket.head(`${key}.idx`));
    await checkHistory();

    const lfsBytes = Buffer.from("The original bytes and the fork have separate LFS keys.\n");
    const object = { oid: createHash("sha256").update(lfsBytes).digest("hex"), size: lfsBytes.length };
    const lfs = async (repo, operation) => {
      const response = await fetch(`${repo}/info/lfs/objects/batch`, {
        method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/vnd.git-lfs+json" },
        body: JSON.stringify({ operation, transfers: ["basic"], objects: [object] }),
      });
      return JSON.parse(await expect(response, 200)).objects[0].actions[operation];
    };
    const upload = await lfs(remote, "upload");
    await expect(await fetch(upload.href, { method: "PUT", headers: { ...upload.header, Authorization: `Bearer ${token}` },
      body: lfsBytes }), 200);
    const forkPath = "/owner/large-fork";
    let fork = await view(`${created.location}/fork`, { name: "large-fork" });
    assert.equal(fork.status, 302, fork.text);
    if (fork.location !== forkPath) {
      const job = fork.location;
      assert.match(job, /^\/user\/settings\/repository-copies\/\d+$/);
      fork = await waitFor(async () => {
        const status = await view(job);
        return status.status === 202 ? null : status;
      });
    }
    assert.equal(fork.location, forkPath, fork.text);

    const { repositories } = JSON.parse(await expect(await recovery("/repositories"), 200));
    assert.equal(repositories.length, 2);
    const manifests = new Map();
    for (const storageId of repositories) {
      const path = `/repositories/${storageId}`;
      manifests.set(path, JSON.parse(await expect(await recovery(path), 200)));
    }
    const restoreManifest = (path, manifest) => recovery(path, { method: "PUT", body: JSON.stringify(manifest) });
    await expect(await restoreManifest(manifestPath, { ...compacted, revision: compacted.revision - 1 }), 409);
    await worker.dispose();

    const restored = join(temporary, "restored");
    await mkdir(join(restored, "v3"), { recursive: true });
    for (const service of ["d1", "r2"]) {
      await cp(join(persistence, "v3", service), join(restored, "v3", service), { recursive: true });
    }
    await start(port, restored);
    const restoredBucket = await mf.getR2Bucket("GIT_PACKS");
    const sourceManifest = manifests.get(manifestPath);
    await expect(await restoreManifest(manifestPath, { ...sourceManifest, head: "refs/heads/.bad" }), 422);
    await expect(await recovery(manifestPath), 404);
    const missingIndex = `${sourceManifest.packs[0]}.idx`;
    const indexBytes = await (await restoredBucket.get(missingIndex)).arrayBuffer();
    await restoredBucket.delete(missingIndex);
    await expect(await restoreManifest(manifestPath, sourceManifest), 422);
    await expect(await recovery(manifestPath), 404);
    await restoredBucket.put(missingIndex, indexBytes);
    for (const [path, manifest] of manifests) {
      await expect(await restoreManifest(path, manifest), 201);
      await expect(await restoreManifest(path, manifest), 204);
      assert.deepEqual(JSON.parse(await expect(await recovery(path), 200)), manifest);
    }
    await checkHistory();

    for (const path of [created.location, forkPath]) {
      const address = `${baseUrl}${path}.git`;
      await expect(await fetch(`${address}/info/refs?service=git-upload-pack`), 401);
      const destination = join(temporary, `restored-${path.split("/").at(-1)}`);
      await git("--config-env=http.extraHeader=SHARD_TEST_GIT_AUTH", "clone", address, destination);
      assert.equal((await git("-C", destination, "rev-parse", "HEAD")).stdout.trim(), latest);
      await git("-C", destination, "fsck", "--full");
      const download = await lfs(address, "download");
      const reply = await fetch(download.href, { headers: { Authorization: `Bearer ${readToken}` } });
      assert.equal(reply.status, 200);
      assert.deepEqual(Buffer.from(await reply.arrayBuffer()), lfsBytes);
    }
  } catch (error) {
    t.diagnostic(`R2 relay errors: ${relay.errors.join(", ") || "none"}`);
    throw error;
  } finally {
    await worker?.dispose().catch(() => {});
    relay.server.closeAllConnections();
    await new Promise(resolveClose => relay.server.close(resolveClose));
    await rm(temporary, { force: true, recursive: true, maxRetries: 3 });
  }
});
