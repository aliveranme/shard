# Shard delivery plan

Use the pinned Wrangler simulator and real Git over loopback for validation. Apply new SQL migrations in disposable D1 state and commit each verified change. `STATUS.md` records observed behavior and verification results.

## Delivery order

| Stage | Implementation | Acceptance |
|---|---|---|
| 1 · Trustworthy pull requests | Compare the saved source tip with the best common ancestor of both branch tips. Report missing objects, ambiguous ancestry and traversal limits. Display bounded per-file line diffs for text. | Accepted pushes schedule a comparison refresh. Existing reviews retain their saved comparison. Target-only changes are absent. Missing objects and exceeded limits are reported separately from an empty diff. |
| 2 · Repository collaboration | Give existing users read or write access to a repository. Apply the same decisions to Web pages, listings and Git tokens. Discover Git refs through Git HTTPS and let repository owners manage branches and tags. | Readers can browse and clone private repos. Only writers can push. Revocation removes access on the next request. The ref inventory preserves slash names. |
| 3 · Review workflow | Bind decisions and line comments to a comparison version. Verify a pushed merge by checking both saved branch tips in the target history. | An update rejects old review forms and preserves links to historical diffs and lines. A deleted or unrelated target ref cannot be marked merged. |
| 4 · Repository workflows | Add repository rename with stable storage identity and old URL handling, transfer and fork/import flows. Add commit detail and source archives. | Old names resolve under the same permission checks. Fork and import only publish metadata after the storage operation can be accounted for. |
| 5 · Issue workflows and integrations | Add edit, labels, assignees and milestones, discussion search, notifications, commit checks, and signed webhooks. | Private results stay private. Repository writes require permission. Repeated CI reports update one context, and webhook retries retain their event ID and signature. |
| 6 · Operational readiness | Stream Git bodies, add rate limiting and request correlation, build account recovery and document backup and restore. | Replays and failed authorizations are covered. Rehearse D1 and Git recovery in disposable local state. |

## Cloudflare capability and prerequisites

| Area | Verified capability and boundary | Delivery condition |
|---|---|---|
| Git storage | The [Artifacts Worker binding](https://developers.cloudflare.com/artifacts/api/workers-binding/) reads commits, trees and blobs by SHA and supports import and fork. [Git HTTPS](https://developers.cloudflare.com/artifacts/api/git-protocol/) supports upload-pack and v1 receive-pack. | Shard validates receive-pack commands at the proxy. The native origin and Artifacts fast-forward path advance the target using an expected old ref. Divergent Artifacts merges use the configured native bridge. |
| Push events | [Artifacts event subscriptions](https://developers.cloudflare.com/artifacts/guides/event-subscriptions/) expose push events with a ref and before/after SHA. The local simulator omits these events. | Shard records pending push ref transitions in D1 and commits Webhook delivery and PR refresh together when an accepted transition is observed. The minute cron reconciles unfinished pushes against Git refs. Provider-side updates outside Shard need their own event ingestion. |
| Import | The binding imports [public HTTPS remotes](https://developers.cloudflare.com/artifacts/guides/import-repositories/). | The native origin performs credentialed imports from allowed HTTPS hosts. The minute cron advances pending publication. D1 transactions apply portable metadata bundles. |
| Search and metadata | D1 supports transactional [batches](https://developers.cloudflare.com/d1/worker-api/d1-database/#batch). | Repository and global discussion lists filter visible issues. Code and commit search scans one Git snapshot within explicit object, byte and history limits. |
| Limits | [Workers](https://developers.cloudflare.com/workers/platform/limits/#memory) have 128 MB per isolate. [Artifacts](https://developers.cloudflare.com/artifacts/platform/limits/) caps pushes at 64 MiB and new repositories at 1 GB. [Streams](https://developers.cloudflare.com/workers/runtime-apis/streams/) reduce buffering within both ceilings. | Show limits to users. Git protocol v2 receive-pack is unavailable upstream. Test the supported clone and push paths locally. |
| Public operations | [Rate Limiting](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) is approximate per Cloudflare location. [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/) indexes structured events. [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) restores metadata in place. | Use `workers.dev` or an owned custom domain and keep paired backups of D1, Git objects, and refs. |
| Email | [Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/) requires Cloudflare DNS and an onboarded sending domain. | Real account verification, password reset and notification delivery require the configured sender and access to a test mailbox. |

## Optional Git backends

Artifacts is the default. The `native` Wrangler environment runs Shard with
an authenticated native Git origin and no Artifacts binding. D1 stores
repository visibility, the backend and opaque storage ID, and forge metadata.
The Shard URLs and permission checks apply in both modes.
[Native Git deployment](README.md#deploy-with-a-native-git-origin)

| Service | Role in an alternative deployment | Limits that affect this design |
|---|---|---|
| R2 | Store immutable packs and indexes, or LFS objects with a separate protocol endpoint. | About 4.995 GiB per single-part upload and 4.995 TiB per multipart object, up to 10,000 parts. Strong object consistency. One concurrent write per second to the same key. Standard free allowance is 10 GB-month, 1 million Class A and 10 million Class B requests per month. [Limits](https://developers.cloudflare.com/r2/platform/limits/) · [Consistency](https://developers.cloudflare.com/r2/reference/consistency/) · [Pricing](https://developers.cloudflare.com/r2/pricing/) |
| D1 | Keep forge metadata and, if needed, an index of LFS object ownership and size. | 500 MB per database on Free, 10 GB on Paid, 2 MB per row or BLOB, and 50/1,000 D1 queries per invocation. A single database executes queries serially. [Limits](https://developers.cloudflare.com/d1/platform/limits/) |
| SQLite Durable Objects | Use one per repository to serialize conflicting pushes and atomically publish refs and the pack index. | The storage-full FAQ lists 1 GB per object on Free and 10 GB on Paid. Budget against the lower Free limit because the limits table lists 10 GB without that distinction. Free also has 5 GB total. SQL rows and BLOBs are capped at 8 MB. [Limits and storage-full FAQ](https://developers.cloudflare.com/durable-objects/platform/limits/#what-happens-when-a-durable-object-exceeds-its-storage-limit) |
| Workers | Route and authorize, stream Git bodies, and run or delegate pack processing. | 128 MB per isolate. HTTP CPU is 10 ms on Free or up to 5 min on Paid. Free has 50 outbound subrequests per invocation, which can constrain native Git reads in large PR comparisons. Inbound body size follows the zone plan, with 100 MB on Free/Pro and 200 MB on Business. [Limits](https://developers.cloudflare.com/workers/platform/limits/) |

For **Cloudflare-only** hosting, pair R2 with per-repository Durable Objects
and a Git engine. Stage the pack, validate objects and deltas, then commit the
new refs and the pack index together. A failed push leaves old refs readable.
Clean up unreferenced packs only after publication or a verified abandoned
upload. Evaluate [durable-git](https://github.com/littledivy/durable-git) and
[git-on-cloudflare](https://github.com/zllovesuki/git-on-cloudflare) for protocol
handling. Shard requires reads by tree and blob SHA, asynchronous import and
fork handling, and access to immutable PR comparison objects after ref changes.
The documented durable-git content API resolves reachable objects by commit
and path, so it needs additional integration for those contracts.

An isolated Wrangler run of `durable-git@0.0.8` stored a pack containing an
18 MiB fixture in R2 without an Artifacts binding. After restart, a clone
matched the commit, annotated tag and file bytes. This exercised the Git
engine on its own. [durable-git content API](https://github.com/littledivy/durable-git/blob/main/docs/api.md)

Native mode uses `git http-backend` and a persistent volume for Git Smart
HTTP, pack handling and ref locking. The origin also serves repository
lifecycle, object reads, and bounded commit-pinned comparisons through an
authenticated HTTP API. Shard budgets background work by deployment profile
and queries Git snapshots on demand. D1 stores no per-file or per-commit search
index from pushes.
[Git HTTP backend](https://git-scm.com/docs/git-http-backend)

Evaluate the same Git and Web workflows before adding a Cloudflare-only
backend. To move an existing repository, pause writes, copy and verify all
refs and any retained PR objects, then change its recorded backend. Retain the
source until the destination and rollback have been verified. Migrate Git LFS
objects separately from Git refs.

Both Git backends use the Shard LFS Batch route. The Worker authorizes each
operation with existing Git token roles, then streams a verified SHA-256
object to or from a private, repository-scoped R2 key. Forks copy LFS pages
before publication. Repository cleanup and copy progress live in D1. Back up
the R2 bucket with D1 and Git storage. [Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)

## Deployment prerequisites

- Use `workers.dev` or an owned HTTPS name for staging. Artifacts staging
  needs Workers Paid and its own namespace. Native staging needs an HTTPS
  Git origin and a persistent volume. Use separate D1 state for each stage.
- Email delivery requires the configured `EMAIL` binding, a verified sending
  domain and a mailbox available for end-to-end verification.
- For divergent or cross-repository Artifacts Web merges, use Git 2.38 or later
  and configure the native bridge with the exact Artifacts Git origin.
- Exercise paid Artifacts remotes, hosted import, Webhook delivery and backups
  in staging with live bindings. Use separate R2 storage and signing secrets.

Current local coverage and remaining gaps are recorded in [STATUS.md](STATUS.md).
