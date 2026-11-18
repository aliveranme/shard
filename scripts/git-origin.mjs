import { spawn } from "node:child_process";
import { createHash, timingSafeEqual } from "node:crypto";
import { once } from "node:events";
import { appendFile, lstat, mkdir, mkdtemp, realpath, rename, rm } from "node:fs/promises";
import { createServer, STATUS_CODES } from "node:http";
import { isIP } from "node:net";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const WINDOWS = process.platform === "win32";
const ID = /^shard-[A-Za-z0-9._-]{1,245}$/;
const OID = /^[0-9a-f]{40}$/;
const DEFAULT_IMPORT_HOSTS = ["github.com", "gitlab.com", "codeberg.org"];
const MAX_BODY_BYTES = 8 * 1024;
const MAX_METADATA_BYTES = 4 * 1024 * 1024;
const MAX_CGI_HEADER_BYTES = 8 * 1024;
const MAX_GIT_DIAGNOSTIC_BYTES = 8 * 1024;
const MAX_QUERY_VALUE_BYTES = 4 * 1024;
const BRANCH_PREFIX = "refs/heads/";
const SERVICE = /^(git-upload-pack|git-receive-pack)$/;
const TREE_ENTRY = /^(040000|100644|100755|120000|160000) (?:tree|blob|commit) ([0-9a-f]{40}) +([0-9]+|-)\t/;

const REPO_CONFIG = `
[http]
	getanyfile = false
	receivepack = true
[receive]
	fsckObjects = true
	autoGC = false
[gc]
	auto = 0
	pruneExpire = never
`;

class HttpError extends Error {
  constructor(status) {
    super(STATUS_CODES[status]);
    this.status = status;
  }
}

class GitError extends Error {
  constructor(operation, status) {
    super("Git failed");
    this.operation = operation;
    this.status = status;
  }
}

function gitEnvironment(cwd) {
  // Do not inherit repository discovery, config, credential, proxy or trace settings.
  const env = {
    LC_ALL: "C",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: WINDOWS ? "NUL" : "/dev/null",
    GIT_CEILING_DIRECTORIES: dirname(cwd),
    GIT_TERMINAL_PROMPT: "0",
    GIT_NO_REPLACE_OBJECTS: "1",
    GIT_ALLOW_PROTOCOL: "",
  };
  for (const name of ["PATH", "SYSTEMROOT", "WINDIR", "SYSTEMDRIVE", "COMSPEC", "PATHEXT", "TMPDIR", "TEMP", "TMP"]) {
    if (process.env[name] !== undefined) env[name] = process.env[name];
  }
  return env;
}

function startGit(operation, args, { cwd, env, signal, pipeInput, missingMessage, command = "git" }) {
  signal?.throwIfAborted();
  const child = spawn(command, args, {
    cwd,
    env: { ...gitEnvironment(cwd), ...env },
    stdio: [pipeInput ? "pipe" : "ignore", "pipe", missingMessage ? "pipe" : "ignore"],
    windowsHide: true,
  });
  let launchFailed = false;
  child.on("error", () => { launchFailed = true; });
  // Git may reject a request while the client is still sending it.
  child.stdin?.on("error", () => {});
  let diagnostics = Buffer.alloc(0);
  child.stderr?.on("data", (chunk) => {
    const tail = chunk.subarray(-MAX_GIT_DIAGNOSTIC_BYTES);
    const keep = Math.min(diagnostics.length, MAX_GIT_DIAGNOSTIC_BYTES - tail.length);
    diagnostics = Buffer.concat([diagnostics.subarray(diagnostics.length - keep), tail]);
  });
  const closed = new Promise((resolve) => child.once("close", (code) => resolve(code)));
  function stop() {
    child.stdin?.destroy();
    child.stdout.destroy();
    child.stderr?.destroy();
    if (child.exitCode === null && child.signalCode === null) child.kill();
  }
  signal?.addEventListener("abort", stop, { once: true });
  void closed.then(() => signal?.removeEventListener("abort", stop));
  async function check(missing) {
    const code = await closed;
    signal?.throwIfAborted();
    if (!launchFailed && code === 0) return true;
    if (!launchFailed && code !== null && code === missing) return false;
    // Read only the stock Git missing-repository diagnostic. Never log child stderr.
    if (!launchFailed && code === 128 && missingMessage &&
        diagnostics.toString("utf8").split(/\r?\n/).includes(missingMessage)) return false;
    throw new GitError(operation, launchFailed ? "process error" : code ?? "terminated");
  }
  return { child, closed, check, stop };
}

function repoCommand(repo, command, args) {
  return ["--no-pager", `--git-dir=${repo}`, command, ...args];
}

async function gitOutput(command, args, options) {
  const { repo, input, missing, maxBytes = MAX_METADATA_BYTES } = options;
  const argv = repo ? repoCommand(repo, command, args) : ["--no-pager", command, ...args];
  const git = startGit(command, argv, { ...options, pipeInput: input !== undefined });
  if (input !== undefined) git.child.stdin.end(input);
  try {
    const chunks = [];
    let length = 0;
    for await (const chunk of git.child.stdout) {
      length += chunk.length;
      if (length > maxBytes) throw new HttpError(413);
      chunks.push(chunk);
    }
    if (!await git.check(missing)) return null;
    return chunks.length === 1 ? chunks[0] : Buffer.concat(chunks, length);
  } finally {
    git.stop();
    await git.closed;
  }
}

function sendJson(res, status, payload) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json; charset=utf-8");
  res.end(JSON.stringify(payload));
}

function sendStatus(res, status) {
  if (status === 202 || status === 204) {
    res.statusCode = status;
    res.end();
  } else {
    sendJson(res, status, { error: STATUS_CODES[status] });
  }
}

async function readBody(req) {
  if (Number(req.headers["content-length"]) > MAX_BODY_BYTES) throw new HttpError(413);
  const chunks = [];
  let length = 0;
  for await (const chunk of req.iterator({ destroyOnReturn: false })) {
    length += chunk.length;
    if (length > MAX_BODY_BYTES) throw new HttpError(413);
    chunks.push(chunk);
  }
  try {
    const body = JSON.parse(Buffer.concat(chunks, length).toString("utf8"));
    if (body && typeof body === "object" && !Array.isArray(body)) return body;
  } catch {
    throw new HttpError(422);
  }
  throw new HttpError(422);
}

function configuredImportHosts(hosts) {
  const configured = new Set();
  for (const name of hosts) {
    if (typeof name !== "string") throw new TypeError("Invalid Git import host");
    const host = name.trim().toLowerCase();
    const label = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
    if (!host.includes(".") || host.length > 253 || isIP(host) || !host.split(".").every((part) => label.test(part))) {
      throw new TypeError("Invalid Git import host");
    }
    configured.add(host);
  }
  return configured;
}

function importUrl(source, hosts) {
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
  if (!hosts.has(url.hostname) || (authority !== url.hostname && authority !== `${url.hostname}:443`)) {
    throw new HttpError(422);
  }
  return url.href;
}

function queryValue(query, key) {
  const value = query.get(key);
  if (!value || query.getAll(key).length !== 1 || value.includes("\0") || Buffer.byteLength(value) > MAX_QUERY_VALUE_BYTES) {
    throw new HttpError(422);
  }
  return value;
}

function queryLimit(query, { optional = false, maximum = Number.MAX_SAFE_INTEGER } = {}) {
  if (optional && !query.has("limit")) return null;
  const value = queryValue(query, "limit");
  const number = Number(value);
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(number) || number > maximum) throw new HttpError(422);
  return number;
}

function queryOid(query) {
  const value = queryValue(query, "oid");
  if (!OID.test(value)) throw new HttpError(422);
  return value;
}

async function statusOf(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

async function headRef(options) {
  const output = await gitOutput("symbolic-ref", ["--quiet", "HEAD"], { ...options, missing: 1 });
  return output?.toString("utf8").replace(/\r?\n$/, "") ?? null;
}

function metadata(name, head) {
  if (head === null) return { name, defaultBranch: "main" };
  if (!head.startsWith(BRANCH_PREFIX) || head.length === BRANCH_PREFIX.length) throw new Error("Invalid Git HEAD");
  return { name, defaultBranch: head.slice(BRANCH_PREFIX.length) };
}

async function resolveRevision(revision, options) {
  const result = await gitOutput("rev-parse", ["--verify", "--quiet", "--end-of-options", revision], {
    ...options, missing: 1,
  });
  if (!result) return null;
  const hash = result.toString("ascii").trimEnd();
  if (!OID.test(hash)) throw new Error("Invalid Git object ID");
  return hash;
}

async function exactObject(hash, type, options) {
  if (await resolveRevision(`${hash}^{${type}}`, options) !== hash) throw new HttpError(404);
  return hash;
}

async function readCommits(hash, limit, fullMessage, options) {
  const format = `%H%x00%T%x00%an%x00%at%x00%P%x00${fullMessage ? "%B" : "%s"}`;
  const output = await gitOutput("log", [
    "--first-parent", "--no-show-signature", "--no-notes", "--encoding=UTF-8", "-z",
    `--max-count=${limit}`, `--format=tformat:${format}`, hash, "--",
  ], options);
  const fields = output.toString("utf8").split("\0");
  if (fields.pop() !== "" || fields.length % 6 !== 0) throw new Error("Invalid Git commit output");
  const commits = [];
  for (let index = 0; index < fields.length; index += 6) {
    const [hash, tree, author, seconds, parentList, message] = fields.slice(index, index + 6);
    const parents = parentList ? parentList.split(" ") : [];
    const timestamp = Number(seconds);
    if (!OID.test(hash) || !OID.test(tree) || !parents.every((parent) => OID.test(parent)) ||
        !/^-?\d+$/.test(seconds) || !Number.isSafeInteger(timestamp)) {
      throw new Error("Invalid Git commit fields");
    }
    commits.push({ hash, tree, message, author, timestamp, parents });
  }
  if (commits[0]?.hash !== hash) throw new Error("Invalid Git commit lookup");
  return commits;
}

async function readTree(hash, limit, options, path) {
  const args = repoCommand(options.repo, "ls-tree", ["-z", "-l", "--full-tree", hash]);
  if (path !== undefined) {
    args.unshift("--literal-pathspecs");
    args.push("--", path);
  }
  const git = startGit("ls-tree", args, options);
  const entries = [];
  let remainder = Buffer.alloc(0);
  let length = 0;
  try {
    for await (const chunk of git.child.stdout) {
      length += chunk.length;
      if (length > MAX_METADATA_BYTES) throw new HttpError(413);
      const bytes = remainder.length ? Buffer.concat([remainder, chunk]) : chunk;
      let start = 0;
      for (let end; (end = bytes.indexOf(0, start)) !== -1; start = end + 1) {
        if (entries.length === limit) throw new HttpError(413);
        const line = bytes.toString("utf8", start, end);
        const fields = TREE_ENTRY.exec(line);
        if (!fields) throw new Error("Invalid Git tree output");
        const [, mode, hash, size] = fields;
        const name = line.slice(fields[0].length);
        if (!name || name === "." || name === ".." || (path === undefined && name.includes("/"))) {
          throw new Error("Invalid Git tree name");
        }
        const entry = { name, mode, hash };
        if (size !== "-") {
          entry.size = Number(size);
          if (!Number.isSafeInteger(entry.size)) throw new Error("Invalid Git tree entry size");
        }
        entries.push(entry);
      }
      remainder = bytes.subarray(start);
    }
    await git.check();
    if (remainder.length) throw new Error("Incomplete Git tree output");
    return entries;
  } finally {
    git.stop();
    await git.closed;
  }
}

async function blobSize(hash, options) {
  const output = await gitOutput("cat-file", ["--batch-check"], { ...options, input: `${hash}\n` });
  const [oid, type, size] = output.toString("ascii").trimEnd().split(" ");
  if (oid !== hash || !type) throw new Error("Invalid Git object output");
  if (type !== "blob") throw new HttpError(404);
  const sizeInBytes = Number(size);
  if (!/^\d+$/.test(size) || !Number.isSafeInteger(sizeInBytes)) throw new Error("Invalid Git blob size");
  return sizeInBytes;
}

async function writeChunk(res, chunk, signal) {
  signal.throwIfAborted();
  if (chunk.length && !res.write(chunk)) await once(res, "drain", { signal });
}

async function streamBlob(res, hash, size, limit, file, options) {
  let remaining = limit === null ? size : Math.min(size, limit);
  res.setHeader("Content-Type", "application/octet-stream");
  res.setHeader("Content-Length", remaining);
  if (file) res.setHeader("X-Shard-Object-Size", size);
  if (!remaining) {
    res.end();
    return;
  }
  const partial = remaining < size;
  const git = startGit("cat-file", repoCommand(options.repo, "cat-file", ["blob", hash]), options);
  try {
    for await (const chunk of git.child.stdout) {
      if (!partial && chunk.length > remaining) throw new Error("Invalid Git blob length");
      const length = Math.min(chunk.length, remaining);
      await writeChunk(res, chunk.subarray(0, length), options.signal);
      remaining -= length;
      if (partial && !remaining) break;
    }
    if (!partial || remaining) await git.check();
    if (remaining) throw new Error("Incomplete Git blob");
    res.end();
  } finally {
    git.stop();
    await git.closed;
  }
}

async function readObject(res, action, query, options) {
  if (action === "log") {
    const ref = queryValue(query, "ref");
    const limit = queryLimit(query, { maximum: 0x7fffffff });
    const hash = limit ? await resolveRevision(`${ref}^{commit}`, options) : null;
    sendJson(res, 200, hash ? await readCommits(hash, limit, false, options) : []);
  } else if (action === "commit") {
    const hash = await exactObject(queryOid(query), "commit", options);
    const [commit] = await readCommits(hash, 1, true, options);
    sendJson(res, 200, commit);
  } else if (action === "tree") {
    const limit = queryLimit(query);
    const hash = await exactObject(queryOid(query), "tree", options);
    sendJson(res, 200, await readTree(hash, limit, options));
  } else if (action === "blob") {
    const hash = queryOid(query);
    const limit = queryLimit(query);
    const size = await blobSize(hash, options);
    if (size > limit) throw new HttpError(413);
    await streamBlob(res, hash, size, null, false, options);
  } else {
    const ref = queryValue(query, "ref");
    const path = queryValue(query, "path");
    const limit = queryLimit(query, { optional: true });
    if (path.split("/").some((part) => !part || part === "." || part === "..")) throw new HttpError(422);
    const commit = await resolveRevision(`${ref}^{commit}`, options);
    if (!commit) throw new HttpError(404);
    const [entry] = await readTree(commit, 1, options, path);
    if (entry?.name !== path || entry.size === undefined) throw new HttpError(404);
    await streamBlob(res, entry.hash, entry.size, limit, true, options);
  }
}

async function readCgiHeaders(stdout) {
  let buffer = Buffer.alloc(0);
  for (;;) {
    const { value, done } = await stdout.next();
    if (done) return null;
    const previousLength = buffer.length;
    // Keep the first pack bytes in the original chunk, separate from the headers.
    buffer = Buffer.concat([buffer, value.subarray(0, MAX_CGI_HEADER_BYTES - previousLength)]);
    const boundary = buffer.indexOf("\r\n\r\n", Math.max(0, previousLength - 3));
    if (boundary !== -1) {
      return {
        headers: buffer.toString("latin1", 0, boundary),
        body: value.subarray(boundary + 4 - previousLength),
      };
    }
    if (buffer.length === MAX_CGI_HEADER_BYTES) throw new Error("Git CGI headers exceed the limit");
  }
}

function setCgiHeaders(res, headers) {
  let status = 200;
  const responseHeaders = new Map();
  for (const line of headers.split("\r\n")) {
    const colon = line.indexOf(":");
    if (colon < 1) throw new Error("Invalid Git CGI header");
    const name = line.slice(0, colon).toLowerCase();
    const value = line.slice(colon + 1).trim();
    if (name === "status") {
      if (!/^[2-5][0-9]{2}(?: |$)/.test(value)) throw new Error("Invalid Git CGI status");
      status = Number(value.slice(0, 3));
    } else if (["content-type", "content-length", "cache-control", "expires", "pragma"].includes(name)) {
      responseHeaders.set(name, value);
    }
  }
  if (status >= 500) throw new GitError("http-backend", `HTTP ${status}`);
  if (status >= 400) throw new HttpError(status);
  if (!responseHeaders.has("content-type")) throw new Error("Missing Git CGI content type");
  res.statusCode = status;
  for (const [name, value] of responseHeaders) res.setHeader(name, value);
}

async function smartHttp(req, res, id, action, service, options) {
  const cgi = {
    GIT_PROJECT_ROOT: WINDOWS ? options.cwd.replaceAll("\\", "/") : options.cwd,
    GIT_HTTP_EXPORT_ALL: "1",
    REQUEST_METHOD: req.method,
    SERVER_PROTOCOL: `HTTP/${req.httpVersion}`,
    PATH_INFO: `/${id}.git/${action}`,
    QUERY_STRING: action === "info/refs" ? `service=${service}` : "",
    REMOTE_USER: "shard",
  };
  for (const [header, key] of [
    ["git-protocol", "HTTP_GIT_PROTOCOL"], ["content-type", "CONTENT_TYPE"],
    ["content-encoding", "HTTP_CONTENT_ENCODING"], ["content-length", "CONTENT_LENGTH"],
  ]) {
    if (req.headers[header] !== undefined) cgi[key] = req.headers[header];
  }
  const receiveBody = req.method === "POST";
  const git = startGit("http-backend", [], { ...options, env: cgi, pipeInput: receiveBody });
  function drainRequest() {
    req.unpipe(git.child.stdin);
    req.resume();
  }
  if (receiveBody) {
    // With no Content-Length, CGI reads the dechunked stream to EOF.
    git.child.stdin.once("close", drainRequest);
    req.pipe(git.child.stdin);
  }
  try {
    const stdout = git.child.stdout[Symbol.asyncIterator]();
    const cgiResponse = await readCgiHeaders(stdout);
    if (!cgiResponse) {
      await git.check();
      throw new Error("Missing Git CGI headers");
    }
    setCgiHeaders(res, cgiResponse.headers);
    await writeChunk(res, cgiResponse.body, options.signal);
    for await (const chunk of stdout) await writeChunk(res, chunk, options.signal);
    await git.check();
    res.end();
  } finally {
    if (receiveBody) {
      git.child.stdin.off("close", drainRequest);
      drainRequest();
    }
    git.stop();
    await git.closed;
  }
}

export async function createGitOrigin({ directory, token, importHosts = DEFAULT_IMPORT_HOSTS }) {
  if (typeof directory !== "string" || !directory) throw new TypeError("A Git directory is required");
  if (typeof token !== "string" || !/^[\x21-\x7e]+$/.test(token)) throw new TypeError("A Git token is required");
  const hosts = configuredImportHosts(importHosts);
  const expectedToken = createHash("sha256").update(`Bearer ${token}`).digest();
  await mkdir(directory, { mode: 0o700, recursive: true });
  const cwd = await realpath(directory);
  const execPath = (await gitOutput("--exec-path", [], { cwd })).toString("utf8").replace(/\r?\n$/, "");
  if (!isAbsolute(execPath)) throw new Error("Invalid Git executable directory");
  const httpBackend = join(execPath, WINDOWS ? "git-http-backend.exe" : "git-http-backend");
  const mutations = new Set();
  const readers = new Map();

  function key(id) {
    return WINDOWS ? id.toLowerCase() : id;
  }

  function repoPath(id) {
    return join(cwd, `${id}.git`);
  }

  async function withRepo(id, operation) {
    const repoKey = key(id);
    if (mutations.has(repoKey)) throw new HttpError(404);
    readers.set(repoKey, (readers.get(repoKey) ?? 0) + 1);
    try {
      const repo = repoPath(id);
      if (!(await statusOf(repo))?.isDirectory()) throw new HttpError(404);
      return await operation(repo);
    } finally {
      const count = readers.get(repoKey);
      if (count === 1) readers.delete(repoKey);
      else readers.set(repoKey, count - 1);
    }
  }

  async function withStaging(operation) {
    const staging = await mkdtemp(join(cwd, ".shard-staging-"));
    try {
      return await operation(staging, join(staging, "repo.git"));
    } finally {
      // Publication is complete once renamed, even if temporary cleanup fails.
      await rm(staging, { recursive: true, force: true, maxRetries: 3 }).catch(() => {
        console.error("Git origin could not remove a staging directory");
      });
    }
  }

  async function mirror(source, staging, repo, protocol) {
    const upstream = source.endsWith("/") ? source : `${source}/`;
    const cloned = await gitOutput("clone", [
      "--mirror", "--no-local", "--template=", "--no-recurse-submodules", "--reject-shallow",
      "--", source, repo,
    ], {
      cwd: staging,
      missingMessage: protocol === "https" ? `fatal: repository '${upstream}' not found` : undefined,
      env: {
        HOME: staging,
        USERPROFILE: staging,
        GIT_ALLOW_PROTOCOL: protocol,
        GIT_CONFIG_COUNT: "5",
        GIT_CONFIG_KEY_0: "http.followRedirects", GIT_CONFIG_VALUE_0: "false",
        GIT_CONFIG_KEY_1: "credential.helper", GIT_CONFIG_VALUE_1: "",
        GIT_CONFIG_KEY_2: "transfer.bundleURI", GIT_CONFIG_VALUE_2: "false",
        GIT_CONFIG_KEY_3: "fetch.fsckObjects", GIT_CONFIG_VALUE_3: "true",
        GIT_CONFIG_KEY_4: "init.defaultBranch", GIT_CONFIG_VALUE_4: "main",
      },
    });
    if (!cloned) throw new HttpError(404);
    const format = await gitOutput("rev-parse", ["--show-object-format"], { cwd, repo });
    if (format.toString("ascii").trimEnd() !== "sha1") throw new HttpError(422);
    // Removing the config section directly preserves every mirrored ref.
    await gitOutput("config", ["--local", "--remove-section", "remote.origin"], { cwd, repo });
  }

  async function createRepo(req, res, id) {
    const repoKey = key(id);
    if (mutations.has(repoKey)) return sendStatus(res, 202);
    const { kind, source } = await readBody(req);
    let remote;
    if (kind === "import") {
      remote = importUrl(source, hosts);
    } else if (kind === "fork") {
      if (typeof source !== "string" || !ID.test(source)) throw new HttpError(422);
    } else if (kind !== "create" || source !== undefined) {
      throw new HttpError(422);
    }
    if (mutations.has(repoKey)) return sendStatus(res, 202);
    mutations.add(repoKey);
    try {
      const target = repoPath(id);
      if (await statusOf(target)) throw new HttpError(409);
      // Once accepted, a copy can be polled even if the POST client disconnects.
      const result = await withStaging(async (staging, repo) => {
        if (kind === "create") {
          await gitOutput("init", ["--bare", "--initial-branch=main", "--object-format=sha1", "--template=", repo], { cwd: staging });
        } else if (kind === "fork") {
          await withRepo(source, async (sourceRepo) => {
            const symbolicHead = await headRef({ cwd, repo: sourceRepo });
            await mirror(pathToFileURL(sourceRepo).href, staging, repo, "file");
            if (symbolicHead) await gitOutput("symbolic-ref", ["HEAD", symbolicHead], { cwd, repo });
          });
        } else {
          await mirror(remote, staging, repo, "https");
        }
        await appendFile(join(repo, "config"), REPO_CONFIG);
        const result = metadata(id, await headRef({ cwd, repo }));
        await rename(repo, target);
        return result;
      });
      if (!res.destroyed) sendJson(res, 201, result);
    } finally {
      mutations.delete(repoKey);
    }
  }

  async function deleteRepo(res, id) {
    const repoKey = key(id);
    if (mutations.has(repoKey) || readers.has(repoKey)) return sendStatus(res, 202);
    mutations.add(repoKey);
    try {
      const target = repoPath(id);
      if ((await statusOf(target))?.isDirectory()) {
        await withStaging(async (_staging, repo) => rename(target, repo));
      }
      sendStatus(res, 204);
    } finally {
      mutations.delete(repoKey);
    }
  }

  async function handle(req, res, signal) {
    const auth = req.headersDistinct.authorization;
    if (auth?.length !== 1 || !timingSafeEqual(createHash("sha256").update(auth[0]).digest(), expectedToken)) {
      res.setHeader("WWW-Authenticate", "Bearer");
      throw new HttpError(401);
    }
    const url = req.url ?? "";
    const delimiter = url.indexOf("?");
    const path = delimiter < 0 ? url : url.slice(0, delimiter);
    const query = new URLSearchParams(delimiter < 0 ? "" : url.slice(delimiter + 1));
    const control = /^\/v1\/repos\/([^/]+)(?:\/(log|commit|tree|blob|file))?$/.exec(path);
    if (control && ID.test(control[1])) {
      const [, id, action] = control;
      if (action && req.method === "GET") {
        return withRepo(id, (repo) => readObject(res, action, query, { cwd, repo, signal }));
      }
      if (!action) {
        if (req.method === "POST") return createRepo(req, res, id);
        if (req.method === "DELETE") return deleteRepo(res, id);
        if (req.method === "GET") {
          if (mutations.has(key(id))) return sendStatus(res, 202);
          return withRepo(id, async (repo) => sendJson(res, 200, metadata(id, await headRef({ cwd, repo, signal }))));
        }
      }
    }
    const route = /^\/v1\/git\/([^/]+)\.git\/(info\/refs|git-upload-pack|git-receive-pack)$/.exec(path);
    if (route && ID.test(route[1])) {
      const [, id, action] = route;
      if (action === "info/refs" && req.method === "GET") {
        const services = query.getAll("service");
        if (services.length === 1 && SERVICE.test(services[0])) {
          return withRepo(id, () => smartHttp(req, res, id, action, services[0], { cwd, signal, command: httpBackend }));
        }
      } else if (action !== "info/refs" && req.method === "POST" && query.size === 0) {
        return withRepo(id, () => smartHttp(req, res, id, action, action, { cwd, signal, command: httpBackend }));
      }
    }
    throw new HttpError(404);
  }

  return createServer((req, res) => {
    const cancellation = new AbortController();
    req.on("error", () => cancellation.abort());
    res.on("error", () => cancellation.abort());
    res.once("close", () => { if (!res.writableFinished) cancellation.abort(); });
    res.setHeader("Cache-Control", "no-store");
    void handle(req, res, cancellation.signal).catch((error) => {
      if (cancellation.signal.aborted) return;
      const expected = error instanceof HttpError;
      if (!expected) {
        if (error instanceof GitError) console.error(`Git origin ${error.operation} failed (${error.status})`);
        else console.error("Git origin request failed");
      }
      if (res.headersSent) {
        res.destroy();
      } else if (!res.destroyed) {
        res.removeHeader("Content-Length");
        res.removeHeader("X-Shard-Object-Size");
        sendStatus(res, expected ? error.status : 502);
      }
    }).finally(() => req.resume());
  });
}

async function main() {
  const port = Number(process.env.SHARD_GIT_PORT ?? "8793");
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("Invalid Git origin port");
  const server = await createGitOrigin({
    directory: process.env.SHARD_GIT_DIRECTORY,
    token: process.env.SHARD_GIT_TOKEN,
    importHosts: process.env.SHARD_GIT_IMPORT_HOSTS?.split(",").filter((host) => host.trim()),
  });
  server.listen(port, process.env.SHARD_GIT_HOST ?? "127.0.0.1");
  await once(server, "listening");
  console.log(`Git origin listening on port ${port}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => {
    console.error("Git origin could not start");
    process.exitCode = 1;
  });
}
