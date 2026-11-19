import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { createRequire } from "node:module";
import { setTimeout } from "node:timers/promises";
import { parseArgs } from "node:util";

const require = createRequire(import.meta.url);
const nilUuid = "00000000-0000-0000-0000-000000000000";
const shaPattern = /^[0-9a-f]{40}$/;
const uuidPattern = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;
const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" };
// Each command selects an explicit target from the same configuration.
delete env.CLOUDFLARE_ENV;
delete env.WRANGLER_CI_OVERRIDE_NAME;

function node(args, capture = false, childEnv = env, stderr = "inherit") {
  return execFileSync(process.execPath, args, {
    env: childEnv, stdio: capture ? ["ignore", "pipe", stderr] : "inherit", encoding: "utf8",
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

async function verifyRelease(wrangler, flags, deployment, tag, versionId) {
  const id = `release-${tag}-${randomUUID().slice(0, 8)}`;
  const json = (args) => JSON.parse(node([wrangler, ...args, ...flags, "--json"], true));
  const params = JSON.stringify({ tag, ...(versionId && { versionId }) });
  const start = json(["workflows", "trigger", deployment.workflow, params, "--id", id]);
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

function parseWranglerJson(output, command) {
  try {
    return JSON.parse(output);
  } catch (error) {
    throw new Error(`Invalid JSON from Wrangler ${command}.`, { cause: error });
  }
}

function deploymentHistory(wrangler, flags) {
  let output;
  try {
    output = node([wrangler, "deployments", "list", ...flags, "--json"], true, env, "pipe");
  } catch (error) {
    // A Worker that has never been uploaded may be absent from the deployments API.
    if (error.status > 0 && /\[code: 10007\]/.test(error.stderr)) return [];
    throw new Error(`Could not read production Worker deployments. ${error.message}`, { cause: error });
  }
  // Wrangler 4.164 emits an array ordered by created_on, oldest first.
  const deployments = parseWranglerJson(output, "deployments list");
  if (!Array.isArray(deployments) || deployments.some((entry, index) =>
    typeof entry?.id !== "string" || !uuidPattern.test(entry.id) || typeof entry.created_on !== "string" ||
    !Number.isFinite(Date.parse(entry.created_on)) || (index > 0 && deployments[index - 1].created_on > entry.created_on) ||
    !Array.isArray(entry.versions) || !entry.versions.length ||
    entry.versions.some((version) => typeof version?.version_id !== "string" || !uuidPattern.test(version.version_id) ||
      !Number.isFinite(version.percentage) || version.percentage < 0 || version.percentage > 100))) {
    throw new Error("Unexpected Wrangler deployments list JSON schema.");
  }
  return deployments;
}

function fullTrafficVersion(deployment) {
  const versions = deployment?.versions;
  return versions?.length === 1 && versions[0].percentage === 100 ? versions[0].version_id : undefined;
}

function readVersion(wrangler, flags, id) {
  const version = parseWranglerJson(node([wrangler, "versions", "view", id, ...flags, "--json"], true), "versions view");
  const annotations = version?.annotations;
  if (version?.id !== id || (annotations != null && (typeof annotations !== "object" || Array.isArray(annotations))) ||
      ["workers/tag", "workers/message"].some((key) => Object.hasOwn(annotations ?? {}, key) && typeof annotations[key] !== "string")) {
    throw new Error(`Unexpected Wrangler versions view JSON schema for ${id}.`);
  }
  return version;
}

async function recoverProduction(wrangler, flags, production, previous, tag, releaseId) {
  const history = deploymentHistory(wrangler, flags);
  const active = history.at(-1);
  if (active?.id === previous?.id) {
    return "Production activation did not change. No Worker rollback was needed.";
  }
  const releaseMessage = `Shard CD release ${releaseId}`;
  const belongsToRelease = (deployment) => {
    const id = fullTrafficVersion(deployment);
    const deploymentAnnotations = deployment?.annotations;
    // The script upload API activates the version immediately. The separate deployments API copies the message.
    if (!id || (deploymentAnnotations?.["workers/triggered_by"] !== "upload" &&
        deploymentAnnotations?.["workers/message"] !== releaseMessage)) return false;
    const { annotations } = readVersion(wrangler, flags, id);
    return annotations?.["workers/tag"] === tag && annotations["workers/message"] === releaseMessage;
  };
  if (!belongsToRelease(active)) {
    return "Production has an unrelated deployment. Its active Worker version was left in place.";
  }
  if (!previous) throw new Error("There is no previous active Worker version to restore.");
  const versionId = fullTrafficVersion(previous);
  if (!versionId) throw new Error("The previous deployment split traffic. Automatic rollback requires one previous active Worker version.");
  const snapshotIndex = history.findIndex(({ id }) => id === previous.id);
  // An upload retry may create more than one deployment. Never cross another publisher's deployment.
  if (snapshotIndex < 0 || history.slice(snapshotIndex + 1).some((entry, index) =>
    entry.created_on <= history[snapshotIndex + index].created_on || (entry !== active && !belongsToRelease(entry)))) {
    return "Production deployment history changed since the snapshot. Its active Worker version was left in place.";
  }

  const version = readVersion(wrangler, flags, versionId);
  const previousTag = version.annotations?.["workers/tag"];
  const bindings = version.resources?.bindings;
  const supportsVerification = shaPattern.test(previousTag) && Array.isArray(bindings) && bindings.some((binding) =>
    binding?.type === "workflow" && binding.name === "RELEASE_VERIFICATION_WORKFLOW" &&
    binding.workflow_name === production.workflow && (!binding.script_name || binding.script_name === production.worker));
  // Cloudflare's rollback API has no expected-active option. Refresh immediately before writing.
  if (deploymentHistory(wrangler, flags).at(-1)?.id !== active.id) {
    return "Production changed while preparing rollback. Its active Worker version was left in place.";
  }
  const message = `Shard CD rollback ${releaseId}`;
  console.error(`Rolling back ${production.worker} to Worker version ${versionId}.`);
  node([wrangler, "rollback", versionId, "--yes", "--message", message, ...flags]);
  const restored = deploymentHistory(wrangler, flags).at(-1);
  if (restored?.id === active.id || fullTrafficVersion(restored) !== versionId || restored.annotations?.["workers/message"] !== message) {
    throw new Error(`Could not confirm rollback to Worker version ${versionId}.`);
  }
  if (!supportsVerification) {
    return `Restored Worker version ${versionId}. Its SHA tag and release Workflow binding could not both be confirmed for verification.`;
  }
  try {
    await verifyRelease(wrangler, flags, production, previousTag, versionId);
    if (deploymentHistory(wrangler, flags).at(-1)?.id !== restored.id) {
      throw new Error("Production deployment changed during verification.");
    }
  } catch (error) {
    throw new Error(`Restored Worker version ${versionId}, but rollback verification failed. ${error.message}`, { cause: error });
  }
  return `Restored and verified Worker version ${versionId} (${previousTag}).`;
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
  // Workers Builds validates the connected production Worker's tag on upload.
  const stagingEnv = { ...env };
  delete stagingEnv.WRANGLER_CI_MATCH_TAG;
  const deploy = (deployment, args) => node(["scripts/deploy.mjs", ...flags(deployment), ...args],
    false, deployment === staging ? stagingEnv : env);
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
  if (!shaPattern.test(tag) || env.WORKERS_CI === "1" && env.WORKERS_CI_COMMIT_SHA !== tag) {
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
  console.log(`Deploying ${tag} to ${staging.worker}`);
  deploy(staging, ["--tag", tag]);
  await verifyRelease(wrangler, flags(staging), staging, tag);

  const productionFlags = flags(production);
  const previous = deploymentHistory(wrangler, productionFlags).at(-1);
  const releaseId = randomUUID();
  try {
    console.log(`Deploying ${tag} to ${production.worker}`);
    deploy(production, ["--tag", tag, "--message", `Shard CD release ${releaseId}`]);
    await verifyRelease(wrangler, productionFlags, production, tag);
  } catch (error) {
    console.error(`Production release failed. ${error.message}`);
    console.error("Any D1 migrations applied by this release remain in place.");
    try {
      console.error(await recoverProduction(wrangler, productionFlags, production, previous, tag, releaseId));
    } catch (recoveryError) {
      console.error(`Production recovery failed. ${recoveryError.message}`);
    }
    process.exitCode = 1;
  }
}

try {
  await main();
} catch (error) {
  console.error(`Shard CD: ${error.message}`);
  process.exitCode = 1;
}
