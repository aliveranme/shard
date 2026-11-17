# shard — project status

Living record of what works, what is missing, and how it was verified.
Update this file whenever a milestone lands or a limitation changes.

**Snapshot: 2026-11-17** · `main` at the UI alignment commit · local dev only (never deployed)

| Item | Value |
|---|---|
| Runtime | Rust 1.100.0, `worker` 0.9.0, `worker-build` 0.9.0 |
| Tooling | Node 26.13.1, Wrangler 4.164.0, mise |
| Storage | D1 (`shard`) + Artifacts namespace `shard` |
| UI | `static/css/shard.css` implementing `DESIGN.md` |
| Deployment | not deployed; `database_id` is still a placeholder |
| Git remote | none configured (`origin` absent) |

## Working end to end (verified locally)

- Registration and login: PBKDF2-SHA256 (100k), HttpOnly session cookie, logout.
- Repository lifecycle: create calls the real Artifacts binding (local simulator), visibility, description, default branch, delete.
- Git smart HTTP proxy: anonymous clone of public repos, token clone, push with a write token; read tokens are rejected for push.
- Private repositories: anonymous page 404, anonymous clone 401, owner clone 200.
- Code browsing: tree, blob preview, raw download, commit log, branches containing `/` via `?branch=feature%2Freadme`.
- Issues: create, Markdown body, comment, close/reopen, open/closed/all filters, title search.
- Pull requests: create with branch validation, discussion, file-level change list from tree comparison, close/reopen.
- Markdown safety: raw HTML escaped, `javascript:`/`data:`/`vbscript:`/`file:` destinations replaced with `#`.
- Design system: measured 1200px measure, 96px hero, 999px pills, 28px cards, amber primary actions; no console errors.
- Forgejo-shaped layouts: repo two-column with About/Clone, single issue filter toolbar, header Close/Reopen, settings side nav, centered auth card.

## Progress

### ✅ Done

| Area | Notes |
|---|---|
| Auth | register, login, logout, session lookup; no email verification by design |
| Personal access tokens | create, list, revoke; `read`/`write` scopes; SHA-256 at rest |
| Repositories | create, visibility, description, default branch, delete, owner profile |
| Git transport | smart HTTP proxy with per-request 1h Artifacts tokens; push scope enforcement |
| Code browser | directory tree, blob preview, raw bytes, commit history |
| Issues | create, list/filter/search, comments, close/reopen |
| Pull requests | create, validate branches, file-level diff, comments, close/reopen |
| UI | DESIGN.md tokens, pill controls, responsive breakpoints (720/980px) |
| Local development | mise tasks, D1 migrations, Artifacts simulator (git ≥ 2.32) |
| Docs | README, this status file, verified Cloudflare API notes |

### 🟡 Partial

| Area | What exists | What is missing |
|---|---|---|
| Branch selection | default branch + branch named in URL | enumeration API absent; no branch list/switcher beyond that |
| Pull requests | file-level add/modify/delete list | line hunks, reviews, approvals, merge action |
| Repo home | README, latest commit, About | language stats, topics, releases, archive |
| Settings | general, storage, danger zone | rename, transfer, collaborators, webhooks |
| Tokens | scope, created, last used | expiry picker, token prefixes, audit trail |
| Search | repo search + issue title search | code search, commit search, full text |
| Errors | friendly 404/403/500 pages | structured error IDs, request correlation |

### ❌ Missing

**P0 — before calling it production-ready**

- Automated tests (unit + integration) and CI; verification so far is manual.
- Account settings: password change, email change, password reset, account deletion.
- Security hardening: CSRF tokens, CSP/security headers, rate limiting, login throttling.
- Pagination for repositories, issues, comments and commits (limits: 30/50/100 hard-coded).
- Expired session/token cleanup job; token expiry UI.
- `closed_by` / issue event timeline (closer is currently not stored; the UI stays neutral).
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
- Repository delete removes metadata first, then Artifacts; an Artifacts failure leaves an orphan repo.
- PR diff walks both full trees over RPC (bounded to 2000 entries / depth 12); large repos will be slow.
- Push bodies are buffered; multi-hundred-MB pushes risk Worker memory limits.
- Sessions and tokens have no cleanup job; expired rows accumulate.
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

## How to update

- Keep the snapshot table current (date, commit, deploy state).
- Move an item only when its behavior is verified, not when code exists.
- When a limitation changes (for example Artifacts adds a write/merge API), update both this file and the README API notes.
