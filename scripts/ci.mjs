import { execFileSync, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { parseArgs } from "node:util";

const { values } = parseArgs({ options: { container: { type: "boolean" } } });
const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" };

function npm(...args) {
  execFileSync(process.execPath, [process.env.npm_execpath, ...args], { env, stdio: "inherit" });
}

function checkContainer() {
  const name = `shard-git-ci-${randomUUID()}`;
  const image = `${name}:local`;
  const docker = (...args) => execFileSync("docker", args, { stdio: "inherit" });
  docker("build", "--file", "containers/git-merge.Dockerfile", "--tag", image, ".");
  try {
    docker("run", "--detach", "--rm", "--network", "none", "--name", name,
      "--env", "SHARD_GIT_TOKEN=isolated-container-smoke-test", image);
    docker("exec", name, "node", "--input-type=module", "--eval", `
      import assert from "node:assert/strict";
      import { setTimeout } from "node:timers/promises";
      const origin = "http://127.0.0.1:8793";
      const deadline = Date.now() + 25_000;
      let health;
      do {
        health = await fetch(origin + "/healthz", { signal: AbortSignal.timeout(1000) }).catch(() => null);
        if (health) break;
        await setTimeout(250);
      } while (Date.now() < deadline);
      assert.equal(health?.status, 200);
      assert.equal((await health.json()).protocol, 1);
      const unauthorized = await fetch(origin + "/v1/artifacts/merge", { method: "POST" });
      assert.equal(unauthorized.status, 401);
      console.log("Git merge container answered its health check and rejected an unauthenticated merge.");
    `);
  } finally {
    spawnSync("docker", ["rm", "--force", name], { stdio: "ignore" });
    spawnSync("docker", ["image", "rm", image], { stdio: "ignore" });
  }
}

try {
  npm("run", "build");
  npm("test");
  if (values.container) checkContainer();
} catch (error) {
  console.error(`Shard CI: ${error.message}`);
  process.exitCode = 1;
}
