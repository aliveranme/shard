use super::common::{can_admin, forbidden, not_found, page_title, visible_repo};
use super::common::{can_write, visible_repo_by_id};
use crate::ctx::AppState;
use crate::db;
use crate::git_storage::is_object_id;
use crate::i18n::{CommonText, DiscussionText, Locale, RepoFeatureText};
use crate::markdown;
use crate::models::{Issue, PullSnapshot, Repo, Ts};
use crate::pagination::{query_url, PageRequest, PAGE_SIZE};
use crate::pull_base;
use crate::pull_diff::{self, Comparison, FileDiff};
use crate::repo_features;
use crate::templates::{
    render, Chrome, DiscussionRow, FilePreview, IssueNewPage, IssuePage, IssueRow, IssuesPage,
    LabelChoice, PullView, ReviewContext, ReviewLine,
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

    let filters = crate::issue_filters::IssueFilters::from_request(&req);
    let (open_count, closed_count) = filters
        .repo_counts(&database, repo.id, kind, &query)
        .await?;
    let mut sql = String::from(
        "SELECT i.id, i.repo_id, i.number, i.kind, i.title, i.state, '' AS body,
                i.author_id, u.username AS author_name, i.created_at, i.updated_at, i.closed_at,
                (SELECT COUNT(*) FROM comments c WHERE c.issue_id = i.id) AS comment_count,
                (SELECT group_concat(l.name, char(31)) FROM issue_labels il
                   JOIN repo_labels l ON l.repo_id = il.repo_id AND l.id = il.label_id
                  WHERE il.issue_id = i.id) AS label_names
           FROM issues i LEFT JOIN users u ON u.id = i.author_id
          WHERE i.repo_id = ? AND i.kind = ?",
    );
    let mut args = vec![util::js_int(repo.id), util::js_text(kind)];
    if state_filter != "all" {
        sql.push_str(" AND i.state = ?");
        args.push(util::js_text(state_filter));
    }
    filters.append(&mut sql, &mut args, &query);
    sql.push_str(" ORDER BY i.updated_at DESC, i.id DESC LIMIT ? OFFSET ?");
    args.extend([
        util::js_int((PAGE_SIZE + 1) as i64),
        util::js_int(request_page.offset()),
    ]);
    let mut issues: Vec<Issue> = db::all(&database, &sql, &args).await?;
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
                labels: issue
                    .label_names
                    .as_deref()
                    .unwrap_or_default()
                    .split('\u{1f}')
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .collect(),
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
        filters,
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
            if kind == "pull" {
                "pulls/new"
            } else {
                "issues/new"
            }
        ));
    }
    let is_owner = can_admin(&state, &repo);
    let can_classify = kind == "issue" && can_write(&state, &repo).await?;
    let labels = if can_classify {
        db::all::<crate::issue_metadata::Named>(
            &state.db()?,
            "SELECT id, name FROM repo_labels WHERE repo_id = ? ORDER BY name, id",
            &[util::js_int(repo.id)],
        )
        .await?
        .into_iter()
        .map(|label| LabelChoice {
            id: label.id,
            name: label.name,
            selected: false,
        })
        .collect()
    } else {
        Vec::new()
    };
    let page = IssueNewPage {
        chrome: Chrome::new(&state),
        title: new_title(state.locale, &repo, kind),
        tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
        is_pull: kind == "pull",
        error: None,
        title_value: String::new(),
        body_value: String::new(),
        base_value: repo.default_branch.clone(),
        source_value: String::new(),
        head_value: String::new(),
        branches: vec![repo.default_branch.clone()],
        issue_templates: if kind == "issue" {
            repo_features::issue_template_choices(&state, repo.id).await?
        } else {
            Vec::new()
        },
        selected_issue_template: None,
        can_classify,
        labels,
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
    let mut title = util::form_value(form, "title").unwrap_or_default();
    let mut body = util::form_value(form, "body").unwrap_or_default();
    let base_branch =
        util::form_value(form, "base_branch").unwrap_or_else(|| repo.default_branch.clone());
    let head_branch = util::form_value(form, "head_branch").unwrap_or_default();
    let source_value = util::form_value(form, "source_repo").unwrap_or_default();
    let source_path = source_value.trim();

    let apply =
        kind == "issue" && util::form_value(form, "apply_issue_template").as_deref() == Some("1");
    let template_value = util::form_value(form, "issue_template").filter(|value| !value.is_empty());
    let selected_issue_template = template_value
        .as_ref()
        .and_then(|value| value.parse::<i64>().ok());
    let mut template_missing = false;
    if apply && template_value.is_some() {
        let defaults = match selected_issue_template {
            Some(id) if (1..=9_007_199_254_740_991).contains(&id) => {
                repo_features::issue_template_defaults(&state, repo.id, id).await?
            }
            _ => None,
        };
        match defaults {
            Some(defaults) => {
                if title.trim().is_empty() {
                    title = defaults.title;
                }
                if body.trim().is_empty() {
                    body = defaults.body;
                }
            }
            None => template_missing = true,
        }
    }

    let is_owner = can_admin(&state, &repo);
    let can_classify = kind == "issue" && can_write(&state, &repo).await?;
    let mut selected_labels: Vec<i64> = if can_classify {
        form.get_all("label_id")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| match entry {
                FormEntry::Field(value) => value.parse::<i64>().ok().filter(|id| *id > 0),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    selected_labels.sort_unstable();
    selected_labels.dedup();
    let labels: Vec<LabelChoice> = if can_classify {
        db::all::<crate::issue_metadata::Named>(
            &state.db()?,
            "SELECT id, name FROM repo_labels WHERE repo_id = ? ORDER BY name, id",
            &[util::js_int(repo.id)],
        )
        .await?
        .into_iter()
        .map(|label| LabelChoice {
            selected: selected_labels.binary_search(&label.id).is_ok(),
            id: label.id,
            name: label.name,
        })
        .collect()
    } else {
        Vec::new()
    };
    let locale = state.locale;
    let render_form = async |error: Option<String>| -> Result<Response> {
        let page = IssueNewPage {
            chrome: Chrome::new(&state),
            title: new_title(locale, &repo, kind),
            tab: if kind == "pull" { "pulls" } else { "issues" }.into(),
            is_pull: kind == "pull",
            error,
            title_value: title.clone(),
            body_value: body.clone(),
            base_value: base_branch.clone(),
            source_value: source_value.clone(),
            head_value: head_branch.clone(),
            branches: if source_path.is_empty() {
                vec![repo.default_branch.clone()]
            } else {
                Vec::new()
            },
            issue_templates: if kind == "issue" {
                repo_features::issue_template_choices(&state, repo.id).await?
            } else {
                Vec::new()
            },
            selected_issue_template,
            can_classify,
            labels: labels.clone(),
            repo: repo.clone(),
            is_owner,
        };
        render(&page)
    };
    let invalid = async |message: String| render_form(Some(message)).await;
    if apply {
        return render_form(
            template_missing.then(|| locale.t(RepoFeatureText::MissingIssueTemplate).into()),
        )
        .await;
    }

    const MAX_TITLE_CHARS: usize = 300;
    const MAX_BODY_BYTES: usize = 256 * 1024;
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return invalid(locale.t(DiscussionText::TitleRequired).into()).await;
    }
    if trimmed_title.chars().count() > MAX_TITLE_CHARS
        || trimmed_title.chars().any(char::is_control)
    {
        return invalid(locale.t(DiscussionText::TitleTooLong).into()).await;
    }
    if body.len() > MAX_BODY_BYTES {
        return invalid(locale.t(DiscussionText::BodyTooLong).into()).await;
    }
    if kind == "pull" && head_branch.trim().is_empty() {
        return invalid(locale.t(DiscussionText::ChooseHeadBranch).into()).await;
    }

    let snapshot = if kind == "pull" {
        let source_repo;
        let source = if source_path.is_empty() {
            &repo
        } else {
            let Some((source_owner, source_name)) = source_path.split_once('/') else {
                return invalid(locale.t(DiscussionText::SourceRepositoryUnavailable).into()).await;
            };
            source_repo = visible_repo(&state, source_owner, source_name).await?;
            let Some(source) = source_repo.as_ref() else {
                return invalid(locale.t(DiscussionText::SourceRepositoryUnavailable).into()).await;
            };
            source
        };
        if repo.id == source.id && head_branch == base_branch {
            return invalid(locale.t(DiscussionText::BranchesMustDiffer).into()).await;
        }
        if source.is_private() && !repo.is_private() {
            return invalid(locale.t(DiscussionText::PrivateSourcePublicTarget).into()).await;
        }
        let handle = match pull_base::Repositories::open(&state.env, &repo, source).await {
            Ok(handle) => handle,
            Err(error) => {
                console_log!("pull request setup: could not open Git repository: {error}");
                return invalid(locale.t(DiscussionText::GitUnavailable).into()).await;
            }
        };
        let (base, head) = match pull_base::branch_tips(&handle, &base_branch, &head_branch).await {
            Ok(tips) => tips,
            Err(message) => return invalid(locale.t(message).into()).await,
        };
        let merge_base = match pull_base::find(&handle, &base, &head).await {
            Ok(merge_base) => merge_base,
            Err(error) => return invalid(locale.t(error.message()).into()).await,
        };
        Some((source.id, base, head, merge_base))
    } else {
        None
    };

    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct Number {
        number: i64,
    }
    let [viewer, admin] = crate::handlers::common::viewer_params(&state);
    let now = util::js_int(util::now());
    let mut statements = vec![
        database.prepare(format!(
            "UPDATE repos SET next_issue_number = next_issue_number + 1
              WHERE id = ?3 AND EXISTS (
                SELECT 1 FROM repos r WHERE r.id = repos.id AND {}
              ) RETURNING next_issue_number - 1 AS number",
            crate::handlers::common::VISIBLE_REPO_SQL,
        )).bind(&[viewer.clone(), admin.clone(), util::js_int(repo.id)])?,
        database.prepare(format!(
            "INSERT INTO issues (repo_id, number, kind, title, body, state, author_id, created_at, updated_at)
             SELECT r.id, r.next_issue_number - 1, ?4, ?5, ?6, 'open', ?7, ?8, ?9
               FROM repos r WHERE r.id = ?3 AND {}",
            crate::handlers::common::VISIBLE_REPO_SQL,
        )).bind(&[
            viewer.clone(), admin.clone(), util::js_int(repo.id),
            util::js_text(kind), util::js_text(trimmed_title), util::js_text(&body),
            util::js_int(user.id), now.clone(), now.clone(),
        ])?,
    ];
    if let Some((source_id, base, head, merge_base)) = snapshot {
        statements.push(database.prepare(format!(
            "INSERT INTO pulls (issue_id, source_repo_id, head_branch, base_branch, head_commit, base_commit, merge_base)
             SELECT i.id, ?4, ?5, ?6, ?7, ?8, ?9 FROM issues i JOIN repos r ON r.id = i.repo_id
              WHERE r.id = ?3 AND i.number = r.next_issue_number - 1
                AND i.kind = 'pull' AND i.author_id = ?10 AND i.created_at = ?11
                AND NOT EXISTS (SELECT 1 FROM pulls p WHERE p.issue_id = i.id)
                AND {}",
            crate::handlers::common::VISIBLE_REPO_SQL,
        )).bind(&[
            viewer.clone(), admin.clone(), util::js_int(repo.id),
            util::js_int(source_id), util::js_text(&head_branch), util::js_text(&base_branch),
            util::js_text(&head), util::js_text(&base), util::js_text(&merge_base),
            util::js_int(user.id), now.clone(),
        ])?);
    }
    if !selected_labels.is_empty() {
        let labels = serde_json::to_string(&selected_labels)
            .map_err(|error| Error::RustError(error.to_string()))?;
        statements.push(
            database
                .prepare(
                    "INSERT INTO issue_labels(issue_id, repo_id, label_id)
             SELECT i.id, i.repo_id, l.id FROM issues i
               JOIN repos r ON r.id = i.repo_id
               JOIN repo_labels l ON l.repo_id = i.repo_id
                AND l.id IN (SELECT value FROM json_each(?1))
              WHERE i.repo_id = ?2 AND i.number = r.next_issue_number - 1
                AND i.kind = ?5 AND i.author_id = ?3 AND i.created_at = ?6
                AND (r.owner_id = ?3 OR ?4 = 1 OR EXISTS (
                    SELECT 1 FROM repo_member_roles c
                    WHERE c.repo_id = r.id AND c.user_id = ?3 AND c.role = 'write'
                ))
             ON CONFLICT(issue_id, label_id) DO NOTHING",
                )
                .bind(&[
                    util::js_text(&labels),
                    util::js_int(repo.id),
                    viewer,
                    admin,
                    util::js_text(kind),
                    now,
                ])?,
        );
    }
    let results = database.batch(statements).await?;
    let Some(Number { number }) = results[0].results::<Number>()?.into_iter().next() else {
        return invalid(locale.t(DiscussionText::AllocateNumberFailed).into()).await;
    };

    let path = if kind == "pull" { "pulls" } else { "issues" };
    util::redirect(&format!(
        "/{}/{}/{}/{}",
        repo.owner_name, repo.name, path, number
    ))
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
    let number: i64 = ctx
        .param("number")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT i.id, i.repo_id, i.number, i.kind, i.title, i.state, i.body,
                i.author_id, u.username AS author_name, i.created_at, i.updated_at, i.closed_at,
                av.user_id AS author_avatar_id,
                (SELECT COUNT(*) FROM comments c WHERE c.issue_id = i.id) AS comment_count
           FROM issues i LEFT JOIN users u ON u.id = i.author_id
           LEFT JOIN user_avatars av ON av.user_id = u.id
          WHERE i.repo_id = ? AND i.number = ? AND (?3 = 'issue' OR i.kind = ?3)",
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
    if kind == "issue" && issue.is_pull() {
        let mut location = format!("/{}/{}/pulls/{number}", repo.owner_name, repo.name);
        if let Some(query) = req.url()?.query() {
            location.push('?');
            location.push_str(query);
        }
        return util::redirect(&location);
    }

    let pull = if kind == "pull" {
        crate::pull_review::load_pull(&database, issue.id).await?
    } else {
        None
    };
    let source_repo;
    let source = match &pull {
        Some(pull) if pull.source_repo_id(repo.id) != repo.id => {
            source_repo = visible_repo_by_id(&state, pull.source_repo_id(repo.id)).await?;
            source_repo
                .as_ref()
                .filter(|candidate| !candidate.is_private() || repo.is_private())
        }
        Some(_) => Some(&repo),
        None => None,
    };
    let comparison_available = source.is_some();
    let locale = state.locale;
    let viewed = if let Some(review) = util::query_param(&req, "review").filter(|_| kind == "pull")
    {
        let id = review
            .parse::<i64>()
            .ok()
            .filter(|id| (1..=9_007_199_254_740_991).contains(id));
        let Some(id) = id.filter(|_| comparison_available) else {
            return not_found(&state, locale.t(CommonText::NotFound));
        };
        let snapshot: Option<PullSnapshot> = db::first(
            &database,
            "SELECT head_commit, base_commit, merge_base, comparison_version
               FROM pull_reviews WHERE issue_id = ? AND id = ?",
            &[util::js_int(issue.id), util::js_int(id)],
        )
        .await?;
        let Some(snapshot) = snapshot else {
            return not_found(&state, locale.t(CommonText::NotFound));
        };
        Some((id, snapshot))
    } else {
        None
    };

    let request_page = PageRequest::new(&req, "comments_page");
    let mut entries: Vec<DiscussionEntry> = db::all(
        &database,
        "SELECT c.id AS id, c.body, 'comment' AS kind, c.created_at AS created_at, u.username AS author_name, 0 AS item_order,
                av.user_id AS avatar_id, NULL AS base, NULL AS head, NULL AS comparison_version, NULL AS path, NULL AS side, NULL AS line
           FROM comments c LEFT JOIN users u ON u.id = c.author_id
           LEFT JOIN user_avatars av ON av.user_id = u.id WHERE c.issue_id = ?1
         UNION ALL
         SELECT e.id, '', e.event, e.created_at, u.username, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL
           FROM issue_events e LEFT JOIN users u ON u.id = e.actor_id WHERE e.issue_id = ?1
         UNION ALL
         SELECT p.id, p.body, p.decision, p.created_at, u.username, 2, av.user_id,
                COALESCE(p.merge_base, p.base_commit), p.head_commit, p.comparison_version, p.path, p.side, p.line
           FROM pull_reviews p LEFT JOIN users u ON u.id = p.actor_id
           LEFT JOIN user_avatars av ON av.user_id = u.id WHERE p.issue_id = ?1 AND ?4 = 1
         ORDER BY created_at ASC, item_order ASC, id ASC LIMIT ?2 OFFSET ?3",
        &[
            util::js_int(issue.id),
            util::js_int((PAGE_SIZE + 1) as i64), util::js_int(request_page.offset()),
            util::js_int(i64::from(comparison_available)),
        ],
    )
    .await?;
    let pagination = request_page.finish(&req, &mut entries, Some("discussion"))?;
    let merged = pull.as_ref().is_some_and(|pull| pull.merged != 0);
    let current_open = comparison_available && viewed.is_none() && !merged && issue.is_open();
    let writer =
        comparison_available && viewed.is_none() && !merged && can_write(&state, &repo).await?;
    let author = state
        .user_id()
        .is_some_and(|id| Some(id) == issue.author_id);
    let mut comparison_notice = None;
    let mut can_refresh_comparison = false;

    let mut changed = Vec::new();
    let mut diff_error = None;
    let mut preview = None;
    let mut preview_missing = false;
    if let (Some(pull), Some(source)) = (&pull, source) {
        let selected = util::query_param(&req, "file");
        let snapshot = viewed
            .as_ref()
            .map_or(&pull.snapshot, |(_, snapshot)| snapshot);
        let comparison = async {
            let handle = pull_base::Repositories::open(&state.env, &repo, source).await?;
            if current_open {
                match pull_base::branch_tips(&handle, &pull.base_branch, &pull.head_branch).await {
                    Ok((base, head)) => {
                        let moved = snapshot.base_commit.as_deref() != Some(base.as_str())
                            || snapshot.head_commit.as_deref() != Some(head.as_str());
                        comparison_notice = if moved {
                            Some(locale.t(DiscussionText::BranchesAdvanced))
                        } else if snapshot.merge_base.is_none() {
                            Some(locale.t(DiscussionText::LegacyComparison))
                        } else {
                            None
                        };
                        can_refresh_comparison =
                            (author || writer) && (moved || snapshot.merge_base.is_none());
                    }
                    Err(message) => comparison_notice = Some(locale.t(message)),
                }
            }
            let Some((base, head)) = snapshot.diff_tips() else {
                return Ok(Comparison::MissingSnapshot);
            };
            let comparison =
                pull_diff::compare(&state.env, &handle.target, handle.source(), base, head).await?;
            if let (Comparison::Complete(files), Some(path)) = (&comparison, selected) {
                if let Ok(index) = files.binary_search_by(|file| file.path.cmp(&path)) {
                    let file = &files[index];
                    let diff = match file.lines(&handle.target, handle.source()).await {
                        Ok(diff) => diff,
                        Err(error) => {
                            console_log!("pull request: Git file comparison failed: {error}");
                            FileDiff::Unavailable
                        }
                    };
                    preview = Some(FilePreview {
                        path,
                        modes: file.modes(),
                        diff,
                    });
                } else {
                    preview_missing = true;
                }
            }
            Ok::<_, Error>(comparison)
        }
        .await;
        match comparison {
            Ok(Comparison::Complete(files)) => changed = files,
            Ok(Comparison::MissingSnapshot) => {
                diff_error = Some(locale.t(DiscussionText::MissingSnapshot))
            }
            Ok(Comparison::Limited) => {
                diff_error = Some(locale.t(DiscussionText::ComparisonLimited))
            }
            Err(error) => {
                console_log!("pull request: saved Git comparison failed: {error}");
                diff_error = Some(locale.t(DiscussionText::CompareBranchesFailed));
            }
        }
    } else if pull.is_some() {
        diff_error = Some(locale.t(DiscussionText::ComparisonUnavailable));
    }

    let can_edit = viewed.is_none() && !merged && (author || can_admin(&state, &repo));
    let has_snapshot = pull.as_ref().is_some_and(|pull| {
        pull.snapshot
            .base_commit
            .as_deref()
            .is_some_and(is_object_id)
            && pull
                .snapshot
                .head_commit
                .as_deref()
                .is_some_and(is_object_id)
    });
    let can_verify_merge = has_snapshot && writer;
    let can_comment_on_diff =
        current_open && has_snapshot && state.user.is_some() && diff_error.is_none();
    let can_review = can_comment_on_diff && writer && !author;
    let merge_workflow = match (&pull, source) {
        (Some(pull), Some(source)) if viewed.is_none() => match pull
            .snapshot
            .head_commit
            .as_deref()
            .filter(|head| is_object_id(head))
        {
            Some(head) => Some(
                crate::branch_rules::workflow(
                    &state,
                    &repo,
                    head,
                    issue.id,
                    current_open
                        && writer
                        && has_snapshot
                        && pull.snapshot.merge_base.is_some()
                        && (!source.is_private() || repo.is_private()),
                )
                .await?,
            ),
            None => None,
        },
        _ => None,
    };
    let (viewed_review, historical) = viewed.unzip();
    let source_name = source
        .filter(|source| source.id != repo.id)
        .map(Repo::full_name);
    let pull = pull.map(|pull| {
        let mut model = PullView::from_pull(pull, source_name);
        if let Some(snapshot) = historical {
            model.snapshot = snapshot;
        }
        model
    });
    let is_owner = can_admin(&state, &repo);
    let page = IssuePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), &format!("#{number}")]),
        author_avatar: crate::templates::Avatar::new(
            issue
                .author_name
                .as_deref()
                .unwrap_or(locale.t(CommonText::Ghost)),
            issue.author_avatar_id,
        ),
        body_html: markdown::render(
            &issue.body,
            &markdown::Context::new(&repo, &repo.default_branch, ""),
        ),
        discussion: entries
            .into_iter()
            .map(|entry| entry.into_row(locale, &repo, source.unwrap_or(&repo)))
            .collect(),
        pagination,
        pull,
        comparison_available,
        changed,
        preview,
        preview_missing,
        diff_error,
        viewed_review,
        comparison_notice,
        can_refresh_comparison,
        metadata: if issue.is_pull() {
            None
        } else {
            Some(crate::issue_metadata::load(&state, &repo, &issue).await?)
        },
        reactions: if issue.is_pull() {
            Vec::new()
        } else {
            repo_features::reactions(&state, issue.id).await?
        },
        merge_workflow,
        can_review,
        can_comment_on_diff,
        can_verify_merge,
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
    let number: i64 = ctx
        .param("number")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
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
    let form = state.form()?;
    let body = util::form_value(form, "body").unwrap_or_default();
    if body.trim().is_empty() {
        let request_page = PageRequest::new(&req, "comments_page");
        return util::redirect(&format!(
            "/{}/{}/{}/{}?comments_page={}#discussion",
            repo.owner_name,
            repo.name,
            section(kind),
            number,
            request_page.number
        ));
    }
    if body.len() > 256 * 1024 {
        return crate::templates::error_response(
            &Chrome::new(&state),
            413,
            state.locale.t(DiscussionText::BodyTooLong),
        );
    }
    let [viewer, admin] = crate::handlers::common::viewer_params(&state);
    let now = util::now();
    let insert = database
        .prepare(format!(
            "INSERT INTO comments (issue_id, author_id, body, created_at, updated_at)
         SELECT i.id, ?4, ?5, ?6, ?6
           FROM issues i JOIN repos r ON r.id = i.repo_id
          WHERE i.id = ?3 AND {}
         RETURNING id",
            crate::handlers::common::VISIBLE_REPO_SQL,
        ))
        .bind(&[
            viewer.clone(),
            admin.clone(),
            util::js_int(issue.id),
            util::js_int(user.id),
            util::js_text(&body),
            util::js_int(now),
        ])?;
    let update = database
        .prepare(format!(
            "UPDATE issues SET updated_at = ?4
          WHERE id = ?3 AND EXISTS (
            SELECT 1 FROM repos r WHERE r.id = issues.repo_id AND {}
          )",
            crate::handlers::common::VISIBLE_REPO_SQL,
        ))
        .bind(&[viewer, admin, util::js_int(issue.id), util::js_int(now)])?;
    let results = database.batch(vec![insert, update]).await?;
    if results[0].results::<serde::de::IgnoredAny>()?.is_empty() {
        return not_found(&state, state.locale.t(CommonText::RepositoryNotFound));
    }
    redirect_to_entry(&state, &database, &repo, &issue, None).await
}

pub async fn set_state(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let number: i64 = ctx
        .param("number")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
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
    if Some(user.id) != issue.author_id && !can_admin(&state, &repo) {
        return forbidden(&state, state.locale.t(DiscussionText::StateChangeForbidden));
    }
    let form = state.form()?;
    let desired = match util::form_value(form, "state").as_deref() {
        Some("closed") => "closed",
        Some("open") => "open",
        _ => {
            return crate::templates::error_response(
                &Chrome::new(&state),
                400,
                state.locale.t(DiscussionText::InvalidState),
            )
        }
    };
    if kind == "pull" {
        #[derive(serde::Deserialize)]
        struct MergeState {
            merged: i64,
            running: i64,
        }
        let merge: Option<MergeState> = db::first(&database,
            "SELECT p.merged,
                    EXISTS (SELECT 1 FROM merge_queue q WHERE q.issue_id = p.issue_id AND q.state = 'running') AS running
               FROM pulls p WHERE p.issue_id = ?",
            &[util::js_int(issue.id)],
        ).await?;
        if merge.is_some_and(|row| row.merged != 0 || row.running != 0) {
            return crate::templates::error_response(
                &Chrome::new(&state),
                409,
                state.locale.t(DiscussionText::MergeStateLocked),
            );
        }
    }
    let [viewer, admin] = crate::handlers::common::viewer_params(&state);
    let now = util::now();
    let state_guard = format!(
        "EXISTS (SELECT 1 FROM repos r WHERE r.id = issues.repo_id AND {} AND (issues.author_id = ?1 OR r.owner_id = ?1 OR ?2 = 1))
         AND NOT EXISTS (SELECT 1 FROM pulls WHERE issue_id = issues.id AND merged = 1)
         AND NOT EXISTS (SELECT 1 FROM merge_queue WHERE issue_id = issues.id AND state = 'running')",
        crate::handlers::common::VISIBLE_REPO_SQL,
    );
    // D1 executes the batch as one transaction. Test live state and permissions inside it,
    // so concurrent or repeated submissions cannot create duplicate events or race revocations.
    let mut statements = vec![
        database
            .prepare(format!(
                "INSERT INTO issue_events (issue_id, actor_id, event, created_at)
             SELECT id, ?1, ?5, ?6 FROM issues WHERE id = ?3 AND state <> ?4 AND {state_guard}"
            ))
            .bind(&[
                viewer.clone(),
                admin.clone(),
                util::js_int(issue.id),
                util::js_text(desired),
                util::js_text(if desired == "closed" {
                    "closed"
                } else {
                    "reopened"
                }),
                util::js_int(now),
            ])?,
        database
            .prepare(format!(
                "UPDATE issues SET state = ?4, closed_at = ?5, updated_at = ?6
              WHERE id = ?3 AND state <> ?4 AND {state_guard}"
            ))
            .bind(&[
                viewer,
                admin,
                util::js_int(issue.id),
                util::js_text(desired),
                if desired == "closed" {
                    util::js_int(now)
                } else {
                    util::js_null()
                },
                util::js_int(now),
            ])?,
    ];
    if desired == "closed" && kind == "pull" {
        statements.push(
            database
                .prepare(
                    "DELETE FROM merge_queue WHERE issue_id = ?1 AND state IN ('queued', 'failed')
               AND EXISTS (SELECT 1 FROM issues WHERE id = ?1 AND state = 'closed')",
                )
                .bind(&[util::js_int(issue.id)])?,
        );
    }
    database.batch(statements).await?;
    if desired == "closed" && kind == "pull" {
        if let Some(base_branch) = database
            .prepare("SELECT base_branch FROM pulls WHERE issue_id = ?")
            .bind(&[util::js_int(issue.id)])?
            .first::<String>(Some("base_branch"))
            .await?
        {
            let env = state.env.clone();
            let repo_id = repo.id;
            let issue_id = issue.id;
            state.worker_ctx.wait_until(async move {
                crate::branch_rules::wake_branch_issue(&env, repo_id, &base_branch, Some(issue_id))
                    .await;
            });
        }
    }
    redirect_to_entry(&state, &database, &repo, &issue, None).await
}

#[derive(serde::Deserialize)]
struct DiscussionEntry {
    id: i64,
    body: String,
    kind: String,
    author_name: Option<String>,
    avatar_id: Option<i64>,
    created_at: Ts,
    base: Option<String>,
    head: Option<String>,
    comparison_version: Option<i64>,
    path: Option<String>,
    side: Option<String>,
    line: Option<i64>,
}

impl DiscussionEntry {
    fn into_row(self, locale: Locale, repo: &Repo, source: &Repo) -> DiscussionRow {
        let is_comment = self.kind == "comment";
        let review =
            self.base
                .zip(self.head)
                .zip(self.comparison_version)
                .map(|((base, head), version)| ReviewContext {
                    base,
                    head,
                    version,
                    line: match (self.path, self.side, self.line) {
                        (Some(path), Some(side), Some(line)) => Some(ReviewLine {
                            url_path: util::url_path(&path),
                            path,
                            side,
                            line,
                        }),
                        _ => None,
                    },
                });
        let html = if is_comment || (review.is_some() && self.kind != "merge_verified") {
            let (context_repo, reference, path) = review
                .as_ref()
                .and_then(|review| {
                    let line = review.line.as_ref()?;
                    let (context_repo, reference) = match line.side.as_str() {
                        "old" => (repo, &review.base),
                        "new" => (source, &review.head),
                        _ => return None,
                    };
                    is_object_id(reference).then_some((
                        context_repo,
                        reference.as_str(),
                        line.path.as_str(),
                    ))
                })
                .unwrap_or((repo, &repo.default_branch, ""));
            markdown::render(
                &self.body,
                &markdown::Context::new(context_repo, reference, path),
            )
        } else {
            String::new()
        };
        DiscussionRow {
            id: self.id,
            avatar: if is_comment || review.is_some() {
                crate::templates::Avatar::new(
                    self.author_name
                        .as_deref()
                        .unwrap_or(locale.t(CommonText::Ghost)),
                    self.avatar_id,
                )
            } else {
                Default::default()
            },
            author: self.author_name,
            date: self.created_at,
            html,
            action: self.kind,
            is_comment,
            review,
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
    if kind == "pull" {
        "pulls"
    } else {
        "issues"
    }
}

fn route_kind(req: &Request) -> Result<&'static str> {
    match req.url()?.path().split('/').nth(3) {
        Some("issues") => Ok("issue"),
        Some("pulls") => Ok("pull"),
        _ => Err(Error::RustError("Invalid discussion route".into())),
    }
}

pub(crate) async fn redirect_to_entry(
    state: &AppState,
    database: &worker::d1::D1Database,
    repo: &Repo,
    issue: &Issue,
    review: Option<i64>,
) -> Result<Response> {
    #[derive(serde::Deserialize)]
    struct Count {
        n: i64,
    }
    let [viewer, admin] = super::common::viewer_params(state);
    let count = db::first::<Count>(
        database,
        "SELECT (SELECT COUNT(*) FROM comments WHERE issue_id = ?1)
              + (SELECT COUNT(*) FROM issue_events WHERE issue_id = ?1)
              + (SELECT COUNT(*) FROM pull_reviews pr JOIN pulls p ON p.issue_id = pr.issue_id
                   JOIN repos s ON s.id = COALESCE(p.source_repo_id, ?4)
                 WHERE pr.issue_id = ?1 AND (s.visibility = 'public' OR s.owner_id = ?2 OR ?3 = 1
                       OR EXISTS (SELECT 1 FROM repo_member_roles m WHERE m.repo_id = s.id AND m.user_id = ?2))) AS n",
        &[util::js_int(issue.id), viewer, admin, util::js_int(repo.id)],
    ).await?.map(|count| count.n).unwrap_or(0);
    let page = (count.saturating_sub(1).max(0) / PAGE_SIZE as i64 + 1)
        .min(i64::from(crate::pagination::MAX_PAGE));
    let fragment = review.map_or_else(|| "discussion".into(), |id| format!("review-{id}"));
    util::redirect(&format!(
        "/{}/{}/{}/{}?comments_page={}#{fragment}",
        repo.owner_name,
        repo.name,
        section(&issue.kind),
        issue.number,
        page
    ))
}
