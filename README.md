# shard

A self-hosted, Cloudflare-native Git forge: repositories, issues and pull
requests with a warm, conversation-first UI, running on Workers + D1 + Artifacts.

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
  artifacts.rs      Rust wrapper over the Artifacts Workers binding
  handlers/
    web.rs          dashboard, explore, global issue lists
    auth_handlers.rs registration, login, access tokens
    repos.rs        create/browse/settings, trees, blobs, commits
    issues.rs       issues, comments, pull requests + file-level diff
    git.rs          Git smart-HTTP proxy to Artifacts
  templates.rs      Askama page structs
templates/          Askama page templates
migrations/         D1 schema
static/             design-system CSS, logo
wrangler.jsonc      bindings: DB (D1), ARTIFACTS, ASSETS
```

## Local development

```sh
mise install          # rust 1.100, node 26.13, wrangler 4.164
mise run setup        # wasm target, worker-build 0.9, cargo/npm deps
mise run db:migrate   # apply migrations to the local D1
mise run dev          # wrangler dev on http://localhost:8787
```

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
wrangler d1 create shard            # copy database_id into wrangler.jsonc
wrangler d1 migrations apply shard --remote
mise run deploy                     # worker-build --release && wrangler deploy
```

Optional custom domain via `routes` in `wrangler.jsonc`.

## URL shape

Branch names contain slashes, so branches are a query parameter:

```
/sh/shard/{owner}/{repo}
/{owner}/{repo}/src/{file-path}?branch=feature%2Freadme
/{owner}/{repo}/blob/{file-path}?branch=main
/{owner}/{repo}/raw/{file-path}?branch=main
/{owner}/{repo}/commits?branch=main
```

## Git access

Shard never stores objects itself. On clone/fetch/push it:

1. authenticates the git client (HTTP basic `username:token` or bearer token),
2. mints a short-lived Artifacts repo token (1 h, `read` for fetch, `write` for push),
3. proxies the smart-HTTP request to the Artifacts remote and streams the response.

Create tokens under `/user/settings/tokens`. Write tokens can push to your own
repositories; read tokens can clone public repositories (private ones require
ownership). Anonymous clone works for public repositories.

## Implemented

- Registration/login with PBKDF2-SHA256 passwords, HTTP-only session cookies.
- Repositories: create (Artifacts-backed), visibility, description, default branch, delete.
- Code browsing: tree, blob preview, raw download, commit log (Artifacts metadata).
- Issues: create, comment (Markdown), close/reopen, search, open/closed/all.
- Pull requests: create from existing branches, discussion, file-level change list,
  close/reopen. Merging is a Git operation by design (Artifacts exposes no write API
  to Workers), so the page states this instead of pretending to merge.
- Personal access tokens and encrypted-at-rest session/token hashes.
- Responsive, light-only UI using the DESIGN.md tokens: pill inputs and buttons,
  rounded cards, amber primary actions, lavender accents and warm neutral surfaces.

## Limits and deliberate choices

- **Merge**: no server-side merge; merge locally and push the base branch.
- **Branches**: the binding exposes `log(ref)`, not branch enumeration, so the
  selector lists the default branch and the branch named in the URL.
- **Diff**: Artifacts has no diff endpoint; shard walks both trees and reports
  added/modified/deleted paths, not line hunks.
- **Push size**: request bodies are buffered before proxying; enormous pushes can
  exceed Worker memory. Stream the body if that becomes a real workload.
- **Email**: registration stores an address but shard sends no mail; accounts are
  active immediately. Put Cloudflare Access in front if this is a private forge.

## Design

The interface implements `DESIGN.md`: a white canvas with warm `#F5F5F0` surfaces,
amber `#F2C94C` primary actions, lavender `#E6D5F5` accents, maroon emphasis,
pill-shaped inputs/buttons, 28px cards, a 1200px measure and dotted focus rings.
Typography uses `Segoe UI` / `Segoe UI Variable Display`; Open AI Sans is
proprietary and is not bundled. There is no dark theme, matching the reference.
All UI lives in `static/css/shard.css`; templates hold no styling framework.

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
  There is **no** branch enumeration, merge or write API; the PR design reflects that.
- Commit metadata uses `treeHash` and `authoredAt`/`committedAt` (Unix seconds);
  tree entries are `{ name, mode, type, hash }`.
- Git remotes accept a Bearer repo token; for Basic auth the password is the token
  secret before its `?expires=` suffix.
