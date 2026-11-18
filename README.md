# shard

A self-hosted, Cloudflare-native Git forge: repositories, issues and pull
requests with a warm, conversation-first UI, running on Workers + D1 + Artifacts.
See [STATUS.md](STATUS.md) for progress, verified behavior and missing pieces.
The [delivery plan](PLAN.md) records Cloudflare dependencies and acceptance criteria.
Contributor and coding-agent conventions live in [AGENT.md](AGENT.md).

- **Runtime**: Rust on Cloudflare Workers (`workers-rs`), built with `worker-build`.
- **Git storage**: one [Cloudflare Artifacts](https://developers.cloudflare.com/artifacts/) repository per project.
- **Metadata**: Cloudflare D1 (users, sessions, repos, issues, comments, tokens).
- **Frontend**: a small CSS design system (amber/lavender accents, pill controls, 28px cards) applied to
  Askama templates; the visual language follows `DESIGN.md`.
- **Toolchain**: Rust 1.100, Node 26.13, Wrangler 4.164 and `worker-build` 0.9, pinned with
  [mise](https://mise.jdx.dev/).

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
static/             design-system CSS, logo
tests/              isolated local HTTP and real Git regression scenarios
wrangler.jsonc      bindings: DB (D1), ARTIFACTS, ASSETS
```

## Local development

```sh
mise install          # Rust and Node versions pinned by the project
mise run setup        # WebAssembly target, IDE tools, worker-build and dependencies
mise run db:migrate   # apply migrations to the local D1
mise run dev          # wrangler dev on http://localhost:8787
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

Register an account, create a repository, then push:

```sh
git clone http://localhost:8787/<user>/<repo>.git
```

## Deploy

Artifacts requires the **Workers Paid** plan.

```sh
node node_modules/wrangler/bin/wrangler.js d1 create shard # copy database_id into wrangler.jsonc
mise run deploy                                         # apply remote migrations, build and deploy
```

Optional custom domain via `routes` in `wrangler.jsonc`.

Set `PUBLIC_ORIGIN` to the public HTTPS origin and `EMAIL_FROM` to an address on
an onboarded sending domain. Bind `EMAIL` using Wrangler's `send_email` binding.
This enables links to verify addresses and reset passwords. Verification links
expire after 24 hours and reset links after 30 minutes. A reset invalidates all
browser sessions for that account. [Cloudflare Email Service](https://developers.cloudflare.com/email-service/get-started/send-emails/)

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

## Interface language

Open **Language** in the footer or **Account settings → Language**. Choose
English, 简体中文 (Simplified Chinese), or **Use browser language**.
Signed-in choices are stored in D1 and apply across sessions. If an account has
no saved preference, Shard uses the browser's language cookie, then its
`Accept-Language` priorities. Unsupported languages fall back to English.
The **Use browser language** setting explicitly selects the browser header.

The Worker translates Web UI labels, validation messages, errors, accessibility
labels, relative dates and counts. Repository contents, issues and commit
messages appear in the language their authors used. Git smart HTTP returns an
empty HTTP 404 when the `.git` repository does not exist.

To add a locale, register its tag and display label in `src/i18n/mod.rs`, add
its text to each catalog in `src/i18n/`, and add its formatting and negotiation
rules. Extend the D1 locale constraint with a forward migration. Keep
interpolated messages together so translators can change word order. Templates
escape the output, including interpolated values.

## Implemented

- Registration/login with PBKDF2-SHA256 passwords and HTTP-only session cookies;
  failed logins are limited to eight per source/account pair in a 15-minute window.
- Account settings support password-confirmed email and password changes, email
  verification, password reset, and account deletion. Deletion requires transfer
  or removal of owned repositories and revokes sessions and access tokens.
- Repository settings support visibility, description, web default branch,
  rename, transfer with recipient acceptance, and deletion. Changes to the web
  default require an existing Git branch. Clones follow the Artifacts remote
  `HEAD`. Previous paths retain current permissions and keep serving Git remotes.
- Collaborators receive read or write access. Forks copy a visible repository.
  Forks from a private source remain private. Imports accept public HTTPS Git URLs.
  The copies page shows status until the repository is available.
- Code browsing includes trees, blobs, raw downloads, commit history with parent
  links, branch and tag discovery, and commit-pinned source archives.
- Issues support editing, labels, a milestone, an assignee, Markdown discussion,
  state history, and search across titles, bodies and comments. The inbox surfaces
  new issue, discussion and review activity based on current access.
- Pull requests save both branch tips and their best common ancestor at creation.
  File and line diffs compare the common ancestor to the source tip, excluding
  target-only changes. Pushes leave the saved comparison in place. The author
  or a repository writer can choose **Update comparison** on an open request.
  Decisions and line comments link to the exact comparison they reviewed.
  Pre-existing comparisons retain their two-tip diff until updated.
  After a local merge and push, a writer can verify that both saved branch tips
  are ancestors of the current target tip.
- Personal access tokens have read/write scopes, expiry, last usage, and revocation.
  Session and access-token secrets are stored as hashes.
- Browser forms use session-bound CSRF tokens and same-origin checks. Recovery
  pages send only the site origin as referrer so email link paths stay private.
- The Worker echoes an `X-Request-ID` on responses for log correlation and applies
  per-source limits separately to browser, Git, and check update traffic.
  Daily maintenance at 03:00 UTC removes expired credential and throttle rows.
- Responsive, light-only UI using the DESIGN.md tokens: pill inputs and buttons,
  rounded cards, amber primary actions, lavender accents and warm neutral surfaces.

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
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 export shard --local --persist-to "$SHARD_SOURCE_D1" --output "$SHARD_BACKUP/shard.sql"
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 execute shard --local --persist-to "$SHARD_RESTORE_D1" --file "$SHARD_BACKUP/shard.sql"
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
