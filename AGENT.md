# Agent instructions

## Project and scope

Shard is a Rust Git forge on Cloudflare Workers. D1 stores forge metadata. SQLite Durable Objects store Git by default. Named environments use Cloudflare Artifacts or an authenticated native Git origin. Askama renders the HTML and `static/css/shard.css` is the shared stylesheet. Read `README.md`, `STATUS.md` and `DESIGN.md` before changing behavior. Preserve unrelated local changes. Never deploy, create paid resources, or delete user data without an explicit request.

## Architecture

- `src/lib.rs`: Worker events and route registration.
- `src/ctx.rs` holds request-scoped authentication, locale, CSRF token and parsed form data.
- `src/handlers/`: HTTP behavior and authorization; use the existing `common`, `db`, `auth` and `util` helpers.
- `src/git_storage.rs`: shared Git backend boundary. Persist the backend with its repository and copy jobs.
- `src/artifacts.rs`: the Artifacts binding adapter. The binding has no merge, branch enumeration or object-write API.
- `src/native_git.rs` and `scripts/git-origin.mjs`: authenticated native Git transport and bare-repository management.
- `worker/git-repository.mjs`: SQLite Durable Object for Free Git storage. It serves the Git protocol and the internal object and ref API.
- `src/templates.rs` and `templates/`: typed page models and autoescaped HTML. Only render sanitized Markdown as safe HTML.
- `src/i18n/` defines typed interface messages. Add user-facing UI text, accessibility labels, and browser errors to the matching catalog with both translations. Keep interpolated phrases whole.
- `migrations/`: append forward-only D1 migrations; never rewrite an applied migration.

## UI contract

Use the `b-end-design-system` skill for data and settings pages and follow the Shard section of `DESIGN.md`. Keep light and dark colors in `static/css/theme.css`. Use shared type, spacing, radius, ring and motion tokens. Use the local proportional UI face with tabular figures and the local monospace face for code and Git identifiers. Keep control text optically centered and use adaptive grids. Verify keyboard focus, narrow screens and reduced motion. Label form controls and show real empty and error states.

## Data and security

Enforce repository visibility and ownership in handlers and SQL, not just templates. Keep Git authentication separate from browser sessions. Every browser POST must pass the centralized origin and CSRF checks; include `templates/csrf.html` in each POST form and read parsed fields through `AppState::form()`. Never put session secrets, access tokens or passwords in URLs or logs. Hash credentials at rest; reveal newly created tokens once. Verify the current password before account credential changes. Preserve search, filter and branch parameters in pagination links, and use deterministic ordering.

## Workflow and verification

Use the pinned mise toolchain. Standard commands:

- `mise run setup` installs the pinned Rust target, IDE tools and dependencies.
- `mise run db:migrate`: apply local D1 migrations.
- `mise run check`: type-check the wasm Worker.
- `mise run lint` runs Clippy for the wasm Worker. `mise run fmt` runs rustfmt.
- `mise run build`: produce the Worker bundle.
- `mise run dev`: run the local simulator at `http://localhost:8787`.
- `mise run test` runs isolated HTTP and real Git regressions for all three storage modes.
- `mise run deploy` and `npm run deploy` resolve the target `DB` binding,
  apply migrations, and publish the Worker. The shipped `keep_vars` preserves
  dashboard-managed Worker variables.
- `npm run deploy:paid -- --secrets-file .env.deploy.paid` selects the managed
  Paid environment and enables R2 multipart uploads. Use `--env artifacts`
  with the regular deploy command to upgrade the original Artifacts instance.
- Managed Workers Builds use `npm run ci -- --container` for build, tests and
  image smoke checks, then `npm run cd` for staged release verification.
  See [README.md](README.md#deploy-with-managed-cloudflare-services).

Run the narrowest relevant checks after implementing a complete change. For UI changes, inspect the actual page in a browser at desktop and mobile widths. Cover permission, CSRF, credential and pagination boundaries with consumer-visible regression tests. Tests must use isolated data and must not contact deployed services. Update `README.md` and `STATUS.md` with verified behavior and remaining constraints. Keep this instruction file current when commands or conventions change.

Commit every completed feature change to Git after its relevant checks pass. Keep each commit focused on one feature and record the behavior and verification in `STATUS.md`. Preserve unrelated worktree changes. A local commit does not authorize a push.
