# Changelog

## Unreleased

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
