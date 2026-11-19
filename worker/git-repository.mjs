// RepoCell and these internal codecs are coupled to durable-git 0.0.8
// (d5eba04eda584b855a1bde1f86a836b751c378c0). Review them together on upgrade.
import { RepoCell } from "durable-git";
import { isGitlinkMode, isTreeMode, parseCommit, parseTag, parseTree } from "../node_modules/durable-git/src/git/objects.ts";
import { ObjCache } from "../node_modules/durable-git/src/git/packstore.ts";
import { FLUSH, pkt, PktParser } from "../node_modules/durable-git/src/git/pktline.ts";
import { advertisement, renderStatus, validatePush } from "../node_modules/durable-git/src/git/protocol.ts";
import { GitStore } from "../node_modules/durable-git/src/git/store.ts";
import { concat, isOid, td, te, ZERO_OID } from "../node_modules/durable-git/src/git/util.ts";
import { shard_receive_request } from "../src/git_wire.js";

const ID = /^shard-[A-Za-z0-9._-]{1,245}$/;
const HEADS = "refs/heads/";
const TAGS = "refs/tags/";
const MAIN = `${HEADS}main`;
const UPLOAD = "git-upload-pack";
const RECEIVE = "git-receive-pack";
const SNAPSHOT = "application/x-shard-git-snapshot";
const INTERNAL = "https://internal";
const READY = "shard-repository";
const STAGING = "shard-staging";
const SYMBOLIC_HEAD = "shard-head";
const IMPORT_HOSTS = new Set(["github.com", "gitlab.com", "codeberg.org"]);
const MIB = 1024 * 1024;
const MAX_CONTROL = 8 * 1024;
const MAX_QUERY = 4 * 1024;
const MAX_METADATA = 4 * MIB;
const MAX_ADVERTISEMENT = 256 * 1024;
const MAX_COPY_REFS = 512;
const MAX_UPLOAD_REQUEST = 16 * MIB;
const CONTROL = /^\/v1\/repos\/([^/]+)(?:\/(log|commit|tree|blob|file|ref|head|merge|fork))?$/;
const SMART = /^\/v1\/git\/([^/]+)\.git\/(info\/refs|git-upload-pack|git-receive-pack)$/;

class HttpError extends Error {
  constructor(status) {
    super(`HTTP ${status}`);
    this.status = status;
  }
}

function empty(status) {
  return new Response(null, { status, headers: { "Cache-Control": "no-store" } });
}

function json(payload, status = 200) {
  const body = te.encode(JSON.stringify(payload));
  if (body.length > MAX_METADATA) throw new HttpError(413);
  return new Response(body, {
    status,
    headers: { "Cache-Control": "no-store", "Content-Type": "application/json; charset=utf-8" },
  });
}

function gitResponse(service, kind, bytes) {
  return new Response(bytes, {
    headers: { "Content-Type": `application/x-${service}-${kind}`, "Cache-Control": "no-cache" },
  });
}

function validId(name) {
  return typeof name === "string" && ID.test(name) && !/\s/.test(name);
}

function validRef(reference) {
  return typeof reference === "string" && reference.isWellFormed() && reference.startsWith("refs/") &&
    te.encode(reference).length <= MAX_QUERY && !/[\x00-\x20\x7f~^:?*\[\\]/.test(reference) &&
    !reference.includes("..") && !reference.includes("@{") && !reference.endsWith(".") &&
    reference.split("/").every(part => part && !part.startsWith(".") && !part.endsWith(".lock"));
}

function queryValue(query, name) {
  const values = query.getAll(name);
  const value = values[0];
  if (values.length !== 1 || !value || value.includes("\0") || te.encode(value).length > MAX_QUERY) {
    throw new HttpError(422);
  }
  return value;
}

function queryLimit(query, optional = false, maximum = Number.MAX_SAFE_INTEGER) {
  if (optional && !query.has("limit")) return null;
  const value = queryValue(query, "limit");
  const number = Number(value);
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(number) || number > maximum) throw new HttpError(422);
  return number;
}

function validOid(value) {
  return typeof value === "string" && value.length === 40 && isOid(value);
}

function queryOid(query) {
  const value = queryValue(query, "oid");
  if (!validOid(value)) throw new HttpError(422);
  return value;
}

async function readBytes(body, limit) {
  if (!body) return new Uint8Array();
  const reader = body.getReader();
  const parts = [];
  let size = 0;
  let done = false;
  try {
    for (;;) {
      const next = await reader.read();
      if (next.done) {
        done = true;
        return parts.length === 1 ? parts[0] : concat(parts);
      }
      size += next.value.length;
      if (size > limit) throw new HttpError(413);
      if (next.value.length) parts.push(next.value);
    }
  } finally {
    if (!done) await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

async function readJson(request) {
  if (Number(request.headers.get("Content-Length")) > MAX_CONTROL) throw new HttpError(413);
  const bytes = await readBytes(request.body, MAX_CONTROL);
  let value;
  try {
    value = JSON.parse(td.decode(bytes));
  } catch {
    throw new HttpError(422);
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new HttpError(422);
  return value;
}

function leasedResponse(response, release) {
  const reader = response.body.getReader();
  let finished = false;
  let canceling = false;
  function finish() {
    if (finished) return;
    finished = true;
    reader.releaseLock();
    release();
  }
  const stream = new ReadableStream({
    async pull(controller) {
      try {
        const { done, value } = await reader.read();
        if (canceling) return;
        if (!done) {
          controller.enqueue(value);
          return;
        }
      } catch (error) {
        if (canceling) return;
        finish();
        controller.error(error);
        return;
      }
      finish();
      controller.close();
    },
    async cancel(reason) {
      canceling = true;
      try {
        await reader.cancel(reason);
      } finally {
        finish();
      }
    },
  });
  return new Response(stream, response);
}

function subject(message) {
  const paragraph = /^\s*([\s\S]*?)(?:\n[ \t\r]*\n|$)/.exec(message)[1];
  return paragraph.split("\n").map(line => line.trimEnd()).join(" ");
}

function commitJson(hash, commit, fullMessage) {
  return {
    hash,
    tree: commit.tree,
    message: fullMessage ? commit.message : subject(commit.message),
    author: commit.author.name,
    author_email: commit.author.email,
    timestamp: commit.author.time,
    parents: commit.parents,
  };
}

function importUrl(source) {
  if (typeof source !== "string" || !/^https:\/\//i.test(source) || /[\x00-\x20\x7f\\?#]/.test(source)) {
    throw new HttpError(422);
  }
  let url;
  try {
    url = new URL(source);
  } catch {
    throw new HttpError(422);
  }
  const authority = source.slice(8).split("/", 1)[0].toLowerCase();
  if (!IMPORT_HOSTS.has(url.hostname) || url.pathname === "/" ||
      (authority !== url.hostname && authority !== `${url.hostname}:443`)) throw new HttpError(422);
  return url;
}

function providerEndpoint(source, action) {
  const url = new URL(source);
  url.pathname = `${url.pathname.replace(/\/+$/, "")}/${action}`;
  return url;
}

function providerReply(response, mime) {
  if (response.status === 200 && response.headers.get("Content-Type")?.split(";", 1)[0].trim() === mime) return;
  void response.body?.cancel().catch(() => {});
  throw new HttpError(response.status === 404 ? 404 : 422);
}

function readAdvertisement(bytes) {
  const parser = new PktParser(bytes);
  if (parser.read()?.text !== `# service=${UPLOAD}` || parser.read()?.kind !== "flush") throw new HttpError(422);
  let line = parser.read();
  if (line?.text === "version 1") line = parser.read();
  const refs = [];
  const names = new Set();
  const caps = line?.text.split("\0", 2)[1]?.split(" ") ?? [];
  const format = caps.find(cap => cap.startsWith("object-format="));
  if (format && format !== "object-format=sha1") throw new HttpError(422);
  const symbolic = caps.find(cap => cap.startsWith("symref=HEAD:"))?.slice("symref=HEAD:".length);
  if (symbolic !== undefined && (!symbolic.startsWith(HEADS) || !validRef(symbolic))) throw new HttpError(422);
  let headOid;
  while (line?.kind === "line") {
    const [oid, name] = line.text.split("\0", 1)[0].split(" ");
    if (!validOid(oid) || !name) throw new HttpError(422);
    if (name === "HEAD") {
      headOid = oid;
    } else if (!name.endsWith("^{}")) {
      if (oid === ZERO_OID || !validRef(name) || names.has(name) || refs.length === MAX_COPY_REFS) {
        throw new HttpError(422);
      }
      names.add(name);
      refs.push({ name, target: oid });
    }
    line = parser.read();
  }
  if (line?.kind !== "flush" || parser.rest().length) throw new HttpError(422);
  const candidates = headOid ? refs.filter(ref => ref.target === headOid && ref.name.startsWith(HEADS)) : [];
  const head = symbolic ?? candidates.find(ref => ref.name === MAIN)?.name ??
    candidates.find(ref => ref.name === `${HEADS}master`)?.name ?? candidates[0]?.name ?? MAIN;
  return { head, refs, ofsDelta: caps.includes("ofs-delta") };
}

function fetchRequest(refs, ofsDelta = false) {
  const wants = [...new Set(refs.map(ref => ref.target))];
  return concat([
    ...wants.map((oid, index) => pkt(`want ${oid}${index === 0 && ofsDelta ? " ofs-delta" : ""}\n`)),
    FLUSH,
    pkt("done\n"),
  ]);
}

function unpackResponse(bytes) {
  const parser = new PktParser(bytes);
  if (parser.read()?.text !== "NAK") throw new HttpError(422);
  const pack = parser.rest();
  if (pack.length < 32 || td.decode(pack.subarray(0, 4)) !== "PACK") throw new HttpError(422);
  return pack;
}

export class GitRepository extends RepoCell {
  #creating = false;
  #writes = null;
  #readers = 0;
  #idle = null;
  #preparingPack = null;

  constructor(ctx, env) {
    super(ctx, env);
    const head = this.store.getMeta(SYMBOLIC_HEAD);
    if (head && this.store.head() !== head) this.store.setHead(head);
    this.#dropOrphanPacks();
  }

  get #maxPush() {
    return (Number.parseInt(this.env.MAX_PUSH_MB, 10) || 8) * MIB;
  }

  async fetch(request) {
    try {
      const url = new URL(request.url);
      const control = CONTROL.exec(url.pathname);
      const route = control ?? SMART.exec(url.pathname);
      if (url.origin !== INTERNAL || !route || !validId(route[1])) return empty(404);
      const [, name, action] = route;
      if (!this.ctx.id.equals(this.env.GIT_REPOS.idFromName(name))) return empty(404);
      return await (control ? this.#control(request, url.searchParams, name, action) :
        this.#smart(request, url.searchParams, name, action));
    } catch (error) {
      if (error instanceof HttpError) return empty(error.status);
      console.error("Durable Git request failed", error);
      return empty(500);
    }
  }

  async alarm() {
    // PR snapshots can refer to objects that a force push or ref deletion unrefs.
    // RepoCell schedules a reachability sweep after such a push. Retain those
    // objects until the repository itself is deleted.
    if (this.store.getMeta("gc-pending") === "1") this.store.setMeta("gc-pending", "0");
  }

  #existing(name) {
    return this.store.getMeta(READY) === name;
  }

  #metadata(name, status) {
    return json({ name, defaultBranch: this.store.head().slice(HEADS.length) }, status);
  }

  async #read(name, operation, stream = false) {
    // Upload-pack can walk the persisted reachability cache throughout a
    // response. Keep the source stable until the pack ends or is canceled.
    while (this.#writes) await this.#writes;
    if (!this.#existing(name)) return empty(404);
    this.#readers++;
    let active = true;
    const release = () => {
      if (!active) return;
      active = false;
      if (--this.#readers === 0) this.#idle?.resolve();
    };
    let held = false;
    try {
      const response = await operation();
      if (!stream || !response.body) return response;
      const wrapped = leasedResponse(response, release);
      held = true;
      return wrapped;
    } finally {
      if (!held) release();
    }
  }

  async #write(name, operation) {
    if (!this.#existing(name)) return empty(404);
    const previous = this.#writes;
    const next = Promise.withResolvers();
    this.#writes = next.promise;
    try {
      await previous;
      if (this.#readers) {
        this.#idle = Promise.withResolvers();
        await this.#idle.promise;
        this.#idle = null;
      }
      return await operation();
    } finally {
      if (this.#writes === next.promise) this.#writes = null;
      next.resolve();
    }
  }

  #reset() {
    this.store = this.ctx.storage.transactionSync(() => {
      this.store.resetPushEdges();
      this.store.wipe();
      return new GitStore(this.ctx.storage.sql);
    });
  }

  #dropOrphanPacks() {
    // An interrupted ingest may have written index entries before completing
    // its pack. RepoCell reclaims those rows at the next push. Do this before
    // serving reads too, since the pack lookup does not join pack_meta.
    this.ctx.storage.transactionSync(() => {
      const sql = this.ctx.storage.sql;
      const last = sql.exec("SELECT COALESCE(MAX(pack_id), 0) AS id FROM pack_meta").one().id;
      let removed = 0;
      for (const table of ["pack_objects", "pack_data", "pack_pending"]) {
        removed += sql.exec(`DELETE FROM ${table} WHERE pack_id > ?`, last).rowsWritten;
      }
      if (removed) this.store = new GitStore(sql);
    });
  }

  async #control(request, query, name, action) {
    if (!action) {
      if (request.method === "POST") return this.#create(name, request);
      if (request.method === "GET") {
        return this.#creating ? empty(202) : this.#read(name, () => this.#metadata(name));
      }
      if (request.method === "DELETE") {
        if (this.#creating || this.#readers || this.#writes) return empty(202);
        if (this.#existing(name) || this.store.getMeta(STAGING) === "1") this.#reset();
        return empty(204);
      }
    }
    if ((action === "ref" || action === "head") && request.method === "POST" && query.size === 0) {
      return this.#write(name, () => this.#updateRefs(action, request));
    }
    if (action === "merge" && request.method === "POST" && query.size === 0) {
      return this.#read(name, () => empty(501));
    }
    if (action === "fork" && request.method === "GET" && query.size === 0) {
      return this.#read(name, () => this.#forkSnapshot());
    }
    if (["log", "commit", "tree", "blob", "file"].includes(action) && request.method === "GET") {
      return this.#read(name, () => this.#object(action, query));
    }
    return empty(404);
  }

  async #create(name, request) {
    if (this.#creating) return empty(202);
    if (this.#existing(name)) return empty(409);
    this.#creating = true;
    try {
      const { kind, source, credential } = await readJson(request);
      if (credential !== undefined) throw new HttpError(422);
      if (kind === "create" && source === undefined) {
        if (this.store.getMeta(STAGING) === "1") this.#reset();
        this.#publish(name, MAIN, []);
      } else if (kind === "fork" || kind === "import") {
        if (kind === "fork" && (!validId(source) || source === name)) throw new HttpError(422);
        const { head, refs, pack } = kind === "fork" ? await this.#copyFork(source) : await this.#import(source);
        if (this.store.getMeta(STAGING) === "1") this.#reset();
        this.store.setMeta(STAGING, "1");
        try {
          await this.#ingestCopy(refs, pack);
          this.#publish(name, head, refs);
        } catch (error) {
          this.#reset();
          throw error;
        }
      } else {
        throw new HttpError(422);
      }
      return this.#metadata(name, 201);
    } finally {
      this.#creating = false;
    }
  }

  #publish(name, head, refs) {
    this.ctx.storage.transactionSync(() => {
      for (const ref of refs) this.store.setRef(ref.name, ref.target);
      this.store.setHead(head);
      this.store.setMeta(SYMBOLIC_HEAD, head);
      this.store.setMeta(STAGING, "0");
      this.store.setMeta(READY, name);
    });
  }

  async #preparePack(request) {
    // Only one cold walk may rebuild the shared reachable table. A second
    // rebuild can otherwise clear rows already being streamed by a warm clone.
    while (this.#preparingPack && !this.store.reachableCacheValid()) await this.#preparingPack;
    if (this.store.reachableCacheValid()) return super.fetch(request);
    const preparation = Promise.withResolvers();
    this.#preparingPack = preparation.promise;
    try {
      return await super.fetch(request);
    } finally {
      this.#preparingPack = null;
      preparation.resolve();
    }
  }

  async #forkSnapshot() {
    const head = this.store.head();
    const refs = this.store.refs();
    const metadata = te.encode(JSON.stringify({ head, refs }));
    if (refs.length > MAX_COPY_REFS || metadata.length > MAX_ADVERTISEMENT) throw new HttpError(413);
    const prefix = new Uint8Array(4);
    new DataView(prefix.buffer).setUint32(0, metadata.length);
    let pack = new Uint8Array();
    if (refs.length) {
      const req = new Request(`${INTERNAL}/${UPLOAD}`, { method: "POST", body: fetchRequest(refs) });
      const response = await this.#preparePack(req);
      if (!response.ok) return response;
      pack = await readBytes(response.body, this.#maxPush + 8);
    }
    return new Response(concat([prefix, metadata, pack]), { headers: { "Content-Type": SNAPSHOT } });
  }

  async #copyFork(source) {
    const namespace = this.env.GIT_REPOS;
    const stub = namespace.get(namespace.idFromName(source));
    const response = await stub.fetch(`${INTERNAL}/v1/repos/${source}/fork`);
    if (!response.ok) {
      void response.body?.cancel().catch(() => {});
      if (response.status === 404) throw new HttpError(404);
      if (response.status === 413) throw new HttpError(422);
      throw new HttpError(500);
    }
    if (response.headers.get("Content-Type") !== SNAPSHOT) throw new Error("Invalid Git fork response");
    const bytes = await readBytes(response.body, this.#maxPush + MAX_ADVERTISEMENT + 12);
    if (bytes.length < 4) throw new Error("Invalid Git fork metadata");
    const length = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0);
    if (length > MAX_ADVERTISEMENT || length > bytes.length - 4) throw new Error("Invalid Git fork metadata");
    const { head, refs } = JSON.parse(td.decode(bytes.subarray(4, 4 + length)));
    const pack = refs.length ? unpackResponse(bytes.subarray(4 + length)) : null;
    return { head, refs, pack };
  }

  async #import(value) {
    const source = importUrl(value);
    const refsUrl = providerEndpoint(source, "info/refs");
    refsUrl.searchParams.set("service", UPLOAD);
    const signal = AbortSignal.timeout(20_000);
    try {
      const advertisement = await fetch(refsUrl, {
        redirect: "manual",
        headers: { Accept: `application/x-${UPLOAD}-advertisement`, "Git-Protocol": "version=0" },
        signal,
      });
      providerReply(advertisement, `application/x-${UPLOAD}-advertisement`);
      const { head, refs, ofsDelta } = readAdvertisement(await readBytes(advertisement.body, MAX_ADVERTISEMENT));
      if (!refs.length) return { head, refs, pack: null };
      const response = await fetch(providerEndpoint(source, UPLOAD), {
        method: "POST",
        redirect: "manual",
        headers: { "Content-Type": `application/x-${UPLOAD}-request`, Accept: `application/x-${UPLOAD}-result` },
        body: fetchRequest(refs, ofsDelta),
        signal,
      });
      providerReply(response, `application/x-${UPLOAD}-result`);
      const pack = unpackResponse(await readBytes(response.body, this.#maxPush + 8));
      return { head, refs, pack };
    } catch (error) {
      if (error instanceof HttpError && error.status === 404) throw error;
      throw new HttpError(422);
    }
  }

  async #ingestCopy(refs, pack) {
    if (!pack) return;
    if (pack.length > this.#maxPush) throw new HttpError(422);
    try {
      const { packId } = await this.store.packs.ingest(pack, null, {
        maxBytes: this.#maxPush,
        cache: new ObjCache(16 * MIB),
        collisionDetect: this.env.SHA1DC === "1",
        flush: () => this.ctx.storage.sync(),
      });
      // Ref names were already accepted at the source. Use the engine's push
      // validation here to check connectivity under its own ref-name rules.
      const commands = refs.map(({ target }, index) => ({ ref: `refs/shard-copy/${index}`, old: ZERO_OID, next: target }));
      const plan = await validatePush(this.store, commands, null, packId);
      if (plan.decisions.some(decision => decision.reject) ||
          refs.some(({ name, target }) => name.startsWith(HEADS) && this.store.typeAndSize(target)?.type !== "commit")) {
        throw new HttpError(422);
      }
    } catch (error) {
      if (error instanceof HttpError) throw error;
      console.error("Durable Git copy pack failed", error);
      throw new HttpError(422);
    }
  }

  async #updateRefs(action, request) {
    const { reference, newOid, oldOid } = await readJson(request);
    if (!validRef(reference) || !(reference.startsWith(HEADS) || action === "ref" && reference.startsWith(TAGS))) {
      throw new HttpError(422);
    }
    return this.ctx.storage.transactionSync(() => {
      const current = this.store.getRef(reference) ?? ZERO_OID;
      if (action === "head") {
        if (current === ZERO_OID || this.store.typeAndSize(current)?.type !== "commit") throw new HttpError(404);
        this.store.setHead(reference);
        this.store.setMeta(SYMBOLIC_HEAD, reference);
      } else {
        if (!validOid(newOid) || !validOid(oldOid) || newOid === ZERO_OID && oldOid === ZERO_OID) {
          throw new HttpError(422);
        }
        if (current !== oldOid) throw new HttpError(412);
        if (newOid === ZERO_OID) {
          if (this.store.head() === reference) throw new HttpError(422);
          this.store.delRef(reference);
        } else {
          const object = this.store.typeAndSize(newOid);
          if (!object || reference.startsWith(HEADS) && object.type !== "commit") throw new HttpError(404);
          if (current === ZERO_OID && this.store.refs().some(({ name }) =>
            name.startsWith(`${reference}/`) || reference.startsWith(`${name}/`))) throw new HttpError(422);
          this.store.setRef(reference, newOid);
        }
      }
      this.store.invalidateReachable();
      return empty(204);
    });
  }

  async #load(oid, type) {
    const meta = this.store.typeAndSize(oid);
    if (meta?.type !== type) throw new HttpError(404);
    if (meta.size > MAX_METADATA) throw new HttpError(413);
    const object = await this.store.get(oid);
    if (object?.type !== type) throw new HttpError(404);
    return object.data;
  }

  async #peel(oid) {
    for (let depth = 0; oid && depth < 64; depth++) {
      const meta = this.store.typeAndSize(oid);
      if (!meta) return null;
      if (meta.type !== "tag") return { oid, type: meta.type };
      oid = parseTag(await this.#load(oid, "tag")).object;
    }
    return null;
  }

  async #resolveCommit(ref) {
    const oid = ref === "HEAD" ? this.store.resolveHead() : validOid(ref) ? ref :
      this.store.getRef(ref) ?? this.store.getRef(`refs/${ref}`) ??
      this.store.getRef(`${TAGS}${ref}`) ?? this.store.getRef(`${HEADS}${ref}`) ??
      this.store.getRef(`refs/remotes/${ref}`) ?? this.store.getRef(`refs/remotes/${ref}/HEAD`) ?? this.store.findOid(ref);
    const peeled = await this.#peel(oid);
    return peeled?.type === "commit" ? peeled.oid : null;
  }

  async #file(ref, path, limit) {
    const parts = path.split("/");
    if (parts.some(part => !part || part === "." || part === "..")) throw new HttpError(422);
    const commit = await this.#resolveCommit(ref);
    if (!commit) throw new HttpError(404);
    let oid = parseCommit(await this.#load(commit, "commit")).tree;
    for (let index = 0; index < parts.length; index++) {
      const entries = parseTree(await this.#load(oid, "tree"));
      const entry = entries.find(candidate => candidate.name === parts[index]);
      if (!entry || isTreeMode(entry.mode) !== (index + 1 < parts.length) || isGitlinkMode(entry.mode)) {
        throw new HttpError(404);
      }
      oid = entry.oid;
    }
    return this.#blob(oid, limit, true);
  }

  async #blob(oid, limit, file = false) {
    const meta = this.store.typeAndSize(oid);
    if (meta?.type !== "blob") throw new HttpError(404);
    if (!file && meta.size > limit) throw new HttpError(413);
    const size = file && limit !== null ? Math.min(limit, meta.size) : meta.size;
    const headers = { "Content-Type": "application/octet-stream", "Content-Length": String(size), "Cache-Control": "no-store" };
    if (file) headers["X-Shard-Object-Size"] = String(meta.size);
    if (!size) return new Response(new Uint8Array(), { headers });
    const object = await this.store.get(oid);
    if (object?.type !== "blob" || object.data.length !== meta.size) throw new Error("Invalid Git blob");
    return new Response(object.data.subarray(0, size), { headers });
  }

  async #object(action, query) {
    switch (action) {
      case "log": {
        const reference = queryValue(query, "ref");
        const limit = queryLimit(query, false, 0x7fffffff);
        const entries = [];
        let size = 2;
        let oid = limit ? await this.#resolveCommit(reference) : null;
        while (oid && entries.length < limit) {
          const commit = parseCommit(await this.#load(oid, "commit"));
          const entry = commitJson(oid, commit, false);
          size += te.encode(JSON.stringify(entry)).length + 1;
          if (size > MAX_METADATA) throw new HttpError(413);
          entries.push(entry);
          oid = commit.parents[0];
        }
        return json(entries);
      }
      case "commit": {
        const oid = queryOid(query);
        return json(commitJson(oid, parseCommit(await this.#load(oid, "commit")), true));
      }
      case "tree": {
        const limit = queryLimit(query);
        const entries = parseTree(await this.#load(queryOid(query), "tree"));
        if (entries.length > limit) throw new HttpError(413);
        return json(entries.map(({ name, mode, oid }) => {
          if (!name || name === "." || name === ".." || name.includes("/")) throw new Error("Invalid Git tree name");
          const entry = { name, mode: mode.padStart(6, "0"), hash: oid };
          if (!isTreeMode(mode) && !isGitlinkMode(mode)) {
            const object = this.store.typeAndSize(oid);
            if (object?.type !== "blob") throw new Error("Invalid Git tree entry");
            entry.size = object.size;
          }
          return entry;
        }));
      }
      case "blob":
        return this.#blob(queryOid(query), queryLimit(query));
      case "file":
        return this.#file(queryValue(query, "ref"), queryValue(query, "path"), queryLimit(query, true));
    }
  }

  async #smart(request, query, name, action) {
    if (action === "info/refs" && request.method === "GET") {
      const services = query.getAll("service");
      const service = services[0];
      if (services.length === 1 && (service === UPLOAD || service === RECEIVE)) {
        return this.#read(name, () => this.#advertise(request, service));
      }
    }
    if (action !== "info/refs" && request.method === "POST" && query.size === 0) {
      const run = () => {
        if (request.headers.get("Content-Type") !== `application/x-${action}-request`) return empty(415);
        return action === RECEIVE ? this.#receive(request, name) : this.#upload(request, name);
      };
      return action === RECEIVE ? this.#write(name, run) : this.#read(name, run, true);
    }
    return empty(404);
  }

  async #advertise(request, service) {
    if (service === RECEIVE) return gitResponse(service, "advertisement", advertisement(this.store, service));
    const v2 = request.headers.get("Git-Protocol")?.split(":").some(part => part.trim() === "version=2");
    if (v2) {
      return gitResponse(service, "advertisement", concat([
        pkt("version 2\n"), pkt("ls-refs=unborn\n"), pkt("fetch=shallow\n"), FLUSH,
      ]));
    }
    const base = advertisement(this.store, service);
    const peeled = [];
    for (const { name, target } of this.store.refs()) {
      if (!name.startsWith(TAGS) || this.store.typeAndSize(target)?.type !== "tag") continue;
      const resolved = await this.#peel(target);
      if (resolved) peeled.push(pkt(`${resolved.oid} ${name}^{}\n`));
    }
    return gitResponse(service, "advertisement", peeled.length ?
      concat([base.subarray(0, -4), ...peeled, FLUSH]) : base);
  }

  async #lsRefs(parser) {
    const prefixes = [];
    let symrefs = false;
    let peel = false;
    let unborn = false;
    for (let packet = parser.read(); packet; packet = parser.read()) {
      if (packet.kind === "flush") break;
      if (packet.text === "symrefs") symrefs = true;
      if (packet.text === "peel") peel = true;
      if (packet.text === "unborn") unborn = true;
      if (packet.text.startsWith("ref-prefix ")) prefixes.push(packet.text.slice("ref-prefix ".length));
    }
    const matches = name => !prefixes.length || prefixes.some(prefix => name.startsWith(prefix));
    const keys = [];
    const head = this.store.resolveHead();
    if (matches("HEAD")) {
      if (head) keys.push(pkt(`${head} HEAD${symrefs ? ` symref-target:${this.store.head()}` : ""}\n`));
      else if (unborn && symrefs) keys.push(pkt(`unborn HEAD symref-target:${this.store.head()}\n`));
    }
    for (const { name, target } of this.store.refs()) {
      if (!matches(name)) continue;
      const resolved = peel && this.store.typeAndSize(target)?.type === "tag" ? await this.#peel(target) : null;
      keys.push(pkt(`${target} ${name}${resolved ? ` peeled:${resolved.oid}` : ""}\n`));
    }
    return gitResponse(UPLOAD, "result", concat([...keys, FLUSH]));
  }

  async #upload(request, name) {
    if (Number(request.headers.get("Content-Length")) > MAX_UPLOAD_REQUEST) return empty(413);
    const encoding = request.headers.get("Content-Encoding")?.trim().toLowerCase();
    if (encoding && encoding !== "gzip" && encoding !== "identity") return empty(415);
    if (!request.body) return empty(400);
    const input = encoding === "gzip" ? request.body.pipeThrough(new DecompressionStream("gzip")) : request.body;
    try {
      const bytes = await readBytes(input, MAX_UPLOAD_REQUEST);
      const parser = new PktParser(bytes);
      if (parser.read()?.text === "command=ls-refs") return this.#lsRefs(parser);
      return await this.#preparePack(new Request(`${INTERNAL}/${UPLOAD}`, {
        method: "POST",
        headers: { "x-repo": name },
        body: bytes,
      }));
    } catch (error) {
      if (error instanceof HttpError) throw error;
      return empty(400);
    }
  }

  async #receive(request, name) {
    if (Number(request.headers.get("Content-Length")) > this.#maxPush) return empty(413);
    const encoding = request.headers.get("Content-Encoding")?.trim().toLowerCase();
    if (encoding && encoding !== "gzip" && encoding !== "identity") return empty(415);
    if (!request.body) return empty(400);
    const input = encoding === "gzip" ? request.body.pipeThrough(new DecompressionStream("gzip")) : request.body;
    const reader = input.getReader();
    const limit = this.#maxPush;
    let size = 0;
    let oversized = false;
    const bounded = new ReadableStream({
      async pull(controller) {
        try {
          const next = await reader.read();
          if (next.done) {
            reader.releaseLock();
            controller.close();
          } else if ((size += next.value.length) > limit) {
            oversized = true;
            controller.error(new Error("request body exceeds maximum size"));
            await reader.cancel().catch(() => {});
            reader.releaseLock();
          } else {
            controller.enqueue(next.value);
          }
        } catch (error) {
          reader.releaseLock();
          controller.error(error);
        }
      },
      async cancel(reason) {
        try {
          await reader.cancel(reason);
        } finally {
          reader.releaseLock();
        }
      },
    });
    let push;
    try {
      push = await shard_receive_request(bounded, "");
    } catch {
      return empty(oversized ? 413 : 400);
    }
    const head = this.store.head();
    const updates = JSON.parse(push.updates);
    // A settings change can reach this cell before the matching D1 update.
    // Check the actual symbolic HEAD under the same lock as receive-pack.
    if (updates.some(({ name, before, after }) => name === head && before !== ZERO_OID && after === ZERO_OID)) {
      void push.body.cancel().catch(() => {});
      if (oversized) return empty(413);
      if (!push.report) return empty(409);
      const results = updates.map(({ name }) => ({ ref: name, ok: false, msg: "cannot delete the default branch" }));
      return gitResponse(RECEIVE, "result", renderStatus(results, null, push.sideband));
    }
    try {
      const response = await super.fetch(new Request(`${INTERNAL}/${RECEIVE}`, {
        method: "POST",
        headers: { "x-repo": name },
        body: push.body,
      }));
      return oversized ? empty(413) : response;
    } finally {
      // RepoCell can select another branch on push. Shard's configured symbolic
      // HEAD changes only through /head or initial repository publication.
      if (this.store.head() !== head) this.store.setHead(head);
      this.#dropOrphanPacks();
      if (this.store.getMeta("gc-pending") === "1") {
        this.store.setMeta("gc-pending", "0");
        await this.ctx.storage.deleteAlarm().catch(() => {});
      }
    }
  }
}
