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
    if (largeStart > bytes.length || bytes.length > limit) throw new Error("Invalid Git pack index");
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

async function inflateAt(readRange, start, size, maxBytes) {
  if (!Number.isSafeInteger(size) || size < 0 || size > maxBytes) throw new Error("Git object exceeds the read limit");
  const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
  const chunks = [];
  let total = 0;
  inflater.onData = (chunk) => {
    total += chunk.length;
    if (total > size || total > maxBytes) throw new Error("Git object exceeded its declared size");
    chunks.push(chunk.slice());
  };
  let position = start;
  let pending = new Uint8Array();
  let stall = 0;
  while (!inflater.ended) {
    if (!pending.length) {
      pending = await readRange(position, Math.min(128 * 1024, Math.max(1024, size + 256)));
      if (!pending.length) throw new Error("Incomplete Git pack object");
      position += pending.length;
    }
    const before = pending.length;
    inflater.push(pending, false);
    const consumed = before - inflater.strm.avail_in;
    pending = pending.subarray(consumed);
    if (inflater.ended) break;
    if (consumed === 0) {
      if (++stall > 2) throw new Error("Git pack inflation stalled");
    } else {
      stall = 0;
    }
  }
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

export function applyDelta(base, delta) {
  const state = { cursor: 0 };
  const baseSize = deltaVarint(delta, state);
  const resultSize = deltaVarint(delta, state);
  if (baseSize !== base.length || resultSize > 64 * 1024 * 1024) throw new Error("Invalid Git delta sizes");
  const result = new Uint8Array(resultSize);
  let written = 0;
  while (state.cursor < delta.length) {
    const command = delta[state.cursor++];
    if (command & 0x80) {
      let offset = 0;
      let size = 0;
      if (command & 0x01) offset |= delta[state.cursor++];
      if (command & 0x02) offset |= delta[state.cursor++] << 8;
      if (command & 0x04) offset |= delta[state.cursor++] << 16;
      if (command & 0x08) offset |= delta[state.cursor++] << 24;
      if (command & 0x10) size |= delta[state.cursor++];
      if (command & 0x20) size |= delta[state.cursor++] << 8;
      if (command & 0x40) size |= delta[state.cursor++] << 16;
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

export async function readPackedObject(index, oid, {
  readRange,
  readBase,
  cache = new Map(),
  maxBytes = 64 * 1024 * 1024,
  verify = true,
} = {}) {
  if (!OID.test(oid) || typeof readRange !== "function") return null;
  if (cache.has(`oid:${oid}`)) return cache.get(`oid:${oid}`);
  const readAt = async (offset, depth) => {
    if (depth > 64) throw new Error("Git delta chain is too deep");
    const cached = cache.get(`at:${offset}`);
    if (cached) return cached;
    const header = await readRange(offset, 128 * 1024);
    const parsed = parsePackEntry(header, offset);
    const delta = parsed.type === 6 || parsed.type === 7;
    const data = await inflateAt(readRange, parsed.start, parsed.size, maxBytes);
    let object;
    if (parsed.type >= 1 && parsed.type <= 4) {
      object = { type: parsed.type, data };
    } else if (parsed.type === 6) {
      const base = await readAt(parsed.baseOffset, depth + 1);
      object = { type: base.type, data: applyDelta(base.data, data) };
    } else if (parsed.type === 7) {
      const base = typeof readBase === "function" ? await readBase(parsed.baseOid, depth + 1) : null;
      if (!base) return null;
      object = { type: base.type, data: applyDelta(base.data, data) };
    } else {
      throw new Error("Unsupported Git pack entry type");
    }
    cache.set(`at:${offset}`, object);
    return object;
  };
  const offset = index.find(oid);
  if (offset === null) return null;
  const object = await readAt(offset, 0);
  if (verify) await verifyObject(oid, object.type, object.data);
  cache.set(`oid:${oid}`, object);
  return object;
}

export function parseCommit(oid, data) {
  const text = new TextDecoder("utf-8", { fatal: false }).decode(data);
  const separator = text.indexOf("\n\n");
  const headers = separator < 0 ? text : text.slice(0, separator);
  const message = separator < 0 ? "" : text.slice(separator + 2);
  const tree = /^tree ([0-9a-f]{40})$/m.exec(headers)?.[1];
  const parents = [...headers.matchAll(/^parent ([0-9a-f]{40})$/gm)].map((match) => match[1]);
  const author = /^author (.*) <([^<>]*)> (-?\d+) [+-]\d{4}$/m.exec(headers);
  if (!tree || !author) throw new Error("Invalid Git commit");
  const timestamp = Number(author[3]);
  if (!Number.isSafeInteger(timestamp)) throw new Error("Invalid Git commit timestamp");
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
  let cursor = 0;
  while (cursor < data.length) {
    const end = data.indexOf(0, cursor);
    if (end < 0 || end + 21 > data.length) throw new Error("Invalid Git tree");
    const header = new TextDecoder("utf-8", { fatal: false }).decode(data.subarray(cursor, end));
    const space = header.indexOf(" ");
    if (space < 1) throw new Error("Invalid Git tree entry");
    const mode = header.slice(0, space);
    const name = header.slice(space + 1);
    if (!name || name === "." || name === ".." || name.includes("/")) throw new Error("Invalid Git tree name");
    entries.push({
      name,
      mode: mode === "40000" ? "040000" : mode,
      hash: hex(data, end + 1, 20),
    });
    cursor = end + 21;
  }
  return entries;
}

export function parseTagTarget(data) {
  const text = new TextDecoder("utf-8", { fatal: false }).decode(data);
  const target = /^object ([0-9a-f]{40})$/m.exec(text)?.[1];
  if (!target) throw new Error("Invalid Git tag");
  return target;
}