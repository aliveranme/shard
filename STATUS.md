# shard — project status

Living record of what works, what is missing, and how it was verified.
Update this file whenever a milestone lands or a limitation changes.

**Snapshot:** 2026-11-19 · Shard 0.2.0 · local verification

| Item | Value |
|---|---|
| Runtime | Rust 1.100.0, `worker` 0.9.0, `worker-build` 0.9.0 |
| Tooling | Node 26.13.1, Wrangler 4.164.0, mise |
| Storage | D1 with SQLite Durable Objects by default, Cloudflare Artifacts in Paid environments, or an authenticated native Git origin. LFS is available with a private R2 binding. |
| UI | Server-rendered Askama templates using `static/css/theme.css` and `static/css/shard.css` |
| Deployment | The Free badge uses the default deploy command. `--env artifacts` targets the original Artifacts Worker, and `deploy:paid` configures managed R2 multipart. Managed CD uses independent staging and production resources and verifies each deploy with a Workflow. |
| Source | [aliveranme/shard](https://github.com/aliveranme/shard) |

## Verified locally

| Area | Behavior |
|---|---|
| Accounts | Registration, login throttling, password-confirmed settings changes, account deletion with an ownership guard, and single-use verification and reset links through the email simulator. Password reset revokes all browser sessions. |
| Avatars | Account settings accept PNG, JPEG and WebP and store one metadata-free PNG thumbnail per user in D1. Uploaded avatars appear in profiles, discussions, branch summaries, commit history and commit details. Git author emails match account emails and fall back to initials. Removal and account deletion remove the stored image. |
| Browser security | Session-bound CSRF, same-origin checks, CSP, safe redirects, and origin-only referrers on recovery pages. |
| Internationalization | Typed English and Simplified Chinese catalogs. Guest language follows the browser cookie or header. Account settings persist across sessions. |
| Access tokens | Read/write scopes, expiry and revocation, one-time secret display, and legacy credential compatibility. Git requests update the last-use timestamp at most once per minute per token. |
| Repository access | Owners and organization teams grant read or write access. Web, Git, search, downloads and notification reads enforce current grants. Revocation removes access on subsequent requests. |
| Repository lifecycle | Rename and accepted transfers preserve Web and Git routes. Forks preserve Git refs. The native origin imports private sources over Basic or Bearer HTTPS and publishes them privately. The minute cron reconciles pending copies. Portable metadata import applies each bundle atomically. |
| Default branch and refs | Settings update the branch selected by new clones through Shard. Owners can create branches and lightweight tags from commit IDs, and delete individual refs after confirming their names. |
| Git transport | The Worker forwards Smart HTTP bodies as streams. The Free Git engine buffers packs within its configured compressed size limit. Private clone requires a credential, and push requires write scope and repository write access. |
| Default Free Git | On fresh local state, real Git pushes, private repository access, raw reads, ref and HEAD changes, and Web fast-forward merges work with D1 and a SQLite Durable Object per repository. A fork retained its refs and cloned after source deletion and Worker restart. |
| Git LFS | All Git backends use repository and token permissions. The Free config leaves R2 optional. With R2 bound, proxy uploads default to 100 MB. Standard Git LFS can use a presigned R2 PUT. The packaged multipart adapter resumes from recorded parts and verifies the whole object before publication. Forks use independent R2 keys. |
| Native Git origin | The native environment supports Git push, clone, fetch, ref browsing, raw downloads, pull request diffs and access revocation. Forked refs and files stay readable after source deletion. Same-origin PRs use commit-pinned merge-base, file comparison and ancestry requests. Authenticated origin controls set HEAD, update refs against an expected tip, merge reviewed commits and import private HTTPS repositories. |
| Code browsing | Branch and tag inventory, branch-scoped trees and files, cursor-paginated first-parent history, parent links, and commit-pinned source TAR downloads. Text previews read at most 256 KiB. README links and images use the displayed commit, and raw downloads stream full files. |
| Pull requests | Common-ancestor diffs support source and target branches in distinct repositories. Accepted branch pushes schedule refreshes. Reviews retain the saved comparison and line. Protected branches require the configured approvals and checks. The queue advanced an Artifacts branch by fast-forward and created a two-parent merge from a fork on the native origin. |
| Issues and search | Issue lists filter by state, label, assignee, milestone and discussion text. Repository search finds paths, text and commit messages on a saved snapshot and reports scan limits. |
| Markdown | READMEs, Markdown files and discussions render repository-relative and HTTPS images, including filtered HTML image tags. Git media inherits repository permissions. Issue and pull numbers are linked with `#12`, `GH-12` and `owner/repo#12` syntax. |
| Forge workflows | Writers draft and publish commit-pinned releases from Git tags. Owners manage issue templates. Users can add issue reactions, watch repositories and list their stars. |
| Notifications and integrations | The inbox enforces repository access. A CI context has one current result per commit, shown on commit details and reviewed pull requests. Owners manage signed webhooks with stable event IDs, delivery IDs, bodies and signatures across retries. Push observation records ref changes, PR refresh and Webhook delivery in one D1 transaction. The minute cron checks live refs for unfinished pushes. |
| Maintenance | Per-source limits, `X-Request-ID` correlation, and daily removal of expired credentials, throttle windows and email links. Free and Paid profiles set minute-cron budgets for copies, cleanup, unfinished pushes, PR refresh, the merge queue and Webhooks. |
| Managed execution | A Durable Object alarm checks the protected queue on enqueue and after a passing check. A copy Workflow publishes forks after their LFS pages finish. D1 retains both jobs and the minute cron recovers outstanding work. |
| Backups | Rehearsed isolated D1 SQL restore, Git bundle restore with matching refs and object IDs, and byte-identical R2 downloads from restored original and fork namespaces. |
| UI and development | The shared theme follows system appearance. Geist and Shard Han set proportional UI text with tabular figures. JetBrains Mono sets code and Git identifiers, with two-cell Han glyphs in code. Buttons and form controls use calibrated font metrics. The refs grid marks default branches and wraps full annotated-tag object IDs on narrow screens. |
| Select controls | Themed pickers use native form and keyboard behavior. Browsers without `base-select` use their system picker. |
| Navigation and motion | Narrow repository tabs wrap and list metadata aligns with its title. Supporting browsers animate same-origin navigation, mobile menus and themed pickers. Reduced motion keeps keyboard focus and diff target markers visible. |

## Remaining work and prerequisites

| Area | Requirement |
|---|---|
| Deployment | Verify the Free badge's full account provisioning and resource limits on hosted Workers Free. Exercise managed releases and Artifacts in an isolated Workers Paid environment. Hosted native staging needs an HTTPS Git origin and persistent storage. |
| Large LFS uploads | Verify presigned S3 uploads with a hosted private R2 bucket. Objects above the single-PUT ceiling require a hosted check of whole-object hashing, conditional multipart publication, fork copy throughput and the Worker CPU budget. |
| Email | An onboarded sending domain, an `EMAIL` binding, and an available mailbox to verify real delivery. |
| Web merges | The Free Git UI advances same-repository fast-forwards. For other Free and mixed-backend merges, integrate through Git. Verify the managed Git container against hosted Artifacts for divergent and cross-repository merges. |
| Repository search | Scans one selected snapshot with explicit limits. A persistent index and whole-history search remain outside the current search scope. |
| Git protocol | Artifacts switches through Shard to the classic advertisement when HEAD is overridden. Production partial clone and fetch compatibility require verification. Provider-side updates that bypass Shard need separate event ingestion. |
| Operations | Live metrics, token-list pagination, and further CSP tightening after moving inline styles. |

Architecture, service boundaries, and operational constraints are recorded in [PLAN.md](PLAN.md#optional-cloudflare-execution-services). Managed CI/CD settings are in the [deployment guide](README.md#deploy-with-managed-cloudflare-services).

## Cloudflare Artifacts constraints (verified 2026-11-17)

- Workers binding: namespace `create/get/list/import/delete`; repo `info/createToken/listTokens/revokeToken/fork/log/readCommit/readTree/readBlob/readFile`.
- Shard uses the binding for object and history reads and Git HTTPS for ref discovery and supported ref mutations. A Git container or an external native bridge handles Artifacts merges that need new Git objects.
- Commit metadata uses `treeHash`, `authoredAt`, `committedAt`; tree entries are `{ name, mode, type, hash }`.
- Git tokens: Bearer, or Basic with the token secret before `?expires=`; write scope required for `git-receive-pack` including its `info/refs` advertisement.
- Artifacts requires Workers Paid; local simulator requires Wrangler ≥ 4.163 and Git ≥ 2.32.

## Technical debt / risks

- `Router` is rebuilt per request; routes allocate strings. Move to a cached router with request-scoped state.
- Repository creation spans D1 and Git storage. Creation attempts to remove storage after a failed metadata insert. Deletion removes the repository record and queues Git and R2 cleanup in D1, and completing the cleanup depends on continued cron runs.
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
| 2026-11-18 | Isolated HTTPS Git source and authenticated native origin | Basic and Bearer imports preserved the commit and file, created private repositories and refused an upstream redirect. |
| 2026-11-18 | Chromium at 1365px and 390px | Filtered a Git ref, queued a protected merge, created a tag, applied an issue template while preserving the title, reacted to an issue and rendered two uploaded commit avatars. The tested pages fit the viewport in light and dark modes. |
| 2026-11-19 | Isolated Artifacts queue and webhook regression | Rejected a direct push to a protected branch. Failed CI kept the target unchanged, and successful CI advanced it to the reviewed commit. A tag event retried with the same body, signature and IDs, and disabling the subscription stopped delivery. Fork and repository creation succeeded after the queue ran. |
| 2026-11-19 | Native Git bridge with an Artifacts-shaped Smart HTTP remote | Merged a divergent fork with both parents and both file changes. A changed target returned 412 and a conflicting merge returned 409 with the target ref intact. |
| 2026-11-19 | Wrangler dry run, both environments | Both resolved D1, R2, rate limiting and assets. The default environment also resolved Artifacts. |
| 2026-11-19 | `mise run build` and both loopback regression suites | The Worker built successfully and all 29 tests passed. The Git proxy reads through HTTP EOF after the final packet so the Artifacts origin can finish updating its refs before later Git operations. |
| 2026-11-19 | `mise run test` for both Git backends | The Wasm build and all 29 tests passed. A cross-repository native PR used one merge-base and one file comparison request. A private LFS fork remained readable after source cleanup. A scheduled check recovered a ref event after its D1 write was interrupted. |
| 2026-11-19 | Prebuilt Worker dry run | Both packaged configurations resolved their bindings. Artifacts used the Paid profile and native used Free. |
| 2026-11-19 | Isolated D1 and R2 restore | The local SQL export placed a trigger ahead of its view. After reordering those statements in the import copy, the restored trigger recorded a ref update. The Worker served exact object bytes from both restored LFS namespaces. |
| 2026-11-19 | Wasm release build and isolated Git LFS tests | The 29 existing HTTP and native Git regressions passed. Two new cases exercised the real Git LFS client against Miniflare's signed S3 endpoint. A staged direct upload stayed hidden before verification. Multipart resumed after the first of two parts and rejected a wrong whole-object SHA-256. |
| 2026-11-19 | Release notes and prebuilt archive rehearsal | The notes extractor selected `0.1.0` and `0.2.0` independently and rejected an undocumented prerelease and an invalid tag. The archive contained the migration, client adapter and changelog. Wrangler dry run passed for both environments. |
| 2026-11-19 | Unified deployment against an isolated Cloudflare API | Wrangler created missing D1 and R2 resources, applied migrations before upload and reused an existing database in native mode. Upload metadata preserved dashboard text, JSON and secrets, and carried the native secret on first deploy. An explicit UUID took precedence. A failed migration prevented publication, and mismatched R2 bucket settings stopped before remote requests. |
| 2026-11-19 | Prebuilt deploy command | The archive installed its pinned npm dependencies independently. Dry runs through the deploy script resolved both environments, and the archive's `npm run deploy -- --dry-run` resolved the default bindings. |
| 2026-11-19 | `npm run ci -- --container` | All 46 tests passed, including automatic D1 reuse and rejection, protected merge alarms, and a multi-page LFS fork published by a Workflow. The Docker image started, reported bridge protocol 1, and returned HTTP 401 to an unauthenticated merge. |
| 2026-11-19 | `npm run cd -- --dry-run` and isolated release gate | Both managed bundles built their Git image and resolved independent bindings. With real Wrangler Workflow commands against a loopback API, production followed matching staging success and was skipped on staging error, wrong revision or a non-production branch. |
| 2026-11-19 | Release Workflow on Linux with Docker | The local Workflow checked the configured version tag, D1, R2 and CSS, then started the actual Git image through its Durable Object and verified readiness. It rejected an invalid release tag. |
| 2026-11-19 | Managed prebuilt deployment dry run | The release archive layout supplied the Worker entrypoint, Wasm, migrations and Docker context without a local Rust build. Wrangler built the merge image and bundled `managed-staging`. |
| 2026-11-19 | CD gate and native Git cron regressions | Four isolated Wrangler fixture cases checked the staging result, revision, branch and deployment order. With the coordinator unbound, the native Git test confirmed that one scheduled tick advanced a queued protected merge after its required check passed. |
| 2026-11-19 | Fresh Free Worker with a real Git client | Registration, public and private push, read-scoped access, pull request diff and fast-forward, ref and HEAD updates, an explicit LFS-disabled response, and fork clone after restart passed with the shipped Free bindings. |
| 2026-11-19 | Populated D1 upgrade | Migration 0027 preserved legacy backend IDs, a linked PR, a pending native fork, and the cleanup queue's high-water mark. It enforced the existing backend-matched path guard and queued cleanup for a Durable Object repository. |
| 2026-11-19 | Prebuilt deployment dry runs | Free, original Artifacts, native, Paid and Paid staging bundles passed. The Free upload compressed to about 1.34 MiB. Both Paid dry runs built the merge container and resolved multipart mode with the selected bucket. |
| 2026-11-19 | `mise run test` at `413d3cb` | Rebuilt the Worker and passed all 74 tests. Coverage included the scheduled pull request refresh in the native Git suite, the default Free Git scenario, Artifacts HTTP, signed R2 direct and multipart uploads, and the deployment and release gates. |
| 2026-11-19 | First Free publish through Wrangler and a loopback Cloudflare API | A dashboard-style D1 binding applied all 25 schema migrations before Worker upload. Upload metadata contained the `free-git-v1` SQLite class migration and no R2, Artifacts or Workflow bindings. |
| 2026-11-19 | Free public Git import | A local Git `upload-pack` fixture supplied the HTTPS advertisement and pack response. The repository kept its `trunk` default, refs and imported file contents. |
| 2026-11-19 | Concurrent SQLite Git mirrors and default branch deletion | Four simultaneous Git mirrors of a repository with 8,300 blobs preserved both refs and the expected file and passed `git fsck --full`. The repository rejected a push that deleted its configured default branch. |
| 2026-11-19 | v0.2.0 pre-release checks | `mise run check` and all 83 tests in `npm run ci -- --container` passed. The Git merge container reported bridge protocol 1 and returned HTTP 401 to an unauthenticated merge. |

## How to update

- Keep the snapshot table current (date, commit, deploy state).
- Move an item only when its behavior is verified, not when code exists.
- When a limitation changes (for example Artifacts adds a write/merge API), update both this file and the README API notes.
