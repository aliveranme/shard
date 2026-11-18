use crate::ctx::AppState;
use crate::db;
use crate::models::Repo;
use crate::templates::{error_response, Chrome};
use worker::*;

pub const REPO_COLUMNS: &str = "r.id, r.owner_id, u.username AS owner_name, r.name, r.description,
    r.default_branch, r.artifacts_repo, r.visibility, r.next_issue_number, r.forked_from,
    r.created_at, r.updated_at,
    (SELECT COUNT(*) FROM issues i WHERE i.repo_id = r.id AND i.kind = 'issue' AND i.state = 'open') AS open_issues,
    (SELECT COUNT(*) FROM issues i WHERE i.repo_id = r.id AND i.kind = 'pull' AND i.state = 'open') AS open_pulls";

pub async fn load_repo(state: &AppState, owner: &str, name: &str) -> Result<Option<Repo>> {
    let database = state.db()?;
    db::first(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE u.username = ? COLLATE NOCASE AND r.name = ? COLLATE NOCASE"
        ),
        &[crate::util::js_text(owner), crate::util::js_text(name)],
    )
    .await
}

pub fn can_admin(state: &AppState, repo: &Repo) -> bool {
    match &state.user {
        Some(user) => user.is_admin == 1 || user.id == repo.owner_id,
        None => false,
    }
}

/// Private repos are invisible (404) to everyone except their owner/admins.
pub fn can_view(state: &AppState, repo: &Repo) -> bool {
    !repo.is_private() || can_admin(state, repo)
}

pub fn not_found(state: &AppState, message: &str) -> Result<Response> {
    error_response(Chrome::new(state), 404, message)
}

pub fn forbidden(state: &AppState, message: &str) -> Result<Response> {
    error_response(Chrome::new(state), 403, message)
}


/// Load a repo and enforce visibility in one go.
pub async fn visible_repo(
    state: &AppState,
    owner: &str,
    name: &str,
) -> Result<Option<Repo>> {
    match load_repo(state, owner, name).await? {
        Some(repo) if can_view(state, &repo) => Ok(Some(repo)),
        _ => Ok(None),
    }
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
