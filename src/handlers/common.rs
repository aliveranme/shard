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

const REPO_FROM: &str = "repos r JOIN users u ON u.id = r.owner_id";
const REPO_PATH_FROM: &str = "repo_paths p JOIN users pu ON pu.id = p.owner_id
    JOIN repos r ON r.id = p.repo_id JOIN users u ON u.id = r.owner_id";

const REPO_VIEWER_COLUMNS: &str = "
    EXISTS (SELECT 1 FROM repo_watches WHERE repo_id = r.id AND user_id = ?1) AS watched,
    EXISTS (SELECT 1 FROM repo_stars WHERE repo_id = r.id AND user_id = ?1) AS starred";

/// Listing queries use `r` for the repository and bind the viewer first.
pub(crate) const VISIBLE_REPO_SQL: &str = "(r.visibility = 'public' OR r.owner_id = ?1 OR ?2 = 1
    OR EXISTS (SELECT 1 FROM repo_member_roles c WHERE c.repo_id = r.id AND c.user_id = ?1))";

pub(crate) fn viewer_params(state: &AppState) -> [JsValue; 2] {
    match &state.user {
        Some(user) => [crate::util::js_int(user.id), crate::util::js_int(user.is_admin)],
        None => [crate::util::js_null(), crate::util::js_int(0)],
    }
}

pub async fn load_repo(state: &AppState, owner: &str, name: &str) -> Result<Option<Repo>> {
    let database = state.db()?;
    let [viewer, _] = viewer_params(state);
    db::first(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS}, {REPO_VIEWER_COLUMNS} FROM {REPO_PATH_FROM}
             WHERE pu.username = ?2 COLLATE NOCASE AND p.name = ?3 COLLATE NOCASE"
        ),
        &[viewer, crate::util::js_text(owner), crate::util::js_text(name)],
    )
    .await
}

/// Load a canonical repository by ID after the caller authorizes the operation.
pub(crate) async fn load_repo_by_id(database: &d1::D1Database, id: i64) -> Result<Option<Repo>> {
    db::first(
        database,
        &format!("SELECT {REPO_COLUMNS} FROM {REPO_FROM} WHERE r.id = ?"),
        &[crate::util::js_int(id)],
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
pub(crate) enum RepoRole {
    Read,
    Write,
}

pub(crate) async fn repo_role(
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
        "SELECT role FROM repo_member_roles WHERE repo_id = ? AND user_id = ?
         ORDER BY role = 'write' DESC LIMIT 1",
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

/// Resolve a repository path and enforce current visibility in the same query.
pub async fn visible_repo(state: &AppState, owner: &str, name: &str) -> Result<Option<Repo>> {
    let [viewer, admin] = viewer_params(state);
    db::first(
        &state.db()?,
        &format!(
            "SELECT {REPO_COLUMNS}, {REPO_VIEWER_COLUMNS} FROM {REPO_PATH_FROM}
             WHERE pu.username = ?3 COLLATE NOCASE AND p.name = ?4 COLLATE NOCASE AND {VISIBLE_REPO_SQL}"
        ),
        &[viewer, admin, crate::util::js_text(owner), crate::util::js_text(name)],
    )
    .await
}

pub(crate) async fn visible_repo_by_id(state: &AppState, id: i64) -> Result<Option<Repo>> {
    let [viewer, admin] = viewer_params(state);
    db::first(
        &state.db()?,
        &format!("SELECT {REPO_COLUMNS}, {REPO_VIEWER_COLUMNS} FROM {REPO_FROM}
                  WHERE r.id = ?3 AND {VISIBLE_REPO_SQL}"),
        &[viewer, admin, crate::util::js_int(id)],
    )
    .await
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
