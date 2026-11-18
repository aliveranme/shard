use super::common::{can_admin, forbidden, not_found, page_title, visible_repo};
use crate::artifacts::Artifacts;
use crate::ctx::AppState;
use crate::db;
use crate::i18n::{CommonText, DiscussionText, Locale};
use crate::markdown;
use crate::models::{Issue, Pull, Repo, Ts};
use crate::pagination::{query_url, PageRequest, PAGE_SIZE};
use crate::templates::{
    render, ChangedRow, Chrome, DiscussionRow, IssueNewPage, IssuePage, IssueRow, IssuesPage, PullView,
};
use crate::util;
use worker::*;

pub async fn list_issues(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    list(req, ctx, "issue").await
}

pub async fn list_pulls(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    list(req, ctx, "pull").await
}

async fn list(req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let is_owner = can_admin(&state, &repo);
    let state_filter = match util::query_param(&req, "state").as_deref() {
        Some("closed") => "closed",
        Some("all") => "all",
        _ => "open",
    };
    let query = util::query_param(&req, "q").unwrap_or_default();
    let request_page = PageRequest::new(&req, "page");
    let database = state.db()?;

    #[derive(serde::Deserialize)]
    struct Count {
        n: i64,
    }
    let open_count = db::first::<Count>(
        &database,
        "SELECT COUNT(*) AS n FROM issues WHERE repo_id = ? AND kind = ? AND state = 'open'",
        &[util::js_int(repo.id), util::js_text(kind)],
    )
    .await?
    .map(|c| c.n)
    .unwrap_or(0);
    let closed_count = db::first::<Count>(
        &database,
        "SELECT COUNT(*) AS n FROM issues WHERE repo_id = ? AND kind = ? AND state = 'closed'",
        &[util::js_int(repo.id), util::js_text(kind)],
    )
    .await?
    .map(|c| c.n)
    .unwrap_or(0);

    let like = format!("%{}%", query.trim());
    let mut issues: Vec<Issue> = db::all(
        &database,
        "SELECT i.id, i.repo_id, i.number, i.kind, i.title, i.state, i.body,
                i.author_id, u.username AS author_name, i.created_at, i.updated_at, i.closed_at,
                (SELECT COUNT(*) FROM comments c WHERE c.issue_id = i.id) AS comment_count
           FROM issues i LEFT JOIN users u ON u.id = i.author_id
          WHERE i.repo_id = ? AND i.kind = ? AND (? = 'all' OR i.state = ?)
                AND (? = '' OR i.title LIKE ?)
          ORDER BY i.updated_at DESC, i.id DESC LIMIT ? OFFSET ?",
        &[
            util::js_int(repo.id), util::js_text(kind),
            util::js_text(state_filter), util::js_text(state_filter),
            util::js_text(query.trim()), util::js_text(&like),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut issues, None)?;
    let heading = state.locale.t(if kind == "pull" {
        CommonText::PullRequests
    } else {
        CommonText::Issues
    });

    let page = IssuesPage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), heading]),
        issues: issues
            .into_iter()
            .map(|issue| IssueRow {
                is_pull: issue.is_pull(),
                number: issue.number,
                title: issue.title,
                author: issue.author_name,
                date: issue.created_at,
                comments: issue.comment_count,
                state: issue.state,
            })
            .collect(),
        repo,
        tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
        state: state_filter.into(),
        query,
        open_count,
        closed_count,
        is_pull: kind == "pull",
        is_owner,
        open_url: query_url(&req, &[("state", Some("open")), ("page", None)], None)?,
        closed_url: query_url(&req, &[("state", Some("closed")), ("page", None)], None)?,
        all_url: query_url(&req, &[("state", Some("all")), ("page", None)], None)?,
        pagination,
    };
    render(&page)
}

pub async fn new_issue_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    new_form(req, ctx, "issue").await
}

pub async fn new_pull_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    new_form(req, ctx, "pull").await
}

async fn new_form(_req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    if state.user.is_none() {
        return util::redirect(&format!(
            "/user/login?next=/{}/{}/{}",
            repo.owner_name,
            repo.name,
            if kind == "pull" { "pulls/new" } else { "issues/new" }
        ));
    }
    let is_owner = can_admin(&state, &repo);
    let page = IssueNewPage {
        chrome: Chrome::new(&state),
        title: new_title(state.locale, &repo, kind),
        tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
        is_pull: kind == "pull",
        error: None,
        title_value: String::new(),
        body_value: String::new(),
        base_value: repo.default_branch.clone(),
        head_value: String::new(),
        branches: vec![repo.default_branch.clone()],
        repo,
        is_owner,
    };
    render(&page)
}

pub async fn create_issue(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    create(req, ctx, "issue").await
}

pub async fn create_pull(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    create(req, ctx, "pull").await
}

async fn create(_req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let form = state.form()?;
    let title = util::form_value(form, "title").unwrap_or_default();
    let body = util::form_value(form, "body").unwrap_or_default();
    let base_branch = util::form_value(form, "base_branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let head_branch = util::form_value(form, "head_branch").unwrap_or_default();

    let is_owner = can_admin(&state, &repo);
    let locale = state.locale;
    let invalid = |message: String| -> Result<Response> {
        let page = IssueNewPage {
            chrome: Chrome::new(&state),
            title: new_title(locale, &repo, kind),
            tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
            is_pull: kind == "pull",
            error: Some(message),
            title_value: title.clone(),
            body_value: body.clone(),
            base_value: base_branch.clone(),
            head_value: head_branch.clone(),
            branches: vec![repo.default_branch.clone()],
            repo: repo.clone(),
            is_owner,
        };
        render(&page)
    };

    if title.trim().is_empty() {
        return invalid(locale.t(DiscussionText::TitleRequired).into());
    }
    if kind == "pull" {
        if head_branch.trim().is_empty() {
            return invalid(locale.t(DiscussionText::ChooseHeadBranch).into());
        }
        if head_branch == base_branch {
            return invalid(locale.t(DiscussionText::BranchesMustDiffer).into());
        }
    }

    let mut head_commit: Option<String> = None;
    let mut base_commit: Option<String> = None;
    if kind == "pull" {
        let artifacts = match Artifacts::from_env(&state.env) {
            Ok(artifacts) => artifacts,
            Err(error) => {
                console_log!("pull request setup: could not configure Git storage: {error}");
                return invalid(locale.t(DiscussionText::GitUnavailable).into());
            }
        };
        let handle = match artifacts.get(&repo.artifacts_repo).await {
            Ok(handle) => handle,
            Err(error) => {
                console_log!("pull request setup: could not open Git repository: {error}");
                return invalid(locale.t(DiscussionText::GitUnavailable).into());
            }
        };
        head_commit = match handle.head_commit(&head_branch).await {
            Ok(commit) => commit.map(|c| c.hash),
            Err(error) => {
                console_log!("pull request setup: could not read the head branch: {error}");
                return invalid(locale.t(DiscussionText::ReadHeadBranchFailed).into());
            }
        };
        base_commit = match handle.head_commit(&base_branch).await {
            Ok(commit) => commit.map(|c| c.hash),
            Err(error) => {
                console_log!("pull request setup: could not read the base branch: {error}");
                return invalid(locale.t(DiscussionText::ReadBaseBranchFailed).into());
            }
        };
        if head_commit.is_none() {
            return invalid(locale.branch_has_no_commits(&head_branch).to_string());
        }
        if base_commit.is_none() {
            return invalid(locale.branch_has_no_commits(&base_branch).to_string());
        }
    }

    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct Number {
        number: i64,
    }
    let number: Option<Number> = db::first(
        &database,
        "UPDATE repos SET next_issue_number = next_issue_number + 1 WHERE id = ? RETURNING next_issue_number - 1 AS number",
        &[util::js_int(repo.id)],
    )
    .await?;
    let Some(Number { number }) = number else {
        return invalid(locale.t(DiscussionText::AllocateNumberFailed).into());
    };

    db::run(
        &database,
        "INSERT INTO issues (repo_id, number, kind, title, body, state, author_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'open', ?, ?, ?)",
        &[
            util::js_int(repo.id),
            util::js_int(number),
            util::js_text(kind),
            util::js_text(title.trim()),
            util::js_text(&body),
            util::js_int(user.id),
            util::js_int(util::now()),
            util::js_int(util::now()),
        ],
    )
    .await?;

    if kind == "pull" {
        db::run(
            &database,
            "INSERT INTO pulls (issue_id, head_branch, base_branch, head_commit, base_commit, merged)
             SELECT id, ?, ?, ?, ?, 0 FROM issues WHERE repo_id = ? AND number = ?",
            &[
                util::js_text(&head_branch),
                util::js_text(&base_branch),
                head_commit.map(|v| util::js_text(&v)).unwrap_or_else(util::js_null),
                base_commit.map(|v| util::js_text(&v)).unwrap_or_else(util::js_null),
                util::js_int(repo.id),
                util::js_int(number),
            ],
        )
        .await?;
    }

    let path = if kind == "pull" { "pulls" } else { "issues" };
    util::redirect(&format!("/{}/{}/{}/{}", repo.owner_name, repo.name, path, number))
}

pub async fn detail_issue(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    detail(req, ctx, "issue").await
}

pub async fn detail_pull(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    detail(req, ctx, "pull").await
}

async fn detail(req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT i.id, i.repo_id, i.number, i.kind, i.title, i.state, i.body,
                i.author_id, u.username AS author_name, i.created_at, i.updated_at, i.closed_at,
                (SELECT COUNT(*) FROM comments c WHERE c.issue_id = i.id) AS comment_count
           FROM issues i LEFT JOIN users u ON u.id = i.author_id
          WHERE i.repo_id = ? AND i.number = ? AND i.kind = ?",
        &[
            util::js_int(repo.id),
            util::js_int(number),
            util::js_text(kind),
        ],
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(&state, state.locale.t(CommonText::NotFound));
    };


    let request_page = PageRequest::new(&req, "comments_page");
    let mut entries: Vec<DiscussionEntry> = db::all(
        &database,
        "SELECT c.id AS id, c.body, 'comment' AS kind, c.created_at AS created_at, u.username AS author_name, 0 AS item_order
           FROM comments c LEFT JOIN users u ON u.id = c.author_id WHERE c.issue_id = ?
         UNION ALL
         SELECT e.id, '' AS body, e.event AS kind, e.created_at, u.username AS author_name, 1 AS item_order
           FROM issue_events e LEFT JOIN users u ON u.id = e.actor_id WHERE e.issue_id = ?
         ORDER BY created_at ASC, item_order ASC, id ASC LIMIT ? OFFSET ?",
        &[
            util::js_int(issue.id), util::js_int(issue.id),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
        ],
    )
    .await?;
    let locale = state.locale;
    let pagination = request_page.finish(&req, &mut entries, Some("discussion"))?;
    let pull: Option<Pull> = if kind == "pull" {
        db::first(
            &database,
            "SELECT issue_id, head_branch, base_branch, head_commit, base_commit, merged, merged_at, merge_commit
               FROM pulls WHERE issue_id = ?",
            &[util::js_int(issue.id)],
        )
        .await?
    } else {
        None
    };

    let mut changed = Vec::new();
    let mut diff_error = None;
    if let Some(pull) = &pull {
        let files = async {
            Artifacts::from_env(&state.env)?
                .get(&repo.artifacts_repo)
                .await?
                .changed_files(&pull.base_branch, &pull.head_branch)
                .await
        }
        .await;
        match files {
            Ok(files) => changed = files
                .into_iter()
                .map(|file| ChangedRow { path: file.path, status: file.status })
                .collect(),
            Err(error) => {
                console_log!("pull request diff: could not compare branches: {error}");
                diff_error = Some(locale.t(DiscussionText::CompareBranchesFailed));
            }
        }
    }

    let can_edit = match &state.user {
        Some(user) => {
            user.id == issue.author_id.unwrap_or(-1)
                || can_admin(&state, &repo)
        }
        None => false,
    };
    let is_owner = can_admin(&state, &repo);
    let page = IssuePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), &format!("#{number}")]),
        author_initial: crate::templates::initial(
            issue.author_name.as_deref().unwrap_or(locale.t(CommonText::Ghost)),
        ),
        body_html: markdown::render(&issue.body),
        discussion: entries.into_iter().map(|entry| entry.into_row(locale)).collect(),
        pagination,
        pull: pull.as_ref().map(PullView::from_pull),
        changed,
        diff_error,
        can_edit,
        is_owner,
        repo,
        tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
        issue,
    };
    render(&page)
}

pub async fn comment(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let kind = route_kind(&req)?;
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT id, repo_id, number, kind, title, body, state, author_id, NULL AS author_name,
                created_at, updated_at, closed_at, 0 AS comment_count
           FROM issues WHERE repo_id = ? AND number = ? AND kind = ?",
        &[util::js_int(repo.id), util::js_int(number), util::js_text(kind)],
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(&state, state.locale.t(CommonText::NotFound));
    };
    let form = state.form()?;
    let body = util::form_value(form, "body").unwrap_or_default();
    if body.trim().is_empty() {
        let request_page = PageRequest::new(&req, "comments_page");
        return util::redirect(&format!(
            "/{}/{}/{}/{}?comments_page={}#discussion",
            repo.owner_name, repo.name, section(kind), number, request_page.number
        ));
    }
    let now = util::now();
    let insert = database.prepare(
        "INSERT INTO comments (issue_id, author_id, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?)"
    ).bind(&[
        util::js_int(issue.id), util::js_int(user.id), util::js_text(&body),
        util::js_int(now), util::js_int(now),
    ])?;
    let update = database.prepare("UPDATE issues SET updated_at = ? WHERE id = ?")
        .bind(&[util::js_int(now), util::js_int(issue.id)])?;
    database.batch(vec![insert, update]).await?;
    redirect_to_discussion(&database, &repo, &issue).await
}

pub async fn set_state(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let kind = route_kind(&req)?;
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT id, repo_id, number, kind, title, body, state, author_id, NULL AS author_name,
                created_at, updated_at, closed_at, 0 AS comment_count
           FROM issues WHERE repo_id = ? AND number = ? AND kind = ?",
        &[util::js_int(repo.id), util::js_int(number), util::js_text(kind)],
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(&state, state.locale.t(CommonText::NotFound));
    };
    if Some(user.id) != issue.author_id && !can_admin(&state, &repo) {
        return forbidden(&state, state.locale.t(DiscussionText::StateChangeForbidden));
    }
    let form = state.form()?;
    let desired = match util::form_value(form, "state").as_deref() {
        Some("closed") => "closed",
        Some("open") => "open",
        _ => return crate::templates::error_response(
            Chrome::new(&state), 400, state.locale.t(DiscussionText::InvalidState),
        ),
    };
    let now = util::now();
    // D1 executes the batch as one transaction. Test the live state inside it,
    // so concurrent or repeated submissions cannot create duplicate events.
    let event = database.prepare(
        "INSERT INTO issue_events (issue_id, actor_id, event, created_at)
         SELECT id, ?, ?, ? FROM issues WHERE id = ? AND state <> ?"
    ).bind(&[
        util::js_int(user.id), util::js_text(if desired == "closed" { "closed" } else { "reopened" }),
        util::js_int(now), util::js_int(issue.id), util::js_text(desired),
    ])?;
    let update = database.prepare(
        "UPDATE issues SET state = ?, closed_at = ?, updated_at = ? WHERE id = ? AND state <> ?"
    ).bind(&[
        util::js_text(desired),
        if desired == "closed" { util::js_int(now) } else { util::js_null() },
        util::js_int(now), util::js_int(issue.id), util::js_text(desired),
    ])?;
    database.batch(vec![event, update]).await?;
    redirect_to_discussion(&database, &repo, &issue).await
}


#[derive(serde::Deserialize)]
struct DiscussionEntry {
    id: i64,
    body: String,
    kind: String,
    author_name: Option<String>,
    created_at: Ts,
}

impl DiscussionEntry {
    fn into_row(self, locale: Locale) -> DiscussionRow {
        let is_comment = self.kind == "comment";
        DiscussionRow {
            id: self.id,
            initial: if is_comment {
                crate::templates::initial(self.author_name.as_deref().unwrap_or(locale.t(CommonText::Ghost)))
            } else {
                String::new()
            },
            author: self.author_name,
            date: self.created_at,
            html: if is_comment { markdown::render(&self.body) } else { String::new() },
            action: self.kind,
            is_comment,
        }
    }
}

fn new_title(locale: Locale, repo: &Repo, kind: &str) -> String {
    let label = locale.t(if kind == "pull" {
        DiscussionText::NewPullRequest
    } else {
        DiscussionText::NewIssue
    });
    page_title(&[&repo.full_name(), label])
}

fn section(kind: &str) -> &'static str {
    if kind == "pull" { "pulls" } else { "issues" }
}

fn route_kind(req: &Request) -> Result<&'static str> {
    match req.url()?.path().split('/').nth(3) {
        Some("issues") => Ok("issue"),
        Some("pulls") => Ok("pull"),
        _ => Err(Error::RustError("Invalid discussion route".into())),
    }
}

async fn redirect_to_discussion(database: &worker::d1::D1Database, repo: &Repo, issue: &Issue) -> Result<Response> {
    #[derive(serde::Deserialize)]
    struct Count { n: i64 }
    let count = db::first::<Count>(
        database,
        "SELECT (SELECT COUNT(*) FROM comments WHERE issue_id = ?)
              + (SELECT COUNT(*) FROM issue_events WHERE issue_id = ?) AS n",
        &[util::js_int(issue.id), util::js_int(issue.id)],
    ).await?.map(|count| count.n).unwrap_or(0);
    let page = (count.saturating_sub(1).max(0) / PAGE_SIZE as i64 + 1)
        .min(i64::from(crate::pagination::MAX_PAGE));
    util::redirect(&format!(
        "/{}/{}/{}/{}?comments_page={}#discussion",
        repo.owner_name, repo.name, section(&issue.kind), issue.number, page
    ))
}
