import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const staging = "managed-staging";
const production = "managed";
const productionWorkerTag = "production-worker-tag";
const previousTag = "a".repeat(40);
const previousVersion = "11111111-1111-4111-8111-111111111111";
const releaseVersion = "22222222-2222-4222-8222-222222222222";
const unrelatedVersion = "33333333-3333-4333-8333-333333333333";
const retryVersion = "44444444-4444-4444-8444-444444444444";
const previousDeployment = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const releaseDeployment = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const unrelatedDeployment = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const retryDeployment = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const rollbackDeployment = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const productionWorkflow = `shard-${production}-release`;

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

function exercise(t, scenario = {}) {
  const { branch = "main", stagingResult, productionResult, recoveryResult } = scenario;
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
  const stateFile = join(dir, "state.json");
  const previous = {
    id: previousDeployment, created_on: "2026-01-01T00:00:00Z",
    versions: [{ version_id: previousVersion, percentage: 100 }],
  };
  if (scenario.split) {
    previous.versions = [{ version_id: previousVersion, percentage: 90 }, { version_id: unrelatedVersion, percentage: 10 }];
  }
  writeFileSync(stateFile, JSON.stringify({
    deployments: scenario.first ? [] : [previous],
    versions: {
      [previousVersion]: {
        id: previousVersion, annotations: scenario.legacyTag ? {} : { "workers/tag": previousTag },
        resources: { bindings: scenario.legacyWorkflow ? [] : [
          { name: "RELEASE_VERIFICATION_WORKFLOW", type: "workflow", workflow_name: productionWorkflow },
        ] },
      },
    },
    instances: {}, migrations: {}, historyReads: 0,
  }));
  const verified = { status: "complete", output: JSON.stringify(tag) };
  const results = { [staging]: { ...verified, ...stagingResult }, [production]: { ...verified, ...productionResult } };
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
    const assert = require("node:assert/strict");
    const { appendFileSync, readFileSync, writeFileSync } = require("node:fs");
    const args = process.argv.slice(2);
    const scenario = ${JSON.stringify(scenario)};
    const statePath = ${JSON.stringify(stateFile)};
    const state = JSON.parse(readFileSync(statePath, "utf8"));
    const save = () => writeFileSync(statePath, JSON.stringify(state));
    const reply = (data) => process.stdout.write(JSON.stringify(data));
    const option = (name) => args.includes(name) ? args[args.indexOf(name) + 1] : undefined;
    const env = option("--env");
    const call = { env, args: args.slice(0, args.indexOf("--config")) };
    if (args[0] === "deploy") {
      call.matchTag = process.env.WRANGLER_CI_MATCH_TAG;
      call.overrideName = process.env.WRANGLER_CI_OVERRIDE_NAME;
    }
    appendFileSync(${JSON.stringify(log)}, JSON.stringify(call) + "\\n");
    const isProduction = env === ${JSON.stringify(production)};
    const previousVersion = ${JSON.stringify(previousVersion)};
    const releaseVersion = ${JSON.stringify(releaseVersion)};
    const releaseTag = ${JSON.stringify(tag)};
    const activate = (deploymentId, versionId, trigger, message) => state.deployments.push({
      id: deploymentId, created_on: new Date(Date.UTC(2026, 0, 1, 0, 0, state.deployments.length)).toISOString(),
      versions: [{ version_id: versionId, percentage: 100 }],
      annotations: { "workers/triggered_by": trigger, "workers/message": message },
    });
    const publishUnrelated = () => {
      const id = ${JSON.stringify(unrelatedVersion)};
      state.versions[id] = { id, annotations: { "workers/tag": releaseTag, "workers/message": "different invocation" } };
      activate(${JSON.stringify(unrelatedDeployment)}, id, "upload");
    };
    if (args[0] === "d1") {
      assert.deepEqual(call.args, ["d1", "migrations", "apply", "DB", "--remote"]);
      state.migrations[env] = (state.migrations[env] ?? 0) + 1;
    } else if (args[0] === "deploy" && isProduction) {
      if (scenario.competingRelease === "before") publishUnrelated();
      if (!scenario.skipActivation) {
        state.versions[releaseVersion] = { id: releaseVersion, annotations: {
          "workers/tag": option("--tag"), "workers/message": option("--message"),
        } };
        activate(${JSON.stringify(releaseDeployment)}, releaseVersion, "upload");
        if (scenario.retryUpload) {
          const id = ${JSON.stringify(retryVersion)};
          state.versions[id] = { id, annotations: state.versions[releaseVersion].annotations };
          activate(${JSON.stringify(retryDeployment)}, id, "upload");
        }
      }
      if (scenario.deployError) process.exitCode = 1;
    } else if (args[0] === "deployments") {
      assert.ok(isProduction && args.includes("--json"));
      state.historyReads++;
      if (scenario.competingRelease === "guard" && state.historyReads === 3) publishUnrelated();
      if (scenario.historyErrorAt === state.historyReads) {
        process.stderr.write("Deployment query failed [code: 10000]\\n");
        process.exitCode = 1;
      } else if (scenario.malformedHistoryAt === state.historyReads) {
        reply({ deployments: state.deployments });
      } else if (scenario.missingWorker && !state.deployments.length) {
        process.stderr.write("Worker not found [code: 10007]\\n");
        process.exitCode = 1;
      } else {
        reply(state.deployments);
      }
    } else if (args[0] === "versions") {
      assert.ok(isProduction && args.includes("--json"));
      assert.ok(state.versions[args[2]], "unexpected version ID");
      reply(state.versions[args[2]]);
    } else if (args[0] === "rollback") {
      assert.ok(isProduction && args.includes("--yes"));
      assert.equal(args[1], previousVersion);
      if (scenario.rollbackError) {
        process.exitCode = 1;
      } else if (!scenario.rollbackNoop) {
        activate(${JSON.stringify(rollbackDeployment)}, previousVersion, "rollback", option("--message"));
      }
    }
    if (args[0] === "workflows") {
      if (args[1] === "trigger") {
        const id = option("--id");
        const params = JSON.parse(args[3]);
        if (params.versionId) {
          assert.ok(isProduction);
          assert.equal(params.versionId, previousVersion);
          assert.equal(state.deployments.at(-1).versions[0].version_id, previousVersion);
          assert.equal(state.migrations[env], 1, "recovery reads the once-migrated production database");
        }
        state.instances[id] = params;
        reply({ id });
      } else {
        const params = state.instances[args[4]];
        assert.ok(params, "describe uses the triggered Workflow instance");
        if (isProduction && !params.versionId && scenario.competingRelease === "after") publishUnrelated();
        if (isProduction && !params.versionId && scenario.competingRelease === "promotion") {
          activate(${JSON.stringify(unrelatedDeployment)}, releaseVersion, "promotion");
        }
        reply(params.versionId ? { status: "complete", output: JSON.stringify(params.tag), ...${JSON.stringify(recoveryResult)} }
          : ${JSON.stringify(results)}[env]);
      }
    }
    save();
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
  return { result, calls, tag, state: JSON.parse(readFileSync(stateFile, "utf8")) };
}

function assertReleaseCalls(calls, tag, targets) {
  let offset = 0;
  for (const env of targets) {
    if (env === production) assert.deepEqual(calls[offset++], { env, args: ["deployments", "list"] });
    const attempts = calls.slice(offset, offset + 4);
    const workflow = `shard-${env}-release`;
    const instance = attempts[2].args.at(-1);
    const upload = ["deploy", "--tag", tag];
    if (env === production) {
      const message = attempts[1].args[4];
      assert.match(message, /^Shard CD release [0-9a-f-]{36}$/);
      upload.push("--message", message);
    }
    assert.match(instance, new RegExp(`^release-${tag}-[0-9a-f]{8}$`));
    assert.deepEqual(attempts, [
      ["d1", "migrations", "apply", "DB", "--remote"],
      upload,
      ["workflows", "trigger", workflow, JSON.stringify({ tag }), "--id", instance],
      ["workflows", "instances", "describe", workflow, instance],
    ].map((args) => ({ env, args, ...(env === production && args[0] === "deploy" ? { matchTag: productionWorkerTag } : {}) })));
    offset += 4;
  }
  assert.equal(calls.length, offset, JSON.stringify(calls));
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

test("managed CD restores and verifies the previous production version on failure", async (t) => {
  for (const [name, scenario] of [
    ["a production probe fails after activation", { productionResult: { status: "errored" } }],
    ["Wrangler errors after activating the production upload", { deployError: true }],
    ["an upload retry activates two versions owned by this run", { deployError: true, retryUpload: true }],
  ]) {
    await t.test(name, (t) => {
      const { result, calls, tag, state } = exercise(t, scenario);
      assert.equal(result.status, 1, result.stderr || result.stdout);
      assert.match(result.stderr, /Production release failed/);
      assert.match(result.stderr, /Any D1 migrations applied by this release remain in place/);
      assert.match(result.stderr, new RegExp(`Restored and verified Worker version ${previousVersion}`));
      const rollbackIndex = calls.findIndex(({ args }) => args[0] === "rollback");
      assert.ok(rollbackIndex > 0);
      const releaseMessage = calls.find(({ env, args }) => env === production && args[0] === "deploy").args[4];
      const message = releaseMessage.replace("release", "rollback");
      const instance = calls[rollbackIndex + 2].args.at(-1);
      assert.match(instance, new RegExp(`^release-${previousTag}-[0-9a-f]{8}$`));
      assert.deepEqual(calls.slice(rollbackIndex), [
        ["rollback", previousVersion, "--yes", "--message", message],
        ["deployments", "list"],
        ["workflows", "trigger", productionWorkflow, JSON.stringify({ tag: previousTag, versionId: previousVersion }), "--id", instance],
        ["workflows", "instances", "describe", productionWorkflow, instance],
        ["deployments", "list"],
      ].map((args) => ({ env: production, args })));
      assert.deepEqual(state.migrations, { [staging]: 1, [production]: 1 });
      assert.equal(state.deployments.at(-1).versions[0].version_id, previousVersion);
      assert.equal(Object.values(state.instances).filter((params) => params.tag === tag).length, scenario.deployError ? 1 : 2);
    });
  }
});

test("managed CD leaves unmatched production deployments in place", async (t) => {
  for (const [name, scenario, error, active] of [
    ["an upload fails before activation", { deployError: true, skipActivation: true }, /activation did not change/, previousDeployment],
    ["a first release fails after activation", { first: true, missingWorker: true }, /no previous active Worker version/, releaseDeployment],
    ["prior production traffic was split", { split: true }, /previous deployment split traffic/, releaseDeployment],
    ["an unrelated publish follows activation with the same SHA", { competingRelease: "after" }, /unrelated deployment/, unrelatedDeployment],
    ["another publisher reactivates this run's uploaded version", { competingRelease: "promotion" }, /unrelated deployment/, unrelatedDeployment],
    ["an unrelated publish intervenes after the snapshot", { competingRelease: "before" }, /history changed since the snapshot/, releaseDeployment],
    ["production changes while checking rollback metadata", { competingRelease: "guard" }, /changed while preparing rollback/, unrelatedDeployment],
    ["the recovery command gets an incompatible response", { malformedHistoryAt: 2 }, /Unexpected Wrangler deployments list JSON schema/, releaseDeployment],
  ]) {
    await t.test(name, (t) => {
      const { result, calls, state } = exercise(t, { productionResult: { status: "errored" }, ...scenario });
      assert.equal(result.status, 1, result.stderr || result.stdout);
      assert.match(result.stderr, error);
      assert.equal(calls.some(({ args }) => args[0] === "rollback"), false);
      assert.equal(state.deployments.at(-1).id, active);
    });
  }
});

test("managed CD handles missing and invalid deployment snapshots safely", async (t) => {
  for (const [name, scenario, error] of [
    ["a Worker without deployment history can publish for the first time", { first: true }],
    ["a missing Worker can publish for the first time", { first: true, missingWorker: true }],
    ["an incompatible snapshot stops production", { malformedHistoryAt: 1 }, /Unexpected Wrangler deployments list JSON schema/],
    ["a deployment API error stops production", { historyErrorAt: 1 }, /Could not read production Worker deployments/],
  ]) {
    await t.test(name, (t) => {
      const { result, calls, tag, state } = exercise(t, scenario);
      assert.equal(result.status, error ? 1 : 0, result.stderr || result.stdout);
      if (error) {
        assert.match(result.stderr, error);
        assert.deepEqual(calls.at(-1), { env: production, args: ["deployments", "list"] });
        assertReleaseCalls(calls.slice(0, -1), tag, [staging]);
        assert.deepEqual(state.migrations, { [staging]: 1 });
      } else {
        assertReleaseCalls(calls, tag, [staging, production]);
        assert.equal(state.deployments.at(-1).id, releaseDeployment);
      }
    });
  }
});

test("managed CD reports incomplete production recovery", async (t) => {
  for (const [name, scenario, error, active, recoveryTriggered] of [
    ["the rollback command exits with an error", { rollbackError: true }, /Production recovery failed.*rollback/, releaseDeployment, false],
    ["the rollback command exits successfully without restoring the version", { rollbackNoop: true }, /Could not confirm rollback/, releaseDeployment, false],
    ["the restored Workflow fails its binding checks", { recoveryResult: { status: "errored" } }, /rollback verification failed/, rollbackDeployment, true],
    ["the restored version has no SHA tag", { legacyTag: true }, /could not both be confirmed for verification/, rollbackDeployment, false],
    ["the restored version lacks the release Workflow", { legacyWorkflow: true }, /could not both be confirmed for verification/, rollbackDeployment, false],
  ]) {
    await t.test(name, (t) => {
      const { result, state } = exercise(t, { productionResult: { status: "errored" }, ...scenario });
      assert.equal(result.status, 1, result.stderr || result.stdout);
      assert.match(result.stderr, error);
      assert.equal(state.deployments.at(-1).id, active);
      assert.equal(Object.values(state.instances).some((params) => params.versionId === previousVersion), recoveryTriggered);
    });
  }
});
