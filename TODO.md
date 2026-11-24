# Shard Audit TODO

Tracked findings from the full-repository code reviews.

## Fourth review (2026-11-24)

### High (complete)
- [x] R2 Git DO imports `parseTagTarget` so annotated tags work on direct reads and ref advertisements (`worker/git-r2-repository.mjs`).
- [x] Free Git DO rollback clears `meta['reachable-version']` alongside `reachable`/`peeled`, preventing empty 0-object clones/forks after a rejected push (`worker/git-repository.mjs`).
- [x] Free Git DO stream-validates incoming packs (`PackStreamValidator`) before `PackStore.ingest` to block oversized delta targets (e.g. 512 MiB) and zlib bombs before allocation/CPU exhaustion (`worker/git-r2-pack.mjs`, `worker/git-repository.mjs`).
- [x] `parseTree` rejects non-adjacent duplicate tree entry names and symlink `.gitmodules` entries (`worker/git-r2-pack.mjs`).
- [x] Free Git `#fsckPacksAfter` validates target object types for `commit.tree`, `commit.parents`, and `tree` entries (`worker/git-repository.mjs`).
- [x] R2 direct-receive `parseReceivedPack` memoizes external `REF_DELTA` base lookups under a 32 MiB cumulative external-base budget (`worker/git-r2-receive.mjs`).
- [x] Mixed-case `refs/HEADS/` and `refs/TAGS/` prefixes are rejected across wire parsing, direct hooks, and `git_activity` admission (`src/git_wire.js`, `src/git_wire.rs`, `src/handlers/git.rs`, `src/git_activity.rs`, `scripts/git-origin.mjs`, `worker/git-r2-receive.mjs`).
- [x] R2 multipart LFS `finish()` no longer short-circuits R2 error `10025` (`InvalidPart`), allowing authoritative `ListParts` reconciliation and abort cleanup (`src/lfs_remote.rs`).
- [x] Pull request creation batch guards `INSERT INTO pulls` (and `issue_labels`) with visibility, `kind`, author, and timestamp checks; migration `0037_pull_creation_hardening.sql` tightens `pull_source_at_creation` to require `issues.kind = 'pull'`.

### Medium (complete)
- [x] Untrusted `X-Shard-Git-Origin-Token` rejected before D1 repository lookup on LFS endpoints to remove the existence oracle (`src/lfs.rs`).
- [x] Wrong-size multipart LFS replacement reconciles against R2 `ListParts` when `repo_lfs_parts` has rows, preventing 6-day OID lockouts from bogus `Record` receipts (`src/lfs_remote.rs`).
- [x] Webhook delivery loop tracks per-repository elapsed time (4.5 s cap) and network/timeout failures alongside delivery counts to prevent slow endpoints from starving other repositories (`src/webhooks.rs`).
- [x] Pull request comparison refresh, review submission, and detail rendering enforce the private-source-to-public-target visibility direction (`src/pull_review.rs`, `src/handlers/issues.rs`).
- [x] Login and re-authentication throttling atomically reserves attempts in D1 before PBKDF2 (`reserve_login_attempt_limit`), preventing concurrent burst bypasses (`src/auth.rs`, `src/handlers/auth_handlers.rs`).
- [x] Commit author avatar matching requires verified emails when email verification is configured (`src/avatar.rs`), and `update_email` throttles repeated collision probes per user (`src/handlers/auth_handlers.rs`).
- [x] `buildR2PackStore` enforces a 128 MiB total pack-size cap right after `head()` before reading pack chunks (`worker/git-r2-packstore.mjs`).
- [x] `readPackedObject` / `R2GitRepository.#readObject` separates base/delta-chain inflation budget from final `maxResultBytes` for `buildSourceTar` (`worker/git-r2-pack.mjs`, `worker/git-r2-repository.mjs`).
- [x] `R2GitRepository.fetch` checks `this.#writer` before pre-buffering 16 MiB receive-pack bodies (`worker/git-r2-repository.mjs`).
- [x] `ArtifactsMergeContainer` authenticates `/healthz`, cleans up `container-token` if `start()` throws, and bounds merge preparation/requests with timeouts (`worker/git-container.mjs`, `scripts/git-origin.mjs`, `scripts/ci.mjs`).
- [x] Direct Native Git origin enforces 60 s socket idle timeout, 128 max connections, bounded 16 MiB `git-upload-pack` (including gzip decompression) and `mirror()` timeout; R2 origin sets 60 s socket idle timeout (`scripts/git-origin.mjs`, `scripts/git-r2-origin.mjs`).
- [x] `git-remote-shard.mjs` passes loopback proxy URL and `http.*` settings through `GIT_CONFIG_*` environment variables instead of `argv` (`scripts/git-remote-shard.mjs`).
- [x] `deploy.mjs` defers writing temporary R2 secrets until immediately before `wrangler deploy`, writes temp configs with `0o600`, and both `deploy.mjs` and `admin.mjs` use async `spawn` with `SIGINT`/`SIGTERM` cleanup; `.gitignore` ignores `.wrangler.admin-*.json`.

### Remaining Low/Info follow-ups
- [ ] Clean up `.receive` spool files on terminal rejected pushes (`ng`) in `scripts/git-remote-shard.mjs`.
- [ ] Add `issue_labels(repo_id, issue_id)` index, `__Host-` cookie prefix on HTTPS, character-based password minimum check, and static asset / 429 security headers.

## Third review (2026-11-24)

- [x] Repository descriptions are capped at 350 characters in creation, settings, forks and imports; migration `0036_repo_description_limit.sql` truncates legacy oversized rows.
- [x] Native branch rules use case-insensitive matching in merge-queue approvals/checks and rule-change guards, matching push admission.

## v0.5.0 audit (H1–H5, M1–M13, L1–L8)

All items are complete; see [CHANGELOG.md](CHANGELOG.md) for 0.5.1.

## Second audit (v0.5.2)

All 40 findings from the second review are implemented and covered by the regression suite where observable:

- [x] High: email resend cooldown, IPv6 `/64` limits, highlighter and Markdown quadratic scans, Durable blob/ingest limits, R2 delta memory and depth, R2 direct-receive fsck, Durable receive D/F and branch-tip checks, Native case-folded refs.
- [x] Medium: avatar CSRF cookie regression, login/email oracle, Git/LFS existence oracle, proxied push admission, private media caching, LFS R2 leaks, LFS reconciliation, partial-index queries, merge-queue wake/timeout/message, container token and workspace cleanup, R2 metadata limits and upload-pack edge cases, webhook starvation, organization listing order, form size limits, loopback downgrade in the LFS client, `admin.mjs` remote/atomic behavior.
- [x] Low: ref-length drift, GPG revocation argument and committer parsing, `?next=` encoding, path validation, ref helper reuse, comment parsing, LFS error handling, import default branch, client retry/error masking, R2 archive and stream details, lifecycle and integer bounds, schema/UI/doc polish.

### Known follow-ups

- [ ] Organization membership still has no invitation/accept step; owned organizations are now listed first to limit displacement.
- [ ] Exercise the R2 container, hosted Free quotas and proxied push timeouts on Cloudflare staging.
