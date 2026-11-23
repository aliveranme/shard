import { spawn } from "node:child_process";
import { createHash, timingSafeEqual } from "node:crypto";
import { once } from "node:events";
import { createReadStream } from "node:fs";
import { appendFile, mkdir, mkdtemp, readdir, realpath, rm, rmdir, stat, statfs, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import {
  createGitOrigin, gitOutput, HttpError, readBody, sendJson, sendStatus, WORKSPACE_REPO_CONFIG,
} from "./git-origin.mjs";

const PORT = 8793;
const ID = "shard-[A-Za-z0-9._-]{1,245}";
const UUID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const WORKSPACE_ROUTE = new RegExp(`^/v1/workspaces/(${UUID})(/.*)?$`);
const CONTROL_ROUTE = new RegExp(`^/(receive|capture|compact)/(${ID})$`);
const REPO_ID = new RegExp(`^${ID}$`);
const PACK_KEY = new RegExp(`^git-packs/(${ID})/${UUID}/pack-[0-9a-f]{40}$`);
const INCOMING_KEY = new RegExp(`^git-incoming/(${ID})/${UUID}$`);
const PACK_FILE = /^pack-[0-9a-f]{40}\.pack$/;
const OID = /^(?!0{40}$)[0-9a-f]{40}$/;
const MAX_CONTROL_BYTES = 4 * 1024 * 1024;
const MAX_REPOSITORIES = 8;
const MAX_COMPACT_BYTES = 256 * 1024 * 1024;
const FUSE_SUPER_MAGIC = 0x65735546;
const UTF8 = new TextDecoder("utf-8", { fatal: true });

function validRef(name) {
  return typeof name === "string" && name.isWellFormed() && Buffer.byteLength(name) <= 4096 &&
    name.startsWith("refs/") && !/[\x00-\x20\x7f~^:?*\[\\]/.test(name) &&
    !name.includes("..") && !name.includes("@{") && !name.endsWith(".") &&
    name.split("/").every((part) => part && !part.startsWith(".") && !part.endsWith(".lock"));
}

function validHead(name) {
  return validRef(name) && name.startsWith("refs/heads/");
}

function validateRepositories(repositories) {
  if (!Array.isArray(repositories) || repositories.length > MAX_REPOSITORIES) throw new HttpError(422);
  const ids = new Set();
  for (const repository of repositories) {
    const { id, head, refs, packs } = repository ?? {};
    if (typeof id !== "string" || !REPO_ID.test(id) || ids.has(id) || !validHead(head) ||
        !Array.isArray(refs) || !Array.isArray(packs)) throw new HttpError(422);
    ids.add(id);
    const names = new Set();
    for (const ref of refs) {
      const { name, oid } = ref ?? {};
      if (!validRef(name) || typeof oid !== "string" || !OID.test(oid) || names.has(name)) throw new HttpError(422);
      names.add(name);
    }
    for (const key of packs) {
      if (typeof key !== "string" || PACK_KEY.exec(key)?.[1] !== id) throw new HttpError(422);
    }
  }
}

async function hydrateRepo(cwd, mount, { id, head, refs, packs }, signal) {
  const repo = join(cwd, `${id}.git`);
  await gitOutput("init", ["--bare", "--object-format=sha1", "--template=", repo], { cwd, signal });
  await appendFile(join(repo, "config"), WORKSPACE_REPO_CONFIG);
  await writeFile(join(repo, "HEAD"), `ref: ${head}\n`);
  if (refs.length) {
    const lines = refs.map(({ name, oid }) => `${oid} ${name}\n`).join("");
    await writeFile(join(repo, "packed-refs"), lines);
  }
  if (!packs.length) return;

  const alternate = join(cwd, ".alternates", id);
  const directory = join(alternate, "pack");
  await mkdir(directory, { recursive: true });
  const linked = new Set();
  for (const pack of packs) {
    signal.throwIfAborted();
    const name = basename(pack);
    if (linked.has(name)) continue;
    linked.add(name);
    for (const extension of [".pack", ".idx"]) {
      await symlink(join(mount, pack + extension), join(directory, name + extension));
    }
  }
  await writeFile(join(repo, "objects", "info", "alternates"), `${alternate}\n`);
}

async function compact(workspace, id, mount, req, signal) {
  const { packs } = await readBody(req, MAX_CONTROL_BYTES);
  if (!Array.isArray(packs) || packs.length < 2 || packs.length > 16 ||
      packs.some(key => typeof key !== "string" || PACK_KEY.exec(key)?.[1] !== id)) throw new HttpError(422);
  const cwd = workspace.directory;
  const repo = join(cwd, `${id}.git`);
  const disk = await statfs(repo);
  if (disk.bavail * disk.bsize < 2 * MAX_COMPACT_BYTES) throw new HttpError(507);

  let bytes = 0;
  for (const key of packs) {
    for (const suffix of [".pack", ".idx"]) {
      const file = await stat(join(mount, key + suffix));
      bytes += file.size;
      if (!file.isFile() || bytes > MAX_COMPACT_BYTES) throw new HttpError(413);
    }
  }
  // stdin-packs includes every object, including commits no current ref reaches.
  const input = packs.map(key => `${basename(key)}.pack\n`).join("");
  const base = join(repo, "objects", "pack", "pack");
  const output = await gitOutput("pack-objects", ["--stdin-packs", "--window=0", "--delta-base-offset", base],
    { cwd, repo, input, signal, maxBytes: 128 });
  if (!OID.test(UTF8.decode(output).trim())) throw new Error("Git pack compaction produced no pack");
}

async function capture(workspace, job, id, upload, signal) {
  if (workspace.captureBlocked) throw new HttpError(409);
  const cwd = workspace.directory;
  const repo = join(cwd, `${id}.git`);
  let entries;
  try {
    entries = await readdir(join(repo, "objects"), { withFileTypes: true });
  } catch (error) {
    if (error.code === "ENOENT") throw new HttpError(404);
    throw error;
  }
  const options = { cwd, repo, signal };
  const head = UTF8.decode(await gitOutput("symbolic-ref", ["HEAD"], options)).replace(/\n$/, "");
  if (!validHead(head)) throw new HttpError(422);
  const output = UTF8.decode(await gitOutput("for-each-ref", ["--format=%(objectname) %(refname)"], options));
  const refs = [];
  for (const line of output.split("\n")) {
    if (!line) continue;
    const oid = line.slice(0, 40);
    const name = line.slice(41);
    if (!OID.test(oid) || line[40] !== " " || !validRef(name)) throw new HttpError(422);
    refs.push({ name, oid });
  }

  if (entries.some((entry) => entry.isDirectory() && /^[0-9a-f]{2}$/.test(entry.name))) {
    // Incremental repack keeps existing packs and excludes objects in the alternate.
    await gitOutput("repack", ["--local", "-n", "--quiet"], options);
  }
  const packDirectory = join(repo, "objects", "pack");
  const files = await readdir(packDirectory, { withFileTypes: true });
  const names = new Set(files.filter((entry) => entry.isFile()).map((entry) => entry.name));
  const bases = [];
  for (const name of names) {
    if (!PACK_FILE.test(name)) continue;
    const base = name.slice(0, -5);
    if (!names.has(`${base}.idx`)) throw new Error("Git pack index is missing");
    bases.push(base);
  }
  bases.sort();
  const packs = bases.map((base) => `git-packs/${id}/${job}/${base}`);
  const body = JSON.stringify({ head, refs, packs });
  if (Buffer.byteLength(body) > MAX_CONTROL_BYTES) throw new HttpError(413);
  for (const [index, base] of bases.entries()) {
    for (const extension of [".pack", ".idx"]) {
      await upload(join(packDirectory, base + extension), packs[index] + extension, signal);
    }
  }
  return body;
}

async function receiveStaged(workspace, id, mount, req, res, signal) {
  const { key, size, sha256 } = await readBody(req);
  if (typeof key !== "string" || INCOMING_KEY.exec(key)?.[1] !== id || !Number.isSafeInteger(size) || size < 1 ||
      typeof sha256 !== "string" || !/^[0-9a-f]{64}$/.test(sha256)) throw new HttpError(422);
  signal.throwIfAborted();
  if (workspace.stagedReceive) throw new HttpError(409);
  workspace.stagedReceive = true;
  workspace.captureBlocked = true;
  const report = join(workspace.directory, ".receive-report");
  const { promise: verified, resolve: finish } = Promise.withResolvers();
  async function* verifiedInput() {
    const hash = createHash("sha256");
    let count = 0;
    let failure = new HttpError(422);
    try {
      for await (const chunk of createReadStream(join(mount, key), { signal })) {
        count += chunk.length;
        if (count > size) return;
        hash.update(chunk);
        yield chunk;
      }
      if (count === size && hash.digest("hex") === sha256) failure = null;
    } catch (error) {
      failure = error;
    } finally {
      finish(failure);
    }
  }
  const input = Object.assign(Readable.from(verifiedInput(), { objectMode: false }), {
    method: "POST",
    httpVersion: req.httpVersion,
    headers: {
      "content-type": "application/x-git-receive-pack-request",
      "content-length": String(size),
    },
    headersDistinct: { authorization: req.headersDistinct.authorization },
  });
  try {
    await workspace.origin(input, res, {
      url: `/v1/git/${id}.git/git-receive-pack`,
      stagedReceive: {
        report,
        async check() {
          const error = await verified;
          if (error) throw error;
        },
      },
    });
    workspace.captureBlocked = Boolean(await verified) || res.statusCode !== 200 || !res.writableEnded;
  } finally {
    await rm(report, { force: true });
  }
}

export async function createR2GitOrigin({ directory, mount, token, importHosts, upload }) {
  if (typeof token !== "string" || !/^[\x21-\x7e]+$/.test(token)) throw new TypeError("A Git token is required");
  directory = await realpath(directory);
  mount = await realpath(mount);
  const expectedToken = createHash("sha256").update(`Bearer ${token}`).digest();
  const workspaces = new Map();

  async function handle(req, res, signal) {
    const auth = req.headersDistinct.authorization;
    if (auth?.length !== 1 || !timingSafeEqual(createHash("sha256").update(auth[0]).digest(), expectedToken)) {
      res.setHeader("WWW-Authenticate", "Bearer");
      throw new HttpError(401);
    }
    if (req.method === "GET" && req.url === "/healthz") return sendJson(res, 200, { protocol: 1 });
    const route = WORKSPACE_ROUTE.exec(req.url);
    if (!route) throw new HttpError(404);
    const [, job, path] = route;
    if (!path && req.method === "POST") {
      const { repositories } = await readBody(req, MAX_CONTROL_BYTES);
      validateRepositories(repositories);
      signal.throwIfAborted();
      if (workspaces.has(job)) throw new HttpError(409);
      const workspace = { directory: join(directory, job) };
      workspaces.set(job, workspace);
      const ready = Promise.withResolvers();
      workspace.tail = ready.promise;
      try {
        await mkdir(workspace.directory, { mode: 0o700 });
        for (const repo of repositories) await hydrateRepo(workspace.directory, mount, repo, signal);
        const origin = await createGitOrigin({ directory: workspace.directory, token, importHosts, workspaceOnly: true });
        signal.throwIfAborted();
        workspace.origin = origin;
        sendStatus(res, 204);
      } finally {
        try {
          if (!workspace.origin) await dispose(job, workspace);
        } finally {
          ready.resolve();
        }
      }
      return;
    }

    const deleting = !path && req.method === "DELETE";
    const workspace = workspaces.get(job);
    if (!workspace && deleting) return sendStatus(res, 204);
    if (!workspace) throw new HttpError(404);
    const previous = workspace.tail;
    const next = Promise.withResolvers();
    workspace.tail = next.promise;
    try {
      // Native receive-pack can finish its ref transaction after the client disconnects.
      await previous;
      if (deleting) {
        if (workspaces.get(job) === workspace) await dispose(job, workspace);
        if (!signal.aborted) return sendStatus(res, 204);
        return;
      }
      signal.throwIfAborted();
      if (workspaces.get(job) !== workspace || !workspace.origin) throw new HttpError(404);
      const control = req.method === "POST" && CONTROL_ROUTE.exec(path ?? "");
      if (control) {
        const [, action, id] = control;
        if (action === "receive") return await receiveStaged(workspace, id, mount, req, res, signal);
        if (action === "compact") {
          await compact(workspace, id, mount, req, signal);
          return sendStatus(res, 204);
        }
        const body = await capture(workspace, job, id, upload, signal);
        res.setHeader("Content-Type", "application/json; charset=utf-8");
        res.end(body);
        return;
      }
      if (path?.startsWith("/v1/repos/") || path?.startsWith("/v1/git/")) {
        return await workspace.origin(req, res, { url: path });
      }
      throw new HttpError(404);
    } finally {
      next.resolve();
    }
  }

  async function dispose(job, workspace) {
    await rm(workspace.directory, { recursive: true, force: true, maxRetries: 3 });
    workspaces.delete(job);
  }

  return createServer({ requestTimeout: 0, headersTimeout: 60_000 }, (req, res) => {
    const cancellation = new AbortController();
    req.on("error", () => cancellation.abort());
    res.on("error", () => cancellation.abort());
    res.once("close", () => { if (!res.writableFinished) cancellation.abort(); });
    res.setHeader("Cache-Control", "no-store");
    void handle(req, res, cancellation.signal).catch((error) => {
      if (cancellation.signal.aborted) return;
      const expected = error instanceof HttpError;
      if (!expected) console.error("R2 Git workspace operation failed");
      if (res.headersSent) res.destroy();
      else if (!res.destroyed) sendStatus(res, expected ? error.status : 502);
    }).finally(() => req.resume());
  });
}

function rcloneConfig(env) {
  const endpoint = new URL(env.SHARD_GIT_R2_ENDPOINT);
  const local = ["127.0.0.1", "[::1]", "localhost", "host.docker.internal"].includes(endpoint.hostname) ||
    endpoint.pathname === "/cdn-cgi/local/r2/s3" && /^172\.(?:1[6-9]|2[0-9]|3[01])\.\d+\.\d+$/.test(endpoint.hostname);
  if (endpoint.username || endpoint.password || endpoint.search || endpoint.hash ||
      !(endpoint.protocol === "https:" || local && endpoint.protocol === "http:")) throw new Error("Invalid R2 endpoint");
  const bucket = env.SHARD_GIT_R2_BUCKET;
  const accessKey = env.SHARD_GIT_R2_ACCESS_KEY_ID;
  const secret = env.SHARD_GIT_R2_SECRET_ACCESS_KEY;
  if (!bucket || !/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(bucket) || !accessKey || !secret) {
    throw new Error("Missing R2 bucket configuration");
  }
  return {
    remote: `shard:${bucket}`,
    env: {
      PATH: env.PATH,
      RCLONE_CONFIG_SHARD_TYPE: "s3",
      RCLONE_CONFIG_SHARD_PROVIDER: "Cloudflare",
      RCLONE_CONFIG_SHARD_ENDPOINT: endpoint.href,
      RCLONE_CONFIG_SHARD_REGION: "auto",
      RCLONE_CONFIG_SHARD_ACCESS_KEY_ID: accessKey,
      RCLONE_CONFIG_SHARD_SECRET_ACCESS_KEY: secret,
    },
  };
}

function startRclone(args, env, signal) {
  const child = spawn("rclone", ["--config", "/dev/null", "--s3-no-check-bucket", ...args], {
    env, signal, stdio: "ignore",
  });
  child.on("error", () => {});
  return { child, closed: new Promise((resolve) => child.once("close", resolve)) };
}

async function waitForMount(path, closed) {
  const deadline = Date.now() + 20_000;
  do {
    if (await Promise.race([closed.then(() => true), delay(100, false)])) throw new Error("R2 mount exited");
    if ((await statfs(path)).type === FUSE_SUPER_MAGIC) return;
  } while (Date.now() < deadline);
  throw new Error("R2 mount did not become ready");
}

async function main() {
  const config = rcloneConfig(process.env);
  const scratch = await mkdtemp(join(tmpdir(), "shard-r2-"));
  const mount = join(scratch, "bucket");
  const directory = join(scratch, "workspaces");
  let mounted;
  let server;
  const stop = () => mounted?.child.kill();
  try {
    await mkdir(mount, { mode: 0o700 });
    await mkdir(directory, { mode: 0o700 });
    mounted = startRclone([
      "mount", config.remote, mount, "--read-only", "--vfs-cache-mode", "off", "--dir-cache-time", "0s",
      "--attr-timeout", "0s", "--buffer-size", "1M", "--vfs-read-chunk-size", "8M", "--vfs-read-chunk-size-limit", "128M",
      "--contimeout", "10s", "--timeout", "1m", "--low-level-retries", "2",
    ], config.env);
    process.once("SIGTERM", stop);
    process.once("SIGINT", stop);
    await waitForMount(mount, mounted.closed);
    server = await createR2GitOrigin({
      directory, mount, token: process.env.SHARD_GIT_TOKEN,
      importHosts: process.env.SHARD_GIT_IMPORT_HOSTS?.split(",").filter((host) => host.trim()),
      async upload(source, key, signal) {
        const transfer = startRclone([
          "copyto", source, `${config.remote}/${key}`, "--immutable", "--no-traverse", "--s3-upload-cutoff", "32M",
          "--s3-chunk-size", "16M", "--s3-upload-concurrency", "2",
          "--contimeout", "10s", "--timeout", "1m", "--low-level-retries", "2", "--retries", "2",
        ], config.env, signal);
        if (await transfer.closed !== 0) throw new Error("R2 Git pack upload failed");
      },
    });
    server.listen(PORT, "0.0.0.0");
    await once(server, "listening");
    console.log(`R2 Git origin listening on port ${PORT}`);
    if (await mounted.closed !== 0) throw new Error("R2 mount exited");
  } finally {
    process.off("SIGTERM", stop);
    process.off("SIGINT", stop);
    server?.close();
    server?.closeAllConnections();
    mounted?.child.kill();
    await mounted?.closed;
    await rm(directory, { recursive: true, force: true });
    // Never recurse into the FUSE directory, including when unmount fails.
    await rmdir(mount).catch((error) => { if (error.code !== "ENOENT") throw error; });
    await rmdir(scratch);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => {
    console.error("R2 Git origin stopped. Check the FUSE device and R2 settings.");
    process.exitCode = 1;
  });
}
