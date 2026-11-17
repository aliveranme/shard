use super::common::{can_admin, forbidden, not_found, page_title, visible_repo};
use crate::artifacts::Artifacts;
use crate::ctx::AppState;
use crate::db;
use crate::markdown;
use crate::models::{Comment, Issue, Pull};
use crate::templates::{
    render, ChangedRow, Chrome, CommentRow, IssueNewPage, IssuePage, IssueRow, IssuesPage, PullView,
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
        return not_found(&state, "Repository not found.");
    };
    let is_owner = can_admin(&state, &repo);
    let state_filter = match util::query_param(&req, "state").as_deref() {
        Some("closed") => "closed",
        Some("all") => "all",
        _ => "open",
    };
    let query = util::query_param(&req, "q").unwrap_or_default();
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
    let issues: Vec<Issue> = db::all(
        &database,
        "SELECT i.id, i.repo_id, i.number, i.kind, i.title, i.state, i.body,
                i.author_id, u.username AS author_name, i.created_at, i.updated_at, i.closed_at,
                (SELECT COUNT(*) FROM comments c WHERE c.issue_id = i.id) AS comment_count
           FROM issues i LEFT JOIN users u ON u.id = i.author_id
          WHERE i.repo_id = ? AND i.kind = ? AND (? = 'all' OR i.state = ?)
                AND (? = '' OR i.title LIKE ?)
          ORDER BY i.updated_at DESC LIMIT 100",
        &[
            util::js_int(repo.id),
            util::js_text(kind),
            util::js_text(state_filter),
            util::js_text(state_filter),
            util::js_text(query.trim()),
            util::js_text(&like),
        ],
    )
    .await?;

    let page = IssuesPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), if kind == "pull" { "Pull requests" } else { "Issues" }]),
        issues: issues
            .iter()
            .map(|issue| IssueRow {
                number: issue.number,
                title: issue.title.clone(),
                author: issue.author().to_string(),
                date: issue.created_at,
                comments: issue.comment_count,
                is_pull: issue.is_pull(),
                state: issue.state.clone(),
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
        return not_found(&state, "Repository not found.");
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
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), if kind == "pull" { "New pull request" } else { "New issue" }]),
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

async fn create(mut req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let form = req.form_data().await?;
    let title = util::form_value(&form, "title").unwrap_or_default();
    let body = util::form_value(&form, "body").unwrap_or_default();
    let base_branch = util::form_value(&form, "base_branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let head_branch = util::form_value(&form, "head_branch").unwrap_or_default();

    let is_owner = can_admin(&state, &repo);
    let invalid = |message: &str| -> Result<Response> {
        let page = IssueNewPage {
            chrome: Chrome::new(&state.user),
            title: page_title(&[&repo.full_name()]),
            tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
            is_pull: kind == "pull",
            error: Some(message.to_string()),
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
        return invalid("A title is required.");
    }
    if kind == "pull" {
        if head_branch.trim().is_empty() {
            return invalid("Choose the branch that contains your changes.");
        }
        if head_branch == base_branch {
            return invalid("Head and base branches must differ.");
        }
    }

    let mut head_commit: Option<String> = None;
    let mut base_commit: Option<String> = None;
    if kind == "pull" {
        let artifacts = match Artifacts::from_env(&state.env) {
            Ok(artifacts) => artifacts,
            Err(e) => return invalid(&format!("Git storage is unavailable: {e}")),
        };
        let handle = match artifacts.get(&repo.artifacts_repo).await {
            Ok(handle) => handle,
            Err(e) => return invalid(&format!("Git storage is unavailable: {e}")),
        };
        head_commit = match handle.head_commit(&head_branch).await {
            Ok(commit) => commit.map(|c| c.hash),
            Err(e) => return invalid(&format!("Could not read the head branch: {e}")),
        };
        base_commit = match handle.head_commit(&base_branch).await {
            Ok(commit) => commit.map(|c| c.hash),
            Err(e) => return invalid(&format!("Could not read the base branch: {e}")),
        };
        if head_commit.is_none() {
            return invalid(&format!("Branch `{head_branch}` has no commits in this repository."));
        }
        if base_commit.is_none() {
            return invalid(&format!("Branch `{base_branch}` has no commits in this repository."));
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
        return invalid("Could not allocate an issue number.");
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

async fn detail(_req: Request, ctx: RouteContext<AppState>, kind: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
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
        return not_found(&state, "Not found.");
    };

    let comments: Vec<Comment> = db::all(
        &database,
        "SELECT c.id, c.issue_id, c.body, c.created_at, u.username AS author_name
           FROM comments c LEFT JOIN users u ON u.id = c.author_id
          WHERE c.issue_id = ? ORDER BY c.created_at ASC, c.id ASC",
        &[util::js_int(issue.id)],
    )
    .await?;

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
        match Artifacts::from_env(&state.env) {
            Ok(artifacts) => match artifacts.get(&repo.artifacts_repo).await {
                Ok(handle) => match handle
                    .changed_files(&pull.base_branch, &pull.head_branch)
                    .await
                {
                    Ok(files) => changed = files
                        .into_iter()
                        .map(|f| ChangedRow {
                            path: f.path,
                            status: f.status.into(),
                        })
                        .collect(),
                    Err(e) => diff_error = Some(format!("Could not compare branches: {e}")),
                },
                Err(e) => diff_error = Some(format!("Could not compare branches: {e}")),
            },
            Err(e) => diff_error = Some(format!("Could not compare branches: {e}")),
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
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), &format!("#{number}")]),
        body_html: markdown::render(&issue.body),
        comments: comments.iter().map(CommentRow::from_comment).collect(),
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

pub async fn comment(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let form = req.form_data().await?;
    let body = util::form_value(&form, "body").unwrap_or_default();
    if body.trim().is_empty() {
        return util::redirect(&format!(
            "/{}/{}/issues/{}",
            repo.owner_name, repo.name, number
        ));
    }
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT id, repo_id, number, kind, title, body, state, author_id, NULL AS author_name,
                created_at, updated_at, closed_at, 0 AS comment_count
           FROM issues WHERE repo_id = ? AND number = ?",
        &[util::js_int(repo.id), util::js_int(number)],
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(&state, "Not found.");
    };
    db::run(
        &database,
        "INSERT INTO comments (issue_id, author_id, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        &[
            util::js_int(issue.id),
            util::js_int(user.id),
            util::js_text(&body),
            util::js_int(util::now()),
            util::js_int(util::now()),
        ],
    )
    .await?;
    db::run(
        &database,
        "UPDATE issues SET updated_at = ? WHERE id = ?",
        &[util::js_int(util::now()), util::js_int(issue.id)],
    )
    .await?;
    let section = if issue.kind == "pull" { "pulls" } else { "issues" };
    util::redirect(&format!(
        "/{}/{}/{}/{}#comment-latest",
        repo.owner_name, repo.name, section, number
    ))
}

pub async fn set_state(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx.param("number").and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    if state.user.is_none() {
        return util::redirect("/user/login");
    }
    let form = req.form_data().await?;
    let desired = match util::form_value(&form, "state").as_deref() {
        Some("closed") => "closed",
        _ => "open",
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT id, repo_id, number, kind, title, body, state, author_id, NULL AS author_name,
                created_at, updated_at, closed_at, 0 AS comment_count
           FROM issues WHERE repo_id = ? AND number = ?",
        &[util::js_int(repo.id), util::js_int(number)],
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(&state, "Not found.");
    };
    if state.user_id() != issue.author_id && !can_admin(&state, &repo) {
        return forbidden(&state, "Only the author or a repository owner can change this state.");
    }
    db::run(
        &database,
        "UPDATE issues SET state = ?, closed_at = ?, updated_at = ? WHERE id = ?",
        &[
            util::js_text(desired),
            if desired == "closed" {
                util::js_int(util::now())
            } else {
                util::js_null()
            },
            util::js_int(util::now()),
            util::js_int(issue.id),
        ],
    )
    .await?;
    let section = if issue.kind == "pull" { "pulls" } else { "issues" };
    util::redirect(&format!(
        "/{}/{}/{}/{}",
        repo.owner_name, repo.name, section, number
    ))
}

