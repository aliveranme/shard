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
import { setTimeout as delay } from "node:timers/promises";
import { unstable_dev } from "wrangler";
import { gpgForm, gpgSettings, signedCommit } from "./gpg.mjs";
import { exerciseLfs, prepareLfsObject } from "./lfs.mjs";

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const wrangler = join(root, "node_modules/wrangler/bin/wrangler.js");
const password = "local-regression-password";
const expiredSecret = "shard_expired_regression_credential";
const expiredSession = "expired-regression-session";
const legacySecret = "legacy-regression-credential";
const sha = (value) => createHash("sha256").update(value).digest("hex");
const fixturePng = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAFElEQVR4nGNcG27DAAZMEIqBgQEAFy0BRO8F2yUAAAAASUVORK5CYII=", "base64");
const replacementPng = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAFElEQVR4nGO0Sq9jAAMmCMXAwAAAFCYBI8t2+TUAAAAASUVORK5CYII=", "base64");

// Each run owns its database, Artifacts namespace state and temporary config.
// No test writes to .wrangler/state or contacts a deployed Worker.
// The scenarios share one forge and run sequentially with real Git subprocesses.
test("forge HTTP regression scenarios", { timeout: 480_000 }, async (t) => {
  const temporary = await mkdtemp(join(tmpdir(), "shard-http-"));
  const persistence = join(temporary, "state");
  const configPath = join(temporary, "wrangler.json");
  const webhookSecret = "local-regression-webhook-signing-key";
  const deliveries = [];
  const subscriptionDeliveries = [];
  const transferDeliveries = [];
  const receiver = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const capture = { id: req.headers["x-shard-event-id"], signature: req.headers["x-shard-signature"],
      method: req.method, path: req.url, body: Buffer.concat(chunks) };
    if (req.url === "/hooks/history") {
      const fail = subscriptionDeliveries.length === 0;
      subscriptionDeliveries.push({ ...capture, delivery: req.headers["x-shard-delivery-id"] });
      res.writeHead(fail ? 503 : 204).end();
    } else if (["/hooks/transfer-old", "/hooks/transfer-new"].includes(req.url)) {
      transferDeliveries.push(capture);
      res.writeHead(204).end();
    } else {
      deliveries.push(capture);
      res.writeHead(deliveries.length === 1 ? 503 : 204).end();
    }
  });
  let worker;
  try {
    receiver.listen({ host: "127.0.0.1", port: 0 });
    await once(receiver, "listening");
    const { env, ...baseConfig } = JSON.parse((await readFile(join(root, "wrangler.jsonc"), "utf8"))
      .replace(/^\s*\/\/.*$/gm, ""));
    const config = { ...baseConfig, ...env.artifacts };
    delete config.$schema;
    delete config.build;
    config.name = "shard-http-tests";
    config.main = resolve(root, config.main);
    config.assets.directory = resolve(root, config.assets.directory);
    config.artifacts = config.artifacts.map((binding) => ({ ...binding, remote: false }));
    const managed = env.managed;
    config.durable_objects = { bindings: managed.durable_objects.bindings.filter(({ name }) => name === "MERGE_COORDINATOR") };
    config.migrations = [{ tag: "managed-test-v1", new_sqlite_classes: ["MergeQueueCoordinator"] }];
    config.workflows = managed.workflows.filter(({ binding }) => binding === "REPOSITORY_COPY_WORKFLOW");
    config.send_email = [{ name: "EMAIL", remote: false }];
    config.vars = {
      ...config.vars, PUBLIC_ORIGIN: "https://forge.example.test", EMAIL_FROM: "shard@example.test",
      WEBHOOK_LOCAL_HTTP: "1", WEBHOOK_SECRET: webhookSecret, WEBHOOK_REPO: "webhook-regression-repo",
      WEBHOOK_ENDPOINT: `http://127.0.0.1:${receiver.address().port}/hooks/shard`,
      WEBHOOK_LOCAL_HTTP_ORIGIN: `http://127.0.0.1:${receiver.address().port}`,
      WEBHOOK_ENCRYPTION_KEY: "19".repeat(32),
      SHARD_LFS_MAX_BYTES: "100000001",
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
    const waitFor = async (condition, message) => {
      const deadline = Date.now() + 25_000;
      while (Date.now() < deadline) {
        if (await condition()) return;
        await delay(100);
      }
      assert.fail(message);
    };
    const tick = async () => {
      const result = await fetch(`${base}/__scheduled?cron=${encodeURIComponent("* * * * *")}`, {
        signal: AbortSignal.timeout(30_000),
      });
      assert.equal(result.status, 200);
      await result.text();
    };
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
        const url = new URL(path, base);
        const response = await fetch(url, {
          signal: AbortSignal.timeout(30_000), ...options, headers, redirect: "manual",
        }).catch((error) => {
          throw new Error(`${options.method ?? "GET"} ${url.pathname} failed`, { cause: error });
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
    const redirected = (response) => assert.equal(response.status, 302, response.body.match(/<h1[^>]*>[^<]*/) ?? response.body.slice(0, 500));
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
      assert.match(page.headers.get("content-security-policy"), /style-src 'self';/);
      assert.doesNotMatch(page.headers.get("content-security-policy"), /unsafe-inline/);
      assert.equal(page.headers.get("x-content-type-options"), "nosniff");
      assert.equal(page.headers.get("cache-control"), "no-store");
      assert.match(page.body, /\/css\/theme\.css\?v=[0-9a-f]{16}/);
      assert.match(page.body, /\/css\/shard\.css\?v=[0-9a-f]{16}/);
      const home = await anonymous.get("/");
      assert.equal(home.headers.get("cache-control"), "private, no-cache");
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

    await t.test("avatars require the owning session and support replacement and removal", async () => {
      const member = await signup("avatarfixture");
      const profile = "/avatarfixture";
      const uploadPath = "/user/settings/avatar";
      const guest = new Client();
      const upload = (bytes, { csrf = member.csrf, origin = base } = {}) => {
        const form = new FormData();
        if (csrf) form.set("csrf_token", csrf);
        form.set("avatar", new Blob([bytes], { type: "image/png" }), "portrait.png");
        return member.request(uploadPath, { method: "POST", headers: { Origin: origin }, body: form });
      };
      const avatarUrl = (html) => html.match(/src="(\/user\/avatar\/\d+)"/)?.[1];
      assert.equal(avatarUrl((await guest.get(profile)).body), undefined);
      assert.equal((await upload(fixturePng, { csrf: "" })).status, 403);
      assert.equal((await upload(fixturePng, { csrf: viewer.csrf })).status, 403);
      assert.equal((await upload(fixturePng, { origin: "https://elsewhere.example" })).status, 403);
      assert.equal((await upload(Buffer.from("Not a PNG"))).status, 400);
      assert.equal((await upload(Buffer.alloc(600 * 1024))).status, 413);
      redirected(await upload(fixturePng));

      const image = avatarUrl((await guest.get(profile)).body);
      assert.ok(image, "public profile includes the uploaded avatar");
      assert.equal(avatarUrl((await member.get("/user/settings")).body), image);
      const first = await fetch(new URL(image, base));
      assert.equal(first.status, 200);
      assert.equal(first.headers.get("content-type"), "image/png");
      assert.equal(first.headers.get("x-content-type-options"), "nosniff");
      assert.match(first.headers.get("cache-control") ?? "", /public, max-age=\d+/);
      const firstEtag = first.headers.get("etag");
      assert.ok(firstEtag, "avatar responses include an ETag");
      const notModified = await fetch(new URL(image, base), { headers: { "If-None-Match": firstEtag } });
      assert.equal(notModified.status, 304);
      const firstBytes = Buffer.from(await first.arrayBuffer());
      assert.deepEqual(firstBytes.subarray(0, 8), fixturePng.subarray(0, 8));

      redirected(await upload(replacementPng));
      const second = await fetch(new URL(image, base), { headers: { "If-None-Match": firstEtag } });
      assert.equal(second.status, 200);
      assert.notEqual(second.headers.get("etag"), firstEtag);
      assert.notDeepEqual(Buffer.from(await second.arrayBuffer()), firstBytes);
      redirected(await member.post(`${uploadPath}/remove`, {}));
      assert.equal((await guest.get(image)).status, 404);
      assert.equal(avatarUrl((await guest.get(profile)).body), undefined);

      redirected(await upload(fixturePng));
      redirected(await member.post("/user/settings/delete", { confirm: "avatarfixture", current_password: password }));
      assert.equal((await guest.get(image)).status, 404);
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
      const fields = { name: "regression reader", scope: "read", expires_in: "30" };
      for (const attempted of [fields, { ...fields, current_password: "wrong" }]) {
        const denied = await owner.post("/user/settings/tokens", attempted);
        assert.equal(denied.status, 403);
        assert.doesNotMatch(denied.body, /\bshard_[a-f0-9]{64}\b|class="list__title">regression reader/);
      }
      const created = await owner.post("/user/settings/tokens", { ...fields, current_password: password });
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

    await t.test("access tokens paginate in creation order and revoke from the current page", async () => {
      const client = await signup("tokenpages");
      const path = "/user/settings/tokens";
      for (let i = 0; i < 21; i++) {
        const created = await client.post(path, {
          name: `page-token-${String(i).padStart(2, "0")}`, scope: "read", expires_in: "7", current_password: password,
        });
        assert.equal(created.status, 200);
        assert.match(created.body, /\bshard_[a-f0-9]{64}\b/);
      }
      const names = ({ body }) => [...body.matchAll(/class="list__title">(page-token-\d{2})</g)].map(match => match[1]);
      const first = await client.get(`${path}?token=never-in-navigation`);
      assert.deepEqual(names(first), Array.from({ length: 20 }, (_, i) => `page-token-${String(20 - i).padStart(2, "0")}`));
      const next = pageLink(first.body, "page", 2);
      assert.equal(next.search, "?page=2");
      assert.doesNotMatch(first.body, /never-in-navigation|\bshard_[a-f0-9]{64}\b/);
      const second = await client.get(next);
      assert.deepEqual(names(second), ["page-token-00"]);
      const revoke = second.body.match(/action="(\/user\/settings\/tokens\/\d+\/delete\?page=2)"/)?.[1];
      assert.ok(revoke);
      const deleted = await client.post(revoke, {});
      redirected(deleted);
      assert.equal(deleted.location, `${path}?page=2`);
      const empty = await client.get(deleted.location);
      assert.deepEqual(names(empty), []);
      assert.equal(pageLink(empty.body, "page", 1).pathname, path);
      assert.doesNotMatch((await client.get(path)).body, /rel="next"/);
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
      const created = await owner.post("/user/settings/tokens", { name: "history writer", scope: "write", expires_in: "7", current_password: password });
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
      await git("-C", worktree, "config", "user.email", "SUITEOWNER@EXAMPLE.TEST");
      await git("-C", worktree, "config", "commit.gpgsign", "false");
      for (let i = 0; i < 21; i++) {
        await writeFile(join(worktree, "README.md"), `# History ${i}\n`);
        await git("-C", worktree, "add", "README.md");
        await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", `history-${String(i).padStart(2, "0")}`);
      }
      await git("-C", worktree, "checkout", "-b", "feature/readme");
      await writeFile(join(worktree, "README.md"), [
        "# Updated branch", "<div>literal text</div>", "",
        "![Local plot](figures/map.png)", "", "[Guide](figures/guide.md) · [Folder](figures/)",
      ].join("\n"));
      await writeFile(join(worktree, "IMAGE.bin"), Buffer.from([0, 1, 2]));
      const largeFile = "long fixture line\n".repeat(16_000) + "raw-download-tail\n";
      await writeFile(join(worktree, "LARGE.txt"), largeFile);
      await writeFile(join(worktree, "FEATURE.md"), "# A branch with a slash\n");
      const folder = join(worktree, "docs and examples");
      await mkdir(folder);
      await writeFile(join(folder, "release%20notes.txt"), "A nested file from the slash branch.\n");
      const figures = join(worktree, "figures");
      await mkdir(figures);
      await writeFile(join(figures, "map.png"), fixturePng);
      await writeFile(join(figures, "guide.md"), [
        "# Field guide", "", "<IMG", '  SRC="./map.png" alt="Local &amp; remote" width="320">',
        "", "[Feature](../FEATURE.md)",
      ].join("\n"));
      await git("-C", worktree, "add", "FEATURE.md", "README.md", "IMAGE.bin", "LARGE.txt", "docs and examples", "figures");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "feature note");
      await git("-C", worktree, "remote", "add", "origin", `${base}/suiteowner/history.git`);
      await git("-C", worktree, "push", "origin", "main", "feature/readme");
      const rootCommit = (await git("-C", worktree, "rev-list", "--max-parents=0", "main")).stdout.trim();
      assert.match((await owner.get(`/suiteowner/history/commit/${rootCommit}`)).body, /Root commit \(no parents\)/);
      const signature = await signedCommit(async (...args) => (await git("-C", worktree, ...args)).stdout.trim(),
        worktree, "suiteowner@example.test");
      await git("-C", worktree, "push", "origin", "gpg-fixture");
      redirected(await owner.request(gpgSettings, {
        method: "POST", headers: { Origin: base }, body: gpgForm(owner.csrf, {
          action: "save", name: "Artifacts signing key", public_key: signature.publicKey, current_password: password,
        }),
      }));
      assert.match((await owner.get(`/suiteowner/history/commit/${signature.signed}`)).body, /Valid GPG signature/);
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
      const preview = await owner.get(`${history}/blob/LARGE.txt?branch=feature%2Freadme`);
      assert.match(preview.body, /Preview truncated to the first 256 KiB/);
      assert.match(preview.body, /class="code-table"/);
      assert.match(preview.body, /id="L1"/);
      assert.match(preview.body, /href="#L1"/);
      assert.match(preview.body, /long fixture line/);
      assert.doesNotMatch(preview.body, /raw-download-tail/);
      const rawFile = await owner.get(`${history}/raw/LARGE.txt?branch=feature%2Freadme`);
      assert.equal(rawFile.headers.get("content-type"), "application/octet-stream");
      assert.equal(rawFile.body, largeFile);
      const binary = await owner.get(`${history}/blob/IMAGE.bin?branch=feature%2Freadme`);
      assert.match(binary.body, /Binary file/);
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
      assert.match((await owner.get(saved.location)).body, /Used for browsing, new pull requests and Git clones/);
      assert.match((await owner.get(history)).body, /href="\/suiteowner\/history\/blob\/FEATURE.md\?branch=feature%2Freadme"/);
      assert.match(await gitHead(), /ref: refs\/heads\/feature\/readme\s+HEAD/);
      const defaultClone = join(temporary, "changed-head-clone");
      await git("clone", "--depth", "1", `${base}${history}.git`, defaultClone);
      assert.equal((await git("-C", defaultClone, "rev-parse", "HEAD")).stdout.trim(), featureCommit);
      redirected(await owner.post(settingsPath, { default_branch: "main", visibility: "public" }));
      assert.equal(await gitHead(), remoteHead);

      for (const [kind, ref] of [["heads", "web/branch"], ["tags", "web/tag"]]) {
        const fields = { kind, name: ref, oid: featureCommit };
        assert.equal((await viewer.post(`${history}/refs`, { ...fields, action: "create" })).status, 403);
        redirected(await owner.post(`${history}/refs`, { ...fields, action: "create" }));
        const remote = `refs/${kind}/${ref}`;
        assert.equal((await git("-C", worktree, "ls-remote", "origin", remote)).stdout.trim(), `${featureCommit}\t${remote}`);
        assert.equal((await owner.post(`${history}/refs`, { ...fields, action: "delete", confirm: "wrong" })).status, 400);
        redirected(await owner.post(`${history}/refs`, { ...fields, action: "delete", confirm: ref }));
        assert.equal((await git("-C", worktree, "ls-remote", "origin", remote)).stdout, "");
      }

      const search = (type, q) => owner.get(`${history}/search?${new URLSearchParams({ type, q, branch: "feature/readme" })}`);
      const foundPath = await search("path", "release%20notes");
      const pathHit = `${history}/blob/${encodeURIComponent("docs and examples/release%20notes.txt")}?branch=${featureCommit}`;
      assert.ok(foundPath.body.includes(`href="${pathHit}"`));
      assert.equal((await owner.get(pathHit)).status, 200);
      const foundCode = await search("code", "Field guide");
      assert.ok(foundCode.body.includes(`blob/${encodeURIComponent("figures/guide.md")}?branch=${featureCommit}`));
      assert.match(foundCode.body, /Partial results/);
      assert.match((await search("commits", "feature note")).body, new RegExp(`/commit/${featureCommit}\\?branch=${featureCommit}`));
      assert.equal((await viewer.get(`${privateRepo}/search?q=secret&type=code`)).status, 404);

      const avatarForm = new FormData();
      avatarForm.set("csrf_token", owner.csrf);
      avatarForm.set("avatar", new Blob([fixturePng], { type: "image/png" }), "portrait.png");
      redirected(await owner.request("/user/settings/avatar", { method: "POST", headers: { Origin: base }, body: avatarForm }));
      const visitor = new Client();
      const avatarUrl = (await visitor.get("/suiteowner")).body.match(/src="(\/user\/avatar\/\d+)"/)?.[1];
      assert.ok(avatarUrl);
      for (const path of [`${history}?branch=feature%2Freadme`, `${history}/commits?branch=feature%2Freadme`, `${history}/commit/${featureCommit}`]) {
        const page = await visitor.get(path);
        assert.ok(page.body.includes(`src="${avatarUrl}"`), path);
        assert.doesNotMatch(page.body, /SUITEOWNER@EXAMPLE\.TEST/i);
      }

      const branchPage = await owner.get("/suiteowner/history?branch=feature%2Freadme");
      assert.match(branchPage.body, /<option value="feature\/readme"\s+selected>/);
      assert.match(branchPage.body, /<button[^>]*type="submit">Browse branch<\/button>/);
      assert.match(branchPage.body, /href="\/suiteowner\/history\/blob\/FEATURE.md\?branch=feature%2Freadme"/);
      const media = branchPage.body.match(/src="([^"]+\/media\/[^"]+)"/)?.[1];
      assert.ok(media, "the README renders a repository image");
      assert.equal(new URL(media, base).searchParams.get("branch"), featureCommit);
      const image = await fetch(new URL(media, base));
      assert.equal(image.status, 200);
      assert.equal(image.headers.get("content-type"), "image/png");
      assert.equal(image.headers.get("cross-origin-resource-policy"), "same-origin");
      assert.deepEqual(Buffer.from(await image.arrayBuffer()), fixturePng);
      const guideLink = branchPage.body.match(/href="([^"]+)"[^>]*>Guide<\/a>/)?.[1];
      assert.ok(guideLink, "the README links to the Markdown file");
      const guide = await fetch(new URL(guideLink, base));
      assert.equal(guide.status, 200);
      const guideBody = await guide.text();
      assert.match(guideBody, /<h1[^>]*>Field guide<\/h1>/);
      assert.ok(guideBody.includes(media), "a nested Markdown image resolves from the file's directory");
      assert.match(guideBody, /<img\b[^>]*alt="Local &amp; remote"[^>]*width="320"/);
      const folderLink = branchPage.body.match(/href="([^"]+)"[^>]*>Folder<\/a>/)?.[1];
      assert.ok(folderLink, "the README links to the directory");
      assert.match((await owner.get(folderLink)).body, /file-row__name mono">map\.png/);
      const imagePreview = await owner.get(`${history}/blob/figures/map.png?branch=${featureCommit}`);
      assert.ok(imagePreview.body.includes(media), "image files render an inline preview");
      assert.equal((await owner.get(`${history}/media/FEATURE.md?branch=${featureCommit}`)).status, 404);
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
      const number = pullUrl.split("/").at(-1);
      const issueAlias = await owner.get(`${history}/issues/${number}`);
      redirected(issueAlias);
      assert.equal(issueAlias.location, pullUrl);
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

      const queueBranch = "automation/checks";
      await git("-C", worktree, "switch", "-c", queueBranch, "main");
      await writeFile(join(worktree, "CHECKED.txt"), "This change passed the required check.\n");
      await git("-C", worktree, "add", "CHECKED.txt");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "Test required checks");
      const reviewedOid = (await git("-C", worktree, "rev-parse", "HEAD")).stdout.trim();
      await git("-C", worktree, "push", "origin", queueBranch);
      const rulePath = `${history}/settings/branches`;
      const rule = { action: "save", branch: "main", approvals: "1", checks: "ci/unit" };
      assert.equal((await viewer.post(rulePath, rule)).status, 403);
      redirected(await owner.post(rulePath, rule));
      await assert.rejects(git("-C", worktree, "push", "origin", `${reviewedOid}:refs/heads/main`), /403/);
      const verifyMain = async (expected, stage) => {
        const ref = await git("-C", worktree, "ls-remote", "origin", "refs/heads/main").catch((error) => {
          throw new Error(`Could not read main ${stage}`, { cause: error });
        });
        assert.equal(ref.stdout.trim(), `${expected}\trefs/heads/main`, stage);
      };
      await verifyMain(mergeTip, "after the rejected direct push");
      const gatePull = await viewer.post(`${history}/pulls/new`, { title: "Require a review and green CI", base_branch: "main", head_branch: queueBranch });
      redirected(gatePull);
      const queuePath = `${gatePull.location}/merge`;
      const queued = { action: "queue", comparison_version: "1" };
      assert.equal((await viewer.post(queuePath, queued)).status, 403);
      redirected(await owner.post(queuePath, queued));
      await waitFor(async () => (await owner.get(gatePull.location)).body.includes("Waiting for required approvals or checks"),
        "branch coordinator did not examine the queued merge");
      const status = (state) => owner.request(`${history}/commit/${reviewedOid}/checks`, {
        method: "POST", headers: { Authorization: `Bearer ${secret}`, "Content-Type": "application/json" },
        body: JSON.stringify({ context: "ci/unit", state }),
      });
      assert.equal((await status("failure")).status, 204);
      redirected(await owner.post(`${gatePull.location}/reviews`, { decision: "approved", comparison_version: "1" }));
      await tick();
      await verifyMain(mergeTip, "with failing CI");
      assert.match((await owner.get(gatePull.location)).body, /Checks on the reviewed commit[\s\S]*ci\/unit[\s\S]*Failed/);
      assert.equal((await status("success")).status, 204);
      await waitFor(async () => (await owner.get(gatePull.location)).body.includes("Merged by Shard"),
        "passing CI did not wake the queued merge");
      await verifyMain(reviewedOid, "after the successful queued merge");
      assert.match((await owner.get(gatePull.location)).body, /Merged by Shard/);

      const hook = `${history}/settings/webhooks`;
      const signingSecret = "repository-hook-regression-key";
      const subscription = { action: "create", endpoint: `${config.vars.WEBHOOK_LOCAL_HTTP_ORIGIN}/hooks/history`, secret: signingSecret, label: "History reporter" };
      assert.equal((await viewer.post(hook, subscription)).status, 403);
      assert.equal((await owner.post(hook, { ...subscription, endpoint: "http://127.0.0.1:1/hooks" })).status, 400);
      redirected(await owner.post(hook, subscription));
      const tagRef = "refs/tags/delivery/v1";
      await git("-C", worktree, "push", "origin", `${featureCommit}:${tagRef}`);
      for (let attempt = 0; attempt < 6 && subscriptionDeliveries.length === 0; attempt++) {
        await tick();
        await delay(50);
      }
      assert.equal(subscriptionDeliveries.length, 1);
      const delivery = subscriptionDeliveries[0];
      const report = JSON.parse(delivery.body);
      assert.deepEqual([report.kind, report.action, report.ref, report.before, report.after], ["tag", "created", tagRef, null, featureCommit]);
      assert.equal(String(report.event_id), delivery.id);
      assert.equal(delivery.signature, `sha256=${createHmac("sha256", signingSecret).update(delivery.body).digest("hex")}`);
      assert.match(delivery.delivery, /^\d+$/);
      await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence, "--command",
        "UPDATE webhook_subscription_deliveries SET next_attempt_at=0 WHERE state='pending'"]);
      await tick();
      assert.deepEqual(subscriptionDeliveries, [delivery, delivery]);
      const recoveryRef = "refs/tags/delivery/recovered";
      const database = (sql) => cli(["d1", "execute", "DB", "--local", "--persist-to", persistence, "--command", sql]);
      await database(`CREATE TRIGGER interrupt_git_observation BEFORE INSERT ON webhook_events
        WHEN NEW.ref = '${recoveryRef}' AND NEW.actor_id IS NOT NULL
        BEGIN SELECT RAISE(ABORT, 'interrupted ref publication'); END`);
      try {
        await git("-C", worktree, "push", "origin", `${featureCommit}:${recoveryRef}`);
        const remote = await git("-C", worktree, "ls-remote", "origin", recoveryRef);
        assert.equal(remote.stdout.trim(), `${featureCommit}\t${recoveryRef}`);
        assert.equal(subscriptionDeliveries.length, 2);
        await database(`UPDATE git_push_intents SET check_at = 0
          WHERE EXISTS (SELECT 1 FROM json_each(updates) WHERE json_extract(value, '$.name') = '${recoveryRef}')`);
        await tick();
        await tick();
        assert.equal(subscriptionDeliveries.length, 3);
        const recovered = JSON.parse(subscriptionDeliveries[2].body);
        assert.deepEqual([recovered.ref, recovered.before, recovered.after, recovered.actor_id],
          [recoveryRef, null, featureCommit, null]);
      } finally {
        await database("DROP TRIGGER interrupt_git_observation");
      }
      const hookPage = await owner.get(hook);
      assert.match(hookPage.body, /History reporter/);
      assert.doesNotMatch(hookPage.body, /repository-hook-regression-key/);
      const subscription_id = hookPage.body.match(/name="subscription_id" value="(\d+)"/)?.[1];
      assert.ok(subscription_id);
      redirected(await owner.post(hook, { action: "disable", subscription_id }));
      await git("-C", worktree, "push", "origin", `:${tagRef}`);
      await tick();
      assert.equal(subscriptionDeliveries.length, 3);
    });

    await t.test("private LFS forks keep their bytes after source cleanup", async () => {
      const repo = "/suiteowner/lfs-objects";
      redirected(await owner.post("/repo/create", { name: "lfs-objects", private: "on" }));
      const token = async (scope) => {
        const response = await owner.post("/user/settings/tokens", { name: `LFS ${scope}`, scope, expires_in: "7", current_password: password });
        const value = response.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
        assert.ok(value);
        return value;
      };
      const canRunGitLfs = await exec("git", ["lfs", "version"]).then(() => true, () => false);
      const writeToken = await token("write");
      const readToken = await token("read");
      await exerciseLfs({ base, repo, writeToken, readToken, canRunGitLfs, maxObjectBytes: 100_000_001 });

      const objects = [];
      for (let index = 0; index < 9; index++) {
        objects.push(await prepareLfsObject({ base, repo, writeToken }));
      }
      const fork = "/suiteowner/lfs-fork";
      const copied = await owner.post(`${repo}/fork`, { name: "lfs-fork" });
      redirected(copied);
      const pendingPath = copied.location;
      assert.match(pendingPath, /^\/user\/settings\/repository-copies\/\d+$/);
      await waitFor(async () => (await owner.get(fork)).status === 200, "copy Workflow did not publish the fork");
      const instanceId = pendingPath.split("/").at(-1);
      const result = JSON.parse(await cli(["workflows", "instances", "describe", config.workflows[0].name,
        instanceId, "--local", "--port", String(worker.port), "--json"]));
      assert.equal(result.id, instanceId);
      assert.equal(result.status, "complete");
      assert.equal((await new Client().get(pendingPath)).status, 302);
      const forbidden = await objects[0].batch({ repo: fork });
      assert.equal(forbidden.status, 401);
      await forbidden.text();

      const sourceRow = JSON.parse(await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence,
        "--json", "--command", "SELECT id FROM repos WHERE name = 'lfs-objects'"])).flatMap((item) => item.results)[0];
      const bucket = config.r2_buckets.find((binding) => binding.binding === "LFS_OBJECTS").bucket_name;
      const sourceKey = `${bucket}/lfs/${sourceRow.id}/${objects[0].spec.oid}`;
      const outfile = join(temporary, "lfs-source-object");
      const readBucket = () => cli(["r2", "object", "get", sourceKey, "--local", "--persist-to", persistence, "--file", outfile]);
      await readBucket();
      assert.deepEqual(await readFile(outfile), objects[0].bytes);
      redirected(await owner.post(`${repo}/settings/delete`, { confirm: "lfs-objects" }), "/");
      assert.equal((await owner.get(repo)).status, 404);
      await tick();
      await assert.rejects(readBucket, /does not exist|not found/i);
      for (const object of objects) {
        const download = await object.download({ repo: fork, token: readToken });
        assert.equal(download.status, 200);
        assert.deepEqual(Buffer.from(await download.arrayBuffer()), object.bytes);
      }
    });

    await t.test("private repository grants govern Git, issue classification and notifications", async () => {
      const access = `${privateRepo}/settings/collaborators`;
      const writeToken = async (account, name) => {
        const result = await account.post("/user/settings/tokens", { name, scope: "write", current_password: password });
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
      await writeFile(join(seed, "map.png"), fixturePng);
      await git(ownerToken, "-C", seed, "add", "README.md", "map.png");
      await git(ownerToken, "-C", seed, "-c", "core.hooksPath=", "-c", "user.name=Regression",
        "-c", "user.email=git@example.test", "-c", "commit.gpgsign=false", "commit", "-m", "Private fixture");
      await git(ownerToken, "-C", seed, "push", remote, "main");
      const oid = (await git(ownerToken, "-C", seed, "rev-parse", "HEAD")).stdout.trim();
      assert.equal((await outsider.get(`${privateRepo}/archive/${oid}`)).status, 404);
      assert.equal((await outsider.get(`${privateRepo}/media/map.png?branch=${oid}`)).status, 404);
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
      assert.equal((await viewer.get(`${privateRepo}/media/map.png?branch=${oid}`)).headers.get("content-type"), "image/png");
      assert.equal((await viewer.get(access)).status, 403);
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 401);
      assert.equal((await checkResult(viewer, readerToken, "pending")).status, 403);
      const clone = join(temporary, "private-clone");
      await git(readerToken, "clone", "--config", "core.autocrlf=false", remote, clone);
      assert.equal(await readFile(join(clone, "README.md"), "utf8"), "# Private source\n");

      const releaseTag = "series/v2";
      await git(ownerToken, "-C", seed, "-c", "user.name=Regression", "-c", "user.email=git@example.test",
        "tag", "-a", releaseTag, "-m", "Private release");
      await git(ownerToken, "-C", seed, "push", remote, releaseTag);
      const releaseNew = `${privateRepo}/releases/new`;
      const releaseFields = { tag_name: releaseTag, title: "Pinned private release", notes: "<script>alert(1)</script>\n\n![Map](map.png)" };
      assert.equal((await viewer.post(releaseNew, releaseFields)).status, 403);
      assert.equal((await owner.post(releaseNew, { ...releaseFields, tag_name: "main" })).status, 422);
      const draft = await owner.post(releaseNew, releaseFields);
      redirected(draft);
      const releasePath = draft.location.replace(/\/edit$/, "");
      assert.equal((await viewer.get(releasePath)).status, 404);
      assert.equal((await owner.post(releaseNew, releaseFields)).status, 409);
      redirected(await owner.post(draft.location, { ...releaseFields, version: "1", action: "publish" }));
      assert.equal((await owner.post(draft.location, { ...releaseFields, version: "1", action: "publish" })).status, 409);
      const alternate = (await git(ownerToken, "-C", seed, "-c", "user.name=Regression", "-c", "user.email=git@example.test",
        "commit-tree", "HEAD^{tree}", "-p", oid, "-m", "Move tag after publication")).stdout.trim();
      await git(ownerToken, "-C", seed, "push", remote, `+${alternate}:refs/tags/${releaseTag}`);
      const release = await viewer.get(releasePath);
      assert.equal(release.status, 200);
      assert.ok(release.body.includes(`/archive/${oid}`));
      assert.ok(release.body.includes(`/media/map.png?branch=${oid}`));
      assert.doesNotMatch(release.body, /<script>/);
      assert.equal((await outsider.get(releasePath)).status, 404);

      for (const path of ["watch", "star"]) {
        redirected(await viewer.post(`${privateRepo}/${path}`, { active: "1" }));
        redirected(await viewer.post(`${privateRepo}/${path}`, { active: "1" }));
      }
      const watched = await owner.post(`${privateRepo}/issues/new`, { title: "Private watched issue" });
      redirected(watched);
      assert.equal([...((await viewer.get("/user/notifications")).body.matchAll(/>Private watched issue</g))].length, 1);
      assert.doesNotMatch((await owner.get("/user/notifications")).body, />Private watched issue</);
      assert.match((await viewer.get("/user/stars")).body, /suiteowner\/private-vault/);
      const reaction = `${watched.location}/reactions`;
      assert.equal((await outsider.post(reaction, { reaction: "up", active: "1" })).status, 404);
      for (let attempt = 0; attempt < 2; attempt++) {
        redirected(await viewer.post(reaction, { reaction: "up", active: "1" }));
      }
      assert.match((await viewer.get(watched.location)).body, /aria-pressed="true">\s*Upvote <span[^>]*>1</);
      redirected(await viewer.post(reaction, { reaction: "up", active: "0" }));
      assert.match((await viewer.get(watched.location)).body, /aria-pressed="false">\s*Upvote <span[^>]*>0</);
      redirected(await owner.post(access, { username: "suiteviewer", role: "write" }));
      const readCredential = await viewer.post("/user/settings/tokens", { name: "read-only CI", scope: "read", current_password: password });
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
      const templates = `${privateRepo}/settings/issue-templates`;
      const template = { name: "Bug report", title: "Default bug title", body: "Reproduction details" };
      assert.equal((await viewer.post(templates, template)).status, 403);
      redirected(await owner.post(templates, template));
      const formPath = `${privateRepo}/issues/new`;
      const newIssue = await viewer.get(formPath);
      const templateId = newIssue.body.match(/<option value="(\d+)"[^>]*>Bug report/)?.[1];
      assert.ok(templateId);
      const draftIssue = await viewer.post(formPath, { issue_template: templateId, apply_issue_template: "1", title: "My title" });
      assert.equal(draftIssue.status, 200);
      assert.match(draftIssue.body, /name="title" value="My title"/);
      assert.match(draftIssue.body, /Reproduction details/);
      assert.doesNotMatch((await viewer.get(`${privateRepo}/issues`)).body, />My title</);
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
      const labeled = await viewer.post(`${privateRepo}/issues/new`, { title: "Labeled on creation", label_id });
      redirected(labeled);
      assert.match((await viewer.get(labeled.location)).body, /badge--label[^>]*>review-required/);
      assert.match((await viewer.get(`${privateRepo}/issues`)).body, /Labeled on creation[\s\S]*review-required/);
      assert.match((await viewer.get("/issues?q=private-body-needle")).body, /Protected classification/);
      assert.doesNotMatch((await outsider.get("/issues?q=private-body-needle")).body, /Protected classification/);
      redirected(await owner.post(`${thread.location}/comments`, { body: "private-comment-needle" }));
      assert.match((await viewer.get(`${privateRepo}/issues?q=private-comment-needle`)).body, /Protected classification/);
      const filters = new URLSearchParams({ label: "review-required", milestone: "delivery-window", assignee: "suiteviewer", q: "private-comment-needle" });
      assert.match((await viewer.get(`${privateRepo}/issues?${filters}`)).body, />Protected classification</);
      assert.match((await viewer.get(`/issues?${filters}`)).body, />Protected classification</);
      assert.doesNotMatch((await outsider.get(`/issues?${filters}`)).body, />Protected classification</);
      filters.set("milestone", "unassigned-milestone");
      assert.doesNotMatch((await viewer.get(`${privateRepo}/issues?${filters}`)).body, />Protected classification</);
      const inbox = await viewer.get("/user/notifications");
      const notification = inbox.body.match(/notification-title-(\d+)"[^>]*>Protected classification/)?.[1];
      assert.ok(notification);
      assert.equal((await outsider.post(`/user/notifications/${notification}/read`, {})).status, 404);
      redirected(await viewer.post(`/user/notifications/${notification}/read`, {}));
      assert.doesNotMatch((await viewer.get("/user/notifications")).body, new RegExp(`/user/notifications/${notification}/read"`));

      redirected(await owner.post(`${access}/revoke`, { username: "suiteviewer" }));
      assert.equal((await viewer.get(privateRepo)).status, 404);
      assert.equal((await viewer.get(`${privateRepo}/media/map.png?branch=${oid}`)).status, 404);
      assert.equal((await viewer.request(`${privateRef}git-upload-pack`, auth(readerToken))).status, 404);
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 404);
      assert.equal((await viewer.post(`/user/notifications/${notification}/read`, {})).status, 404);
      assert.doesNotMatch((await viewer.get("/user/notifications")).body, /Protected classification/);
      assert.doesNotMatch((await viewer.get("/explore/repos?q=private-vault")).body, /href="\/suiteowner\/private-vault/);
      assert.equal((await viewer.get(releasePath)).status, 404);
      assert.doesNotMatch((await viewer.get("/user/stars")).body, /suiteowner\/private-vault/);
      assert.doesNotMatch((await viewer.get("/user/notifications")).body, />Private watched issue</);

      const orgs = "/user/organizations";
      const org = `${orgs}/regression-team`;
      const team = `${org}/teams/maintainers`;
      const offer = `${privateRepo}/settings/organization`;
      redirected(await owner.post(orgs, { slug: "regression-team" }));
      redirected(await owner.post(org, { action: "add_member", username: "suiteviewer" }));
      redirected(await owner.post(org, { action: "create_team", slug: "maintainers" }));
      const organization_id = (await owner.get(offer)).body.match(/<option value="(\d+)">regression-team</)?.[1];
      assert.ok(organization_id);
      redirected(await owner.post(offer, { action: "offer", organization_id }));
      const link_id = (await owner.get(org)).body.match(/name="link_id" value="(\d+)"/)?.[1];
      assert.ok(link_id);
      assert.equal((await viewer.post(org, { action: "accept", link_id })).status, 403);
      redirected(await owner.post(org, { action: "accept", link_id }));
      redirected(await owner.post(team, { action: "add_member", user_id: assignee_id }));
      redirected(await owner.post(team, { action: "grant", link_id, role: "read" }));
      assert.equal((await viewer.get(privateRepo)).status, 200);
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 401);
      redirected(await owner.post(team, { action: "grant", link_id, role: "write" }));
      assert.equal((await viewer.request(`${privateRef}git-receive-pack`, auth(readerToken))).status, 200);
      redirected(await viewer.post(edit, fields));
      redirected(await owner.post(access, { username: "suiteviewer", role: "read" }));
      redirected(await owner.post(`${access}/revoke`, { username: "suiteviewer" }));
      filters.set("milestone", "delivery-window");
      const filtered = `${privateRepo}/issues?${filters}`;
      const assigned = /Assignee<\/span>\s*<span><a href="\/suiteviewer"/;
      assert.match((await owner.get(thread.location)).body, assigned);
      const kept = await owner.get(filtered);
      assert.match(kept.body, />Protected classification</);
      assert.match(kept.body, /Open\s+<span class="badge badge--count">1</);
      assert.match((await owner.get(`/issues?${filters}`)).body, />Protected classification</);
      redirected(await owner.post(team, { action: "remove_member", user_id: assignee_id }));
      assert.equal((await viewer.get(privateRepo)).status, 404);
      assert.equal((await viewer.request(`${privateRef}git-upload-pack`, auth(readerToken))).status, 404);
      const assertCleared = async () => {
        const detail = await owner.get(thread.location);
        assert.match(detail.body, /Assignee<\/span>\s*<span>Unassigned/);
        const list = await owner.get(filtered);
        assert.doesNotMatch(list.body, />Protected classification</);
        assert.match(list.body, /Open\s+<span class="badge badge--count">0</);
        assert.doesNotMatch((await owner.get(`/issues?${filters}`)).body, />Protected classification</);
      };
      await assertCleared();
      redirected(await owner.post(team, { action: "add_member", user_id: assignee_id }));
      assert.equal((await viewer.get(privateRepo)).status, 200);
      await assertCleared();
      redirected(await owner.post(team, { action: "remove_member", user_id: assignee_id }));
    });

    await t.test("portable metadata import is owner-only and atomic", async () => {
      const repo = "/suiteowner/portable-metadata";
      redirected(await owner.post("/repo/create", { name: "portable-metadata" }));
      const path = `${repo}/settings/metadata-import`;
      const thread = { number: 42, title: "Imported work item", state: "open", created_at: 1700000000, updated_at: 1700000000 };
      const bundle = { version: 1, source: { url: "https://git.example.test/owner/repo.git", private: false }, threads: [thread], releases: [] };
      const submit = (client, value) => {
        const form = new FormData();
        form.set("csrf_token", client.csrf);
        form.set("bundle", JSON.stringify(value));
        return client.request(path, { method: "POST", headers: { Origin: base }, body: form });
      };
      assert.equal((await submit(viewer, bundle)).status, 404);
      assert.equal((await submit(owner, { ...bundle, version: 2 })).status, 422);
      assert.equal((await submit(owner, bundle)).status, 200);
      assert.match((await viewer.get(`${repo}/issues/42`)).body, /Imported work item/);
      assert.equal((await submit(owner, { ...bundle, threads: [{ ...thread, number: 43 }, thread] })).status, 409);
      assert.equal((await viewer.get(`${repo}/issues/43`)).status, 404);
      const next = await viewer.post(`${repo}/issues/new`, { title: "Next local work item" });
      redirected(next);
      assert.equal(next.location, `${repo}/issues/43`);
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
      const forkPath = "/suiteviewer/fork-fixture";
      await waitFor(async () => (await viewer.get(forkPath)).status === 200, "copy Workflow did not publish the repository");
      assert.match((await viewer.get(`${forkPath}/refs`)).body, /feature\/readme/);
      const rejected = await viewer.post("/user/settings/repository-import", { name: "unsafe-import", source_url: "http://127.0.0.1/repo.git" });
      assert.equal(rejected.status, 400);
      assert.equal((await viewer.get("/suiteviewer/unsafe-import")).status, 404);
    });

    await t.test("private transfers cancel former webhooks and wait for Git HEAD updates", async () => {
      const source = "/suiteowner/transfer-hooks";
      const target = "/suiteviewer/transfer-hooks";
      const transfers = "/user/settings/transfers";
      const database = (sql) => cli(["d1", "execute", "DB", "--local", "--persist-to", persistence,
        "--command", sql, "--json"]);
      const setHeadLease = (until) => database(`UPDATE repos SET git_head_lease_until = ${until}
        WHERE name = 'transfer-hooks'`);
      const oldDeliveries = async () => JSON.parse(await database(`
        SELECT e.issue_number, d.state, s.encrypted_secret IS NULL AS key_cleared
        FROM webhook_subscription_deliveries d
        JOIN webhook_subscriptions s ON s.id = d.subscription_id
        JOIN webhook_events e ON e.id = d.event_id
        WHERE s.label = 'Former reporter' ORDER BY e.issue_number
      `))[0].results.map(({ issue_number, state, key_cleared }) => [issue_number, state, key_cleared]);
      const hook = (name, label, secret) => ({ action: "create", label, secret,
        endpoint: `${config.vars.WEBHOOK_LOCAL_HTTP_ORIGIN}/hooks/transfer-${name}` });

      redirected(await owner.post("/repo/create", { name: "transfer-hooks", private: "on" }));
      redirected(await owner.post(`${source}/settings/webhooks`, hook("old", "Former reporter", "former-reporter-key")));
      redirected(await owner.post(`${source}/issues/new`, { title: "Delivered before transfer" }));
      await tick();
      assert.deepEqual(transferDeliveries.map(({ path }) => path), ["/hooks/transfer-old"]);
      redirected(await owner.post(`${source}/issues/new`, { title: "Pending before transfer" }));
      redirected(await owner.post(`${source}/issues/new`, { title: "Claimed before transfer" }));
      await database(`UPDATE webhook_subscription_deliveries SET state = 'sending', attempts = 1,
        next_attempt_at = unixepoch() + 300 WHERE event_id IN (
          SELECT e.id FROM webhook_events e JOIN repos r ON r.id = e.repo_id
          WHERE r.name = 'transfer-hooks' AND e.issue_number = 3
        )`);
      const original = [[1, "sent", 0], [2, "pending", 0], [3, "sending", 0]];
      assert.deepEqual(await oldDeliveries(), original);

      redirected(await owner.post(`${source}/settings/lifecycle`, { action: "request_transfer", recipient: "suiteviewer" }));
      const incoming = await viewer.get(transfers);
      const id = incoming.body.match(/name="transfer_id" value="(\d+)"/)?.[1];
      assert.ok(id);
      const accept = () => viewer.post(transfers, { action: "accept", transfer_id: id });
      await setHeadLease("unixepoch() + 300");
      const busy = await accept();
      assert.equal(busy.status, 409);
      assert.match(busy.body, /Git HEAD is being updated/);
      assert.equal((await viewer.get(source)).status, 404);
      assert.deepEqual(await oldDeliveries(), original);
      await setHeadLease("0");
      const accepted = await accept();
      redirected(accepted);
      assert.equal(accepted.location, target);

      assert.equal((await owner.get(`${source}/settings/webhooks`)).status, 404);
      const settings = await viewer.get(`${target}/settings/webhooks`);
      assert.equal(settings.status, 200);
      assert.match(settings.body, /Former reporter/);
      assert.deepEqual(await oldDeliveries(), [[1, "sent", 1], [2, "canceled", 1], [3, "canceled", 1]]);
      const secret = "recipient-webhook-regression-key";
      redirected(await viewer.post(`${target}/settings/webhooks`, hook("new", "Recipient reporter", secret)));
      redirected(await viewer.post(`${target}/issues/new`, { title: "After transfer" }));
      await tick();
      assert.deepEqual(transferDeliveries.map(({ path }) => path), ["/hooks/transfer-old", "/hooks/transfer-new"]);
      const delivery = transferDeliveries[1];
      assert.equal(JSON.parse(delivery.body).issue_number, 4);
      assert.equal(delivery.signature, `sha256=${createHmac("sha256", secret).update(delivery.body).digest("hex")}`);
    });

    await t.test("account deletion rejects owners and revokes every session and token", async () => {
      const account = await signup("deletefixture");
      const other = await login("deletefixture");
      redirected(other.result);
      const deletion = "/user/settings/delete";
      const fields = { confirm: "deletefixture", current_password: password };
      const result = await account.post("/user/settings/tokens", { name: "deleted token", scope: "read", current_password: password });
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
          body: i === 0 ? '<script>alert(1)</script> [unsafe](javascript:alert(1))'
            : i === 1 ? [
              "See #1, GH-1 and suiteowner/private-vault#1.", "",
              "https://github.com/octocat/Hello-World/issues/7", "",
              'Inline <img src="https://assets.example.test/diagram.png" alt="External figure" onclick="return false">',
              "", "`#99999` stays code.",
            ].join("\n") : "A real issue body.",
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
      const threaded = await viewer.get(`${publicRepo}/issues/2`);
      assert.match(threaded.body, /href="\/suiteowner\/project\/issues\/1"[^>]*>#1<\/a>/);
      assert.match(threaded.body, /href="\/suiteowner\/project\/issues\/1"[^>]*>GH-1<\/a>/);
      assert.match(threaded.body, /href="\/suiteowner\/private-vault\/issues\/1"[^>]*>suiteowner\/private-vault#1<\/a>/);
      assert.equal((await viewer.get(`${privateRepo}/issues/1`)).status, 404);
      assert.match(threaded.body, /href="https:\/\/github\.com\/octocat\/Hello-World\/issues\/7"/);
      const externalImage = threaded.body.match(/<img[^>]*src="https:\/\/assets\.example\.test\/diagram\.png"[^>]*>/)?.[0];
      assert.ok(externalImage, "remote Markdown images render in the discussion");
      assert.match(externalImage, /referrerpolicy="no-referrer"/);
      assert.doesNotMatch(externalImage, /onclick=/);
      assert.match(externalImage, /alt="External figure"/);
      assert.match(threaded.body, /<code>#99999<\/code>/);
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

    await t.test("concurrent credential changes cannot both reuse the old password", async () => {
      const first = await signup("passwordrace");
      const other = await login("passwordrace");
      redirected(other.result);
      await other.client.get("/user/settings");
      const proposals = [[first, "new-first-password"], [other.client, "new-second-password"]];
      const responses = await Promise.all(proposals.map(([client, next]) => client.post("/user/settings/password", {
        current_password: password, password: next, password_confirm: next,
      })));
      const succeeded = responses.map((response, i) => response.location === "/user/settings?updated=password" ? i : -1)
        .filter(i => i !== -1);
      assert.equal(succeeded.length, 1);
      const [winner] = succeeded;
      assert.equal((await proposals[winner][0].get("/user/settings")).status, 200);
      assert.match((await proposals[1 - winner][0].get("/user/settings")).location, /^\/user\/login/);
      redirected((await login("passwordrace", proposals[winner][1])).result);
      assert.notEqual((await login("passwordrace", proposals[1 - winner][1])).result.status, 302);
    });

    await t.test("simulated verification and reset emails consume a link exactly once", async () => {
      const account = await signup("recoveryfixture");
      const tokenPage = await account.post("/user/settings/tokens", { name: "recovery reader", scope: "read", current_password: password });
      const token = tokenPage.body.match(/\bshard_[a-f0-9]{64}\b/)?.[0];
      assert.ok(token);
      const legacy = "legacy-recovery-regression-credential";
      await cli(["d1", "execute", "DB", "--local", "--persist-to", persistence, "--command",
        `INSERT INTO access_tokens (user_id, name, token_hash, scope, created_at)
         SELECT id, 'legacy recovery', '${sha(legacy)}', 'read', unixepoch() FROM users WHERE username='recoveryfixture'`]);
      redirected(await account.post("/repo/create", { name: "repo", private: "on" }));
      const refs = "/recoveryfixture/repo.git/info/refs?service=git-upload-pack";
      const gitStatus = async (credential) => (await new Client().request(refs, {
        headers: { Authorization: `Bearer ${credential}` },
      })).status;
      assert.equal(await gitStatus(token), 200);
      assert.equal(await gitStatus(legacy), 200);
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
      assert.match((await guest.get(resetLink.path)).body, /revokes all Git access tokens/);
      const newPassword = "reset-local-secret";
      const fields = { token: resetLink.token, password: newPassword, password_confirm: newPassword };
      assert.equal((await guest.post(reset, fields)).status, 200);
      assert.equal((await guest.post(reset, fields)).status, 400);
      assert.match((await account.get("/user/settings")).location, /^\/user\/login/);
      assert.equal(await gitStatus(token), 401);
      assert.equal(await gitStatus(legacy), 401);
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

    await t.test("appearance preference, reserved usernames and literal explore search enforce boundaries", async () => {
      const guest = new Client();
      const appearance = "/user/settings/appearance";
      const page = await guest.get(appearance);
      assert.equal(page.status, 200);
      assert.equal(page.headers.get("cache-control"), "no-store");
      assert.doesNotMatch(page.body, /data-theme=/);
      assert.equal((await guest.post(appearance, { theme: "dark" }, { csrf: false })).status, 403);
      assert.equal((await guest.post(appearance, { theme: "neon" })).status, 400);

      const dark = await guest.post(appearance, { theme: "dark" });
      redirected(dark);
      assert.match(dark.headers.get("set-cookie") ?? "", /shard_theme=dark; Path=\/; HttpOnly; SameSite=Lax/);
      assert.match((await guest.get(dark.location)).body, /<html lang="en" data-theme="dark">/);

      const light = await guest.post(appearance, { theme: "light" });
      redirected(light);
      assert.match((await guest.get("/")).body, /<html lang="en" data-theme="light">/);

      const auto = await guest.post(appearance, { theme: "auto" });
      redirected(auto);
      assert.doesNotMatch((await guest.get("/")).body, /data-theme=/);

      await guest.get("/user/sign_up");
      for (const reserved of ["repo", "css", "fonts", "img"]) {
        const res = await guest.post("/user/sign_up", {
          username: reserved, email: `${reserved}@example.test`, password,
        });
        assert.equal(res.status, 200);
        assert.match(res.body, /reserved/i);
      }

      const wildcardSearch = await guest.get("/explore/repos?q=___");
      assert.equal(wildcardSearch.status, 200);
      assert.doesNotMatch(wildcardSearch.body, /suiteowner\/project/);
      const literalSearch = await guest.get("/explore/repos?q=project");
      assert.match(literalSearch.body, /suiteowner\/project/);
    });

    await t.test("discussion limits and organization member leave/offer cleanup enforce invariants", async () => {
      const longTitle = await owner.post("/suiteowner/project/issues/new", { title: "t".repeat(301), body: "ok" });
      assert.equal(longTitle.status, 200);
      assert.match(longTitle.body, /300/);
      const controlTitle = await owner.post("/suiteowner/project/issues/new", { title: "bad\u0007title", body: "ok" });
      assert.equal(controlTitle.status, 200);
      assert.match(controlTitle.body, /300|control/i);
      const longBody = await owner.post("/suiteowner/project/issues/new", { title: "valid", body: "x".repeat(256 * 1024 + 1) });
      assert.equal(longBody.status, 200);
      assert.match(longBody.body, /256 KiB/);

      const member = new Client();
      await member.get("/user/sign_up");
      redirected(await member.post("/user/sign_up", {
        username: "orgleaver", email: "orgleaver@example.test", password,
      }));
      const memberRepo = "/orgleaver/member-repo";
      await member.get("/new");
      redirected(await member.post("/repo/create", { name: "member-repo", private: "on" }));
      const orgPath = "/user/organizations/leave-check";
      redirected(await owner.post("/user/organizations", { slug: "leave-check" }));
      redirected(await owner.post(orgPath, { action: "add_member", username: "orgleaver" }));
      const orgId = (await member.get(`${memberRepo}/settings/organization`)).body
        .match(/<option value="(\d+)">leave-check</)?.[1];
      assert.ok(orgId);
      redirected(await member.post(`${memberRepo}/settings/organization`, { action: "offer", organization_id: orgId }));
      const pendingBefore = (await owner.get(orgPath)).body;
      assert.match(pendingBefore, /orgleaver\/member-repo/);
      // Member voluntarily leaves the organization; pending repo offer is cleared.
      assert.match((await member.get(orgPath)).body, /name="action" value="leave"/);
      redirected(await member.post(orgPath, { action: "leave" }), "/user/organizations");
      assert.equal((await member.get(orgPath)).status, 404);
      assert.doesNotMatch((await owner.get(orgPath)).body, /orgleaver\/member-repo/);
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
