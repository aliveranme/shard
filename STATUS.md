# shard — project status

Living record of what works, what is missing, and how it was verified.
Update this file whenever a milestone lands or a limitation changes.

**Snapshot: 2026-11-18** · account/security/pagination increment · local development only

| Item | Value |
|---|---|
| Runtime | Rust 1.100.0, `worker` 0.9.0, `worker-build` 0.9.0 |
| Tooling | Node 26.13.1, Wrangler 4.164.0, mise |
| Storage | D1 (`shard`) + Artifacts namespace `shard` |
| UI | `static/css/shard.css` implementing `DESIGN.md` |
| Deployment | not deployed; `database_id` is still a placeholder |
| Git remote | none configured (`origin` absent) |

## Working end to end (verified locally)

- Registration and login: PBKDF2-SHA256 (100k), HttpOnly session cookie, logout, failed-login throttling.
- Account settings: password-confirmed email/password updates; password rotation invalidates older sessions.
- Browser protection: CSRF tokens, origin checks, CSP/security headers, safe post-login redirects.
- Access tokens: strict scope/name/expiry validation, one-time secret display, expiration enforcement, legacy compatibility and revocation.
- Repository lifecycle: create calls the real Artifacts binding (local simulator), visibility, description, default branch, delete.
- Git smart HTTP proxy: anonymous clone of public repos, token clone, push with a write token; read tokens are rejected for push.
- Private repositories: anonymous page 404, anonymous clone 401, owner clone 200.
- Code browsing: tree, blob preview, raw download, cursor-paginated first-parent history, branches containing `/` via `?branch=feature%2Freadme`.
- Issues: create, Markdown body, comments, close/reopen, open/closed/all filters, title search, paginated actor-attributed discussion history.
- Pull requests: branch-validated creation, discussion, file-level changes, close/reopen and state history.
- Markdown safety: raw HTML escaped, `javascript:`/`data:`/`vbscript:`/`file:` destinations replaced with `#`.
- Design system: measured 1200px measure, 96px hero, 999px pills, 28px cards, amber primary actions; no console errors.
- Forgejo-shaped layouts: repo two-column with About/Clone, single issue filter toolbar, header Close/Reopen, settings side nav, centered auth card.
- Pagination: 20-item repository/issue/discussion pages, stable ordering and preserved query filters.
- Scheduled cleanup: expired session/token rows are removed by the configured daily event.
- Repository deletion: incorrect confirmation preserves the repo; confirmed deletion removes storage and metadata.

## Progress

### ✅ Done

| Area | Notes |
|---|---|
| Auth | register, login, logout, session lookup, eight-failure/15-minute source/account throttle |
| Account | email/password settings, current-password checks, password-change session rotation |
| Personal access tokens | one-time `shard_` secrets, read/write scopes, expiry picker, list/revoke, SHA-256 at rest |
| Browser security | centralized CSRF/origin checks, CSP/security headers, safe local redirects |
| Repositories | create, visibility, description, default branch, delete, owner profile |
| Git transport | smart HTTP proxy with per-request 1h Artifacts tokens; push scope enforcement |
| Code browser | directory tree, blob preview, raw bytes, bounded cursor history |
| Issues | create, list/filter/search, comments, atomic close/reopen events with actor attribution |
| Pull requests | create, validate branches, file-level diff, comments, close/reopen |
| UI | DESIGN.md tokens, pill controls, responsive breakpoints (720/980px) |
| Pagination | dashboard, explore, profiles, global/repository issue lists, combined discussion and commit history |
| Cleanup | daily scheduled removal of expired sessions, tokens and throttle windows |
| Regression tests | 14 isolated HTTP scenarios; real Git push and slash-branch PR creation included |
| CI | GitHub Actions workflow added; remote workflow run has not been observed |
| Agent guidance | AGENT.md covers architecture, design, migrations, security and verification |
| Local development | mise tasks, D1 migrations, Artifacts simulator (git ≥ 2.32) |
| Docs | README, this status file, verified Cloudflare API notes |

### 🟡 Partial

| Area | What exists | What is missing |
|---|---|---|
| Branch selection | default branch + branch named in URL | enumeration API absent; no branch list/switcher beyond that |
| Pull requests | file-level add/modify/delete list | line hunks, reviews, approvals, merge action |
| Repo home | README, latest commit, About | language stats, topics, releases, archive |
| Settings | general, storage, danger zone | rename, transfer, collaborators, webhooks |
| Tokens | scope, prefix, expiry, created/last used, revocation | audit trail, token-list pagination |
| Search | repo search + issue title search | code search, commit search, full text |
| Errors | friendly 404/403/500 pages | structured error IDs, request correlation |

### ❌ Missing

**P0 — before calling it production-ready**

- Account recovery: email delivery/verification, password reset and account deletion.
- Global/edge rate limiting beyond the implemented source/account failed-login throttle.
- Remove remaining template inline styles before tightening CSP `style-src` further.
- Real deployment checklist: D1 `database_id`, remote migrations, custom domain, backups.
- Repository rename (Artifacts internal name is stable; UI surface needs mapping rules).

**P1 — forge parity**

- Labels, milestones, assignees, reactions, issue templates.
- PR line diffs, per-line review comments, approvals, merge queue/merge commit tracking.
- Notifications, activity feed, watch/star/fork.
- Repository import and fork UI (binding supports `import`, `fork`).
- Tags/branches management, archive/export (`git archive`, zip/tar).
- Organizations, teams, collaborators; current permissions are owner-or-admin only.

**P2 — scale and polish**

- Streaming Git push (currently the whole pack body is buffered in memory).
- Git LFS, shallow/partial clone verification, protocol v2 receive-pack (Artifacts limitation).
- Caching for hot UI paths and repeated Artifacts reads.
- Observability: structured logs, tracing IDs, Artifacts metrics.
- Dark theme (not defined in DESIGN.md; light-only today).

## Cloudflare Artifacts constraints (verified 2026-11-17)

- Workers binding: namespace `create/get/list/import/delete`; repo `info/createToken/listTokens/revokeToken/fork/log/readCommit/readTree/readBlob/readFile`.
- No branch enumeration, no merge, no write API. PR merge therefore remains a Git operation.
- Commit metadata uses `treeHash`, `authoredAt`, `committedAt`; tree entries are `{ name, mode, type, hash }`.
- Git tokens: Bearer, or Basic with the token secret before `?expires=`; write scope required for `git-receive-pack` including its `info/refs` advertisement.
- Artifacts requires Workers Paid; local simulator requires Wrangler ≥ 4.163 and Git ≥ 2.32.

## Technical debt / risks

- `Router` is rebuilt per request; routes allocate strings. Move to a cached router with request-scoped state.
- Repository create is not transactional across Artifacts + D1: Artifacts repo is deleted best-effort if the D1 insert fails.
- Repository deletion spans Artifacts and D1; if D1 fails after storage deletion succeeds, metadata can remain for a deleted repository.
- PR diff walks both full trees over RPC (bounded to 2000 entries / depth 12); large repos will be slow.
- Push bodies are buffered; multi-hundred-MB pushes risk Worker memory limits.
- Token lists remain unpaginated; accounts with many tokens can produce large settings pages.
- Error responses hide internals (good) but emit no correlation ID, making support harder.
- Claims of "public read" rely on the proxy; the Artifacts remote URL/token never reaches clients.

## Verification log

| Date | Check | Result |
|---|---|---|
| 2026-11-17 | `mise run check` (wasm32) | pass, zero warnings |
| 2026-11-17 | `mise run build` | pass, 29.2 KB entry |
| 2026-11-17 | register/login/logout in browser | pass |
| 2026-11-17 | repo create via Artifacts simulator | pass |
| 2026-11-17 | `git push` / anonymous `git clone` | pass |
| 2026-11-17 | read-token push | rejected 401 (expected) |
| 2026-11-17 | private repo visibility | 404 / 401 / 200 as expected |
| 2026-11-17 | issue create/comment/close | pass |
| 2026-11-17 | PR create + file-level diff | pass |
| 2026-11-17 | Markdown XSS probe | HTML escaped, script URLs neutralized |
| 2026-11-17 | browser console on redesigned pages | no errors |
| 2026-11-18 | `mise run check` | pass, zero warnings |
| 2026-11-18 | `mise run test` (includes release build) | pass: 14 scenarios / 15 Node test results; zero build warnings |
| 2026-11-18 | credential changes, stale sessions, CSRF/origin probes, expiry/revocation/legacy tokens | pass in isolated HTTP suite |
| 2026-11-18 | real Git push, slash branches, bounded commit cursor and PR file changes | pass in isolated HTTP suite |
| 2026-11-18 | repository/issue/discussion pagination, wrong-kind mutations, repeated close/reopen | pass in isolated HTTP suite |
| 2026-11-18 | daily cleanup event | expired session and token rows removed |
| 2026-11-18 | Chrome screenshots at 1365px and 390px | account, tokens and discussion pages fit without horizontal overflow |
| 2026-11-18 | Chrome account/token forms and actual pagination click | email success, token reveal/reload, attributed history and navigation pass |
| 2026-11-18 | repository deletion in isolated browser runtime | wrong confirmation retained repo; confirmed deletion returned 404 |
| 2026-11-18 | `mise run db:migrate` on local development state | migrations 0002 and 0003 applied successfully |

## How to update

- Keep the snapshot table current (date, commit, deploy state).
- Move an item only when its behavior is verified, not when code exists.
- When a limitation changes (for example Artifacts adds a write/merge API), update both this file and the README API notes.
