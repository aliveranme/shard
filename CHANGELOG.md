# Changelog

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
