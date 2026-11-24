# Shard Audit TODO

Tracked findings from the full-repository code reviews.

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
