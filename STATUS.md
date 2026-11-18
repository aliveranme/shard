# shard — project status

Living record of what works, what is missing, and how it was verified.
Update this file whenever a milestone lands or a limitation changes.

**Snapshot:** 2026-11-18 · page transitions, control motion, and responsive alignment · local development

| Item | Value |
|---|---|
| Runtime | Rust 1.100.0, `worker` 0.9.0, `worker-build` 0.9.0 |
| Tooling | Node 26.13.1, Wrangler 4.164.0, mise |
| Storage | D1 with Cloudflare Artifacts by default or an authenticated native Git origin. Both environments use a private R2 bucket for LFS objects. |
| UI | Server-rendered Askama templates using `static/css/theme.css` and `static/css/shard.css` |
| Deployment | Deploy to Cloudflare and Wrangler scripts are configured. Production verification is pending. |
| Source | [aliveranme/shard](https://github.com/aliveranme/shard) |

## Verified locally

| Area | Behavior |
|---|---|
| Accounts | Registration, login throttling, password-confirmed settings changes, account deletion with an ownership guard, and single-use verification and reset links through the email simulator. Password reset revokes all browser sessions. |
| Avatars | Account settings accept PNG, JPEG and WebP and store one metadata-free PNG thumbnail per user in D1. Uploaded avatars appear in profiles, discussions, branch summaries, commit history and commit details. Git author emails match account emails and fall back to initials. Removal and account deletion remove the stored image. |
| Browser security | Session-bound CSRF, same-origin checks, CSP, safe redirects, and origin-only referrers on recovery pages. |
| Internationalization | Typed English and Simplified Chinese catalogs. Guest language follows the browser cookie or header. Account settings persist across sessions. |
| Access tokens | Read/write scopes, expiry and revocation, one-time secret display, last usage, and legacy credential compatibility. |
| Repository access | Owners grant read or write access. Private visibility is enforced for Web, Git, search, downloads, and notification reads. Revocation removes access on subsequent requests. |
| Repository lifecycle | Rename and accepted transfers preserve existing Web and Git routes. Forks preserve Git refs. The import form rejects sources outside its public HTTPS URL policy. |
| Web default branch | Settings accept an existing Git branch. Clones follow the Git backend's `HEAD`. |
| Git transport | Smart HTTP request and response bodies are streamed. Private clone requires a credential, and push also requires write scope and repository write access. |
| Git LFS | Both Git backends transfer objects through the authenticated Batch and basic transfer API. R2 verifies the SHA-256 and size before serving a repository-scoped object. |
| Native Git origin | The `native` Wrangler environment passes Git push, clone, fetch, branch and tag browsing, raw downloads, pull request line diffs, and access revocation. Forked refs and files stay readable after source deletion. |
| Code browsing | Branch and tag inventory, branch-scoped trees and files, cursor-paginated first-parent history, parent links, and commit-pinned source TAR downloads. Text previews read at most 256 KiB. README links and images use the displayed commit, and raw downloads stream full files. |
| Pull requests | Common-ancestor file and line diffs. Authors and writers can explicitly update open comparisons. Review links retain the saved diff and line. Writers can verify integration after pushing a merge. |
| Issues and search | Edit, label, assign, milestone, close/reopen, and search across titles, bodies, and comments. Paginated discussions preserve actor attribution. |
| Markdown | READMEs, Markdown files and discussions render repository-relative and HTTPS images, including filtered HTML image tags. Git media inherits repository permissions. Issue and pull numbers are linked with `#12`, `GH-12` and `owner/repo#12` syntax. |
| Notifications and integrations | Permission-aware inbox, one current CI result per commit and context, and signed webhook retries using stable event IDs. |
| Maintenance | Per-source limits, `X-Request-ID` correlation, and daily removal of expired sessions, tokens, throttle windows, and email links. |
| Backups | Rehearsed D1 export and restore into isolated persistence and Git bundle restore with matching refs and object IDs. |
| UI and development | The shared theme follows system appearance. Geist and Shard Han set proportional UI text with tabular figures. JetBrains Mono sets code and Git identifiers, with two-cell Han glyphs in code. Buttons and form controls use calibrated font metrics. The refs grid marks default branches and wraps full annotated-tag object IDs on narrow screens. |
| Select controls | Themed pickers use native form and keyboard behavior. Browsers without `base-select` use their system picker. |
| Navigation and motion | Narrow repository tabs wrap and list metadata aligns with its title. Supporting browsers animate same-origin navigation, mobile menus and themed pickers. Reduced motion keeps keyboard focus and diff target markers visible. |

## Remaining work and prerequisites

| Area | Requirement |
|---|---|
| Deployment | Artifacts staging needs Workers Paid and an Artifacts namespace. Native staging needs an HTTPS Git origin and a persistent volume. Hosted builds, production Git operations, public imports, scheduled delivery and restore still need staging verification. |
| Email | An onboarded sending domain, an `EMAIL` binding, and an available mailbox to verify real delivery. |
| Merge automation | Web merges, protected refs and merge queues need a component that can update a validated target ref atomically. Current merge verification relies on reachable commit IDs. |
| Repository search | Code and commit search require a bounded permission-aware index. A push event subscription can drive ingestion, with staging needed to exercise push delivery. |
| Forge workflows | Organizations, teams, releases, issue templates, reactions, and watch/star controls. |
| Git protocol | Verify shallow and partial clones on Artifacts. Shard's Git HTTPS remote does not expose an LFS Batch API. Verify production Artifacts separately. |
| Operations | Public deployment metrics, webhook retention, token-list pagination, and further CSP tightening after moving inline styles. |

Architecture decisions and provider evidence are recorded in [PLAN.md](PLAN.md).

## Cloudflare Artifacts constraints (verified 2026-11-17)

- Workers binding: namespace `create/get/list/import/delete`; repo `info/createToken/listTokens/revokeToken/fork/log/readCommit/readTree/readBlob/readFile`.
- Object and history reads use the binding. Shard performs ref discovery through Git HTTPS and records merges verified against Git ancestry.
- Commit metadata uses `treeHash`, `authoredAt`, `committedAt`; tree entries are `{ name, mode, type, hash }`.
- Git tokens: Bearer, or Basic with the token secret before `?expires=`; write scope required for `git-receive-pack` including its `info/refs` advertisement.
- Artifacts requires Workers Paid; local simulator requires Wrangler ≥ 4.163 and Git ≥ 2.32.

## Technical debt / risks

- `Router` is rebuilt per request; routes allocate strings. Move to a cached router with request-scoped state.
- Repository creation and deletion span D1 and Git storage. Creation attempts to remove storage after a failed metadata insert. Deletion can leave metadata if D1 fails after storage has been removed.
- PR comparisons bound ancestry reads, tree depth, entry count and tree reads. Historical diffs require the original Git objects to remain available.
- Token lists remain unpaginated; accounts with many tokens can produce large settings pages.
- Request IDs link client responses to handler errors. Production latency, Git transfer size and throttling metrics still need live measurements.
- The Worker mediates repository access. Git clients receive neither the Artifacts repository token nor the native origin's shared secret.

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
| 2026-11-18 | `mise run setup`, rust-analyzer references and `mise run lint` | pinned components installed, symbol queries work, Clippy reports no warnings |
| 2026-11-18 | `mise run db:migrate` with Wrangler 4.164.0 | migration 0004 applied to local D1 |
| 2026-11-18 | `mise run dev` | local Worker returns HTTP 200 with a Chinese language settings page |
| 2026-11-18 | Wasm check and `mise run test` | 16 scenarios, 17 Node test results passed |
| 2026-11-18 | Nested file preview and raw download regression | relative links resolve directory spaces and literal percent sequences on a slash-named branch |
| 2026-11-18 | Ego Browser at 1365px, 390px and 320px | verified language switches, settings navigation, branch form, file links and issue/PR layouts without horizontal overflow |
| 2026-11-18 | Wasm release build and focused Git HTTP regression | missing Git repository returns an empty, non-cacheable 404, and the missing `.git` suffix page is localized |
| 2026-11-18 | `mise run build` | Wasm release built successfully |
| 2026-11-18 | Chromium at 320px, 390px, and 1365px | Sign-in, recovery request, and saved Simplified Chinese preference worked. Six views had one main landmark, one H1, and no page overflow. Long diff lines scrolled within the diff. |
| 2026-11-18 | Local D1 and Git restore | Imported D1 users, routes, PRs, and notification counts matched the source. The restored Git mirror passed `git fsck --full` with identical refs and object IDs. |
| 2026-11-18 | `mise exec -- node --test tests/http.test.mjs` | 22 isolated HTTP and Git scenarios passed, with 23 passing Node test results. The suite covered collaborator revocation, immutable PR comparisons, verified integration, CI result updates, email link replay, and signed webhook retries. |
| 2026-11-18 | HTTP and Git regression, Chromium at 390px and 1365px | Rejected missing branches, tags and revision expressions. Saved a slash-named web default and confirmed the remote `HEAD` was unchanged. |
| 2026-11-18 | Wasm release build and focused HTTP/Git regression | Excluded target-only changes, rejected ambiguous ancestry and stale review forms, allowed one concurrent update, preserved historical line links, and verified an actual Git merge. |
| 2026-11-18 | Migration 0013 and Ego Browser, desktop and 390px | Updated an existing PR from version 1 to 2 in Simplified Chinese, followed a historical comment to its original line, and verified that long lines scroll within the diff. |
| 2026-11-18 | Clean Linux Workers Builds rehearsal and Wrangler dry run | Installed the pinned Rust/Wasm toolchain, built the bundle, resolved all four bindings, and applied all 13 migrations to an isolated D1 database with a different name. |
| 2026-11-18 | Published GitHub README badge | Cloudflare opened the Shard configuration with a new D1 database option and prefilled `npm run build` and `npm run deploy`. |
| 2026-11-18 | Git LFS 3.7.0 over loopback | Shard returned HTTP 400 for upload and download Batch requests. A regular push left the ref absent. After a pointer-only push, regular checkout failed during smudge and a skip-smudge clone retrieved the pointer. The local Artifacts remote returned HTTP 404 for Batch. |
| 2026-11-18 | Wasm check and isolated HTTP/Git suite | 23 tests passed, including a bounded large-file preview, full raw bytes, binary preview and slash-named branch browsing. |
| 2026-11-18 | Prebuilt native Worker dry run and workflow lint | The archive includes the origin script. Wrangler resolved native D1, rate limiting and static assets without an Artifacts binding. The default Artifacts regression suite still passed all 23 tests. |
| 2026-11-18 | Independent R2 Git engine rehearsal | `durable-git` stored an 18 MiB pack in local R2. A clone after restart matched the commit ID, annotated tag and file contents. |
| 2026-11-18 | Isolated native Git and Chromium settings | The native suite passed all 3 reported tests. The origin required its own credential, rejected an unlisted import host and preserved repository permissions through rename and revocation. English and Simplified Chinese storage settings fit 1365px and 390px widths. |
| 2026-11-18 | `mise run build` and Chromium at 1365px, 820px, 390px and 320px | Verified repository search, branch selection, file preview, a submitted line comment and its historical link, the keyboard-operated mobile menu, saved Chinese language preferences and reduced motion. Confirmed a palette override recolors the page logo. |
| 2026-11-18 | `mise run build`, B-end template audit, and Chromium at 1365px, 1089px, 820px, 390px and 320px | Verified system appearance and reduced motion, the 40-unit dark weight offset, local font rendering, responsive refs and settings, empty states, an annotated tag, a long slash branch and keyboard navigation. Narrow home, code, file, review and login pages fit, with long diffs scrolling internally. |
| 2026-11-18 | `mise run build`, FontTools, B-end template audit and Chromium at 1365px and 320px | Verified proportional UI text, distinct code and path fonts, equal advances for digits 0–9, and the 40-unit dark weight offset. Home, language settings, repository, file and refs pages fit the tested widths. In 3× captures, the visible text of 12 English and Simplified Chinese buttons, fields, tabs and badges centered within 1.1 CSS px of their controls. |
| 2026-11-18 | `mise run build`, both local Git backends, B-end template audit, and Chromium at 1365px and 390px | All 27 HTTP and Git tests passed. Verified avatar replacement, removal and account deletion, commit-pinned image links, and private media access after a grant and revocation. Chromium verified upload, branch picker selection and Escape, HTML image rendering, and a reference to a pull request. The external HTTPS image used an isolated browser response. |
| 2026-11-18 | `mise run build`, `mise exec -- npm test`, B-end template audit, and Chromium at 1365px, 820px, 390px and 320px | All 27 HTTP and Git tests passed, including native Git SVG media with sandbox CSP. Chromium verified the 200ms page transition, keyboard menu and picker opening and closing, English language save, and reduced-motion behavior. Tested pages fit the viewport, with repository tabs visible and list badges and hashes aligned. |

## How to update

- Keep the snapshot table current (date, commit, deploy state).
- Move an item only when its behavior is verified, not when code exists.
- When a limitation changes (for example Artifacts adds a write/merge API), update both this file and the README API notes.
