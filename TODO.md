# Shard Audit TODO

Tracked findings from the full-repository code review (v0.5.0).

## High Severity (H1–H5)

- [x] **H1. GPG commit signature author/committer verification & signer display (`src/gpg.rs`, `templates/commit.html`)**
  - Strictly validate Git `committer` header syntax (`<name> <email> <timestamp> <tz>`), require `tree` first, and forbid duplicate `tree`/`author`/`committer` lines in `signed_commit`.
  - Resolve and display the verified GPG key owner's account identity (`@signer`) alongside the signature badge and fingerprint on `#commit-signature` in `templates/commit.html`.
- [x] **H2. Revoke accepted organization repository links on member departure or removal (`src/organizations.rs`, `migrations/0034_audit_hardening.sql`)**
  - Remove `AND accepted = 0` when cleaning up `organization_repositories` during `leave` and `remove_member`.
  - Add D1 trigger `organization_member_removed_repos` (`AFTER DELETE ON organization_members`) that deletes all `organization_repositories` rows owned by the departing user.
  - Ensure `update_team` (`action == "grant"`) requires the repository owner to remain an organization member.
- [x] **H3. Harden `R2GitRepository.#directReceive` object validation (`worker/git-r2-pack.mjs`, `worker/git-r2-repository.mjs`)**
  - Enforce canonical Git tree entry modes (`40000`, `100644`, `100755`, `120000`, `160000`), reject `.git` (case-insensitively and HFS+/NTFS variants), reject duplicate entry names, and enforce canonical Git tree sort order.
  - Enforce strict commit header validation (`tree` first, single `tree`/`author`/`committer`, required `committer` line with valid timestamp).
- [x] **H4. Eliminate redundant R2 range reads in `readPackedObject` and bound Durable Object memory caches (`worker/git-r2-pack.mjs`, `worker/git-r2-repository.mjs`, `worker/git-r2-packstore.mjs`, `worker/git-r2-archive.mjs`)**
  - Pass the remaining bytes of the initial 128 KiB header read (`header.subarray(parsed.start - offset)`) into `inflateAt` so objects under 128 KiB do not issue a second R2 range `GET`.
  - Bound `#indexes` by a 16 MiB total byte budget, retain only the latest revision's `R2PackStore` in `#packStores` with a bounded 4 MiB chunk cache, and enforce a `MAX_READS = 400` budget in `buildSourceTar`.
- [x] **H5. Handle Git Protocol v2 `command=ls-refs` directly in `R2GitRepository.#directUploadPack` and guard oversized direct pack entries (`worker/git-r2-repository.mjs`)**
  - Intercept v2 `command=ls-refs` at the start of `#directUploadPack` (supporting `ref-prefix`, `symrefs`, `peel`, and `unborn` directly from `repository.refs` without building `R2PackStore`).
  - Fall back to the container before streaming if any pack entry exceeds `MAX_DIRECT_FILE` or has an unresolved type.

## Medium Severity (M1–M13)

- [x] **M1. Preserve `MergeQueueCoordinator` exponential backoff across minute-cron ticks (`src/branch_rules.rs`)**
  - Separate explicit event `/wake` signals (which reset `wait_attempts`) from cron `/recover` signals so the minute cron only arms the coordinator when no backoff alarm is active.
  - Mark queued head-of-line entries as `failed` (`reason = 'checks'`) once `wait_attempts` exceeds `MAX_WAIT_ATTEMPTS` instead of stalling the branch queue indefinitely.
- [x] **M2. Prune legacy `webhook_deliveries` rows and index `repo_id` (`src/webhooks.rs`, `migrations/0034_audit_hardening.sql`)**
  - Add non-partial indexes on `webhook_deliveries(repo_id)`, `webhook_deliveries(created_at, event_id)`, and `merge_queue(repo_id)` for `ON DELETE CASCADE` performance.
  - Prune expired `webhook_deliveries` in `clean_subscriptions`.
- [x] **M3. Lock `R2GitRepository.#publish` and release rejected/conflicted non-multipart prepared `work` rows (`worker/git-r2-repository.mjs`, `src/handlers/git.rs`)**
  - Wrap `#publish` in `this.#lock(...)`.
  - Release non-multipart prepared R2 `work` rows when `publish_r2` rejects a push or hits a `409` revision conflict so compaction is not blocked for 65 minutes while preserving multipart `--resume` retries.
- [x] **M4. Support chained `OFS_DELTA` -> `REF_DELTA` resolution in thin packs (`worker/git-r2-receive.mjs`)**
  - Defer `OFS_DELTA` entries whose base offset belongs to an unresolved `REF_DELTA` into `pending` and resolve them across passes.
- [x] **M5. Fix Native Git `readObject("file")` on directories and validate `?branch=` query parameters (`scripts/git-origin.mjs`, `src/handlers/repos.rs`)**
  - Return early in `readTree` when `path !== undefined` and `entries.length === limit` so requesting a directory with $\ge 2$ entries returns `404` instead of `413` / `500`.
  - Validate `?branch=` in `src/handlers/repos.rs` and restrict `resolveBrowseCommit` in `scripts/git-origin.mjs` to `HEAD`, 40-hex OIDs, or valid refs (`refs/heads/*`, `refs/tags/*`).
- [x] **M6. Protect active LFS multipart sessions from size-mismatch aborts and clean up `lfs-incoming/` (`src/lfs_remote.rs`, `src/repo_lifecycle.rs`, `src/lfs.rs`)**
  - Do not destroy an active `repo_lfs_multipart` session that has recorded parts or staged R2 data when a `start` request arrives with a different `:size`.
  - Abort expired R2 multipart uploads and delete staged `lfs-incoming/` objects during cleanup and repository deletion.
- [x] **M7. Validate `private_fork` visibility before acquiring `git_head_lease_until` and mutating Git `HEAD` (`src/handlers/repos.rs`)**
  - Reject setting a `private_fork` repository to `public` before claiming the 5-minute `git_head_lease_until` lease and calling `handle.set_default_branch`.
- [x] **M8. Release branch admission lock on `WriteError::Unavailable` in `branch_rules::write_ref` (`src/branch_rules.rs`)**
  - Call `git_activity::release_admission` when `handle.write_ref` fails with `WriteError::Unavailable`.
- [x] **M9. Unify Git branch/tag ref name validation across wire and web handlers (`src/git_wire.js`, `src/handlers/git.rs`, `worker/git-repository.mjs`, `worker/git-r2-repository.mjs`, `worker/git-r2-receive.mjs`, `scripts/git-origin.mjs`)**
  - Reject stripped branch/tag names starting with `-` or exceeding 255 bytes on the Git wire path so pushed refs match `git_wire::valid_ref`.
  - Apply ref deletions before D/F conflict checks in `R2GitRepository.#directReceive` and sort `repository.refs`.
- [x] **M10. Prevent password-reset link overwrite DoS and pre-send failure invalidation (`src/recovery.rs`)**
  - Do not allow unauthenticated reset requests to overwrite an unexpired reset link before `expires_at`, allow immediate verification send when `token_hash IS NULL`, and restore any previous active link on mail send failure.
- [x] **M11. Bound standard POST form bodies and harden login / `current_password` throttling (`src/lib.rs`, `src/avatar.rs`, `src/auth.rs`, `src/handlers/auth_handlers.rs`)**
  - Read all non-machine POST form bodies through `avatar::read_form_bounded` before `form_data()`.
  - Key login throttling on resolved canonical account identity (plus normalized IPv6 `/64` prefix) and enforce failure throttling on authenticated `current_password` checks (`verified_password_hash`).
- [x] **M12. Restrict `Cache-Control: private, no-cache` (bfcache) to anonymous public views (`src/lib.rs`)**
  - Only enable `bfcache` when `user.is_none()` so private repository pages and authenticated CSRF forms receive `Cache-Control: no-store`.
- [x] **M13. Guard `Ts::fmt` against out-of-range JS `Date` `RangeError` and fix future timestamps in `Ts::relative` (`src/models.rs`)**
  - Clamp/validate timestamps before calling `Date::to_iso_string()` and handle negative `elapsed` in `Ts::relative` by falling back to the formatted date.

## Low / Informational Severity (L1–L8)

- [x] **L1. Use `headers.append` instead of `headers.set` when cloning `Headers` (`src/lib.rs`, `src/security.rs`)**
- [x] **L2. Reject any `Sec-Fetch-Site` other than `same-origin` or `none` in `security::same_origin` (`src/security.rs`)**
- [x] **L3. Reject slugs ending in `.` or `.git` (or containing `..`) in `util::valid_slug` (`src/util.rs`)**
- [x] **L4. Allow scheduled PR comparison refreshes on private same-repository PRs when `author_id` is `NULL` or revoked (`src/pull_review.rs`)**
- [x] **L5. Emit notifications and webhooks for reviews on cross-repository PRs from public forks (`migrations/0034_audit_hardening.sql`)**
- [x] **L6. Validate emails and bound batch size in `git_author_ids` and skip session lookup on `/user/avatar/:id` (`src/avatar.rs`, `src/lib.rs`)**
- [x] **L7. Fix UI/template/CSS polish items (`templates/repo_transfers.html`, `templates/repo_code.html`, `static/css/shard.css`, `src/pull_review.rs`, `src/handlers/issues.rs`)**
  - Include `account_nav.html` in `templates/repo_transfers.html`.
  - Render the final directory breadcrumb in `templates/repo_code.html` with `aria-current="page"`.
  - Add `user-select: none` to `.diff-table__number` and remove stale `.nav__menu > summary .icon` in `static/css/shard.css`.
  - Clear stale `merge_queue` status on manual `merge_verified`, PR close, or comparison version advancement.
- [x] **L8. Format all Rust sources with `cargo fmt` (`src/**/*.rs`)**
