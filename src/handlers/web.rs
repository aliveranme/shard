use super::common::{REPO_COLUMNS, VISIBLE_REPO_SQL, page_title, viewer_params};
use crate::ctx::AppState;
use crate::db;
use crate::i18n::{CommonText, WebText};
use crate::models::{Repo, Ts};
use crate::pagination::{query_url, PageRequest, PAGE_SIZE};
use crate::templates::{
    render, Avatar, Chrome, GlobalIssueRow, GlobalIssuesPage, HomePage, ProfilePage, RepoRow,
};
use crate::util;
use worker::*;

pub async fn home(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let [viewer_id, viewer_admin] = viewer_params(&state);
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE {VISIBLE_REPO_SQL}
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[
            viewer_id,
            viewer_admin,
            util::js_int((PAGE_SIZE + 1) as i64),
            util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = HomePage {
        chrome: Chrome::new(&state),
        title: page_title(&[state.locale.t(WebText::Dashboard)]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_explore: false,
        query: String::new(),
        pagination,
    };
    render(&page)
}

pub async fn explore(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let [viewer_id, viewer_admin] = viewer_params(&state);
    let query = util::query_param(&req, "q").unwrap_or_default();
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let like = format!("%{}%", query.trim());
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE {VISIBLE_REPO_SQL}
               AND (? = '' OR r.name LIKE ? OR r.description LIKE ? OR u.username LIKE ?)
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[
            viewer_id, viewer_admin,
            util::js_text(query.trim()), util::js_text(&like),
            util::js_text(&like), util::js_text(&like),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = HomePage {
        chrome: Chrome::new(&state),
        title: page_title(&[state.locale.t(CommonText::Explore)]),
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
        avatar_id: Option<i64>,
    }
    let user: Option<Profile> = db::first(
        &database,
        "SELECT u.id, u.username, u.created_at, av.user_id AS avatar_id
           FROM users u LEFT JOIN user_avatars av ON av.user_id = u.id
          WHERE u.username = ? COLLATE NOCASE",
        &[util::js_text(&username)],
    )
    .await?;
    let Some(user) = user else {
        return super::common::not_found(&state, state.locale.t(WebText::UserNotFound));
    };
    let [viewer_id, viewer_admin] = viewer_params(&state);
    let request_page = PageRequest::new(&req, "page");
    let mut repos: Vec<Repo> = db::all(
        &database,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repos r JOIN users u ON u.id = r.owner_id
             WHERE {VISIBLE_REPO_SQL} AND r.owner_id = ?
             ORDER BY r.updated_at DESC, r.id DESC LIMIT ? OFFSET ?"
        ),
        &[
            viewer_id,
            viewer_admin,
            util::js_int(user.id),
            util::js_int((PAGE_SIZE + 1) as i64),
            util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut repos, None)?;
    let page = ProfilePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&user.username]),
        avatar: Avatar::new(&user.username, user.avatar_id),
        name: user.username.clone(),
        joined: user.created_at,
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        is_self: state.user_id() == Some(user.id),
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
    if state.user.is_none() {
        return util::redirect(if kind == "pull" { "/user/login?next=/pulls" } else { "/user/login?next=/issues" });
    }
    let [viewer_id, viewer_admin] = viewer_params(&state);
    let state_filter = match util::query_param(&req, "state").as_deref() {
        Some("closed") => "closed",
        Some("all") => "all",
        _ => "open",
    };
    let query = util::query_param(&req, "q").unwrap_or_default();
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;
    let filters = crate::issue_filters::IssueFilters::from_request(&req);
    let mut sql = format!(
        "SELECT i.number, i.title, i.created_at, i.kind, i.state, u.username AS author_name,
                r.name AS repo_name, o.username AS repo_owner
           FROM issues i
           JOIN repos r ON r.id = i.repo_id
           JOIN users o ON o.id = r.owner_id
           LEFT JOIN users u ON u.id = i.author_id
          WHERE {VISIBLE_REPO_SQL}
            AND i.kind = ? AND (? = 'all' OR i.state = ?)"
    );
    let mut args = vec![
        viewer_id, viewer_admin,
        util::js_text(kind), util::js_text(state_filter), util::js_text(state_filter),
    ];
    filters.append(&mut sql, &mut args, &query);
    sql.push_str(" ORDER BY i.updated_at DESC, i.id DESC LIMIT ? OFFSET ?");
    args.extend([
        util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
    ]);
    let mut rows: Vec<GlobalRow> = db::all(&database, &sql, &args).await?;
    let pagination = request_page.finish(&req, &mut rows, None)?;
    let issues = rows
        .into_iter()
        .map(|row| GlobalIssueRow {
            repo_full: format!("{}/{}", row.repo_owner, row.repo_name),
            owner: row.repo_owner,
            repo: row.repo_name,
            number: row.number,
            title: row.title,
            author: row.author_name,
            date: row.created_at,
            is_pull: row.kind == "pull",
            state: row.state,
        })
        .collect();
    let is_pull = kind == "pull";
    let page = GlobalIssuesPage {
        chrome: Chrome::new(&state),
        title: page_title(&[state.locale.t(if is_pull { CommonText::PullRequests } else { CommonText::Issues })]),
        heading: state.locale.t(if is_pull { WebText::GlobalPulls } else { WebText::GlobalIssues }),
        issues,
        state: state_filter.into(),
        query,
        filters,
        open_url: query_url(&req, &[("state", Some("open")), ("page", None)], None)?,
        closed_url: query_url(&req, &[("state", Some("closed")), ("page", None)], None)?,
        all_url: query_url(&req, &[("state", Some("all")), ("page", None)], None)?,
        pagination,
    };
    render(&page)
}

