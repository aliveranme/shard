# Changelog

## 0.5.1

### Security & Access Control

- **GPG signature verification (`src/gpg.rs`, `templates/commit.html`)**: Enforce canonical `tree`-first and unique `tree`/`author`/`committer` commit headers, strictly parse `committer` identity/timestamp lines, and display the verified `@signer` account alongside the signature badge and fingerprint.
- **Organization member departure (`src/organizations.rs`, `migrations/0034_audit_hardening.sql`)**: Revoke all `organization_repositories` links (including `accepted = 1`) when the owning member leaves or is removed, backed by an `AFTER DELETE ON organization_members` trigger and an active-membership check on team grants.
- **Auth, CSRF & caching (`src/lib.rs`, `src/avatar.rs`, `src/auth.rs`, `src/security.rs`, `src/recovery.rs`, `src/util.rs`)**: Bound all non-machine POST form bodies, key login throttling on canonical account identity, throttle `current_password` re-authentication, restrict `Cache-Control: private, no-cache` (bfcache) to anonymous public views, protect unexpired password-reset links from overwrite/send-failure invalidation, reject non-`same-origin`/`none` `Sec-Fetch-Site` values, preserve multi-value `Set-Cookie` headers with `headers.append`, and disallow `.` / `..` / `.git` endings in `valid_slug`.

### Git & Storage Engines

- **R2 Git pack & Durable Object hardening (`worker/git-r2-pack.mjs`, `worker/git-r2-packstore.mjs`, `worker/git-r2-receive.mjs`, `worker/git-r2-repository.mjs`, `worker/git-r2-archive.mjs`)**:
  - Enforce canonical Git tree modes, forbid `.git`/`git~1` entries, and require canonical tree sort order and strict commit headers in `#directReceive`.
  - Pass the initial 128 KiB header read tail into `inflateAt` to eliminate redundant R2 range `GET`s, bound `#indexes` to 16 MiB and `#packStores` to the latest revision with 4 MiB chunk caches, and cap `buildSourceTar` at 400 object reads.
  - Handle Git Protocol v2 `command=ls-refs` directly from `repository.refs` in `#directUploadPack` and fall back to the container before streaming oversized or unresolved objects.
  - Resolve `OFS_DELTA` entries chained onto `REF_DELTA` objects across passes in thin packs, wrap `#publish` in `#lock`, release rejected/conflicted non-multipart `work` rows, apply ref deletions before D/F checks, and sort `repository.refs`.
- **Native Git, LFS & ref consistency (`scripts/git-origin.mjs`, `src/handlers/git.rs`, `src/handlers/repos.rs`, `src/lfs_remote.rs`, `src/lfs.rs`, `src/repo_lifecycle.rs`)**: Return `404` instead of `413`/`500` when reading directory paths in Native Git `readTree`, validate `?branch=` browse refs, unify `-`/255-byte ref validation across wire and web handlers, protect active LFS multipart sessions from size-mismatch aborts, and clean up `lfs-incoming/` on expiry and repository deletion.

### Forge Workflows & UI

- **Merge queue & webhooks (`src/branch_rules.rs`, `src/webhooks.rs`, `src/pull_review.rs`, `src/handlers/issues.rs`, `migrations/0034_audit_hardening.sql`)**: Preserve `MergeQueueCoordinator` exponential backoff across minute-cron ticks via `/recover` vs `/wake`, prune legacy `webhook_deliveries` rows, add foreign-key indexes on `webhook_deliveries(repo_id)` and `merge_queue(repo_id)`, allow private same-repo PR comparison refreshes when `author_id` is `NULL`, and emit notifications/webhooks for reviews on public-fork PRs.
- **Templates, formatting & timestamps (`src/models.rs`, `templates/repo_transfers.html`, `templates/repo_code.html`, `static/css/shard.css`)**: Guard `Ts::fmt` against out-of-range JS `Date` exceptions and future `Ts::relative` values, include `account_nav.html` on `/user/settings/transfers`, mark the final directory breadcrumb with `aria-current="page"`, and add `user-select: none` to `.diff-table__number`.

### Upgrade

- Apply D1 migration `0034_audit_hardening.sql` before deploying `v0.5.1`.

## 0.5.0

### Added

- Appearance preferences at `/user/settings/appearance` persist explicit `light`, `dark`, or `auto` (`data-theme`) choices without JavaScript.
- Non-owner organization members can now voluntarily leave an organization (`leave`), automatically clearing any pending repository offers they made.
- Code previews display line numbers with `#L{n}` anchor highlighting; private repository badges add lock styling, and mobile navigation uses the native Popover API.
- R2 multipart Git LFS adds an explicit `abort` action and `ListParts` reconciliation on completion errors to recover from stale or mismatched part receipts.
- Migration `0033_audit_indexes_and_triggers.sql` adds 17 composite, partial, and foreign-key indexes across hot D1 tables and hardens the `git_ref_observe` trigger.

### Changed

- Hardened browser CSP to strict `style-src 'self'` with zero inline template styles, added compile-time content-hashed CSS/SVG URLs with immutable 1-year caching, `ETag`/`304` avatar caching, and bfcache-friendly `private, no-cache` on read-only HTML browse pages.
- Replaced the 1.1 MiB CJK webfont with metric-calibrated local system CJK font stacks and made primary buttons a solid yellow accent.
- Decoupled Git push admission TTL (5 minutes) from reconciliation intent TTL (1 hour) and ensured non-reporting, failed, or unconfirmed pushes release branch-rule locks immediately.
- Switched waiting merge queue Durable Object alarms to exponential backoff with an attempt cap, immediate wakeups on approval reviews, and a per-repository active queue limit.

### Fixed

- R2 Git pack engine: wrote full 40-byte v2 `.idx` trailers (pack checksum + index checksum), fixed `REF_DELTA` OID type resolution, upgraded `#directReceive` to a full memoized reachability walk, eliminated `request.clone()` / unbounded `arrayBuffer()` buffering, added cumulative inflate and `applyDelta` bounds checks, guarded ref D/F conflicts and concurrent lock state reads, and aligned archive control-character and error handling.
- Security & auth: equalized PBKDF2 timing for missing-user logins, expanded reserved usernames, escaped literal explore search patterns, verified all case-insensitive OpenPGP v4/v6 matching User IDs and revocations with packet/signature bounds, re-enforced live SQL repository/writer visibility checks on discussions and merges, and moved auth before R2 backend checks on multipart push control endpoints.
- Discussions, LFS & reliability: bounded issue/comment title and body sizes, stopped loading discarded issue bodies in list queries, propagated source archive errors instead of falling back to Worker tar assembly, released LFS upload leases on storage failures, reported concurrent signup collisions cleanly, fixed recovery email cooldown rollback on send failure, and prevented wrong-size multipart LFS sessions from locking an OID for 6 days.

### Upgrade

- Apply D1 migration `0033_audit_indexes_and_triggers.sql` before deploying `v0.5.0`. Existing bindings, secrets, and R2/Durable Object state remain fully compatible.

## 0.4.0

- Large R2 repositories clone without the Durable Object inflating every blob: upload-pack metadata is now synchronous and reads pack entry types directly.
- R2 directory listings hide the size column when the backend does not provide sizes, instead of leaving an empty column; the object cache is capped at 32 MiB with eviction.
- The project is released under the MIT License (`LICENSE`, package and Cargo metadata).
- The README Deploy to Cloudflare button is an inline link, so the rendered repository page no longer shows a bare reference label.
- Text file previews use server-side syntax highlighting for Rust, JavaScript/TypeScript, Python, Go, Java, C/C++, CSS, HTML/XML, shell, SQL, YAML, TOML, JSON, Dockerfile and Makefile.
- Issues can select labels while being created, issue lists show label chips, and labels on an issue link to the filtered list.
- R2 source archives are built inside the repository Durable Object from pack indexes instead of issuing one Worker request per file.
- R2 Git web reads resolve commits, trees, blobs, files and first-parent history directly from R2 pack indexes. Clone and fetch generate the transfer pack in the Worker; stored full entries and offset deltas are copied from the existing packs instead of being recompressed. Small pushes, including thin packs completed against existing R2 objects, are validated and published by the Worker. Large pushes, merges, comparisons, imports and compaction still use the container. Receive-pack advertisements no longer prewarm a container; a direct push that succeeds does not start one.
- The R2 Durable Object persists its container token across restarts and keeps an idle container for 10 minutes by default (`SHARD_GIT_R2_IDLE_MS`).
- README images are cached by visibility, and `static/_headers` gives fonts, images and CSS browser cache lifetimes.
- `SHARD_REGISTRATION=closed` disables signup while existing accounts keep signing in. Guest navigation and the signup page follow the setting.
- Added `npm run admin -- grant|revoke|list` for administrator bootstrap and review through D1, with `--replace`, `--config`, `--env` and `--local`.
- `npm run deploy:paid` preserves an explicitly configured `SHARD_LFS_UPLOAD_MODE`; the Paid preset still defaults to `r2-multipart`.
- Added a Chinese Cloudflare Dashboard manual covering private deployment repositories, Workers Builds, resource bindings and credentials, every deployment environment, and upgrade verification.

## 0.3.1

### Storage and deployment

- R2 Git compacts small packs in the background and retains objects used by reviews of deleted branches. The manifest switches after the replacement pack and index are durable. Replaced pairs remain available for 24 hours.
- The R2 Git recovery helper exports and restores Durable Object manifests alongside D1 and R2 backups. The local Docker/FUSE rehearsal restored a private repository, a fork, a historical review and byte-identical LFS data. [Recovery procedure](https://github.com/aliveranme/shard/blob/v0.3.1/README.md#r2-git-backup-and-restore)
- Managed CD restores the previous active Worker version after a failed production publish or probe when the failed release owns the active deployment. It verifies the restored version and, when available, runs its release Workflow.
- Paid multipart LFS defaults to 5,363,466,240 bytes. Existing published objects remain readable after lowering the upload cap.
- The Chinese deployment guide covers Cloudflare Dashboard setup, prebuilt archives, the Free deployment button, and multipart sizing. [Deployment guide](https://github.com/aliveranme/shard/blob/v0.3.1/docs/deployment-capacity.md)
- The Free deploy button supports a private GitHub or GitLab deployment repository. Production branch pushes deploy to the same Worker, migrate its selected D1 database, and tag Worker versions with the Git commit SHA.

### Upgrade

- Preserve the existing Worker and resource bindings. For a new LFS object larger than the default, including an unfinished upload, set `SHARD_LFS_MAX_BYTES` explicitly after measuring verification and publication capacity.
- Serialize production releases and keep D1 migrations compatible with the previous Worker so the rollback can serve the migrated database.

## 0.3.0

### Added

- Account settings accept GPG public keys. Commit details verify OpenPGP signatures, show the signing certificate fingerprint, and separately indicate verified committer email.
- The Paid native preset can advertise a direct Git hostname backed by persistent disk. Git pushes can exceed the Artifacts and Worker ingress limits while Shard checks access and branch protection. The preset enables R2 multipart LFS uploads.
- The `r2` Paid environment runs native Git in Cloudflare Containers with durable R2 pack storage and transactional refs. The packaged `git-remote-shard` helper supports multipart push, resume across process restarts, and replay of completed Git results.

### Changed

- The SQLite Git backend streams forks and imports. Push and copy budgets rise from 8 MiB to 32 MiB, and the copy ref limit rises from 512 to 2,048.
- The Paid profile expands search, pull request comparison and scheduled maintenance budgets. Legacy Artifacts and explicitly opted-in Paid native deployments use up to 300 seconds of Worker CPU by default.
- Managed Artifacts merges start a `standard-1` Cloudflare Container by default. The instance size is configurable.
- Large LFS uploads have separate wall-time budgets for checksum verification and R2 publication. Scheduled recovery caps S3 multipart copy work per invocation.
- Creating a Git access token requires the current password. Password recovery revokes all Git access tokens and browser sessions.
- Accepting a repository transfer disables its repository-managed webhooks and cancels outstanding deliveries. Past delivery history remains visible to the recipient.

### Fixed

- Access token settings load 20 tokens at a time in creation order. Revoking a token keeps the current page, and navigation links carry only the page number.
- Fresh R2 pushes have distinct retry receipts. The helper resumes expired multipart sessions, retries preparation when refs change, and cleans up rejected payloads.
- Account settings and login bind writes to the verified password. Repository settings, collaborator grants and branch rules verify current ownership when they change.
- Branch-rule edits wait for writes already admitted on that branch. Direct Git accepts valid UTF-8 ref names containing non-ASCII spaces.
- Issue assignments follow effective repository access. Removing the final eligible grant clears the assignment and updates filtered lists and counts.
- GitHub CI and release validation exercise both container images. Release packaging checks version consistency and dry-runs all supported deployment presets from the extracted archive.

### Upgrade

- Keep the bindings of an existing instance and deploy using its named environment. The deploy command applies pending D1 migrations.
- After a password reset, create replacement Git tokens for clients and automation. After a repository transfer, recreate the repository-managed webhooks you want to receive.
- For new R2 Git repositories, configure the `r2` environment with separate private Git and LFS buckets. See the [R2 Git deployment and resume instructions](https://github.com/aliveranme/shard/blob/v0.3.0/README.md#large-repositories-on-cloudflare-r2).

## 0.2.0

### Added

- The Deploy to Cloudflare button now starts a Workers Free instance with D1 and one SQLite Durable Object per Git repository. The default includes Git over HTTPS, public and private repositories, forks, issue tracking, and same-repository fast-forward merges. Bind a private R2 bucket to enable Git LFS on this instance.
- `npm run deploy:paid` selects managed Workers Paid, configures the Artifacts origin, and enables R2 multipart uploads with bucket-scoped credentials.
- Git LFS supports signed single PUTs in `r2-direct` mode and resumable uploads in `r2-multipart` mode with the packaged `scripts/git-lfs-r2-multipart.mjs` adapter. Shard verifies object size and SHA-256 before publication. Downloads use the standard Git LFS Basic adapter in every mode.
- Managed deployments coordinate protected merges in Durable Objects, resume repository copies in Workflows, and run divergent Artifacts merges in a Git container. Workers Builds can verify the same commit in staging and production.

### Changed

- The deploy command creates or reuses the selected D1 database, applies migrations before upload, and preserves dashboard-managed Worker variables. The prebuilt archive uses the same command and accepts `--secrets-file` for first deployment.
- GitHub Releases include version-specific notes, a prebuilt Worker archive, and a SHA-256 checksum.

### Fixed

- Concurrent SQLite Git clones and forks share reachability-cache preparation. Git protects the current default branch even while repository settings are syncing to D1.
- Named deployments reject mismatched Workers Builds targets and shadowed D1 bindings before migrating. Managed staging now deploys independently while production retains its Workers Builds identity check.

### Upgrade

- Existing Artifacts instances use `npm run deploy -- --env artifacts`. Carry over the existing Worker name, D1 database ID, Artifacts namespace, R2 bucket, and routes. Existing native instances use `--env native`.
- Deploy applies the pending D1 migrations. Ordinary R2 bindings continue to use the 100 MB proxy upload by default. Use the Paid preset or configure the upload mode explicitly to switch to signed uploads.
- See the [deployment guide](https://github.com/aliveranme/shard/blob/main/README.md#deploy-to-cloudflare) for all modes and the [Git LFS guide](https://github.com/aliveranme/shard/blob/main/README.md#git-lfs-upload-modes) for R2 setup.

## 0.1.0

### Added

- Git hosting on Cloudflare Workers with Cloudflare Artifacts or a native Git origin, plus Git LFS storage in a private R2 bucket.
- Repository collaboration with issues, pull requests, protected merges, releases, organizations, team grants, and signed webhooks.
