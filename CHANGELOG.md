# Changelog

## 0.2.0

### Added

- `r2-direct` lets Git LFS clients upload directly to private R2 using signed single PUT requests.
- `r2-multipart` lets Git LFS clients use the packaged `scripts/git-lfs-r2-multipart.mjs` custom transfer agent for multipart uploads to private R2. Shard verifies the full object's SHA-256 and size after multipart assembly and before publishing it. Direct uploads also pass whole-object verification before publication.

### Changed

- GitHub releases use notes from the matching version section. The prebuilt archive includes the changelog and multipart transfer agent.

### Upgrade

- `proxy` remains the default and streams uploads through the Worker with the existing 100 MB limit. Downloads continue to use the Git LFS Basic transfer adapter in all modes.
- Apply the new D1 migration before deploying the Worker. To opt in, set `SHARD_LFS_UPLOAD_MODE` to `r2-direct` or `r2-multipart`, configure the R2 endpoint and bucket, and add the bucket-scoped S3 credentials and staging lifecycle rule. The [Git LFS documentation](https://github.com/aliveranme/shard/blob/main/README.md#git-lfs-upload-modes) covers Worker and client setup.

## 0.1.0

### Added

- Git hosting on Cloudflare Workers with Cloudflare Artifacts or a native Git origin, plus Git LFS storage in a private R2 bucket.
- Repository collaboration with issues, pull requests, protected merges, releases, organizations, team grants, and signed webhooks.
