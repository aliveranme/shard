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
| Git storage | The default uses SQLite Durable Objects for Git objects and refs. The [Artifacts Worker binding](https://developers.cloudflare.com/artifacts/api/workers-binding/) supports immutable reads, imports and forks. [Artifacts Git HTTPS](https://developers.cloudflare.com/artifacts/api/git-protocol/) supports upload-pack and v1 receive-pack. | Shard validates receive-pack commands at the proxy and checks the expected target for ref writes. Divergent Artifacts merges run through the configured native bridge. |
| Push events | [Artifacts event subscriptions](https://developers.cloudflare.com/artifacts/guides/event-subscriptions/) expose push events with a ref and before/after SHA. The local simulator omits these events. | Shard records pending push ref transitions in D1 and commits Webhook delivery and PR refresh together when an accepted transition is observed. The minute cron reconciles unfinished pushes against Git refs. Provider-side updates outside Shard need their own event ingestion. |
| Import | The Free and Artifacts backends support public HTTPS sources. The native origin also supports credentialed imports from allowed hosts. | The minute cron advances pending publication. D1 transactions apply portable metadata bundles. |
| Search and metadata | D1 supports transactional [batches](https://developers.cloudflare.com/d1/worker-api/d1-database/#batch). | Repository and global discussion lists filter visible issues. Code and commit search scans one Git snapshot within explicit object, byte and history limits. |
| Limits | [Workers](https://developers.cloudflare.com/workers/platform/limits/#memory) have 128 MB per isolate. The default Free Git configuration streams pushes, forks and imports with a 32 MiB pack budget. [Artifacts](https://developers.cloudflare.com/artifacts/platform/limits/) caps pushes at 64 MiB and new repositories at 1 GB. | Verify Git sizes and CPU usage under the target plan. Stream Worker requests and bound operations that buffer objects. |
| Public operations | [Rate Limiting](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) is approximate per Cloudflare location. [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/) indexes structured events. [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) restores metadata in place. | Use `workers.dev` or an owned custom domain and keep paired backups of D1, Git objects, and refs. |
| Email | [Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/) requires Cloudflare DNS and an onboarded sending domain. | Real account verification, password reset and notification delivery require the configured sender and access to a test mailbox. |

## Optional Git backends

By default, new repositories use a SQLite Durable Object named by the opaque
repository storage ID. D1 keeps visibility, access grants, the selected
backend and the same storage ID. `env.artifacts` retains the original Artifacts
deployment. `managed` and `managed-staging` add the Git bridge and background
execution services on Workers Paid. `native` uses an authenticated origin and
persistent disk. Shard applies the same Git URLs and permission checks to each.
[Deployment guide](README.md#deploy-to-cloudflare)

| Service | Role in an alternative deployment | Limits that affect this design |
|---|---|---|
| R2 | Store immutable packs and indexes, or LFS objects with a separate protocol endpoint. | About 4.995 GiB per single-part upload and 4.995 TiB per multipart object, up to 10,000 parts. Strong object consistency. One concurrent write per second to the same key. Standard free allowance is 10 GB-month, 1 million Class A and 10 million Class B requests per month. [Limits](https://developers.cloudflare.com/r2/platform/limits/) · [Consistency](https://developers.cloudflare.com/r2/reference/consistency/) · [Pricing](https://developers.cloudflare.com/r2/pricing/) |
| D1 | Keep forge metadata and, if needed, an index of LFS object ownership and size. | 500 MB per database on Free, 10 GB on Paid, 2 MB per row or BLOB, and 50/1,000 D1 queries per invocation. A single database executes queries serially. [Limits](https://developers.cloudflare.com/d1/platform/limits/) |
| SQLite Durable Objects | Use one per repository to serialize conflicting pushes and atomically publish refs and the pack index. | Cloudflare's limits table lists 10 GB per object, 5 GB total per Workers Free account, and 8 MB per SQL row or BLOB. The storage-full FAQ lists 1 GB per object on Free. [SQLite limits table](https://developers.cloudflare.com/durable-objects/platform/limits/#sqlite-backed-durable-objects-general-limits) · [Storage-full FAQ](https://developers.cloudflare.com/durable-objects/platform/limits/#what-happens-when-a-durable-object-exceeds-its-storage-limit) |
| Workers | Route and authorize, stream Git bodies, and run or delegate pack processing. | 128 MB per isolate. HTTP CPU is 10 ms on Free or up to 5 min on Paid. Free has 50 outbound subrequests per invocation, which can constrain native Git reads in large PR comparisons. Inbound body size follows the zone plan, with 100 MB on Free/Pro and 200 MB on Business. [Limits](https://developers.cloudflare.com/workers/platform/limits/) |

The Free path uses `durable-git@0.0.8` for Git Smart HTTP and pack ingestion.
Shard's adapter exposes commit, tree and blob reads by SHA, plus controlled
ref and HEAD updates, repo lifecycle, and forks. Keep objects required by saved
PR reviews after their branches move. Account for SQLite writes, memory and
object expansion as well as the compressed request cap. Web merge writes on
this backend use same-repository fast-forwards. Shard compares other PRs with
its bounded snapshot readers and can verify integration pushed from Git.
[durable-git](https://github.com/littledivy/durable-git)

Native mode uses `git http-backend` and a persistent volume for Git Smart
HTTP, pack handling and ref locking. The origin also serves repository
lifecycle, object reads, and bounded commit-pinned comparisons through an
authenticated HTTP API. Shard budgets background work by deployment profile
and queries Git snapshots on demand. D1 stores no per-file or per-commit search
index from pushes.
[Git HTTP backend](https://git-scm.com/docs/git-http-backend)

To move an existing repository between backends, pause writes, copy and
verify the refs and retained PR objects, then change its recorded backend.
Keep the source for rollback until the destination is verified. R2 LFS keys
follow the D1 repository or fork identity and require their own backup.

All Git backends can use the Shard LFS Batch route with `LFS_OBJECTS` bound.
The Worker checks Git token roles and verifies SHA-256 before publication.
Forks copy LFS pages before publication. The default Free deployment can run
with D1 and Git storage alone. Repository cleanup and copy progress live in D1.
Back up any R2 bucket with D1 and Git storage.
[Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)

## Optional Cloudflare execution services

The `managed-staging` and `managed` environments enable branch coordination,
durable repository copies and an ephemeral Git merge container. Git owns
objects and refs, D1 owns permissions and job outcomes, and R2 owns LFS bytes.
The minute cron remains the recovery path for outstanding jobs.
[Scheduled handler](src/lib.rs) · [Verified behavior](STATUS.md#verified-locally)

### Durable Objects for branch coordination

A Durable Object provides a globally addressed, single-threaded instance with
transactional SQLite storage. SQLite-backed classes are available on Workers
Free and Paid. Alarms can wake an idle object and deliver at least once, with one
scheduled alarm per object. Awaiting external I/O can interleave requests, so an
object still needs an explicit operation claim and recovery logic around remote
writes. [Durable Objects](https://developers.cloudflare.com/durable-objects/) · [Rules](https://developers.cloudflare.com/durable-objects/best-practices/rules-of-durable-objects/) · [SQLite storage and class setup](https://developers.cloudflare.com/durable-objects/best-practices/access-durable-objects-storage/) · [Alarms](https://developers.cloudflare.com/durable-objects/api/alarms/) · [Limits and pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)

A SQLite-backed Durable Object is addressed by the stable repository ID and
full target ref. It schedules an alarm on enqueue or a matching CI update.
The D1 claim still checks queue order, approvals, commit checks and access.
Git compares the expected target tip before updating the ref. After an
ambiguous write, the queue checks the live ref before retrying. The minute
cron reschedules unfinished work. [Merge queue](src/branch_rules.rs)

### Workflows for repository copies

Workflows is the Workers durable execution engine for multi-step jobs. Completed
steps checkpoint their results, failed steps can retry, and an instance can sleep
or wait for an event. External operations can still commit before a step reports
success. Keep retries idempotent and retain user-visible job status in D1 because
Workflow history expires. [Workflows](https://developers.cloudflare.com/workflows/) · [Workers API](https://developers.cloudflare.com/workflows/build/workers-api/) · [Idempotency and retries](https://developers.cloudflare.com/workflows/build/rules-of-workflows/) · [Limits](https://developers.cloudflare.com/workflows/reference/limits/) · [Pricing](https://developers.cloudflare.com/workflows/reference/pricing/)

Each copy Workflow uses the committed `repo_copies.id` for its instance ID and
payload. A step observes the destination Git storage and advances a bounded
LFS page with the existing D1 lease and cursor. Publication rechecks access.
Authenticated imports keep their credentials at the native origin. D1 stores
the visible outcome. The cron recovers missed starts and resumes jobs whose
Workflow is missing or has ended. [Copy reconciliation](src/repo_lifecycle.rs)

### Containers for native Git compute

Cloudflare Containers provides Linux processes, local scratch space and larger
CPU and memory budgets on Workers Paid. Shard routes the Artifacts merge
bridge through its own Durable Object. The bridge fetches the pinned base
and head into a temporary bare repository, creates the merge with native Git
and pushes with a lease on the expected target tip. The Worker checks access
and sends short-lived, repository-scoped credentials to the bridge.
[Git bridge](scripts/git-origin.mjs) · [Containers](https://developers.cloudflare.com/containers/) · [Durable Object Container API](https://developers.cloudflare.com/containers/api/durable-object-container/) · [Outbound traffic controls](https://developers.cloudflare.com/containers/configuration/outbound-traffic/)

Use disposable workspaces and keep published results in the existing Git or R2
backend. Container disks are ephemeral. The snapshot beta captures a point in
time and ties it to an image version. R2 FUSE lacks full POSIX semantics, which
matters for Git ref locks and atomic renames. The native Git origin requires a
persistent filesystem with those guarantees. Large LFS verification is a later
candidate if Worker CPU or elapsed time limits it. Stream the complete staged
object from R2 for SHA-256, check its size and use the existing publication
conditions. Account for outbound upload size limits on intercepted requests.
[Disk lifecycle](https://developers.cloudflare.com/containers/faq/#is-disk-persistent-what-happens-to-my-disk-when-my-container-sleeps) · [Snapshots](https://developers.cloudflare.com/containers/guides/snapshots/) · [R2 FUSE](https://developers.cloudflare.com/containers/examples/r2-fuse-mount/) · [Limits](https://developers.cloudflare.com/containers/platform/limits/) · [Pricing](https://developers.cloudflare.com/containers/platform/pricing/)

Hosted staging is the place to measure cold and warm merges and verify
conflicts, changed target refs and container termination during a push.
The release Workflow checks the bridge protocol and configured origin after
each deployment. [Container deployment and rollouts](https://developers.cloudflare.com/containers/guides/deploy/)

### Workers Builds releases

Workers Builds runs the isolated regressions and smokes the Git image. The
release script gives staging and production separate Workers and bindings.
It runs each environment's D1 migrations, deploys the checkout SHA, and waits
for that SHA to pass the release Workflow. A staging failure blocks the
production step. [Managed deployment](README.md#deploy-with-managed-cloudflare-services)

## Migrate GitHub Actions jobs to Cloudflare

| GitHub Actions responsibility | Cloudflare implementation | Acceptance check |
| --- | --- | --- |
| Push and PR build and test in `ci.yml` | Workers Builds checks out the revision and runs the pinned setup, Wasm checks and isolated regressions. Use a distinct non-production build command for PRs. | Compare results and required checks for passing and failing commits, including fork PRs. Preview jobs get no production bindings or credentials. |
| Docker build and smoke | Build and exercise `containers/git-merge.Dockerfile` in the release build. Wrangler publishes the image to Workers Containers, where the Durable Object starts it. | Verify runner Docker support, image identity, authenticated Git merge, ref leases and cold-start readiness on staging. |
| Deploy and verify each release | Workers Builds runs `npm run cd` on the protected branch. Wrangler applies D1 migrations and deploys the checkout SHA. Workflows check the deployed revision, D1, R2, Static Assets and bridge before and after promotion. | A failed or stale staging result blocks promotion. A production verification failure records the affected deployment and Workflow instance. |
| Build a prebuilt package and publish a release on `v*` tags | A release build packages the same tested checkout and records its checksum. A release Workflow can publish through the GitHub Releases API with a scoped GitHub App token after it receives a verified tag and artifact. | Check tag/SHA equality, prerelease handling, checksum, duplicate delivery, and draft-to-published behavior. Keep the tag workflow until the event source, builder and publisher pass these checks. |

1. Run Workers Builds checks alongside GitHub CI. Compare success, failure and
   logs on the same SHA. Make the new check required only after it covers
   protected branches and the PRs the repository accepts.
2. Rehearse managed releases on a separate `managed-staging` Worker with its
   own D1, R2, Artifacts namespace and Workflows. Confirm Docker runs in the
   build environment, scoped credentials work, the image matches the checkout,
   and the Workflow observes the correct deployed SHA and live bindings.
3. Enable Workers Builds promotion on the protected production branch. Use the
   same immutable revision for staging and production. Serialize release runs
   and keep one deployment trigger for each target. Exercise a failed staging
   check and a production verification failure before cutover.
4. Replace GitHub tag packaging and publishing last. Add a trusted tag event
   source and a Linux build job for the pinned toolchain. Persist the package
   and checksum before starting the publisher Workflow. Rehearse a prerelease
   and a repeated trigger while the GitHub release workflow still owns
   public publication. Switch ownership after both produce equivalent output.

Repository Copy Workflows remain application jobs. D1 persists their state
and Cron reconciles pending copies when the Workflow cannot complete them.
During CI/CD cutover, restore the prior check or deploy trigger if its
replacement fails. Code rollback uses the existing bindings. Recover data
from a paired backup or a compatible forward migration.
[Workers Builds](https://developers.cloudflare.com/workers/ci-cd/builds/) · [Workflows](https://developers.cloudflare.com/workflows/) · [Containers](https://developers.cloudflare.com/containers/) · [GitHub Releases API](https://docs.github.com/en/rest/releases/releases)

## Deployment prerequisites

- Use `workers.dev` or an owned HTTPS origin for staging. A Free instance needs
  its own D1 database and SQLite Durable Object namespace. Paid staging needs
  Artifacts access, Containers and separate R2 storage. Native staging needs
  an HTTPS Git origin and persistent volume.
- Email delivery requires the configured `EMAIL` binding, a verified sending
  domain and a mailbox available for end-to-end verification.
- Divergent or cross-repository Artifacts Web merges require the exact Artifacts
  Git origin and a configured managed container or external Git 2.38+ bridge.
- Exercise paid Artifacts remotes, hosted import, Webhook delivery and backups
  in staging with live bindings. Use separate R2 storage and signing secrets.

Current local coverage and remaining gaps are recorded in [STATUS.md](STATUS.md).
