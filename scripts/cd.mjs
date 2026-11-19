import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { createRequire } from "node:module";
import { setTimeout } from "node:timers/promises";
import { parseArgs } from "node:util";

const require = createRequire(import.meta.url);
const nilUuid = "00000000-0000-0000-0000-000000000000";
const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" };
// Each command selects an explicit target from the same configuration.
delete env.CLOUDFLARE_ENV;
delete env.WRANGLER_CI_OVERRIDE_NAME;

function node(args, capture = false) {
  return execFileSync(process.execPath, args, {
    env, stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit", encoding: "utf8",
    maxBuffer: 4 * 1024 * 1024, timeout: capture ? 30_000 : undefined,
  });
}

function target(name, config) {
  const db = config.d1_databases.find((binding) => binding.binding === "DB");
  const r2 = config.r2_buckets.find((binding) => binding.binding === "LFS_OBJECTS");
  const artifacts = config.artifacts.find((binding) => binding.binding === "ARTIFACTS");
  const workflow = config.workflows.find((binding) => binding.binding === "RELEASE_VERIFICATION_WORKFLOW");
  const copy = config.workflows.find((binding) => binding.binding === "REPOSITORY_COPY_WORKFLOW");
  if (!config.name || !db?.database_name || !r2?.bucket_name || !artifacts?.namespace || !workflow || !copy ||
      config.version_metadata?.binding !== "SHARD_VERSION" ||
      !config.containers?.some(({ class_name }) => class_name === "ArtifactsMergeContainer")) {
    throw new Error(`Wrangler environment '${name}' must configure Shard's managed release bindings.`);
  }
  const id = db.database_id?.toLowerCase();
  return {
    name, worker: config.name, database: db.database_name, uuid: id === nilUuid ? undefined : id,
    bucket: r2.bucket_name, namespace: artifacts.namespace, workflow: workflow.name,
    workflows: [workflow.name, copy.name],
    routes: [config.route, ...(config.routes ?? [])].filter(Boolean)
      .map((route) => typeof route === "string" ? route : route.pattern),
  };
}

async function verifyRelease(wrangler, flags, deployment, tag) {
  const id = `release-${tag}-${randomUUID().slice(0, 8)}`;
  const json = (args) => JSON.parse(node([wrangler, ...args, ...flags, "--json"], true));
  const start = json(["workflows", "trigger", deployment.workflow, JSON.stringify({ tag }), "--id", id]);
  if (start.id !== id) throw new Error(`Unexpected verification instance for ${deployment.worker}.`);
  console.log(`Verifying ${deployment.worker} in ${deployment.workflow}/${id}`);
  const deadline = Date.now() + 3 * 60_000;
  do {
    const result = json(["workflows", "instances", "describe", deployment.workflow, id]);
    if (result.status === "complete") {
      if (result.output !== tag && result.output !== JSON.stringify(tag)) {
        throw new Error(`${deployment.worker} verified a different revision.`);
      }
      console.log(`Verified ${deployment.worker} at ${tag}`);
      return;
    }
    if (!["queued", "running", "waiting", "waitingForPause"].includes(result.status)) {
      throw new Error(`Release verification ended with status '${result.status}' for ${deployment.worker}. Inspect ${id} in Workflows.`);
    }
    await setTimeout(4_000);
  } while (Date.now() < deadline);
  throw new Error(`Timed out verifying ${deployment.worker}. Inspect ${id} in Workflows.`);
}

async function main() {
  const { values } = parseArgs({ options: {
    config: { type: "string", short: "c" },
    env: { type: "string", short: "e", default: "managed" },
    "staging-env": { type: "string", default: "managed-staging" },
    "dry-run": { type: "boolean" },
    help: { type: "boolean", short: "h" },
  } });
  if (values.help) {
    console.log("npm run cd -- [--config PATH] [--staging-env NAME] [--env NAME] [--dry-run]");
    return;
  }
  const { experimental_readRawConfig, unstable_readConfig } = require("wrangler");
  const { rawConfig, configPath } = experimental_readRawConfig({ config: values.config });
  if (!configPath) throw new Error("Provide a Wrangler configuration with --config.");
  const config = resolve(configPath);
  const environments = [values["staging-env"], values.env];
  const [staging, production] = environments.map((name) => {
    if (!Object.hasOwn(rawConfig.env ?? {}, name)) throw new Error(`Unknown Wrangler environment '${name}'.`);
    return target(name, unstable_readConfig({ config, env: name }));
  });
  const flags = (deployment) => ["--config", config, "--env", deployment.name];
  const deploy = (deployment, args) => node(["scripts/deploy.mjs", ...flags(deployment), ...args]);
  if (["worker", "database", "bucket", "namespace"].some((key) => staging[key] === production[key]) ||
      new Set([...staging.workflows, ...production.workflows]).size !== 4 ||
      staging.uuid && staging.uuid === production.uuid || staging.routes.some((route) => production.routes.includes(route))) {
    throw new Error("Staging and production need separate Workers, databases, buckets, namespaces, Workflows and routes.");
  }
  if (values["dry-run"]) {
    for (const deployment of [staging, production]) deploy(deployment, ["--dry-run"]);
    return;
  }

  if (env.WORKERS_CI === "1" && env.WORKERS_CI_BRANCH !== (env.SHARD_RELEASE_BRANCH ?? "main")) {
    throw new Error("Workers Builds must run the release command on the configured production branch.");
  }
  const tag = execFileSync("git", ["rev-parse", "--verify", "HEAD"], { encoding: "utf8" }).trim();
  if (!/^[0-9a-f]{40}$/.test(tag) || env.WORKERS_CI === "1" && env.WORKERS_CI_COMMIT_SHA !== tag) {
    throw new Error("The checkout SHA differs from the Workers Builds revision.");
  }
  const wrangler = join(dirname(require.resolve("wrangler/package.json")), "bin", "wrangler.js");
  // Compare actual UUIDs when only one environment pins an existing database.
  if (Boolean(staging.uuid) !== Boolean(production.uuid)) {
    const named = staging.uuid ? production : staging;
    const pinned = staging.uuid ?? production.uuid;
    const databases = JSON.parse(node([wrangler, "d1", "list", "--json", ...flags(named)], true));
    if (databases.some((db) => db.name === named.database && db.uuid.toLowerCase() === pinned)) {
      throw new Error("The staging and production DB bindings resolve to the same D1 database.");
    }
  }
  for (const deployment of [staging, production]) {
    console.log(`Deploying ${tag} to ${deployment.worker}`);
    deploy(deployment, ["--tag", tag]);
    await verifyRelease(wrangler, flags(deployment), deployment, tag);
  }
}

try {
  await main();
} catch (error) {
  console.error(`Shard CD: ${error.message}`);
  process.exitCode = 1;
}
