import { DurableObject } from "cloudflare:workers";

const PORT = 8793;
const HEALTH_URL = "https://internal/healthz";
const MERGE_URL = "https://internal/v1/artifacts/merge";
const IDLE_TIMEOUT_MS = 60_000;
const STARTUP_TIMEOUT_MS = 20_000;

export class ArtifactsMergeContainer extends DurableObject {
  #readiness;
  #sessionToken;

  async fetch(request) {
    const health = request.method === "GET" && request.url === HEALTH_URL;
    if (!health && (request.method !== "POST" || request.url !== MERGE_URL)) {
      return new Response(null, { status: 404 });
    }

    const container = this.ctx.container;
    try {
      this.#readiness ??= this.#prepare(container).finally(() => { this.#readiness = undefined; });
      const ready = await this.#readiness;
      if (health) return Response.json(ready.health, { headers: { "Cache-Control": "no-store" } });
      if (!ready.health.mergeEnabled) return new Response(null, { status: 501 });

      // After handing off a mutation, a lost response may hide a successful push.
      return await container.getTcpPort(PORT).fetch("http://container/v1/artifacts/merge", {
        method: "POST",
        redirect: "manual",
        headers: {
          "Authorization": `Bearer ${ready.token}`,
          "Content-Type": "application/json",
        },
        body: request.body,
      });
    } catch {
      return new Response(null, { status: health ? 503 : 502 });
    }
  }

  async #prepare(container) {
    // A recreated Durable Object cannot authenticate to its predecessor's process.
    if (container.running && !this.#sessionToken) await container.destroy();
    if (!container.running) {
      const token = crypto.randomUUID();
      container.start({
        image: container.images.bridge,
        enableInternet: true,
        env: {
          SHARD_GIT_TOKEN: token,
          SHARD_GIT_ARTIFACTS_ORIGINS: this.env.SHARD_GIT_ARTIFACTS_ORIGINS ?? "",
        },
      });
      this.#sessionToken = token;
    }
    await container.setInactivityTimeout(IDLE_TIMEOUT_MS);

    const port = container.getTcpPort(PORT);
    const deadline = Date.now() + STARTUP_TIMEOUT_MS;
    do {
      try {
        const response = await port.fetch("http://container/healthz", { signal: AbortSignal.timeout(1_000) });
        if (response.status === 200) {
          const health = await response.json();
          if (health?.protocol === 1 && typeof health.mergeEnabled === "boolean") {
            return { health, token: this.#sessionToken };
          }
        } else {
          await response.body?.cancel();
        }
      } catch {
        // The VM may be running before Git startup has finished.
      }
      await scheduler.wait(200);
    } while (Date.now() < deadline);
    throw new Error("Artifacts merge bridge did not become ready");
  }
}
