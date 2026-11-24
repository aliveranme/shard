#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, open, readdir, rename, unlink } from "node:fs/promises";
import { createServer, request as httpRequest, validateHeaderValue } from "node:http";
import { request as httpsRequest } from "node:https";
import { join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { setTimeout as sleep } from "node:timers/promises";

const RECEIVE = "application/x-git-receive-pack";
const REPORT = `${RECEIVE}-result`;
const PROBE = Buffer.from("0000");
const SPOOL_ID = /^[0-9a-f]{64}-(?:[0-9a-f]{24}|[0-9a-f]{32})$/;
const MAX_BYTES = 5 * 1024 ** 4;
const MAX_PART_BYTES = 5 * 1024 ** 3;
const MAX_PARTS = 10_000;
const BUFFER_BYTES = 256 * 1024;
const CONTROL_BYTES = 128 * 1024;
const REPORT_BYTES = 8 * 1024 ** 2;
const CONTROL_TIMEOUT = 25_000;
const CONTROL_RETRY = 90_000;
const FINISH_TIMEOUT = 31 * 60_000;
const PART_TIMEOUT = 55 * 60_000;
const PART_RETRY = 60 * 60_000;
const CONNECTION_TIMEOUT = 15_000;
const SOCKET_IDLE = 60_000;

class PushError extends Error {
  constructor(message, { status, retry = false, delay = 0 } = {}) {
    super(message);
    this.status = status;
    this.retry = retry;
    this.delay = delay;
  }
}

function notice(message) {
  process.stderr.write(`Shard: ${message}\n`);
}

function safeUrl(value, label, { userinfo = false, loopback = true } = {}) {
  const url = typeof value === "string" && !/[\u0000-\u0020\u007f]/.test(value) ? URL.parse(value) : null;
  const local = url && /^(?:localhost|\[::1\]|127\.(?:\d{1,3}\.){2}\d{1,3})$/.test(url.hostname);
  if (!url || url.href.includes("#") || (!userinfo && (url.username || url.password)) ||
      (url.protocol !== "https:" && !(url.protocol === "http:" && loopback && local))) {
    throw new PushError(`${label} must use HTTPS or loopback HTTP and have no fragment`);
  }
  return url;
}

function repository(value) {
  const url = safeUrl(value?.replace(/^shard::/, ""), "Git remote URL", { userinfo: true });
  url.pathname = url.pathname.replace(/\/+$/, "");
  if (url.href.includes("?") || !url.pathname.endsWith(".git")) {
    throw new PushError("Use a Git remote URL ending in .git with no query string");
  }
  const credentialUrl = url.href;
  const configUrl = new URL(url);
  configUrl.password = "";
  url.username = "";
  url.password = "";
  return {
    url: url.href,
    secure: url.protocol === "https:",
    credentialUrl,
    configUrl: configUrl.href,
    key: createHash("sha256").update(url.href).digest("hex"),
  };
}

const gitEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !/^(?:GIT_TRACE(?:2)?(?:_.*)?|GIT_CURL_VERBOSE)$/i.test(key)));

async function gitOutput(args, input, signal) {
  signal.throwIfAborted();
  const child = spawn("git", args, { env: gitEnv, signal, stdio: ["pipe", "pipe", "ignore"] });
  child.stdin.on("error", () => {});
  child.stdin.end(input);
  return await new Promise((resolve, reject) => {
    let text = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      text += chunk;
      if (text.length > CONTROL_BYTES) {
        child.kill();
        reject(new PushError("Git returned an oversized configuration or credential response"));
      }
    });
    child.once("error", () => reject(new PushError("Could not run Git")));
    child.once("close", (code) => resolve({ code, text }));
  });
}

function authorization(value) {
  const match = /^(basic|bearer) ([!-~]+)$/i.exec(value ?? "");
  if (!match) throw new PushError("Shard requires a Git access token via Basic or Bearer authorization");
  return `${match[1].toLowerCase() === "basic" ? "Basic" : "Bearer"} ${match[2]}`;
}

function gitAuthorization(remote, signal) {
  let configured;
  let credential;
  let approved = false;
  const report = (action) => gitOutput(["credential", action], credential.input,
    AbortSignal.any([signal, AbortSignal.timeout(10_000)])).catch(() => {});

  return async function authenticated(send, { replay = true } = {}) {
    configured ??= gitOutput(["config", "--null", "--get-urlmatch", "http.extraHeader", remote.configUrl], undefined, signal)
      .then(({ code, text }) => {
        if (code !== 0 && code !== 1) throw new PushError("Could not read the Git HTTP configuration");
        const header = /^authorization:\s*([^\0]*)\0$/i.exec(text)?.[1];
        return header === undefined ? undefined : authorization(header.trim());
      });
    const explicit = await configured;
    let result = await send(explicit ?? credential?.header);
    if (result.unauthorized && !explicit && !credential && replay) {
      const challenges = result.unauthorized.map((value) => `wwwauth[]=${value}\n`).join("");
      const { code, text } = await gitOutput(["credential", "fill"],
        `capability[]=authtype\nurl=${remote.credentialUrl}\n${challenges}\n`, signal);
      if (code !== 0 || !text) throw new PushError("Configure a Git credential helper or provide a Shard access token when Git prompts", { status: 401 });
      const values = new Map(text.split(/\r?\n/).flatMap((line) => {
        const separator = line.indexOf("=");
        return separator > 0 ? [[line.slice(0, separator), line.slice(separator + 1)]] : [];
      }));
      let header;
      if (values.has("authtype") && values.has("credential")) {
        header = authorization(`${values.get("authtype")} ${values.get("credential")}`);
      } else if (values.has("username") && values.has("password")) {
        header = `Basic ${Buffer.from(`${values.get("username")}:${values.get("password")}`).toString("base64")}`;
      } else {
        throw new PushError("Git did not provide a Shard access token", { status: 401 });
      }
      credential = { header, input: `${text.replace(/(?:\r?\n)+$/, "")}\n\n` };
      result = await send(header);
    }
    if (result.unauthorized) {
      if (credential) {
        await report("reject");
        credential = undefined;
        approved = false;
      }
      throw new PushError("Shard refused Git authentication. Check that your token has write access to this repository", { status: 401 });
    }
    if (credential && !approved && result.status >= 200 && result.status < 300) {
      approved = true;
      await report("approve");
    }
    return result;
  };
}

function retryAfter(header) {
  if (typeof header !== "string") return 0;
  const delay = /^\d+$/.test(header) ? Number(header) * 1000 : Date.parse(header) - Date.now();
  return Math.max(0, delay || 0);
}

function httpFailure(response, label, retryConflict = false) {
  const status = response.statusCode;
  const hint = status === 409 ? ". The upload may be incomplete or its refs may conflict" : "";
  const delay = retryAfter(response.headers["retry-after"]);
  return new PushError(`${label} returned HTTP ${status}${hint}`, {
    status,
    retry: status === 408 || status === 425 || status === 429 || status >= 500 || retryConflict && status === 409 && delay > 0,
    delay,
  });
}

async function retry(operation, budget, parentSignal) {
  const signal = AbortSignal.any([parentSignal, AbortSignal.timeout(budget)]);
  const deadline = performance.now() + budget;
  for (let attempt = 0; ; attempt += 1) {
    signal.throwIfAborted();
    try {
      return await operation(signal, attempt);
    } catch (error) {
      parentSignal.throwIfAborted();
      if (!(error instanceof PushError) || !error.retry) throw error;
      const delay = Math.max(error.delay, Math.min(30_000, 500 * 2 ** attempt));
      if ((attempt >= 6 && error.status !== 425) || signal.aborted || performance.now() + delay >= deadline) throw error;
      await sleep(delay, undefined, { signal });
    }
  }
}

async function exchange(url, { method, headers, body, signal, timeout, idle = SOCKET_IDLE, label }, consume) {
  const active = AbortSignal.any([signal, AbortSignal.timeout(timeout)]);
  const request = url.protocol === "https:" ? httpsRequest : httpRequest;
  let outgoing;
  let incoming;
  let connectTimer;
  const receive = new Promise((resolve, reject) => {
    outgoing = request(url, { method, headers, signal: active }, resolve);
    outgoing.once("error", reject);
    outgoing.once("upgrade", (response, socket) => {
      socket.destroy();
      reject(httpFailure(response, label));
    });
    connectTimer = setTimeout(() => outgoing.destroy(), CONNECTION_TIMEOUT);
    connectTimer.unref();
    outgoing.once("socket", (socket) => {
      if (outgoing.reusedSocket || (url.protocol === "http:" && !socket.connecting)) clearTimeout(connectTimer);
      else socket.once(url.protocol === "https:" ? "secureConnect" : "connect", () => clearTimeout(connectTimer));
    });
    if (idle) outgoing.setTimeout(idle, () => outgoing.destroy());
  }).then((response) => {
    incoming = response;
    return consume(response);
  });
  const send = outgoing ? Promise.resolve().then(() => {
    if (body && !Buffer.isBuffer(body)) return pipeline(body, outgoing, { signal: active });
    outgoing.end(body);
  }) : Promise.resolve();
  try {
    const [recv, sent] = await Promise.allSettled([receive, send]);
    if (recv.status === "rejected") throw recv.reason;
    if (sent.status === "rejected") throw sent.reason;
    return recv.value;
  } catch (error) {
    incoming?.destroy();
    outgoing?.destroy();
    await send?.catch(() => {});
    signal.throwIfAborted();
    if (error instanceof PushError) throw error;
    throw new PushError(`${label} connection failed or timed out`, { retry: true });
  } finally {
    clearTimeout(connectTimer);
  }
}

async function responseBytes(response, limit) {
  const chunks = [];
  let size = 0;
  for await (const chunk of response) {
    size += chunk.length;
    if (size > limit) throw new PushError("Shard returned an oversized HTTP response");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

function unauthorized(response) {
  const challenges = response.headersDistinct["www-authenticate"] ?? [];
  response.destroy();
  return { unauthorized: challenges };
}

function mediaType(value) {
  return typeof value === "string" ? value.split(";", 1)[0].trim().toLowerCase() : "";
}

async function control(remote, authenticated, command, signal) {
  const { action } = command;
  const label = `Shard multipart ${action}`;
  const body = Buffer.from(JSON.stringify(command));
  const finish = action === "finish";
  const response = await authenticated((auth) => exchange(new URL(`${remote.url}/shard-upload`), {
    method: "POST",
    headers: {
      ...(auth ? { authorization: auth } : {}),
      "content-type": "application/json",
      "content-length": body.length,
      accept: finish ? REPORT : "application/json",
    },
    body, signal, timeout: finish ? FINISH_TIMEOUT : CONTROL_TIMEOUT, idle: finish ? 0 : SOCKET_IDLE, label,
  }, async (incoming) => {
    const status = incoming.statusCode;
    if (status === 401) return unauthorized(incoming);
    // Repeating an abort can see 404 after the first abort removed the session.
    if (status === 404 && action === "abort") {
      incoming.destroy();
      return { status };
    }
    if (status !== (action === "abort" ? 204 : 200)) throw httpFailure(incoming, label, finish);
    if (finish && (mediaType(incoming.headers["content-type"]) !== REPORT || incoming.headers["content-encoding"] &&
        incoming.headers["content-encoding"] !== "identity")) {
      throw new PushError("Shard returned an invalid Git receive-pack result");
    }
    return { status, body: await responseBytes(incoming, finish ? REPORT_BYTES : CONTROL_BYTES) };
  }));
  if (finish) {
    if (!response.body.length) throw new PushError("Shard returned an empty Git receive-pack result");
    return response.body;
  }
  if (action === "abort") return;
  try {
    return JSON.parse(response.body);
  } catch {
    throw new PushError(`${label} returned invalid JSON`);
  }
}

function planFor(plan, size) {
  const { id, partBytes, uploaded, staged, complete } = plan ?? {};
  if (typeof id !== "string" || !/^[!-~]{1,4096}$/.test(id) || typeof staged !== "boolean" || typeof complete !== "boolean") {
    throw new PushError("Shard returned an invalid multipart session");
  }
  if (staged || complete) return { id, ready: true, complete };
  const count = Number.isSafeInteger(partBytes) && partBytes > 0 && partBytes <= MAX_PART_BYTES
    ? Math.ceil(size / partBytes) : 0;
  if (!count || count > MAX_PARTS || !Array.isArray(uploaded) || uploaded.length > count ||
      uploaded.some((part) => !Number.isInteger(part) || part < 1 || part > count)) {
    throw new PushError("Shard returned an invalid multipart plan");
  }
  return { id, complete, partBytes, count, uploaded: new Set(uploaded) };
}

function partHeaders(headers, url, length) {
  if (!headers || typeof headers !== "object" || Array.isArray(headers)) throw new PushError("Shard returned invalid signing headers");
  const safe = { "content-length": String(length) };
  const seen = new Set();
  for (const [name, value] of Object.entries(headers)) {
    const key = name.toLowerCase();
    if (!/^(?:host|content-length|content-type|content-md5|x-amz-[a-z0-9-]+)$/.test(key) ||
        typeof value !== "string" || seen.has(key)) throw new PushError("Shard returned invalid signing headers");
    try {
      validateHeaderValue(key, value);
    } catch {
      throw new PushError("Shard returned invalid signing headers");
    }
    seen.add(key);
    if ((key === "host" && value !== url.host) || (key === "content-length" && value !== String(length))) {
      throw new PushError("Shard signing headers do not match the upload part");
    }
    if (key !== "host") safe[key] = value;
  }
  return safe;
}

async function* fileRange(file, start, length, signal, progress) {
  let offset = 0;
  while (offset < length) {
    signal.throwIfAborted();
    const buffer = Buffer.allocUnsafe(Math.min(BUFFER_BYTES, length - offset));
    const { bytesRead } = await file.read(buffer, 0, buffer.length, start + offset).catch(() => {
      throw new PushError("Could not read the saved push");
    });
    if (!bytesRead) throw new PushError("The saved push ended before its recorded size");
    yield buffer.subarray(0, bytesRead);
    offset += bytesRead;
    progress?.(offset);
  }
}

function startCommand(spool) {
  const [sha256, nonce] = spool.id.split("-");
  // Spools from older helpers used a 12-byte nonce only in the filename.
  return { action: "start", sha256, size: spool.size, ...(nonce.length === 32 && { nonce }) };
}

async function transfer(remote, authenticated, spool, signal) {
  const send = (command, budget, parent) => retry((active) => control(remote, authenticated, command, active), budget, parent);
  const start = async (active) => planFor(await send(startCommand(spool), CONTROL_RETRY, active), spool.size);
  const plan = await start(signal);
  if (!plan.ready) {
    const lengthOf = (part) => Math.min(plan.partBytes, spool.size - (part - 1) * plan.partBytes);
    let completed = 0;
    let lastProgress = -Infinity;
    let reported = -1;
    const progress = (bytes, force = false) => {
      const now = performance.now();
      if (!force && (bytes <= reported || now - lastProgress < 1000)) return;
      notice(`Uploading Git push ${Math.floor(bytes * 100 / spool.size)}% (${bytes} of ${spool.size} bytes)`);
      lastProgress = now;
      reported = bytes;
    };
    for (const part of plan.uploaded) completed += lengthOf(part);
    progress(completed, true);
    for (let part = 1; part <= plan.count; part += 1) {
      if (plan.uploaded.has(part)) continue;
      const length = lengthOf(part);
      const reconcilePart = async (active) => {
        const current = await start(active);
        if (current.id !== plan.id || (!current.ready && current.partBytes !== plan.partBytes)) {
          throw new PushError("The multipart session changed during upload");
        }
        if (current.ready) return false;
        if (current.uploaded.has(part)) return true;
        return undefined;
      };
      const stillSending = await retry(async (active, attempt) => {
        if (attempt) {
          // A lost PUT response may still have committed the part on R2.
          const state = await reconcilePart(active);
          if (state !== undefined) return state;
        }
        try {
          const signed = await send({ action: "part", id: plan.id, number: part }, CONTROL_RETRY, active);
          const target = safeUrl(signed?.href, "Signed R2 part URL", { loopback: !remote.secure });
          const headers = partHeaders(signed?.header, target, length);
          const offset = (part - 1) * plan.partBytes;
          const label = `R2 upload part ${part}`;
          await exchange(target, {
            method: "PUT", headers, body: fileRange(spool.file, offset, length, active, (bytes) => progress(completed + bytes)),
            signal: active, timeout: PART_TIMEOUT, label,
          }, async (response) => {
            if (response.statusCode < 200 || response.statusCode >= 300) throw httpFailure(response, label);
            await responseBytes(response, CONTROL_BYTES);
          });
          return true;
        } catch (error) {
          // Another process can complete this exact upload while we hold an older part list.
          if (error.status === 404 || error.status === 409) {
            const state = await reconcilePart(active);
            if (state !== undefined) return state;
          }
          throw error;
        }
      }, PART_RETRY, signal);
      if (!stillSending) break;
      completed += length;
      if (completed === spool.size) progress(completed, true);
    }
  }
  notice(plan.complete ? "Retrieving the saved Git push result" : "Waiting for Shard to finish the Git push");
  return send({ action: "finish", id: plan.id }, PART_RETRY, signal);
}

async function spoolDirectory(signal) {
  const { code, text } = await gitOutput(["rev-parse", "--git-common-dir"], undefined, signal);
  if (code !== 0 || !text.trim()) throw new PushError("Run this command from the local Git repository");
  return resolve(text.replace(/\r?\n$/, ""), "shard-uploads");
}

function spoolPath(directory, remote, id) {
  return join(directory, `${remote.key}-${id}.receive`);
}

async function savePush(directory, remote, prefix, iterator, signal) {
  const nonce = randomBytes(16).toString("hex");
  const temporary = join(directory, `.incoming-${nonce}`);
  const hash = createHash("sha256");
  let size = 0;
  let file;
  try {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    async function* requestBody() {
      yield* prefix;
      yield* { [Symbol.asyncIterator]: () => iterator };
    }
    await pipeline(async function* () {
      for await (const chunk of requestBody()) {
        size += chunk.length;
        if (size > MAX_BYTES) throw new PushError("The receive-pack body exceeds the R2 multipart object limit");
        hash.update(chunk);
        yield chunk;
      }
    }, createWriteStream(temporary, { flags: "wx", mode: 0o600, flush: true }), { signal });
    const id = `${hash.digest("hex")}-${nonce}`;
    const path = spoolPath(directory, remote, id);
    file = await open(temporary, "r");
    // Only complete, flushed requests are visible to --list or --resume.
    await rename(temporary, path);
    return { id, size, path, file };
  } catch (error) {
    await file?.close().catch(() => {});
    await unlink(temporary).catch(() => {});
    signal.throwIfAborted();
    if (error instanceof PushError) throw error;
    throw new PushError("Could not stage the Git push. Check free space and permissions in the Git common directory");
  }
}

async function openPush(directory, remote, id, signal, verify) {
  const path = spoolPath(directory, remote, id);
  const file = await open(path, "r").catch(() => {
    throw new PushError("No saved push matches this spool ID and remote in the local Git repository");
  });
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size <= PROBE.length || info.size > MAX_BYTES) throw new PushError("The saved push has an invalid size");
    if (verify) {
      const hash = createHash("sha256");
      for await (const chunk of fileRange(file, 0, info.size, signal)) hash.update(chunk);
      if (hash.digest("hex") !== id.slice(0, 64)) throw new PushError("The saved push no longer matches its SHA-256 digest");
    }
    return { id, size: info.size, path, file };
  } catch (error) {
    await file.close().catch(() => {});
    throw error;
  }
}

async function removePush(path) {
  try {
    await unlink(path);
  } catch (error) {
    if (error.code !== "ENOENT") throw new PushError("Could not remove the saved push from the Git common directory");
  }
}

function resumeHint(id) {
  notice(`Saved push ${id}. Resume with git remote-shard --resume ${id} <remote-url>`);
}

function* packets(bytes) {
  let offset = 0;
  while (offset < bytes.length) {
    const head = bytes.toString("latin1", offset, offset + 4);
    if (!/^[0-9a-f]{4}$/i.test(head)) throw new PushError("Shard returned an invalid Git status packet");
    const length = Number.parseInt(head, 16);
    if (length === 0 && offset + 4 === bytes.length) return;
    if (length <= 4 || offset + length > bytes.length) throw new PushError("Shard returned an invalid Git status packet length");
    yield bytes.subarray(offset + 4, offset + length);
    offset += length;
  }
  throw new PushError("Shard returned a Git status report without a final flush packet");
}

function printReport(bytes) {
  const outer = packets(bytes);
  const first = outer.next().value;
  if (!first) throw new PushError("Shard returned an empty Git status report");
  let lines;
  if (first[0] >= 1 && first[0] <= 3) {
    const chunks = [];
    function* sideband() { yield first; yield* outer; }
    for (const packet of sideband()) {
      if (packet[0] === 1) chunks.push(packet.subarray(1));
      else if (packet[0] === 3) throw new PushError("Git receive-pack reported a fatal remote error");
      else if (packet[0] !== 2) throw new PushError("Shard returned an invalid Git sideband packet");
    }
    lines = packets(Buffer.concat(chunks));
  } else {
    lines = (function* () { yield first; yield* outer; })();
  }
  let unpacked = false;
  let seenRef = false;
  let failed = false;
  for (const packet of lines) {
    const line = packet.toString("utf8").trimEnd().replace(/[\p{C}]/gu, " ");
    if (!unpacked && line.startsWith("unpack ")) {
      unpacked = true;
      failed = line !== "unpack ok";
      if (failed) notice(line);
    } else if (unpacked && /^(ok|ng) refs\//.test(line)) {
      seenRef = true;
      if (line.startsWith("ng ")) failed = true;
      notice(line);
    } else if (!seenRef || !line.startsWith("option ")) {
      throw new PushError("Shard returned an invalid Git receive-pack status");
    }
  }
  if (!unpacked || !seenRef && !failed) throw new PushError("Shard returned an incomplete Git receive-pack status");
  return failed ? 1 : 0;
}

async function manual(command, first, second, signal) {
  const listing = command === "--list";
  if (listing ? !first || second : !SPOOL_ID.test(first ?? "") || !second) {
    throw new PushError("Use git remote-shard --list <remote-url>, or --resume / --abort / --discard <spool-id> <remote-url>");
  }
  const remote = repository(listing ? first : second);
  const directory = await spoolDirectory(signal);
  if (listing) {
    const names = await readdir(directory).catch((error) => {
      if (error.code === "ENOENT") return [];
      throw new PushError("Could not list saved pushes in the Git common directory");
    });
    for (const name of names.sort()) {
      const id = name.slice(remote.key.length + 1, -".receive".length);
      if (name.startsWith(`${remote.key}-`) && name.endsWith(".receive") && SPOOL_ID.test(id)) process.stdout.write(`${id}\n`);
    }
    return 0;
  }
  if (command === "--discard") {
    await removePush(spoolPath(directory, remote, first));
    return 0;
  }
  const authenticated = gitAuthorization(remote, signal);
  const spool = await openPush(directory, remote, first, signal, command === "--resume");
  let result = 0;
  try {
    if (command === "--abort") {
      const lookup = { ...startCommand(spool), lookup: true };
      const session = await retry((active) => control(remote, authenticated, lookup, active), CONTROL_RETRY, signal);
      if (session?.complete === true) notice("The saved Git push already finished on Shard");
      else if (session !== null) await retry((active) => control(remote, authenticated,
        { action: "abort", id: session.id }, active), CONTROL_RETRY, signal);
    } else {
      result = printReport(await transfer(remote, authenticated, spool, signal));
    }
  } finally {
    await spool.file.close();
  }
  await removePush(spool.path);
  return result;
}

async function forward(remote, authenticated, suffix, request, reply, signal) {
  const headers = {};
  for (const key of ["content-type", "content-encoding", "content-length", "accept", "git-protocol", "user-agent", "accept-language"]) {
    if (request.headers[key] !== undefined) headers[key] = request.headers[key];
  }
  const post = request.method === "POST";
  const relay = (active) => authenticated((auth) => exchange(new URL(`${remote.url}${suffix}`), {
    method: request.method,
    headers: { ...headers, ...(auth ? { authorization: auth } : {}) },
    body: post ? request : undefined, signal: active, timeout: FINISH_TIMEOUT, label: "Git HTTPS remote",
  }, async (response) => {
    if (response.statusCode === 401) return unauthorized(response);
    if (response.statusCode >= 300 && response.statusCode < 400) {
      throw new PushError("The Git remote redirected the request. Configure its final repository URL", { status: 502 });
    }
    if (response.statusCode >= 400) throw httpFailure(response, "Git remote");
    const returned = {};
    for (const name of ["content-type", "content-encoding", "content-length", "cache-control"]) {
      if (response.headers[name] !== undefined) returned[name] = response.headers[name];
    }
    reply.writeHead(response.statusCode, returned);
    await pipeline(response, reply, { signal: active });
    return { status: response.statusCode };
  }), { replay: !post });
  if (post) return relay(signal);
  return retry(async (active) => {
    try {
      return await relay(active);
    } catch (error) {
      if (reply.headersSent && error instanceof PushError) error.retry = false;
      throw error;
    }
  }, CONTROL_RETRY, signal);
}

async function receive(remote, authenticated, request, reply, completed, signal) {
  if (mediaType(request.headers["content-type"]) !== `${RECEIVE}-request` ||
      request.headers["content-encoding"] && request.headers["content-encoding"] !== "identity") {
    throw new PushError("Expected an uncompressed Git receive-pack request", { status: 415 });
  }
  const iterator = request.iterator({ destroyOnReturn: false });
  const prefix = [];
  let bytes = 0;
  while (bytes <= PROBE.length) {
    const { done, value } = await iterator.next();
    if (done) {
      // remote-curl's POST probe for a streaming push carries no ref updates.
      if (bytes === 0 || bytes === PROBE.length && Buffer.concat(prefix).equals(PROBE)) {
        reply.writeHead(200, { "content-type": REPORT, "content-length": PROBE.length }).end(PROBE);
        return;
      }
      throw new PushError("Git sent a truncated receive-pack request", { status: 400 });
    }
    prefix.push(value);
    bytes += value.length;
  }
  let spool;
  try {
    const directory = await spoolDirectory(signal);
    spool = await savePush(directory, remote, prefix, iterator, signal);
    notice(`Staged Git push ${spool.id} (${spool.size} bytes)`);
    const body = await transfer(remote, authenticated, spool, signal);
    reply.writeHead(200, { "content-type": REPORT, "content-length": body.length });
    await pipeline([body], reply, { signal });
    completed.push(spool.path);
  } catch (error) {
    if (spool) resumeHint(spool.id);
    throw error;
  } finally {
    await iterator.return().catch(() => {});
    await spool?.file.close();
  }
}

async function remoteHelper(remote, controller) {
  const { signal } = controller;
  const authenticated = gitAuthorization(remote, signal);
  const prefix = `/${randomBytes(24).toString("hex")}`;
  const completed = [];
  const pending = new Set();
  let authority;
  const handle = async (request, reply) => {
    if (request.headers.host !== authority || !request.url?.startsWith(`${prefix}/`)) {
      reply.writeHead(404).end();
      return;
    }
    const path = request.url.slice(prefix.length);
    const client = new AbortController();
    const active = AbortSignal.any([signal, client.signal]);
    const disconnect = () => { if (!reply.writableFinished) client.abort(); };
    reply.once("close", disconnect);
    try {
      if (request.method === "GET" && /^\/info\/refs\?service=git-(?:upload|receive)-pack$/.test(path) ||
          request.method === "POST" && path === "/git-upload-pack") {
        await forward(remote, authenticated, path, request, reply, active);
      } else if (request.method === "POST" && path === "/git-receive-pack") {
        await receive(remote, authenticated, request, reply, completed, active);
      } else {
        reply.writeHead(404).end();
      }
    } catch (error) {
      if (!active.aborted) notice(error instanceof PushError ? error.message : "Git transport failed");
      if (!reply.headersSent && !reply.destroyed) {
        const status = error instanceof PushError && error.status >= 400 && error.status <= 599 ? error.status : 502;
        reply.writeHead(status === 401 ? 403 : status, { connection: "close" }).end();
        request.resume();
      } else if (!reply.writableFinished) {
        reply.destroy();
      }
    } finally {
      reply.off("close", disconnect);
    }
  };
  const server = createServer({ headersTimeout: 15_000, requestTimeout: 0, keepAliveTimeout: 1000 }, (request, reply) => {
    const task = handle(request, reply).catch(() => { reply.destroy(); });
    pending.add(task);
    void task.then(() => pending.delete(task));
  });
  server.maxConnections = 8;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  authority = `127.0.0.1:${server.address().port}`;
  const url = `http://${authority}${prefix}/`;
  const settings = {
    proxy: "", extraHeader: "", cookieFile: "", saveCookies: "false",
    followRedirects: "false", proactiveAuth: "none", version: "HTTP/1.1",
  };
  const baseCount = /^\d+$/.test(gitEnv.GIT_CONFIG_COUNT ?? "") ? Number.parseInt(gitEnv.GIT_CONFIG_COUNT, 10) : 0;
  const configPairs = [
    ["remote.shard-loopback.url", url],
    ["remote.shard-loopback.proxy", ""],
    ...Object.entries(settings).map(([key, value]) => [`http.${url}.${key}`, value]),
  ];
  const configEnv = { GIT_CONFIG_COUNT: String(baseCount + configPairs.length) };
  configPairs.forEach(([key, value], index) => {
    configEnv[`GIT_CONFIG_KEY_${baseCount + index}`] = key;
    configEnv[`GIT_CONFIG_VALUE_${baseCount + index}`] = value;
  });
  let exitCode = 1;
  try {
    signal.throwIfAborted();
    exitCode = await new Promise((resolve, reject) => {
      const child = spawn("git", ["remote-http", "shard-loopback"], {
        env: { ...gitEnv, ...configEnv, GIT_HTTP_LOW_SPEED_LIMIT: "0", NO_PROXY: "127.0.0.1", no_proxy: "127.0.0.1" },
        signal,
        stdio: "inherit",
      });
      child.once("error", () => reject(new PushError("Could not run Git's stock remote-http helper")));
      child.once("close", (code) => resolve(code ?? 1));
    });
  } finally {
    controller.abort();
    server.close();
    server.closeAllConnections();
    await Promise.allSettled(pending);
    for (const path of completed) await removePush(path).catch((error) => notice(error.message));
  }
  return exitCode;
}

const controller = new AbortController();
let interrupted = 0;
for (const [name, code] of [["SIGINT", 130], ["SIGTERM", 143], ["SIGHUP", 129]]) {
  process.once(name, () => {
    interrupted = code;
    controller.abort();
  });
}
try {
  const [first, second, third, ...rest] = process.argv.slice(2);
  if (!rest.length && ["--list", "--resume", "--abort", "--discard"].includes(first)) {
    process.exitCode = await manual(first, second, third, controller.signal);
  } else if (first && second && third === undefined) {
    process.exitCode = await remoteHelper(repository(second), controller);
  } else {
    throw new PushError("Use shard::<remote-url> as the Git push URL or run git remote-shard --list <remote-url>");
  }
  if (interrupted) process.exitCode = interrupted;
} catch (error) {
  if (!interrupted) notice(error instanceof PushError ? error.message : "Git multipart helper failed");
  process.exitCode = interrupted || 1;
}
