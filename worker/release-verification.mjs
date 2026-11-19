import { WorkflowEntrypoint } from "cloudflare:workers";
import { NonRetryableError } from "cloudflare:workflows";

const retries = { limit: 5, delay: "2 seconds", backoff: "exponential" };

export class ReleaseVerification extends WorkflowEntrypoint {
  async run(event, step) {
    const { tag, versionId } = event.payload ?? {};
    if (typeof tag !== "string" || !/^[0-9a-f]{40}$/.test(tag)) {
      throw new NonRetryableError("Release verification requires a commit SHA.");
    }
    await step.do("Verify the deployed revision and its bindings", { retries, timeout: "20 seconds" }, async () => {
      if (this.env.SHARD_VERSION.tag !== tag) throw new Error("Worker version tag differs from the release commit.");
      if (versionId !== undefined && this.env.SHARD_VERSION.id !== versionId) {
        throw new Error("Worker version ID differs from the requested recovery version.");
      }
      await this.env.DB.prepare("SELECT git_backend, lfs_copy_id FROM repos LIMIT 0").run();
      await this.env.DB.prepare("SELECT lfs_lease_until FROM repo_copies LIMIT 0").run();
      await this.env.DB.prepare("SELECT comparison_version FROM merge_queue LIMIT 0").run();
      await this.env.DB.prepare("SELECT fingerprint FROM gpg_keys LIMIT 0").run();
      await this.env.LFS_OBJECTS.head("__shard_release_probe__");
      const asset = await this.env.ASSETS.fetch("https://internal/css/shard.css");
      await asset.body?.cancel();
      if (!asset.ok) throw new Error("Worker static assets are unavailable.");
    });
    await step.do("Verify Git merge container readiness", { retries, timeout: "45 seconds" }, async () => {
      // Probe a fresh Durable Object even when separate deployments carry the same SHA.
      const bridge = this.env.ARTIFACTS_MERGE_CONTAINER.getByName(`release-${tag}-${event.instanceId}`);
      const ready = await bridge.fetch("https://internal/healthz");
      if (!ready.ok) {
        await ready.body?.cancel();
        throw new Error(`Git merge container is unavailable (HTTP ${ready.status}).`);
      }
      const result = await ready.json();
      if (result.protocol !== 1) throw new Error("Git merge container protocol is incompatible.");
      if (result.mergeEnabled !== true) throw new Error("Configure the Artifacts Git origin allowlist for this Worker.");
    });
    return tag;
  }
}
