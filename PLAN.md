# Shard delivery plan

Use the pinned Wrangler simulator and real Git over loopback for validation. Apply new SQL migrations in disposable D1 state and commit each verified change. `STATUS.md` records observed behavior and verification results.

## Delivery order

| Stage | Implementation | Acceptance |
|---|---|---|
| 1 · Trustworthy pull requests | Compare the saved source tip with the best common ancestor of both branch tips. Report missing objects, ambiguous ancestry and traversal limits. Display bounded per-file line diffs for text. | A push leaves the saved comparison in place until an explicit update. Target-only changes are absent. Missing objects and exceeded limits are reported separately from an empty diff. |
| 2 · Repository collaboration | Give existing users read or write access to a repository. Apply the same decisions to Web pages, listings and Git tokens. Add a read-only branch and tag inventory through Git HTTPS. | Readers can browse and clone private repos. Only writers can push. Revocation removes access on the next request. The list contains real Git refs and preserves slash names. |
| 3 · Review workflow | Bind decisions and line comments to a comparison version. Verify a pushed merge by checking both saved branch tips in the target history. | An update rejects old review forms and preserves links to historical diffs and lines. A deleted or unrelated target ref cannot be marked merged. |
| 4 · Repository workflows | Add repository rename with stable storage identity and old URL handling, transfer and fork/import flows. Add commit detail and source archives. | Old names resolve under the same permission checks. Fork and import only publish metadata after the storage operation can be accounted for. |
| 5 · Issue workflows and integrations | Add edit, labels, assignees and milestones, discussion search, notifications, commit checks, and signed webhooks. | Private results stay private. Repository writes require permission. Repeated CI reports update one context, and webhook retries retain their event ID and signature. |
| 6 · Operational readiness | Stream Git bodies, add rate limiting and request correlation, build account recovery and document backup and restore. | Replays and failed authorizations are covered. Rehearse D1 and Git recovery in disposable local state. |

## Cloudflare capability and prerequisites

| Area | Verified capability and boundary | Delivery condition |
|---|---|---|
| Git storage | The [Artifacts Worker binding](https://developers.cloudflare.com/artifacts/api/workers-binding/) can read commits, trees and blobs by SHA and can import and fork repositories. The binding exposes no read/write refs or merge method. [Git HTTPS](https://developers.cloudflare.com/artifacts/api/git-protocol/) supports upload-pack and v1 receive-pack. | Line diffs and ref discovery fit the current architecture. Web merge, merge queue and protected ref enforcement require a Git execution or receive path that can verify and atomically apply permitted changes. The documented [Worker Git client example](https://developers.cloudflare.com/artifacts/examples/isomorphic-git/) uses an in-memory filesystem. Its use for arbitrary repository merges needs a resource-bounded design and live verification. |
| Push events | [Artifacts event subscriptions](https://developers.cloudflare.com/artifacts/guides/event-subscriptions/) expose push events with ref and before/after SHA. The [local simulator](https://developers.cloudflare.com/artifacts/guides/local-development/) does not publish push, clone or fetch events. | Automatic merge/index/webhook updates driven by these events need a configured subscription and a paid staging namespace for end-to-end verification. Git reads and explicitly triggered verification can run locally. |
| Import | The binding imports [public HTTPS remotes](https://developers.cloudflare.com/artifacts/guides/import-repositories/). Import can complete asynchronously. | Private source credentials have no documented native binding flow. Require a separately authorized Git import path for private sources. |
| Search and metadata | D1 supports transactional [batches](https://developers.cloudflare.com/d1/worker-api/d1-database/#batch) and [FTS5](https://developers.cloudflare.com/d1/sql-api/sql-statements/#supported-sqlite-extensions). | Repository and issue metadata can be indexed in D1. Code and commit search need a visibility-aware, bounded Git indexing pipeline. |
| Limits | [Workers](https://developers.cloudflare.com/workers/platform/limits/#memory) have 128 MB per isolate. [Artifacts](https://developers.cloudflare.com/artifacts/platform/limits/) caps pushes at 64 MiB and new repositories at 1 GB. [Streams](https://developers.cloudflare.com/workers/runtime-apis/streams/) reduce buffering within both ceilings. | Show limits to users. Git protocol v2 receive-pack is unavailable upstream. Test the supported clone and push paths locally. |
| Public operations | [Rate Limiting](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) is approximate per Cloudflare location. [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/) indexes structured events. [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) restores metadata in place. | A deployed site needs a Workers Paid account, an Artifacts namespace, real D1 IDs and ownership of the target domain. Keep an independent mirror of Git objects and refs. Production restores need operator authorization. |
| Email | [Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/) requires Cloudflare DNS and an onboarded sending domain. | Real account verification, password reset and notification delivery require the configured sender and access to a test mailbox. |

## Deployment prerequisites

- Staging requires Workers Paid with Artifacts access, an Artifacts namespace,
  real D1 IDs and an owned HTTPS origin. Use staging to verify allowed public
  import through completion, real scheduled delivery and Artifacts push events.
- Email delivery requires the configured `EMAIL` binding, a verified sending
  domain and a mailbox available for end-to-end verification.
- Web merges, protected refs and merge queues require an authorized execution
  path that can validate and update the target ref atomically.
- Code and commit search require a permission-aware index with bounded ingestion
  and reconciliation for ref changes. Local Artifacts lifecycle events can be
  tested through Workflows. Push-triggered ingestion requires staging.

Current local coverage and remaining gaps are recorded in [STATUS.md](STATUS.md).
