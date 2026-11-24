// Random access to Git objects stored in R2 pack and index files.
//
// The Durable Object's write path still uses native Git in a container. Web
// reads use this module so a page view does not pay for a container workspace.
import { Inflate } from "pako";

const OID = /^[0-9a-f]{40}$/;
const INDEX_MAGIC = 0xff744f63;
const TYPE_NAMES = [null, "commit", "tree", "blob", "tag"];

function be32(bytes, offset) {
  return (bytes[offset] << 24 | bytes[offset + 1] << 16 | bytes[offset + 2] << 8 | bytes[offset + 3]) >>> 0;
}

function be64(bytes, offset) {
  return be32(bytes, offset) * 0x100000000 + be32(bytes, offset + 4);
}

function hex(bytes, offset, length) {
  let value = "";
  for (let index = 0; index < length; index++) value += bytes[offset + index].toString(16).padStart(2, "0");
  return value;
}

function oidBytes(oid) {
  if (!OID.test(oid)) throw new Error("Invalid Git object ID");
  const bytes = new Uint8Array(20);
  for (let index = 0; index < 20; index++) bytes[index] = Number.parseInt(oid.slice(index * 2, index * 2 + 2), 16);
  return bytes;
}

function compareOid(bytes, offset, target) {
  for (let index = 0; index < 20; index++) {
    const difference = bytes[offset + index] - target[index];
    if (difference) return difference;
  }
  return 0;
}

export class PackIndex {
  constructor(bytes, limit = 32 * 1024 * 1024) {
    if (bytes.length < 8 + 1024 || be32(bytes, 0) !== INDEX_MAGIC || be32(bytes, 4) !== 2) {
      throw new Error("Unsupported Git pack index");
    }
    this.count = be32(bytes, 8 + 255 * 4);
    const shaStart = 8 + 1024;
    const crcStart = shaStart + this.count * 20;
    const offsetStart = crcStart + this.count * 4;
    const largeStart = offsetStart + this.count * 4;
    if (largeStart + 40 > bytes.length || bytes.length > limit) throw new Error("Invalid Git pack index");
    this.bytes = bytes;
    this.fanout = 8;
    this.shaStart = shaStart;
    this.offsetStart = offsetStart;
    this.largeStart = largeStart;
    this.offsets = new Map();
  }

  find(oid) {
    const target = oidBytes(oid);
    const first = target[0];
    let low = first ? be32(this.bytes, this.fanout + (first - 1) * 4) : 0;
    let high = be32(this.bytes, this.fanout + first * 4);
    while (low < high) {
      const middle = (low + high) >>> 1;
      const difference = compareOid(this.bytes, this.shaStart + middle * 20, target);
      if (difference === 0) return this.offsetAt(middle);
      if (difference < 0) low = middle + 1;
      else high = middle;
    }
    return null;
  }

  offsetAt(index) {
    const cached = this.offsets.get(index);
    if (cached !== undefined) return cached;
    const value = be32(this.bytes, this.offsetStart + index * 4);
    let offset = value;
    if (value & 0x80000000) {
      const slot = value & 0x7fffffff;
      if (this.largeStart + (slot + 1) * 8 > this.bytes.length) throw new Error("Invalid Git pack offset");
      offset = be64(this.bytes, this.largeStart + slot * 8);
    }
    if (!Number.isSafeInteger(offset) || offset < 12) throw new Error("Invalid Git pack offset");
    this.offsets.set(index, offset);
    return offset;
  }

  oidAt(index) {
    if (!Number.isInteger(index) || index < 0 || index >= this.count) throw new Error("Invalid Git pack index");
    return hex(this.bytes, this.shaStart + index * 20, 20);
  }

  offsetsAscending() {
    if (this.order) return this.order;
    this.order = Array.from({ length: this.count }, (_, index) => ({ offset: this.offsetAt(index), index }));
    this.order.sort((left, right) => left.offset - right.offset);
    return this.order;
  }
}

export function parsePackEntry(bytes, offset) {
  let cursor = 0;
  let byte = bytes[cursor++];
  const type = (byte >> 4) & 7;
  let size = byte & 15;
  let shift = 4;
  while (byte & 0x80) {
    if (cursor >= bytes.length || shift > 56) throw new Error("Invalid Git pack entry header");
    byte = bytes[cursor++];
    size += (byte & 0x7f) * 2 ** shift;
    shift += 7;
  }
  let baseOffset;
  let baseOid;
  if (type === 6) {
    if (cursor >= bytes.length) throw new Error("Invalid Git delta base");
    byte = bytes[cursor++];
    let distance = byte & 0x7f;
    while (byte & 0x80) {
      if (cursor >= bytes.length || distance > 0x7fffffff) throw new Error("Invalid Git delta base");
      byte = bytes[cursor++];
      distance = ((distance + 1) * 2 ** 7) + (byte & 0x7f);
    }
    baseOffset = offset - distance;
    if (!Number.isSafeInteger(baseOffset) || baseOffset < 12 || baseOffset >= offset) {
      throw new Error("Invalid Git delta base offset");
    }
  } else if (type === 7) {
    if (cursor + 20 > bytes.length) throw new Error("Invalid Git delta base");
    baseOid = hex(bytes, cursor, 20);
    cursor += 20;
  }
  return { type, size, baseOffset, baseOid, start: offset + cursor };
}

async function inflateAt(readRange, start, size, maxBytes, initial = new Uint8Array()) {
  if (!Number.isSafeInteger(size) || size < 0 || size > maxBytes) throw new Error("Git object exceeds the read limit");
  const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
  const chunks = [];
  let total = 0;
  inflater.onData = (chunk) => {
    total += chunk.length;
    if (total > size || total > maxBytes) throw new Error("Git object exceeded its declared size");
    chunks.push(chunk.slice());
  };
  let position = start + initial.length;
  let pending = initial;
  let stall = 0;
  while (!inflater.ended) {
    if (!pending.length) {
      pending = await readRange(position, Math.min(128 * 1024, Math.max(1024, size + 256)));
      if (!pending.length) throw new Error("Incomplete Git pack object");
      position += pending.length;
    }
    const before = pending.length;
    inflater.push(pending, false);
    if (inflater.err) throw new Error(`Corrupt Git pack object: ${inflater.msg || inflater.err}`);
    const consumed = before - inflater.strm.avail_in;
    pending = pending.subarray(consumed);
    if (inflater.ended) break;
    if (consumed === 0) {
      if (++stall > 2) throw new Error("Git pack inflation stalled");
    } else {
      stall = 0;
    }
  }
  if (inflater.err) throw new Error(`Corrupt Git pack object: ${inflater.msg || inflater.err}`);
  if (total !== size) throw new Error("Git pack object size mismatch");
  const data = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.length;
  }
  return data;
}

function deltaVarint(bytes, state) {
  let value = 0;
  let shift = 0;
  for (;;) {
    if (state.cursor >= bytes.length || shift > 35) throw new Error("Invalid Git delta");
    const byte = bytes[state.cursor++];
    value += (byte & 0x7f) * 2 ** shift;
    if (!(byte & 0x80)) return value;
    shift += 7;
  }
}

export function applyDelta(base, delta, maxBytes = 64 * 1024 * 1024) {
  const state = { cursor: 0 };
  const baseSize = deltaVarint(delta, state);
  const resultSize = deltaVarint(delta, state);
  if (baseSize !== base.length || resultSize > maxBytes) throw new Error("Invalid Git delta sizes");
  const result = new Uint8Array(resultSize);
  let written = 0;
  const nextByte = () => {
    if (state.cursor >= delta.length) throw new Error("Truncated Git delta copy");
    return delta[state.cursor++];
  };
  while (state.cursor < delta.length) {
    const command = delta[state.cursor++];
    if (command & 0x80) {
      let offset = 0;
      let size = 0;
      if (command & 0x01) offset += nextByte();
      if (command & 0x02) offset += nextByte() << 8;
      if (command & 0x04) offset += nextByte() << 16;
      if (command & 0x08) offset += nextByte() * 2 ** 24;
      if (command & 0x10) size += nextByte();
      if (command & 0x20) size += nextByte() << 8;
      if (command & 0x40) size += nextByte() << 16;
      if (!size) size = 0x10000;
      if (offset < 0 || size < 0 || offset + size > base.length || written + size > result.length) {
        throw new Error("Invalid Git delta copy");
      }
      result.set(base.subarray(offset, offset + size), written);
      written += size;
    } else if (command) {
      if (state.cursor + command > delta.length || written + command > result.length) {
        throw new Error("Invalid Git delta insert");
      }
      result.set(delta.subarray(state.cursor, state.cursor + command), written);
      state.cursor += command;
      written += command;
    } else {
      throw new Error("Invalid Git delta command");
    }
  }
  if (written !== result.length) throw new Error("Incomplete Git delta");
  return result;
}

function typeName(type) {
  const name = TYPE_NAMES[type];
  if (!name) throw new Error("Unsupported Git object type");
  return name;
}

async function verifyObject(oid, type, data) {
  const header = new TextEncoder().encode(`${typeName(type)} ${data.length}\0`);
  const object = new Uint8Array(header.length + data.length);
  object.set(header);
  object.set(data, header.length);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-1", object));
  let actual = "";
  for (const byte of digest) actual += byte.toString(16).padStart(2, "0");
  if (actual !== oid) throw new Error("Git object digest mismatch");
}

const MAX_CACHE_ENTRY = 1024 * 1024;
const MAX_CACHE_TOTAL = 4 * 1024 * 1024;

export async function readPackedObject(index, oid, {
  readRange,
  readBase,
  cache = new Map(),
  maxBytes = 64 * 1024 * 1024,
  verify = true,
  initialDepth = 0,
} = {}) {
  if (!OID.test(oid) || typeof readRange !== "function") return null;
  if (cache.has(`oid:${oid}`)) return cache.get(`oid:${oid}`);
  let cacheBytes = cache.__bytes ?? 0;
  const remember = (key, value) => {
    const size = value?.data?.length ?? 0;
    if (size > MAX_CACHE_ENTRY || cacheBytes + size > MAX_CACHE_TOTAL) return;
    if (!cache.has(key)) cacheBytes += size;
    cache.set(key, value);
    cache.__bytes = cacheBytes;
  };
  const readAt = async (startOffset, startDepth) => {
    const chain = [];
    const seenOffsets = new Set();
    let curOffset = startOffset;
    let curDepth = startDepth;
    let current = null;
    for (;;) {
      if (curDepth > 64) throw new Error("Git delta chain is too deep");
      const cached = cache.get(`at:${curOffset}`);
      if (cached) {
        current = cached;
        break;
      }
      if (seenOffsets.has(curOffset)) throw new Error("Git delta cycle detected");
      seenOffsets.add(curOffset);
      const header = await readRange(curOffset, 128 * 1024);
      const parsed = parsePackEntry(header, curOffset);
      const initial = parsed.start >= curOffset && parsed.start <= curOffset + header.length
        ? header.subarray(parsed.start - curOffset)
        : new Uint8Array();
      const data = await inflateAt(readRange, parsed.start, parsed.size, maxBytes, initial);
      if (parsed.type >= 1 && parsed.type <= 4) {
        current = { type: parsed.type, data };
        remember(`at:${curOffset}`, current);
        break;
      } else if (parsed.type === 6) {
        chain.push({ offset: curOffset, data });
        curOffset = parsed.baseOffset;
        curDepth += 1;
      } else if (parsed.type === 7) {
        const base = typeof readBase === "function" ? await readBase(parsed.baseOid, curDepth + 1) : null;
        if (!base) return null;
        current = { type: base.type, data: applyDelta(base.data, data, maxBytes) };
        remember(`at:${curOffset}`, current);
        break;
      } else {
        throw new Error("Unsupported Git pack entry type");
      }
    }
    for (let i = chain.length - 1; i >= 0; i--) {
      const step = chain[i];
      current = { type: current.type, data: applyDelta(current.data, step.data, maxBytes) };
      chain[i] = null;
      remember(`at:${step.offset}`, current);
    }
    return current;
  };
  const offset = index.find(oid);
  if (offset === null) return null;
  const object = await readAt(offset, initialDepth);
  if (!object) return null;
  if (verify) await verifyObject(oid, object.type, object.data);
  remember(`oid:${oid}`, object);
  return object;
}

const CANONICAL_TREE_MODES = new Set(["40000", "100644", "100755", "120000", "160000"]);
const IGNORABLE_GIT_CHARS = /[\u00ad\u034f\u115f\u1160\u17b4\u17b5\u180b-\u180d\u200b-\u200f\u202a-\u202e\u2060-\u206f\u3164\ufeff\uffa0]/g;

export function isForbiddenDotGit(name) {
  const stripped = name.replace(IGNORABLE_GIT_CHARS, "");
  const base = stripped.split(":", 1)[0].replace(/[. ]+$/, "");
  return /^(?:\.git|git~[1-9]\d*)$/i.test(base);
}

function isForbiddenGitmodules(name) {
  const stripped = name.replace(IGNORABLE_GIT_CHARS, "");
  const base = stripped.split(":", 1)[0].replace(/[. ]+$/, "");
  return /^(?:\.gitmodules|gitmod~[1-9]\d*)$/i.test(base);
}

export class PackLimitError extends Error {}

export class PackStreamValidator {
  #maxObjectBytes;
  #maxTotalInflated;
  #maxObjects;
  #state = "header";
  #seenBytes = 0;
  #header = new Uint8Array(12);
  #headerLen = 0;
  #count = 0;
  #index = 0;
  #entryFirst = true;
  #entryType = 0;
  #entrySize = 0;
  #entryShift = 4;
  #ofsBytes = 0;
  #refBytes = 0;
  #inflater = null;
  #inflated = 0;
  #stall = 0;
  #onDataError = null;
  #deltaBuf = new Uint8Array(20);
  #deltaLen = 0;
  #deltaTerminators = 0;
  #deltaParsed = false;
  #totalInflated = 0;
  #totalOutput = 0;
  #trailerBytes = 0;

  constructor({
    maxObjectBytes = 32 * 1024 * 1024,
    maxTotalInflated = 64 * 1024 * 1024,
    maxObjects = 200_000,
  } = {}) {
    this.#maxObjectBytes = maxObjectBytes;
    this.#maxTotalInflated = maxTotalInflated;
    this.#maxObjects = maxObjects;
  }

  #startInflate() {
    this.#inflated = 0;
    this.#stall = 0;
    this.#onDataError = null;
    this.#deltaLen = 0;
    this.#deltaTerminators = 0;
    this.#deltaParsed = false;
    const isDelta = this.#entryType === 6 || this.#entryType === 7;
    const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
    inflater.onData = (out) => {
      if (this.#onDataError) return;
      this.#inflated += out.length;
      if (this.#inflated > this.#entrySize || this.#inflated > this.#maxObjectBytes ||
          this.#totalInflated + this.#inflated > this.#maxTotalInflated) {
        this.#onDataError = new PackLimitError("Git pack inflated size exceeds limit");
        return;
      }
      if (isDelta && !this.#deltaParsed) {
        for (let i = 0; i < out.length && !this.#deltaParsed; i++) {
          if (this.#deltaLen >= this.#deltaBuf.length) {
            this.#onDataError = new Error("Invalid Git delta header");
            return;
          }
          const byte = out[i];
          this.#deltaBuf[this.#deltaLen++] = byte;
          if ((byte & 0x80) === 0) {
            this.#deltaTerminators++;
            if (this.#deltaTerminators === 2) {
              try {
                const state = { cursor: 0 };
                const slice = this.#deltaBuf.subarray(0, this.#deltaLen);
                const srcSize = deltaVarint(slice, state);
                const tgtSize = deltaVarint(slice, state);
                if (srcSize > this.#maxObjectBytes || tgtSize > this.#maxObjectBytes) {
                  this.#onDataError = new PackLimitError("Git delta object exceeds size limit");
                  return;
                }
                this.#totalOutput += tgtSize;
                if (this.#totalOutput > this.#maxTotalInflated) {
                  this.#onDataError = new PackLimitError("Git pack inflated size exceeds limit");
                  return;
                }
                this.#deltaParsed = true;
              } catch (error) {
                this.#onDataError = error;
                return;
              }
            }
          }
        }
      }
    };
    this.#inflater = inflater;
    this.#state = "inflate";
  }

  push(chunk) {
    if (!chunk?.length) return;
    this.#seenBytes += chunk.length;
    let pos = 0;
    while (pos < chunk.length) {
      if (this.#state === "header") {
        const take = Math.min(12 - this.#headerLen, chunk.length - pos);
        this.#header.set(chunk.subarray(pos, pos + take), this.#headerLen);
        this.#headerLen += take;
        pos += take;
        if (this.#headerLen === 12) {
          if (this.#header[0] !== 80 || this.#header[1] !== 65 || this.#header[2] !== 67 || this.#header[3] !== 75) {
            throw new Error("Invalid Git pack header");
          }
          const version = be32(this.#header, 4);
          if (version !== 2 && version !== 3) throw new Error("Unsupported Git pack version");
          this.#count = be32(this.#header, 8);
          if (this.#count > this.#maxObjects) throw new PackLimitError("Git pack has too many objects");
          this.#index = 0;
          this.#state = this.#count === 0 ? "trailer" : "entry_head";
          this.#entryFirst = true;
        }
      } else if (this.#state === "entry_head") {
        const byte = chunk[pos++];
        if (this.#entryFirst) {
          this.#entryType = (byte >> 4) & 7;
          if (!((this.#entryType >= 1 && this.#entryType <= 4) || this.#entryType === 6 || this.#entryType === 7)) {
            throw new Error(`Invalid Git pack entry type ${this.#entryType}`);
          }
          this.#entrySize = byte & 0x0f;
          this.#entryShift = 4;
          this.#entryFirst = false;
        } else {
          if (this.#entryShift > 56) throw new Error("Invalid Git pack entry header");
          this.#entrySize += (byte & 0x7f) * 2 ** this.#entryShift;
          this.#entryShift += 7;
        }
        if ((byte & 0x80) === 0) {
          if (!Number.isSafeInteger(this.#entrySize) || this.#entrySize < 0 || this.#entrySize > this.#maxObjectBytes) {
            throw new PackLimitError("Git object exceeds size limit");
          }
          if (this.#totalInflated + this.#entrySize > this.#maxTotalInflated) {
            throw new PackLimitError("Git pack inflated size exceeds limit");
          }
          if (this.#entryType >= 1 && this.#entryType <= 4 &&
              this.#totalOutput + this.#entrySize > this.#maxTotalInflated) {
            throw new PackLimitError("Git pack inflated size exceeds limit");
          }
          if (this.#entryType === 6) {
            this.#ofsBytes = 0;
            this.#state = "ofs_base";
          } else if (this.#entryType === 7) {
            this.#refBytes = 0;
            this.#state = "ref_base";
          } else {
            this.#startInflate();
          }
        }
      } else if (this.#state === "ofs_base") {
        const byte = chunk[pos++];
        if (++this.#ofsBytes > 10) throw new Error("Invalid Git delta base");
        if ((byte & 0x80) === 0) this.#startInflate();
      } else if (this.#state === "ref_base") {
        const take = Math.min(20 - this.#refBytes, chunk.length - pos);
        this.#refBytes += take;
        pos += take;
        if (this.#refBytes === 20) this.#startInflate();
      } else if (this.#state === "inflate") {
        const slice = chunk.subarray(pos);
        const before = slice.length;
        this.#inflater.push(slice, false);
        if (this.#onDataError) throw this.#onDataError;
        if (this.#inflater.err) throw new Error(`Corrupt Git pack entry: ${this.#inflater.msg || this.#inflater.err}`);
        const consumed = before - this.#inflater.strm.avail_in;
        pos += consumed;
        if (this.#inflater.ended) {
          if (this.#inflated !== this.#entrySize) throw new Error("Git pack entry size mismatch");
          if ((this.#entryType === 6 || this.#entryType === 7) && !this.#deltaParsed) {
            throw new Error("Invalid Git delta header");
          }
          this.#totalInflated += this.#inflated;
          if (this.#entryType >= 1 && this.#entryType <= 4) {
            this.#totalOutput += this.#inflated;
          }
          this.#inflater = null;
          this.#index++;
          if (this.#index === this.#count) {
            this.#state = "trailer";
          } else {
            this.#state = "entry_head";
            this.#entryFirst = true;
          }
        } else if (consumed === 0) {
          if (++this.#stall > 2) throw new Error("Git pack inflation stalled");
        } else {
          this.#stall = 0;
        }
      } else {
        this.#trailerBytes += chunk.length - pos;
        pos = chunk.length;
        if (this.#trailerBytes > 20) throw new Error("Unexpected bytes after Git pack entries");
      }
    }
  }

  finish(allowEmpty = false) {
    if (allowEmpty && this.#seenBytes === 0) return;
    if (this.#state !== "trailer" || this.#trailerBytes !== 20) {
      throw new Error("Truncated Git pack");
    }
  }
}

function compareTreeEntryBytes(leftBytes, leftIsDir, rightBytes, rightIsDir) {
  const len = Math.min(leftBytes.length, rightBytes.length);
  for (let i = 0; i < len; i++) {
    if (leftBytes[i] !== rightBytes[i]) return leftBytes[i] - rightBytes[i];
  }
  if (leftBytes.length === rightBytes.length) return 0;
  if (leftBytes.length < rightBytes.length) {
    const leftNext = leftIsDir ? 0x2f : -1;
    return leftNext - rightBytes[leftBytes.length];
  }
  const rightNext = rightIsDir ? 0x2f : -1;
  return leftBytes[rightBytes.length] - rightNext;
}

const IDENTITY_LINE = /^([^<>\n]+) <([^<>\n]*)> (0|[1-9]\d*) [+-]\d{4}$/;

export function parseCommit(oid, data) {
  if (data.includes(0)) throw new Error("Invalid Git commit");
  const text = new TextDecoder("utf-8", { fatal: false }).decode(data);
  const separator = text.indexOf("\n\n");
  const headers = separator < 0 ? text : text.slice(0, separator);
  const message = separator < 0 ? "" : text.slice(separator + 2);
  const lines = headers.split("\n");
  let index = 0;
  const treeMatch = /^tree ([0-9a-f]{40})$/.exec(lines[index] ?? "");
  if (!treeMatch) throw new Error("Invalid Git commit");
  const tree = treeMatch[1];
  index += 1;
  const parents = [];
  while (index < lines.length && lines[index].startsWith("parent ")) {
    const parentMatch = /^parent ([0-9a-f]{40})$/.exec(lines[index]);
    if (!parentMatch) throw new Error("Invalid Git commit");
    parents.push(parentMatch[1]);
    index += 1;
  }
  const authorLine = lines[index] ?? "";
  if (!authorLine.startsWith("author ")) throw new Error("Invalid Git commit");
  const author = IDENTITY_LINE.exec(authorLine.slice("author ".length));
  if (!author) throw new Error("Invalid Git commit");
  index += 1;
  const committerLine = lines[index] ?? "";
  if (!committerLine.startsWith("committer ")) throw new Error("Invalid Git commit");
  const committer = IDENTITY_LINE.exec(committerLine.slice("committer ".length));
  if (!committer) throw new Error("Invalid Git commit");
  index += 1;
  for (; index < lines.length; index++) {
    const line = lines[index];
    if (line.startsWith("tree ") || line.startsWith("parent ") || line.startsWith("author ") || line.startsWith("committer ")) {
      throw new Error("Invalid Git commit");
    }
  }
  const timestamp = Number(author[3]);
  const committerTimestamp = Number(committer[3]);
  if (!Number.isSafeInteger(timestamp) || !Number.isSafeInteger(committerTimestamp)) {
    throw new Error("Invalid Git commit timestamp");
  }
  return {
    hash: oid,
    tree,
    message,
    author: author[1],
    author_email: author[2],
    timestamp,
    parents,
  };
}

export function parseTree(data) {
  const entries = [];
  const seenNames = new Set();
  let cursor = 0;
  let prevNameBytes = null;
  let prevIsDir = false;
  while (cursor < data.length) {
    const end = data.indexOf(0, cursor);
    if (end < 0 || end + 21 > data.length) throw new Error("Invalid Git tree");
    const headerBytes = data.subarray(cursor, end);
    const space = headerBytes.indexOf(0x20);
    if (space < 1) throw new Error("Invalid Git tree entry");
    const mode = new TextDecoder("ascii").decode(headerBytes.subarray(0, space));
    if (!CANONICAL_TREE_MODES.has(mode)) throw new Error("Invalid Git tree mode");
    const nameBytes = headerBytes.subarray(space + 1);
    const name = new TextDecoder("utf-8", { fatal: false }).decode(nameBytes);
    if (!name || name === "." || name === ".." || name.includes("/") || isForbiddenDotGit(name) ||
        (mode === "120000" && isForbiddenGitmodules(name))) {
      throw new Error("Invalid Git tree name");
    }
    let nameKey = "";
    for (let i = 0; i < nameBytes.length; i++) nameKey += String.fromCharCode(nameBytes[i]);
    if (seenNames.has(nameKey)) throw new Error("Duplicate Git tree entry");
    seenNames.add(nameKey);
    const isDir = mode === "40000";
    if (prevNameBytes !== null && compareTreeEntryBytes(prevNameBytes, prevIsDir, nameBytes, isDir) >= 0) {
      throw new Error("Invalid Git tree entry order");
    }
    prevNameBytes = nameBytes;
    prevIsDir = isDir;
    entries.push({
      name,
      mode: isDir ? "040000" : mode,
      hash: hex(data, end + 1, 20),
    });
    cursor = end + 21;
  }
  return entries;
}

export function parseTag(data) {
  if (data.includes(0)) throw new Error("Invalid Git tag");
  const text = new TextDecoder("utf-8", { fatal: false }).decode(data);
  const separator = text.indexOf("\n\n");
  const headers = separator < 0 ? text : text.slice(0, separator);
  const lines = headers.split("\n");
  const objMatch = /^object ([0-9a-f]{40})$/.exec(lines[0] ?? "");
  const typeMatch = /^type (commit|tree|blob|tag)$/.exec(lines[1] ?? "");
  const tagMatch = /^tag ([^\n]+)$/.exec(lines[2] ?? "");
  if (!objMatch || !typeMatch || !tagMatch) throw new Error("Invalid Git tag");
  let index = 3;
  if (index < lines.length && lines[index].startsWith("tagger ")) {
    const tagger = IDENTITY_LINE.exec(lines[index].slice("tagger ".length));
    if (!tagger || !Number.isSafeInteger(Number(tagger[3]))) throw new Error("Invalid Git tag");
    index += 1;
  }
  for (; index < lines.length; index++) {
    const line = lines[index];
    if (line.startsWith("object ") || line.startsWith("type ") || line.startsWith("tag ") || line.startsWith("tagger ")) {
      throw new Error("Invalid Git tag");
    }
  }
  return {
    target: objMatch[1],
    type: TYPE_NAMES.indexOf(typeMatch[1]),
    typeName: typeMatch[1],
    tag: tagMatch[1],
  };
}

export function parseTagTarget(data) {
  return parseTag(data).target;
}