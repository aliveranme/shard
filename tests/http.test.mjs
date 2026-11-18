import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash, createHmac, pbkdf2Sync } from "node:crypto";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";
import { unstable_dev } from "wrangler";

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const wrangler = join(root, "node_modules/wrangler/bin/wrangler.js");
const password = "local-regression-password";
const expiredSecret = "shard_expired_regression_credential";
const expiredSession = "expired-regression-session";
const legacySecret = "legacy-regression-credential";
const sha = (value) => createHash("sha256").update(value).digest("hex");

// Each run owns its database, Artifacts namespace state and temporary config.
// No test writes to .wrangler/state or contacts a deployed Worker.
test("forge HTTP regression scenarios", { timeout: 240_000 }, async (t) => {
  const temporary = await mkdtemp(join(tmpdir(), "shard-http-"));
  const persistence = join(temporary, "state");
  const configPath = join(temporary, "wrangler.json");
  const webhookSecret = "local-regression-webhook-signing-key";
  const deliveries = [];
  const receiver = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    deliveries.push({ id: req.headers["x-shard-event-id"], signature: req.headers["x-shard-signature"],
      method: req.method, path: req.url, body: Buffer.concat(chunks) });
    res.writeHead(deliveries.length === 1 ? 503 : 204).end();
  });
  let worker;
  try {
    receiver.listen({ host: "127.0.0.1", port: 0 });
    await once(receiver, "listening");
    const config = JSON.parse((await readFile(join(root, "wrangler.jsonc"), "utf8"))
      .replace(/^\s*\/\/.*$/gm, ""));
    delete config.$schema;
    delete config.build;
    config.name = "shard-http-tests";
    config.main = resolve(root, config.main);
    config.assets.directory = resolve(root, config.assets.directory);
    config.artifacts = config.artifacts.map((binding) => ({ ...binding, remote: false }));
    config.send_email = [{ name: "EMAIL", remote: false }];
    config.vars = {
      ...config.vars, PUBLIC_ORIGIN: "https://forge.example.test", EMAIL_FROM: "shard@example.test",
      WEBHOOK_LOCAL_HTTP: "1", WEBHOOK_SECRET: webhookSecret, WEBHOOK_REPO: "webhook-regression-repo",
      WEBHOOK_ENDPOINT: `http://127.0.0.1:${receiver.address().port}/hooks/shard`,
    };
    config.d1_databases = config.d1_databases.map((binding) => ({
      ...binding, database_name: `${binding.database_name}-http-tests`,
      remote: false, migrations_dir: resolve(root, binding.migrations_dir),
    }));
    await writeFile(configPath, JSON.stringify(config));
    const cli = async (args) => {
      const { stdout } = await exec(process.execPath, [wrangler, ...args,
        "--config", configPath], {
        cwd: root,
        env: { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" },
        timeout: 60_000,
        maxBuffer: 4 * 1024 * 1024,
      });
      return stdout;
    };
    await cli(["d1", "migrations", "apply", "DB", "--local", "--persist-to", persistence]);
    const salt = "1f".repeat(16);
    const hash = `pbkdf2_sha256$100000$${salt}$${pbkdf2Sync(password, Buffer.from(salt, "hex"), 100_000, 32, "sha256").toString("hex")}`;
    const now = Math.floor(Date.now() / 1000);
    const fixture = join(temporary, "expired.sql");
    await writeFile(fixture, `
      INSERT INTO users (username,email,password_hash,is_admin,created_at)
        VALUES ('expiredfixture','expired@example.test','${hash}',0,${now});
      INSERT INTO access_tokens (user_id,name,token_hash,scope,created_at,expires_at)
        SELECT id,'expired fixture','${sha(expiredSecret)}','read',${now - 100},${now - 1}
        FROM users WHERE username='expiredfixture';
      INSERT INTO access_tokens (user_id,name,token_hash,scope,created_at,expires_at)
        SELECT id,'legacy fixture','${sha(legacySecret)}','read',${now - 100},NULL
        FROM users WHERE username='expiredfixture';
      INSERT INTO sessions (token_hash,user_id,created_at,expires_at)
        SELECT '${sha(expiredSession)}',id,${now - 100},${now - 1}
        FROM users WHERE username='expiredfixture';
      INSERT INTO repos (owner_id, name, artifacts_repo, visibility, created_at, updated_at)
        SELECT id,'webhook-fixture','webhook-regression-repo','private',${now},${now}
        FROM users WHERE username='expiredfixture';
    `);
    await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence, "--file", fixture]);
    worker = await unstable_dev(config.main, {
      config: configPath, local: true, ip: "127.0.0.1", port: 0,
      persist: true, persistTo: persistence, logLevel: "error",
      experimental: {
        disableExperimentalWarning: true, disableDevRegistry: true,
        forceLocal: true, testScheduled: true, watch: false,
        showInteractiveDevSession: false,
      },
    });
    const base = `http://127.0.0.1:${worker.port}`;
    let clientNumber = 0;

    class Client {
      source = `2001:db8::${(++clientNumber).toString(16)}`;
      cookies = new Map();
      csrf = "";
      async request(path, options = {}) {
        const headers = new Headers(options.headers);
        if (!headers.has("CF-Connecting-IP")) headers.set("CF-Connecting-IP", this.source);
        if (this.cookies.size) headers.set("Cookie", [...this.cookies]
          .map(([name, value]) => `${name}=${value}`).join("; "));
        const response = await fetch(new URL(path, base), {
          ...options, headers, redirect: "manual",
        });
        for (const cookie of response.headers.getSetCookie()) {
          const pair = cookie.split(";", 1)[0];
          const at = pair.indexOf("=");
          if (pair.slice(at + 1)) this.cookies.set(pair.slice(0, at), pair.slice(at + 1));
          else this.cookies.delete(pair.slice(0, at));
        }
        const body = await response.text();
        assert.ok(response.status < 500, `${path}: HTTP ${response.status}\n${body.slice(-1400)}`);
        const csrf = body.match(/name="csrf_token" value="([a-f0-9]{64})"/);
        if (csrf) this.csrf = csrf[1];
        return { status: response.status, headers: response.headers, body,
          location: response.headers.get("location") };
      }
      get(path) { return this.request(path); }
      post(path, fields, { csrf = true, headers = {} } = {}) {
        return this.request(path, {
          method: "POST",
          headers: { Origin: base, ...headers },
          body: new URLSearchParams({ ...(csrf ? { csrf_token: this.csrf } : {}), ...fields }),
        });
      }
    }
    const redirected = (response) => assert.equal(response.status, 302, response.body.slice(0, 500));
    async function signup(name, email = `${name}@example.test`) {
      const client = new Client();
      await client.get("/user/sign_up");
      redirected(await client.post("/user/sign_up", { username: name, email, password }));
      await client.get("/user/settings");
      return client;
    }
    async function login(name, secret = password) {
      const client = new Client();
      await client.get("/user/login");
      const result = await client.post("/user/login", { login: name, password: secret });
      return { client, result };
    }
    function pageLink(html, parameter, value) {
      for (const match of html.matchAll(/<a\b[^>]*href="([^"]+)"/g)) {
        const href = match[1].replace(/&(?:amp|#38|#x26);/gi, "&");
        const url = new URL(href, base);
        if (url.searchParams.get(parameter) === String(value)) return url;
      }
      assert.fail(`Missing ${parameter}=${value} navigation`);
    }
    const owner = await signup("suiteowner");
    const viewer = await signup("suiteviewer");
    const publicRepo = "/suiteowner/project";
    const privateRepo = "/suiteowner/private-vault";
    redirected(await owner.post("/repo/create", { name: "project", description: "HTTP regression repository" }));
    redirected(await owner.post("/repo/create", { name: "private-vault", private: "on" }));

    await t.test("browser forms enforce CSRF and same-origin requests", async () => {
      const anonymous = new Client();
      const page = await anonymous.get("/user/login");
      assert.match(page.headers.get("content-security-policy"), /script-src 'none'/);
      assert.equal(page.headers.get("x-content-type-options"), "nosniff");
      assert.equal(page.headers.get("cache-control"), "no-store");
      assert.match(page.headers.get("set-cookie"), /HttpOnly; SameSite=Lax/);
      const fields = { login: "suiteowner", password };
      assert.equal((await anonymous.post("/user/login", fields, { csrf: false })).status, 403);
      assert.equal((await anonymous.post("/user/login", { ...fields, csrf_token: "0".repeat(64) })).status, 403);
      assert.equal((await anonymous.post("/user/login", fields, { headers: { Origin: "https://elsewhere.example" } })).status, 403);
      assert.equal((await anonymous.post("/user/login", fields, { headers: { "Sec-Fetch-Site": "cross-site" } })).status, 403);
      assert.equal((await viewer.post("/user/settings/email", {
        email: "stolen@example.test", current_password: password, csrf_token: owner.csrf,
      })).status, 403);
      redirected(await anonymous.post("/user/login", fields));
    });

    await t.test("redirect targets cannot escape the forge through backslashes", async () => {
      const anonymous = new Client();
      await anonymous.get("/user/login");
      const result = await anonymous.post("/user/login", {
        login: "suiteowner", password, next: "/\\outside.example",
      });
      redirected(result);
      assert.equal(result.location, "/");
    });

    await t.test("guest language follows browser priorities and persists an explicit choice", async () => {
      const guest = new Client();
      const path = "/user/settings/language";
      const preferred = "fr-CA, zh-CN;q=0.9, en-GB;q=0.5";
      const settings = await guest.request(path, { headers: { "Accept-Language": preferred } });
      assert.equal(settings.headers.get("content-language"), "zh-Hans");
      assert.equal(settings.headers.get("cache-control"), "no-store");
      assert.match(settings.body, /<html lang="zh-Hans">[\s\S]*界面语言/);
      const unsupported = await guest.request(path, { headers: { "Accept-Language": "zh-Hant, en;q=0.5" } });
      assert.equal(unsupported.headers.get("content-language"), "en");
      const wildcard = await guest.request(path, { headers: { "Accept-Language": "en;q=0, *;q=0.5" } });
      assert.equal(wildcard.headers.get("content-language"), "zh-Hans");

      const missingToken = await guest.post(path, { language: "zh-Hans" }, {
        csrf: false, headers: { "Accept-Language": preferred },
      });
      assert.equal(missingToken.status, 403);
      assert.match(missingToken.body, /此表单已过期/);
      const invalid = await guest.post(path, { language: "unsupported" });
      assert.equal(invalid.status, 400);
      assert.doesNotMatch(invalid.headers.getSetCookie().join("\n"), /shard_lang=/);

      const saved = await guest.post(path, { language: "zh-Hans" });
      redirected(saved);
      assert.match(saved.headers.get("set-cookie"), /shard_lang=zh-Hans; Path=\/; HttpOnly; SameSite=Lax/);
      assert.match((await guest.get(saved.location)).body, /语言偏好已保存/);
      const explore = await guest.request("/explore/repos?q=project&page=2", {
        headers: { "Accept-Language": "en" },
      });
      assert.match(explore.body, /探索仓库[\s\S]*上一页[\s\S]*第 2 页/);
      const missing = await guest.get("/this/is/a/missing/page");
      assert.equal(missing.status, 404);
      assert.match(missing.body, /找不到此页面/);

      redirected(await guest.post(path, { language: "auto" }));
      assert.equal((await guest.request("/", { headers: { "Accept-Language": "en" } }))
        .headers.get("content-language"), "en");
    });

    await t.test("an account language overrides a conflicting browser across sessions", async () => {
      const path = "/user/settings/language";
      const account = await signup("localeaccount");
      redirected(await account.post(path, { language: "zh-Hans" }));
      const otherBrowser = new Client();
      await otherBrowser.get(path);
      redirected(await otherBrowser.post(path, { language: "en" }));
      await otherBrowser.get("/user/login");
      redirected(await otherBrowser.post("/user/login", { login: "localeaccount", password }));

      const accessTokens = await otherBrowser.get("/user/settings/tokens");
      assert.equal(accessTokens.headers.get("content-language"), "zh-Hans");
      assert.match(accessTokens.body, /<h1 class="page-title">访问令牌<\/h1>/);
      const repository = await otherBrowser.get(publicRepo);
      assert.match(repository.body, /此仓库为空/);
      assert.match(repository.body, /HTTP regression repository/);

      redirected(await account.post(path, { language: "auto" }));
      const browserChoice = await otherBrowser.request("/user/settings", {
        headers: { "Accept-Language": "zh-Hans" },
      });
      assert.equal(browserChoice.headers.get("content-language"), "zh-Hans");
      redirected(await otherBrowser.post("/user/logout", {}));
      const anonymousChoice = await otherBrowser.request("/", {
        headers: { "Accept-Language": "zh-Hans" },
      });
      assert.equal(anonymousChoice.headers.get("content-language"), "en");
    });

    await t.test("private repositories and Git credentials keep their access boundary", async () => {
      const anonymous = new Client();
      assert.equal((await anonymous.get(privateRepo)).status, 404);
      assert.equal((await viewer.get(privateRepo)).status, 404);
      assert.equal((await owner.get(privateRepo)).status, 200);
      const headers = { "Accept-Language": "zh-CN" };
      const missing = await anonymous.request("/suiteowner/missing.git/info/refs?service=git-upload-pack", { headers });
      assert.equal(missing.status, 404);
      assert.equal(missing.body, "");
      assert.equal(missing.headers.get("cache-control"), "no-store");
      const withoutSuffix = await anonymous.request(`${publicRepo}/info/refs?service=git-upload-pack`, { headers });
      assert.equal(withoutSuffix.status, 404);
      assert.equal(withoutSuffix.headers.get("content-language"), "zh-Hans");
      assert.match(withoutSuffix.body, /Git 远程地址须以 \.git 结尾/);
      assert.equal((await anonymous.request(`${privateRepo}.git/git-receive-pack`, {
        method: "POST", headers: { "Content-Type": "application/x-git-receive-pack-request" }, body: "0000",
      })).status, 401);
      assert.equal((await anonymous.post(`${publicRepo}/git-upload-pack`, {}, { csrf: false })).status, 403);
    });

    await t.test("token creation validates permissions and only reveals a secret once", async () => {
      await owner.get("/user/settings/tokens");
      const invalid = await owner.post("/user/settings/tokens", { name: "invalid", scope: "admin", expires_in: "30" });
      assert.notEqual(invalid.status, 302);
      assert.doesNotMatch(invalid.body, /shard_[a-f0-9]{64}/);
      const created = await owner.post("/user/settings/tokens", { name: "regression reader", scope: "read", expires_in: "30" });
      assert.equal(created.status, 200);
      const secret = created.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
      assert.ok(secret, "new token must be shown once");
      const listed = await owner.get("/user/settings/tokens?token=should-never-be-echoed");
      assert.doesNotMatch(listed.body, /should-never-be-echoed/);
      assert.ok(!listed.body.includes(secret));
      assert.match(listed.body, /regression reader/);
      const id = listed.body.match(/\/user\/settings\/tokens\/(\d+)\/delete/)?.[1];
      assert.ok(id);
      const headers = { Authorization: `Bearer ${secret}` };
      assert.equal((await owner.request(`${privateRepo}.git/info/refs?service=git-upload-pack`, { headers })).status, 200);
      assert.equal((await owner.request(`${privateRepo}.git/info/refs?service=git-receive-pack`, { headers })).status, 401);
      redirected(await owner.post(`/user/settings/tokens/${id}/delete`, {}));
      assert.equal((await owner.request(`${privateRepo}.git/info/refs?service=git-upload-pack`, { headers })).status, 401);
    });

    await t.test("expired Git tokens and browser sessions are rejected", async () => {
      const fixtureLogin = await login("expiredfixture");
      redirected(fixtureLogin.result);
      await fixtureLogin.client.get("/new");
      redirected(await fixtureLogin.client.post("/repo/create", { name: "private", private: "on" }));
      const response = await new Client().request("/expiredfixture/private.git/info/refs?service=git-upload-pack", {
        headers: { Authorization: `Bearer ${expiredSecret}` },
      });
      assert.equal(response.status, 401);
      const expired = new Client();
      expired.cookies.set("shard_session", expiredSession);
      const account = await expired.get("/user/settings");
      redirected(account);
      assert.match(account.location, /^\/user\/login/);
    });

    await t.test("legacy tokens remain valid and commit history paginates across slash branches", async () => {
      assert.equal((await new Client().request("/expiredfixture/private.git/info/refs?service=git-upload-pack", {
        headers: { Authorization: `Bearer ${legacySecret}` },
      })).status, 200);
      await owner.get("/user/settings/tokens");
      const created = await owner.post("/user/settings/tokens", { name: "history writer", scope: "write", expires_in: "7" });
      const secret = created.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
      assert.ok(secret);
      redirected(await owner.post("/repo/create", { name: "history" }));
      const worktree = join(temporary, "history-worktree");
      const gitEnv = {
        ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_CONFIG_COUNT: "1",
        GIT_CONFIG_KEY_0: "http.extraHeader", GIT_CONFIG_VALUE_0: `Authorization: Bearer ${secret}`,
      };
      const git = async (...args) => exec("git", args, { cwd: root, env: gitEnv, timeout: 30_000 });
      await git("init", "--initial-branch=main", worktree);
      await git("-C", worktree, "config", "user.name", "HTTP regression");
      await git("-C", worktree, "config", "user.email", "git@example.test");
      await git("-C", worktree, "config", "commit.gpgsign", "false");
      for (let i = 0; i < 21; i++) {
        await writeFile(join(worktree, "README.md"), `# History ${i}\n`);
        await git("-C", worktree, "add", "README.md");
        await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", `history-${String(i).padStart(2, "0")}`);
      }
      await git("-C", worktree, "checkout", "-b", "feature/readme");
      await writeFile(join(worktree, "README.md"), "# Updated branch\n<div>literal text</div>");
      await writeFile(join(worktree, "IMAGE.bin"), Buffer.from([0, 1, 2]));
      await writeFile(join(worktree, "LARGE.txt"), "long fixture line\n".repeat(513));
      await writeFile(join(worktree, "FEATURE.md"), "# A branch with a slash\n");
      const folder = join(worktree, "docs and examples");
      await mkdir(folder);
      await writeFile(join(folder, "release%20notes.txt"), "A nested file from the slash branch.\n");
      await git("-C", worktree, "add", "FEATURE.md", "README.md", "IMAGE.bin", "LARGE.txt", "docs and examples");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "feature note");
      await git("-C", worktree, "remote", "add", "origin", `${base}/suiteowner/history.git`);
      await git("-C", worktree, "push", "origin", "main", "feature/readme");
      const rootCommit = (await git("-C", worktree, "rev-list", "--max-parents=0", "main")).stdout.trim();
      assert.match((await owner.get(`/suiteowner/history/commit/${rootCommit}`)).body, /Root commit \(no parents\)/);
      const featureCommit = (await git("-C", worktree, "rev-parse", "feature/readme")).stdout.trim();
      const download = await fetch(`${base}/suiteowner/history/archive/${featureCommit}`);
      assert.equal(download.status, 200);
      assert.equal(download.headers.get("content-type"), "application/x-tar");
      const tar = Buffer.from(await download.arrayBuffer());
      assert.equal(tar.toString("ascii", 257, 262), "ustar");
      const members = new Map();
      for (let at = 0; at + 512 < tar.length && tar[at] !== 0;) {
        const string = (start, end) => tar.toString("utf8", at + start, at + end).split("\0", 1)[0];
        const path = [string(345, 500), string(0, 100)].filter(Boolean).join("/");
        const size = Number.parseInt(string(124, 136), 8);
        members.set(path, tar.subarray(at + 512, at + 512 + size));
        at += 512 + Math.ceil(size / 512) * 512;
      }
      const prefix = `source-${featureCommit}/`;
      assert.deepEqual(members.get(`${prefix}IMAGE.bin`), Buffer.from([0, 1, 2]));
      assert.equal(members.get(`${prefix}docs and examples/release%20notes.txt`)?.toString(), "A nested file from the slash branch.\n");
      const history = "/suiteowner/history";
      const tag = "review-tag";
      await git("-C", worktree, "tag", "-a", tag, "-m", "Annotated tag");
      await git("-C", worktree, "push", "origin", tag);
      const refs = await owner.get(`${history}/refs`);
      assert.match(refs.body, /\?branch=feature%2Freadme"/);
      for (const ref of [tag, `${tag}^{}`]) {
        const oid = (await git("-C", worktree, "rev-parse", ref)).stdout.trim();
        assert.ok(refs.body.includes(oid));
      }
      assert.equal((await new Client().get(`${privateRepo}/refs`)).status, 404);

      const settingsPath = `${history}/settings`;
      const gitHead = async () => (await git("-C", worktree, "ls-remote", "--symref", "origin", "HEAD")).stdout;
      const remoteHead = await gitHead();
      assert.match(remoteHead, /ref: refs\/heads\/main\s+HEAD/);
      for (const invalid of [tag, "feature/missing", "main~1"]) {
        const rejected = await owner.post(settingsPath, {
          default_branch: invalid, description: "Must not be saved", visibility: "private",
        });
        redirected(rejected);
        const page = await owner.get(rejected.location);
        assert.match(page.body, /Push the branch to this repository/);
        assert.doesNotMatch(page.body, /Must not be saved/);
      }
      assert.equal((await new Client().get(history)).status, 200);
      const saved = await owner.post(settingsPath, { default_branch: "feature/readme", visibility: "public" });
      redirected(saved);
      assert.match((await owner.get(saved.location)).body, /Git clones follow the remote HEAD/);
      assert.match((await owner.get(history)).body, /href="\/suiteowner\/history\/blob\/FEATURE.md\?branch=feature%2Freadme"/);
      assert.equal(await gitHead(), remoteHead);
      redirected(await owner.post(settingsPath, { default_branch: "main", visibility: "public" }));

      const branchPage = await owner.get("/suiteowner/history?branch=feature%2Freadme");
      assert.match(branchPage.body, /<option value="feature\/readme"\s+selected>/);
      assert.match(branchPage.body, /<button[^>]*type="submit">Browse branch<\/button>/);
      assert.match(branchPage.body, /href="\/suiteowner\/history\/blob\/FEATURE.md\?branch=feature%2Freadme"/);
      const treeLink = branchPage.body.match(/href="([^"\n]*\/src\/docs%20and%20examples\?branch=[^"\n]+)"/)?.[1];
      assert.ok(treeLink, "branch page links to the encoded directory");
      const tree = await owner.get(treeLink);
      assert.equal(tree.status, 200);
      const blobLink = tree.body.match(/href="([^"\n]*\/blob\/[^"\n]+)"/)?.[1];
      assert.ok(blobLink, "directory links to the nested file");
      const blob = await owner.get(blobLink);
      assert.equal(blob.status, 200);
      assert.match(blob.body, /A nested file from the slash branch\./);
      const rawLink = blob.body.match(/href="([^"\n]*\/raw\/[^"\n]+)"/)?.[1];
      assert.ok(rawLink, "file preview links to the raw download");
      const raw = await owner.get(rawLink);
      assert.equal(raw.status, 200);
      assert.equal(raw.headers.get("content-type"), "application/octet-stream");
      assert.equal(raw.headers.get("content-disposition"), "inline; filename*=UTF-8''release%2520notes.txt");
      assert.equal(raw.body, "A nested file from the slash branch.\n");
      const first = await owner.get("/suiteowner/history/commits?branch=feature%2Freadme");
      assert.equal(first.status, 200);
      const next = [...first.body.matchAll(/<a\b[^>]*href="([^"]+)"/g)]
        .map((match) => new URL(match[1].replace(/&(?:amp|#38|#x26);/gi, "&"), base))
        .find((url) => url.pathname === "/suiteowner/history/commits"
          && [...url.searchParams.keys()].some((key) => key !== "branch"));
      assert.ok(next, "commit history must have real next-page navigation");
      assert.equal(next.searchParams.get("branch"), "feature/readme");
      const second = await owner.get(next);
      assert.equal(second.status, 200);
      const messages = (body) => new Set([...body.matchAll(/history-\d+/g)].map((match) => match[0]));
      const firstMessages = messages(first.body);
      const secondMessages = messages(second.body);
      assert.equal(new Set([...firstMessages, ...secondMessages]).size, 21);
      assert.ok([...firstMessages].every((message) => !secondMessages.has(message)));
      assert.match(first.body, /feature note/);
      const tagged = await owner.post(`${history}/pulls/new`, {
        title: "A tag is not a branch", base_branch: "main", head_branch: tag,
      });
      assert.notEqual(tagged.status, 302);

      await git("-C", worktree, "checkout", "main");
      await writeFile(join(worktree, "TARGET-ONLY.txt"), "A change only on the target branch.\n");
      await git("-C", worktree, "add", "TARGET-ONLY.txt");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "Advance only the target");
      await git("-C", worktree, "push", "origin", "main");
      const common = (await git("-C", worktree, "merge-base", "main", "feature/readme")).stdout.trim();
      const initialTarget = (await git("-C", worktree, "rev-parse", "main")).stdout.trim();
      await git("-C", worktree, "checkout", "feature/readme");

      const createdPull = await viewer.post(`${history}/pulls/new`, {
        title: "Review a slash branch", body: "Review this feature.", base_branch: "main", head_branch: "feature/readme",
      });
      redirected(createdPull);
      const pullUrl = createdPull.location;
      const pull = await owner.get(pullUrl);
      assert.match(pull.body, /\?file=FEATURE.md#file-diff/);
      assert.match(pull.body, /\?file=docs%20and%20examples%2Frelease%2520notes.txt#file-diff/);
      assert.doesNotMatch(pull.body, /TARGET-ONLY\.txt/);
      assert.match(pull.body, new RegExp(`Base commit</span><code[^>]*>${initialTarget}</code>`));
      assert.match(pull.body, new RegExp(`Common ancestor</span><code[^>]*>${common}</code>`));
      const readme = await owner.get(`${pullUrl}?file=README.md`);
      assert.match(readme.body, /diff-table__deleted[\s\S]*# History 20/);
      assert.match(readme.body, /diff-table__added[\s\S]*# Updated branch/);
      assert.doesNotMatch(readme.body, /<div>literal text<\/div>/);
      assert.match(readme.body, /(?:&lt;|&#60;|&#x3c;)div(?:&gt;|&#62;|&#x3e;)literal text/i);
      assert.match(readme.body, /No newline at end of file/);
      assert.match((await owner.get(`${pullUrl}?file=IMAGE.bin`)).body, /binary or non-UTF-8/);
      assert.match((await owner.get(`${pullUrl}?file=LARGE.txt`)).body, /exceeds the line diff size limit/);

      const comparison = (body) => body.match(/name="comparison_version" value="(\d+)"/)?.[1];
      const fields = { comparison_version: comparison(readme.body) };
      assert.equal(fields.comparison_version, "1");
      const updateUrl = `${pullUrl}/comparison`;
      const reviews = `${pullUrl}/reviews`;
      assert.equal((await viewer.post(reviews, { ...fields, decision: "approved" })).status, 403);
      assert.equal((await owner.post(reviews, { ...fields, comparison_version: "0", decision: "approved" })).status, 409);
      assert.equal((await owner.post(reviews, { ...fields, decision: "merge_verified" })).status, 409);
      const decision = await owner.post(reviews, { ...fields, decision: "approved", body: "Approval for the saved pair." });
      redirected(decision);
      assert.match((await owner.get(decision.location)).body, /Approved this comparison[\s\S]*Approval for the saved pair/);
      const lineComment = { ...fields, decision: "line_comment", path: "README.md", line: "new:2", body: "Comment for the exact line." };
      assert.equal((await viewer.post(reviews, { ...lineComment, path: "TARGET-ONLY.txt", line: "old:1" })).status, 400);
      assert.equal((await viewer.post(reviews, { ...lineComment, line: "new:900" })).status, 400);
      const comment = await viewer.post(reviews, lineComment);
      redirected(comment);
      const review = await viewer.get(comment.location);
      const historicalLink = pageLink(review.body, "file", "README.md");
      assert.equal(historicalLink.hash, "#diff-new-2");
      assert.match(review.body, /Comment for the exact line/);

      await writeFile(join(worktree, "LATER.txt"), "Changed after review started.\n");
      await writeFile(join(worktree, "README.md"), "# Updated after refresh\nA different line after refresh.\n");
      await git("-C", worktree, "add", "LATER.txt", "README.md");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "Advance the head");
      await git("-C", worktree, "push", "origin", "feature/readme");
      const updatedHead = (await git("-C", worktree, "rev-parse", "feature/readme")).stdout.trim();
      await git("-C", worktree, "checkout", "main");
      await writeFile(join(worktree, "README.md"), "# Moved base\n");
      await git("-C", worktree, "add", "README.md");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "Advance the base");
      await git("-C", worktree, "push", "origin", "main");
      const frozen = await owner.get(`${pullUrl}?file=README.md`);
      assert.match(frozen.body, /The source or target branch has moved/);
      assert.doesNotMatch(frozen.body, /LATER\.txt/);
      assert.match(frozen.body, /# History 20/);
      assert.doesNotMatch(frozen.body, /# Moved base/);
      assert.equal(comparison(frozen.body), fields.comparison_version);
      const observer = await signup("reviewobserver");
      assert.doesNotMatch((await observer.get(pullUrl)).body, /Update comparison/);
      assert.equal((await observer.post(updateUrl, fields)).status, 403);
      assert.equal((await viewer.post(updateUrl, fields, { csrf: false })).status, 403);

      const updates = await Promise.all([viewer.post(updateUrl, fields), viewer.post(updateUrl, fields)]);
      assert.deepEqual(updates.map(({ status }) => status).sort(), [302, 409]);
      assert.equal(updates.find(({ status }) => status === 302).location, `${pullUrl}#pull-comparison`);
      const current = await owner.get(`${pullUrl}?file=README.md`);
      const currentFields = { comparison_version: comparison(current.body) };
      assert.equal(currentFields.comparison_version, "2");
      assert.match(current.body, /LATER\.txt/);
      assert.match(current.body, /# History 20/);
      assert.match(current.body, /# Updated after refresh/);
      assert.doesNotMatch(current.body, /# Moved base|TARGET-ONLY\.txt/);
      assert.equal((await owner.post(reviews, { ...fields, decision: "approved" })).status, 409);
      assert.equal((await viewer.post(reviews, lineComment)).status, 409);

      const historical = await owner.get(new URL(`${pullUrl}${historicalLink.search}`, base));
      assert.match(historical.body, /Review comparison/);
      assert.match(historical.body, /Comparison 1/);
      assert.match(historical.body, /id="diff-new-2"[\s\S]*literal text/);
      assert.doesNotMatch(historical.body, /# Updated after refresh|LATER\.txt|name="comparison_version"/);
      assert.match(historical.body, /file=FEATURE\.md#file-diff/);
      const otherFile = pageLink(historical.body, "file", "FEATURE.md");
      assert.equal(otherFile.searchParams.get("review"), historicalLink.searchParams.get("review"));
      assert.match(historical.body, /View current comparison/);
      assert.equal((await owner.get(`${pullUrl}?review=9007199254740992`)).status, 404);

      assert.equal((await owner.post(reviews, { ...currentFields, decision: "merge_verified" })).status, 409);
      // Integrate the current source as a non-first parent.
      await git("-C", worktree, "merge", "--no-ff", "-X", "theirs", updatedHead, "-m", "Integrate the reviewed branch");
      await git("-C", worktree, "push", "origin", "main");
      const integrated = await owner.post(updateUrl, currentFields);
      assert.equal(integrated.status, 409);
      assert.match(integrated.body, /The target already contains the latest source commit/);
      const merged = await owner.post(reviews, { ...currentFields, decision: "merge_verified" });
      redirected(merged);
      assert.match((await owner.get(merged.location)).body, /Verified integration in Git/);
      const mergeTip = (await git("-C", worktree, "rev-parse", "main")).stdout.trim();
      assert.match((await owner.get(`${history}/commit/${mergeTip}`)).body,
        new RegExp(`/commit/${updatedHead}\\?branch=main`));
      assert.equal((await owner.post(`${pullUrl}/state`, { state: "open" })).status, 409);
      await git("-C", worktree, "checkout", "-b", "feature/deep");
      const deep = join(worktree, ...Array.from({ length: 14 }, (_, index) => `d${index}`));
      await mkdir(deep, { recursive: true });
      await writeFile(join(deep, "deep.txt"), "past the tree depth limit\n");
      await git("-C", worktree, "add", "d0");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "Deep comparison");
      await git("-C", worktree, "push", "origin", "feature/deep");
      const deepPull = await owner.post(`${history}/pulls/new`, {
        title: "Deep comparison", base_branch: "main", head_branch: "feature/deep",
      });
      redirected(deepPull);
      const limited = await owner.get(deepPull.location);
      assert.match(limited.body, /exceeds the file or tree limits/);
      assert.doesNotMatch(limited.body, /No file changes/);
      const empty = await owner.post("/suiteowner/history/pulls/1/comments", { body: " " });
      redirected(empty);
      assert.match(empty.location, /^\/suiteowner\/history\/pulls\/1/);

      await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence,
        "--command", `UPDATE pulls SET merge_base = '${"f".repeat(40)}'
          WHERE issue_id = (SELECT id FROM issues WHERE title = 'Deep comparison')`]);
      const unavailable = await owner.get(deepPull.location);
      assert.match(unavailable.body, /saved comparison commits are unavailable/);
      assert.doesNotMatch(unavailable.body, /No file changes/);

      const treeId = (await git("-C", worktree, "rev-parse", "HEAD^{tree}")).stdout.trim();
      const commit = async (message, ...parents) => (await git("-C", worktree, "commit-tree", treeId,
        ...parents.flatMap((parent) => ["-p", parent]), "-m", message)).stdout.trim();
      const leftParent = await commit("Left parent", featureCommit);
      const rightParent = await commit("Right parent", featureCommit);
      const leftTip = await commit("Left merge", leftParent, rightParent);
      const rightTip = await commit("Right merge", rightParent, leftParent);
      await git("-C", worktree, "push", "origin", `${leftTip}:refs/heads/compare/left`, `${rightTip}:refs/heads/compare/right`);
      const crisscross = await owner.post(`${history}/pulls/new`, {
        title: "Ambiguous history", base_branch: "compare/left", head_branch: "compare/right",
      });
      assert.notEqual(crisscross.status, 302);
      assert.match(crisscross.body, /more than one best common ancestor/);
    });

    await t.test("private repository grants govern Git, issue classification and notifications", async () => {
      const access = `${privateRepo}/settings/collaborators`;
      const writeToken = async (account, name) => {
        const result = await account.post("/user/settings/tokens", { name, scope: "write" });
        const secret = result.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
        assert.equal(result.status, 200);
        assert.ok(secret);
        return secret;
      };
      const ownerToken = await writeToken(owner, "private repo owner");
      const readerToken = await writeToken(viewer, "private repo collaborator");
      const outsider = await signup("outsidefixture");
      const outsiderToken = await writeToken(outsider, "private repo outsider");
      const privateRef = `${privateRepo}.git/info/refs?service=`;
      const auth = (token) => ({ headers: { Authorization: `Bearer ${token}` } });
      const git = (token, ...args) => exec("git", args, {
        cwd: root, timeout: 30_000,
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_CONFIG_COUNT: "1",
          GIT_CONFIG_KEY_0: "http.extraHeader", GIT_CONFIG_VALUE_0: `Authorization: Bearer ${token}` },
      });
      const remote = `${base}${privateRepo}.git`;
      const seed = join(temporary, "private-seed");
      await git(ownerToken, "init", "--initial-branch=main", seed);
      await writeFile(join(seed, "README.md"), "# Private source\n");
      await git(ownerToken, "-C", seed, "add", "README.md");
      await git(ownerToken, "-C", seed, "-c", "core.hooksPath=", "-c", "user.name=Regression",
        "-c", "user.email=git@example.test", "-c", "commit.gpgsign=false", "commit", "-m", "Private fixture");
      await git(ownerToken, "-C", seed, "push", remote, "main");
      const oid = (await git(ownerToken, "-C", seed, "rev-parse", "HEAD")).stdout.trim();
      assert.equal((await outsider.get(`${privateRepo}/archive/${oid}`)).status, 404);
      const checkPath = `${privateRepo}/commit/${oid}/checks`;
      const checkResult = (client, token, state, summary) => client.request(checkPath, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...(token ? auth(token).headers : {}) },
        body: JSON.stringify({ context: "ci/unit", state, summary }),
      });
      assert.equal((await checkResult(owner, null, "pending")).status, 401);
      assert.equal((await checkResult(outsider, outsiderToken, "pending")).status, 404);

      assert.equal((await outsider.post(access, { username: "suiteviewer", role: "read" })).status, 404);
      redirected(await owner.post(access, { username: "suiteviewer", role: "read" }));
      assert.match((await viewer.get(privateRepo)).body, /Private source/);
      assert.equal((await viewer.get(access)).status, 403);
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 401);
      assert.equal((await checkResult(viewer, readerToken, "pending")).status, 403);
      const clone = join(temporary, "private-clone");
      await git(readerToken, "clone", "--config", "core.autocrlf=false", remote, clone);
      assert.equal(await readFile(join(clone, "README.md"), "utf8"), "# Private source\n");
      redirected(await owner.post(access, { username: "suiteviewer", role: "write" }));
      const readCredential = await viewer.post("/user/settings/tokens", { name: "read-only CI", scope: "read" });
      const readSecret = readCredential.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
      assert.ok(readSecret);
      assert.equal((await checkResult(viewer, readSecret, "pending")).status, 403);
      await git(readerToken, "-C", clone, "push", remote, "HEAD:refs/heads/from/viewer");
      assert.equal((await checkResult(viewer, readerToken, "pending", "First run")).status, 204);
      assert.equal((await checkResult(viewer, readerToken, "success", "Checks complete")).status, 204);
      assert.equal((await checkResult(viewer, readerToken, "success", "Checks complete")).status, 204);
      const checks = await viewer.get(`${privateRepo}/commit/${oid}`);
      assert.equal([...checks.body.matchAll(/>ci\/unit</g)].length, 1);
      assert.match(checks.body, /ci\/unit[\s\S]*Passed[\s\S]*Checks complete/);
      assert.doesNotMatch(checks.body, /First run/);

      const catalog = `${privateRepo}/settings/issues`;
      redirected(await owner.post(catalog, { action: "create_label", name: "review-required" }));
      redirected(await owner.post(catalog, { action: "create_milestone", name: "delivery-window" }));
      assert.equal((await owner.post(catalog, { action: "create_label", name: "REVIEW-REQUIRED" })).status, 409);
      const thread = await viewer.post(`${privateRepo}/issues/new`, { title: "Protected classification", body: "before edit" });
      redirected(thread);
      const edit = `${thread.location}/edit`;
      const form = await viewer.get(edit);
      const label_id = form.body.match(/name="label_id" value="(\d+)"/)?.[1];
      const milestone_id = form.body.match(/<option value="(\d+)"[^>]*>delivery-window/)?.[1];
      const assignee_id = form.body.match(/<option value="(\d+)"[^>]*>suiteviewer/)?.[1];
      assert.ok(label_id && milestone_id && assignee_id);
      const fields = { title: "Protected classification", body: "private-body-needle", label_id, milestone_id, assignee_id, classify: "1" };
      assert.equal((await viewer.post(edit, { ...fields, label_id: "9999999" })).status, 400);
      redirected(await viewer.post(edit, fields));
      const detail = await viewer.get(thread.location);
      assert.match(detail.body, /review-required[\s\S]*delivery-window[\s\S]*href="\/suiteviewer"/);
      assert.match((await viewer.get("/issues?q=private-body-needle")).body, /Protected classification/);
      assert.doesNotMatch((await outsider.get("/issues?q=private-body-needle")).body, /Protected classification/);
      redirected(await owner.post(`${thread.location}/comments`, { body: "private-comment-needle" }));
      assert.match((await viewer.get(`${privateRepo}/issues?q=private-comment-needle`)).body, /Protected classification/);
      const inbox = await viewer.get("/user/notifications");
      const notification = inbox.body.match(/notification-title-(\d+)"[^>]*>Protected classification/)?.[1];
      assert.ok(notification);
      assert.equal((await outsider.post(`/user/notifications/${notification}/read`, {})).status, 404);
      redirected(await viewer.post(`/user/notifications/${notification}/read`, {}));
      assert.doesNotMatch((await viewer.get("/user/notifications")).body, new RegExp(`/user/notifications/${notification}/read"`));

      redirected(await owner.post(`${access}/revoke`, { username: "suiteviewer" }));
      assert.equal((await viewer.get(privateRepo)).status, 404);
      assert.equal((await viewer.request(`${privateRef}git-upload-pack`, auth(readerToken))).status, 404);
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 404);
      assert.equal((await viewer.post(`/user/notifications/${notification}/read`, {})).status, 404);
      assert.doesNotMatch((await viewer.get("/user/notifications")).body, /Protected classification/);
      assert.doesNotMatch((await viewer.get("/explore/repos?q=private-vault")).body, /href="\/suiteowner\/private-vault/);
    });

    await t.test("renames, transfers and forks preserve Git data with canonical access", async () => {
      redirected(await owner.post("/repo/create", { name: "move-fixture" }));
      const old = "/suiteowner/move-fixture";
      const renamed = "/suiteowner/moved-fixture";
      redirected(await owner.post(`${old}/settings/lifecycle`, { action: "rename", name: "moved-fixture" }));
      assert.match((await owner.get(old)).body, /\/suiteowner\/moved-fixture\/commits/);
      const reserved = await owner.post("/repo/create", { name: "MOVE-fixture" });
      assert.notEqual(reserved.status, 302);
      assert.equal((await new Client().request(`${old}.git/info/refs?service=git-upload-pack`)).status, 200);
      redirected(await owner.post(`${renamed}/settings/lifecycle`, { action: "request_transfer", recipient: "suiteviewer" }));
      const transfers = "/user/settings/transfers";
      const id = (await viewer.get(transfers)).body.match(/name="transfer_id" value="(\d+)"/)?.[1];
      assert.ok(id);
      assert.equal((await owner.post(transfers, { action: "accept", transfer_id: id })).status, 404);
      const accepted = await viewer.post(transfers, { action: "accept", transfer_id: id });
      redirected(accepted);
      assert.equal(accepted.location, "/suiteviewer/moved-fixture");
      assert.match((await owner.get(old)).body, /\/suiteviewer\/moved-fixture\/commits/);
      assert.equal((await owner.get(`${old}/settings`)).status, 403);
      assert.equal((await new Client().request(`${old}.git/info/refs?service=git-upload-pack`)).status, 200);

      const fork = await viewer.post("/suiteowner/history/fork", { name: "fork-fixture" });
      redirected(fork);
      assert.equal(fork.location, "/suiteviewer/fork-fixture");
      assert.match((await viewer.get(`${fork.location}/refs`)).body, /feature\/readme/);
      const rejected = await viewer.post("/user/settings/repository-import", { name: "unsafe-import", source_url: "http://127.0.0.1/repo.git" });
      assert.equal(rejected.status, 400);
      assert.equal((await viewer.get("/suiteviewer/unsafe-import")).status, 404);
    });

    await t.test("account deletion rejects owners and revokes every session and token", async () => {
      const account = await signup("deletefixture");
      const other = await login("deletefixture");
      redirected(other.result);
      const deletion = "/user/settings/delete";
      const fields = { confirm: "deletefixture", current_password: password };
      const result = await account.post("/user/settings/tokens", { name: "deleted token", scope: "read" });
      const token = result.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
      assert.ok(token);
      assert.equal((await account.post(deletion, { ...fields, confirm: "other" })).status, 400);
      assert.equal((await account.post(deletion, { ...fields, current_password: "wrong" })).status, 403);
      assert.equal((await account.post(deletion, fields, { csrf: false })).status, 403);
      assert.equal((await owner.post(deletion, { confirm: "suiteowner", current_password: password })).status, 409);
      redirected(await account.post(deletion, fields));
      assert.match((await other.client.get("/user/settings")).location, /^\/user\/login/);
      assert.equal((await new Client().request(`${privateRepo}.git/info/refs?service=git-upload-pack`, {
        headers: { Authorization: `Bearer ${token}` },
      })).status, 401);
      assert.notEqual((await login("deletefixture")).result.status, 302);
    });

    await t.test("repository pagination preserves search and excludes private results", async () => {
      for (let i = 0; i < 21; i++) {
        redirected(await owner.post("/repo/create", { name: `page-fixture-${String(i).padStart(2, "0")}` }));
      }
      const anonymous = new Client();
      const first = await anonymous.get("/explore/repos?q=page-fixture-");
      const next = pageLink(first.body, "page", 2);
      assert.equal(next.searchParams.get("q"), "page-fixture-");
      const second = await anonymous.get(next);
      const names = (body) => [...body.matchAll(/href="\/suiteowner\/(page-fixture-\d+)"/g)].map((m) => m[1]);
      assert.equal(names(first.body).length, 20);
      assert.equal(names(second.body).length, 1);
      assert.equal(new Set([...names(first.body), ...names(second.body)]).size, 21);
      const profile = await anonymous.get("/suiteowner?page=2");
      assert.doesNotMatch(profile.body, /private-vault/);
      assert.match((await owner.get("/suiteowner?page=2")).body, /private-vault/);
      const home = await owner.get("/");
      pageLink(home.body, "page", 2);
      assert.equal((await anonymous.get("/explore/repos?page=9999999999999999999999999999999999")).status, 200);
    });

    await t.test("issue and global list pagination retain filters without leaking private issues", async () => {
      for (let i = 0; i < 21; i++) {
        redirected(await owner.post(`${publicRepo}/issues/new`, {
          title: `Pagination issue ${String(i).padStart(2, "0")}`,
          body: i === 0 ? '<script>alert(1)</script> [unsafe](javascript:alert(1))' : "A real issue body.",
        }));
      }
      redirected(await owner.post(`${privateRepo}/issues/new`, { title: "private-issue-marker", body: "Keep private" }));
      const first = await owner.get(`${publicRepo}/issues?state=all&q=Pagination`);
      const next = pageLink(first.body, "page", 2);
      assert.equal(next.searchParams.get("state"), "all");
      assert.equal(next.searchParams.get("q"), "Pagination");
      const second = await owner.get(next);
      const rows = (body) => [...body.matchAll(/href="\/suiteowner\/project\/issues\/\d+"/g)].length;
      assert.equal(rows(first.body), 20);
      assert.equal(rows(second.body), 1);
      const global = await viewer.get("/issues");
      pageLink(global.body, "page", 2);
      assert.doesNotMatch(global.body, /private-issue-marker/);
      const detail = await owner.get(`${publicRepo}/issues/1`);
      assert.doesNotMatch(detail.body, /<script>/);
      assert.doesNotMatch(detail.body, /href="javascript:/i);
      assert.match(detail.body, /&lt;script&gt;/);
    });

    await t.test("discussion pagination reaches every comment and rejects wrong-kind mutations", async () => {
      for (let i = 0; i < 21; i++) {
        redirected(await owner.post(`${publicRepo}/issues/1/comments`, { body: `Comment marker ${String(i).padStart(2, "0")}` }));
      }
      const first = await owner.get(`${publicRepo}/issues/1`);
      const next = pageLink(first.body, "comments_page", 2);
      const second = await owner.get(next);
      const comments = (body) => [...body.matchAll(/Comment marker \d+/g)].map((m) => m[0]);
      assert.equal(comments(first.body).length, 20);
      assert.equal(comments(second.body).length, 1);
      assert.equal(new Set([...comments(first.body), ...comments(second.body)]).size, 21);
      assert.equal((await owner.post(`${publicRepo}/pulls/1/state`, { state: "closed" })).status, 404);
      assert.equal((await owner.post(`${publicRepo}/pulls/1/comments`, { body: "wrong kind" })).status, 404);
      assert.equal((await viewer.post(`${publicRepo}/issues/1/state`, { state: "closed" })).status, 403);
      assert.equal((await owner.post(`${publicRepo}/issues/1/state`, { state: "invalid" })).status, 400);
    });

    await t.test("closing and reopening produce one actor-attributed event per transition", async () => {
      redirected(await owner.post(`${publicRepo}/issues/1/state`, { state: "closed" }));
      redirected(await owner.post(`${publicRepo}/issues/1/state`, { state: "closed" }));
      redirected(await owner.post(`${publicRepo}/issues/1/state`, { state: "open" }));
      const discussion = await owner.get(`${publicRepo}/issues/1?comments_page=2`);
      const events = [...discussion.body.matchAll(/class="timeline-event"[^>]*>([\s\S]*?)<\/div>/g)].map((m) => m[1]);
      assert.equal(events.length, 2);
      assert.ok(events.every((event) => event.includes("suiteowner")));
      assert.ok(events.some((event) => event.includes("closed")));
      assert.ok(events.some((event) => event.includes("reopened")));
    });

    await t.test("login throttling bounds failed attempts", async () => {
      const anonymous = new Client();
      await anonymous.get("/user/login");
      for (let i = 0; i < 8; i++) {
        const failed = await anonymous.post("/user/login", { login: "unregistered-throttle", password: "incorrect" });
        assert.notEqual(failed.status, 302);
        assert.notEqual(failed.status, 429);
      }
      const blocked = await anonymous.post("/user/login", { login: "unregistered-throttle", password: "incorrect" });
      assert.equal(blocked.status, 429);
      assert.ok(Number(blocked.headers.get("retry-after")) > 0);
    });

    await t.test("successful login clears previous failures", async () => {
      const anonymous = new Client();
      await anonymous.get("/user/login");
      for (let i = 0; i < 7; i++) {
        assert.notEqual((await anonymous.post("/user/login", { login: "suiteviewer", password: "wrong" })).status, 429);
      }
      redirected(await anonymous.post("/user/login", { login: "suiteviewer", password }));
      await anonymous.get("/user/settings");
      redirected(await anonymous.post("/user/logout", {}));
      await anonymous.get("/user/login");
      const failed = await anonymous.post("/user/login", { login: "suiteviewer", password: "wrong" });
      assert.notEqual(failed.status, 302);
      assert.notEqual(failed.status, 429);
    });

    await t.test("email and password changes verify current credentials and replace all sessions", async () => {
      await owner.get("/user/settings");
      const invalid = await owner.post("/user/settings/email", { email: "taken@example.test", current_password: "wrong" });
      assert.notEqual(invalid.status, 302);
      const duplicate = await owner.post("/user/settings/email", { email: "suiteviewer@example.test", current_password: password });
      assert.notEqual(duplicate.status, 302);
      redirected(await owner.post("/user/settings/email", { email: "updated@example.test", current_password: password }));
      assert.match((await owner.get("/user/settings")).body, /updated@example.test/);
      const otherSession = await login("suiteowner");
      redirected(otherSession.result);
      const mismatch = await owner.post("/user/settings/password", {
        current_password: password, password: "new-local-password", password_confirm: "mismatch",
      });
      assert.notEqual(mismatch.status, 302);
      redirected(await owner.post("/user/settings/password", {
        current_password: password, password: "new-local-password", password_confirm: "new-local-password",
      }));
      assert.equal((await owner.get("/user/settings")).status, 200);
      assert.match((await otherSession.client.get("/user/settings")).location, /^\/user\/login/);
      assert.notEqual((await login("suiteowner", password)).result.status, 302);
      redirected((await login("suiteowner", "new-local-password")).result);
    });

    await t.test("simulated verification and reset emails consume a link exactly once", async () => {
      const account = await signup("recoveryfixture");
      const recipient = "recoveryfixture@example.test";
      const explorer = `${base}/cdn-cgi/local/explorer/api/local/email/sending`;
      const outbox = async () => {
        const response = await fetch(explorer);
        assert.equal(response.status, 200);
        return (await response.json()).result;
      };
      async function receivedLink(subject, path) {
        const deadline = Date.now() + 5_000;
        for (;;) {
          const item = (await outbox()).find((message) => message.subject === subject && message.to.includes(recipient));
          if (item) {
            const response = await fetch(`${explorer}?email_id=${encodeURIComponent(item.messageId)}`);
            assert.equal(response.status, 200);
            const { text } = (await response.json()).result;
            const link = new URL(text.match(/https:\/\/[^\s]+/)?.[0]);
            assert.equal(link.origin, "https://forge.example.test");
            assert.equal(link.pathname, path);
            const token = link.searchParams.get("token");
            assert.match(token, /^[a-f0-9]{64}$/);
            return { token, path: link.pathname + link.search };
          }
          assert.ok(Date.now() < deadline, `No simulated email with subject ${subject}`);
          await new Promise((done) => setTimeout(done, 30));
        }
      }
      const verify = "/user/verify_email";
      const reset = "/user/reset_password";
      const guest = new Client();
      await guest.get("/user/forgot_password");
      const unknown = await guest.post("/user/forgot_password", { email: "missing@example.test" });
      const unverified = await guest.post("/user/forgot_password", { email: recipient });
      assert.equal(unknown.body, unverified.body);
      redirected(await account.post("/user/settings/email/verify", {}));
      const emailLink = await receivedLink("Verify email", verify);
      const first = await guest.get(emailLink.path);
      const second = await guest.get(emailLink.path);
      assert.equal(first.status, 200);
      assert.equal(second.status, 200);
      assert.equal(first.headers.get("referrer-policy"), "strict-origin");
      assert.equal((await guest.post(verify, { token: emailLink.token }, { csrf: false })).status, 403);
      assert.equal((await guest.post(verify, { token: emailLink.token })).status, 200);
      assert.equal((await guest.post(verify, { token: emailLink.token })).status, 400);
      assert.match((await account.get("/user/settings")).body, /Email address verified/);
      const requested = await guest.post("/user/forgot_password", { email: recipient });
      assert.equal(requested.body, unknown.body);
      const resetLink = await receivedLink("Reset password", reset);
      await guest.get(resetLink.path);
      const newPassword = "reset-local-secret";
      const fields = { token: resetLink.token, password: newPassword, password_confirm: newPassword };
      assert.equal((await guest.post(reset, fields)).status, 200);
      assert.equal((await guest.post(reset, fields)).status, 400);
      assert.match((await account.get("/user/settings")).location, /^\/user\/login/);
      assert.notEqual((await login("recoveryfixture", password)).result.status, 302);
      redirected((await login("recoveryfixture", newPassword)).result);
    });

    await t.test("signed webhooks retry the same event and respect repository scope", async () => {
      const account = await login("expiredfixture");
      redirected(account.result);
      await account.client.get("/user/settings");
      const opened = await account.client.post("/expiredfixture/webhook-fixture/issues/new", {
        title: "Webhook test issue",
      });
      redirected(opened);
      const tick = async () => {
        const result = await fetch(`${base}/__scheduled?cron=${encodeURIComponent("* * * * *")}`);
        assert.equal(result.status, 200);
        await result.text();
      };
      const stored = async () => JSON.parse(await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence,
        "--json", "--command", "SELECT d.state, d.attempts, d.last_status FROM webhook_deliveries d JOIN repos r ON r.id=d.repo_id WHERE r.artifacts_repo='webhook-regression-repo'"]))[0].results;
      await tick();
      assert.deepEqual(await stored(), [{ state: "pending", attempts: 1, last_status: 503 }]);
      await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence, "--command",
        "UPDATE webhook_deliveries SET next_attempt_at=0 WHERE repo_id=(SELECT id FROM repos WHERE artifacts_repo='webhook-regression-repo')"]);
      await tick();
      assert.deepEqual(await stored(), [{ state: "sent", attempts: 2, last_status: 204 }]);
      await tick();
      assert.equal(deliveries.length, 2);
      assert.deepEqual(deliveries[0], deliveries[1]);
      const event = deliveries[0];
      assert.equal(event.method, "POST");
      assert.equal(event.path, "/hooks/shard");
      assert.equal(event.signature, `sha256=${createHmac("sha256", webhookSecret).update(event.body).digest("hex")}`);
      const payload = JSON.parse(event.body);
      assert.equal(String(payload.event_id), event.id);
      assert.equal(payload.repo, "webhook-regression-repo");
      assert.equal(payload.action, "opened");
    });

    await t.test("request limiting is scoped by source and supplies a correlation ID", async () => {
      const headers = { "CF-Connecting-IP": "192.0.2.15", "Accept-Language": "zh-CN" };
      const guest = new Client();
      let limited;
      const { limit } = config.ratelimits.find((binding) => binding.name === "REQUEST_LIMITER").simple;
      // The simulator aligns its counters to wall-clock windows. Allow the
      // full quota on both sides of a rollover before requiring a rejection.
      for (let index = 0; index <= 2 * limit; index++) {
        const result = await guest.request("/user/settings/language", { headers });
        if (result.status === 429) {
          limited = result;
          break;
        }
      }
      assert.ok(limited, "the local binding should enforce its limit");
      assert.equal(limited.headers.get("retry-after"), "60");
      assert.equal(limited.headers.get("content-language"), "zh-Hans");
      assert.match(limited.headers.get("x-request-id"), /^[A-Za-z0-9-]{16,64}$/);
      assert.equal((await guest.request("/user/settings/language", { headers: { "CF-Connecting-IP": "192.0.2.16" } })).status, 200);
    });

    await t.test("scheduled maintenance removes expired credentials", async () => {
      const response = await fetch(`${base}/__scheduled?cron=0+3+*+*+*`);
      assert.equal(response.status, 200);
      await response.text();
      const output = await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence,
        "--json", "--command", `SELECT
          (SELECT COUNT(*) FROM access_tokens WHERE token_hash='${sha(expiredSecret)}') AS tokens,
          (SELECT COUNT(*) FROM sessions WHERE token_hash='${sha(expiredSession)}') AS sessions`]);
      const result = JSON.parse(output).flatMap((item) => item.results)[0];
      assert.equal(result.tokens, 0);
      assert.equal(result.sessions, 0);
    });
  } finally {
    await worker?.stop();
    if (receiver.listening) {
      receiver.close();
      receiver.closeAllConnections();
      await once(receiver, "close");
    }
    await rm(temporary, { recursive: true, force: true });
  }
});
