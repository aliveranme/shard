import { DurableObject } from "cloudflare:workers";
import { PackIndex, parseCommit, parseTagTarget, parseTree, readPackedObject } from "./git-r2-pack.mjs";
import { buildR2PackStore } from "./git-r2-packstore.mjs";
import { ArchiveLimitError, ArchiveUnsupportedError, buildSourceTar } from "./git-r2-archive.mjs";
import { parseReceiveBytes, parseReceivedPack, PackTooLargeError, ThinPackError } from "./git-r2-receive.mjs";
import { renderStatus, uploadPack } from "../node_modules/durable-git/src/git/protocol.ts";
import { gitR2S3 } from "../src/lfs_s3.js";
import { shard_receive_report, shard_receive_request } from "../src/git_wire.js";

const CONTROL = /^\/v1\/repos\/(shard-[A-Za-z0-9._-]{1,122})(?:\/(log|commit|commit-raw|tree|blob|file|archive|ref|head|merge|merge-base|compare|integrated))?$/;
const SMART = /^\/v1\/git\/(shard-[A-Za-z0-9._-]{1,122})\.git\/(info\/refs|git-upload-pack|git-receive-pack)$/;
const STORAGE = /^\/v1\/r2\/(shard-[A-Za-z0-9._-]{1,122})\/(snapshot|multipart|publish|recovery)$/;
const ID = /^shard-[A-Za-z0-9._-]{1,122}$/;
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const ACTOR = /^[1-9][0-9]{0,18}$/;
const OID = /^[a-f0-9]{40}$/;
const SHA256 = /^[a-f0-9]{64}$/;
const UTF8 = new TextEncoder();
const ZERO = "0".repeat(40);
const MiB = 1024 ** 2;
const MAX_INPUT = (5 * 1024 - 5) * 1024 ** 3;
const MAX_PARTS = 10_000;
const MAX_INFO = 4 * MiB;
const MAX_REPORT = MiB;
const COMMAND_LIMIT = 8 * 1024;
const SESSION_MS = 6 * 24 * 60 * 60_000;
const WORK_MS = 60 * 60_000;
const JOB_MS = WORK_MS + 5 * 60_000;
const COMPACT_AFTER = 15_000;
const COMPACT_RETRY_AFTER = 5 * 60_000;
const COMPACT_TIMEOUT = 2 * 60_000;
const COMPACT_MIN_PACKS = 8;
const COMPACT_MAX_PACKS = 16;
const COMPACT_MAX_BYTES = 256 * MiB;
const RETIRED_PACK_MS = 24 * 60 * 60_000;
const STARTUP_MS = 45_000;
const PORT = 8793;
const NO_STORE = { "Cache-Control": "no-store" };
const DIRECT_ACTIONS = new Set(["archive", "blob", "commit", "commit-raw", "file", "log", "tree"]);
const MAX_DIRECT_INDEX = 32 * MiB;
const MAX_DIRECT_FILE = 32 * MiB;
const MAX_DIRECT_COMMIT = 256 * 1024;
const MAX_DIRECT_CACHE = MiB;
const MAX_OBJECT_CACHE = 32 * MiB;
const MAX_DIRECT_UPLOAD = MiB;
const MAX_DIRECT_OBJECTS = 200_000;
const MAX_DIRECT_PUSH = 16 * MiB;
const OBJECT_TYPES = [null, "commit", "tree", "blob", "tag"];
const REPORT_TYPE = "application/x-git-receive-pack-result";
const INGEST_TYPE = "application/x-git-receive-pack-request";

class GitHttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function json(body, status = 200) {
  return Response.json(body, { status, headers: NO_STORE });
}

function empty(status) {
  return new Response(null, { status, headers: NO_STORE });
}

function result(bytes, replayed = false) {
  return new Response(bytes, { headers: { ...NO_STORE, "Content-Type": REPORT_TYPE,
    ...(replayed && { "X-Shard-Replayed-Push": "1" }) } });
}

function busy() {
  return Response.json({ message: "Another Git write is in progress" }, {
    status: 425, headers: { ...NO_STORE, "Retry-After": "2" },
  });
}

async function bytes(body, max, errorStatus) {
  if (!body) return new Uint8Array();
  const chunks = [];
  let length = 0;
  for await (const chunk of body) {
    length += chunk.byteLength;
    if (length > max) throw new GitHttpError(errorStatus, "Git control response or request exceeds the size limit");
    chunks.push(chunk);
  }
  if (chunks.length === 1 && chunks[0].byteLength === chunks[0].buffer.byteLength) return chunks[0];
  const data = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return data;
}

async function control(body, limit = COMMAND_LIMIT, errorStatus = 400) {
  const data = await bytes(body, limit, errorStatus === 400 ? 413 : errorStatus);
  try {
    const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(data));
    if (value && typeof value === "object" && !Array.isArray(value)) return value;
  } catch {
    // Only a JSON object is valid for an internal command or result.
  }
  throw new GitHttpError(errorStatus, "Invalid Git control message");
}

function packPrefix(id, job = "") {
  return `git-packs/${id}/${job ? `${job}/` : ""}`;
}

function incomingKey(id, upload) {
  return `git-incoming/${id}/${upload}`;
}

function partLength(upload, number) {
  const start = (number - 1) * upload.part_bytes;
  return Number.isSafeInteger(number) && number > 0 && start < upload.size
    ? Math.min(upload.part_bytes, upload.size - start) : 0;
}

function validRef(name) {
  return typeof name === "string" && name.startsWith("refs/") && name.isWellFormed() && UTF8.encode(name).byteLength <= 4096 &&
    !/[\x00-\x20\x7f~^:?*\[\\]/.test(name) && !name.includes("..") && !name.includes("@{") && !name.endsWith(".") &&
    name.split("/").every(part => part && !part.startsWith(".") && !part.endsWith(".lock"));
}

function validSnapshot(snapshot, validPack) {
  return validRef(snapshot.head) && snapshot.head.startsWith("refs/heads/")
    && Array.isArray(snapshot.refs) && Array.isArray(snapshot.packs)
    && snapshot.refs.every(ref => validRef(ref?.name) && OID.test(ref.oid) && ref.oid !== ZERO)
    && new Set(snapshot.refs.map(ref => ref.name)).size === snapshot.refs.length
    && new Set(snapshot.packs).size === snapshot.packs.length
    && snapshot.packs.every(key => typeof key === "string" && validPack(key));
}

function packKeyFor(key, id) {
  const prefix = packPrefix(id);
  if (!key.startsWith(prefix)) return false;
  const [job, name, ...extra] = key.slice(prefix.length).split("/");
  return !extra.length && UUID.test(job) && /^pack-[a-f0-9]{40}$/.test(name);
}

function changedRefs(before, after, updates, reported) {
  const old = new Map(before.refs.map(({ name, oid }) => [name, oid]));
  const next = new Map(after.refs.map(({ name, oid }) => [name, oid]));
  const names = new Set(reported);
  const accepted = [];
  for (const update of updates) {
    const was = old.get(update.name) ?? ZERO;
    const now = next.get(update.name) ?? ZERO;
    if (was === now && !names.has(update.name)) continue;
    if (was !== update.before || now !== update.after || reported && !names.delete(update.name)) {
      throw new Error("Unexpected Git receive ref result");
    }
    accepted.push(update.name);
    if (now === ZERO) old.delete(update.name);
    else old.set(update.name, now);
  }
  if (names.size || old.size !== next.size || [...old].some(([name, oid]) => next.get(name) !== oid) || before.head !== after.head) {
    throw new Error("Unexpected Git receive snapshot");
  }
  return accepted;
}

export class R2GitRepository extends DurableObject {
  #sql;
  #bucket;
  #s3;
  #writer = false;
  #active = 0;
  #compacting = false;
  #ready;
  #sessionToken;
  #directReads;
  #indexes = new Map();
  #counts = new Map();
  #packStores = new Map();

  constructor(ctx, env) {
    super(ctx, env);
    this.#sql = ctx.storage.sql;
    this.#bucket = env.GIT_PACKS;
    this.#directReads = env.SHARD_R2_DIRECT_READS !== "0";
    this.#sql.exec(`
      CREATE TABLE IF NOT EXISTS object_cache (
        oid TEXT PRIMARY KEY, type INTEGER NOT NULL, size INTEGER NOT NULL, data BLOB NOT NULL
      );
      CREATE TABLE IF NOT EXISTS repository (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1), id TEXT NOT NULL,
        revision INTEGER NOT NULL, head TEXT NOT NULL, refs TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS packs (oid TEXT PRIMARY KEY, key TEXT NOT NULL, bytes INTEGER);
      CREATE TABLE IF NOT EXISTS retired_packs (key TEXT PRIMARY KEY, expires INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS uploads (
        id TEXT PRIMARY KEY, actor TEXT NOT NULL, digest TEXT NOT NULL, size INTEGER NOT NULL,
        part_bytes INTEGER NOT NULL, expires INTEGER NOT NULL, upload TEXT, report BLOB,
        aborted INTEGER NOT NULL DEFAULT 0,
        UNIQUE (actor, digest, size)
      );
      CREATE TABLE IF NOT EXISTS work (
        id TEXT PRIMARY KEY, repo TEXT NOT NULL, revision INTEGER NOT NULL, expires INTEGER NOT NULL,
        actor TEXT, upload TEXT, candidate TEXT, updates TEXT, accepted TEXT, report BLOB,
        published INTEGER NOT NULL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS work_upload ON work(upload, revision);
    `);
    if (![...this.#sql.exec("PRAGMA table_info(packs)")].some(column => column.name === "bytes")) {
      this.#sql.exec("ALTER TABLE packs ADD COLUMN bytes INTEGER");
    }
    this.#sql.exec("CREATE INDEX IF NOT EXISTS packs_by_size ON packs (bytes, oid)");
    this.#sql.exec("CREATE INDEX IF NOT EXISTS retired_packs_by_expiry ON retired_packs (expires)");
  }

  #one(query, ...params) {
    return this.#sql.exec(query, ...params).next().value;
  }

  #current() {
    return this.#one("SELECT id, revision, head, deleted FROM repository WHERE singleton = 1");
  }

  #repository() {
    const row = this.#one("SELECT id, revision, head, refs, deleted FROM repository WHERE singleton = 1");
    if (!row || row.deleted) throw new GitHttpError(404, "Git repository is unavailable");
    return { id: row.id, revision: row.revision, head: row.head, refs: JSON.parse(row.refs),
      packs: [...this.#sql.exec("SELECT key FROM packs ORDER BY oid")].map(row => row.key) };
  }

  #metadata(snapshot) {
    return { name: snapshot.id, remote: "", defaultBranch: snapshot.head.slice("refs/heads/".length) };
  }

  #refsFor(snapshot, publishedBytes = 0) {
    const refs = JSON.stringify(snapshot.refs);
    const paths = snapshot.packs.reduce((bytes, key) => bytes + key.length + 3, publishedBytes);
    if (UTF8.encode(refs + JSON.stringify(snapshot.head)).byteLength + paths + 512 > MAX_INFO) {
      throw new GitHttpError(413, "Git refs and pack manifest exceed the workspace metadata limit");
    }
    return refs;
  }

  #insert(id, snapshot, revision = 1) {
    this.#sql.exec("INSERT INTO repository (singleton, id, revision, head, refs) VALUES (1, ?, ?, ?, ?)",
      id, revision, snapshot.head, this.#refsFor(snapshot));
    this.#addPacks(snapshot.packs, snapshot.packSizes);
  }

  #addPacks(packs, sizes = []) {
    for (const [index, key] of packs.entries()) {
      this.#sql.exec("INSERT INTO packs (oid, key, bytes) VALUES (?, ?, ?)", key.slice(-40), key, sizes[index] ?? null);
    }
  }

  #update(before, after) {
    const current = this.#current();
    if (current?.deleted || current?.revision !== before) throw new GitHttpError(409, "Git refs changed during preparation");
    const { length } = this.#one("SELECT COALESCE(SUM(length(key) + 3), 0) AS length FROM packs");
    this.#sql.exec("UPDATE repository SET revision = revision + 1, head = ?, refs = ? WHERE singleton = 1",
      after.head, this.#refsFor(after, length));
    this.#addPacks(after.packs, after.packSizes);
  }

  async #restore(id, request) {
    return this.#lock(async () => {
      const manifest = await control(request.body, MAX_INFO);
      if (manifest.format !== 1 || manifest.id !== id || !Number.isSafeInteger(manifest.revision) ||
          manifest.revision < 1 || manifest.revision === Number.MAX_SAFE_INTEGER ||
          !validSnapshot(manifest, key => packKeyFor(key, id))) return empty(422);
      if (new Set(manifest.packs.map(key => key.slice(-40))).size !== manifest.packs.length ||
          (manifest.refs.length && !manifest.packs.length)) return empty(422);
      const existing = this.#current();
      if (existing) {
        if (existing.deleted) return empty(409);
        const current = this.#repository();
        const keys = new Set(current.packs);
        if (current.revision === manifest.revision && current.head === manifest.head &&
            JSON.stringify(current.refs) === JSON.stringify(manifest.refs) && keys.size === manifest.packs.length &&
            manifest.packs.every(key => keys.has(key))) return empty(204);
        return empty(409);
      }

      // Validate every pack/index pair; a failed preflight leaves the object empty.
      const packSizes = {};
      const indexSizes = {};
      const expected = new Set(manifest.packs);
      let needed = expected.size * 2;
      let cursor;
      while (needed) {
        const page = await this.#bucket.list({ prefix: packPrefix(id), limit: 1000, cursor });
        for (const { key, size } of page.objects) {
          const base = key.slice(0, key.lastIndexOf("."));
          if (!expected.has(base)) continue;
          if (key.endsWith(".pack") && size >= 32) { packSizes[base] = size; needed--; }
          if (key.endsWith(".idx") && size > 0) { indexSizes[base] = size; needed--; }
        }
        if (!page.truncated) break;
        cursor = page.cursor;
      }
      if (needed) {
        return json({ message: "Restore the Git pack and index objects before importing the manifest" }, 422);
      }
      const packBytes = manifest.packs.map(key => packSizes[key] + indexSizes[key]);
      this.ctx.storage.transactionSync(() => this.#insert(id, { ...manifest, packSizes: packBytes }, manifest.revision));
      this.ctx.waitUntil(this.#arm());
      return empty(201);
    });
  }

  #inputLimit() {
    const value = Number(this.env.SHARD_GIT_R2_MAX_PUSH_BYTES ?? 2 * 1024 ** 3);
    if (!Number.isSafeInteger(value) || value < 1 || value > MAX_INPUT) throw new Error("Invalid SHARD_GIT_R2_MAX_PUSH_BYTES");
    return value;
  }

  #signer() {
    return this.#s3 ??= gitR2S3(this.env);
  }

  async #lock(operation) {
    if (this.#writer) return busy();
    this.#writer = true;
    try {
      return await operation();
    } finally {
      this.#writer = false;
    }
  }

  async #arm(compactDelay = COMPACT_AFTER) {
    const { next } = this.#one(`SELECT MIN(expires) AS next FROM (
      SELECT expires FROM uploads UNION ALL SELECT expires FROM work UNION ALL SELECT expires FROM retired_packs
    )`);
    const compact = !this.#compacting && this.#compactionDue() ? Date.now() + compactDelay : Infinity;
    if (next === null && compact === Infinity) return;
    const due = Math.max(Math.min(next ?? Infinity, compact), Date.now() + 1_000);
    const existing = await this.ctx.storage.getAlarm();
    if (existing === null || due < existing) await this.ctx.storage.setAlarm(due);
  }

  async #reserve(id, revision, actor = null, upload = null) {
    const job = crypto.randomUUID();
    this.#sql.exec("INSERT INTO work (id, repo, revision, expires, actor, upload) VALUES (?, ?, ?, ?, ?, ?)",
      job, id, revision, Date.now() + JOB_MS, actor, upload);
    await this.#arm();
    return job;
  }

  async #removePage(prefix) {
    const page = await this.#bucket.list({ prefix, limit: 256 });
    if (page.objects.length) await this.#bucket.delete(page.objects.map(object => object.key));
    return !page.truncated;
  }

  async #dropWork(job) {
    if (!job.published && !await this.#removePage(packPrefix(job.repo, job.id))) return false;
    this.#sql.exec("DELETE FROM work WHERE id = ?", job.id);
    return true;
  }

  async #dropUpload(id, upload, repo) {
    this.#sql.exec("UPDATE uploads SET aborted = 1 WHERE id = ?", id);
    const key = incomingKey(repo, id);
    if (upload !== null && !await this.#bucket.head(key)) {
      try {
        await this.#bucket.resumeMultipartUpload(key, upload).abort();
      } catch (error) {
        if (!/\(10024\)$/.test(String(error))) throw error;
      }
    }
    await this.#bucket.delete(key);
    this.#sql.exec("DELETE FROM uploads WHERE id = ?", id);
  }

  #preparedWrite(revision) {
    return this.#one("SELECT 1 FROM work WHERE revision = ? AND published = 0 AND report IS NOT NULL AND expires > ? LIMIT 1",
      revision, Date.now());
  }

  #compactionDue() {
    const { count } = this.#one("SELECT COUNT(*) AS count FROM (SELECT 1 FROM packs LIMIT ?)", COMPACT_MIN_PACKS);
    const repo = this.#current();
    if (count < COMPACT_MIN_PACKS || repo?.deleted !== 0 || this.#preparedWrite(repo.revision)) return false;
    const { small } = this.#one("SELECT COUNT(*) AS small FROM (SELECT 1 FROM packs WHERE bytes IS NULL OR bytes <= ? LIMIT 2)",
      COMPACT_MAX_BYTES / 2);
    return small === 2;
  }

  async #compactPacks() {
    if (this.#compacting || this.#writer || !this.#compactionDue()) return;
    this.#compacting = true;
    try {
      const repository = this.#current();
      for (const { key } of [...this.#sql.exec("SELECT key FROM packs WHERE bytes IS NULL LIMIT 32")]) {
        const [pack, index] = await Promise.all([this.#bucket.head(`${key}.pack`), this.#bucket.head(`${key}.idx`)]);
        if (!pack || !index) throw new Error("Published Git pack or index is unavailable");
        this.#sql.exec("UPDATE packs SET bytes = ? WHERE key = ?", pack.size + index.size, key);
      }
      const keys = [];
      let bytes = 0;
      const rows = this.#sql.exec("SELECT key, bytes FROM packs WHERE bytes <= ? ORDER BY bytes, oid LIMIT ?",
        COMPACT_MAX_BYTES / 2, COMPACT_MAX_PACKS);
      for (const pack of rows) {
        bytes += pack.bytes;
        if (bytes > COMPACT_MAX_BYTES) break;
        keys.push(pack.key);
      }
      if (keys.length < 2) return;

      const { id, revision, head } = repository;
      const job = await this.#reserve(id, revision);
      let workspace;
      let persisted = false;
      try {
        workspace = await this.#workspace(job, [{ id, head, refs: [], packs: keys }]);
        const compacted = await workspace.fetch(`/compact/${id}`, { method: "POST", payload: { packs: keys }, timeout: COMPACT_TIMEOUT });
        await compacted.body?.cancel();
        if (!compacted.ok) throw new Error(`Git pack compaction failed (HTTP ${compacted.status})`);
        const snapshot = await this.#capture(workspace, { id, packs: [] }, job, COMPACT_TIMEOUT);
        if (snapshot.packs.length !== 1) throw new Error("Git compaction did not produce one pack");
        const [output] = snapshot.packs;

        // A prepared push already validated against this revision. Let it publish first.
        const live = this.#current();
        if (this.#writer || !live || live.deleted || live.revision !== revision || this.#preparedWrite(revision)) return;
        const existing = this.#one("SELECT key FROM packs WHERE oid = ?", output.slice(-40));
        // Reuse an identical active pack when Git produces one.
        const keep = existing?.key;
        this.ctx.storage.transactionSync(() => {
          const expires = Date.now() + RETIRED_PACK_MS;
          for (const key of keys) {
            if (key === keep) continue;
            this.#sql.exec("DELETE FROM packs WHERE oid = ?", key.slice(-40));
            this.#sql.exec("INSERT INTO retired_packs (key, expires) VALUES (?, ?)", key, expires);
          }
          if (!keep) {
            this.#addPacks(snapshot.packs, snapshot.packSizes);
            this.#sql.exec("DELETE FROM work WHERE id = ?", job);
          }
          this.#sql.exec("UPDATE repository SET revision = revision + 1 WHERE singleton = 1");
        });
        persisted = !keep;
      } finally {
        await workspace?.close();
        if (!persisted) await this.#dropWork({ id: job, repo: id, published: false });
      }
    } finally {
      this.#compacting = false;
    }
  }

  async alarm() {
    if (this.#writer) {
      await this.ctx.storage.setAlarm(Date.now() + 30_000);
      return;
    }
    await this.#lock(async () => {
      const now = Date.now();
      for (const job of [...this.#sql.exec("SELECT id, repo, published FROM work WHERE expires <= ? ORDER BY expires LIMIT 16", now)]) {
        await this.#dropWork(job);
      }
      const repo = this.#current();
      if (repo) {
        for (const upload of [...this.#sql.exec("SELECT id, upload, report FROM uploads WHERE expires <= ? ORDER BY expires LIMIT 16", now)]) {
          await this.#dropUpload(upload.id, upload.report === null ? upload.upload : null, repo.id);
        }
      }
      const retired = [...this.#sql.exec("SELECT key FROM retired_packs WHERE expires <= ? ORDER BY expires LIMIT 32", now)];
      if (retired.length) {
        await this.#bucket.delete(retired.flatMap(({ key }) => [`${key}.pack`, `${key}.idx`]));
        for (const { key } of retired) this.#sql.exec("DELETE FROM retired_packs WHERE key = ?", key);
      }
    });
    let delay = COMPACT_AFTER;
    try {
      await this.#compactPacks();
    } catch (error) {
      delay = COMPACT_RETRY_AFTER;
      console.error("R2 Git pack compaction failed", error?.message);
    }
    await this.#arm(delay);
  }

  async #startContainer() {
    const instance = this.ctx.container;
    const stored = await this.ctx.storage.get("container-token");
    if (instance.running && !this.#sessionToken && typeof stored === "string") {
      this.#sessionToken = stored;
    }
    if (instance.running && !this.#sessionToken) await instance.destroy();
    if (!instance.running) {
      const token = crypto.randomUUID();
      this.#signer();
      instance.start({
        image: instance.images.git,
        instance: this.env.SHARD_GIT_R2_INSTANCE ?? "standard-4",
        enableInternet: true,
        env: {
          SHARD_GIT_TOKEN: token,
          SHARD_GIT_R2_ENDPOINT: this.env.SHARD_GIT_R2_CONTAINER_ENDPOINT ?? this.env.SHARD_GIT_R2_ENDPOINT,
          SHARD_GIT_R2_BUCKET: this.env.SHARD_GIT_R2_BUCKET,
          SHARD_GIT_R2_ACCESS_KEY_ID: this.env.SHARD_GIT_R2_ACCESS_KEY_ID,
          SHARD_GIT_R2_SECRET_ACCESS_KEY: this.env.SHARD_GIT_R2_SECRET_ACCESS_KEY,
          ...(this.env.SHARD_GIT_IMPORT_HOSTS !== undefined && { SHARD_GIT_IMPORT_HOSTS: this.env.SHARD_GIT_IMPORT_HOSTS }),
        },
      });
      this.#sessionToken = token;
      await this.ctx.storage.put("container-token", token);
    }
    const configuredIdle = Number(this.env.SHARD_GIT_R2_IDLE_MS ?? 10 * 60_000);
    const idle = Number.isSafeInteger(configuredIdle) && configuredIdle >= 60_000 ? configuredIdle : 10 * 60_000;
    await instance.setInactivityTimeout(idle);
    const port = instance.getTcpPort(PORT);
    const end = Date.now() + STARTUP_MS;
    do {
      try {
        const response = await port.fetch("http://container/healthz", { signal: AbortSignal.timeout(1_000) });
        if (response.ok) {
          const health = await control(response.body, COMMAND_LIMIT, 502);
          if (health.protocol === 1) return { port, token: this.#sessionToken };
        } else {
          await response.body?.cancel();
          if (response.status === 401 || response.status === 403) break;
        }
      } catch {
        // The container port can be listening before the read-only mount is ready.
      }
      await scheduler.wait(200);
    } while (Date.now() < end);
    // A stale persisted token cannot authenticate to a recreated process.
    this.#sessionToken = undefined;
    await this.ctx.storage.delete("container-token");
    await instance.destroy();
    throw new GitHttpError(503, "Git container did not become ready");
  }

  #warmContainer() {
    this.#ready ??= this.#startContainer().finally(() => { this.#ready = undefined; });
    return this.#ready;
  }

  async #workspace(job, repositories) {
    const { port, token } = await this.#warmContainer();
    const prefix = `http://container/v1/workspaces/${job}`;
    const fetch = (path, { method = "GET", headers, body, payload, timeout = WORK_MS } = {}) => {
      const internal = new Headers(headers);
      internal.set("Authorization", `Bearer ${token}`);
      if (payload !== undefined) internal.set("Content-Type", "application/json");
      return port.fetch(`${prefix}${path}`, { method, headers: internal,
        body: payload === undefined ? body : JSON.stringify(payload), redirect: "manual", signal: AbortSignal.timeout(timeout) });
    };
    this.#active++;
    let closed = false;
    const close = async () => {
      if (closed) return;
      closed = true;
      try {
        const response = await fetch("", { method: "DELETE", timeout: 15_000 });
        await response.body?.cancel();
      } catch {
        // The container scratch space is disposable, including after eviction.
      } finally {
        this.#active--;
      }
    };
    try {
      const response = await fetch("", { method: "POST", payload: { repositories } });
      if (response.status !== 204) {
        await response.body?.cancel();
        throw new GitHttpError(response.status === 413 ? 413 : 502, "Could not open the Git workspace");
      }
      return { fetch, close };
    } catch (error) {
      await close();
      throw error;
    }
  }

  #forward(workspace, path, request, body = request.body) {
    const headers = new Headers();
    for (const name of ["content-type", "content-encoding", "git-protocol", "accept", "user-agent"]) {
      const value = request.headers.get(name);
      if (value !== null) headers.set(name, value);
    }
    return workspace.fetch(path, { method: request.method, headers, body });
  }

  async #snapshot(id) {
    if (!ID.test(id)) throw new GitHttpError(422, "Invalid Git source repository");
    const remote = this.env.GIT_R2_REPOS.getByName(id);
    const response = await remote.fetch(`https://internal/v1/r2/${id}/snapshot`);
    if (!response.ok) {
      await response.body?.cancel();
      throw new GitHttpError(response.status === 404 ? 404 : 502, "Git source repository is unavailable");
    }
    return control(response.body, MAX_INFO, 502);
  }

  async #capture(workspace, repository, job, timeout) {
    const id = repository.id;
    const response = await workspace.fetch(`/capture/${id}`, { method: "POST", timeout });
    if (!response.ok) {
      await response.body?.cancel();
      throw new GitHttpError(response.status === 413 ? 413 : 502, "Could not persist Git objects");
    }
    const captured = await control(response.body, MAX_INFO, 502);
    const prefix = packPrefix(id, job);
    if (!validSnapshot(captured, key => key.startsWith(prefix) && /^pack-[a-f0-9]{40}$/.test(key.slice(prefix.length)))) {
      throw new Error("Invalid Git container snapshot");
    }
    const old = new Set(repository.packs.map(key => key.slice(-40)));
    const unique = [];
    const packSizes = [];
    for (const key of captured.packs) {
      const oid = key.slice(-40);
      if (old.has(oid)) {
        await this.#bucket.delete([`${key}.pack`, `${key}.idx`]);
      } else {
        const [pack, idx] = await Promise.all([this.#bucket.head(`${key}.pack`), this.#bucket.head(`${key}.idx`)]);
        if (!pack || !idx || pack.size < 32 || idx.size === 0) throw new Error("Git container did not persist a pack and index");
        old.add(oid);
        unique.push(key);
        packSizes.push(pack.size + idx.size);
      }
    }
    captured.packs = unique;
    captured.packSizes = packSizes;
    return captured;
  }

  async #create(id, request, path) {
    return this.#lock(async () => {
      if (this.#current()) return empty(409);
      const payload = await control(request.body);
      const initial = { id, head: "refs/heads/main", refs: [], packs: [] };
      if (payload.kind === "create" && payload.source === undefined && payload.credential === undefined) {
        this.ctx.storage.transactionSync(() => this.#insert(id, initial));
        return json(this.#metadata(initial), 201);
      }
      if (payload.kind !== "fork" && payload.kind !== "import") return empty(422);
      const job = await this.#reserve(id, 0);
      let next;
      if (payload.kind === "fork") {
        if (typeof payload.source !== "string" || payload.source === id || payload.credential !== undefined) return empty(422);
        const source = await this.#snapshot(payload.source);
        const deadline = Date.now() + WORK_MS;
        next = { ...source, id, packs: [], packSizes: [] };
        for (const key of source.packs) {
          const target = `${packPrefix(id, job)}pack-${key.slice(-40)}`;
          let bytes = 0;
          for (const extension of [".pack", ".idx"]) {
            const object = await this.#bucket.head(key + extension);
            const remaining = deadline - Date.now();
            if (remaining <= 0) throw new GitHttpError(504, "Git fork exceeded its copy time limit");
            if (!object || !await this.#signer().copy(key + extension, target + extension, object, Math.min(remaining, WORK_MS / 2))) {
              throw new GitHttpError(502, "Could not copy an existing Git pack");
            }
            bytes += object.size;
          }
          next.packs.push(target);
          next.packSizes.push(bytes);
        }
      } else {
        const workspace = await this.#workspace(job, []);
        try {
          const response = await workspace.fetch(path, { method: "POST", payload });
          await response.body?.cancel();
          if (response.status !== 201) return empty(response.status);
          next = await this.#capture(workspace, initial, job);
          next.id = id;
        } finally {
          await workspace.close();
        }
      }
      this.ctx.storage.transactionSync(() => {
        this.#insert(id, next);
        this.#sql.exec("DELETE FROM work WHERE id = ?", job);
      });
      this.ctx.waitUntil(this.#arm());
      return json(this.#metadata(next), 201);
    });
  }

  async #delete(id) {
    return this.#lock(async () => {
      if (this.#active || this.#compacting) return empty(202);
      this.#sql.exec("INSERT INTO repository (singleton, id, revision, head, refs, deleted) VALUES (1, ?, 0, '', '[]', 1) "
        + "ON CONFLICT (singleton) DO UPDATE SET deleted = 1", id);
      const uploads = [...this.#sql.exec("SELECT id, upload, report FROM uploads LIMIT 16")];
      for (const upload of uploads) {
        await this.#dropUpload(upload.id, upload.report === null ? upload.upload : null, id);
      }
      const packs = await this.#removePage(packPrefix(id));
      const incoming = await this.#removePage(`git-incoming/${id}/`);
      if (!packs || !incoming || this.#one("SELECT id FROM uploads LIMIT 1")) return empty(202);
      this.ctx.storage.transactionSync(() => {
        this.#sql.exec("DELETE FROM object_cache;");
        this.#sql.exec("DELETE FROM work; DELETE FROM packs; DELETE FROM retired_packs;");
        this.#sql.exec("UPDATE repository SET head = '', refs = '[]' WHERE singleton = 1");
      });
      return empty(204);
    });
  }

  async #packIndex(key) {
    const cached = this.#indexes.get(key);
    if (cached) return cached;
    const object = await this.#bucket.get(`${key}.idx`);
    if (!object || object.size > MAX_DIRECT_INDEX) throw new Error("Git pack index is unavailable");
    const index = new PackIndex(new Uint8Array(await object.arrayBuffer()), MAX_DIRECT_INDEX);
    this.#indexes.set(key, index);
    return index;
  }

  async #packRange(key, offset, length) {
    const object = await this.#bucket.get(`${key}.pack`, { range: { offset, length } });
    if (!object) throw new Error("Git pack is unavailable");
    return new Uint8Array(await object.arrayBuffer());
  }

  #cachedObject(oid) {
    const row = this.#one("SELECT type, size, data FROM object_cache WHERE oid = ?", oid);
    if (!row) return null;
    const data = row.data instanceof Uint8Array ? row.data : new Uint8Array(row.data);
    return { type: row.type, data };
  }

  #cacheObject(oid, object) {
    if (object.data.length > MAX_DIRECT_CACHE) return;
    const total = this.#one("SELECT COALESCE(SUM(size), 0) AS bytes FROM object_cache")?.bytes ?? 0;
    if (total + object.data.length > MAX_OBJECT_CACHE) {
      // The cache is optional. Evict a quarter of its oldest rows rather than
      // letting a public repository consume the Durable Object storage limit.
      const rows = this.#one("SELECT COUNT(*) AS count FROM object_cache")?.count ?? 0;
      const remove = Math.max(1, Math.floor(rows / 4));
      this.#sql.exec("DELETE FROM object_cache WHERE rowid IN (SELECT rowid FROM object_cache ORDER BY rowid LIMIT ?)", remove);
    }
    if ((this.#one("SELECT COALESCE(SUM(size), 0) AS bytes FROM object_cache")?.bytes ?? 0) + object.data.length > MAX_OBJECT_CACHE) return;
    const data = object.data;
    const stored = data.byteOffset === 0 && data.byteLength === data.buffer.byteLength
      ? data.buffer
      : data.slice().buffer;
    this.#sql.exec("INSERT OR REPLACE INTO object_cache (oid, type, size, data) VALUES (?, ?, ?, ?)",
      oid, object.type, data.length, stored);
  }

  async #readObject(oid, depth = 0, packs) {
    if (!OID.test(oid) || depth > 64) return null;
    const cached = this.#cachedObject(oid);
    if (cached) return cached;
    const keys = packs ?? this.#repository().packs;
    for (const key of keys) {
      let index;
      try {
        index = await this.#packIndex(key);
      } catch {
        continue;
      }
      const object = await readPackedObject(index, oid, {
        readRange: (offset, length) => this.#packRange(key, offset, length),
        readBase: (base, nextDepth) => this.#readObject(base, nextDepth, keys),
        maxBytes: MAX_DIRECT_FILE,
      });
      if (object) {
        this.#cacheObject(oid, object);
        return object;
      }
    }
    return null;
  }

  async #resolveCommit(repository, reference, packs) {
    const peeled = (reference || repository.head).replace(/\^\{[^}]*\}$/, "");
    let oid = OID.test(peeled) ? peeled : null;
    if (!oid) {
      const candidates = peeled.startsWith("refs/")
        ? [peeled]
        : [`refs/heads/${peeled}`, `refs/tags/${peeled}`];
      const found = repository.refs.find((ref) => candidates.includes(ref.name));
      if (found) oid = found.oid;
    }
    if (!oid) return null;
    const seen = new Set();
    for (let depth = 0; depth < 8; depth++) {
      if (seen.has(oid)) return null;
      seen.add(oid);
      const object = await this.#readObject(oid, 0, packs);
      if (!object) return null;
      if (object.type === 1) return { oid, object, commit: parseCommit(oid, object.data) };
      if (object.type !== 4) return null;
      oid = parseTagTarget(object.data);
    }
    return null;
  }

  async #directRead(repository, action, url) {
    const query = url.searchParams;
    const value = (name) => {
      const values = query.getAll(name);
      if (values.length !== 1 || !values[0] || values[0].includes("\0")) {
        throw new GitHttpError(422, "Invalid Git query");
      }
      return values[0];
    };
    const limit = (optional = false) => {
      if (!query.has("limit") && optional) return null;
      const text = value("limit");
      if (!/^\d+$/.test(text)) throw new GitHttpError(422, "Invalid Git query");
      const number = Number(text);
      if (!Number.isSafeInteger(number)) throw new GitHttpError(422, "Invalid Git query");
      return number;
    };
    const headers = new Headers(NO_STORE);
    const binary = (data, extra = {}) => {
      for (const [name, value] of Object.entries(extra)) headers.set(name, value);
      return new Response(data, { headers });
    };
    const packs = repository.packs;

    if (action === "commit") {
      const oid = value("oid");
      const object = await this.#readObject(oid, 0, packs);
      if (!object || object.type !== 1) throw new GitHttpError(404, "Git object not found");
      return json(parseCommit(oid, object.data));
    }
    if (action === "commit-raw") {
      const oid = value("oid");
      const object = await this.#readObject(oid, 0, packs);
      if (!object || object.type !== 1) throw new GitHttpError(404, "Git object not found");
      if (object.data.length > MAX_DIRECT_COMMIT) throw new GitHttpError(413, "Git commit is too large");
      return binary(object.data, { "Content-Type": "application/octet-stream", "Content-Length": String(object.data.length) });
    }
    if (action === "archive") {
      const oid = value("oid");
      const name = value("name");
      const object = await this.#readObject(oid, 0, packs);
      if (!object || object.type !== 1) throw new GitHttpError(404, "Git object not found");
      try {
        const tar = await buildSourceTar(parseCommit(oid, object.data),
          (id) => this.#readObject(id, 0, packs));
        const filename = encodeURIComponent(`${name}-${oid}.tar`);
        return binary(tar, { "Content-Type": "application/x-tar", "Content-Length": String(tar.length),
          "Content-Disposition": `attachment; filename*=UTF-8''${filename}`,
          "Cache-Control": "private, no-store", "X-Content-Type-Options": "nosniff" });
      } catch (error) {
        if (error instanceof ArchiveLimitError) throw new GitHttpError(413, error.message);
        if (error instanceof ArchiveUnsupportedError) throw new GitHttpError(422, error.message);
        throw error;
      }
    }
    if (action === "tree") {
      const oid = value("oid");
      const maximum = limit();
      const object = await this.#readObject(oid, 0, packs);
      if (!object || object.type !== 2) throw new GitHttpError(404, "Git object not found");
      const entries = parseTree(object.data);
      if (entries.length > maximum) throw new GitHttpError(413, "Git tree is too large");
      // Directory listings do not probe every blob for its uncompressed size:
      // that turns one page view into hundreds of MiB of pack reads. The file
      // table leaves the optional size blank instead.
      return json(entries);
    }
    if (action === "blob") {
      const oid = value("oid");
      const maximum = limit();
      const object = await this.#readObject(oid, 0, packs);
      if (!object || object.type !== 3) throw new GitHttpError(404, "Git object not found");
      if (object.data.length > maximum) throw new GitHttpError(413, "Git blob is too large");
      return binary(object.data, { "Content-Type": "application/octet-stream", "Content-Length": String(object.data.length) });
    }
    if (action === "file") {
      const reference = value("ref");
      const path = value("path");
      if (!path || path.startsWith("/") || path.endsWith("/") || path.split("/").some((part) => !part || part === "." || part === "..")) {
        throw new GitHttpError(404, "Git file not found");
      }
      const resolved = await this.#resolveCommit(repository, reference, packs);
      if (!resolved) throw new GitHttpError(404, "Git file not found");
      let tree = resolved.commit.tree;
      let entry;
      for (const part of path.split("/")) {
        const object = await this.#readObject(tree, 0, packs);
        if (!object || object.type !== 2) throw new GitHttpError(404, "Git file not found");
        entry = parseTree(object.data).find((candidate) => candidate.name === part);
        if (!entry) throw new GitHttpError(404, "Git file not found");
        tree = entry.hash;
      }
      const object = await this.#readObject(tree, 0, packs);
      if (!object || object.type !== 3) throw new GitHttpError(404, "Git file not found");
      const maximum = limit(true);
      if (maximum === null && object.data.length > MAX_DIRECT_FILE) return null;
      const length = maximum === null ? object.data.length : Math.min(maximum, object.data.length);
      return binary(object.data.subarray(0, length), {
        "Content-Type": "application/octet-stream",
        "Content-Length": String(length),
        "X-Shard-Object-Size": String(object.data.length),
      });
    }
    if (action === "log") {
      const reference = value("ref");
      const maximum = Math.min(limit(), 1000);
      const resolved = await this.#resolveCommit(repository, reference, packs);
      if (!resolved) return json([]);
      const commits = [];
      let oid = resolved.oid;
      while (oid && commits.length < maximum) {
        const object = await this.#readObject(oid, 0, packs);
        if (!object || object.type !== 1) break;
        const commit = parseCommit(oid, object.data);
        commits.push(commit);
        oid = commit.parents[0];
      }
      return json(commits);
    }
    return null;
  }

  async #findObject(oid, packs) {
    if (!OID.test(oid)) return null;
    for (const key of packs) {
      let index;
      try {
        index = await this.#packIndex(key);
      } catch {
        continue;
      }
      const offset = index.find(oid);
      if (offset !== null) return { key, offset };
    }
    return null;
  }

  async #objectCount(packs) {
    let total = 0;
    for (const key of packs) {
      let count = this.#counts.get(key);
      if (count === undefined) {
        try {
          count = (await this.#packIndex(key)).count;
          this.#counts.set(key, count);
        } catch {
          count = 0;
        }
      }
      total += count;
    }
    return total;
  }

  #uploadStore(repository, count, packStore) {
    const typeName = (type) => OBJECT_TYPES[type];
    const metadata = (oid) => {
      const entry = packStore.lookup(oid);
      if (entry?.type) return { type: entry.type, size: entry.entrySize ?? entry.size ?? 0 };
      const cached = this.#cachedObject(oid);
      return cached ? { type: typeName(cached.type), size: cached.data.length } : null;
    };
    const object = async (oid) => {
      const cached = await this.#readObject(oid, 0, repository.packs);
      return cached ? { type: typeName(cached.type), data: cached.data } : null;
    };
    return {
      head: () => repository.head,
      resolveHead: () => repository.refs.find((ref) => ref.name === repository.head)?.oid ?? null,
      refs: () => repository.refs.map((ref) => ({ name: ref.name, target: ref.oid })),
      get: object,
      has: (oid) => metadata(oid) !== null,
      typeAndSize: metadata,
      objectCount: () => count,
      reachableCacheValid: () => false,
      loadReachable: () => null,
      saveReachable: async () => {},
      reachableVersion: () => String(repository.revision),
      packs: packStore,
    };
  }

  async #directUploadPack(repository, request) {
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (Number.isFinite(declared) && declared > MAX_DIRECT_UPLOAD) return null;
    const count = await this.#objectCount(repository.packs);
    if (count > MAX_DIRECT_OBJECTS) return null;
    const cacheKey = `${repository.revision}:${repository.packs.join("|")}`;
    let packStore = this.#packStores.get(cacheKey);
    if (!packStore) {
      packStore = await buildR2PackStore(
        this.#bucket, repository.packs, (key) => this.#packIndex(key), MAX_DIRECT_OBJECTS,
      );
      if (!packStore) return null;
      if (this.#packStores.size > 4) this.#packStores.clear();
      this.#packStores.set(cacheKey, packStore);
    }
    const body = new Uint8Array(await request.arrayBuffer());
    if (body.length > MAX_DIRECT_UPLOAD) throw new GitHttpError(413, "Git upload request is too large");
    return await uploadPack(this.#uploadStore(repository, count, packStore), body);
  }

  async #directAdvertisement(repository, v2 = false) {
    const encoder = new TextEncoder();
    const packet = (text) => {
      const length = encoder.encode(text).byteLength + 4;
      if (length > 0xffff) throw new GitHttpError(413, "Git advertisement is too large");
      return length.toString(16).padStart(4, "0") + text;
    };
    if (v2) {
      return new Response(
        packet("version 2\n") + packet("agent=shard/0.3.1\n") +
        packet("ls-refs\n") + packet("fetch=shallow\n") + "0000",
        { headers: {
          "Cache-Control": "no-store",
          "Content-Type": "application/x-git-upload-pack-advertisement",
        } },
      );
    }
    const head = repository.refs.find((ref) => ref.name === repository.head);
    let body = packet("# service=git-upload-pack\n") + "0000";
    if (head) {
      body += packet(`${head.oid} HEAD\0` +
        `multi_ack_detailed side-band-64k thin-pack ofs-delta deepen-since deepen-not ` +
        `agent=shard/0.3.1 symref=HEAD:${repository.head} object-format=sha1\n`);
    }
    const refs = [];
    for (const ref of repository.refs) {
      refs.push(packet(`${ref.oid} ${ref.name}\n`));
      if (ref.name.startsWith("refs/tags/")) {
        let oid = ref.oid;
        let peeled = null;
        for (let depth = 0; depth < 8; depth++) {
          const object = await this.#readObject(oid, 0, repository.packs);
          if (!object || object.type !== 4) break;
          oid = parseTagTarget(object.data);
          peeled = oid;
        }
        if (peeled) refs.push(packet(`${peeled} ${ref.name}^{}\n`));
      }
    }
    body += refs.join("") + "0000";
    return new Response(body, { headers: {
      "Cache-Control": "no-store",
      "Content-Type": "application/x-git-upload-pack-advertisement",
    } });
  }

  async #directReceiveAdvertisement(repository) {
    const encoder = new TextEncoder();
    const packet = (text) => {
      const length = encoder.encode(text).byteLength + 4;
      if (length > 0xffff) throw new GitHttpError(413, "Git advertisement is too large");
      return length.toString(16).padStart(4, "0") + text;
    };
    const capabilities = "report-status delete-refs side-band-64k ofs-delta agent=shard/0.3.1 object-format=sha1";
    const head = repository.refs.find((ref) => ref.name === repository.head);
    const ordered = head
      ? [{ name: "HEAD", oid: head.oid }, ...repository.refs.filter((ref) => ref.name !== "HEAD")]
      : repository.refs;
    let body = packet("# service=git-receive-pack\n") + "0000";
    if (ordered.length === 0) {
      body += packet(`${ZERO} capabilities^{}\0${capabilities}\n`);
    } else {
      body += packet(`${ordered[0].oid} ${ordered[0].name}\0${capabilities}\n`);
      for (const ref of ordered.slice(1)) body += packet(`${ref.oid} ${ref.name}\n`);
    }
    body += "0000";
    return new Response(body, { headers: {
      "Cache-Control": "no-store",
      "Content-Type": "application/x-git-receive-pack-advertisement",
    } });
  }

  async #read(repository, url, request, includeSource) {
    const repositories = [repository];
    const source = includeSource ? url.searchParams.get("source") : null;
    if (source && source !== repository.id) repositories.push(await this.#snapshot(source));
    const workspace = await this.#workspace(crypto.randomUUID(), repositories);
    let response;
    try {
      response = await this.#forward(workspace, url.pathname + url.search, request);
      if (!response.body) {
        await workspace.close();
        return response;
      }
    } catch (error) {
      await workspace.close();
      throw error;
    }
    const reader = response.body.getReader();
    let ended = false;
    const end = () => {
      if (ended) return;
      ended = true;
      reader.releaseLock();
      this.ctx.waitUntil(workspace.close());
    };
    const body = new ReadableStream({
      async pull(controller) {
        try {
          const next = await reader.read();
          if (!next.done) return controller.enqueue(next.value);
          controller.close();
        } catch (error) {
          controller.error(error);
        }
        end();
      },
      async cancel(reason) {
        try { await reader.cancel(reason); } finally { end(); }
      },
    });
    return new Response(body, { status: response.status, headers: response.headers });
  }

  async #mutate(repository, path, request) {
    return this.#lock(async () => {
      const payload = await control(request.body);
      const repos = [repository];
      if (payload.source !== undefined && payload.source !== repository.id) {
        if (typeof payload.source !== "string") return empty(422);
        repos.push(await this.#snapshot(payload.source));
      }
      const job = await this.#reserve(repository.id, repository.revision);
      const workspace = await this.#workspace(job, repos);
      try {
        const response = await workspace.fetch(path, { method: "POST", payload });
        if (!response.ok) return empty(response.status);
        const reply = response.body ? await bytes(response.body, COMMAND_LIMIT, 502) : null;
        const candidate = await this.#capture(workspace, repository, job);
        this.ctx.storage.transactionSync(() => {
          this.#update(repository.revision, candidate);
          this.#sql.exec("DELETE FROM work WHERE id = ?", job);
        });
        this.ctx.waitUntil(this.#arm());
        return new Response(reply, { status: response.status, headers: response.headers });
      } finally {
        await workspace.close();
      }
    });
  }

  #prepared(job) {
    return json({ prepared: job.id, updates: JSON.parse(job.updates), accepted: JSON.parse(job.accepted) }, 202);
  }

  async #prepare(repository, actor, push, receive, upload = null) {
    const updates = JSON.parse(push.updates);
    // Git's HTTP helper sends an empty pkt-line probe before a streamed push.
    if (!updates.length) return upload === null ? result(null) : empty(422);
    if (updates.some(update => update.name === repository.head && update.after === ZERO)) {
      return json({ message: "The default Git branch cannot be deleted" }, 422);
    }
    const job = await this.#reserve(repository.id, repository.revision, actor, upload);
    const workspace = await this.#workspace(job, [repository]);
    try {
      const response = await receive(workspace);
      if (!response.ok || response.headers.get("Content-Type")?.split(";", 1)[0] !== REPORT_TYPE) {
        await response.body?.cancel();
        return json({ message: "Git receive validation failed" }, response.ok ? 502 : response.status);
      }
      const report = await bytes(response.body, MAX_REPORT, 502);
      const reported = push.report ? JSON.parse(await shard_receive_report(new Blob([report]).stream(), push.sideband)) : null;
      if (reported?.length === 0) {
        if (upload !== null) {
          this.#sql.exec("UPDATE uploads SET report = ? WHERE id = ?", report.buffer, upload);
          this.ctx.waitUntil(this.#deleteStaged(repository.id, upload));
        }
        this.#sql.exec("DELETE FROM work WHERE id = ?", job);
        return result(report);
      }
      const candidate = await this.#capture(workspace, repository, job);
      const accepted = changedRefs(repository, candidate, updates, reported);
      const saved = this.#one("UPDATE work SET candidate = ?, updates = ?, accepted = ?, report = ? WHERE id = ? "
        + "RETURNING id, updates, accepted", JSON.stringify(candidate), JSON.stringify(updates), JSON.stringify(accepted), report.buffer, job);
      return this.#prepared(saved);
    } finally {
      await workspace.close();
    }
  }

  async #directReceive(repository, request) {
    if (request.headers.get("Content-Type") !== INGEST_TYPE) return null;
    const actor = request.headers.get("x-shard-git-actor");
    if (!ACTOR.test(actor ?? "")) return null;
    const encoding = (request.headers.get("content-encoding") ?? "").trim().toLowerCase();
    if (encoding && encoding !== "identity") return null;
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (Number.isFinite(declared) && declared > MAX_DIRECT_PUSH) return null;
    const bytes = new Uint8Array(await request.arrayBuffer());
    if (bytes.length > MAX_DIRECT_PUSH) return null;
    let push;
    try {
      push = parseReceiveBytes(bytes);
    } catch {
      return null;
    }
    if (!push.updates.length) return null;
    if (push.updates.some((update) => update.name === repository.head && update.after === ZERO)) {
      return json({ message: "The default Git branch cannot be deleted" }, 422);
    }
    return await this.#lock(async () => {
      let pack = null;
      if (push.pack.length) {
        try {
          pack = await parseReceivedPack(push.pack, MAX_DIRECT_OBJECTS, async (oid) => {
            const object = await this.#readObject(oid, 0, repository.packs);
            return object ? { type: OBJECT_TYPES[object.type], data: object.data } : null;
          });
          if (pack.packBytes.length > MAX_DIRECT_PUSH) return null;
        } catch (error) {
          if (error instanceof ThinPackError || error instanceof PackTooLargeError) return null;
          console.error("R2 direct receive parse failed", error?.message);
          return null;
        }
      }
      if (!pack && push.updates.some((update) => update.after !== ZERO)) return null;
      const lookup = async (oid) => {
        const local = pack?.objects.get(oid);
        if (local) return local;
        const object = await this.#readObject(oid, 0, repository.packs);
        return object ? { oid, type: object.type, data: object.data } : null;
      };
      let inspected = 0;
      const checkTree = async (oid) => {
        if (++inspected > MAX_DIRECT_OBJECTS) throw new PackTooLargeError("Git push object graph is too large");
        const object = await lookup(oid);
        if (!object || object.type !== 2) return false;
        let entries;
        try {
          entries = parseTree(object.data);
        } catch {
          return false;
        }
        for (const entry of entries) {
          if (entry.mode === "040000") {
            if (!await checkTree(entry.hash)) return false;
          } else if (entry.mode !== "160000") {
            const blob = await lookup(entry.hash);
            if (!blob || blob.type !== 3) return false;
          }
        }
        return true;
      };
      const checkCommit = async (oid) => {
        const object = await lookup(oid);
        if (!object || object.type !== 1) return false;
        let commit;
        try {
          commit = parseCommit(oid, object.data);
        } catch {
          return false;
        }
        for (const parent of commit.parents) {
          const value = await lookup(parent);
          if (!value || value.type !== 1) return false;
        }
        return await checkTree(commit.tree);
      };
      const refs = new Map(repository.refs.map((ref) => [ref.name, ref.oid]));
      for (const update of push.updates) {
        if (update.before !== (refs.get(update.name) ?? ZERO)) throw new GitHttpError(409, "Git refs changed during push");
        if (update.after === ZERO) {
          refs.delete(update.name);
          continue;
        }
        const object = await lookup(update.after);
        if (!object) return null;
        if (object.type === 1) {
          if (!await checkCommit(update.after)) return null;
        } else if (object.type === 4 && update.name.startsWith("refs/tags/")) {
          let target;
          try {
            target = parseTagTarget(object.data);
          } catch {
            return null;
          }
          const peeled = await lookup(target);
          if (!peeled || peeled.type !== 1) return null;
        } else {
          return null;
        }
        refs.set(update.name, update.after);
      }
      const job = await this.#reserve(repository.id, repository.revision, actor);
      let key = null;
      try {
        if (pack) {
          key = `${packPrefix(repository.id, job)}pack-${pack.packHash}`;
          await this.#bucket.put(`${key}.pack`, pack.packBytes);
          await this.#bucket.put(`${key}.idx`, pack.idx);
        }
        const candidate = {
          id: repository.id,
          revision: repository.revision,
          head: repository.head,
          refs: [...refs].map(([name, oid]) => ({ name, oid })),
          packs: key ? [key] : [],
          packSizes: key ? [pack.packBytes.length + pack.idx.length] : [],
        };
        const accepted = changedRefs(repository, candidate, push.updates, push.updates.map((update) => update.name));
        const report = renderStatus(push.updates.map((update) => ({ ref: update.name, ok: true })), null, push.sideband);
        const saved = this.#one("UPDATE work SET candidate = ?, updates = ?, accepted = ?, report = ? WHERE id = ? "
          + "RETURNING id, updates, accepted", JSON.stringify(candidate), JSON.stringify(push.updates),
          JSON.stringify(accepted), report.buffer, job);
        if (!saved) throw new Error("Git push preparation was lost");
        return this.#prepared(saved);
      } catch (error) {
        await this.#dropWork({ repo: repository.id, id: job });
        throw error;
      }
    });
  }

  async #standard(repository, path, request) {
    if (request.headers.get("Content-Type") !== INGEST_TYPE) return empty(415);
    const actor = request.headers.get("x-shard-git-actor");
    if (!ACTOR.test(actor ?? "") || !request.body) return empty(400);
    const limit = this.#inputLimit();
    let length = 0;
    let oversized = false;
    const bounded = request.body.pipeThrough(new TransformStream({
      transform(chunk, controller) {
        length += chunk.byteLength;
        if (length <= limit) controller.enqueue(chunk);
        else {
          oversized = true;
          controller.error(new Error("Git upload size limit exceeded"));
        }
      },
    }));
    return this.#lock(async () => {
      let push;
      try {
        push = await shard_receive_request(bounded, "");
      } catch {
        return empty(oversized ? 413 : 400);
      }
      try {
        const response = await this.#prepare(repository, actor, push,
          workspace => this.#forward(workspace, path, request, push.body));
        if (!oversized) return response;
        await response.body?.cancel();
        return empty(413);
      } catch (error) {
        if (oversized) return empty(413);
        throw error;
      } finally {
        if (!push.body.locked) await push.body.cancel().catch(() => {});
      }
    });
  }

  async #deleteStaged(repo, id) {
    try { await this.#bucket.delete(incomingKey(repo, id)); } catch {
      // The persisted upload expiry retries removal.
    }
  }

  async #publish(request) {
    const { prepared, actor } = await control(request.body);
    if (typeof prepared !== "string" || !UUID.test(prepared) || typeof actor !== "string" || !ACTOR.test(actor)) return empty(400);
    const job = this.#one("SELECT * FROM work WHERE id = ? AND actor = ? AND expires > ?", prepared, actor, Date.now());
    if (!job || job.report === null) return empty(409);
    if (job.published) return result(job.report, true);
    const upload = job.upload === null ? null : this.#one("SELECT id, report FROM uploads WHERE id = ? AND actor = ? AND aborted = 0 AND expires > ?", job.upload, actor, Date.now());
    if (job.upload !== null && !upload) return empty(409);
    if (upload && upload.report !== null) return result(upload.report, true);
    const current = this.#current();
    if (current?.deleted || !current) return empty(409);
    if (upload && current.revision !== job.revision) {
      // Finish will prepare against the new snapshot and recheck branch protection.
      return Response.json({ message: "Git refs changed during preparation" }, {
        status: 409, headers: { ...NO_STORE, "Retry-After": "1" },
      });
    }
    const candidate = JSON.parse(job.candidate);
    // The new pack/index pairs are already durable. Refs and the retry receipt become visible together.
    this.ctx.storage.transactionSync(() => {
      this.#update(job.revision, candidate);
      this.#sql.exec("UPDATE work SET published = 1, candidate = NULL WHERE id = ?", job.id);
      if (upload) this.#sql.exec("UPDATE uploads SET report = ? WHERE id = ?", job.report, upload.id);
    });
    this.ctx.waitUntil(this.#arm());
    if (upload) this.ctx.waitUntil(this.#deleteStaged(job.repo, upload.id));
    return result(job.report);
  }

  async #parts(upload, key) {
    return (await this.#signer().parts(key, upload.upload)).filter(p => partLength(upload, p.partNumber) === p.size);
  }

  async #plan(id, actor, command) {
    const { sha256, size, nonce, lookup } = command;
    if (typeof sha256 !== "string" || !SHA256.test(sha256) || !Number.isSafeInteger(size) || size < 4 ||
        nonce !== undefined && (typeof nonce !== "string" || !/^[a-f0-9]{32}$/.test(nonce))) return empty(422);
    // Each new spool has its own receipt even when two pushes produce identical Git requests.
    const digest = nonce === undefined ? sha256 : `${sha256}-${nonce}`;
    let active = this.#one("SELECT * FROM uploads WHERE actor = ? AND digest = ? AND size = ?", actor, digest, size);
    if (lookup === true) {
      if (!active || active.aborted || active.expires <= Date.now()) return json(null);
      return json({ id: active.id, complete: active.report !== null });
    }
    if (active && (active.aborted || active.expires <= Date.now())) {
      await this.#dropUpload(active.id, active.report === null ? active.upload : null, id);
      active = null;
    }
    if (!active) {
      if (size > this.#inputLimit()) return empty(413);
      if (this.#one("SELECT COUNT(*) AS count FROM uploads WHERE report IS NULL AND expires > ?", Date.now()).count >= 8) {
        return json({ message: "Finish or abort an existing Git upload before creating another" }, 429);
      }
      const partBytes = Math.max(32, Math.ceil(size / (MAX_PARTS * MiB))) * MiB;
      active = this.#one("INSERT INTO uploads (id, actor, digest, size, part_bytes, expires) VALUES (?, ?, ?, ?, ?, ?) RETURNING *",
        crypto.randomUUID(), actor, digest, size, partBytes, Date.now() + SESSION_MS);
      await this.#arm();
    }
    const plan = { id: active.id, partBytes: active.part_bytes, uploaded: [], staged: false, complete: active.report !== null };
    if (plan.complete) return json(plan);
    const key = incomingKey(id, active.id);
    const staged = await this.#bucket.head(key);
    if (staged) {
      return json({ ...plan, staged: true });
    }
    if (active.upload !== null) {
      try {
        plan.uploaded = (await this.#parts(active, key)).map(part => part.partNumber);
        return json(plan);
      } catch (error) {
        if (error.status !== 404 || error.code !== "NoSuchUpload") throw error;
      }
    }
    this.#signer();
    const opened = await this.#bucket.createMultipartUpload(key);
    this.#sql.exec("UPDATE uploads SET upload = ? WHERE id = ?", opened.uploadId, active.id);
    return json(plan);
  }

  async #finish(repository, upload, key) {
    const prepared = this.#one("SELECT id, updates, accepted FROM work WHERE upload = ? AND revision = ? "
      + "AND report IS NOT NULL AND expires > ? ORDER BY expires DESC LIMIT 1", upload.id, repository.revision, Date.now());
    if (prepared) return this.#prepared(prepared);
    let staged = await this.#bucket.head(key);
    if (!staged) {
      let parts;
      try {
        parts = await this.#parts(upload, key);
      } catch (error) {
        if (error.status === 404 && error.code === "NoSuchUpload") {
          return json({ message: "Git multipart upload expired. Start again to resume" }, 409);
        }
        throw error;
      }
      if (parts.length !== Math.ceil(upload.size / upload.part_bytes)) {
        return json({ message: "Git upload is missing one or more parts" }, 409);
      }
      try {
        staged = await this.#bucket.resumeMultipartUpload(key, upload.upload).complete(parts.map(({ partNumber, etag }) => ({ partNumber, etag })));
      } catch (error) {
        // The R2 completion can succeed even if the reply was lost.
        staged = await this.#bucket.head(key);
        if (!staged) throw error;
      }
    }
    if (staged.size !== upload.size) return empty(422);
    const prefix = await this.#bucket.get(key, { range: { offset: 0, length: Math.min(upload.size, 256 * 1024) } });
    if (!prefix) throw new GitHttpError(502, "Git staged upload could not be read");
    let push;
    try {
      push = await shard_receive_request(prefix.body, "");
      await push.body.cancel();
    } catch {
      return json({ message: "Invalid Git push commands" }, 422);
    }
    return this.#prepare(repository, upload.actor, push, workspace => workspace.fetch(`/receive/${repository.id}`, {
      method: "POST", payload: { key, size: upload.size, sha256: upload.digest.slice(0, 64) },
    }), upload.id);
  }

  async #multipart(id, request) {
    const { actor, command } = await control(request.body);
    if (typeof actor !== "string" || !ACTOR.test(actor) || !command || typeof command !== "object") return empty(400);
    if (command.action === "start") return this.#lock(() => this.#plan(id, actor, command));
    if (typeof command.id !== "string" || !UUID.test(command.id)) return empty(400);
    const lookup = () => this.#one("SELECT * FROM uploads WHERE id = ? AND actor = ? AND aborted = 0 AND expires > ?", command.id, actor, Date.now());
    if (command.action === "part") {
      const upload = lookup();
      if (!upload) return empty(404);
      if (upload.report !== null || upload.upload === null) return empty(409);
      const length = partLength(upload, command.number);
      if (!length) return empty(422);
      const key = incomingKey(id, upload.id);
      if (await this.#bucket.head(key)) return empty(409);
      return json(await this.#signer().signPart(key, upload.upload, command.number, length));
    }
    return this.#lock(async () => {
      const upload = lookup();
      if (!upload) return empty(404);
      if (command.action === "abort") {
        if (upload.report !== null) return empty(409);
        await this.#dropUpload(upload.id, upload.upload, id);
        return empty(204);
      }
      if (command.action !== "finish") return empty(400);
      if (upload.report !== null) return result(upload.report);
      if (upload.upload === null) return empty(409);
      const response = await this.#finish(this.#repository(), upload, incomingKey(id, upload.id));
      if (response.status === 422) await this.#dropUpload(upload.id, upload.upload, id);
      return response;
    });
  }

  async fetch(request) {
    try {
      const url = new URL(request.url);
      const internal = STORAGE.exec(url.pathname);
      const smart = SMART.exec(url.pathname);
      const command = CONTROL.exec(url.pathname);
      const route = internal ?? smart ?? command;
      if (url.origin !== "https://internal" || !route) return empty(404);
      const [, id, action] = route;
      if (!this.ctx.id.equals(this.env.GIT_R2_REPOS.idFromName(id))) return empty(404);
      if (command && !action) {
        if (request.method === "POST") return await this.#create(id, request, url.pathname);
        if (request.method === "DELETE") return await this.#delete(id);
        if (request.method === "GET") {
          if (this.#writer && !this.#current()) return empty(202);
          return json(this.#metadata(this.#repository()));
        }
        return empty(404);
      }
      if (internal) {
        if (action === "snapshot" && request.method === "GET") return json(this.#repository());
        if (action === "recovery") {
          if (request.method === "GET") return json({ format: 1, ...this.#repository() });
          if (request.method === "PUT") return await this.#restore(id, request);
          return empty(404);
        }
        const repo = this.#current();
        if (!repo || repo.deleted) return empty(404);
        if (action === "publish" && request.method === "POST") return await this.#publish(request);
        if (action === "multipart" && request.method === "POST") return await this.#multipart(id, request);
        return empty(404);
      }
      const repository = this.#repository();
      if (smart && action === "info/refs" && request.method === "GET" &&
          url.searchParams.get("service") === "git-receive-pack" && this.#directReads) {
        try {
          return await this.#directReceiveAdvertisement(repository);
        } catch (error) {
          if (error instanceof GitHttpError) throw error;
          console.error("R2 direct receive advertisement failed", error?.message);
        }
      }
      if (smart && action === "git-receive-pack" && request.method === "POST" && !url.search) {
        const fallback = request.clone();
        if (this.#directReads) {
          try {
            const direct = await this.#directReceive(repository, request);
            if (direct) return direct;
          } catch (error) {
            if (error instanceof GitHttpError) throw error;
            console.error("R2 direct receive failed", error?.message);
          }
        }
        return await this.#standard(repository, url.pathname, fallback);
      } else if (command && ["ref", "head", "merge"].includes(action) && request.method === "POST" && !url.search) {
        return await this.#mutate(repository, url.pathname, request);
      } else if (smart && action === "info/refs" && request.method === "GET" &&
        url.searchParams.get("service") === "git-upload-pack" && this.#directReads) {
        const v2 = (request.headers.get("git-protocol") ?? "").toLowerCase().includes("version=2");
        try {
          return await this.#directAdvertisement(repository, v2);
        } catch (error) {
          if (error instanceof GitHttpError) throw error;
          console.error("R2 direct advertisement failed", error?.message);
        }
        return await this.#read(repository, url, request, false);
      } else if (smart && action === "git-upload-pack" && request.method === "POST" &&
        !url.search && this.#directReads) {
        const fallback = request.clone();
        try {
          const direct = await this.#directUploadPack(repository, request);
          if (direct) {
            const headers = new Headers(direct.headers);
            headers.set("X-Shard-R2-Read", "direct");
            return new Response(direct.body, { status: direct.status, statusText: direct.statusText, headers });
          }
        } catch (error) {
          if (error instanceof GitHttpError) throw error;
          console.error("R2 direct upload-pack failed", error?.message);
        }
        const response = await this.#read(repository, url, fallback, false);
        const headers = new Headers(response.headers);
        headers.set("X-Shard-R2-Read", "container");
        return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
      } else if (!smart && request.method === "GET" && this.#directReads && DIRECT_ACTIONS.has(action)) {
        try {
          const direct = await this.#directRead(repository, action, url);
          if (direct) {
            const headers = new Headers(direct.headers);
            headers.set("X-Shard-R2-Read", "direct");
            return new Response(direct.body, { status: direct.status, statusText: direct.statusText, headers });
          }
        } catch (error) {
          if (error instanceof GitHttpError) throw error;
          console.error("R2 direct read failed", error?.message);
        }
        const response = await this.#read(repository, url, request, false);
        const headers = new Headers(response.headers);
        headers.set("X-Shard-R2-Read", "container");
        return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
      } else if (request.method === "GET" || smart && action === "git-upload-pack" && request.method === "POST") {
        return await this.#read(repository, url, request, command && ["merge-base", "compare"].includes(action));
      }
      return empty(404);
    } catch (error) {
      if (error instanceof GitHttpError) return json({ message: error.message }, error.status);
      console.error("R2 Git repository operation failed", error?.operation ?? error?.name);
      return json({ message: "R2 Git storage is temporarily unavailable" }, 502);
    }
  }
}
