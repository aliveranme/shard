import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

const placeholder = "00000000-0000-0000-0000-000000000000";
const uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const explicit = "11111111-2222-3333-4444-555555555555";
const migration = readFileSync(new URL("../migrations/0001_init.sql", import.meta.url), "utf8");
const schema = ["repos", "pulls"].map((name) => ({
  type: "table", name, sql: migration.match(new RegExp(`CREATE TABLE IF NOT EXISTS ${name} \\([\\s\\S]+?\\n\\);`))[0],
}));
const ledger = (name) => ({ type: "table", name, sql: `CREATE TABLE "${name}" (id INTEGER PRIMARY KEY, name TEXT UNIQUE)` });
const rows = (results) => JSON.stringify([{ success: true, results }]);
const existing = JSON.stringify([{ name: "shard", uuid }]);

function exercise(t, { list = existing, queries = [], id = placeholder, table, dryRun = false } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "shard-deploy-d1-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const cli = join(dir, "node_modules", "wrangler");
  const config = join(dir, "custom.json");
  const log = join(dir, "calls.jsonl");
  mkdirSync(join(cli, "bin"), { recursive: true });
  writeFileSync(join(dir, "deploy.mjs"), readFileSync(new URL("../scripts/deploy.mjs", import.meta.url)));
  writeFileSync(config, JSON.stringify({ env: { production: { d1_databases: [
    { binding: "DB", database_name: "shard", database_id: id, migrations_table: table },
  ] } } }));
  writeFileSync(join(cli, "package.json"), '{"main":"index.cjs"}');
  writeFileSync(join(cli, "index.cjs"), `exports.experimental_readRawConfig = ({ config }) => ({
    rawConfig: JSON.parse(require("node:fs").readFileSync(config, "utf8")), configPath: config
  });`);
  writeFileSync(join(cli, "bin", "wrangler.js"), `
    const fs = require("node:fs");
    const args = process.argv.slice(2);
    const config = args[args.indexOf("--config") + 1];
    const env = args[args.indexOf("--env") + 1];
    const binding = JSON.parse(fs.readFileSync(config, "utf8")).env[env].d1_databases[0];
    const log = ${JSON.stringify(log)};
    const calls = fs.existsSync(log) ? fs.readFileSync(log, "utf8").trim().split("\\n").map(JSON.parse) : [];
    fs.appendFileSync(log, JSON.stringify({ args, config, env, id: binding.database_id }) + "\\n");
    if (args[1] === "list") process.stdout.write(${JSON.stringify(list)});
    if (args[1] === "execute") {
      const response = ${JSON.stringify(queries)}[calls.filter((call) => call.args[1] === "execute").length];
      if (typeof response === "number") process.exitCode = response;
      else process.stdout.write(response ?? "unexpected query");
    }
    if (args[1] === "create") {
      const raw = JSON.parse(fs.readFileSync(config, "utf8"));
      raw.env[env].d1_databases[0].database_id = ${JSON.stringify(uuid)};
      fs.writeFileSync(config, JSON.stringify(raw));
    }
  `);
  const args = [join(dir, "deploy.mjs"), "--config", config, "--env", "production"];
  if (dryRun) args.push("--dry-run");
  const result = spawnSync(process.execPath, args, { cwd: dir, encoding: "utf8" });
  assert.ifError(result.error);
  const calls = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse);
  assert.ok(calls.every((call) => call.env === "production"));
  assert.equal(JSON.parse(readFileSync(config, "utf8")).env.production.d1_databases[0].database_id, id);
  assert.equal(readdirSync(dir).some((name) => name.startsWith(".wrangler.deploy-")), false);
  return { ...result, calls, commands: calls.map(({ args }) => args[0] === "d1" ? args[1] : args[0]) };
}

test("automatic D1 reuse checks the pinned DB before migration", async (t) => {
  const table = 'shard "history"';
  for (const [name, options, commands] of [
    ["empty", { queries: [rows([])] }, ["list", "execute", "migrations", "deploy"]],
    ["Shard schema with custom ledger", {
      table, queries: [rows([...schema, ledger(table)]), rows([{ name: "0001_init.sql" }])],
    }, ["list", "execute", "execute", "migrations", "deploy"]],
    ["missing database", { list: "[]" }, ["list", "create", "migrations", "deploy"]],
    ["explicit UUID", { id: explicit }, ["migrations", "deploy"]],
    ["dry run", { dryRun: true }, ["deploy"]],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, options);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.commands, commands);
      const lookups = result.calls.filter(({ args }) => args[1] === "execute");
      const apply = result.calls.find(({ args }) => args[1] === "migrations");
      if (lookups.length) {
        assert.notEqual(apply.config, result.calls[0].config);
        assert.equal(dirname(apply.config), dirname(result.calls[0].config));
      }
      for (const call of lookups) {
        assert.deepEqual(call.args.slice(0, 6), ["d1", "execute", "DB", "--remote", "--json", "--command"]);
        assert.equal(call.id, uuid);
        assert.equal(call.config, apply.config);
      }
      if (lookups.length) assert.match(lookups[0].args[6], /FROM sqlite_schema/);
      if (lookups.length === 2) assert.match(lookups[1].args[6], /FROM "shard ""history"""/);
      if (apply) assert.equal(apply.id, options.id ?? uuid);
      if (options.dryRun) assert.ok(result.calls[0].args.includes("--dry-run"));
    });
  }
});

test("automatic D1 reuse fails closed", async (t) => {
  for (const [name, options, status, commands] of [
    ["invalid resolution", { list: '[{"name":"shard","uuid":"invalid"}]' }, 1, ["list"]],
    ["ambiguous resolution", { list: JSON.stringify([...JSON.parse(existing), ...JSON.parse(existing)]) }, 1, ["list"]],
    ["unrelated schema", { queries: [rows([
      { type: "table", name: "repos", sql: "CREATE TABLE repos(id)" }, ledger("d1_migrations"),
    ])] }, 1, ["list", "execute"]],
    ["Shard schema without migration history", {
      queries: [rows([...schema, ledger("d1_migrations")]), rows([])],
    }, 1, ["list", "execute", "execute"]],
    ["failed command", { queries: [7] }, 7, ["list", "execute"]],
    ["failed result", { queries: ['[{"success":false,"results":[]}]'] }, 1, ["list", "execute"]],
    ["malformed result", { queries: ["[]"] }, 1, ["list", "execute"]],
    ["malformed schema row", { queries: [rows([{ type: "table", name: "other" }])] }, 1, ["list", "execute"]],
  ]) {
    await t.test(name, (t) => {
      const result = exercise(t, options);
      assert.equal(result.status, status, result.stderr);
      assert.deepEqual(result.commands, commands);
    });
  }
});
