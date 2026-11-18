import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash, pbkdf2Sync } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
  let worker;
  try {
    const config = JSON.parse((await readFile(join(root, "wrangler.jsonc"), "utf8"))
      .replace(/^\s*\/\/.*$/gm, ""));
    delete config.$schema;
    delete config.build;
    config.name = "shard-http-tests";
    config.main = resolve(root, config.main);
    config.assets.directory = resolve(root, config.assets.directory);
    config.artifacts = config.artifacts.map((binding) => ({ ...binding, remote: false }));
    config.d1_databases = config.d1_databases.map((binding) => ({
      ...binding, remote: false, migrations_dir: resolve(root, binding.migrations_dir),
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
    await cli(["d1", "migrations", "apply", "shard", "--local", "--persist-to", persistence]);
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
    `);
    await cli(["d1", "execute", "shard", "--local", "--persist-to", persistence, "--file", fixture]);
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

    class Client {
      cookies = new Map();
      csrf = "";
      async request(path, options = {}) {
        const headers = new Headers(options.headers);
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

    await t.test("private repositories and Git credentials keep their access boundary", async () => {
      const anonymous = new Client();
      assert.equal((await anonymous.get(privateRepo)).status, 404);
      assert.equal((await viewer.get(privateRepo)).status, 404);
      assert.equal((await owner.get(privateRepo)).status, 200);
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
      await writeFile(join(worktree, "FEATURE.md"), "# A branch with a slash\n");
      await git("-C", worktree, "add", "FEATURE.md");
      await git("-C", worktree, "-c", "core.hooksPath=", "commit", "--no-verify", "-m", "feature note");
      await git("-C", worktree, "remote", "add", "origin", `${base}/suiteowner/history.git`);
      await git("-C", worktree, "push", "origin", "main", "feature/readme");
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
      redirected(await owner.post("/suiteowner/history/pulls/new", {
        title: "Review a slash branch", body: "Review this feature.", base_branch: "main", head_branch: "feature/readme",
      }));
      const pull = await owner.get("/suiteowner/history/pulls/1");
      assert.match(pull.body, /FEATURE\.md/);
      const empty = await owner.post("/suiteowner/history/pulls/1/comments", { body: " " });
      redirected(empty);
      assert.match(empty.location, /^\/suiteowner\/history\/pulls\/1/);
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

    await t.test("scheduled maintenance removes expired credentials", async () => {
      const response = await fetch(`${base}/__scheduled?cron=0+3+*+*+*`);
      assert.equal(response.status, 200);
      await response.text();
      const output = await cli(["d1", "execute", "shard", "--local", "--persist-to", persistence,
        "--json", "--command", `SELECT
          (SELECT COUNT(*) FROM access_tokens WHERE token_hash='${sha(expiredSecret)}') AS tokens,
          (SELECT COUNT(*) FROM sessions WHERE token_hash='${sha(expiredSession)}') AS sessions`]);
      const result = JSON.parse(output).flatMap((item) => item.results)[0];
      assert.equal(result.tokens, 0);
      assert.equal(result.sessions, 0);
    });
  } finally {
    await worker?.stop();
    await rm(temporary, { recursive: true, force: true });
  }
});
