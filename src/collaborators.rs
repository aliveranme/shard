use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{can_admin, forbidden, not_found, page_title, visible_repo};
use crate::i18n::{CollaboratorText, CommonText};
use crate::models::Repo;
use crate::templates::{render, Chrome};
use crate::util;
use askama::Template;
use worker::*;

#[derive(serde::Deserialize)]
struct Collaborator {
    id: i64,
    username: String,
    role: String,
}

#[derive(Template)]
#[template(path = "collaborators.html")]
struct CollaboratorsPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    collaborators: Vec<Collaborator>,
    notice: Option<&'static str>,
    error: Option<&'static str>,
}

enum Operation {
    View,
    Save,
    Revoke,
}

pub async fn settings(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    manage(req, ctx, Operation::View).await
}

pub async fn save(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    manage(req, ctx, Operation::Save).await
}

pub async fn revoke(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    manage(req, ctx, Operation::Revoke).await
}

async fn manage(
    req: Request,
    ctx: RouteContext<AppState>,
    operation: Operation,
) -> Result<Response> {
    let state = &ctx.data;
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(user_id) = state.user_id().filter(|_| can_admin(state, &repo)) else {
        return forbidden(state, state.locale.t(CollaboratorText::ManageForbidden));
    };
    if matches!(operation, Operation::View) {
        let notice = match util::query_param(&req, "result").as_deref() {
            Some("saved") => Some(CollaboratorText::Saved),
            Some("revoked") => Some(CollaboratorText::Revoked),
            _ => None,
        };
        return render_page(state, repo, notice, None).await;
    }

    let form = state.form()?;
    let username = util::form_value(form, "username").unwrap_or_default();
    let username = username.trim();
    if !util::valid_slug(username) {
        return render_page(state, repo, None, Some(CollaboratorText::InvalidAccount)).await;
    }
    let database = state.db()?;
    let (result, success, missing) = match operation {
        Operation::Save => {
            let role = util::form_value(form, "role");
            let Some(role @ ("read" | "write")) = role.as_deref() else {
                return render_page(state, repo, None, Some(CollaboratorText::InvalidRole)).await;
            };
            let result = database
                .prepare(
                    "INSERT INTO repo_collaborators (repo_id, user_id, role)
                     SELECT r.id, target.id, ?2 FROM repos r JOIN users actor ON actor.id = ?4
                     JOIN users target ON target.username = ?3 COLLATE NOCASE
                     WHERE r.id = ?1 AND target.id <> r.owner_id AND (actor.id = r.owner_id OR actor.is_admin = 1)
                     ON CONFLICT(repo_id, user_id) DO UPDATE SET role = excluded.role",
                )
                .bind(&[
                    util::js_int(repo.id),
                    util::js_text(role),
                    util::js_text(username),
                    util::js_int(user_id),
                ])?
                .run()
                .await?;
            (result, "saved", CollaboratorText::InvalidAccount)
        }
        Operation::Revoke => {
            let result = database
                .prepare(
                    "DELETE FROM repo_collaborators WHERE repo_id = ?1 AND user_id =
                     (SELECT id FROM users WHERE username = ?2 COLLATE NOCASE)
                     AND EXISTS (SELECT 1 FROM repos r JOIN users actor ON actor.id = ?3
                                  WHERE r.id = ?1 AND (actor.id = r.owner_id OR actor.is_admin = 1))",
                )
                .bind(&[util::js_int(repo.id), util::js_text(username), util::js_int(user_id)])?
                .run()
                .await?;
            (result, "revoked", CollaboratorText::MissingGrant)
        }
        Operation::View => unreachable!(),
    };
    let changes = result
        .meta()?
        .and_then(|meta| meta.changes)
        .ok_or_else(|| Error::RustError("D1 did not report collaborator changes".into()))?;
    if changes == 0 {
        let allowed = database
            .prepare(
                "SELECT r.id FROM repos r JOIN users actor ON actor.id = ?2
              WHERE r.id = ?1 AND (actor.id = r.owner_id OR actor.is_admin = 1)",
            )
            .bind(&[util::js_int(repo.id), util::js_int(user_id)])?
            .first::<i64>(Some("id"))
            .await?;
        if allowed.is_none() {
            return forbidden(state, state.locale.t(CollaboratorText::ManageForbidden));
        }
        return render_page(state, repo, None, Some(missing)).await;
    }
    util::redirect(&format!(
        "/{}/settings/collaborators?result={success}",
        repo.full_name()
    ))
}

async fn render_page(
    state: &AppState,
    repo: Repo,
    notice: Option<CollaboratorText>,
    error: Option<CollaboratorText>,
) -> Result<Response> {
    let collaborators = db::all(
        &state.db()?,
        "SELECT u.id, u.username, c.role FROM repo_collaborators c
         JOIN repos r ON r.id = c.repo_id JOIN users u ON u.id = c.user_id
         JOIN users actor ON actor.id = ?2
         WHERE c.repo_id = ?1 AND (actor.id = r.owner_id OR actor.is_admin = 1)
         ORDER BY u.username COLLATE NOCASE, u.id",
        &[
            util::js_int(repo.id),
            state.user_id().map_or_else(util::js_null, util::js_int),
        ],
    )
    .await?;
    let page = CollaboratorsPage {
        chrome: Chrome::new(state),
        title: page_title(&[
            &repo.full_name(),
            state.locale.t(CollaboratorText::Collaborators),
        ]),
        repo,
        tab: "settings",
        is_owner: true,
        collaborators,
        notice: notice.map(|key| state.locale.t(key)),
        error: error.map(|key| state.locale.t(key)),
    };
    Ok(render(&page)?.with_status(if error.is_some() { 400 } else { 200 }))
}
