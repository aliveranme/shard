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

</details>

## Features

- **Browse code** by branch, tag or commit. View the first 256 KiB of a file,
  stream the full file, or download a source archive pinned to a commit.
- **Collaborate** with scoped Git tokens, read and write collaborators, forks,
  public HTTPS imports, and transfers. Existing URLs keep their permission
  checks after rename or transfer. Private forks stay private.
- **Review changes** against a common ancestor. Approvals and line comments
  stay linked to the saved comparison. Merge locally, push, and verify the
  integration in Shard. Comparisons created before merge-base support keep
  their two-tip diff until updated.
- **Organize work** with searchable issues, labels, milestones, assignees, and
  a permission-aware inbox. Record CI results on commits or receive signed
  issue and review events through webhooks.
- **Customize your profile** with a public avatar. Upload, replace or remove it
  in **Account settings**. Avatars appear on profiles and in discussions.

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
Enable R2 and configure a separate `LFS_OBJECTS` bucket for each instance that
serves LFS. Use an isolated D1 database and Git storage for staging.
[Artifacts pricing](https://developers.cloudflare.com/artifacts/platform/pricing/) · [Custom domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/)

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
uses D1, static assets and a rate limiter on Cloudflare, and stores bare Git
repositories on the origin's disk. Repository URLs and Shard token scopes are
the same in both modes. New repositories and imports use `GIT_BACKEND`. A fork
uses its source's backend. Existing repositories retain their recorded backend.

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

Native imports accept final public HTTPS Git URLs from `github.com`,
`gitlab.com` and `codeberg.org` by default. Administrators can replace that list
with `SHARD_GIT_IMPORT_HOSTS` on the origin, using comma-separated trusted
hostnames. Back up the origin volume separately from D1. To keep serving legacy
Artifacts repositories from an existing database, retain the original Artifacts
binding alongside the native origin until those repositories have been migrated.
[Wrangler environments](https://developers.cloudflare.com/workers/wrangler/environments/) · [Git backend plan](PLAN.md#optional-git-backends)

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

Pushing a tag such as `v0.1.0` or `v0.1.0-rc.1` runs the release workflow. It
checks the Worker, runs the isolated Git and HTTP tests, and attaches a
prebuilt Worker archive and SHA-256 checksum to the GitHub release.
Suffixes such as `-rc.1` mark prereleases. The workflow runs Wrangler's dry
run against the archive before publishing it.

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

The web default branch must exist in Git. Clones follow the storage backend's
`HEAD`. Account changes that affect credentials require the current password.
Before deleting an account, transfer or remove its repositories. Deletion
revokes the account's sessions and tokens.

Passwords use PBKDF2-SHA256. Browser sessions use HTTP-only cookies. Session and
token secrets are stored as hashes. Browser forms require session-bound CSRF
tokens and same-origin requests. Failed logins are limited to eight per
source/account pair within 15 minutes. Recovery pages send only the site origin
as referrer. Git smart HTTP returns an empty HTTP 404 for a missing `.git` repository.

### Git LFS

Install Git LFS and use the regular Shard HTTPS remote. `git lfs track` stores
pointers in Git, while the LFS Batch endpoint transfers the files using the
basic adapter. Uploads require a write-scoped token and repository write access.
Public objects can be downloaded anonymously. Private downloads require a
repository grant. Each object is scoped to its repository and checked against
the declared SHA-256 and size. The limits are 100 objects per batch and
100,000,000 bytes per upload. R2 data needs its own backup.
Copy LFS objects separately when moving a repository or creating a Git mirror.
[Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)

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
has one current result per commit, shown in the commit detail. A successful write
returns `204`.

For signed issue and review events, set `WEBHOOK_REPO` to the stable Git storage
repository name, `WEBHOOK_ENDPOINT` to the receiver's HTTPS URL, and
`WEBHOOK_SECRET` as a Worker secret. Add `"* * * * *"` to `triggers.crons` alongside
the daily maintenance schedule. Verify `X-Shard-Signature` as `sha256=` followed
by the lowercase hex HMAC-SHA256 of the exact request body. Use `X-Shard-Event-Id`
to deduplicate deliveries from the Worker. Check `webhook_deliveries` for state,
attempt count and last HTTP status. The Worker attempts delivery up to six times.

## Backup and restore

Pause writes while taking a paired D1 and Git backup. Keep the SQL and each Git
mirror in private storage outside the workspace. Record each repository's
`repos.git_backend` and stable storage ID (`repos.artifacts_repo`) from D1.

For a loopback rehearsal, set `SHARD_SOURCE_D1` to the local persistence directory,
`SHARD_RESTORE_D1` to an empty persistence path, and `SHARD_BACKUP` to an absolute
backup directory. Set `SHARD_REPO_URL` to the clone URL for one repository with refs
and `SHARD_REPO_BACKUP` to its own absolute backup directory. Create both backup
directories before running these commands.

```sh
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 export DB --local --persist-to "$SHARD_SOURCE_D1" --output "$SHARD_BACKUP/shard.sql"
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 execute DB --local --persist-to "$SHARD_RESTORE_D1" --file "$SHARD_BACKUP/shard.sql"
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
[D1 export and import](https://developers.cloudflare.com/d1/best-practices/import-export-data/) · [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)

Worker responses include `X-Request-ID` for log correlation. Daily maintenance
at 03:00 UTC removes expired sessions, tokens, login throttle windows, and email links.

## Limits

- Pull request comparisons report missing Git objects and traversal limits.
  Shard requires a unique best common ancestor and reports unrelated branches
  or multiple best common ancestors explicitly. Text diffs cover UTF-8 files
  up to 96 KiB and 512 lines per side. Historical diffs depend on the saved Git
  objects remaining available. Merge verification separately uses bounded
  ancestry traversal and requires the saved branch tips to be reachable.
- Branch and tag discovery reads at most 512 advertised refs within 256 KiB.
  Create, update and delete refs through Git.
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
- The binding exposes object and history reads. Shard uses Git HTTPS for ref
  discovery and leaves ref changes and merges to the Git client.
- Commit metadata uses `treeHash` and `authoredAt`/`committedAt` (Unix seconds);
  tree entries are `{ name, mode, type, hash }`.
- Git remotes accept a Bearer repo token; for Basic auth the password is the token
  secret before its `?expires=` suffix.

[cloudflare-deploy]: https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Faliveranme%2Fshard
