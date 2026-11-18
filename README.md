# Shard

Shard is a self-hosted Git forge for personal projects and small teams on
Cloudflare Workers. It hosts public and private repositories, issues, and pull
requests in the same app. Standard Git clients clone, fetch, and push over
HTTPS. The browser interface supports English and Simplified Chinese.

[![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)][cloudflare-deploy]

[Deploy](#deploy-to-cloudflare) · [First repository](#first-repository) · [Local development](#local-development) · [简体中文](#简体中文)

## Features

- **Repositories** support read and write collaborators, forks, public HTTPS
  imports, and transfers. Previous Web and Git URLs retain their access checks
  after rename or transfer. Forks of private repositories remain private.
  Browse branches, tags, files, and commit history or download a source archive
  pinned to a commit. The web default branch must exist in Git. Clones follow
  the Artifacts remote `HEAD`.
- **Issues** support editing, labels, milestones, assignees, state history, and
  search across titles, bodies, and comments. The inbox shows activity in
  repositories you can still access.
- **Pull requests** compare the source branch with its best common ancestor.
  Authors and writers can update the saved comparison. Approvals, change
  requests, and line comments link to the version reviewed. Comparisons saved
  before merge-base support retain their original two-tip diff until updated.
  Merge locally, push, then verify the integration in Shard.
- **Accounts and integrations** include scoped Git tokens, commit checks, and
  signed issue and review webhooks. Account settings support password-confirmed
  credential changes. Account deletion requires transferring or removing owned
  repositories and revokes sessions and access tokens.

## Architecture

| Layer | Responsibility |
|---|---|
| Cloudflare Workers | Rust compiled to WebAssembly with `workers-rs`. Handles Web and Git requests. |
| D1 (`DB`) | Accounts, sessions, access tokens, repository metadata, issues, reviews and notifications. |
| Cloudflare Artifacts (`ARTIFACTS`) | Git objects and refs. Each Shard repository has its own Artifacts repository. |
| Askama and Workers Static Assets (`ASSETS`) | Server-rendered pages and responsive CSS. [Design guide](DESIGN.md) |

Local verification and the delivery plan cover tested workflows and remaining
platform requirements. [Project status](STATUS.md) · [Delivery plan](PLAN.md)

## Deploy to Cloudflare

Shard requires a Cloudflare account on **Workers Paid** with Artifacts access.
The Worker can use its `workers.dev` address or a Cloudflare custom domain.
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

### First repository

1. Open your instance and create an account at `/user/sign_up`. Registration
   is open on the instance.
2. Choose **New repository** and set its visibility. Create a write token in
   **Account settings → Access tokens** and save it when Shard displays it.
3. Copy the HTTPS clone URL from the repository page. Use your Shard username
   and the write token as the password when Git prompts for a push. New
   repositories use `main` by default.

```sh
git clone "$SHARD_CLONE_URL" my-repository # Set this to the URL copied from Shard.
cd my-repository
printf '# My repository\n' > README.md
git add README.md
git commit -m "Initial commit"
git push -u origin HEAD:main
```

### Optional email setup

Set `PUBLIC_ORIGIN` to the HTTPS origin of your instance and `EMAIL_FROM` to an
address on your onboarded sending domain. Add the `EMAIL` binding via Wrangler
`send_email`. Verification links expire after 24 hours. Password-reset links
expire after 30 minutes and a successful reset revokes all browser sessions for
the account. [Cloudflare Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/)

## 简体中文

Shard 是部署在 Cloudflare 上的自托管 Git 平台，适用于个人项目和小团队。数据库保存账户、权限、议题和审查记录，Git 对象由 Artifacts 管理。界面语言可在页脚或账户设置中切换。

点击顶部的 **Deploy to Cloudflare** 徽章，选择 Cloudflare 账户，连接 GitHub 或 GitLab，并确认 Worker 和 D1 数据库名称。Cloudflare 会复制源码、创建 D1 数据库、构建 Worker 并应用迁移。部署需要开通 **Workers Paid** 并可使用 Artifacts。 [官方部署按钮文档](https://developers.cloudflare.com/workers/platform/deploy-buttons/)

在本地试用无需 Cloudflare 账号。安装 Git 2.32 或更高版本和 mise 后，运行 `mise install`、`mise run setup`、`mise run dev`，打开 `http://localhost:8787`。

## Layout

```
src/
  lib.rs            fetch handler + route table
  auth.rs           sessions, login, git credentials
  crypto.rs         PBKDF2 / SHA-256 / random tokens via WebCrypto
  security.rs       same-origin/CSRF checks and browser security headers
  pagination.rs     bounded list pagination and query-preserving links
  i18n/             typed UI message catalogs and language negotiation
  account.rs        account and access-token page models
  recovery.rs       email verification and password reset
  artifacts.rs      Rust wrapper over the Artifacts Workers binding
  refs.rs           read-only branch and tag discovery over Git HTTPS
  repo_lifecycle.rs stable repository paths, transfers, forks and imports
  repo_objects.rs   commit detail and source archives
  pull_base.rs      exact branch tips and bounded common-ancestor lookup
  pull_diff.rs      file and line differences for a saved comparison
  pull_review.rs    comparison updates, review decisions and merge verification
  issue_metadata.rs issue edits and classification
  notifications.rs inbox and current repository access checks
  webhooks.rs       signed event delivery from the D1 outbox
  handlers/
    web.rs          dashboard, explore, global issue lists
    auth_handlers.rs registration, login, account language, access tokens
    repos.rs        create/browse/settings, trees, blobs, commits
    issues.rs       issue and pull request lists, discussion and state changes
    git.rs          Git smart-HTTP proxy to Artifacts
    checks.rs       authenticated commit check updates
  templates.rs      Askama page structs
templates/          Askama page templates
migrations/         D1 schema
scripts/            Workers Builds toolchain bootstrap
static/             design-system CSS, logo
tests/              isolated local HTTP and real Git regression scenarios
wrangler.jsonc      Worker bindings, scheduled maintenance and assets
```

## Local development

```sh
mise install          # Rust and Node versions pinned by the project
mise run setup        # WebAssembly target, IDE tools, worker-build and dependencies
mise run dev          # applies local D1 migrations and starts http://localhost:8787
```

`rust-toolchain.toml` selects the same Rust version for shells and IDEs. Setup
installs rust-analyzer, Rust sources, rustfmt and Clippy for that toolchain. With
an editor using rust-analyzer, open the repository root and set its Cargo target
to `wasm32-unknown-unknown` to match the Worker build.

Mise uses the Wrangler executable installed by `npm ci`. This keeps local
development and CI on the version pinned by `package-lock.json`.

Run `mise run check` to type-check the Worker, `mise run lint` to run Clippy,
and `mise run test` to build and run the HTTP regression suite. Tests start
their own local simulator, apply all migrations in a temporary directory, and
remove it afterward. They use isolated local data. `npm test` runs the same suite
against an existing build. Git must be on `PATH`, including in CI.

The Artifacts binding uses Wrangler's local simulator (git 2.32+ must be on
`PATH`). Local repositories and tokens are separate from deployed ones.
`cargo check --target wasm32-unknown-unknown` type-checks; `mise run build`
produces `build/index.js`.

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

Shard never stores objects itself. On clone/fetch/push it:

1. authenticates the git client (HTTP basic `username:token` or bearer token),
2. mints a short-lived Artifacts repo token (1 h, `read` for fetch, `write` for push),
3. streams the smart-HTTP request body and response through the Artifacts remote.

Create tokens under `/user/settings/tokens`, choosing read/write scope and a
7/30/90/365-day lifetime (30 days by default). In a private repo, owners and
collaborators with either role can clone using either token scope. Pushing
requires both a write-scoped token and repository write permission. Expired or
revoked tokens are rejected. Legacy tokens without an expiry remain valid until
revoked. The secret is shown only at creation. Use it as the password when Git
prompts. Anonymous clone works for public repositories.

Passwords use PBKDF2-SHA256. Browser sessions use HTTP-only cookies. Session and
token secrets are stored as hashes. Browser forms require session-bound CSRF
tokens and same-origin requests. Failed logins are limited to eight per
source/account pair within 15 minutes. Recovery pages send only the site origin
as referrer. Git smart HTTP returns an empty HTTP 404 for a missing `.git` repository.

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

For signed issue and review events, set `WEBHOOK_REPO` to the stable Artifacts
repository name, `WEBHOOK_ENDPOINT` to the receiver's HTTPS URL, and
`WEBHOOK_SECRET` as a Worker secret. Add `"* * * * *"` to `triggers.crons` alongside
the daily maintenance schedule. Verify `X-Shard-Signature` as `sha256=` followed
by the lowercase hex HMAC-SHA256 of the exact request body. Use `X-Shard-Event-Id`
to deduplicate deliveries from the Worker. Check `webhook_deliveries` for state,
attempt count and last HTTP status. The Worker attempts delivery up to six times.

## Backup and restore

Pause writes while taking a paired D1 and Git backup. Keep the SQL and each Git
mirror in private storage outside the workspace. Record the D1 `repos.artifacts_repo`
value for each mirror so a replacement namespace can preserve repository bindings.

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
D1 record. Production recovery includes restoring or provisioning the Artifacts
repositories under their expected names before serving the restored database.
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

## Design

The interface implements `DESIGN.md`: a white canvas with warm `#F5F5F0` surfaces,
amber `#F2C94C` primary actions, lavender `#E6D5F5` accents, maroon emphasis,
pill-shaped inputs/buttons, 28px cards, a 1200px measure and dotted focus rings.
Typography uses `Segoe UI` / `Segoe UI Variable Display`; Open AI Sans is
proprietary and is not bundled. There is no dark theme, matching the reference.
All UI lives in `static/css/shard.css`; templates hold no styling framework.

Account settings, token management, pagination and discussion history reuse the
same warm cards, pill controls, settings side navigation and dotted focus rings.
The CSP blocks scripts and cross-origin form submissions. Inline styles remain
allowed for the existing server-rendered templates.

## Prior art

- [ripgit](https://github.com/deathbyknowledge/ripgit) — one Durable Object per repo,
  SQLite objects, FTS5, and text/markdown negotiation for agents.
- [git-on-cloudflare](https://github.com/zllovesuki/git-on-cloudflare) — Smart HTTP
  v2 with Durable Object refs, R2 packs and a streaming push pipeline.
- [durable-git](https://github.com/littledivy/durable-git) — Git implemented inside a
  Durable Object, optional R2 pack cache, cgit-style surface.

Shard delegates object storage and pack negotiation to Artifacts, so it implements
only the smart-HTTP proxy plus forge metadata. The useful patterns borrowed here are
owner profile listings, safe raw-file serving, branch-scoped URLs and explicit
file-level change lists.

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
