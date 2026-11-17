use super::common::{REPO_COLUMNS, page_title};
use crate::ctx::AppState;
use crate::db;
use crate::models::{Repo, Ts};
use crate::templates::{
    render, Chrome, GlobalIssueRow, GlobalIssuesPage, HomePage, ProfilePage, RepoRow,
};
use crate::util;
use worker::*;

pub async fn home(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let viewer = state.user_id().unwrap_or(-1);
    let database = state.db()?;
    let repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE r.visibility = 'public' OR r.owner_id = ?
             ORDER BY r.updated_at DESC LIMIT 30"
        ),
        &[util::js_int(viewer)],
    )
    .await?;
    let page = HomePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["Dashboard"]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_explore: false,
        query: String::new(),
    };
    render(&page)
}

pub async fn explore(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let query = util::query_param(&req, "q").unwrap_or_default();
    let database = state.db()?;
    let repos: Vec<Repo> = if query.trim().is_empty() {
        db::all(
            &database,
            &format!(
                "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
                 WHERE r.visibility = 'public'
                 ORDER BY r.updated_at DESC LIMIT 50"
            ),
            &[],
        )
        .await?
    } else {
        let like = format!("%{}%", query.trim());
        db::all(
            &database,
            &format!(
                "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
                 WHERE r.visibility = 'public' AND (r.name LIKE ? OR r.description LIKE ? OR u.username LIKE ?)
                 ORDER BY r.updated_at DESC LIMIT 50"
            ),
            &[util::js_text(&like), util::js_text(&like), util::js_text(&like)],
        )
        .await?
    };
    let page = HomePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["Explore"]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_explore: true,
        query,
    };
    render(&page)
}

pub async fn profile(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let username = ctx.param("owner").cloned().unwrap_or_default();
    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct Profile {
        id: i64,
        username: String,
        created_at: Ts,
    }
    let user: Option<Profile> = db::first(
        &database,
        "SELECT id, username, created_at FROM users WHERE username = ? COLLATE NOCASE",
        &[util::js_text(&username)],
    )
    .await?;
    let Some(user) = user else {
        return super::common::not_found(&state, "User not found.");
    };
    let viewer = state.user_id().unwrap_or(-1);
    let repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE r.owner_id = ? AND (r.visibility = 'public' OR r.owner_id = ?)
             ORDER BY r.updated_at DESC LIMIT 100"
        ),
        &[util::js_int(user.id), util::js_int(viewer)],
    )
    .await?;
    let page = ProfilePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&user.username]),
        name: user.username.clone(),
        joined: user.created_at,
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_self: viewer == user.id,
    };
    render(&page)
}

#[derive(serde::Deserialize)]
struct GlobalRow {
    pub number: i64,
    pub title: String,
    pub created_at: Ts,
    pub kind: String,
    pub author_name: Option<String>,
    pub repo_name: String,
    pub repo_owner: String,
}

pub async fn global_issues(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    global_list(req, ctx, "issue").await
}

pub async fn global_pulls(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    global_list(req, ctx, "pull").await
}

async fn global_list(
    _req: Request,
    ctx: RouteContext<AppState>,
    kind: &str,
) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/issues");
    };
    let database = state.db()?;
    let rows: Vec<GlobalRow> = db::all(
        &database,
        "SELECT i.number, i.title, i.created_at, i.kind, u.username AS author_name,
                r.name AS repo_name, o.username AS repo_owner
           FROM issues i
           JOIN repos r ON r.id = i.repo_id
           JOIN users o ON o.id = r.owner_id
           LEFT JOIN users u ON u.id = i.author_id
          WHERE i.kind = ? AND i.state = 'open' AND (r.owner_id = ? OR r.visibility = 'public')
          ORDER BY i.updated_at DESC LIMIT 50",
        &[util::js_text(kind), util::js_int(user.id)],
    )
    .await?;
    let issues = rows
        .into_iter()
        .map(|row| GlobalIssueRow {
            repo_full: format!("{}/{}", row.repo_owner, row.repo_name),
            owner: row.repo_owner,
            repo: row.repo_name,
            number: row.number,
            title: row.title,
            author: row.author_name.unwrap_or_else(|| "ghost".into()),
            date: row.created_at,
            is_pull: row.kind == "pull",
        })
        .collect();
    let is_pull = kind == "pull";
    let page = GlobalIssuesPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[if is_pull { "Pull requests" } else { "Issues" }]),
        heading: if is_pull {
            "Pull requests across your projects".into()
        } else {
            "Issues across your projects".into()
        },
        issues,
    };
    render(&page)
}

