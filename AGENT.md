# Agent instructions

## Project and scope

Shard is a Rust Git forge on Cloudflare Workers. D1 stores forge metadata; Cloudflare Artifacts stores Git repositories. Askama renders the HTML; `static/css/shard.css` is the shared stylesheet. Read `README.md`, `STATUS.md` and `DESIGN.md` before changing behavior. Preserve unrelated local changes. Never deploy, create paid resources, or delete user data without an explicit request.

## Architecture

- `src/lib.rs`: Worker events and route registration.
- `src/ctx.rs`: request-scoped authentication, CSRF token and parsed form data.
- `src/handlers/`: HTTP behavior and authorization; use the existing `common`, `db`, `auth` and `util` helpers.
- `src/artifacts.rs`: the Artifacts binding boundary. The binding has no merge, branch enumeration or object-write API. Do not invent unsupported functionality.
- `src/templates.rs` and `templates/`: typed page models and autoescaped HTML. Only render sanitized Markdown as safe HTML.
- `migrations/`: append forward-only D1 migrations; never rewrite an applied migration.

## UI contract

Follow `DESIGN.md`: white canvas, warm `#F5F5F0` surfaces, amber `#F2C94C` primary actions, lavender `#E6D5F5` accents, rounded cards and pill controls. Reuse existing components and settings layouts. Use Segoe UI/system fonts; do not bundle proprietary fonts. Keep keyboard focus visible, label every form control, and verify narrow screens. Show real empty, error and success states. Do not add decorative statistics or nonfunctional controls.

## Data and security

Enforce repository visibility and ownership in handlers and SQL, not just templates. Keep Git authentication separate from browser sessions. Every browser POST must pass the centralized origin and CSRF checks; include `templates/csrf.html` in each POST form and read parsed fields through `AppState::form()`. Never put session secrets, access tokens or passwords in URLs or logs. Hash credentials at rest; reveal newly created tokens once. Verify the current password before account credential changes. Preserve search, filter and branch parameters in pagination links, and use deterministic ordering.

## Workflow and verification

Use the pinned mise toolchain. Standard commands:

- `mise run setup`: install dependencies and the wasm target (first setup only).
- `mise run db:migrate`: apply local D1 migrations.
- `mise run check`: type-check the wasm Worker.
- `mise run build`: produce the Worker bundle.
- `mise run dev`: run the local simulator at `http://localhost:8787`.
- `mise run test`: run the HTTP regression suite against an isolated local simulator.

Run the narrowest relevant checks after implementing a complete change. For UI changes, inspect the actual page in a browser at desktop and mobile widths. Cover permission, CSRF, credential and pagination boundaries with consumer-visible regression tests. Tests must use isolated data and must not contact deployed services. Update `README.md` and `STATUS.md` with verified behavior and remaining constraints. Keep this instruction file current when commands or conventions change.
