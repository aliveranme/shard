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

### Remaining Medium/Low follow-ups
- [ ] LFS `X-Shard-Git-Origin-Token` existence oracle (`src/lfs.rs`) and 6-day wrong-size multipart lockout (`src/lfs_remote.rs`).
- [ ] Webhook per-repo time/timeout fairness budget (`src/webhooks.rs`) and private-source-to-public-target guard in `src/pull_review.rs`.
- [ ] Atomic pre-PBKDF2 login/re-auth throttling (`src/auth.rs`) and verified-email gating for commit avatars (`src/avatar.rs`).
- [ ] R2 pack-size cap in `buildR2PackStore`, split delta-base budget in `buildSourceTar`, pre-lock receive buffering in `R2GitRepository.fetch`, and `ArtifactsMergeContainer` `/healthz` auth/timeouts.
- [ ] Direct native Git origin socket idle timeout / concurrency / gzip bounds (`scripts/git-origin.mjs`), loopback proxy secret via `GIT_CONFIG_*` env vars (`scripts/git-remote-shard.mjs`), and signal-safe temp secret cleanup (`scripts/deploy.mjs`, `scripts/admin.mjs`).

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
