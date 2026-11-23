const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const flush = Symbol("flush");
const maxPrefix = 256 * 1024;

class Packets {
  constructor(stream, limit, replay = false) {
    this.reader = stream.getReader();
    this.remaining = limit;
    this.chunk = new Uint8Array();
    this.offset = 0;
    this.saved = replay ? [] : null;
  }

  async take(length, eof = false) {
    if (length > this.remaining) throw new Error("Git packet budget exceeded");
    let copied = 0;
    let result;
    while (copied < length) {
      if (this.offset === this.chunk.length) {
        const next = await this.reader.read();
        if (next.done) {
          if (eof && copied === 0) return null;
          throw new Error("Incomplete Git packet");
        }
        this.chunk = next.value;
        this.offset = 0;
        this.saved?.push(this.chunk);
        if (!this.chunk.length) continue;
      }
      const count = Math.min(length - copied, this.chunk.length - this.offset);
      const part = this.chunk.subarray(this.offset, this.offset + count);
      this.offset += count;
      if (count === length) {
        result = part;
      } else {
        result ??= new Uint8Array(length);
        result.set(part, copied);
      }
      copied += count;
    }
    this.remaining -= length;
    return result ?? new Uint8Array();
  }

  async next() {
    const tag = await this.take(4, true);
    if (tag === null) return null;
    const length = packetLength(tag);
    if (length === 0) return flush;
    return this.take(length - 4);
  }

  async atEnd() {
    if (this.offset !== this.chunk.length) return false;
    for (;;) {
      const { done, value } = await this.reader.read();
      if (done) return true;
      if (value.length) {
        this.saved?.push(value);
        return false;
      }
    }
  }

  replay(maxDurationMs = 0) {
    const { reader, saved } = this;
    const deadline = maxDurationMs > 0 ? Date.now() + maxDurationMs : 0;
    let index = 0;
    return new ReadableStream({
      async pull(controller) {
        if (index < saved.length) {
          controller.enqueue(saved[index]);
          saved[index++] = null;
          return;
        }
        let next;
        if (deadline > 0) {
          const remaining = deadline - Date.now();
          if (remaining <= 0) {
            void reader.cancel().catch(() => {});
            controller.error(new Error("Git push admission expired"));
            return;
          }
          let timer;
          try {
            next = await Promise.race([
              reader.read(),
              new Promise((_, reject) => {
                timer = setTimeout(() => reject(new Error("Git push admission expired")), remaining);
              }),
            ]);
          } catch (error) {
            void reader.cancel().catch(() => {});
            controller.error(error);
            return;
          } finally {
            clearTimeout(timer);
          }
        } else {
          next = await reader.read();
        }
        if (next.done) {
          reader.releaseLock();
          controller.close();
        } else {
          controller.enqueue(next.value);
        }
      },
      cancel(reason) { return reader.cancel(reason); },
    });
  }

  cancel() { void this.reader.cancel().catch(() => {}); }
}

function packetLength(bytes) {
  const text = decoder.decode(bytes);
  if (!/^[0-9a-fA-F]{4}$/.test(text)) throw new Error("Invalid Git packet header");
  const length = Number.parseInt(text, 16);
  if (length !== 0 && (length < 4 || length > 65_520)) throw new Error("Invalid Git packet length");
  return length;
}

function concat(parts) {
  const bytes = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}

function packet(text) {
  const data = encoder.encode(text);
  return concat([encoder.encode((data.length + 4).toString(16).padStart(4, "0")), data]);
}

export async function shard_receive_request(body, encoding, maxDurationMs = 0) {
  if (encoding === "gzip") body = body.pipeThrough(new DecompressionStream("gzip"));
  const input = new Packets(body, maxPrefix, true);
  const updates = [];
  const seen = new Set();
  let sideband = false;
  let report = false;
  try {
    for (;;) {
      const item = await input.next();
      if (item === flush) break;
      if (!item) throw new Error("Missing Git receive commands");
      const line = decoder.decode(item).replace(/\n$/, "");
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
      const [, before, after, name] = match;
      const short = name.startsWith("refs/heads/")
        ? name.slice("refs/heads/".length)
        : name.startsWith("refs/tags/")
          ? name.slice("refs/tags/".length)
          : null;
      if (short !== null && (!short || short.startsWith("-") || encoder.encode(short).length > 255)) {
        throw new Error("Invalid Git receive command");
      }
      seen.add(name);
      updates.push({ name, before, after });
    }
    return { body: input.replay(maxDurationMs), updates: JSON.stringify(updates), sideband, report };
  } catch (error) {
    input.cancel();
    throw error;
  }
}

function* packets(bytes) {
  let offset = 0;
  while (offset < bytes.length) {
    if (bytes.length - offset < 4) throw new Error("Incomplete Git packet header");
    const length = packetLength(bytes.subarray(offset, offset + 4));
    if (length === 0) {
      yield flush;
      offset += 4;
      continue;
    }
    const end = offset + length;
    if (end > bytes.length) throw new Error("Incomplete Git packet payload");
    yield bytes.subarray(offset + 4, end);
    offset = end;
  }
}

export async function shard_receive_report(body, sideband) {
  const input = new Packets(body, 1024 * 1024);
  try {
    const collected = [];
    let fatal = false;
    for (;;) {
      const item = await input.next();
      if (item === null) throw new Error("Incomplete Git receive report");
      if (item === flush) break;
      if (!sideband || item[0] === 1) collected.push(sideband ? item.subarray(1) : item);
      if (sideband && item[0] === 3) fatal = true;
    }
    // A packet flush ends the Git report. HTTP EOF lets the origin finish its write.
    if (!await input.atEnd() || fatal) throw new Error("Unconfirmed Git receive result");
    const status = sideband ? packets(concat(collected)) : collected;
    let unpacked = false;
    const accepted = [];
    for (const line of status) {
      if (line === flush) {
        if (!unpacked) throw new Error("Missing Git unpack status");
        return JSON.stringify(accepted);
      }
      const text = decoder.decode(line).replace(/\n$/, "");
      if (!unpacked) {
        if (!text.startsWith("unpack ")) throw new Error("Missing Git unpack status");
        if (text !== "unpack ok") return "[]";
        unpacked = true;
      } else if (text.startsWith("ok refs/")) {
        accepted.push(text.slice(3));
      }
    }
    if (!unpacked || sideband) throw new Error("Incomplete Git receive report");
    return JSON.stringify(accepted);
  } finally {
    input.cancel();
  }
}

export async function shard_default_head(body, branch) {
  const input = new Packets(body, maxPrefix, true);
  try {
    const service = await input.next();
    if (!service || service === flush || decoder.decode(service) !== "# service=git-upload-pack\n" ||
        await input.next() !== flush) return input.replay();
    const lines = [];
    for (;;) {
      const item = await input.next();
      if (item === flush) break;
      if (item === null) return input.replay();
      lines.push(decoder.decode(item));
    }
    const reference = `refs/heads/${branch}`;
    const record = lines.find(line => line.slice(41).split(/[\0\n]/, 1)[0] === reference);
    if (!record) return input.replay();
    const first = lines[0].split("\0", 2);
    const capabilities = (first[1] ?? "").trim().split(/ +/).filter(value => value && !value.startsWith("symref=HEAD:"));
    capabilities.push(`symref=HEAD:${reference}`);
    lines[0] = first[0];
    const entries = lines.filter(line => !/^([a-f0-9]{40}) HEAD\n?$/.test(line));
    const bytes = concat([
      packet("# service=git-upload-pack\n"), encoder.encode("0000"),
      packet(`${record.slice(0, 40)} HEAD\0${capabilities.join(" ")}\n`),
      ...entries.map(line => packet(`${line.trimEnd()}\n`)), encoder.encode("0000"),
    ]);
    if (!await input.atEnd()) return input.replay();
    const rewritten = new Response(bytes).body;
    input.reader.releaseLock();
    return rewritten;
  } catch {
    return input.replay();
  }
}

export async function shard_update_request(reference, before, after) {
  const prefix = [packet(`${before} ${after} ${reference}\0report-status\n`), encoder.encode("0000")];
  if (!/^0{40}$/.test(after)) {
    const pack = Uint8Array.of(80, 65, 67, 75, 0, 0, 0, 2, 0, 0, 0, 0);
    prefix.push(pack, new Uint8Array(await crypto.subtle.digest("SHA-1", pack)));
  }
  return concat(prefix);
}
