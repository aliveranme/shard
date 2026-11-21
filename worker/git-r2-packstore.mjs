// Metadata and raw-byte access for existing R2 packs. The Git protocol emits
// stored entries verbatim when possible, avoiding an inflate/recompress cycle.
import { PackIndex, parsePackEntry } from "./git-r2-pack.mjs";

const MiB = 1024 ** 2;
const TYPES = [null, "commit", "tree", "blob", "tag"];
const CHUNK = MiB;

export class R2PackStore {
  constructor(bucket, packs, sizes, entries, total, cache) {
    this.bucket = bucket;
    this.packs = packs;
    this.sizes = sizes;
    this.entries = entries;
    this.total = total;
    this.cache = cache;
  }

  lookup(oid) {
    return this.entries.get(oid) ?? null;
  }

  oidsOfTypes(types, limit = 4_096) {
    const accepted = types instanceof Set ? types : new Set(types);
    const oids = [];
    for (const entry of this.entries.values()) {
      if (entry.type && accepted.has(entry.type)) {
        oids.push(entry.oid);
        if (oids.length >= limit) break;
      }
    }
    return oids;
  }

  totalPackBytes() {
    return this.total;
  }

  rawCacheWindow() {
    return CHUNK;
  }

  async readRaw(packId, offset, length) {
    if (!Number.isInteger(packId) || packId < 1 || packId > this.packs.length) throw new Error("Invalid pack id");
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length < 0) {
      throw new Error("Invalid pack range");
    }
    const first = Math.floor(offset / CHUNK);
    const last = Math.floor((offset + Math.max(length, 1) - 1) / CHUNK);
    if (first === last) {
      const chunk = await this.#chunk(packId, first);
      const start = offset - first * CHUNK;
      if (start + length > chunk.length) throw new Error("Incomplete Git pack range");
      return chunk.subarray(start, start + length);
    }
    const data = new Uint8Array(length);
    let copied = 0;
    for (let seq = first; seq <= last; seq++) {
      const chunk = await this.#chunk(packId, seq);
      const from = Math.max(offset, seq * CHUNK);
      const to = Math.min(offset + length, seq * CHUNK + chunk.length);
      if (to > from) {
        data.set(chunk.subarray(from - seq * CHUNK, to - seq * CHUNK), from - offset);
        copied += to - from;
      }
    }
    if (copied !== length) throw new Error("Incomplete Git pack range");
    return data;
  }

  async #chunk(packId, seq) {
    const key = `${packId}:${seq}`;
    const cached = this.cache.get(key);
    if (cached) return cached;
    const pack = this.packs[packId - 1];
    const object = await this.bucket.get(`${pack}.pack`, { range: { offset: seq * CHUNK, length: CHUNK } });
    if (!object) throw new Error("Missing Git pack range");
    const data = new Uint8Array(await object.arrayBuffer());
    if (this.cache.size >= 4) this.cache.clear();
    this.cache.set(key, data);
    return data;
  }
}

class PackHeaders {
  constructor(bucket, key) {
    this.bucket = bucket;
    this.key = key;
    this.chunks = new Map();
  }

  async read(offset, length) {
    const start = Math.floor(offset / CHUNK);
    const end = Math.floor((offset + length - 1) / CHUNK);
    if (start === end) {
      const chunk = await this.chunk(start);
      return chunk.subarray(offset - start * CHUNK, offset - start * CHUNK + length);
    }
    const data = new Uint8Array(length);
    let copied = 0;
    for (let seq = start; seq <= end; seq++) {
      const chunk = await this.chunk(seq);
      const from = Math.max(offset, seq * CHUNK);
      const to = Math.min(offset + length, seq * CHUNK + chunk.length);
      if (to > from) {
        data.set(chunk.subarray(from - seq * CHUNK, to - seq * CHUNK), from - offset);
        copied += to - from;
      }
    }
    if (copied !== length) throw new Error("Incomplete Git pack header");
    return data;
  }

  async chunk(seq) {
    const cached = this.chunks.get(seq);
    if (cached) return cached;
    const object = await this.bucket.get(`${this.key}.pack`, { range: { offset: seq * CHUNK, length: CHUNK } });
    if (!object || !object.size) throw new Error("Missing Git pack chunk");
    const data = new Uint8Array(await object.arrayBuffer());
    if (this.chunks.size >= 4) this.chunks.clear();
    this.chunks.set(seq, data);
    return data;
  }
}

export async function buildR2PackStore(bucket, packs, getIndex, maxObjects = 200_000) {
  const indexes = [];
  let count = 0;
  for (const key of packs) {
    const index = await getIndex(key);
    indexes.push(index);
    count += index.count;
    if (count > maxObjects) return null;
  }
  const sizes = [];
  const entries = new Map();
  const cache = new Map();
  let total = 0;
  for (let position = 0; position < packs.length; position++) {
    const key = packs[position];
    const index = indexes[position];
    const pack = await bucket.head(`${key}.pack`);
    if (!pack || !Number.isSafeInteger(pack.size) || pack.size < 32) return null;
    sizes.push(pack.size);
    total += pack.size;
    const headers = new PackHeaders(bucket, key);
    const order = index.offsetsAscending();
    const packEnd = pack.size - 20;
    const offsetOids = new Map();
    for (const entry of order) offsetOids.set(entry.offset, index.oidAt(entry.index));
    const types = new Map();
    for (let positionInPack = 0; positionInPack < order.length; positionInPack++) {
      const { offset, index: entryIndex } = order[positionInPack];
      const headerLen = Math.min(64, packEnd - offset);
      if (headerLen <= 0) return null;
      const raw = await headers.read(offset, headerLen);
      const parsed = parsePackEntry(raw, offset);
      const next = positionInPack + 1 < order.length ? order[positionInPack + 1].offset : packEnd;
      const dataOff = parsed.start;
      const dataLen = next - dataOff;
      if (!Number.isSafeInteger(dataLen) || dataLen <= 0) return null;
      let type = parsed.type >= 1 && parsed.type <= 4 ? TYPES[parsed.type] : null;
      let baseOid = null;
      if (parsed.type === 6) {
        baseOid = offsetOids.get(parsed.baseOffset) ?? null;
        type = types.get(parsed.baseOffset) ?? null;
      } else if (parsed.type === 7) {
        baseOid = parsed.baseOid;
        // A ref-delta base may live in another pack or later in this one.
        // Leave it to the object reader instead of guessing its type.
        type = null;
      }
      if (type) types.set(offset, type);
      const oid = index.oidAt(entryIndex);
      entries.set(oid, {
        oid, packId: position + 1, offset, dataOff, dataLen,
        type, size: parsed.size, entrySize: parsed.size, baseOid,
      });
    }
  }
  // Ref-deltas were stored without a type because their base may appear later
  // or in another pack. Resolve what can be resolved so the synchronous
  // metadata API used by upload-pack's graph walk sees the object type.
  for (const entry of entries.values()) {
    if (entry.type) continue;
    let base = entry.baseOid;
    for (let depth = 0; base && depth < 64; depth++) {
      const parent = entries.get(base);
      if (!parent) break;
      if (parent.type) {
        entry.type = parent.type;
        entry.size = parent.entrySize ?? parent.size;
        break;
      }
      base = parent.baseOid;
    }
  }
  return new R2PackStore(bucket, packs, sizes, entries, total, cache);
}