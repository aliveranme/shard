import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { chmod, copyFile, mkdir, mkdtemp, readdir, rm, stat, writeFile } from "node:fs/promises";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { experimental_readRawConfig, unstable_startWorker } from "wrangler";

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

// Miniflare implements signed UploadPart and completion but returns 501 for
// ListParts. Keep the listing in this relay from successful PUTs only.
function s3Relay(workerPort) {
  const received = new Map();
  const incomingPuts = [];
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

test("R2 Git resumes signed multipart uploads across restarts and serves stock Git", {
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
    const start = async (port = 0) => {
      worker = await unstable_startWorker({ config: configPath, dev: {
        remote: false, watch: false, enableContainers: true, persist: persistence,
        containerBuildId: randomBytes(4).toString("hex"),
        routeRequestsByRoutes: true, inferOriginFromRoutes: false,
        server: { hostname: "127.0.0.1", port }, inspector: false, logLevel: "log",
      } });
      const runtime = worker.raw.runtimes[0];
      if (!runtime.mf) {
        await once(worker.raw, "reloadComplete", { signal: AbortSignal.timeout(90_000) });
      }
      // The public Wrangler proxy checks the Host before forwarding to Miniflare.
      s3Port = Number((await runtime.mf.ready).port);
      return await worker.url;
    };
    const address = await start();
    const port = Number(address.port);
    const baseUrl = address.origin;
    const view = browser(baseUrl);
    await view("/user/sign_up");
    const signup = await view("/user/sign_up", { username: "owner", email: "owner@example.test", password: "local-r2-test-password" });
    assert.equal(signup.status, 302, signup.text);
    await view("/user/settings/tokens");
    const token = (await view("/user/settings/tokens", { name: "R2 test push", scope: "write", expires_in: "7" })).text
      .match(/\bshard_[a-f0-9]{64}\b/)?.[0];
    assert.ok(token);
    const readToken = (await view("/user/settings/tokens", { name: "R2 test read", scope: "read", expires_in: "7" })).text
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
    const spoolId = spool.match(/-([a-f0-9]{64}-[a-f0-9]{24})\.receive$/)?.[1];
    assert.ok(spoolId);
    const startUpload = { action: "start", sha256: spoolId.slice(0, 64), size: (await stat(join(directory, spool))).size };
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
