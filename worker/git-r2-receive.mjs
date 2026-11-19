// Direct receive-pack support for small, non-thin pushes. Large or thin packs
// fall back to the native Git container.
import { Inflate, deflate } from "pako";
import { parsePackEntry } from "./git-r2-pack.mjs";

const TYPES = [null, "commit", "tree", "blob", "tag"];
const OID = /^[0-9a-f]{40}$/;
const MAX_OBJECT_BYTES = 64 * 1024 * 1024;

export class ThinPackError extends Error {}
export class PackTooLargeError extends Error {}

export function parseReceiveBytes(bytes) {
  const decoder = new TextDecoder();
  const updates = [];
  const seen = new Set();
  let sideband = false;
  let report = false;
  let offset = 0;
  for (;;) {
    if (offset + 4 > bytes.length) throw new Error("Missing Git receive commands");
    const tag = decoder.decode(bytes.subarray(offset, offset + 4));
    if (!/^[0-9a-fA-F]{4}$/.test(tag)) throw new Error("Invalid Git packet header");
    const length = Number.parseInt(tag, 16);
    offset += 4;
    if (length === 0) break;
    if (length < 4 || offset + length - 4 > bytes.length) throw new Error("Invalid Git packet length");
    const line = decoder.decode(bytes.subarray(offset, offset + length - 4)).replace(/\n$/, "");
    offset += length - 4;
    if (/^shallow [a-f0-9]{40}$/.test(line) && updates.length === 0) continue;
    const separator = line.indexOf("\0");
    const command = separator === -1 ? line : line.slice(0, separator);
    if (separator !== -1) {
      if (updates.length !== 0) throw new Error("Unexpected Git capabilities");
      const capabilities = line.slice(separator + 1).split(" ");
      sideband = capabilities.includes("side-band-64k");
      report = capabilities.includes("report-status") || capabilities.includes("report-status-v2");
    }
    const match = /^([a-f0-9]{40}) ([a-f0-9]{40}) (refs\/[^\x00-\x20\x7f]+)$/.exec(command);
    if (!match || seen.has(match[3])) throw new Error("Invalid Git receive command");
    seen.add(match[3]);
    updates.push({ name: match[3], before: match[1], after: match[2] });
  }
  return { updates, sideband, report, pack: bytes.subarray(offset) };
}

function inflateEntry(pack, start) {
  const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
  const chunks = [];
  let total = 0;
  inflater.onData = (chunk) => {
    total += chunk.length;
    if (total > MAX_OBJECT_BYTES) throw new PackTooLargeError("Git object exceeds the direct-push limit");
    chunks.push(chunk.slice());
  };
  let position = start;
  let stall = 0;
  while (!inflater.ended) {
    if (position >= pack.length - 20) throw new Error("Truncated Git pack entry");
    const input = pack.subarray(position, Math.min(position + 64 * 1024, pack.length - 20));
    const before = input.length;
    inflater.push(input, false);
    const consumed = before - inflater.strm.avail_in;
    position += consumed;
    if (inflater.ended) break;
    if (!consumed) {
      if (++stall > 2) throw new Error("Git pack inflation stalled");
    } else {
      stall = 0;
    }
  }
  const data = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.length;
  }
  return { data, end: position };
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

function applyDelta(base, delta) {
  const state = { cursor: 0 };
  const baseSize = deltaVarint(delta, state);
  const resultSize = deltaVarint(delta, state);
  if (baseSize !== base.length || resultSize > MAX_OBJECT_BYTES) throw new PackTooLargeError("Invalid Git delta size");
  const result = new Uint8Array(resultSize);
  let written = 0;
  while (state.cursor < delta.length) {
    const command = delta[state.cursor++];
    if (command & 0x80) {
      let offset = 0;
      let size = 0;
      if (command & 0x01) offset += delta[state.cursor++];
      if (command & 0x02) offset += delta[state.cursor++] << 8;
      if (command & 0x04) offset += delta[state.cursor++] << 16;
      if (command & 0x08) offset += delta[state.cursor++] * 2 ** 24;
      if (command & 0x10) size += delta[state.cursor++];
      if (command & 0x20) size += delta[state.cursor++] << 8;
      if (command & 0x40) size += delta[state.cursor++] << 16;
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

async function objectOid(type, data) {
  const header = new TextEncoder().encode(`${type} ${data.length}\0`);
  const object = new Uint8Array(header.length + data.length);
  object.set(header);
  object.set(data, header.length);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-1", object));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function sha1(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-1", bytes));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index++) {
    let value = index;
    for (let bit = 0; bit < 8; bit++) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    table[index] = value >>> 0;
  }
  return table;
})();

function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) value = CRC_TABLE[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
}

function hexBytes(hex) {
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index++) bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  return bytes;
}

function indexBytes(entries) {
  const sorted = [...entries].sort((left, right) => left.oid < right.oid ? -1 : left.oid > right.oid ? 1 : 0);
  const fanout = new Uint32Array(256);
  for (const entry of sorted) fanout[Number.parseInt(entry.oid.slice(0, 2), 16)]++;
  for (let index = 1; index < 256; index++) fanout[index] += fanout[index - 1];
  const large = sorted.filter((entry) => entry.offset > 0x7fffffff).length;
  const size = 8 + 1024 + sorted.length * 20 + sorted.length * 4 + sorted.length * 4 + large * 8 + 20;
  const bytes = new Uint8Array(size);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, 0xff744f63);
  view.setUint32(4, 2);
  for (let index = 0; index < 256; index++) view.setUint32(8 + index * 4, fanout[index]);
  let offset = 8 + 1024;
  for (const entry of sorted) {
    bytes.set(entry.oidBytes, offset);
    offset += 20;
  }
  for (const entry of sorted) {
    view.setUint32(offset, entry.crc);
    offset += 4;
  }
  let largeOffset = offset + sorted.length * 4;
  sorted.forEach((entry, index) => {
    if (entry.offset > 0x7fffffff) {
      view.setUint32(offset + index * 4, 0x80000000 | (largeOffset - (offset + sorted.length * 4)) / 8);
      view.setBigUint64(largeOffset, BigInt(entry.offset));
      largeOffset += 8;
    } else {
      view.setUint32(offset + index * 4, entry.offset);
    }
  });
  return { bytes, preTrailer: bytes.subarray(0, size - 20) };
}

function concatChunks(chunks, total) {
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
}

function packHeader(type, size) {
  const header = [];
  let byte = (type << 4) | (size & 0x0f);
  size = Math.floor(size / 16);
  while (size) {
    header.push(byte | 0x80);
    byte = size & 0x7f;
    size = Math.floor(size / 128);
  }
  header.push(byte);
  return Uint8Array.from(header);
}

async function repack(entries) {
  const chunks = [];
  let total = 0;
  const emit = (chunk) => { chunks.push(chunk); total += chunk.length; };
  const header = new Uint8Array(12);
  header.set([80, 65, 67, 75]);
  new DataView(header.buffer).setUint32(4, 2);
  new DataView(header.buffer).setUint32(8, entries.length);
  emit(header);
  const spans = [];
  for (const entry of entries) {
    const start = total;
    const bytes = deflate(entry.data);
    emit(packHeader(entry.type, entry.data.length));
    emit(bytes);
    spans.push({ start, end: total });
  }
  const body = concatChunks(chunks, total);
  const trailer = hexBytes(await sha1(body));
  const bytes = new Uint8Array(body.length + 20);
  bytes.set(body);
  bytes.set(trailer, body.length);
  const rebuilt = entries.map((entry, index) => ({
    ...entry,
    offset: spans[index].start,
    crc: crc32(bytes.subarray(spans[index].start, spans[index].end)),
  }));
  return { bytes, entries: rebuilt, packHash: [...trailer].map((byte) => byte.toString(16).padStart(2, "0")).join("") };
}

export async function parseReceivedPack(pack, maxObjects = 20_000, readBase = null) {
  if (pack.length < 32 || pack.length > 32 * 1024 * 1024) throw new PackTooLargeError("Git pack exceeds the direct-push limit");
  const view = new DataView(pack.buffer, pack.byteOffset, pack.byteLength);
  if (String.fromCharCode(pack[0], pack[1], pack[2], pack[3]) !== "PACK" ||
      (view.getUint32(4) !== 2 && view.getUint32(4) !== 3)) {
    throw new Error("Invalid Git pack header");
  }
  const count = view.getUint32(8);
  if (count > maxObjects) throw new PackTooLargeError("Git pack has too many objects");
  const entries = [];
  const byOid = new Map();
  const byOffset = new Map();
  const pending = [];
  let thin = false;
  let offset = 12;
  for (let index = 0; index < count; index++) {
    if (offset + 1 >= pack.length - 20) throw new Error("Truncated Git pack");
    const parsed = parsePackEntry(pack.subarray(offset, pack.length - 20), offset);
    const inflated = inflateEntry(pack, parsed.start);
    const data = inflated.data;
    let type = parsed.type;
    let baseOid = parsed.baseOid ?? null;
    if (type === 6) {
      const base = byOffset.get(parsed.baseOffset);
      if (!base) throw new ThinPackError("Git pack references a missing offset base");
      type = base.type;
      baseOid = base.oid;
      const resolved = applyDelta(base.data, data);
      const oid = await objectOid(TYPES[type], resolved);
      const entry = { oid, type, data: resolved, offset, start: parsed.start, end: inflated.end,
        entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid) };
      byOid.set(oid, entry);
      byOffset.set(offset, entry);
      entries.push(entry);
    } else if (type === 7) {
      const base = byOid.get(baseOid);
      if (!base) {
        pending.push({ offset, parsed, data, inflated, baseOid });
      } else {
        const resolved = applyDelta(base.data, data);
        const oid = await objectOid(base.type, resolved);
        const entry = { oid, type: base.type, data: resolved, offset, start: parsed.start, end: inflated.end,
          entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid) };
        byOid.set(oid, entry);
        byOffset.set(offset, entry);
        entries.push(entry);
      }
    } else if (type >= 1 && type <= 4) {
      const oid = await objectOid(TYPES[type], data);
      const entry = { oid, type, data, offset, start: parsed.start, end: inflated.end,
        entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid) };
      byOid.set(oid, entry);
      byOffset.set(offset, entry);
      entries.push(entry);
    } else {
      throw new Error(`Invalid Git pack entry type ${type} at ${offset}`);
    }
    offset = inflated.end;
  }
  // Ref-delta bases can appear after their dependants in a legal pack.
  for (let pass = 0; pending.length && pass < pending.length + 1; pass++) {
    const next = [];
    for (const item of pending) {
      let base = byOid.get(item.baseOid);
      if (!base && typeof readBase === "function") {
        const external = await readBase(item.baseOid);
        if (external) {
          const type = typeof external.type === "number" ? external.type : TYPES.indexOf(external.type);
          if (type >= 1 && type <= 4) {
            base = { type, data: external.data };
            thin = true;
          }
        }
      }
      if (!base) {
        next.push(item);
        continue;
      }
      const resolved = applyDelta(base.data, item.data);
      const oid = await objectOid(base.type, resolved);
      const entry = { oid, type: base.type, data: resolved, offset: item.offset, start: item.parsed.start,
        end: item.inflated.end, entrySize: item.parsed.size, crc: crc32(pack.subarray(item.offset, item.inflated.end)),
        oidBytes: hexBytes(oid) };
      byOid.set(oid, entry);
      byOffset.set(item.offset, entry);
      entries.push(entry);
    }
    pending.length = 0;
    pending.push(...next);
  }
  if (pending.length) throw new ThinPackError("Git pack is thin");
  if (offset !== pack.length - 20) throw new Error("Unexpected bytes after Git pack entries");
  if (await sha1(pack.subarray(0, pack.length - 20)) !== [...pack.subarray(pack.length - 20)].map((byte) => byte.toString(16).padStart(2, "0")).join("")) {
    throw new Error("Git pack checksum mismatch");
  }
  let finalEntries = entries;
  let packBytes = pack;
  let packHash = [...pack.subarray(pack.length - 20)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  if (thin) {
    const rebuilt = await repack(entries);
    finalEntries = rebuilt.entries;
    packBytes = rebuilt.bytes;
    packHash = rebuilt.packHash;
  }
  const index = indexBytes(finalEntries);
  const indexDigest = await sha1(index.preTrailer);
  const idx = new Uint8Array(index.bytes.length);
  idx.set(index.bytes.subarray(0, index.bytes.length - 20));
  idx.set(hexBytes(indexDigest), idx.length - 20);
  return { entries: finalEntries, objects: byOid, packBytes, packHash, idx };
}