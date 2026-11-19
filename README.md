<h1><img src="assets/readme/shard.svg" width="100%" alt="Shard, a self-hosted Git forge on Cloudflare Workers"></h1>

Shard brings public and private repositories, issues, and pull request review
together for personal projects and small teams. Standard Git clients clone,
fetch, and push over HTTPS. The web interface supports English and Simplified Chinese.

[![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)][cloudflare-deploy]

[Run locally](#local-development) · [First push](#first-repository) · [Deploy guide](#deploy-to-cloudflare) · [简体中文](#简体中文)

<a href="assets/readme/pull-request.webp"><img src="assets/readme/pull-request.webp" width="100%" alt="Shard pull request with a versioned comparison, changed files, and a review comment linked to an added line"></a>

A line review in `demo/field-notes` on a local Shard instance.

## Local development

Install Git 2.32+ and mise, then run these commands from the checkout.

```sh
mise install
mise run setup
mise run dev
```

Open <http://localhost:8787>. Wrangler runs D1 and Artifacts locally, so you can
try Shard without a Cloudflare account.

### First repository

1. Create an account at `/user/sign_up`. Registration is open on the instance.
2. Choose **New repository** and set its visibility. Create a write token in
   **Account settings → Access tokens** and save it when Shard displays it.
3. Copy the repository's clone URL into `SHARD_CLONE_URL`. When Git prompts for
   the push, enter your Shard username and use the write token as the password.

```sh
git clone "$SHARD_CLONE_URL" my-repository
cd my-repository
printf '# My repository\n' > README.md
git add README.md
git commit -m "Initial commit"
git push -u origin HEAD:main
```

<details>
<summary>Build, tests, and editor setup</summary>

`mise run check` type-checks the Worker, `mise run lint` runs Clippy, and
`mise run build` produces `build/index.js`. `mise run test` builds and tests
both Git backends with isolated D1 databases and real Git clients. `npm test`
runs the tests against an existing build. Keep Git on `PATH`, including in CI.

`mise run setup` installs the pinned toolchain, editor tools and npm dependencies.
For rust-analyzer, set the Cargo target to `wasm32-unknown-unknown`.

The divergent merge regression requires Git 2.38 or later. When `git lfs` is
installed, both backends also run a real LFS push and clone.
</details>

## Features

- **Browse and search code** by Git ref. Find paths, text, and commits within a
  bounded scan. Results link to the searched commit. View a file, stream the
  full contents, or download a commit-pinned source archive.
- **Collaborate** with scoped Git tokens, repository collaborators, organization
  teams, forks, imports, and transfers. Existing URLs keep their permission
  checks after rename or transfer.
- **Review and merge** pull requests from a branch or another repository.
  Accepted pushes schedule a comparison refresh. Reviews and line comments
  link to their original snapshot. Protected branches can require approvals
  and CI results before the merge queue advances the target ref.
- **Organize work** with issue templates, reactions, label and assignee filters,
  milestones, repository watches and stars, and a permission-aware inbox.
  Repository writers can draft and publish releases pinned to Git tags.
- **Integrate** CI results and signed repository webhooks for issues, pull
  requests, branch updates, and tag updates.
- **Customize your profile** with an uploaded avatar. Commit lists, branch
  summaries, and commit details use it when the Git author email matches
  your account email. Other authors use their initials.

## Architecture

The Rust Worker serves the web interface and checks repository access before
forwarding Git traffic. D1 keeps accounts, permissions, issues, and reviews.
Cloudflare Artifacts stores Git data by default. With `--env native`, an
authenticated Git origin stores bare repositories on a persistent disk. Both
modes store Git LFS bytes in the private `LFS_OBJECTS` R2 binding.

[Storage design](PLAN.md#optional-git-backends) · [Verified behavior and staging status](STATUS.md)

## Deploy to Cloudflare

The default Artifacts deployment requires **Workers Paid** with Artifacts access.
The Worker can use its `workers.dev` address or a Cloudflare custom domain.
Enable R2 and configure a private `LFS_OBJECTS` bucket for each instance.
Use separate D1, Git storage, and R2 resources for staging.
[Artifacts pricing](https://developers.cloudflare.com/artifacts/platform/pricing/) · [Custom domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/)

### Deployment limits

Set `SHARD_LIMIT_PROFILE` in Wrangler `vars` to `free` or `paid`. The
Artifacts configuration uses `paid`, the `native` environment uses `free`,
and a configuration that omits the variable uses `free`. Set
`env.native.vars.SHARD_LIMIT_PROFILE` to `paid` when a native deployment uses
a Workers Paid account and needs larger batches. Release archives use the
same settings in `wrangler.json`.

| Work | `free` | `paid` |
| --- | ---: | ---: |
| Repository copy checks per tick | 2 | 8 |
| Repository cleanups per tick | 1 | 8 |
| R2 objects per copy page | 2 | 8 |
| R2 keys per cleanup page | 64 | 256 |
| Unfinished push checks per tick | 1 | 12 |
| PR refreshes per tick | 1 | 24 |
| Merge queue entries per tick | 1 | 12 |
| Webhook deliveries per tick, per delivery mechanism | 1 | 16 |

The default Git LFS mode (`proxy`) streams uploads through the Worker. Its
`SHARD_LFS_MAX_BYTES` default is `100000000`. The `free` profile can lower it.
The `paid` profile can raise it up to the lower of the domain's request-body
limit and R2's single-PUT limit of `5363466240` bytes. The domain's Free and
Pro plans accept 100 MB and Business accepts 200 MB. The two R2 upload modes
use the [limits below](#git-lfs-upload-modes).

Workers Free has 10 ms of CPU per HTTP or cron invocation and 50 external
subrequests per invocation. D1 Free allows 50 queries per Worker invocation
and 100,000 written rows per day across the account. Workers and R2 usage
follow their account and zone settings when you change the Shard profile.
[Workers limits](https://developers.cloudflare.com/workers/platform/limits/) · [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) · [R2 limits](https://developers.cloudflare.com/r2/platform/limits/)

Active Worker HTTP requests can stream while the client is connected.
An HTTPS origin behind Cloudflare's proxy has a default 125-second read
timeout and 30-second write timeout. Configure the origin proxy to send
and receive streams. Shard gives each proxied LFS upload 15 minutes for its
body and tracks unfinished Git pushes in D1 for scheduled recovery.
[Worker duration](https://developers.cloudflare.com/workers/platform/limits/#duration) · [Proxy timeouts](https://developers.cloudflare.com/fundamentals/reference/connection-limits/)

### Deploy from GitHub or GitLab

1. Click **Deploy to Cloudflare** at the top of this page and select your
   Cloudflare account.
2. Connect GitHub or GitLab and choose the destination repository, Worker name,
   and D1 database name. Cloudflare copies Shard into your Git account and
   provisions the `DB` binding.
3. Keep the repository root as the build root, `npm run build` as the build
   command, and `npm run deploy` as the deploy command. Click **Deploy** and
   open the `workers.dev` URL when Workers Builds finishes.

The badge uses the public Shard repository as its source. If you import an
existing copy directly through Workers & Pages, [create and configure its D1 database](#deploy-from-a-local-checkout)
before the first build. A custom build API token needs Workers Scripts Edit
and D1 Edit in the target Cloudflare account.
[Deploy to Cloudflare buttons](https://developers.cloudflare.com/workers/platform/deploy-buttons/) · [Workers Builds settings](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)

Workers Builds uses the Node version in `.node-version` and installs the pinned
Rust toolchain and `worker-build`. The deploy command applies migrations to
the `DB` binding before publishing the Worker. Artifacts creates the configured
namespace when Shard creates its first Git repository.
[Artifacts namespaces](https://developers.cloudflare.com/artifacts/concepts/namespaces/)

### Deploy from a local checkout

Set up the toolchain as shown in the local development section. Authenticate,
then create a production D1 database in the same account as the Worker.

```sh
mise exec -- npx wrangler login
mise exec -- npx wrangler d1 create shard
```

Copy the database ID into `d1_databases[0].database_id` in `wrangler.jsonc`.
If you chose another database name, update `database_name` and keep the binding
named `DB`. Run `mise run deploy` to apply remote migrations and deploy.

### Deploy with a native Git origin

Choose the `native` Wrangler environment to use a Git server you operate. It
uses D1, R2, static assets and a rate limiter on Cloudflare, and stores bare Git
repositories on the origin's disk. Repository URLs and Shard token scopes are
the same in both modes. `GIT_BACKEND` selects storage for new repositories and
credential-free imports. A fork uses its source's backend. Existing repositories
retain their recorded backend.

1. Run `node scripts/git-origin.mjs` on a host with the pinned Node version and
   Git 2.32 or later. Set `SHARD_GIT_DIRECTORY` to a dedicated directory on a
   persistent volume and `SHARD_GIT_TOKEN` to a randomly generated shared
   secret. The service defaults to `127.0.0.1:8793`. Run one process per volume
   and put it behind an HTTPS reverse proxy that streams request bodies.
2. In `env.native` in `wrangler.jsonc`, set `NATIVE_GIT_ORIGIN` to the proxy's
   HTTPS origin. Set the native D1 database ID and name and the native
   `LFS_OBJECTS` bucket name. Set `NATIVE_GIT_TOKEN` to the same value as
   `SHARD_GIT_TOKEN`. Use a dedicated origin directory and token per instance.
3. Apply migrations and deploy the same environment:

   ```sh
   mise exec -- npx wrangler secret put NATIVE_GIT_TOKEN --env native
   mise exec -- npx wrangler d1 migrations apply DB --env native --remote
   mise exec -- npx wrangler deploy --env native
   ```

For Windows or Linux local development, start the origin with a disposable
directory and token. Put `NATIVE_GIT_TOKEN` in `.dev.vars.native` and run:

```sh
mise exec -- npx wrangler d1 migrations apply DB --env native --local
mise exec -- npx wrangler dev --env native --local
```

Native imports accept final HTTPS Git URLs from `github.com`, `gitlab.com`,
and `codeberg.org` by default. Set `SHARD_GIT_IMPORT_HOSTS` on the origin to
replace the list with trusted hostnames. The import form accepts Basic or
Bearer credentials. Credentialed copies use the configured native origin even
when Artifacts is the default. They start as private repositories, and the
minute cron advances pending copies.

Back up the origin volume separately from D1. To keep serving legacy Artifacts
repositories, retain the Artifacts binding alongside the native origin until
those repositories are migrated. Native non-fast-forward Web merges require
Git 2.38 or later.
[Wrangler environments](https://developers.cloudflare.com/workers/wrangler/environments/) · [Git backend plan](PLAN.md#optional-git-backends)

### Web merges on Artifacts

Artifacts supports same-repository fast-forwards through Git HTTPS. For a
divergent or cross-repository merge between Artifacts repositories, point the
Worker's `NATIVE_GIT_ORIGIN` and `NATIVE_GIT_TOKEN` at a Git 2.38+ origin and
configure `SHARD_GIT_ARTIFACTS_ORIGINS` there with the exact HTTPS origin from
your Artifacts repository remotes. The origin stages the reviewed objects and
pushes one branch with a lease on the expected target tip. Mixed-backend Web
merges require manual integration and verification.

### Optional email setup

Set `PUBLIC_ORIGIN` to the HTTPS origin of your instance and `EMAIL_FROM` to an
address on your onboarded sending domain. Add the `EMAIL` binding via Wrangler
`send_email`. Verification links expire after 24 hours. Password-reset links
expire after 30 minutes and a successful reset revokes all browser sessions for
the account. [Cloudflare Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/)

## 简体中文

Shard 是部署在 Cloudflare 上的自托管 Git 平台，适用于个人项目和小团队。D1 保存账户、权限、议题和审查记录。Git 默认使用 Artifacts，也可配置原生 Git 源站。界面语言可在页脚或账号设置中切换。

点击顶部的 **Deploy to Cloudflare** 徽章，选择 Cloudflare 账户，连接 GitHub 或 GitLab，并确认 Worker 和 D1 数据库名称。Cloudflare 会复制源码、创建 D1 数据库、构建 Worker 并应用迁移。部署需要开通 **Workers Paid** 并可使用 Artifacts。 [官方部署按钮文档](https://developers.cloudflare.com/workers/platform/deploy-buttons/)

在本地试用无需 Cloudflare 账号。安装 Git 2.32 或更高版本和 mise 后，运行 `mise install`、`mise run setup`、`mise run dev`，打开 `http://localhost:8787`。

需要避开 Artifacts 时，选择 `--env native`，并配置自有 Git 源站、持久磁盘和 D1。详见 [原生 Git 部署说明](#deploy-with-a-native-git-origin)。

## Project releases

Add an exact `## <version>` section to [CHANGELOG.md](CHANGELOG.md) in the
tagged commit. Pushing `v0.2.0` publishes that version's notes. A prerelease
such as `v0.2.0-rc.1` requires its own `## 0.2.0-rc.1` section and is marked
as a prerelease on GitHub. Check the selected notes locally with
`mise exec -- node scripts/release-notes.mjs v0.2.0`.

The workflow type-checks and builds the Worker, runs the isolated regressions,
and dry-runs both deployment environments from the prebuilt package. It
checks the archive's SHA-256 and uploads the archive and checksum to a draft
before publishing the GitHub Release.

To deploy the prebuilt archive, verify its checksum and extract it. Use the
pinned Node version from [`.node-version`](.node-version) and set the D1 name
and ID in the extracted `wrangler.json`, following the
[local checkout setup](#deploy-from-a-local-checkout). From the extracted directory:

```sh
npx --yes wrangler@4.164.0 d1 migrations apply DB --remote
npx --yes wrangler@4.164.0 deploy
```

For native mode, follow the origin and secret setup above, use the packaged
`scripts/git-origin.mjs`, and add `--env native` to both Wrangler commands.

[Release workflow](.github/workflows/release.yml) · [GitHub Releases](https://github.com/aliveranme/shard/releases)

## URL shape

Branch names contain slashes, so branches are a query parameter:

```
/{owner}/{repo}?branch=main
/{owner}/{repo}/src/{file-path}?branch=feature%2Freadme
/{owner}/{repo}/blob/{file-path}?branch=main
/{owner}/{repo}/raw/{file-path}?branch=main
/{owner}/{repo}/commits?branch=feature%2Freadme&start={commit-hash}
/{owner}/{repo}/refs
/{owner}/{repo}/commit/{commit-hash}
/{owner}/{repo}/archive/{commit-hash}
```

Repository and issue lists accept `page` (20 items per page); search and state
filters survive navigation. Discussion pages use `comments_page` and include
both comments and close/reopen events. Commit history follows first parents:
the `start` cursor identifies the first commit on the next page, so each request
reads at most 21 commits. “Latest commits” returns to the selected branch tip.

## Git access

Shard checks the Git client's Shard token and repository access, then streams
the Smart HTTP request and response through its recorded Git backend.
Artifacts requests use a short-lived repository token. Native origin requests
use the instance's shared secret. Git clients use the Shard URL in both modes.

Create tokens under `/user/settings/tokens`, choosing read/write scope and a
7/30/90/365-day lifetime (30 days by default). In a private repo, owners and
collaborators with either role can clone using either token scope. Pushing
requires both a write-scoped token and repository write permission. Expired or
revoked tokens are rejected. Legacy tokens without an expiry remain valid until
revoked. The secret is shown only at creation. Use it as the password when Git
prompts. Anonymous clone works for public repositories.

The default branch must exist in Git. Changing it also changes the branch
new clones select through Shard. Use **Save and sync Git HEAD** to synchronize
an existing default. Native storage writes the symbolic ref directly. Artifacts
advertises the configured branch through Shard's upload-pack proxy using the
classic Git advertisement. Account changes that affect credentials require the
current password.
Before deleting an account, transfer or remove its repositories. Deletion
revokes the account's sessions and tokens.

Passwords use PBKDF2-SHA256. Browser sessions use HTTP-only cookies. Session and
token secrets are stored as hashes. Browser forms require session-bound CSRF
tokens and same-origin requests. Failed logins are limited to eight per
source/account pair within 15 minutes. Recovery pages send only the site origin
as referrer. Git smart HTTP returns an empty HTTP 404 for a missing `.git` repository.

### Git LFS

Install Git LFS and use the regular Shard HTTPS remote. `git lfs track` stores
pointers in Git and the Batch endpoint negotiates object transfers. Uploads
require a write-scoped token and repository write access. Downloads use the
Basic adapter in every mode. Public objects can be downloaded anonymously
and private downloads require a repository grant.

Each object is scoped to its repository and verified against its declared
SHA-256 and size. A batch can contain up to 100 objects. A fork publishes
after copying its LFS objects into an independent R2 namespace. Repository
deletion removes access immediately and schedules Git and R2 cleanup. Copy
LFS objects separately for external Git imports and mirrors.
[Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)

#### Git LFS upload modes

Set `SHARD_LFS_UPLOAD_MODE` in `vars` for Artifacts or `env.native.vars` for
the native origin. Apply D1 migration `0026_lfs_multipart.sql` when upgrading.

| Mode | Client | New object limit |
| --- | --- | --- |
| `proxy` (default) | Standard Git LFS | 100,000,000 bytes by default. The [deployment limit](#deployment-limits) also applies. |
| `r2-direct` | Standard Git LFS | 5,363,466,240 bytes per single PUT. A failed PUT restarts the object. |
| `r2-multipart` | Git LFS with the Shard upload adapter | 5,363,466,240 bytes with `free`. With `paid`, R2 permits up to 4.995 TiB per object. Set a practical limit for the verifier and origin throughput. |

Set `SHARD_LFS_MAX_BYTES` in bytes to lower either R2 mode's limit. Uploads
in both R2 modes go from the client to the R2 S3 endpoint and bypass the
Shard domain's request-body size limit. Multipart uploads can resume recorded
parts for up to six days. The complete operation verifies the file before
it becomes visible in the repository. With large objects above the single
PUT ceiling, the Worker streams the staged object through SHA-256 and uses
R2 multipart copy to publish it. Budget CPU and wall time for this step and
for copying objects into forks. Each publication has a 15-minute window.

Both R2 modes use the following settings in the same Worker environment.

| Setting | Value |
| --- | --- |
| `SHARD_LFS_R2_ENDPOINT` | The bucket's HTTPS S3 endpoint, such as `https://<account-id>.r2.cloudflarestorage.com` |
| `SHARD_LFS_R2_BUCKET` | The same private bucket as `LFS_OBJECTS` |
| `SHARD_LFS_R2_ACCESS_KEY_ID` | Worker secret for a bucket-scoped R2 Object Read & Write credential |
| `SHARD_LFS_R2_SECRET_ACCESS_KEY` | Worker secret for that credential |

Add a bucket lifecycle rule for `lfs-incoming/` that expires staged objects
and aborts incomplete multipart uploads after seven days. For example, use
the R2 dashboard or run the following command with the configured bucket.

```sh
npx --yes wrangler@4.164.0 r2 bucket lifecycle add "$SHARD_LFS_R2_BUCKET" shard-lfs-staging lfs-incoming/ --expire-days 7 --abort-multipart-days 7
```

Published objects live under `lfs/`. Keep the S3 settings available
after changing modes if repositories contain objects above the single-PUT
limit, since large fork copies still use the S3 API.
[R2 API credentials](https://developers.cloudflare.com/r2/api/tokens/) · [R2 lifecycle rules](https://developers.cloudflare.com/r2/buckets/object-lifecycles/)

For `r2-multipart`, install the pinned Node version and configure the
packaged `scripts/git-lfs-r2-multipart.mjs` in each Git checkout.

```sh
git config --local lfs.customtransfer.shard-r2-multipart.path node
git config --local lfs.customtransfer.shard-r2-multipart.args '"/absolute/path/to/scripts/git-lfs-r2-multipart.mjs"'
git config --local lfs.customtransfer.shard-r2-multipart.direction upload
```

Use the same `git push` and `git lfs push` commands after setup. The adapter
uses Shard only for control requests. It streams missing parts to R2 and
reports success after Shard verifies and publishes the assembled object.
[Git LFS custom transfers](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md)

## Images and issue references

An avatar can be a PNG, JPEG or WebP image up to 512 KiB and 1024 × 1024 px.
Shard saves a metadata-free PNG thumbnail up to 128 × 128 px in D1.

README files, Markdown file previews and discussions render Markdown images and
links. For example:

```markdown
![Field map](images/site-map.png)
[Survey notes](docs/survey.md#measurements)
![Public status](https://assets.example.org/status.png)

<img src="images/site-map.png" alt="Field map" width="320">

See #12, GH-12, and mira/field-journal#34.
```

Repository-relative URLs in a file use the displayed commit and resolve from that
file's directory. In a discussion they use the repository root on its default
branch. A line review uses the saved commit and directory for the reviewed side.
Repository media uses the same access checks as source files. The browser loads
external HTTPS images without a referrer. HTML image tags can use `src`, `alt`,
`title`, `width`, and `height`. Shard escapes other HTML tags and discards other
attributes from rendered images.

Issue references open the issue or pull request with that number in Shard.
Paste an HTTPS URL to link directly to a GitHub issue or pull request.

## Repository workflows

**Branches and tags** lets repository owners create a branch or lightweight tag
from a commit ID, or remove a ref after confirming its name. **Settings → Branch
protection** adds rules for exact branch names. Protected refs accept changes
through the PR merge queue. Required check results belong to the reviewed head
commit and the target repository. Once a comparison changes, review its new
version before queuing again.

**Settings → Organizations** lets a repository owner offer the repository to an
organization. The organization owner accepts it and grants teams read or write
access. Repository ownership and its URL stay with the account that offered it.

**Settings → Import metadata** accepts a version 1 JSON bundle for issues,
pull requests, and releases. The form includes the schema and size limits.
The destination Git store must contain the referenced commits and release tags.
Each bundle imports in one D1 transaction.

## Interface language

Open **Language** in the footer or **Account settings → Language**. Choose
English, 简体中文 (Simplified Chinese), or **Use browser language**.
Signed-in choices are stored in D1 and apply across sessions. If an account has
no saved preference, Shard uses the browser's language cookie, then its
`Accept-Language` priorities. Unsupported languages fall back to English.
The **Use browser language** setting explicitly selects the browser header.

The Worker translates Web UI labels, validation messages, errors, accessibility
labels, relative dates and counts. Repository contents, issues and commit
messages appear in the language their authors used.

To add a locale, register its tag and display label in `src/i18n/mod.rs`, add
its text to each catalog in `src/i18n/`, and add its formatting and negotiation
rules. Extend the D1 locale constraint with a forward migration. Keep
interpolated messages together so translators can change word order. Templates
escape the output, including interpolated values.

## Integrations

To record a check, POST JSON to `/{owner}/{repo}/commit/{hash}/checks` with
`Authorization: Bearer <token>` and a write-scoped token belonging to a repository
writer. Send `{ "context": "ci/unit", "state": "success", "summary": "42 tests passed" }`.
The accepted states are `pending`, `success`, `failure` and `error`. Each context
has one current result per commit. Results appear on the commit detail and the
corresponding pull request. A successful write returns `204`.
For a cross-repository pull request, a target writer with source access can
report a check for the saved head SHA at the target repository's endpoint.

To manage signed repository webhooks, configure `WEBHOOK_ENCRYPTION_KEY` as a
Worker secret containing 32 random bytes in hex, then open **Settings → Webhooks**.
Add an HTTPS URL on a public hostname and a signing secret. Each repository can
save up to 10 subscriptions. The minute cron retries a delivery up to six
attempts. Events are retained for 30 days and settings shows five recent
deliveries per subscription.

Verify `X-Shard-Signature` as `sha256=` followed by the lowercase hex HMAC-SHA256
of the exact request body. `X-Shard-Event-Id` identifies the event and
`X-Shard-Delivery-Id` identifies the subscription delivery across retries.
Git events include `ref`, `before`, and `after`. A missing tip is JSON `null`.
For a push through Shard, D1 records the proposed ref transitions before
forwarding the pack. A complete Git receive report commits the ref event,
Webhook outbox, and PR refresh together. If that report is lost, the minute
cron checks the live refs for up to one hour. A recovered observation has
an unknown actor. Intermediate updates superseded before observation can
be missed.

Existing instance-level `WEBHOOK_REPO`, `WEBHOOK_ENDPOINT`, and `WEBHOOK_SECRET`
configuration continues to deliver issue and review events.

## Backup and restore

Pause writes and scheduled cleanup while taking a paired D1, Git, and R2
backup. Keep the SQL, each Git mirror or volume snapshot, and the R2 `lfs/`
prefix in private backup storage. Record each repository's `id`,
`git_backend`, stable storage ID (`artifacts_repo`), and `lfs_copy_id` from D1.

For an in-place hosted D1 recovery, record a Time Travel bookmark while writes
are paused. Bookmarks cover seven days on Workers Free and 30 days on Workers
Paid. Keep a SQL export for recovery into a new database.

For a loopback rehearsal, set `SHARD_RESTORE_D1` to an empty persistence path
and `SHARD_BACKUP` to an absolute backup directory. Local D1 export reads
`.wrangler/state` next to the Wrangler configuration. If the source used
`--persist-to`, copy its `v3/d1` directory into `.wrangler/state/v3/` next to
a temporary copy of the configuration and pass that copy with `--config`.
For the native environment, add `--env native` to both D1 commands.

Set `SHARD_REPO_URL` to the clone URL for one repository with refs and
`SHARD_REPO_BACKUP` to its own absolute backup directory. Create both backup
directories before running these commands.

```sh
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 export DB --local --output "$SHARD_BACKUP/shard.sql"
```

Copy `shard.sql` to `shard-restore.sql`. With Wrangler 4.164.0, move the
`CREATE VIEW git_ref_observations` statement before `CREATE TRIGGER git_ref_observe`
in the import copy. The local exporter writes triggers before views.

```sh
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 execute DB --local --persist-to "$SHARD_RESTORE_D1" --file "$SHARD_BACKUP/shard-restore.sql"
git clone --mirror "$SHARD_REPO_URL" "$SHARD_REPO_BACKUP/source.git"
git -C "$SHARD_REPO_BACKUP/source.git" bundle create "$SHARD_REPO_BACKUP/source.bundle" --all
git clone --mirror "$SHARD_REPO_BACKUP/source.bundle" "$SHARD_REPO_BACKUP/restored.git"
git -C "$SHARD_REPO_BACKUP/restored.git" fsck --full
```

Compare D1 row counts and check that `git for-each-ref` returns the same names and
object IDs in both mirrors. For an empty repository, retain the mirror and its
D1 record. Restore storage to the expected IDs before serving the restored
database. For native storage, preserve the bare-repository volume, including
objects from deleted branches that archived pull request comparisons still use.

For R2, configure private S3 remotes in rclone. Set `SHARD_R2_SOURCE` and
`SHARD_R2_RESTORE` to values of the form `remote:bucket`, pointing to the
source and an empty restore bucket. Use the same backup directory as the
D1 export and preserve the complete keys under `lfs/`, including
`lfs/<repo-id>/<oid>` and `lfs/fork-<copy-id>/<oid>`.

```sh
rclone copy "$SHARD_R2_SOURCE/lfs" "$SHARD_BACKUP/r2/lfs" --immutable
rclone check "$SHARD_R2_SOURCE/lfs" "$SHARD_BACKUP/r2/lfs" --download
rclone copy "$SHARD_BACKUP/r2/lfs" "$SHARD_R2_RESTORE/lfs" --immutable
rclone check "$SHARD_BACKUP/r2/lfs" "$SHARD_R2_RESTORE/lfs" --download
```

Bind the restored bucket with its matching D1 and Git backup. Clone a
repository and a fork with LFS and verify that saved PR revisions also open
before resuming traffic. [R2 with rclone](https://developers.cloudflare.com/r2/examples/rclone/)

[D1 export and import](https://developers.cloudflare.com/d1/best-practices/import-export-data/) · [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)

Worker responses include `X-Request-ID` for log correlation. Daily maintenance
at 03:00 UTC removes expired sessions, tokens, login throttle windows, and email links.

## Limits

- Pull request comparisons report missing Git objects and traversal limits.
  Shard requires a unique best common ancestor and reports unrelated branches
  or multiple best common ancestors explicitly. Text diffs cover UTF-8 files
  up to 96 KiB and 512 lines per side. Same-origin native repositories have
  one authenticated origin request per merge-base, file-tree comparison, or
  ancestry-verification step, with a five-second Git operation budget and
  up to 256 changed files. Artifacts and mixed-backend comparisons use
  bounded object traversal. Historical diffs require the saved objects.
- Branch and tag discovery reads at most 512 advertised refs within 256 KiB.
  Search is scoped to one Git snapshot. Commit search follows up to 24
  first-parent commits. Code search skips non-text files and files over 64 KiB
  and reports when the scan or result budgets are reached.
- Source downloads contain regular files and directories up to 16 MiB and 2,000
  entries in `ustar` format. Use a Git mirror for history, symlinks and submodules.
- The edge limit is 300 requests per minute per source and traffic class, counted
  separately at each Cloudflare location. [Rate Limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/)
- Historical issue closures from before state history was introduced appear without
  an actor.

## Layout

- Worker routes, permissions, and Git backends live in [src/](src/).
- Server-rendered HTML and styles live in [templates/](templates/) and [static/](static/).
- D1 schema changes live in [migrations/](migrations/).
- The build bootstrap and native Git origin live in [scripts/](scripts/).
- Isolated HTTP and Git scenarios live in [tests/](tests/).

## Design

Shard follows the system's light or dark appearance. Geist and Shard Han set
interface text with tabular figures. JetBrains Mono sets code, paths and Git
identifiers. Askama renders the pages. Repository tabs wrap on narrow screens.
Supporting browsers animate same-origin page changes, menus and native pickers
according to the system's reduced-motion preference.
[Design guide](DESIGN.md) · [Palette](static/css/theme.css) · [Stylesheet](static/css/shard.css)

## Prior art

- [ripgit](https://github.com/deathbyknowledge/ripgit) — one Durable Object per repo,
  SQLite objects, FTS5, and text/markdown negotiation for agents.
- [git-on-cloudflare](https://github.com/zllovesuki/git-on-cloudflare) — Smart HTTP
  v2 with Durable Object refs, R2 packs and a streaming push pipeline.
- [durable-git](https://github.com/littledivy/durable-git) — Git implemented inside a
  Durable Object, optional R2 pack cache, cgit-style surface.

## Cloudflare API notes (verified 2026-11-17)

- `wrangler@4.164.0`'s schema accepts `artifacts: [{ binding, namespace, remote? }]`.
  The local simulator requires Wrangler ≥ 4.163 and Git ≥ 2.32 on `PATH`.
- Workers binding surface: namespace `create/get/list/import/delete`; repo handle
  `info/createToken/listTokens/revokeToken/fork/log/readCommit/readTree/readBlob/readFile`.
- Shard uses Git HTTPS for ref discovery and supported ref writes. The optional
  native origin stages Artifacts merges that require a pack with new objects.
- Commit metadata uses `treeHash` and `authoredAt`/`committedAt` (Unix seconds);
  tree entries are `{ name, mode, type, hash }`.
- Git remotes accept a Bearer repo token; for Basic auth the password is the token
  secret before its `?expires=` suffix.

[cloudflare-deploy]: https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Faliveranme%2Fshard
