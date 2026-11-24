// Direct receive-pack support for small, non-thin pushes. Large or thin packs
// fall back to the native Git container.
import { Inflate, deflate } from "pako";
import { applyDelta as applyGitDelta, parsePackEntry } from "./git-r2-pack.mjs";

const TYPES = [null, "commit", "tree", "blob", "tag"];
const MAX_OBJECT_BYTES = 32 * 1024 * 1024;
const MAX_TOTAL_INFLATED = 32 * 1024 * 1024;
const MAX_EXTERNAL_BASE_BYTES = 32 * 1024 * 1024;

export class ThinPackError extends Error {}
export class PackTooLargeError extends Error {}

export function parseReceiveBytes(bytes) {
  const decoder = new TextDecoder("utf-8", { fatal: true });
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
    const lowerName = match[3].toLowerCase();
    if (((lowerName === "refs/heads" || lowerName.startsWith("refs/heads/")) && !match[3].startsWith("refs/heads/")) ||
        ((lowerName === "refs/tags" || lowerName.startsWith("refs/tags/")) && !match[3].startsWith("refs/tags/"))) {
      throw new Error("Invalid Git receive command");
    }
    const short = match[3].startsWith("refs/heads/")
      ? match[3].slice("refs/heads/".length)
      : match[3].startsWith("refs/tags/")
        ? match[3].slice("refs/tags/".length)
        : null;
    if (short !== null && (!short || short.startsWith("-") || new TextEncoder().encode(short).length > 255)) {
      throw new Error("Invalid Git receive command");
    }
    seen.add(match[3]);
    updates.push({ name: match[3], before: match[1], after: match[2] });
  }
  return { updates, sideband, report, pack: bytes.subarray(offset) };
}

function inflateEntry(pack, start, expectedSize, remainingBudget) {
  if (!Number.isSafeInteger(expectedSize) || expectedSize < 0 || expectedSize > MAX_OBJECT_BYTES || expectedSize > remainingBudget) {
    throw new PackTooLargeError("Git object exceeds the direct-push limit");
  }
  const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
  const chunks = [];
  let total = 0;
  inflater.onData = (chunk) => {
    total += chunk.length;
    if (total > expectedSize || total > remainingBudget) {
      throw new PackTooLargeError("Git object exceeds the direct-push limit");
    }
    chunks.push(chunk.slice());
  };
  let position = start;
  let stall = 0;
  while (!inflater.ended) {
    if (position >= pack.length - 20) throw new Error("Truncated Git pack entry");
    const input = pack.subarray(position, Math.min(position + 64 * 1024, pack.length - 20));
    const before = input.length;
    inflater.push(input, false);
    if (inflater.err) throw new Error(`Corrupt Git pack entry: ${inflater.msg || inflater.err}`);
    const consumed = before - inflater.strm.avail_in;
    position += consumed;
    if (inflater.ended) break;
    if (!consumed) {
      if (++stall > 2) throw new Error("Git pack inflation stalled");
    } else {
      stall = 0;
    }
  }
  if (inflater.err) throw new Error(`Corrupt Git pack entry: ${inflater.msg || inflater.err}`);
  if (total !== expectedSize) throw new Error("Git pack entry size mismatch");
  const data = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.length;
  }
  return { data, end: position };
}

function applyDelta(base, delta, maxBytes) {
  try {
    return applyGitDelta(base, delta, Math.min(MAX_OBJECT_BYTES, maxBytes));
  } catch (error) {
    if (error?.message === "Invalid Git delta sizes") {
      throw new PackTooLargeError("Git delta object exceeds the direct-push limit");
    }
    throw error;
  }
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

function indexBytes(entries, packHash) {
  const sorted = [...entries].sort((left, right) => left.oid < right.oid ? -1 : left.oid > right.oid ? 1 : 0);
  const fanout = new Uint32Array(256);
  for (const entry of sorted) fanout[Number.parseInt(entry.oid.slice(0, 2), 16)]++;
  for (let index = 1; index < 256; index++) fanout[index] += fanout[index - 1];
  const large = sorted.filter((entry) => entry.offset > 0x7fffffff).length;
  // Pack index v2 trailer is 40 bytes: 20-byte pack checksum + 20-byte index checksum.
  const size = 8 + 1024 + sorted.length * 20 + sorted.length * 4 + sorted.length * 4 + large * 8 + 40;
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
  bytes.set(hexBytes(packHash), size - 40);
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

export async function parseReceivedPack(pack, maxObjects = 20_000, readBase = null, { hashObject = null } = {}) {
  const computeOid = typeof hashObject === "function" ? hashObject : objectOid;
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
  const missingBases = new Set();
  const externalBases = new Map();
  let externalBaseBudget = MAX_EXTERNAL_BASE_BYTES;
  let thin = false;
  let remainingBudget = MAX_TOTAL_INFLATED;
  const consumeBudget = (size) => {
    if (size > remainingBudget) throw new PackTooLargeError("Git pack inflated size exceeds the direct-push limit");
    remainingBudget -= size;
  };
  const registerEntry = (entry) => {
    if (byOid.has(entry.oid)) throw new Error("Duplicate Git object in pack");
    byOid.set(entry.oid, entry);
    byOffset.set(entry.offset, entry);
    entries.push(entry);
  };
  let offset = 12;
  for (let index = 0; index < count; index++) {
    if (offset + 1 >= pack.length - 20) throw new Error("Truncated Git pack");
    const parsed = parsePackEntry(pack.subarray(offset, pack.length - 20), offset);
    const inflated = inflateEntry(pack, parsed.start, parsed.size, remainingBudget);
    const data = inflated.data;
    consumeBudget(data.length);
    let type = parsed.type;
    let baseOid = parsed.baseOid ?? null;
    if (type === 6) {
      const base = byOffset.get(parsed.baseOffset);
      if (!base) {
        pending.push({ offset, parsed, data, inflated, baseOffset: parsed.baseOffset });
      } else {
        const depth = (base.depth ?? 0) + 1;
        if (depth > 64) throw new PackTooLargeError("Git delta chain is too deep");
        type = base.type;
        baseOid = base.oid;
        const resolved = applyDelta(base.data, data, remainingBudget);
        consumeBudget(resolved.length);
        const oid = await computeOid(TYPES[type], resolved);
        registerEntry({
          oid, type, data: resolved, depth, offset, start: parsed.start, end: inflated.end,
          entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid),
        });
      }
    } else if (type === 7) {
      const base = byOid.get(baseOid);
      if (!base) {
        pending.push({ offset, parsed, data, inflated, baseOid });
      } else {
        const depth = (base.depth ?? 0) + 1;
        if (depth > 64) throw new PackTooLargeError("Git delta chain is too deep");
        const resolved = applyDelta(base.data, data, remainingBudget);
        consumeBudget(resolved.length);
        const oid = await computeOid(TYPES[base.type], resolved);
        registerEntry({
          oid, type: base.type, data: resolved, depth, offset, start: parsed.start, end: inflated.end,
          entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid),
        });
      }
    } else if (type >= 1 && type <= 4) {
      const oid = await computeOid(TYPES[type], data);
      registerEntry({
        oid, type, data, depth: 0, offset, start: parsed.start, end: inflated.end,
        entrySize: parsed.size, crc: crc32(pack.subarray(offset, inflated.end)), oidBytes: hexBytes(oid),
      });
    } else {
      throw new Error(`Invalid Git pack entry type ${type} at ${offset}`);
    }
    offset = inflated.end;
  }
  // Ref-delta bases (and ofs-deltas chained onto them) can resolve across passes.
  for (let pass = 0; pending.length && pass < 64; pass++) {
    const next = [];
    for (const item of pending) {
      let base = item.baseOffset !== undefined
        ? byOffset.get(item.baseOffset)
        : byOid.get(item.baseOid);
      if (!base && item.baseOid && typeof readBase === "function" && !missingBases.has(item.baseOid)) {
        if (externalBases.has(item.baseOid)) {
          base = externalBases.get(item.baseOid);
          thin = true;
        } else {
          const external = await readBase(item.baseOid);
          if (external) {
            const type = typeof external.type === "number" ? external.type : TYPES.indexOf(external.type);
            if (type >= 1 && type <= 4) {
              const size = external.data?.length ?? 0;
              if (size > MAX_OBJECT_BYTES || size > externalBaseBudget) {
                throw new PackTooLargeError("Git external delta base exceeds the direct-push limit");
              }
              externalBaseBudget -= size;
              base = { type, data: external.data, depth: 0 };
              externalBases.set(item.baseOid, base);
              thin = true;
            }
          } else {
            missingBases.add(item.baseOid);
          }
        }
      }
      if (!base) {
        next.push(item);
        continue;
      }
      const depth = (base.depth ?? 0) + 1;
      if (depth > 64) throw new PackTooLargeError("Git delta chain is too deep");
      const resolved = applyDelta(base.data, item.data, remainingBudget);
      consumeBudget(resolved.length);
      const oid = await computeOid(TYPES[base.type], resolved);
      registerEntry({
        oid, type: base.type, data: resolved, depth, offset: item.offset, start: item.parsed.start,
        end: item.inflated.end, entrySize: item.parsed.size, crc: crc32(pack.subarray(item.offset, item.inflated.end)),
        oidBytes: hexBytes(oid),
      });
    }
    if (next.length === pending.length) break;
    pending.length = 0;
    pending.push(...next);
  }
  if (pending.length) throw new ThinPackError("Git pack is thin");
  if (offset !== pack.length - 20) throw new Error("Unexpected bytes after Git pack entries");
  const packHashHex = [...pack.subarray(pack.length - 20)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  if (await sha1(pack.subarray(0, pack.length - 20)) !== packHashHex) {
    throw new Error("Git pack checksum mismatch");
  }
  let finalEntries = entries;
  let packBytes = pack;
  let packHash = packHashHex;
  if (thin) {
    const rebuilt = await repack(entries);
    finalEntries = rebuilt.entries;
    packBytes = rebuilt.bytes;
    packHash = rebuilt.packHash;
  }
  const index = indexBytes(finalEntries, packHash);
  const indexDigest = await sha1(index.preTrailer);
  const idx = index.bytes;
  idx.set(hexBytes(indexDigest), idx.length - 20);
  return { entries: finalEntries, objects: byOid, packBytes, packHash, idx };
}
