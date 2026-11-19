import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

const account = "aa11bb22cc33dd44ee55ff6677889900";
const uuid = "11111111-2222-3333-4444-555555555555";
const accessName = "SHARD_LFS_R2_ACCESS_KEY_ID";
const secretName = "SHARD_LFS_R2_SECRET_ACCESS_KEY";
const credentials = { [accessName]: "fixture-access-key", [secretName]: "fixture-$secret-key" };
const source = readFileSync(new URL("../scripts/deploy.mjs", import.meta.url));

function managed(prefix) {
  return {
    vars: { PUBLIC_ORIGIN: `https://${prefix}.example.com` },
    d1_databases: [{ binding: "DB", database_name: `${prefix}-db`, database_id: uuid }],
    r2_buckets: [{ binding: "LFS_OBJECTS", bucket_name: `${prefix}-lfs`, preview_bucket_name: `${prefix}-preview` }],
    artifacts: [{ binding: "ARTIFACTS", namespace: `${prefix}-git` }],
  };
}

function exercise(t, {
  args = ["--paid"],
  accountId = account,
  secrets = JSON.stringify(credentials),
  configure = () => {},
  deployStatus = 0,
  workerOverride,
  workersBuilds = false,
  workersCommit,
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "shard-deploy-paid-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const cli = join(dir, "node_modules", "wrangler");
  const configPath = join(dir, "custom.json");
  const secretsPath = join(dir, "provided-secrets");
  const temp = join(dir, "temp");
  const log = join(dir, "calls.jsonl");
  const raw = {
    name: "free",
    keep_vars: true,
    assets: { directory: "./static", binding: "ASSETS" },
    vars: { GIT_BACKEND: "durable", SHARD_LIMIT_PROFILE: "free" },
    d1_databases: [{ binding: "DB", database_name: "free-db", database_id: uuid }],
    env: {
      managed: managed("production"),
      "managed-staging": managed("staging"),
      native: {
        name: "native",
        vars: { GIT_BACKEND: "native", SHARD_LIMIT_PROFILE: "free" },
        d1_databases: [{ binding: "DB", database_name: "native-db", database_id: uuid }],
      },
    },
  };
  configure(raw);
  mkdirSync(join(cli, "bin"), { recursive: true });
  mkdirSync(temp);
  writeFileSync(join(dir, "deploy.mjs"), source);
  writeFileSync(configPath, JSON.stringify(raw));
  writeFileSync(log, "");
  if (typeof secrets === "string") writeFileSync(secretsPath, secrets);
  writeFileSync(join(cli, "package.json"), '{"main":"index.cjs"}');
  writeFileSync(join(cli, "index.cjs"), `exports.experimental_readRawConfig = ({ config }) => ({
    rawConfig: JSON.parse(require("node:fs").readFileSync(config, "utf8")), configPath: config,
  });`);
  writeFileSync(join(cli, "bin", "wrangler.js"), `
    const fs = require("node:fs");
    const args = process.argv.slice(2);
    const flag = (name) => args.includes(name) ? args[args.indexOf(name) + 1] : undefined;
    const configPath = flag("--config");
    const environment = flag("--env");
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
    const target = environment ? config.env[environment] : config;
    const secretsPath = flag("--secrets-file");
    let secretsMatch;
    if (secretsPath) {
      const data = JSON.parse(fs.readFileSync(secretsPath, "utf8"));
      const credentials = ${JSON.stringify(credentials)};
      secretsMatch = Object.entries(credentials).every(([key, value]) => data[key] === value);
    }
    fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({
      args, environment, configPath, config, secretsPath, secretsMatch,
      sanitize: process.env.WRANGLER_LOG_SANITIZE, ci: process.env.CI, workerOverride: process.env.WRANGLER_CI_OVERRIDE_NAME,
    }) + "\\n");
    switch (args.slice(0, 2).join(" ")) {
      case "d1 list":
        process.stdout.write("[]");
        break;
      case "d1 create":
        target.d1_databases[0].database_id = ${JSON.stringify(uuid)};
        fs.writeFileSync(configPath, JSON.stringify(config));
        break;
      case "d1 migrations":
        if (target.d1_databases[0].database_id !== ${JSON.stringify(uuid)}) process.exitCode = 23;
        break;
      default:
        process.exitCode = args[0] === "deploy" ? ${deployStatus} : 19;
    }
  `);
  const env = { ...process.env, TMPDIR: temp, TMP: temp, TEMP: temp, WRANGLER_LOG_SANITIZE: "false" };
  delete env.CLOUDFLARE_ENV;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  delete env.WRANGLER_CI_OVERRIDE_NAME;
  delete env.WORKERS_CI;
  delete env.WORKERS_CI_COMMIT_SHA;
  if (workersBuilds) env.WORKERS_CI = "1";
  if (workersCommit !== undefined) env.WORKERS_CI_COMMIT_SHA = workersCommit;
  if (workerOverride !== undefined) env.WRANGLER_CI_OVERRIDE_NAME = workerOverride;
  if (accountId !== null) env.CLOUDFLARE_ACCOUNT_ID = accountId;
  env[secretName] = "different-shell-value";
  const command = [join(dir, "deploy.mjs"), "--config", configPath, ...args];
  if (secrets !== null) command.push("--secrets-file", secretsPath);
  const result = spawnSync(process.execPath, command, { cwd: dir, env, encoding: "utf8" });
  assert.ifError(result.error);
  const trace = readFileSync(log, "utf8");
  const calls = trace.split("\n").filter(Boolean).map(JSON.parse);
  for (const value of Object.values(credentials)) {
    assert.equal(`${result.stdout}${result.stderr}${trace}`.includes(value), false);
  }
  assert.equal(readFileSync(configPath, "utf8"), JSON.stringify(raw));
  assert.equal(readdirSync(dir).some((name) => name.startsWith(".wrangler.deploy-")), false);
  assert.deepEqual(readdirSync(temp), []);
  if (typeof secrets === "string") assert.equal(readFileSync(secretsPath, "utf8"), secrets);
  const commands = calls.map(({ args }) => args[0] === "d1" ? args.slice(0, 2).join(" ") : args[0]);
  return { ...result, calls, commands, raw, configPath, secretsPath };
}

test("paid deployment selects managed bindings and isolates the derived preset", (t) => {
  const result = exercise(t, { args: ["--paid", "--tag", "revision"] });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
  const [migrate, deploy] = result.calls;
  assert.deepEqual(migrate.args.slice(0, 5), ["d1", "migrations", "apply", "DB", "--remote"]);
  assert.equal(migrate.ci, "true");
  assert.equal(deploy.args[deploy.args.indexOf("--tag") + 1], "revision");
  assert.equal(deploy.configPath, migrate.configPath);
  assert.equal(migrate.secretsPath, undefined);
  assert.notEqual(deploy.secretsPath, result.secretsPath);
  assert.equal(deploy.secretsMatch, true);
  for (const call of result.calls) {
    assert.equal(call.environment, "managed");
    assert.equal(call.sanitize, "true");
    assert.notEqual(call.configPath, result.configPath);
    assert.equal(dirname(call.configPath), dirname(result.configPath));
    assert.equal(call.config.keep_vars, true);
    assert.deepEqual(call.config.vars, result.raw.vars);
    assert.deepEqual(call.config.assets, result.raw.assets);
    assert.deepEqual(call.config.env["managed-staging"], result.raw.env["managed-staging"]);
    const target = call.config.env.managed;
    assert.equal(target.account_id, account);
    assert.deepEqual(target.limits, { cpu_ms: 300000, subrequests: 20000 });
    assert.deepEqual(target.vars, {
      ...result.raw.env.managed.vars,
      SHARD_LIMIT_PROFILE: "paid",
      SHARD_LFS_UPLOAD_MODE: "r2-multipart",
      SHARD_LFS_R2_BUCKET: "production-lfs",
      SHARD_LFS_R2_ENDPOINT: `https://${account}.r2.cloudflarestorage.com`,
      SHARD_GIT_ARTIFACTS_ORIGINS: `https://${account}.artifacts.cloudflare.net`,
    });
  }
});

test("paid staging accepts dotenv and its configured jurisdiction and lower cap", (t) => {
  const result = exercise(t, {
    args: ["--paid", "--env", "managed-staging"],
    secrets: Object.entries(credentials).map(([name, value]) => `${name}='${value}'`).join("\n"),
    configure(raw) {
      const stage = raw.env["managed-staging"];
      stage.r2_buckets[0].jurisdiction = "eu";
      stage.vars.SHARD_LFS_R2_ENDPOINT = `https://${account}.eu.r2.cloudflarestorage.com/`;
      stage.vars.SHARD_LFS_MAX_BYTES = 6500000000;
      stage.limits = { cpu_ms: 60000 };
      delete stage.d1_databases[0].database_id;
    },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.commands, ["d1 list", "d1 create", "d1 migrations", "deploy"]);
  for (const call of result.calls) {
    assert.equal(call.environment, "managed-staging");
    assert.deepEqual(call.config.env.managed, result.raw.env.managed);
    const stage = call.config.env["managed-staging"];
    assert.equal(stage.vars.SHARD_LFS_R2_ENDPOINT, `https://${account}.eu.r2.cloudflarestorage.com`);
    assert.equal(stage.vars.SHARD_LFS_R2_BUCKET, "staging-lfs");
    assert.equal(stage.vars.SHARD_LFS_MAX_BYTES, "6500000000");
    assert.deepEqual(stage.limits, { cpu_ms: 60000, subrequests: 20000 });
  }
  assert.equal(result.calls.at(-1).secretsMatch, true);
});

test("paid preflight rejects invalid inputs before invoking Wrangler", async (t) => {
  for (const [name, options, error] of [
    ["missing account", { accountId: null }, /CLOUDFLARE_ACCOUNT_ID/],
    ["malformed account", { accountId: "invalid" }, /CLOUDFLARE_ACCOUNT_ID/],
    ["account mismatch", { configure: (raw) => { raw.account_id = "a".repeat(32); } }, /must match.*account_id/],
    ["missing file option", { secrets: null }, /Provide --secrets-file/],
    ["unreadable file", { secrets: false }, /Could not read --secrets-file/],
    ["missing key", { secrets: JSON.stringify({ [accessName]: credentials[accessName] }) }, new RegExp(secretName)],
    ["empty key", { secrets: JSON.stringify({ ...credentials, [secretName]: " " }) }, new RegExp(secretName)],
    ["malformed JSON", { secrets: `{${credentials[secretName]}` }, /Could not read --secrets-file/],
    ["conflicting secret", { secrets: JSON.stringify({ ...credentials, SHARD_LFS_UPLOAD_MODE: "proxy" }) }, /must not redefine/],
    ["invalid environment", { args: ["--paid", "--env", "artifacts"] }, /--paid selects/],
    ["missing bucket", { configure: (raw) => { raw.env.managed.r2_buckets = []; } }, /LFS_OBJECTS/],
    ["different bucket", { configure: (raw) => { raw.env.managed.vars.SHARD_LFS_R2_BUCKET = "wrong-bucket"; } }, /must match the LFS_OBJECTS bucket/],
    ["different jurisdiction", { configure: (raw) => { raw.env.managed.vars.SHARD_LFS_R2_ENDPOINT = `https://${account}.eu.r2.cloudflarestorage.com`; } }, /must match.*jurisdiction/],
    ["too large", { configure: (raw) => { raw.env.managed.vars.SHARD_LFS_MAX_BYTES = "5492189429761"; } }, /SHARD_LFS_MAX_BYTES/],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, options);
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, error);
      assert.deepEqual(result.calls, []);
    });
  }
});

test("paid dry run needs no account or secrets and runs only the bundle check", (t) => {
  const result = exercise(t, { args: ["--paid", "--dry-run", "--outdir", "bundle"], accountId: null, secrets: false });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.commands, ["deploy"]);
  const [deploy] = result.calls;
  assert.equal(deploy.environment, "managed");
  assert.equal(deploy.secretsPath, undefined);
  assert.ok(deploy.args.includes("--dry-run"));
  assert.ok(deploy.args.includes("--outdir"));
  assert.equal(deploy.config.env.managed.vars.SHARD_LFS_UPLOAD_MODE, "r2-multipart");
  assert.equal(deploy.config.env.managed.vars.SHARD_LFS_R2_ENDPOINT, undefined);
});

test("the R2 Git preset derives independent Git and LFS bucket endpoints", (t) => {
  const result = exercise(t, {
    args: ["--paid", "--env", "r2"],
    secrets: JSON.stringify({ ...credentials, SHARD_GIT_R2_ACCESS_KEY_ID: "git-fixture-access", SHARD_GIT_R2_SECRET_ACCESS_KEY: "git-fixture-secret" }),
    configure(raw) {
      const config = managed("r2");
      delete config.artifacts;
      config.vars.GIT_BACKEND = "r2";
      config.r2_buckets.push({ binding: "GIT_PACKS", bucket_name: "r2-packs", jurisdiction: "eu" });
      raw.env.r2 = config;
    },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
  const { vars } = result.calls.at(-1).config.env.r2;
  assert.equal(vars.SHARD_GIT_R2_BUCKET, "r2-packs");
  assert.equal(vars.SHARD_GIT_R2_ENDPOINT, `https://${account}.eu.r2.cloudflarestorage.com`);
  assert.equal(vars.SHARD_LFS_R2_ENDPOINT, `https://${account}.r2.cloudflarestorage.com`);
  assert.equal(vars.SHARD_LFS_UPLOAD_MODE, "r2-multipart");
});

test("the native Paid preset enables R2 multipart without Artifacts", (t) => {
  const result = exercise(t, {
    args: ["--paid", "--env", "native"],
    secrets: JSON.stringify({ ...credentials, NATIVE_GIT_TOKEN: "native-origin-fixture" }),
    configure(raw) {
      const native = raw.env.native;
      native.vars.NATIVE_GIT_ORIGIN = "https://git.example.com";
      native.vars.NATIVE_GIT_DIRECT_ORIGIN = "https://git.example.com";
      native.r2_buckets = [{ binding: "LFS_OBJECTS", bucket_name: "native-lfs" }];
    },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
  assert.equal(result.calls.at(-1).secretsMatch, true);
  const config = result.calls.at(-1).config.env.native;
  assert.equal(config.artifacts, undefined);
  assert.deepEqual(config.vars, {
    ...result.raw.env.native.vars,
    SHARD_LIMIT_PROFILE: "paid",
    SHARD_LFS_UPLOAD_MODE: "r2-multipart",
    SHARD_LFS_R2_BUCKET: "native-lfs",
    SHARD_LFS_R2_ENDPOINT: `https://${account}.r2.cloudflarestorage.com`,
  });
  assert.deepEqual(config.limits, { cpu_ms: 300000, subrequests: 20000 });
});

test("a failed paid publish removes both temporary files", (t) => {
  const result = exercise(t, { deployStatus: 7 });
  assert.equal(result.status, 7, result.stderr);
  assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
  assert.match(result.stderr, /Wrangler deploy failed/);
});

test("ordinary deploy and the managed CD invocation keep the original config", async (t) => {
  for (const [name, args, environment] of [
    ["default", [], ""],
    ["native Free", ["--env", "native"], "native"],
    ["managed CD", ["--env", "managed", "--tag", "revision"], "managed"],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, { args, accountId: null, secrets: null });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
      for (const call of result.calls) {
        assert.equal(call.environment, environment);
        assert.equal(call.configPath, result.configPath);
        assert.deepEqual(call.config, result.raw);
      }
    });
  }
});

test("native Paid opt-in preserves bindings and explicit limit overrides", async (t) => {
  for (const [name, dryRun, configured, expected] of [
    ["deploy", false, undefined, { cpu_ms: 300000 }],
    ["dry run", true, { cpu_ms: 60000, subrequests: 5000 }, { cpu_ms: 60000, subrequests: 5000 }],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, {
        args: ["--env", "native", ...(dryRun ? ["--dry-run"] : [])],
        accountId: null,
        secrets: null,
        configure(raw) {
          raw.env.native.vars.SHARD_LIMIT_PROFILE = "paid";
          if (configured) raw.env.native.limits = configured;
        },
      });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.commands, dryRun ? ["deploy"] : ["d1 migrations", "deploy"]);
      for (const call of result.calls) {
        assert.equal(call.environment, "native");
        assert.notEqual(call.configPath, result.configPath);
        assert.deepEqual(call.config, {
          ...result.raw,
          env: { ...result.raw.env, native: { ...result.raw.env.native, limits: expected } },
        });
      }
    });
  }
});

test("a connected Worker name cannot redirect a named deployment", async (t) => {
  for (const [name, args] of [
    ["legacy Artifacts", ["--env", "artifacts"]],
    ["Paid staging", ["--paid", "--env", "managed-staging"]],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, {
        args, workerOverride: "connected-production",
        configure(raw) { raw.env.artifacts = { ...managed("legacy"), name: "legacy" }; },
      });
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /Workers Builds is connected to 'connected-production'.*targets/);
      assert.deepEqual(result.calls, []);
    });
  }
});

test("deployment accepts the connected Worker for the default and matching named targets", (t) => {
  for (const [args, workerOverride] of [[[], "custom-free"], [["--env", "managed"], "free-managed"]]) {
    const result = exercise(t, { args, workerOverride, accountId: null, secrets: null });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
    assert.equal(result.calls[1].workerOverride, workerOverride);
  }
});

test("Workers Builds deploys the initial commit and updates the same private-repo instance", async (t) => {
  const workerName = "private-shard";
  const database = { binding: "DB", database_name: "private-shard-db", database_id: uuid, migrations_dir: "migrations" };
  for (const [name, commit] of [["first deployment", "1".repeat(40)], ["production branch push", "2".repeat(40)]]) {
    await t.test(name, (t) => {
      const result = exercise(t, {
        args: [], secrets: null, workersBuilds: true, workersCommit: commit, workerOverride: workerName,
        configure(raw) {
          raw.name = workerName;
          raw.d1_databases = [database];
        },
      });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.commands, ["d1 migrations", "deploy"]);
      const [migrate, deploy] = result.calls;
      assert.deepEqual(migrate.args.slice(0, 5), ["d1", "migrations", "apply", "DB", "--remote"]);
      assert.deepEqual(deploy.args.slice(0, 3), ["deploy", "--tag", commit]);
      for (const call of result.calls) {
        assert.equal(call.configPath, result.configPath);
        assert.equal(call.config.name, workerName);
        assert.equal(call.workerOverride, workerName);
        assert.deepEqual(call.config.d1_databases, [database]);
      }
    });
  }
});

test("Workers Builds refuses a different configured account before invoking Wrangler", (t) => {
  const result = exercise(t, {
    args: ["--env", "managed-staging"], workersBuilds: true, secrets: null,
    configure(raw) { raw.env["managed-staging"].account_id = "a".repeat(32); },
  });
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /account_id must match the Workers Builds account/);
  assert.deepEqual(result.calls, []);
});
