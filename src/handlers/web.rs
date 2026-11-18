use super::common::{REPO_COLUMNS, page_title};
use crate::ctx::AppState;
use crate::db;
use crate::models::{Repo, Ts};
use crate::pagination::{query_url, PageRequest, PAGE_SIZE};
use crate::templates::{
    render, Chrome, GlobalIssueRow, GlobalIssuesPage, HomePage, ProfilePage, RepoRow,
};
use crate::util;
use worker::*;

pub async fn home(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let viewer = state.user_id().unwrap_or(-1);
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE r.visibility = 'public' OR r.owner_id = ?
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[util::js_int(viewer), util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset())],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = HomePage {
        chrome: Chrome::new(&state),
        title: page_title(&["Dashboard"]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_explore: false,
        query: String::new(),
        pagination,
    };
    render(&page)
}

pub async fn explore(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let query = util::query_param(&req, "q").unwrap_or_default();
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let like = format!("%{}%", query.trim());
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE r.visibility = 'public'
               AND (? = '' OR r.name LIKE ? OR r.description LIKE ? OR u.username LIKE ?)
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[
            util::js_text(query.trim()), util::js_text(&like),
            util::js_text(&like), util::js_text(&like),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = HomePage {
        chrome: Chrome::new(&state),
        title: page_title(&["Explore"]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_explore: true,
        query,
        pagination,
    };
    render(&page)
}

pub async fn profile(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
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
    let request_page = PageRequest::new(&req, "page");
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE r.owner_id = ? AND (r.visibility = 'public' OR r.owner_id = ?)
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[util::js_int(user.id), util::js_int(viewer), util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset())],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = ProfilePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&user.username]),
        name: user.username.clone(),
        joined: user.created_at,
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_self: viewer == user.id,
        pagination,
    };
    render(&page)
}

#[derive(serde::Deserialize)]
struct GlobalRow {
    pub number: i64,
    pub title: String,
    pub created_at: Ts,
    pub kind: String,
    pub state: String,
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
    req: Request,
    ctx: RouteContext<AppState>,
    kind: &str,
) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect(if kind == "pull" { "/user/login?next=/pulls" } else { "/user/login?next=/issues" });
    };
    let state_filter = match util::query_param(&req, "state").as_deref() {
        Some("closed") => "closed",
        Some("all") => "all",
        _ => "open",
    };
    let query = util::query_param(&req, "q").unwrap_or_default();
    let like = format!("%{}%", query.trim());
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let mut rows: Vec<GlobalRow> = db::all(
        &database,
        "SELECT i.number, i.title, i.created_at, i.kind, i.state, u.username AS author_name,
                r.name AS repo_name, o.username AS repo_owner
           FROM issues i
           JOIN repos r ON r.id = i.repo_id
           JOIN users o ON o.id = r.owner_id
           LEFT JOIN users u ON u.id = i.author_id
          WHERE i.kind = ? AND (? = 'all' OR i.state = ?)
            AND (r.owner_id = ? OR r.visibility = 'public')
            AND (? = '' OR i.title LIKE ?)
          ORDER BY i.updated_at DESC, i.id DESC LIMIT ? OFFSET ?",
        &[
            util::js_text(kind), util::js_text(state_filter), util::js_text(state_filter),
            util::js_int(user.id), util::js_text(query.trim()), util::js_text(&like),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut rows, None)?;
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
            state: row.state,
        })
        .collect();
    let is_pull = kind == "pull";
    let page = GlobalIssuesPage {
        chrome: Chrome::new(&state),
        title: page_title(&[if is_pull { "Pull requests" } else { "Issues" }]),
        heading: if is_pull {
            "Pull requests across your projects".into()
        } else {
            "Issues across your projects".into()
        },
        issues,
        state: state_filter.into(),
        query,
        open_url: query_url(&req, &[("state", Some("open")), ("page", None)], None)?,
        closed_url: query_url(&req, &[("state", Some("closed")), ("page", None)], None)?,
        all_url: query_url(&req, &[("state", Some("all")), ("page", None)], None)?,
        pagination,
    };
    render(&page)
}

