#!/usr/bin/env node
// Grant, revoke or list Shard administrators through the selected D1 database.
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { parseArgs } from "node:util";

const require = createRequire(import.meta.url);
const wrangler = join(dirname(require.resolve("wrangler/package.json")), "bin", "wrangler.js");
const { experimental_readRawConfig } = require("wrangler");
const usernamePattern = /^[A-Za-z0-9._-]{2,100}$/;
const databaseIdPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const unsetDatabaseId = "00000000-0000-0000-0000-000000000000";

function usage() {
  console.log(`Usage: npm run admin -- <grant|revoke|list> [username] [options]

  grant <username>   Set is_admin = 1
  revoke <username>  Set is_admin = 0
  list               Show current administrators

Options:
  --config PATH      Wrangler configuration (default: wrangler.jsonc or wrangler.json)
  --env NAME         Wrangler environment
  --local            Use the local D1 database instead of the remote one
  --replace          With grant: demote every other administrator first`);
}

function runWrangler(args) {
  const result = spawnSync(process.execPath, [wrangler, ...args], { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr || "");
    throw new Error(`wrangler ${args[0]} failed with status ${result.status}`);
  }
  return result.stdout.trim();
}

function parseJsonOutput(output) {
  try {
    return JSON.parse(output);
  } catch {}
  const match = output.match(/^\[/m);
  if (!match || match.index === undefined) throw new Error("Wrangler returned no JSON result");
  return JSON.parse(output.slice(match.index));
}

function resolveRemoteConfig(values) {
  const env = values.env ?? (process.env.CLOUDFLARE_ENV || undefined);
  const { rawConfig, configPath } = experimental_readRawConfig({ config: values.config, env });
  if (!configPath) return { config: values.config, env, cleanup: null };
  const target = env ? rawConfig.env?.[env] : rawConfig;
  if (env && !target) throw new Error(`Wrangler environment '${env}' is not defined.`);
  const database = target?.d1_databases?.find((entry) => entry.binding === "DB");
  if (!database || (database.database_id && database.database_id !== unsetDatabaseId)) {
    return { config: values.config, env, cleanup: null };
  }
  const { database_name: name, jurisdiction } = database;
  if (!name) throw new Error("Set database_name or database_id on the DB binding.");
  const listArgs = ["d1", "list", "--json", "--config", configPath];
  if (env) listArgs.push("--env", env);
  const databases = parseJsonOutput(runWrangler(listArgs));
  if (!Array.isArray(databases) || !databases.every((candidate) =>
    typeof candidate?.name === "string" && candidate.name &&
    typeof candidate.uuid === "string" && databaseIdPattern.test(candidate.uuid) &&
    candidate.uuid !== unsetDatabaseId)) {
    throw new Error("Unexpected response to the D1 database list lookup.");
  }
  const matches = databases.filter((candidate) => candidate.name === name);
  if (matches.length === 0) throw new Error(`D1 database '${name}' was not found.`);
  if (matches.length > 1) throw new Error(`Multiple D1 databases are named '${name}'.`);
  const [existing] = matches;
  if (jurisdiction && existing.jurisdiction !== jurisdiction) {
    throw new Error(`D1 database '${name}' does not match the configured '${jurisdiction}' jurisdiction.`);
  }
  database.database_id = existing.uuid;
  const copy = join(dirname(configPath), `.wrangler.admin-${randomUUID()}.json`);
  writeFileSync(copy, JSON.stringify(rawConfig, null, 2) + "\n", { flag: "wx" });
  return { config: copy, env, cleanup: () => rmSync(copy, { force: true }) };
}

function execute(sql, values, resolved) {
  const args = ["d1", "execute", "DB", values.local ? "--local" : "--remote", "--json", "--command", sql];
  if (resolved.config) args.push("--config", resolved.config);
  if (resolved.env) args.push("--env", resolved.env);
  const payload = parseJsonOutput(runWrangler(args));
  if (!Array.isArray(payload) || payload.some((entry) => entry?.success !== true)) {
    throw new Error("D1 reported a failed statement");
  }
  return payload.flatMap((entry) => Array.isArray(entry.results) ? entry.results : []);
}

function validUsername(command, username) {
  if (!username || !usernamePattern.test(username)) {
    throw new Error(`${command} needs a username of 2 to 100 letters, digits, dots, dashes or underscores.`);
  }
  return username;
}

function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      config: { type: "string" },
      env: { type: "string" },
      local: { type: "boolean", default: false },
      replace: { type: "boolean", default: false },
      help: { type: "boolean", short: "h" },
    },
  });
  if (values.help || positionals.length === 0) {
    usage();
    return;
  }
  const [command, raw] = positionals;
  if (command === "list") {
    if (positionals.length !== 1) throw new Error("list takes no positional arguments.");
    if (values.replace) throw new Error("--replace is only valid with grant.");
  } else if (command === "grant" || command === "revoke") {
    if (positionals.length !== 2) throw new Error(`${command} expects exactly one username.`);
    if (values.replace && command !== "grant") throw new Error("--replace is only valid with grant.");
    validUsername(command, raw);
  } else {
    throw new Error(`Unknown command '${command}'. Expected grant, revoke or list.`);
  }

  const resolved = values.local
    ? { config: values.config, env: values.env ?? (process.env.CLOUDFLARE_ENV || undefined), cleanup: null }
    : resolveRemoteConfig(values);
  try {
    if (command === "list") {
      const rows = execute(
        "SELECT username, email FROM users WHERE is_admin = 1 ORDER BY username COLLATE NOCASE",
        values,
        resolved,
      );
      if (rows.length === 0) {
        console.log("No administrators.");
        return;
      }
      for (const row of rows) console.log(`${row.username}\t${row.email ?? ""}`);
      return;
    }
    const username = validUsername(command, raw);
    if (values.replace) {
      const rows = execute(
        `UPDATE users
            SET is_admin = CASE WHEN username = '${username}' COLLATE NOCASE THEN 1 ELSE 0 END
          WHERE (is_admin = 1 OR username = '${username}' COLLATE NOCASE)
            AND EXISTS (SELECT 1 FROM users WHERE username = '${username}' COLLATE NOCASE)
      RETURNING id, username, is_admin`,
        values,
        resolved,
      );
      const target = rows.find((row) => String(row.username).toLowerCase() === username.toLowerCase());
      if (!target || target.is_admin !== 1) throw new Error(`User '${username}' was not found.`);
      console.log(`${target.username} is now an administrator.`);
      return;
    }
    const isAdmin = command === "grant" ? 1 : 0;
    const rows = execute(
      `UPDATE users SET is_admin = ${isAdmin} WHERE username = '${username}' COLLATE NOCASE RETURNING id, username`,
      values,
      resolved,
    );
    if (rows.length === 0) throw new Error(`User '${username}' was not found.`);
    console.log(`${rows[0].username} is ${isAdmin === 1 ? "now an administrator" : "no longer an administrator"}.`);
  } finally {
    resolved.cleanup?.();
  }
}

try {
  main();
} catch (error) {
  console.error(`Shard admin: ${error.message}`);
  process.exitCode = 1;
}
