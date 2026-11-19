import { WorkflowEntrypoint } from "cloudflare:workers";
import { NonRetryableError } from "cloudflare:workflows";
import { RepositoryCopyStep } from "../build/index.js";

const MAX_CHUNKS = 120;
const CHUNK_OPTIONS = {
  retries: { limit: 2, delay: "5 seconds", backoff: "exponential" },
  timeout: "16 minutes",
};

export class RepositoryCopyWorkflow extends WorkflowEntrypoint {
  async run(event, step) {
    const copyId = event.payload?.copyId;
    if (typeof copyId !== "string" || Object.keys(event.payload).length !== 1 ||
        !Number.isSafeInteger(Number(copyId)) || Number(copyId) < 1 ||
        String(Number(copyId)) !== copyId || event.instanceId !== copyId) {
      throw new NonRetryableError("Invalid repository copy job ID");
    }

    for (let chunk = 1; chunk <= MAX_CHUNKS; chunk++) {
      const progress = await step.do(`advance copy ${chunk}`, CHUNK_OPTIONS,
        () => RepositoryCopyStep.advance(this.env, copyId));
      if (progress === "done") return;
      if (progress === "waiting" && chunk < MAX_CHUNKS) {
        await step.sleep(`wait for copy ${chunk}`, "10 seconds");
      }
    }
    // Cron continues any copy that still needs work after this instance completes.
  }
}
