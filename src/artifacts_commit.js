// durable-git 0.0.8 uses this same locked inflater for individual pack entries.
// Its pack indexer requires a complete pack and a Durable Object SQL database.
import { Inflate } from "pako";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const maxWireBytes = 2 * 1024 * 1024;
const maxInflatedBytes = 4 * 1024 * 1024;
const maxPackEntries = 128;
const oidPattern = /^[0-9a-f]{40}$/;
const uploadResult = "application/x-git-upload-pack-result";

function packet(line) {
  return (line.length + 4).toString(16).padStart(4, "0") + line;
}

class Bytes {
  constructor(reader, limit = Infinity) {
    this.reader = reader;
    this.remaining = limit;
    this.chunk = new Uint8Array();
    this.offset = 0;
  }

  async fill() {
    while (this.offset === this.chunk.length) {
      const { done, value } = await this.reader();
      if (done || value.length > this.remaining) throw new Error("Git response missing or over budget");
      this.remaining -= value.length;
      this.chunk = value;
      this.offset = 0;
    }
  }

  async take(length) {
    let result;
    let copied = 0;
    while (copied < length) {
      await this.fill();
      const count = Math.min(length - copied, this.chunk.length - this.offset);
      const part = this.chunk.subarray(this.offset, this.offset + count);
      this.offset += count;
      if (count === length) return part;
      result ??= new Uint8Array(length);
      result.set(part, copied);
      copied += count;
    }
    return result;
  }

  async byte() {
    await this.fill();
    return this.chunk[this.offset++];
  }
}

async function* packChunks(wire) {
  let inPack = false;
  for (;;) {
    const tag = decoder.decode(await wire.take(4));
    if (!/^[0-9a-fA-F]{4}$/.test(tag)) throw new Error("Invalid Git packet length");
    if (tag === "0000" || tag === "0002") return;
    if (!inPack && tag === "0001") continue;
    const size = Number.parseInt(tag, 16);
    if (size < 5 || size > 65_520) throw new Error("Invalid Git packet size");
    const packet = await wire.take(size - 4);
    if (inPack) {
      if (packet[0] === 1) yield packet.subarray(1);
      else if (packet[0] !== 2) return;
    } else {
      // Git v2 can send shallow info and acknowledgments before the packfile.
      const line = decoder.decode(packet).replace(/\n$/, "");
      if (line === "packfile") inPack = true;
      else if (line !== "shallow-info" && line !== "acknowledgments" && line !== "NAK" && line !== "ready" &&
               !/^(?:shallow|unshallow|ACK) [0-9a-f]{40}$/.test(line)) return;
    }
  }
}

async function findCommit(pack, oid, maxCommitBytes) {
  const header = await pack.take(12);
  const view = new DataView(header.buffer, header.byteOffset, header.length);
  const version = view.getUint32(4);
  if (view.getUint32(0) !== 0x5041434b || (version !== 2 && version !== 3)) return null;
  const count = Math.min(view.getUint32(8), maxPackEntries);
  let remaining = maxInflatedBytes;

  for (let index = 0; index < count; index++) {
    let byte = await pack.byte();
    const type = (byte >> 4) & 7;
    let size = byte & 15;
    for (let shift = 4; byte & 0x80; shift += 7) {
      if (shift > 25) return null;
      byte = await pack.byte();
      size += (byte & 0x7f) * 2 ** shift;
    }
    if (size > remaining || (type === 1 && size > maxCommitBytes)) return null;
    remaining -= size;

    // We did not request thin packs or offset deltas. Other entries can be
    // skipped, but a delta-encoded commit cannot establish the requested OID.
    if (type === 7) await pack.take(20);
    else if (type < 1 || type > 4) return null;

    const prefix = type === 1 ? encoder.encode(`commit ${size}\0`) : null;
    const data = prefix && new Uint8Array(prefix.length + size);
    data?.set(prefix);
    let inflated = 0;
    const inflater = new Inflate({ chunkSize: 8192, windowBits: 15 });
    inflater.onData = (chunk) => {
      if (chunk.length > size - inflated) throw new Error("Git pack entry exceeded its declared size");
      data?.set(chunk, prefix.length + inflated);
      inflated += chunk.length;
    };
    while (!inflater.ended) {
      await pack.fill();
      const input = pack.chunk.subarray(pack.offset);
      if (!inflater.push(input, false)) return null;
      pack.offset += input.length - inflater.strm.avail_in;
    }
    if (inflated !== size) return null;
    if (data) {
      const hash = new Uint8Array(await crypto.subtle.digest("SHA-1", data));
      let actual = "";
      for (const byte of hash) actual += byte.toString(16).padStart(2, "0");
      // The exact object hash authenticates the payload independently of the
      // rest of the pack. Stop reading before unrelated trees and blobs.
      if (actual === oid) return data.subarray(prefix.length);
    }
  }
  return null;
}

export async function readArtifactsCommit(remote, token, oid, maxCommitBytes) {
  const url = new URL(remote);
  const local = url.protocol === "http:" && url.port && ["127.0.0.1", "[::1]"].includes(url.hostname);
  const hosted = url.protocol === "https:" && !url.port && /^[0-9a-f]{32}\.artifacts\.cloudflare\.net$/.test(url.hostname);
  if ((!local && !hosted) || remote !== `${url.origin}${url.pathname}` ||
      !/^\/git\/[a-zA-Z0-9._-]+\/[a-zA-Z0-9._-]+\.git$/.test(url.pathname) || !oidPattern.test(oid)) return null;

  const response = await fetch(`${remote}/git-upload-pack`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/x-git-upload-pack-request",
      "Git-Protocol": "version=2",
    },
    body: packet("command=fetch\n") + "0001" + packet("no-progress\n") +
      packet("deepen 1\n") + packet(`want ${oid}\n`) + packet("done\n") + "0000",
    redirect: "manual",
    signal: AbortSignal.timeout(5000),
  });
  if (!response.body) return null;
  const reader = response.body.getReader();
  try {
    if (response.status !== 200 || response.headers.get("Content-Type")?.split(";", 1)[0].trim() !== uploadResult) return null;
    const wire = new Bytes(() => reader.read(), maxWireBytes);
    const chunks = packChunks(wire);
    return await findCommit(new Bytes(() => chunks.next()), oid, maxCommitBytes);
  } catch {
    return null;
  } finally {
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
