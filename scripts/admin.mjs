#!/usr/bin/env node
// Grant, revoke or list Shard administrators through the selected D1 database.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { parseArgs } from "node:util";

const require = createRequire(import.meta.url);
const wrangler = join(dirname(require.resolve("wrangler/package.json")), "bin", "wrangler.js");
const usernamePattern = /^[A-Za-z0-9._-]{2,100}$/;

function usage() {
  console.log(`Usage: npm run admin -- <grant|revoke|list> [username] [options]

  grant <username>   Set is_admin = 1
  revoke <username>  Set is_admin = 0
  list               Show current administrators

Options:
  --config PATH      Wrangler configuration (default: wrangler.jsonc)
  --env NAME         Wrangler environment
  --local            Use the local D1 database instead of the remote one
  --replace          With grant: demote every other administrator first`);
}

function execute(sql, values) {
  const args = ["d1", "execute", "DB", values.local ? "--local" : "--remote", "--json", "--command", sql];
  if (values.config) args.push("--config", values.config);
  if (values.env) args.push("--env", values.env);
  const result = spawnSync(process.execPath, [wrangler, ...args], { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr || "");
    throw new Error(`wrangler d1 execute failed with status ${result.status}`);
  }
  const output = result.stdout.trim();
  const start = output.indexOf("[");
  if (start < 0) throw new Error("Wrangler returned no JSON result");
  const payload = JSON.parse(output.slice(start));
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
    const rows = execute(
      "SELECT username, email FROM users WHERE is_admin = 1 ORDER BY username COLLATE NOCASE",
      values,
    );
    if (rows.length === 0) {
      console.log("No administrators.");
      return;
    }
    for (const row of rows) console.log(`${row.username}\t${row.email ?? ""}`);
    return;
  }
  if (command !== "grant" && command !== "revoke") {
    throw new Error(`Unknown command '${command}'. Expected grant, revoke or list.`);
  }
  const username = validUsername(command, raw);
  if (values.replace && command !== "grant") throw new Error("--replace is only valid with grant.");
  if (values.replace) {
    const existing = execute(
      `SELECT id, username FROM users WHERE username = '${username}' COLLATE NOCASE`,
      values,
    );
    if (existing.length === 0) throw new Error(`User '${username}' was not found.`);
    const rows = execute(
      `UPDATE users
          SET is_admin = CASE WHEN username = '${username}' COLLATE NOCASE THEN 1 ELSE 0 END
        WHERE is_admin = 1 OR username = '${username}' COLLATE NOCASE
    RETURNING id, username, is_admin`,
      values,
    );
    const target = rows.find((row) => String(row.username).toLowerCase() === username.toLowerCase());
    if (!target || target.is_admin !== 1) throw new Error(`User '${username}' could not be promoted.`);
    console.log(`${target.username} is now an administrator.`);
    return;
  }
  const isAdmin = command === "grant" ? 1 : 0;
  const rows = execute(
    `UPDATE users SET is_admin = ${isAdmin} WHERE username = '${username}' COLLATE NOCASE RETURNING id, username`,
    values,
  );
  if (rows.length === 0) throw new Error(`User '${username}' was not found.`);
  console.log(`${rows[0].username} is ${isAdmin === 1 ? "now an administrator" : "no longer an administrator"}.`);
}

try {
  main();
} catch (error) {
  console.error(`Shard admin: ${error.message}`);
  process.exitCode = 1;
}