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
const ZERO_OID = "0".repeat(40);
const DEFAULT_IMPORT_HOSTS = ["github.com", "gitlab.com", "codeberg.org"];
const MAX_BODY_BYTES = 8 * 1024;
const MAX_METADATA_BYTES = 4 * 1024 * 1024;
const MAX_CGI_HEADER_BYTES = 8 * 1024;
const MAX_GIT_DIAGNOSTIC_BYTES = 8 * 1024;
const MAX_QUERY_VALUE_BYTES = 4 * 1024;
const COMPARISON_TIMEOUT_MS = 5_000;
const MAX_COMPARISON_CHANGES = 256;
const MAX_COMPARISON_PATH_BYTES = 4_096;
const MAX_COMPARISON_PATH_PARTS = 13;
// Each raw record has a 98-byte header and two NUL delimiters.
const MAX_RAW_DIFF_BYTES = MAX_COMPARISON_CHANGES * (MAX_COMPARISON_PATH_BYTES + 100);
const RAW_DIFF_ENTRY = /^:(000000|100644|100755|120000|160000) (000000|100644|100755|120000|160000) ([0-9a-f]{40}) ([0-9a-f]{40}) [ADMT]$/;
const BRANCH_PREFIX = "refs/heads/";
const TAG_PREFIX = "refs/tags/";
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
  const { repo, input, missing, keepMissing, capturePrefix, maxBytes = MAX_METADATA_BYTES } = options;
  const argv = repo ? repoCommand(repo, command, args) : ["--no-pager", command, ...args];
  const git = startGit(command, argv, { ...options, pipeInput: input !== undefined });
  if (input !== undefined) git.child.stdin.end(input);
  try {
    const chunks = [];
    let length = 0;
    for await (const chunk of git.child.stdout) {
      const captured = capturePrefix === undefined ? chunk : chunk.subarray(0, capturePrefix - length);
      if (!captured.length) continue;
      length += captured.length;
      if (length > maxBytes) throw new HttpError(413);
      chunks.push(captured);
    }
    if (!await git.check(missing) && !keepMissing) return null;
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

function sendComparison(res, payload) {
  const body = JSON.stringify(payload);
  const length = Buffer.byteLength(body);
  if (length > MAX_METADATA_BYTES) throw new HttpError(413);
  res.setHeader("Content-Type", "application/json; charset=utf-8");
  res.setHeader("Content-Length", length);
  res.end(body);
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

function configuredArtifactsOrigins(origins) {
  const result = new Set();
  for (const origin of origins) {
    const url = URL.parse(origin.trim());
    if (!url || origin.trim() !== url.origin) throw new TypeError("Invalid Artifacts Git origin");
    const hosted = url.protocol === "https:" && !url.port && /^[0-9a-f]{32}\.artifacts\.cloudflare\.net$/.test(url.hostname);
    const local = url.protocol === "http:" && url.port && ["127.0.0.1", "[::1]"].includes(url.hostname);
    if (!hosted && !local) throw new TypeError("Invalid Artifacts Git origin");
    result.add(url.origin);
  }
  return result;
}

function artifactsRemote(value, allowedOrigins) {
  const { remote, token } = value ?? {};
  const url = typeof remote === "string" ? URL.parse(remote) : null;
  if (!url || remote !== `${url.origin}${url.pathname}` || !allowedOrigins.has(url.origin) ||
      !/^\/git\/[a-zA-Z0-9._-]+\/[a-zA-Z0-9._-]+\.git$/.test(url.pathname)) {
    throw new HttpError(422);
  }
  return { url: remote, protocol: url.protocol.slice(0, -1), authorization: bearerAuthorization(token) };
}

function bearerAuthorization(token) {
  if (typeof token !== "string" || !token || /[^\x21-\x7e]/.test(token)) throw new HttpError(422);
  return `Authorization: Bearer ${token}`;
}

function importAuthorization(credential) {
  if (credential === undefined) return undefined;
  if (credential?.kind === "bearer") return bearerAuthorization(credential.token);
  if (credential?.kind === "basic") {
    const { username, password } = credential;
    if (typeof username === "string" && username && !/[:\x00-\x1f\x7f]/.test(username) &&
        typeof password === "string" && password && !/[\x00-\x1f\x7f]/.test(password)) {
      return `Authorization: Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`;
    }
  }
  throw new HttpError(422);
}

function gitConfig(settings) {
  const env = { GIT_CONFIG_COUNT: String(settings.length) };
  for (const [index, [name, value]] of settings.entries()) {
    env[`GIT_CONFIG_KEY_${index}`] = name;
    env[`GIT_CONFIG_VALUE_${index}`] = value;
  }
  return env;
}

function httpConfig(url, authorization) {
  return [
    ["http.followRedirects", "false"],
    ["credential.helper", ""],
    ...(authorization ? [[`http.${url}.extraHeader`, authorization]] : []),
  ];
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

function controlOid(value, allowZero = false) {
  if (typeof value !== "string" || !OID.test(value) || (!allowZero && value === ZERO_OID)) throw new HttpError(422);
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

async function comparisonBase(base, head, target, options) {
  // The first two results suffice to distinguish a unique base from an ambiguous one.
  const output = await gitOutput("merge-base", ["--all", base, head], {
    ...options, missing: 1, capturePrefix: 2 * (40 + 2),
  });
  if (!output) return [];
  const bases = output.toString("latin1").trimEnd().split(/\r?\n/, 2);
  if (bases.some((hash) => !OID.test(hash) || hash === ZERO_OID)) throw new Error("Invalid Git merge base output");
  // The saved base must remain readable directly in the target repository.
  if (bases.length === 1) await exactObject(bases[0], "commit", target);
  return bases;
}

function diffObject(mode, hash) {
  if ((mode === "000000") !== (hash === ZERO_OID)) throw new Error("Invalid Git diff object");
  return mode === "000000" ? null : { mode, hash };
}

async function comparisonFiles(base, head, options) {
  const output = await gitOutput("diff-tree", [
    "-r", "-z", "--raw", "--no-abbrev", "--no-commit-id", "--no-renames", "--no-ext-diff",
    "--no-relative", "--ignore-submodules=none", base, head, "--",
  ], { ...options, maxBytes: MAX_RAW_DIFF_BYTES });
  const changes = [];
  let at = 0;
  while (at < output.length) {
    if (changes.length === MAX_COMPARISON_CHANGES) throw new HttpError(413);
    const fieldEnd = output.indexOf(0, at);
    const pathEnd = output.indexOf(0, fieldEnd + 1);
    if (fieldEnd < 0 || pathEnd < 0) throw new Error("Incomplete Git diff output");
    const fields = RAW_DIFF_ENTRY.exec(output.toString("latin1", at, fieldEnd));
    if (!fields) throw new Error("Invalid Git diff output");
    if (pathEnd - fieldEnd - 1 > MAX_COMPARISON_PATH_BYTES) throw new HttpError(413);
    const path = output.toString("utf8", fieldEnd + 1, pathEnd);
    const parts = path.split("/");
    if (parts.some((part) => part === "" || part === "." || part === "..")) throw new Error("Invalid Git diff path");
    if (parts.length > MAX_COMPARISON_PATH_PARTS) throw new HttpError(413);
    const [, oldMode, newMode, oldHash, newHash] = fields;
    changes.push({ path, before: diffObject(oldMode, oldHash), after: diffObject(newMode, newHash) });
    at = pathEnd + 1;
  }
  return changes;
}

async function carriesComparison(tip, base, head, options) {
  await exactObject(tip, "commit", options);
  for (const ancestor of [base, head]) {
    if (await resolveRevision(`${ancestor}^{commit}`, options) !== ancestor) return false;
    if (!await gitOutput("merge-base", ["--is-ancestor", ancestor, tip], { ...options, missing: 1 })) return false;
  }
  return true;
}

async function validateRef(reference, branchesOnly, options) {
  if (typeof reference !== "string" || !reference.isWellFormed() || /[\x00-\x20\x7f]/.test(reference) ||
      Buffer.byteLength(reference) > MAX_QUERY_VALUE_BYTES ||
      !(reference.startsWith(BRANCH_PREFIX) && reference.length > BRANCH_PREFIX.length ||
        !branchesOnly && reference.startsWith(TAG_PREFIX) && reference.length > TAG_PREFIX.length) ||
      !await gitOutput("check-ref-format", [reference], { ...options, missing: 1 })) {
    throw new HttpError(422);
  }
}

async function refTip(reference, options) {
  // A pattern can also return descendants. The exact ref sorts first if present.
  const output = await gitOutput("for-each-ref", [
    "--count=1", "--sort=refname", "--format=%(refname)%00%(objectname)%00%(symref)", "--", reference,
  ], options);
  const [name, hash, symbolic] = output.toString("utf8").trimEnd().split("\0");
  if (name !== reference) return ZERO_OID;
  if (symbolic) throw new HttpError(422);
  if (!OID.test(hash)) throw new Error("Invalid Git ref output");
  return hash;
}

async function updateRef(reference, newOid, oldOid, options) {
  try {
    await gitOutput("update-ref", ["--no-deref", "--", reference, newOid, oldOid], options);
  } catch (error) {
    if (error instanceof GitError && error.status === 128 && await refTip(reference, options) !== oldOid) {
      throw new HttpError(412);
    }
    throw error;
  }
}

async function mergeCommit(baseOid, headOid, options, canWriteTree) {
  const ancestry = { ...options, missing: 1 };
  if (await gitOutput("merge-base", ["--is-ancestor", baseOid, headOid], ancestry)) return headOid;
  if (await gitOutput("merge-base", ["--is-ancestor", headOid, baseOid], ancestry)) return baseOid;
  if (!await gitOutput("merge-base", [baseOid, headOid], ancestry)) throw new HttpError(409);
  if (!canWriteTree) throw new HttpError(501);
  const tree = await gitOutput("merge-tree", ["--write-tree", "--no-messages", baseOid, headOid], {
    ...ancestry, capturePrefix: 42,
  });
  if (!tree) throw new HttpError(409);
  const treeOid = tree.toString("ascii").trimEnd();
  if (!OID.test(treeOid)) throw new Error("Invalid Git merge tree output");
  const commit = await gitOutput("commit-tree", [treeOid, "-p", baseOid, "-p", headOid, "-m", "Merge pull request"], {
    ...options,
    env: {
      GIT_AUTHOR_NAME: "Shard", GIT_AUTHOR_EMAIL: "shard@localhost",
      GIT_COMMITTER_NAME: "Shard", GIT_COMMITTER_EMAIL: "shard@localhost",
    },
  });
  const hash = commit.toString("ascii").trimEnd();
  if (!OID.test(hash)) throw new Error("Invalid Git merge commit output");
  return hash;
}

async function fetchObjects(remote, refspec, options) {
  await gitOutput("fetch", [
    "--no-tags", "--no-write-fetch-head", "--no-auto-gc", "--no-recurse-submodules", "--", remote, refspec,
  ], options);
}

function remoteOptions(remote, options) {
  return {
    ...options,
    env: {
      HOME: options.cwd,
      USERPROFILE: options.cwd,
      GIT_ALLOW_PROTOCOL: remote.protocol,
      ...gitConfig([
        ...httpConfig(remote.url, remote.authorization),
        ["transfer.bundleURI", "false"],
        ["fetch.fsckObjects", "true"],
      ]),
    },
  };
}

async function remoteTip(remote, reference, options) {
  const output = await gitOutput("ls-remote", ["--refs", "--", remote, reference], options);
  for (const line of output.toString("utf8").trimEnd().split("\n")) {
    const [hash, name] = line.split("\t");
    if (name !== reference) continue;
    if (!OID.test(hash)) throw new Error("Invalid Git remote ref output");
    return hash;
  }
  return ZERO_OID;
}

async function pushMerge(remote, reference, baseOid, oid, options) {
  const result = await gitOutput("push", [
    "--porcelain", `--force-with-lease=${reference}:${baseOid}`, "--", remote, `${oid}:${reference}`,
  ], { ...options, missing: 1, keepMissing: true });
  for (const line of result.toString("utf8").split("\n")) {
    const [flag, mapping, summary] = line.split("\t");
    if (!mapping?.endsWith(`:${reference}`)) continue;
    if (flag === " " || flag === "+") return;
    if (flag === "=" && oid === baseOid) return;
    if (flag === "=" || flag === "!" && summary === "[rejected] (stale info)") throw new HttpError(412);
  }
  // The server can reject the old OID after the push advertisement was sent.
  if (await remoteTip(remote, reference, options) !== baseOid) throw new HttpError(412);
  throw new GitError("push", 1);
}

async function readCommits(hash, limit, fullMessage, options) {
  const format = `%H%x00%T%x00%an%x00%ae%x00%at%x00%P%x00${fullMessage ? "%B" : "%s"}`;
  const output = await gitOutput("log", [
    "--first-parent", "--no-show-signature", "--no-notes", "--encoding=UTF-8", "-z",
    `--max-count=${limit}`, `--format=tformat:${format}`, hash, "--",
  ], options);
  const fields = output.toString("utf8").split("\0");
  if (fields.pop() !== "" || fields.length % 7 !== 0) throw new Error("Invalid Git commit output");
  const commits = [];
  for (let index = 0; index < fields.length; index += 7) {
    const [hash, tree, author, author_email, seconds, parentList, message] = fields.slice(index, index + 7);
    const parents = parentList ? parentList.split(" ") : [];
    const timestamp = Number(seconds);
    if (!OID.test(hash) || !OID.test(tree) || !parents.every((parent) => OID.test(parent)) ||
        !/^-?\d+$/.test(seconds) || !Number.isSafeInteger(timestamp)) {
      throw new Error("Invalid Git commit fields");
    }
    commits.push({ hash, tree, message, author, author_email, timestamp, parents });
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
  options.signal.throwIfAborted();
  const pushing = action === "git-receive-pack";
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
  // A receive-pack must finish before another operation can switch HEAD.
  const git = startGit("http-backend", [], {
    ...options, env: cgi, pipeInput: receiveBody, signal: pushing ? undefined : options.signal,
  });
  function drainRequest() {
    req.unpipe(git.child.stdin);
    req.resume();
  }
  function finishPushInput() {
    drainRequest();
    git.child.stdin.end();
  }
  if (pushing) options.signal.addEventListener("abort", finishPushInput, { once: true });
  if (receiveBody) {
    // With no Content-Length, CGI reads the dechunked stream to EOF.
    git.child.stdin.once("close", drainRequest);
    req.pipe(git.child.stdin);
  }
  try {
    const stdout = git.child.stdout.iterator({ destroyOnReturn: !pushing });
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
    if (pushing) {
      options.signal.removeEventListener("abort", finishPushInput);
      finishPushInput();
      git.child.stdout.resume();
    } else {
      git.stop();
    }
    await git.closed;
  }
}

export async function createGitOrigin({ directory, token, importHosts = DEFAULT_IMPORT_HOSTS, artifactsOrigins = [] }) {
  if (typeof directory !== "string" || !directory) throw new TypeError("A Git directory is required");
  if (typeof token !== "string" || !/^[\x21-\x7e]+$/.test(token)) throw new TypeError("A Git token is required");
  const hosts = configuredImportHosts(importHosts);
  const allowedOrigins = configuredArtifactsOrigins(artifactsOrigins);
  const expectedToken = createHash("sha256").update(`Bearer ${token}`).digest();
  await mkdir(directory, { mode: 0o700, recursive: true });
  const cwd = await realpath(directory);
  const execPath = (await gitOutput("--exec-path", [], { cwd })).toString("utf8").replace(/\r?\n$/, "");
  if (!isAbsolute(execPath)) throw new Error("Invalid Git executable directory");
  const httpBackend = join(execPath, WINDOWS ? "git-http-backend.exe" : "git-http-backend");
  const version = (await gitOutput("--version", [], { cwd })).toString("ascii");
  const [, major, minor] = /^git version (\d+)\.(\d+)\./.exec(version) ?? [];
  const canWriteTree = Number(major) > 2 || Number(major) === 2 && Number(minor) >= 38;
  const mutations = new Set();
  const readers = new Map();
  const writes = new Map();

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

  async function withWrite(id, signal, operation) {
    return withRepo(id, async (repo) => {
      const repoKey = key(id);
      const previous = writes.get(repoKey);
      const { promise, resolve } = Promise.withResolvers();
      writes.set(repoKey, promise);
      try {
        await previous;
        signal.throwIfAborted();
        return await operation(repo);
      } finally {
        if (writes.get(repoKey) === promise) writes.delete(repoKey);
        resolve();
      }
    });
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

  async function mirror(source, staging, repo, protocol, authorization) {
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
        ...gitConfig([
          ...httpConfig(source, authorization),
          ["transfer.bundleURI", "false"],
          ["fetch.fsckObjects", "true"],
          ["init.defaultBranch", "main"],
        ]),
      },
    }).catch((error) => {
      // The staged clone failed before publication, so the caller can release its reservation.
      if (protocol === "https" && error instanceof GitError && error.status === 128) throw new HttpError(422);
      throw error;
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
    const { kind, source, credential } = await readBody(req);
    let remote;
    let authorization;
    if (kind === "import") {
      remote = importUrl(source, hosts);
      authorization = importAuthorization(credential);
      if (authorization && new URL(remote).pathname === "/") throw new HttpError(422);
    } else if (kind === "fork") {
      if (typeof source !== "string" || !ID.test(source) || credential !== undefined) throw new HttpError(422);
    } else if (kind !== "create" || source !== undefined || credential !== undefined) {
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
          await mirror(remote, staging, repo, "https", authorization);
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

  async function compareSnapshots(res, id, action, query, signal) {
    res.setHeader("X-Shard-Comparison", "1");
    const base = controlOid(queryValue(query, "base"), true);
    const head = controlOid(queryValue(query, "head"), true);
    const source = query.has("source") ? queryValue(query, "source") : id;
    const tip = action === "integrated" ? controlOid(queryValue(query, "tip"), true) : null;
    if (!ID.test(source) || (tip !== null && query.has("source"))) throw new HttpError(422);

    const timeout = AbortSignal.timeout(COMPARISON_TIMEOUT_MS);
    const comparisonSignal = AbortSignal.any([signal, timeout]);
    try {
      const result = await withRepo(id, async (repo) => {
        const target = { cwd, repo, signal: comparisonSignal };
        if (tip !== null) return carriesComparison(tip, base, head, target);

        await exactObject(base, "commit", target);
        async function compare(sourceRepo) {
          const sourceOptions = sourceRepo === repo ? target : { ...target, repo: sourceRepo };
          await exactObject(head, "commit", sourceOptions);
          const objects = join(sourceRepo, "objects");
          // Git accepts C-quoted alternate paths, including the platform's list separator.
          const options = sourceRepo === repo ? target : {
            ...target, env: { GIT_ALTERNATE_OBJECT_DIRECTORIES: `"${objects.replace(/[\\"]/g, "\\$&")}"` },
          };
          if (action === "merge-base") return comparisonBase(base, head, target, options);
          const baseTree = await resolveRevision(`${base}^{tree}`, target);
          const headTree = await resolveRevision(`${head}^{tree}`, sourceOptions);
          if (!baseTree || !headTree) throw new Error("Invalid Git comparison tree");
          return comparisonFiles(baseTree, headTree, options);
        }

        return key(source) === key(id) ? compare(repo) : withRepo(source, compare);
      });
      comparisonSignal.throwIfAborted();
      sendComparison(res, result);
    } catch (error) {
      if (timeout.aborted) throw new HttpError(413);
      throw error;
    }
  }

  async function mutateRefs(req, res, id, action, signal) {
    const { reference, newOid, oldOid, baseOid, headOid, source } = await readBody(req);
    await withWrite(id, signal, async (repo) => {
      // Once begun, finish any accepted ref transaction before releasing the lock.
      const options = { cwd, repo };
      await validateRef(reference, action !== "ref", options);
      const current = await refTip(reference, options);
      if (action === "head") {
        if (current === ZERO_OID) throw new HttpError(404);
        await exactObject(current, "commit", options);
        await gitOutput("symbolic-ref", ["HEAD", reference], options);
        return sendStatus(res, 204);
      }
      if (action === "ref") {
        controlOid(newOid, true);
        controlOid(oldOid, true);
        if (newOid === ZERO_OID && oldOid === ZERO_OID) throw new HttpError(422);
        if (current !== oldOid) throw new HttpError(412);
        if (newOid === ZERO_OID) {
          if (await headRef(options) === reference) throw new HttpError(422);
        } else {
          await exactObject(newOid, reference.startsWith(BRANCH_PREFIX) ? "commit" : "object", options);
        }
        await updateRef(reference, newOid, oldOid, options);
        return sendStatus(res, 204);
      }

      controlOid(baseOid);
      controlOid(headOid);
      if (source !== undefined && (typeof source !== "string" || !ID.test(source))) throw new HttpError(422);
      if (current !== baseOid) throw new HttpError(412);
      await exactObject(baseOid, "commit", options);
      if (source !== undefined && key(source) !== key(id)) {
        await withRepo(source, async (sourceRepo) => {
          await exactObject(headOid, "commit", { cwd, repo: sourceRepo });
          await fetchObjects(pathToFileURL(sourceRepo).href, headOid, {
            ...options,
            env: { GIT_ALLOW_PROTOCOL: "file", ...gitConfig([["protocol.version", "2"]]) },
          });
        });
      }
      await exactObject(headOid, "commit", options);
      const oid = await mergeCommit(baseOid, headOid, options, canWriteTree);
      await updateRef(reference, oid, baseOid, options);
      sendJson(res, 200, { oid });
    });
  }

  async function mergeArtifacts(req, res, signal) {
    if (!allowedOrigins.size) throw new HttpError(501);
    const { reference, baseOid, headOid, target, source } = await readBody(req);
    controlOid(baseOid);
    controlOid(headOid);
    const targetRepo = artifactsRemote(target, allowedOrigins);
    const sourceRepo = source === undefined ? targetRepo : artifactsRemote(source, allowedOrigins);
    signal.throwIfAborted();
    await withStaging(async (staging, repo) => {
      const options = { cwd: staging, repo };
      await validateRef(reference, true, { cwd: staging });
      await gitOutput("init", ["--bare", "--initial-branch=main", "--object-format=sha1", "--template=", repo], { cwd: staging });
      const targetOptions = remoteOptions(targetRepo, options);
      const baseRef = "refs/shard/base";
      try {
        await fetchObjects(targetRepo.url, `${reference}:${baseRef}`, targetOptions);
      } catch (error) {
        if (error instanceof GitError && await remoteTip(targetRepo.url, reference, targetOptions) !== baseOid) {
          throw new HttpError(412);
        }
        throw error;
      }
      if (await refTip(baseRef, options) !== baseOid) throw new HttpError(412);
      await exactObject(baseOid, "commit", options);
      if (targetRepo.url !== sourceRepo.url || await resolveRevision(`${headOid}^{commit}`, options) !== headOid) {
        await fetchObjects(sourceRepo.url, headOid, remoteOptions(sourceRepo, options));
      }
      await exactObject(headOid, "commit", options);
      const oid = await mergeCommit(baseOid, headOid, options, canWriteTree);
      await pushMerge(targetRepo.url, reference, baseOid, oid, targetOptions);
      sendJson(res, 200, { oid });
    });
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
    if (path === "/v1/artifacts/merge" && req.method === "POST" && query.size === 0) {
      return mergeArtifacts(req, res, signal);
    }
    const control = /^\/v1\/repos\/([^/]+)(?:\/(log|commit|tree|blob|file|ref|head|merge|merge-base|compare|integrated))?$/.exec(path);
    if (control && ID.test(control[1])) {
      const [, id, action] = control;
      if (action === "ref" || action === "head" || action === "merge") {
        if (req.method === "POST" && query.size === 0) return mutateRefs(req, res, id, action, signal);
      } else if (action === "merge-base" || action === "compare" || action === "integrated") {
        if (req.method === "GET") return compareSnapshots(res, id, action, query, signal);
      } else if (action && req.method === "GET") {
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
        if (action === "git-receive-pack") {
          return withWrite(id, signal, () => smartHttp(req, res, id, action, action, { cwd, signal, command: httpBackend }));
        }
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
    artifactsOrigins: process.env.SHARD_GIT_ARTIFACTS_ORIGINS?.split(",").filter((origin) => origin.trim()),
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
