import { execFileSync, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { parseArgs } from "node:util";

const { values } = parseArgs({ options: { container: { type: "boolean" } } });
const env = { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false" };
const docker = (...args) => execFileSync("docker", args, { stdio: "inherit" });

function npm(...args) {
  execFileSync(process.execPath, [process.env.npm_execpath, ...args], { env, stdio: "inherit" });
}

function checkContainer(dockerfile, smoke) {
  const name = `shard-${dockerfile}-ci-${randomUUID()}`;
  const image = `${name}:local`;
  docker("build", "--load", "--platform", "linux/amd64", "--file", `containers/${dockerfile}.Dockerfile`, "--tag", image, ".");
  try {
    smoke(name, image);
  } finally {
    spawnSync("docker", ["rm", "--force", name], { stdio: "ignore" });
    spawnSync("docker", ["image", "rm", image], { stdio: "ignore" });
  }
}

function checkMergeContainer(name, image) {
  docker("run", "--detach", "--rm", "--network", "none", "--name", name,
    "--env", "SHARD_GIT_TOKEN=isolated-container-smoke-test", image);
  docker("exec", name, "node", "--input-type=module", "--eval", `
    import assert from "node:assert/strict";
    import { setTimeout } from "node:timers/promises";
    const origin = "http://127.0.0.1:8793";
    const token = "isolated-container-smoke-test";
    const deadline = Date.now() + 25_000;
    let health;
    do {
      health = await fetch(origin + "/healthz", {
        headers: { Authorization: "Bearer " + token },
        signal: AbortSignal.timeout(1000),
      }).catch(() => null);
      if (health) break;
      await setTimeout(250);
    } while (Date.now() < deadline);
    assert.equal(health?.status, 200);
    assert.equal((await health.json()).protocol, 1);
    const unauthHealth = await fetch(origin + "/healthz", { signal: AbortSignal.timeout(5000) });
    assert.equal(unauthHealth.status, 401);
    const unauthorized = await fetch(origin + "/v1/artifacts/merge", { method: "POST", signal: AbortSignal.timeout(5000) });
    assert.equal(unauthorized.status, 401);
    console.log("Git merge container answered its authenticated health check and rejected unauthenticated requests.");
  `);
}

function checkR2Container(name, image) {
  // The full R2 integration suite exercises the entrypoint and FUSE on supported Docker hosts.
  docker("run", "--rm", "--network", "none", "--name", name, "--entrypoint", "node", image,
    "--input-type=module", "--eval", `
      import assert from "node:assert/strict";
      import { execFileSync } from "node:child_process";
      import { once } from "node:events";
      import { mkdtemp, rm } from "node:fs/promises";
      import { tmpdir } from "node:os";
      import { join } from "node:path";
      import { createR2GitOrigin } from "./git-r2-origin.mjs";

      execFileSync("rclone", ["version"], { timeout: 5000 });
      execFileSync("fusermount3", ["--version"], { timeout: 5000 });
      const directory = await mkdtemp(join(tmpdir(), "shard-r2-smoke-"));
      const token = "isolated-container-smoke-test";
      let server;
      try {
        server = await createR2GitOrigin({ directory, mount: directory, token });
        server.listen(0, "127.0.0.1");
        await once(server, "listening");
        const origin = "http://127.0.0.1:" + server.address().port;
        const request = (path, options) => fetch(origin + path, { signal: AbortSignal.timeout(5000), ...options });
        assert.equal((await request("/healthz")).status, 401);
        const health = await request("/healthz", { headers: { Authorization: "Bearer " + token } });
        assert.equal(health.status, 200);
        assert.equal((await health.json()).protocol, 1);

        const workspace = "/v1/workspaces/00000000-0000-0000-0000-000000000001";
        const body = JSON.stringify({ repositories: [{ id: "shard-ci", head: "refs/heads/main", refs: [], packs: [] }] });
        const create = (headers = {}) => request(workspace, { method: "POST", headers, body });
        assert.equal((await create()).status, 401);
        const headers = { Authorization: "Bearer " + token };
        assert.equal((await create(headers)).status, 204);
        assert.equal((await request(workspace, { method: "DELETE", headers })).status, 204);
        console.log("R2 Git container served health and authenticated a native Git workspace.");
      } finally {
        server?.close();
        server?.closeAllConnections();
        await rm(directory, { recursive: true, force: true });
      }
    `);
}

try {
  npm("run", "build");
  npm("test");
  if (values.container) {
    checkContainer("git-merge", checkMergeContainer);
    checkContainer("git-r2", checkR2Container);
  }
} catch (error) {
  console.error(`Shard CI: ${error.message}`);
  process.exitCode = 1;
}
