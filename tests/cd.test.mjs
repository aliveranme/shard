import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const staging = "managed-staging";
const production = "managed";
const productionWorkerTag = "production-worker-tag";

function target(environment, databaseId) {
  const name = `shard-${environment}`;
  return {
    name,
    d1_databases: [{ binding: "DB", database_name: name, database_id: databaseId }],
    r2_buckets: [{ binding: "LFS_OBJECTS", bucket_name: `${name}-lfs` }],
    artifacts: [{ binding: "ARTIFACTS", namespace: name }],
    workflows: [
      { binding: "REPOSITORY_COPY_WORKFLOW", name: `${name}-copy` },
      { binding: "RELEASE_VERIFICATION_WORKFLOW", name: `${name}-release` },
    ],
    version_metadata: { binding: "SHARD_VERSION" },
    containers: [{ class_name: "ArtifactsMergeContainer" }],
  };
}

function exercise(t, { branch = "main", stagingResult } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "shard-cd-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")));
  Object.assign(env, { GIT_CONFIG_GLOBAL: join(dir, "gitconfig"), GIT_CONFIG_NOSYSTEM: "1" });
  const git = (...args) => execFileSync("git", args, { cwd: dir, env, encoding: "utf8", timeout: 10_000 });
  git("init", "--initial-branch=main", "--object-format=sha1", "--template=");
  git("-c", "user.name=Shard CD Fixture", "-c", "user.email=cd@example.test",
    "commit", "--allow-empty", "--no-gpg-sign", "-m", "Release fixture");
  const tag = git("rev-parse", "HEAD").trim();

  const cli = join(dir, "node_modules", "wrangler");
  const config = join(dir, "wrangler.json");
  const log = join(dir, "calls.jsonl");
  const verified = { status: "complete", output: JSON.stringify(tag) };
  const results = { [staging]: { ...verified, ...stagingResult }, [production]: verified };
  mkdirSync(join(cli, "bin"), { recursive: true });
  mkdirSync(join(dir, "scripts"));
  for (const script of ["cd.mjs", "deploy.mjs"]) {
    copyFileSync(new URL(`../scripts/${script}`, import.meta.url), join(dir, "scripts", script));
  }
  writeFileSync(config, JSON.stringify({ env: {
    [staging]: target(staging, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
    [production]: target(production, "11111111-2222-3333-4444-555555555555"),
  } }));
  writeFileSync(log, "");
  writeFileSync(join(cli, "package.json"), '{"main":"index.cjs"}');
  writeFileSync(join(cli, "index.cjs"), `
    const { readFileSync } = require("node:fs");
    const read = (config) => JSON.parse(readFileSync(config, "utf8"));
    exports.experimental_readRawConfig = ({ config }) => ({ rawConfig: read(config), configPath: config });
    exports.unstable_readConfig = ({ config, env }) => read(config).env[env];
  `);
  writeFileSync(join(cli, "bin", "wrangler.js"), `
    const { appendFileSync } = require("node:fs");
    const args = process.argv.slice(2);
    const option = (name) => args[args.indexOf(name) + 1];
    const env = option("--env");
    const call = { env, args: args.slice(0, args.indexOf("--config")) };
    if (args[0] === "deploy") {
      call.matchTag = process.env.WRANGLER_CI_MATCH_TAG;
      call.overrideName = process.env.WRANGLER_CI_OVERRIDE_NAME;
    }
    appendFileSync(${JSON.stringify(log)}, JSON.stringify(call) + "\\n");
    if (args[0] === "workflows") {
      const result = args[1] === "trigger" ? { id: option("--id") } : ${JSON.stringify(results)}[env];
      process.stdout.write(JSON.stringify(result));
    }
  `);
  const result = spawnSync(process.execPath, [join(dir, "scripts", "cd.mjs"), "--config", config], {
    cwd: dir, encoding: "utf8", timeout: 20_000,
    env: {
      ...env, WORKERS_CI: "1", SHARD_RELEASE_BRANCH: "main", WORKERS_CI_BRANCH: branch, WORKERS_CI_COMMIT_SHA: tag,
      WRANGLER_CI_MATCH_TAG: productionWorkerTag, WRANGLER_CI_OVERRIDE_NAME: `shard-${production}`,
    },
  });
  assert.ifError(result.error);
  const calls = readFileSync(log, "utf8").split("\n").filter(Boolean).map(JSON.parse);
  return { result, calls, tag };
}

function assertReleaseCalls(calls, tag, targets) {
  assert.equal(calls.length, targets.length * 4, JSON.stringify(calls));
  for (const [index, env] of targets.entries()) {
    const attempts = calls.slice(index * 4, (index + 1) * 4);
    const workflow = `shard-${env}-release`;
    const instance = attempts[2].args.at(-1);
    assert.ok(instance);
    assert.deepEqual(attempts, [
      ["d1", "migrations", "apply", "DB", "--remote"],
      ["deploy", "--tag", tag],
      ["workflows", "trigger", workflow, JSON.stringify({ tag }), "--id", instance],
      ["workflows", "instances", "describe", workflow, instance],
    ].map((args) => ({ env, args, ...(env === production && args[0] === "deploy" ? { matchTag: productionWorkerTag } : {}) })));
  }
}

test("managed CD gates production on staging verification", async (t) => {
  for (const scenario of [
    {
      name: "a failed staging Workflow stops before production",
      stagingResult: { status: "errored" },
      targets: [staging],
      error: /verification ended with status 'errored' for shard-managed-staging/,
    },
    {
      name: "a different staging revision stops before production",
      stagingResult: { output: JSON.stringify("f".repeat(40)) },
      targets: [staging],
      error: /shard-managed-staging verified a different revision/,
    },
    {
      name: "a non-production Workers Builds branch skips both environments",
      branch: "feature/example",
      targets: [],
      error: /must run the release command on the configured production branch/,
    },
    {
      name: "matching staging verification precedes deployment and verification of production",
      targets: [staging, production],
    },
  ]) {
    await t.test(scenario.name, (t) => {
      const { result, calls, tag } = exercise(t, scenario);
      assert.equal(result.status, scenario.error ? 1 : 0, result.stderr || result.stdout);
      if (scenario.error) assert.match(result.stderr, scenario.error);
      assertReleaseCalls(calls, tag, scenario.targets);
    });
  }
});
