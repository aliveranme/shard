import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { experimental_readRawConfig, unstable_dev } from "wrangler";
import { createGitOrigin } from "../scripts/git-origin.mjs";
import { exerciseLfs } from "./lfs.mjs";

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const wrangler = join(root, "node_modules/wrangler/bin/wrangler.js");
const bearer = (token) => ({ Authorization: `Bearer ${token}` });
const remoteUrl = (base, repo) => `${base}${repo}.git`;
const infoRefs = (repo, service) => `${repo}.git/info/refs?service=git-${service}-pack`;
const request = (url, options) => fetch(url, {
  redirect: "manual", signal: AbortSignal.timeout(30_000), ...options,
});

let clientNumber = 0;
class Client {
  source = `2001:db8::${(++clientNumber).toString(16)}`;
  cookies = new Map();
  csrf = "";

  constructor(base) { this.base = base; }

  async send(path, options = {}) {
    const headers = new Headers(options.headers);
    headers.set("Accept-Language", "en");
    headers.set("CF-Connecting-IP", this.source);
    if (this.cookies.size) headers.set("Cookie", [...this.cookies]
      .map(([name, value]) => `${name}=${value}`).join("; "));
    const response = await request(new URL(path, this.base), { ...options, headers });
    for (const cookie of response.headers.getSetCookie()) {
      const pair = cookie.split(";", 1)[0];
      const at = pair.indexOf("=");
      if (pair.slice(at + 1)) this.cookies.set(pair.slice(0, at), pair.slice(at + 1));
      else this.cookies.delete(pair.slice(0, at));
    }
    const body = await response.text();
    assert.ok(response.status < 500, `${path}: HTTP ${response.status}\n${body.slice(-1000)}`);
    const csrf = body.match(/name="csrf_token" value="([a-f0-9]{64})"/);
    if (csrf) this.csrf = csrf[1];
    return { status: response.status, body, location: response.headers.get("location") };
  }

  get(path, token) { return this.send(path, { headers: token ? bearer(token) : {} }); }

  post(path, fields) {
    return this.send(path, {
      method: "POST", headers: { Origin: this.base },
      body: new URLSearchParams({ csrf_token: this.csrf, ...fields }),
    });
  }
}

function redirected(response, location) {
  assert.equal(response.status, 302, response.body.slice(0, 500));
  if (location !== undefined) assert.equal(response.location, location);
}

async function signup(base, username) {
  const client = new Client(base);
  await client.get("/user/sign_up");
  redirected(await client.post("/user/sign_up", {
    username, email: `${username}@example.test`, password: "local-native-git-password",
  }));
  await client.get("/user/settings");
  return client;
}

async function scopedToken(client, scope) {
  await client.get("/user/settings/tokens");
  const result = await client.post("/user/settings/tokens", {
    name: `Native Git ${scope}`, scope, expires_in: "7",
  });
  assert.equal(result.status, 200);
  const token = result.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
  assert.ok(token, `expected a ${scope} token from the forge`);
  return token;
}

async function startWorker(temporary, nativeOrigin, token) {
  const configPath = join(temporary, "wrangler.json");
  const persistence = join(temporary, "state");
  const { rawConfig: config } = experimental_readRawConfig({ config: join(root, "wrangler.jsonc") });
  delete config.$schema;
  delete config.build;
  config.name = "shard-git-tests";
  config.main = resolve(root, config.main);
  config.assets.directory = resolve(root, config.assets.directory);
  const native = config.env.native;
  native.vars = {
    ...native.vars,
    PUBLIC_ORIGIN: "https://forge.example.test",
    NATIVE_GIT_ORIGIN: nativeOrigin,
  };
  native.d1_databases = native.d1_databases.map((binding) => ({
    ...binding, remote: false, migrations_dir: resolve(root, binding.migrations_dir),
  }));
  await writeFile(configPath, JSON.stringify(config));
  await writeFile(join(temporary, ".dev.vars.native"), `NATIVE_GIT_TOKEN=${token}\n`);
  await exec(process.execPath, [wrangler, "d1", "migrations", "apply", "DB", "--local",
    "--env", "native", "--persist-to", persistence, "--config", configPath], {
    cwd: root, env: { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" },
    timeout: 60_000, maxBuffer: 4 * 1024 * 1024,
  });
  return unstable_dev(config.main, {
    config: configPath, env: "native", local: true, ip: "127.0.0.1", port: 0,
    persist: true, persistTo: persistence, logLevel: "error",
    experimental: {
      disableExperimentalWarning: true, disableDevRegistry: true,
      forceLocal: true, watch: false, showInteractiveDevSession: false,
    },
  });
}

// Each run owns its origin volume, D1 state, and Git configuration.
test("forge native Git over loopback", { timeout: 240_000 }, async (t) => {
  const temporary = await mkdtemp(join(tmpdir(), "shard-native-git-"));
  const token = randomBytes(32).toString("hex");
  let origin;
  let worker;
  try {
    origin = await createGitOrigin({ directory: join(temporary, "bare"), token });
    origin.listen({ host: "127.0.0.1", port: 0 });
    await once(origin, "listening");
    const nativeOrigin = `http://127.0.0.1:${origin.address().port}`;

    await t.test("the origin authenticates management and smart HTTP requests", async () => {
      const id = "shard-origin-auth";
      const control = `${nativeOrigin}/v1/repos/${id}`;
      const gitRepo = `${nativeOrigin}/v1/git/${id}`;
      const rejectedImport = await request(control, {
        method: "POST", headers: { ...bearer(token), "Content-Type": "application/json" },
        body: JSON.stringify({ kind: "import", source: "https://git.example.test/project.git" }),
      });
      await rejectedImport.arrayBuffer();
      assert.equal(rejectedImport.status, 422);
      const created = await request(control, {
        method: "POST", headers: { ...bearer(token), "Content-Type": "application/json" },
        body: JSON.stringify({ kind: "create" }),
      });
      const createdBody = await created.text();
      assert.equal(created.status, 201, createdBody);
      assert.deepEqual(JSON.parse(createdBody), { name: id, defaultBranch: "main" });

      const wrong = bearer(randomBytes(32).toString("hex"));
      for (const [url, options] of [
        [control, {}],
        [control, { method: "DELETE", headers: wrong }],
        [infoRefs(gitRepo, "upload"), { headers: wrong }],
        [`${gitRepo}.git/git-receive-pack`, {
          method: "POST", headers: { "Content-Type": "application/x-git-receive-pack-request" }, body: "0000",
        }],
      ]) {
        const rejected = await request(url, options);
        await rejected.arrayBuffer();
        assert.equal(rejected.status, 401, url);
      }

      const present = await request(control, { headers: bearer(token) });
      assert.equal(present.status, 200);
      assert.deepEqual(await present.json(), { name: id, defaultBranch: "main" });
      const advertised = await request(infoRefs(gitRepo, "upload"), { headers: bearer(token) });
      assert.equal(advertised.status, 200);
      assert.equal(advertised.headers.get("content-type"), "application/x-git-upload-pack-advertisement");
      assert.match(await advertised.text(), /# service=git-upload-pack\n/);
      assert.equal((await request(control, { method: "DELETE", headers: bearer(token) })).status, 204);
      const absent = await request(control, { headers: bearer(token) });
      await absent.arrayBuffer();
      assert.equal(absent.status, 404);
    });

    worker = await startWorker(temporary, nativeOrigin, token);
    const base = `http://127.0.0.1:${worker.port}`;
    const gitConfig = join(temporary, "gitconfig");
    await writeFile(gitConfig, `[core]
  autocrlf = false
[http]
  proxy =
[user]
  name = Native Git Fixture
  email = native-git@example.test
[commit]
  gpgSign = false
[tag]
  gpgSign = false
`);
    const git = async (credential, ...args) => (await exec("git", args, {
      cwd: temporary, timeout: 30_000,
      env: {
        ...process.env, GIT_CONFIG_GLOBAL: gitConfig, GIT_CONFIG_NOSYSTEM: "1",
        GIT_TERMINAL_PROMPT: "0", GIT_CONFIG_COUNT: credential ? "1" : "0",
        ...(credential ? {
          GIT_CONFIG_KEY_0: "http.extraHeader", GIT_CONFIG_VALUE_0: `Authorization: Bearer ${credential}`,
        } : {}),
      },
    })).stdout.trim();

    await t.test("browser and Git operations share repository data and forge permissions", async () => {
      const owner = await signup(base, "nativeowner");
      const viewer = await signup(base, "nativeviewer");
      const guest = new Client(base);
      const project = "/nativeowner/project";
      const vault = "/nativeowner/vault";
      const projectRemote = remoteUrl(base, project);
      const vaultRemote = remoteUrl(base, vault);
      redirected(await owner.post("/repo/create", { name: "project" }), project);
      redirected(await owner.post("/repo/create", { name: "vault", private: "on" }), vault);
      const empty = await guest.get(project);
      assert.equal(empty.status, 200);
      assert.match(empty.body, /This repository is empty/);
      assert.equal((await owner.get(vault)).status, 200);
      assert.equal((await guest.get(vault)).status, 404);
      assert.equal((await viewer.get(vault)).status, 404);
      assert.equal((await guest.get(infoRefs(vault, "upload"))).status, 401);
      assert.equal(await git(null, "ls-remote", projectRemote), "");

      const writeToken = await scopedToken(owner, "write");
      const readToken = await scopedToken(owner, "read");
      const viewerToken = await scopedToken(viewer, "write");
      assert.equal((await owner.get(infoRefs(project, "receive"))).status, 401);
      const unsignedPush = await owner.send(`${project}.git/git-receive-pack`, {
        method: "POST", headers: { "Content-Type": "application/x-git-receive-pack-request" }, body: "0000",
      });
      assert.equal(unsignedPush.status, 401);
      assert.equal((await guest.get(infoRefs(project, "receive"), readToken)).status, 401);
      assert.equal((await guest.get(infoRefs(vault, "upload"), viewerToken)).status, 404);

      const worktree = join(temporary, "seed");
      await git(null, "init", "--object-format=sha1", "--initial-branch=main", worktree);
      const localGit = (...args) => git(null, "-C", worktree, ...args);
      const mainContent = "# Native Git main\n";
      await writeFile(join(worktree, "README.md"), mainContent);
      await localGit("add", "README.md");
      await localGit("commit", "-m", "Native Git initial commit");
      const mainOid = await localGit("rev-parse", "HEAD");
      await assert.rejects(git(readToken, "-C", worktree, "push", projectRemote, "main"), { code: 128 });
      assert.equal(await git(null, "ls-remote", projectRemote), "");
      await git(writeToken, "-C", worktree, "push", projectRemote, "main");
      assert.deepEqual((await git(null, "ls-remote", "--symref", projectRemote, "HEAD")).split(/\r?\n/), [
        "ref: refs/heads/main\tHEAD", `${mainOid}\tHEAD`,
      ]);
      const clone = join(temporary, "public-clone");
      await git(null, "clone", projectRemote, clone);
      assert.equal(await readFile(join(clone, "README.md"), "utf8"), mainContent);

      const branch = "feature/notes";
      const branchQuery = `?branch=${encodeURIComponent(branch)}`;
      const folder = "release notes";
      const filename = "version%20notes.txt";
      const textPath = encodeURIComponent(`${folder}/${filename}`);
      const notes = "First line of the native Git release notes.\nSecond line.\n";
      const binary = Buffer.from([0, 10, 255, 13, 127]);
      const tag = "native-release";
      await localGit("checkout", "-b", branch);
      await mkdir(join(worktree, folder));
      await writeFile(join(worktree, folder, filename), notes);
      const figure = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><circle cx="4" cy="4" r="3"/></svg>';
      await writeFile(join(worktree, folder, "figure.svg"), figure);
      await writeFile(join(worktree, "README.md"), `${mainContent}\n![Release figure](release%20notes/figure.svg)\n`);
      await writeFile(join(worktree, "marker.bin"), binary);
      await localGit("add", folder, "marker.bin", "README.md");
      await localGit("commit", "-m", "Add release files", "-m", "Preserve this commit message body.");
      const featureOid = await localGit("rev-parse", "HEAD");
      await localGit("tag", "-a", tag, "-m", "Native Git release");
      const tagOid = await localGit("rev-parse", `refs/tags/${tag}`);
      await git(writeToken, "-C", worktree, "push", projectRemote, branch, tag);
      await git(null, "-C", clone, "fetch", "origin");
      assert.equal(await git(null, "-C", clone, "rev-parse", `refs/remotes/origin/${branch}`), featureOid);

      const opened = await owner.post(`${project}/pulls/new`, {
        title: "Review native Git history", body: "Review release notes.", base_branch: "main", head_branch: branch,
      });
      redirected(opened);
      const comparison = await guest.get(`${opened.location}?file=${textPath}`);
      assert.equal(comparison.status, 200);
      assert.match(comparison.body, /diff-table__added[\s\S]*?<code>\+ First line of the native Git release notes\./);

      const refs = await guest.get(`${project}/refs`);
      assert.equal(refs.status, 200);
      assert.ok(refs.body.includes(`branch=${encodeURIComponent(branch)}`));
      assert.ok(refs.body.includes(tag));
      const tagRow = refs.body.match(new RegExp(`<bdi[^>]*>${tag}</bdi>([\\s\\S]*?)</li>`))?.[1];
      assert.ok(tagRow?.includes(tagOid) && tagRow.includes(featureOid), "the annotated tag shows its object and target");
      const directory = await guest.get(`${project}/src/${encodeURIComponent(folder)}${branchQuery}`);
      assert.equal(directory.status, 200);
      assert.ok(directory.body.includes(`/blob/${textPath}${branchQuery}`), "nested file link preserves path and branch");
      const preview = await guest.get(`${project}/blob/${textPath}${branchQuery}`);
      assert.equal(preview.status, 200);
      assert.match(preview.body, /First line of the native Git release notes\./);
      const raw = await guest.get(`${project}/raw/${textPath}${branchQuery}`);
      assert.equal(raw.status, 200);
      assert.equal(raw.body, notes);
      assert.equal((await guest.get(`${project}/raw/${textPath}`)).status, 404);
      const binaryRaw = await request(`${base}${project}/raw/marker.bin${branchQuery}`);
      assert.equal(binaryRaw.status, 200);
      assert.equal(binaryRaw.headers.get("content-type"), "application/octet-stream");
      assert.deepEqual(Buffer.from(await binaryRaw.arrayBuffer()), binary);
      const snapshot = await guest.get(`${project}${branchQuery}`);
      const figureUrl = snapshot.body.match(/src="([^"]+\/media\/[^"]+)" alt="Release figure"/)?.[1];
      assert.ok(figureUrl, "the native backend renders the README image link");
      assert.equal(new URL(figureUrl, base).searchParams.get("branch"), featureOid);
      const image = await request(new URL(figureUrl, base));
      assert.equal(image.status, 200);
      assert.equal(image.headers.get("content-type"), "image/svg+xml");
      assert.match(image.headers.get("content-security-policy"), /^sandbox(?:;|$)/);
      assert.equal(await image.text(), figure);
      const history = await guest.get(`${project}/commits${branchQuery}`);
      assert.equal(history.status, 200);
      assert.match(history.body, /Add release files[\s\S]*Native Git initial commit/);
      const commit = await guest.get(`${project}/commit/${featureOid}${branchQuery}`);
      assert.equal(commit.status, 200);
      assert.ok(commit.body.includes(`<code>${featureOid}</code>`));
      assert.ok(commit.body.includes(`/commit/${mainOid}?`));
      assert.match(commit.body, /Native Git Fixture/);
      assert.match(commit.body, /Add release files\n\nPreserve this commit message body\./);
      const rootCommit = await guest.get(`${project}/commit/${mainOid}`);
      assert.equal(rootCommit.status, 200);
      assert.match(rootCommit.body, /Root commit \(no parents\)/);

      const privateContent = "Private vault fixture.\n";
      await localGit("checkout", "-b", "private-fixture", "main");
      await writeFile(join(worktree, "private.txt"), privateContent);
      await localGit("add", "private.txt");
      await localGit("commit", "-m", "Add private file");
      const privateOid = await localGit("rev-parse", "HEAD");
      await git(writeToken, "-C", worktree, "push", vaultRemote, "HEAD:refs/heads/main");
      assert.equal((await guest.get(`${vault}/raw/private.txt`)).status, 404);
      const collaborators = `${vault}/settings/collaborators`;
      redirected(await owner.post(collaborators, { username: "nativeviewer", role: "read" }));
      const renamed = "/nativeowner/renamed-vault";
      redirected(await owner.post(`${vault}/settings/lifecycle`, { action: "rename", name: "renamed-vault" }));
      assert.equal((await guest.get(renamed)).status, 404);
      const privateRaw = await viewer.get(`${vault}/raw/private.txt`);
      assert.equal(privateRaw.status, 200);
      assert.equal(privateRaw.body, privateContent);
      assert.equal((await guest.get(infoRefs(vault, "receive"), viewerToken)).status, 401);
      const privateClone = join(temporary, "private-clone");
      await git(viewerToken, "clone", vaultRemote, privateClone);
      assert.equal(await readFile(join(privateClone, "private.txt"), "utf8"), privateContent);
      redirected(await owner.post(`${renamed}/settings/collaborators`, { username: "nativeviewer", role: "write" }));
      await git(viewerToken, "-C", privateClone, "push", vaultRemote, "HEAD:refs/heads/from/viewer");
      assert.equal(await git(viewerToken, "ls-remote", remoteUrl(base, renamed), "refs/heads/from/viewer"),
        `${privateOid}\trefs/heads/from/viewer`);
      redirected(await owner.post(`${renamed}/settings/collaborators/revoke`, { username: "nativeviewer" }));
      for (const path of [vault, renamed]) {
        assert.equal((await viewer.get(path)).status, 404);
        assert.equal((await guest.get(infoRefs(path, "upload"), viewerToken)).status, 404);
      }
      const canRunGitLfs = await exec("git", ["lfs", "version"]).then(() => true, () => false);
      await exerciseLfs({ base, repo: renamed, writeToken, readToken, canRunGitLfs });

      const fork = "/nativeviewer/project-fork";
      let copied = await viewer.post(`${project}/fork`, { name: "project-fork" });
      redirected(copied);
      if (copied.location !== fork) {
        assert.match(copied.location, /^\/user\/settings\/repository-copies\/\d+$/);
        const statusPath = copied.location;
        copied = await viewer.get(statusPath);
        for (let attempt = 0; copied.status === 202 && attempt < 4; attempt++) {
          await delay(5_000);
          copied = await viewer.post(statusPath, {});
        }
      }
      redirected(copied, fork);
      assert.equal((await guest.get(fork)).status, 200);
      const originalRefs = await git(null, "ls-remote", projectRemote);
      assert.equal(await git(null, "ls-remote", remoteUrl(base, fork)), originalRefs);

      redirected(await owner.post(`${project}/settings/delete`, { confirm: "project" }), "/");
      assert.equal((await guest.get(project)).status, 404);
      assert.equal((await guest.get(infoRefs(project, "upload"))).status, 404);
      const forkClone = join(temporary, "fork-clone");
      await git(null, "clone", "--branch", branch, remoteUrl(base, fork), forkClone);
      assert.equal(await git(null, "-C", forkClone, "rev-parse", "HEAD"), featureOid);
      assert.equal(await readFile(join(forkClone, folder, filename), "utf8"), notes);
      assert.deepEqual(await readFile(join(forkClone, "marker.bin")), binary);
    });
  } finally {
    try {
      await worker?.stop();
    } finally {
      try {
        if (origin?.listening) {
          const closed = once(origin, "close");
          origin.close();
          origin.closeAllConnections();
          await closed;
        }
      } finally {
        await rm(temporary, { recursive: true, force: true, maxRetries: 5 });
      }
    }
  }
});
