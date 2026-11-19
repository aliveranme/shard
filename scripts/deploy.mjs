import { spawnSync } from "node:child_process";
import { accessSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";

const require = createRequire(import.meta.url);
const unsetDatabaseId = "00000000-0000-0000-0000-000000000000";

function deploy() {
  const { values } = parseArgs({ options: {
    config: { type: "string", short: "c" },
    env: { type: "string", short: "e" },
    "secrets-file": { type: "string" },
    "dry-run": { type: "boolean" },
    outdir: { type: "string" },
    help: { type: "boolean", short: "h" },
  } });
  if (values.help) {
    console.log("npm run deploy -- [--config PATH] [--env NAME] [--secrets-file PATH] [--dry-run] [--outdir PATH]");
    return;
  }

  const { experimental_readRawConfig } = require("wrangler");
  const wranglerBin = join(dirname(require.resolve("wrangler/package.json")), "bin", "wrangler.js");
  const environment = values.env ?? process.env.CLOUDFLARE_ENV;
  const { rawConfig, configPath } = experimental_readRawConfig({ config: values.config, env: environment });
  if (!configPath) throw new Error("Provide a Wrangler configuration with --config.");

  if (environment && !Object.hasOwn(rawConfig.env ?? {}, environment)) {
    throw new Error(`Wrangler environment '${environment}' is missing from the configuration.`);
  }
  const target = environment ? rawConfig.env[environment] : rawConfig;
  const database = target.d1_databases?.find(({ binding }) => binding === "DB");
  if (!database) throw new Error("Configure a D1 database bound as DB in the deployment environment.");

  if (values["secrets-file"]) accessSync(resolve(values["secrets-file"]));
  const deployArgs = ["deploy"];
  if (values["secrets-file"]) deployArgs.push("--secrets-file", resolve(values["secrets-file"]));
  if (values.outdir) deployArgs.push("--outdir", resolve(values.outdir));

  let deploymentConfig = resolve(configPath);
  function run(args, { capture = false, ci = false } = {}) {
    const flags = ["--config", deploymentConfig, "--env", environment ?? ""];
    const result = spawnSync(process.execPath, [wranglerBin, ...args, ...flags], {
      encoding: "utf8",
      env: ci ? { ...process.env, CI: "true" } : process.env,
      stdio: capture ? ["inherit", "pipe", "inherit"] : "inherit",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      if (result.stdout) process.stdout.write(result.stdout);
      process.exitCode = result.status ?? 1;
      throw new Error(`Wrangler ${args[0]} failed.`);
    }
    return result.stdout;
  }

  if (values["dry-run"]) {
    run([...deployArgs, "--dry-run"]);
    return;
  }

  const origin = target.vars?.NATIVE_GIT_ORIGIN;
  if (origin !== undefined) {
    const url = URL.parse(origin);
    if (url?.protocol !== "https:" || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
      throw new Error("Use an HTTPS NATIVE_GIT_ORIGIN for deployment, or omit it here to use the Worker's dashboard variable.");
    }
  }
  const lfsBucket = target.vars?.SHARD_LFS_R2_BUCKET;
  const lfsBinding = target.r2_buckets?.find(({ binding }) => binding === "LFS_OBJECTS");
  if (lfsBucket !== undefined && lfsBucket !== lfsBinding?.bucket_name) {
    throw new Error("SHARD_LFS_R2_BUCKET must match the LFS_OBJECTS bucket name in this environment.");
  }

  let temporaryConfig;
  try {
    if (!database.database_id || database.database_id === unsetDatabaseId) {
      const name = database.database_name;
      if (!name) throw new Error("Set database_name on the DB binding for automatic D1 setup.");
      const databases = JSON.parse(run(["d1", "list", "--json"], { capture: true }));
      const existing = databases.find((candidate) => candidate.name === name);
      if (existing) database.database_id = existing.uuid;

      // Keep the copy beside the source so asset, build and migration paths still resolve.
      const copy = join(dirname(deploymentConfig), `.wrangler.deploy-${randomUUID()}.json`);
      writeFileSync(copy, JSON.stringify(rawConfig, null, 2) + "\n", { flag: "wx" });
      temporaryConfig = deploymentConfig = copy;
      if (!existing) run(["d1", "create", name, "--binding", "DB"]);
    }
    const migrationOutput = run(["d1", "migrations", "apply", "DB", "--remote"], { capture: true, ci: true });
    process.stdout.write(migrationOutput);
    // Wrangler 4.164.0 can mark a migration failed without returning an error exit code.
    if (/│[^│\n]+│\s*\u274c/.test(migrationOutput)) {
      throw new Error("D1 reported a failed migration.");
    }
    run(deployArgs);
  } finally {
    if (temporaryConfig) rmSync(temporaryConfig, { force: true });
  }
}

try {
  deploy();
} catch (error) {
  console.error(`Shard deploy: ${error.message}`);
  process.exitCode ||= 1;
}
