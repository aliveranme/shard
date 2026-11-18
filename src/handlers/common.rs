use crate::ctx::AppState;
use crate::db;
use crate::models::Repo;
use crate::templates::{error_response, Chrome};
use wasm_bindgen::JsValue;
use worker::*;

pub const REPO_COLUMNS: &str = "r.id, r.owner_id, u.username AS owner_name, r.name, r.description,
    r.default_branch, r.git_backend, r.artifacts_repo AS storage_id, r.visibility, r.next_issue_number, r.forked_from,
    r.created_at, r.updated_at,
    (SELECT COUNT(*) FROM issues i WHERE i.repo_id = r.id AND i.kind = 'issue' AND i.state = 'open') AS open_issues,
    (SELECT COUNT(*) FROM issues i WHERE i.repo_id = r.id AND i.kind = 'pull' AND i.state = 'open') AS open_pulls";

/// Listing queries use `r` for the repository and bind the viewer first.
pub(crate) const VISIBLE_REPO_SQL: &str = "(r.visibility = 'public' OR r.owner_id = ?1 OR ?2 = 1
    OR EXISTS (SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = ?1))";

pub(crate) fn viewer_params(state: &AppState) -> [JsValue; 2] {
    match &state.user {
        Some(user) => [crate::util::js_int(user.id), crate::util::js_int(user.is_admin)],
        None => [crate::util::js_null(), crate::util::js_int(0)],
    }
}

pub async fn load_repo(state: &AppState, owner: &str, name: &str) -> Result<Option<Repo>> {
    let database = state.db()?;
    db::first(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repo_paths p JOIN users pu ON pu.id = p.owner_id
             JOIN repos r ON r.id = p.repo_id JOIN users u ON u.id = r.owner_id
             WHERE pu.username = ? COLLATE NOCASE AND p.name = ? COLLATE NOCASE"
        ),
        &[crate::util::js_text(owner), crate::util::js_text(name)],
    )
    .await
}

pub fn can_admin(state: &AppState, repo: &Repo) -> bool {
    match &state.user {
        Some(user) => is_repo_admin(repo.owner_id, user.id, user.is_admin),
        None => false,
    }
}

fn is_repo_admin(owner_id: i64, user_id: i64, is_admin: i64) -> bool {
    is_admin == 1 || owner_id == user_id
}

#[derive(Clone, Copy, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum RepoRole {
    Read,
    Write,
}

pub(super) async fn repo_role(
    database: &d1::D1Database,
    repo_id: i64,
    owner_id: i64,
    user_id: i64,
    is_admin: i64,
) -> Result<Option<RepoRole>> {
    if is_repo_admin(owner_id, user_id, is_admin) {
        return Ok(Some(RepoRole::Write));
    }
    #[derive(serde::Deserialize)]
    struct Grant {
        role: RepoRole,
    }
    let grant: Option<Grant> = db::first(
        database,
        "SELECT role FROM repo_collaborators WHERE repo_id = ? AND user_id = ?",
        &[crate::util::js_int(repo_id), crate::util::js_int(user_id)],
    )
    .await?;
    Ok(grant.map(|grant| grant.role))
}

/// Repository writers can push. Only owners and site admins can manage settings.
pub async fn can_write(state: &AppState, repo: &Repo) -> Result<bool> {
    let Some(user) = &state.user else {
        return Ok(false);
    };
    if can_admin(state, repo) {
        return Ok(true);
    }
    Ok(repo_role(&state.db()?, repo.id, repo.owner_id, user.id, user.is_admin).await?
        == Some(RepoRole::Write))
}

pub fn not_found(state: &AppState, message: &'static str) -> Result<Response> {
    error_response(&Chrome::new(state), 404, message)
}

pub fn forbidden(state: &AppState, message: &'static str) -> Result<Response> {
    error_response(&Chrome::new(state), 403, message)
}

/// Load a repo and enforce visibility in one go.
pub async fn visible_repo(state: &AppState, owner: &str, name: &str) -> Result<Option<Repo>> {
    let Some(repo) = load_repo(state, owner, name).await? else {
        return Ok(None);
    };
    if repo.visibility == "public" || can_admin(state, &repo) {
        return Ok(Some(repo));
    }
    let Some(user) = &state.user else {
        return Ok(None);
    };
    let role = repo_role(&state.db()?, repo.id, repo.owner_id, user.id, user.is_admin).await?;
    Ok(role.map(|_| repo))
}

pub fn page_title(parts: &[&str]) -> String {
    let mut title = String::new();
    for part in parts {
        title.push_str(part);
        title.push_str(" - ");
    }
    title.push_str("shard");
    title
}
