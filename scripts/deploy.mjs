import { spawn } from "node:child_process";
import { accessSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parseArgs, parseEnv } from "node:util";

const require = createRequire(import.meta.url);
const unsetDatabaseId = "00000000-0000-0000-0000-000000000000";
const databaseIdPattern = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;
const accountIdPattern = /^[0-9a-f]{32}$/i;
const r2BucketPattern = /^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/;
const r2Jurisdictions = new Set(["eu", "fedramp", "us"]);
const maxR2ObjectBytes = (5 * 1024 - 5) * 1024 ** 3;
const r2SecretNames = ["SHARD_LFS_R2_ACCESS_KEY_ID", "SHARD_LFS_R2_SECRET_ACCESS_KEY"];
const gitSecretNames = ["SHARD_GIT_R2_ACCESS_KEY_ID", "SHARD_GIT_R2_SECRET_ACCESS_KEY"];

function reservedBindingNames(rawConfig, target, vars) {
  const names = new Set([
    "DB", "ARTIFACTS", "LFS_OBJECTS", "GIT_PACKS", "GIT_R2_REPOS",
    "SHARD_GIT_R2_CONTAINER_ENDPOINT", "SHARD_GIT_R2_MAX_PUSH_BYTES", "SHARD_LFS_MAX_BYTES",
    ...Object.keys(vars),
  ]);
  for (const source of [rawConfig, target]) {
    if (!source || typeof source !== "object") continue;
    if (source.assets?.binding) names.add(source.assets.binding);
    if (source.version_metadata?.binding) names.add(source.version_metadata.binding);
    for (const key of ["d1_databases", "r2_buckets", "artifacts", "workflows", "send_email"]) {
      for (const entry of source[key] ?? []) {
        if (entry?.binding) names.add(entry.binding);
        if (entry?.name && key === "send_email") names.add(entry.name);
      }
    }
    for (const entry of source.durable_objects?.bindings ?? []) {
      if (entry?.name) names.add(entry.name);
    }
    for (const entry of source.ratelimits ?? []) {
      if (entry?.name) names.add(entry.name);
    }
  }
  return [...names];
}

function readPaidSecrets(file, rawConfig, target, vars, required) {
  if (!file) throw new Error("Provide --secrets-file with the bucket-scoped R2 credentials for this environment.");
  let secrets;
  try {
    const contents = readFileSync(file, "utf8").trimStart();
    secrets = contents.startsWith("{") || contents.startsWith("[") ? JSON.parse(contents) : parseEnv(contents);
  } catch {
    throw new Error("Could not read --secrets-file as JSON or dotenv.");
  }
  if (!secrets || typeof secrets !== "object" || Array.isArray(secrets) ||
      Object.values(secrets).some((value) => value !== null && typeof value !== "string")) {
    throw new Error("--secrets-file must contain named string values.");
  }
  for (const name of required) {
    if (!Object.hasOwn(secrets, name) || typeof secrets[name] !== "string" || !/^[!-~]+$/.test(secrets[name])) {
      throw new Error(`Provide a nonempty ${name} in --secrets-file.`);
    }
  }
  const reserved = reservedBindingNames(rawConfig, target, vars);
  if (reserved.some((name) => Object.hasOwn(secrets, name) && secrets[name] !== null)) {
    throw new Error("--secrets-file must not redefine paid preset bindings or selected Worker variables.");
  }
  return secrets;
}

function configurePaid(rawConfig, target, lfsBinding, dryRun) {
  const artifacts = target.artifacts?.some(({ binding, namespace }) => binding === "ARTIFACTS" && namespace);
  const r2Git = target.vars?.GIT_BACKEND === "r2";
  if (!artifacts && !r2Git && target.vars?.GIT_BACKEND !== "native") {
    throw new Error("Configure an Artifacts namespace bound as ARTIFACTS in the paid environment.");
  }
  const bucket = lfsBinding?.bucket_name;
  if (typeof bucket !== "string" || !r2BucketPattern.test(bucket)) {
    throw new Error("Configure a named R2 bucket bound as LFS_OBJECTS in the paid environment.");
  }
  const jurisdiction = lfsBinding.jurisdiction;
  if (jurisdiction !== undefined && !r2Jurisdictions.has(jurisdiction)) {
    throw new Error("Set the LFS_OBJECTS jurisdiction to eu, fedramp or us, or omit it for the standard R2 endpoint.");
  }
  const vars = target.vars ??= {};
  if (r2SecretNames.some((name) => Object.hasOwn(vars, name))) {
    throw new Error("Supply the LFS credentials through --secrets-file as Worker secrets.");
  }
  const gitBinding = r2Git ? target.r2_buckets?.find(({ binding }) => binding === "GIT_PACKS") : null;
  if (r2Git) {
    const name = gitBinding?.bucket_name;
    if (typeof name !== "string" || !r2BucketPattern.test(name) || name === bucket) {
      throw new Error("Bind a separate private Git pack bucket as GIT_PACKS in the R2 Git environment.");
    }
    if (gitBinding.jurisdiction !== undefined && !r2Jurisdictions.has(gitBinding.jurisdiction)) {
      throw new Error("Set the GIT_PACKS jurisdiction to eu, fedramp or us, or omit it for the standard R2 endpoint.");
    }
    if (gitSecretNames.some((key) => Object.hasOwn(vars, key)) || vars.SHARD_GIT_R2_CONTAINER_ENDPOINT !== undefined && !dryRun) {
      throw new Error("Use Worker secrets for the Git pack R2 credentials and omit the local container endpoint for deployment.");
    }
    const max = vars.SHARD_GIT_R2_MAX_PUSH_BYTES;
    if (max !== undefined && (!/^\d+$/.test(String(max)) || Number(max) < 4 || Number(max) > maxR2ObjectBytes)) {
      throw new Error(`SHARD_GIT_R2_MAX_PUSH_BYTES must be a whole number between 4 and ${maxR2ObjectBytes}.`);
    }
    if (max !== undefined) vars.SHARD_GIT_R2_MAX_PUSH_BYTES = String(max);
    vars.SHARD_GIT_R2_BUCKET = name;
  }
  const maxBytes = vars.SHARD_LFS_MAX_BYTES;
  if (maxBytes !== undefined) {
    if (!(typeof maxBytes === "string" || typeof maxBytes === "number") ||
        !/^\d+$/.test(String(maxBytes)) || Number(maxBytes) < 1 || Number(maxBytes) > maxR2ObjectBytes) {
      throw new Error(`SHARD_LFS_MAX_BYTES must be a whole number between 1 and ${maxR2ObjectBytes}.`);
    }
    vars.SHARD_LFS_MAX_BYTES = String(maxBytes);
  }
  vars.SHARD_LIMIT_PROFILE = "paid";
  // The preset defaults to multipart, but an explicit mode keeps stock Git LFS clients working.
  vars.SHARD_LFS_UPLOAD_MODE ??= "r2-multipart";
  vars.SHARD_LFS_R2_BUCKET = bucket;
  rawConfig.keep_vars = true;
  // Whole-object hashing consumes CPU, and one LFS page can copy 8 × 1,024 R2 ranges.
  target.limits = { cpu_ms: 300_000, subrequests: 20_000, ...(target.limits ?? rawConfig.limits) };
  if (dryRun) return;

  const account = process.env.CLOUDFLARE_ACCOUNT_ID?.toLowerCase();
  if (!accountIdPattern.test(account ?? "")) {
    throw new Error("Set CLOUDFLARE_ACCOUNT_ID to the 32-digit hexadecimal account ID for the paid deployment.");
  }
  const configuredAccount = target.account_id || rawConfig.account_id;
  if (configuredAccount && String(configuredAccount).toLowerCase() !== account) {
    throw new Error("CLOUDFLARE_ACCOUNT_ID must match the Wrangler account_id for the paid deployment.");
  }
  target.account_id = account;
  const endpoint = `https://${account}${jurisdiction ? `.${jurisdiction}` : ""}.r2.cloudflarestorage.com`;
  const configuredEndpoint = vars.SHARD_LFS_R2_ENDPOINT;
  if (configuredEndpoint !== undefined && configuredEndpoint !== endpoint && configuredEndpoint !== `${endpoint}/`) {
    throw new Error("SHARD_LFS_R2_ENDPOINT must match the account and jurisdiction of the LFS_OBJECTS binding.");
  }
  vars.SHARD_LFS_R2_ENDPOINT = endpoint;
  if (gitBinding) {
    const r2Jurisdiction = gitBinding.jurisdiction;
    const gitEndpoint = `https://${account}${r2Jurisdiction ? `.${r2Jurisdiction}` : ""}.r2.cloudflarestorage.com`;
    if (vars.SHARD_GIT_R2_ENDPOINT !== undefined &&
        ![gitEndpoint, `${gitEndpoint}/`].includes(vars.SHARD_GIT_R2_ENDPOINT)) {
      throw new Error("SHARD_GIT_R2_ENDPOINT must match the account and jurisdiction of the GIT_PACKS binding.");
    }
    vars.SHARD_GIT_R2_ENDPOINT = gitEndpoint;
  }
  if (artifacts) vars.SHARD_GIT_ARTIFACTS_ORIGINS = `https://${account}.artifacts.cloudflare.net`;
}

function readD1Rows(output) {
  const response = JSON.parse(output);
  if (!Array.isArray(response) || response.length !== 1 || response[0]?.success !== true ||
      !Array.isArray(response[0].results)) {
    throw new Error("Unexpected response to the D1 ownership query.");
  }
  return response[0].results;
}

async function verifyExistingDatabase(run, database) {
  const { database_id: id, database_name: name } = database;
  const query = async (sql) => readD1Rows(await run([
    "d1", "execute", "DB", "--remote", "--json", "--command", sql,
  ], { capture: true }));
  const schema = await query("SELECT type, name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' " +
    "AND NOT (type = 'table' AND name = '_cf_KV')");
  if (!schema.every((row) => row && ["table", "index", "view", "trigger"].includes(row.type) &&
      typeof row.name === "string" && row.name && typeof row.sql === "string" && row.sql)) {
    throw new Error("Unexpected schema rows in the D1 ownership query.");
  }
  if (schema.length === 0) return;

  const migrationsTable = database.migrations_table || "d1_migrations";
  const tableSql = (name) => schema.find((row) => row.type === "table" && row.name === name)?.sql ?? "";
  // These distinctive columns and constraints come from 0001_init.sql and survive later migrations.
  if (tableSql(migrationsTable) &&
      /\bartifacts_repo\s+TEXT\s+NOT NULL\s*,/i.test(tableSql("repos")) &&
      /\bnext_issue_number\s+INTEGER\s+NOT NULL\s+DEFAULT\s+1\s*,/i.test(tableSql("repos")) &&
      /\bissue_id\s+INTEGER\s+PRIMARY KEY\s+REFERENCES\s+issues\(id\)\s+ON DELETE CASCADE\s*,/i.test(tableSql("pulls")) &&
      /\bmerge_commit\s+TEXT\b/i.test(tableSql("pulls"))) {
    const identifier = `"${migrationsTable.replaceAll('"', '""')}"`;
    const applied = await query(`SELECT name FROM ${identifier} WHERE name = '0001_init.sql'`);
    if (applied.length === 1 && applied[0]?.name === "0001_init.sql") return;
  }
  throw new Error(`Refusing to migrate D1 database '${name}' (${id}): its nonempty schema could not be verified as Shard.`);
}

async function deploy() {
  const { values } = parseArgs({ options: {
    config: { type: "string", short: "c" },
    env: { type: "string", short: "e" },
    "secrets-file": { type: "string" },
    tag: { type: "string" },
    message: { type: "string" },
    paid: { type: "boolean" },
    "dry-run": { type: "boolean" },
    outdir: { type: "string" },
    help: { type: "boolean", short: "h" },
  } });
  if (values.help) {
    console.log("npm run deploy -- [--paid] [--config PATH] [--env NAME] [--secrets-file PATH] [--tag TAG] [--message MESSAGE] [--dry-run] [--outdir PATH]");
    return;
  }

  const { experimental_readRawConfig } = require("wrangler");
  const wranglerBin = join(dirname(require.resolve("wrangler/package.json")), "bin", "wrangler.js");
  const paid = values.paid;
  const dryRun = values["dry-run"];
  const environment = values.env ?? process.env.CLOUDFLARE_ENV ?? (paid ? "managed" : undefined);
  if (paid && !["managed", "managed-staging", "native", "r2"].includes(environment)) {
    throw new Error("--paid selects managed, managed-staging, native or r2.");
  }
  const { rawConfig, configPath } = experimental_readRawConfig({ config: values.config, env: environment });
  if (!configPath) throw new Error("Provide a Wrangler configuration with --config.");

  if (environment && !Object.hasOwn(rawConfig.env ?? {}, environment)) {
    throw new Error(`Wrangler environment '${environment}' is missing from the configuration.`);
  }
  const target = environment ? rawConfig.env[environment] : rawConfig;
  const nativePaid = environment === "native" && target.vars?.SHARD_LIMIT_PROFILE === "paid";
  if (nativePaid) {
    // The explicit profile is the operator's Workers Paid opt-in for this native deployment.
    target.limits = { cpu_ms: 300_000, ...(target.limits ?? rawConfig.limits) };
  }
  const workerOverride = process.env.WRANGLER_CI_OVERRIDE_NAME;
  const workerName = target.name ?? (rawConfig.name && `${rawConfig.name}-${environment}`);
  if (environment && workerOverride !== undefined && workerOverride !== workerName) {
    throw new Error(`Workers Builds is connected to '${workerOverride}', but Wrangler environment '${environment}' targets '${workerName}'.`);
  }
  const database = target.d1_databases?.find(({ binding }) => binding === "DB");
  if (!database) throw new Error("Configure a D1 database bound as DB in the deployment environment.");

  const secretsFile = values["secrets-file"] ? resolve(values["secrets-file"]) : undefined;
  const deployArgs = ["deploy"];
  if (secretsFile && !paid) {
    accessSync(secretsFile);
    deployArgs.push("--secrets-file", secretsFile);
  }
  if (values.outdir) deployArgs.push("--outdir", resolve(values.outdir));
  const buildsCommit = process.env.WORKERS_CI_COMMIT_SHA ?? "";
  const tag = values.tag ?? (!environment && process.env.WORKERS_CI === "1" &&
    /^[0-9a-f]{40}$/.test(buildsCommit) ? buildsCommit : undefined);
  if (tag) deployArgs.push("--tag", tag);
  if (values.message) deployArgs.push("--message", values.message);

  let deploymentConfig = resolve(configPath);
  let activeChild = null;
  function run(args, { capture = false, ci = false } = {}) {
    const flags = ["--config", deploymentConfig, "--env", environment ?? ""];
    return new Promise((resolveRun, rejectRun) => {
      const child = spawn(process.execPath, [wranglerBin, ...args, ...flags], {
        env: paid || ci ? {
          ...process.env,
          ...(paid && { WRANGLER_LOG_SANITIZE: "true" }),
          ...(ci && { CI: "true" }),
        } : process.env,
        stdio: capture ? ["inherit", "pipe", "inherit"] : "inherit",
      });
      activeChild = child;
      let stdout = "";
      if (capture) {
        child.stdout.setEncoding("utf8");
        child.stdout.on("data", (chunk) => { stdout += chunk; });
      }
      child.once("error", (error) => {
        if (activeChild === child) activeChild = null;
        rejectRun(error);
      });
      child.once("close", (status) => {
        if (activeChild === child) activeChild = null;
        if (status !== 0) {
          if (stdout) process.stdout.write(stdout);
          process.exitCode = status ?? 1;
          rejectRun(new Error(`Wrangler ${args[0]} failed.`));
          return;
        }
        resolveRun(stdout);
      });
    });
  }

  if (dryRun && !paid && !nativePaid) {
    await run([...deployArgs, "--dry-run"]);
    return;
  }

  // Wrangler resolves DB to the first binding or database name equal to DB.
  if (!dryRun && target.d1_databases.find((db) => db.binding === "DB" || db.database_name === "DB") !== database) {
    throw new Error("Another D1 database shadows the DB binding in Wrangler.");
  }

  for (const name of ["NATIVE_GIT_ORIGIN", "NATIVE_GIT_DIRECT_ORIGIN"]) {
    const origin = target.vars?.[name];
    if (origin === undefined || dryRun) continue;
    const url = URL.parse(origin);
    if (url?.protocol !== "https:" || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
      throw new Error(`Use an HTTPS ${name} for deployment, or omit it here to use the Worker's dashboard variable.`);
    }
  }
  const lfsBucket = target.vars?.SHARD_LFS_R2_BUCKET;
  const lfsBinding = target.r2_buckets?.find(({ binding }) => binding === "LFS_OBJECTS");
  if ((!dryRun || paid) && lfsBucket !== undefined && lfsBucket !== lfsBinding?.bucket_name) {
    throw new Error("SHARD_LFS_R2_BUCKET must match the LFS_OBJECTS bucket name in this environment.");
  }
  let paidSecrets;
  if (paid) {
    configurePaid(rawConfig, target, lfsBinding, dryRun);
    if (!dryRun) paidSecrets = readPaidSecrets(secretsFile, rawConfig, target, target.vars,
      target.vars.GIT_BACKEND === "r2" ? [...r2SecretNames, ...gitSecretNames] : r2SecretNames);
  }

  // Wrangler prefers a configured account ID, but Workers Builds can only release to its own account.
  const buildsAccount = process.env.CLOUDFLARE_ACCOUNT_ID || process.env.CF_ACCOUNT_ID;
  const configuredAccount = target.account_id || rawConfig.account_id;
  if (!dryRun && process.env.WORKERS_CI === "1" && buildsAccount && configuredAccount &&
      String(configuredAccount).toLowerCase() !== buildsAccount.toLowerCase()) {
    throw new Error("Wrangler account_id must match the Workers Builds account before applying D1 migrations.");
  }

  let temporaryConfig;
  let temporarySecrets;
  function cleanup() {
    if (temporarySecrets) {
      rmSync(temporarySecrets, { recursive: true, force: true });
      temporarySecrets = undefined;
    }
    if (temporaryConfig) {
      rmSync(temporaryConfig, { force: true });
      temporaryConfig = undefined;
    }
  }
  function onSignal(signal) {
    try { activeChild?.kill(signal); } catch {}
    cleanup();
    process.exit(signal === "SIGINT" ? 130 : 143);
  }
  process.on("SIGINT", onSignal);
  process.on("SIGTERM", onSignal);
  process.on("exit", cleanup);
  function writeConfig() {
    // Keep the copy beside the source so asset, build and migration paths still resolve.
    const copy = temporaryConfig ?? join(dirname(deploymentConfig), `.wrangler.deploy-${randomUUID()}.json`);
    writeFileSync(copy, JSON.stringify(rawConfig, null, 2) + "\n", { flag: temporaryConfig ? "w" : "wx", mode: 0o600 });
    temporaryConfig = deploymentConfig = copy;
  }
  try {
    if (paid || nativePaid) writeConfig();
    if (dryRun) {
      await run([...deployArgs, "--dry-run"]);
      return;
    }
    if (!database.database_id || database.database_id === unsetDatabaseId) {
      const { database_name: name, jurisdiction } = database;
      if (!name) throw new Error("Set database_name on the DB binding for automatic D1 setup.");
      const databases = JSON.parse(await run(["d1", "list", "--json"], { capture: true }));
      if (!Array.isArray(databases) || !databases.every((candidate) =>
        typeof candidate?.name === "string" && candidate.name &&
        typeof candidate.uuid === "string" && databaseIdPattern.test(candidate.uuid) &&
        candidate.uuid !== unsetDatabaseId)) {
        throw new Error("Unexpected response to the D1 database list lookup.");
      }
      const matches = databases.filter((candidate) => candidate.name === name);
      if (matches.length > 1) throw new Error(`Multiple D1 databases are named '${name}'.`);
      const [existing] = matches;
      if (existing) {
        if (jurisdiction && existing.jurisdiction !== jurisdiction) {
          throw new Error(`D1 database '${name}' does not match the configured '${jurisdiction}' jurisdiction.`);
        }
        database.database_id = existing.uuid;
      }

      writeConfig();
      if (existing) await verifyExistingDatabase(run, database);
      else await run(["d1", "create", name, "--binding", "DB", ...(jurisdiction ? ["--jurisdiction", jurisdiction] : [])]);
    }
    const migrationOutput = await run(["d1", "migrations", "apply", "DB", "--remote"], { capture: true, ci: true });
    process.stdout.write(migrationOutput);
    // Wrangler 4.164.0 can mark a migration failed without returning an error exit code.
    if (/│[^│\n]+│\s*\u274c/.test(migrationOutput)) {
      throw new Error("D1 reported a failed migration.");
    }
    if (paidSecrets) {
      // Upload the validated values as JSON so Wrangler does not expand dotenv values or replace them from the shell.
      temporarySecrets = mkdtempSync(join(tmpdir(), "shard-deploy-secrets-"));
      const file = join(temporarySecrets, "secrets.json");
      writeFileSync(file, JSON.stringify(paidSecrets), { flag: "wx", mode: 0o600 });
      deployArgs.push("--secrets-file", file);
    }
    await run(deployArgs);
  } finally {
    process.off("SIGINT", onSignal);
    process.off("SIGTERM", onSignal);
    process.off("exit", cleanup);
    cleanup();
  }
}

deploy().catch((error) => {
  console.error(`Shard deploy: ${error.message}`);
  process.exitCode ||= 1;
});
