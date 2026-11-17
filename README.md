# shard

A self-hosted, Cloudflare-native Git forge: repositories, issues and pull
requests with a Forgejo-derived UI, running on Workers + D1 + Artifacts.

- **Runtime**: Rust on Cloudflare Workers (`workers-rs`), built with `worker-build`.
- **Git storage**: one [Cloudflare Artifacts](https://developers.cloudflare.com/artifacts/) repository per project.
- **Metadata**: Cloudflare D1 (users, sessions, repos, issues, comments, tokens).
- **Frontend**: Forgejo's compiled CSS plus Askama templates that reproduce its page structure.
- **Toolchain**: [mise](https://mise.jdx.dev/) pins Rust, Node, Wrangler and `worker-build`.

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
templates/          Forgejo-shaped HTML
migrations/         D1 schema
static/             Forgejo CSS (vendored), logo
wrangler.jsonc      bindings: DB (D1), ARTIFACTS, ASSETS
```

## Local development

```sh
mise install          # rust 1.100, node 22, wrangler 4.164, worker-build 0.9
mise run setup        # wasm target + cargo/npm deps
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
- Forgejo-derived responsive UI, light/dark/auto themes from the vendored CSS.

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

## UI attribution

`static/css/index.css` and `theme.css` are compiled Forgejo assets (Forgejo is
GPL-3.0-or-later and bundles MIT-licensed Fomantic UI). They are vendored
unchanged; see `static/css/LICENSE`. Layout class names and page structure follow
Forgejo templates so the CSS renders as upstream intends.
