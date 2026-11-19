import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { request as httpRequest, validateHeaderValue } from "node:http";
import { request as httpsRequest } from "node:https";
import { createInterface } from "node:readline";
import { pipeline } from "node:stream/promises";
import { setTimeout as sleep } from "node:timers/promises";

const MEDIA = "application/vnd.git-lfs+json";
const OID = /^[0-9a-f]{64}$/;
const MAX_PARTS = 10_000;
const MAX_PART_BYTES = 5 * 1024 ** 3;
const MAX_OBJECT_BYTES = 5 * 1024 ** 4 - MAX_PART_BYTES;
const MAX_RESPONSE_BYTES = 64 * 1024;
const CONTROL_TIMEOUT_MS = 25_000;
const CONTROL_RETRY_MS = 90_000;
const PART_TIMEOUT_MS = 55 * 60_000;
const PART_RETRY_MS = 60 * 60_000;
const PART_IDLE_MS = 60_000;
const FINISH_TIMEOUT_MS = 16 * 60_000;
const MAX_ATTEMPTS = 5;

class TransferError extends Error {
  constructor(message, { code = 1, retryable = false, retryAfter = 0 } = {}) {
    super(message);
    this.code = code;
    this.retryable = retryable;
    this.retryAfter = retryAfter;
  }
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function endpoint(href, label) {
  const url = typeof href === "string" ? URL.parse(href) : null;
  const loopback = url && /^(?:localhost|\[::1\]|127\.(?:\d{1,3}\.){2}\d{1,3})$/.test(url.hostname);
  if (!url || url.username || url.password || url.hash ||
      (url.protocol !== "https:" && !(url.protocol === "http:" && loopback))) {
    throw new TransferError(`${label} URL must use HTTPS or loopback HTTP`);
  }
  return url;
}

function shardAuthorization(headers) {
  const values = isRecord(headers)
    ? Object.entries(headers).filter(([name]) => name.toLowerCase() === "authorization")
    : [];
  const value = values[0]?.[1];
  if (values.length !== 1 || typeof value !== "string" || !/^(Basic|Bearer) [!-~]+$/i.test(value)) {
    throw new TransferError("Invalid Shard upload authorization");
  }
  return value;
}

function r2Headers(headers, url, length) {
  if (!isRecord(headers)) throw new TransferError("Invalid R2 signing headers");
  const result = { "content-length": String(length) };
  const seen = new Set();
  for (const [name, value] of Object.entries(headers)) {
    const key = name.toLowerCase();
    if (!/^(?:host|content-length|content-type|content-md5|x-amz-[a-z0-9-]+)$/.test(key)) continue;
    if (typeof value !== "string" || seen.has(key)) throw new TransferError("Invalid R2 signing headers");
    try {
      validateHeaderValue(key, value);
    } catch {
      throw new TransferError("Invalid R2 signing headers");
    }
    seen.add(key);
    if (key === "host" && value !== url.host || key === "content-length" && value !== String(length)) {
      throw new TransferError("R2 signing headers do not match the upload part");
    }
    if (key !== "host") result[key] = value;
  }
  return result;
}

function shardErrorDetail(body, authorization) {
  const message = parseJson(body)?.message;
  if (typeof message !== "string" || message.length > 240) return "";
  const detail = message.normalize("NFKC").replace(/\s+/g, " ").trim();
  // Only expose short plain-text diagnostics that contain no credentials or URLs.
  if (!/^[\p{L}\p{N} .,'()_:-]+$/u.test(detail) || detail.length > 240 ||
      /www\.|x-amz|authorization|\bbasic\b|\bbearer\b/i.test(detail)) return "";
  const [scheme, credential] = authorization.split(" ");
  const password = /^basic$/i.test(scheme)
    ? Buffer.from(credential, "base64").toString("utf8").split(/:(.*)/s)[1]
    : null;
  if (detail.includes(credential) || (password && detail.includes(password))) return "";
  return ` (${detail})`;
}

function httpFailure(label, status, retryAfter, detail = "") {
  const retryable = status === 408 || status === 425 || status === 429 || status >= 500;
  const wait = /^\d+$/.test(retryAfter)
    ? Number(retryAfter) * 1000
    : Date.parse(retryAfter) - Date.now();
  return new TransferError(`${label} returned HTTP ${status}${detail}`, {
    code: status,
    retryable,
    retryAfter: Math.max(0, wait || 0),
  });
}

async function retry(operation, budgetMs, parentSignal) {
  const started = performance.now();
  const deadline = AbortSignal.timeout(budgetMs);
  const signal = parentSignal ? AbortSignal.any([parentSignal, deadline]) : deadline;
  let failure;
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await operation(signal);
    } catch (error) {
      if (!(error instanceof TransferError) || !error.retryable) throw error;
      failure = error;
    }
    const wait = Math.max(failure.retryAfter, 250 * 2 ** (attempt - 1));
    if (signal.aborted || attempt >= MAX_ATTEMPTS || performance.now() - started + wait >= budgetMs) break;
    try {
      await sleep(wait, undefined, { signal });
    } catch {
      break;
    }
  }
  failure.retryable = false;
  throw failure;
}

async function exchange(url, { method, headers, send, label, timeout, idle = 0, parentSignal, authorization }) {
  const deadline = AbortSignal.timeout(timeout);
  const signal = parentSignal ? AbortSignal.any([parentSignal, deadline]) : deadline;
  const request = url.protocol === "https:" ? httpsRequest : httpRequest;
  let outgoing;
  let incoming;
  const receive = new Promise((resolve, reject) => {
    outgoing = request(url, { method, headers, signal }, resolve);
    outgoing.on("error", reject);
    outgoing.once("upgrade", (response, socket) => {
      socket.destroy();
      reject(httpFailure(label, response.statusCode));
    });
    outgoing.setTimeout(idle, () => outgoing.destroy(new Error("HTTP request idle timeout")));
  }).then(async (response) => {
    incoming = response;
    const status = response.statusCode;
    const retryAfter = response.headers["retry-after"] ?? "";
    const ok = status >= 200 && status < 300;
    if (!ok && !authorization) throw httpFailure(label, status, retryAfter);

    const chunks = [];
    let bytes = 0;
    for await (const chunk of response) {
      bytes += chunk.length;
      if (bytes > MAX_RESPONSE_BYTES) {
        response.destroy();
        if (!ok) throw httpFailure(label, status, retryAfter);
        throw new TransferError(`${label} returned an oversized response`);
      }
      if (authorization) chunks.push(chunk);
    }
    const body = authorization ? Buffer.concat(chunks, bytes).toString("utf8") : "";
    if (!ok) throw httpFailure(label, status, retryAfter, shardErrorDetail(body, authorization));
    return { status, body, etag: response.headers.etag };
  });
  const upload = outgoing ? Promise.resolve().then(() => send(outgoing)) : undefined;
  try {
    const [response] = await Promise.all([receive, upload]);
    return response;
  } catch (error) {
    incoming?.destroy();
    outgoing?.destroy();
    await upload?.catch(() => {});
    if (error instanceof TransferError) throw error;
    if (error?.path !== undefined) throw new TransferError("Cannot read the local Git LFS object");
    throw new TransferError(`${label} request failed or timed out`, { retryable: true });
  }
}

async function postShard(url, authorization, payload, parentSignal) {
  const body = JSON.stringify(payload);
  const finish = payload.action === "finish";
  const label = `Shard multipart ${payload.action}`;
  const { status, body: reply } = await exchange(url, {
    method: "POST",
    headers: { authorization, "content-type": MEDIA, "content-length": Buffer.byteLength(body) },
    send: (request) => { request.end(body); },
    label,
    timeout: finish ? FINISH_TIMEOUT_MS : CONTROL_TIMEOUT_MS,
    parentSignal,
    authorization,
  });
  if (payload.action === "record") return;
  const result = parseJson(reply);
  if (finish && (status === 204 || status === 200 && (!reply.trim() || result?.complete === true))) return;
  if (!finish && isRecord(result)) return result;
  throw new TransferError(`${label} returned an invalid response`);
}

async function putPart(url, headers, path, start, length, progress, parentSignal) {
  const label = "R2 multipart PUT";
  const { etag } = await exchange(url, {
    method: "PUT",
    headers: r2Headers(headers, url, length),
    send: (request) => pipeline(
      createReadStream(path, { start, end: start + length - 1, highWaterMark: 256 * 1024 }),
      async function* (chunks) {
        let bytes = 0;
        for await (const chunk of chunks) {
          yield chunk;
          bytes += chunk.length;
          await progress(bytes);
        }
        if (bytes !== length) throw new TransferError("Local Git LFS object ended before the part was complete");
      },
      request,
    ),
    label,
    timeout: PART_TIMEOUT_MS,
    idle: PART_IDLE_MS,
    parentSignal,
  });
  if (typeof etag !== "string" || !/^[!-~]{1,256}$/.test(etag)) {
    throw new TransferError("R2 did not return a valid upload part ETag");
  }
  return etag;
}

function writeMessage(message) {
  return new Promise((resolve, reject) => {
    process.stdout.write(`${JSON.stringify(message)}\n`, (error) => error ? reject(error) : resolve());
  });
}

async function uploadObject({ oid, size, path, action }) {
  if (typeof oid !== "string" || !OID.test(oid) || !Number.isSafeInteger(size) || size < 0 ||
      size > MAX_OBJECT_BYTES || typeof path !== "string" || !path || !isRecord(action)) {
    throw new TransferError("Invalid Git LFS upload request");
  }
  const url = endpoint(action.href, "Shard upload");
  const authorization = shardAuthorization(action.header);
  let reported = 0;
  let lastProgressAt;
  async function progress(bytes, force = false) {
    const now = performance.now();
    if (lastProgressAt !== undefined && (bytes <= reported || (!force && now - lastProgressAt < 250))) return;
    await writeMessage({ event: "progress", oid, bytesSoFar: bytes, bytesSinceLast: bytes - reported });
    reported = bytes;
    lastProgressAt = now;
  }

  const plan = await retry((signal) => postShard(url, authorization, { action: "start" }, signal), CONTROL_RETRY_MS);
  if (plan.complete === true) {
    await progress(size, true);
    return;
  }
  const { uploadId, partBytes, uploaded, staged } = plan;
  if (typeof uploadId !== "string" || !uploadId || uploadId.length > 4096 || typeof staged !== "boolean") {
    throw new TransferError("Shard returned an invalid multipart upload session");
  }
  if (staged) {
    await progress(size, true);
  } else {
    const partCount = Number.isSafeInteger(partBytes) && partBytes > 0 && partBytes <= MAX_PART_BYTES
      ? Math.ceil(size / partBytes)
      : 0;
    if (partCount < 1 || partCount > MAX_PARTS || !Array.isArray(uploaded) || uploaded.length > partCount ||
        uploaded.some((part) => !Number.isSafeInteger(part) || part < 1 || part > partCount)) {
      throw new TransferError("Shard returned an invalid multipart upload plan");
    }
    const partsDone = new Set(uploaded);
    const partLength = (part) => Math.min(partBytes, size - (part - 1) * partBytes);
    let transferred = 0;
    for (const part of partsDone) transferred += partLength(part);
    await progress(transferred, true);

    if (partsDone.size !== partCount) {
      const file = await stat(path).catch(() => null);
      if (!file?.isFile() || file.size !== size) throw new TransferError("Local Git LFS object does not match the upload size");
    }
    for (let partNumber = 1; partNumber <= partCount; partNumber += 1) {
      if (partsDone.has(partNumber)) continue;
      const payload = { action: "part", uploadId, partNumber };
      const length = partLength(partNumber);
      const start = (partNumber - 1) * partBytes;
      const etag = await retry(async (partSignal) => {
        // Refresh the signing URL on each PUT attempt. Re-uploading a part replaces it.
        const action = await retry((signal) => postShard(url, authorization, payload, signal), CONTROL_RETRY_MS, partSignal);
        const target = endpoint(action.href, "R2 part");
        return putPart(target, action.header, path, start, length, (bytes) => progress(transferred + bytes), partSignal);
      }, PART_RETRY_MS);
      const receipt = { action: "record", uploadId, partNumber, etag };
      await retry((signal) => postShard(url, authorization, receipt, signal), CONTROL_RETRY_MS);
      transferred += length;
      await progress(transferred, true);
    }
  }
  await postShard(url, authorization, { action: "finish", uploadId });
}

function protocolError(error) {
  return error instanceof TransferError
    ? { code: error.code, message: error.message }
    : { code: 1, message: "Git LFS multipart transfer failed" };
}

async function run() {
  const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
  let initialized = false;
  try {
    for await (const line of input) {
      const message = parseJson(line);
      if (message?.event === "terminate") return;
      if (!initialized) {
        if (message?.event !== "init" || message.operation !== "upload") {
          await writeMessage({ error: { code: 1, message: "Shard multipart requires Git LFS upload initialization" } });
          return;
        }
        await writeMessage({});
        initialized = true;
        continue;
      }
      if (message?.event !== "upload" && message?.event !== "download") {
        throw new Error("Invalid Git LFS protocol message");
      }
      const complete = { event: "complete", oid: typeof message.oid === "string" && OID.test(message.oid) ? message.oid : "" };
      try {
        if (message.event === "download") throw new TransferError("Shard multipart only supports uploads");
        await uploadObject(message);
      } catch (error) {
        complete.error = protocolError(error);
      }
      await writeMessage(complete);
    }
  } finally {
    input.close();
  }
}

process.stdout.on("error", () => { process.exitCode = 1; });
try {
  await run();
} catch {
  process.exitCode = 1;
  process.stderr.write("Shard multipart adapter or Git LFS protocol failed\n");
}
