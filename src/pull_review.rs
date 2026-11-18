use crate::git_storage::{is_object_id, RepoHandle};
use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{can_write, forbidden, not_found, visible_repo};
use crate::i18n::{CommonText, DiscussionText};
use crate::models::{Issue, Pull, Repo};
use crate::pull_base;
use crate::pull_diff::{self, Comparison, FileDiff};
use crate::templates::{error_response, Chrome};
use crate::util;
use std::collections::{HashSet, VecDeque};
use worker::*;

const MAX_HISTORY_READS: usize = 80;
const MAX_QUEUED_PARENTS: usize = 512;

enum Operation {
    Review,
    Refresh,
}

pub async fn submit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    mutate(ctx, Operation::Review).await
}

pub async fn refresh(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    mutate(ctx, Operation::Refresh).await
}

async fn mutate(ctx: RouteContext<AppState>, operation: Operation) -> Result<Response> {
    let state = &ctx.data;
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let number = ctx.param("number").and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(actor) = state.user_id() else {
        return util::redirect("/user/login");
    };
    let database = state.db()?;
    let issue: Option<Issue> = db::first(
        &database,
        "SELECT id, repo_id, number, kind, title, state, body, author_id, NULL AS author_name,
                created_at, updated_at, closed_at, 0 AS comment_count
         FROM issues WHERE repo_id = ? AND number = ? AND kind = 'pull'",
        &[util::js_int(repo.id), util::js_int(number)],
    ).await?;
    let Some(issue) = issue else {
        return not_found(state, state.locale.t(CommonText::NotFound));
    };
    let pull: Option<Pull> = db::first(
        &database,
        "SELECT issue_id, head_branch, base_branch, head_commit, base_commit, merge_base,
                comparison_version, merged, merged_at, merge_commit
         FROM pulls WHERE issue_id = ?",
        &[util::js_int(issue.id)],
    ).await?;
    let conflict = || error(state, 409, DiscussionText::ReviewConflict);
    let Some(pull) = pull else { return conflict() };
    let form = state.form()?;
    let snapshot = &pull.snapshot;
    let version = snapshot.comparison_version;
    let submitted_version = util::form_value(form, "comparison_version").and_then(|value| value.parse().ok());
    if submitted_version != Some(version) || pull.merged != 0 {
        return conflict();
    }
    if matches!(operation, Operation::Refresh) {
        if !issue.is_open() {
            return conflict();
        }
        if issue.author_id != Some(actor) && !can_write(state, &repo).await? {
            return forbidden(state, state.locale.t(DiscussionText::RefreshForbidden));
        }
        return refresh_comparison(state, &database, &repo, &issue, &pull, actor).await;
    }
    let Some((diff_base, head)) = snapshot.diff_tips() else {
        return conflict();
    };
    let decision = util::form_value(form, "decision").unwrap_or_default();
    let comment = util::form_value(form, "body").unwrap_or_default();
    let comment = comment.trim();
    if comment.len() > 8_192 {
        return error(state, 400, DiscussionText::ReviewInvalid);
    }
    let mut path = None;
    let mut side = None;
    let mut line = None;
    match decision.as_str() {
        "approved" | "changes_requested" => {
            if Some(actor) == issue.author_id || !can_write(state, &repo).await? {
                return forbidden(state, state.locale.t(DiscussionText::ReviewForbidden));
            }
        }
        "line_comment" if !comment.is_empty() => {
            path = util::form_value(form, "path").filter(|p| !p.is_empty() && p.len() <= 4096);
            let anchor = util::form_value(form, "line");
            if let Some((side_name, number)) = anchor.as_deref().and_then(|s| s.split_once(':')) {
                side = match side_name {
                    "old" => Some("old"),
                    "new" => Some("new"),
                    _ => None,
                };
                line = number.parse::<usize>().ok().filter(|n| *n > 0 && *n <= 512);
            }
            if path.is_none() || side.is_none() || line.is_none() {
                return error(state, 400, DiscussionText::ReviewInvalid);
            }
        }
        "merge_verified" => {
            if !can_write(state, &repo).await? {
                return forbidden(state, state.locale.t(DiscussionText::MergeForbidden));
            }
            return record_merge(state, &database, &repo, &issue, &pull, actor).await;
        }
        _ => return error(state, 400, DiscussionText::ReviewInvalid),
    }
    if !issue.is_open() {
        return conflict();
    }
    let handle = RepoHandle::open(&state.env, &repo).await?;
    let Comparison::Complete(files) = pull_diff::compare(&handle, diff_base, head).await? else {
        return conflict();
    };
    if let (Some(path), Some(side), Some(line)) = (&path, side, line) {
        let Ok(index) = files.binary_search_by(|file| file.path.cmp(path)) else {
            return error(state, 400, DiscussionText::ReviewInvalid);
        };
        let FileDiff::Text(diff) = files[index].lines(&handle).await? else {
            return error(state, 400, DiscussionText::ReviewInvalid);
        };
        if !diff.rows().any(|row| match side {
            "old" => row.old == line && row.new == 0,
            _ => row.new == line && row.old == 0,
        }) {
            return error(state, 400, DiscussionText::ReviewInvalid);
        }
    }
    let recorded: Option<ReviewId> = db::first(
        &database,
        "INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit,
                                   merge_base, comparison_version, path, side, line, body, created_at)
         SELECT p.issue_id, ?1, ?2, p.base_commit, p.head_commit, p.merge_base, p.comparison_version, ?3, ?4, ?5, ?6, ?7
         FROM pulls p JOIN issues i ON i.id = p.issue_id
         WHERE p.issue_id = ?8 AND p.comparison_version = ?9 AND p.merged = 0 AND i.state = 'open'
         RETURNING id",
        &[
            util::js_int(actor), util::js_text(&decision),
            path.as_deref().map_or_else(util::js_null, util::js_text),
            side.map_or_else(util::js_null, util::js_text),
            line.map_or_else(util::js_null, |n| util::js_int(n as i64)),
            util::js_text(comment), util::js_int(util::now()), util::js_int(issue.id),
            util::js_int(version),
        ],
    ).await?;
    match recorded {
        Some(review) => crate::handlers::issues::redirect_to_entry(&database, &repo, &issue, Some(review.id)).await,
        None => conflict(),
    }
}

async fn refresh_comparison(
    state: &AppState,
    database: &d1::D1Database,
    repo: &Repo,
    issue: &Issue,
    pull: &Pull,
    actor: i64,
) -> Result<Response> {
    let handle = match RepoHandle::open(&state.env, repo).await {
        Ok(handle) => handle,
        Err(cause) => {
            console_log!("pull comparison: could not open Git repository: {cause}");
            return error(state, 502, DiscussionText::GitUnavailable);
        }
    };
    let (base, head) = match pull_base::branch_tips(&handle, &pull.base_branch, &pull.head_branch).await {
        Ok(tips) => tips,
        Err(message) => return error(state, 409, message),
    };
    let snapshot = &pull.snapshot;
    let url = format!("/{}/{}/pulls/{}#pull-comparison", repo.owner_name, repo.name, issue.number);
    if snapshot.merge_base.is_some()
        && snapshot.base_commit.as_deref() == Some(base.as_str())
        && snapshot.head_commit.as_deref() == Some(head.as_str())
    {
        return util::redirect(&url);
    }
    let merge_base = match pull_base::find(&handle, &base, &head).await {
        Ok(merge_base) => merge_base,
        Err(cause) => return error(state, 409, cause.message()),
    };
    // Keep the pre-integration snapshot available to the merge verifier.
    if merge_base == head {
        return error(state, 409, DiscussionText::RefreshIntegrated);
    }

    let result = database.prepare(
        "UPDATE pulls SET base_commit = ?1, head_commit = ?2, merge_base = ?3,
                          comparison_version = comparison_version + 1
         WHERE issue_id = ?4 AND comparison_version = ?5 AND merged = 0
           AND EXISTS (
               SELECT 1 FROM issues i JOIN repos r ON r.id = i.repo_id JOIN users u ON u.id = ?6
               WHERE i.id = pulls.issue_id AND i.state = 'open'
                 AND (u.id = r.owner_id OR u.is_admin = 1 OR EXISTS (
                      SELECT 1 FROM repo_collaborators c
                      WHERE c.repo_id = r.id AND c.user_id = u.id
                        AND (c.role = 'write' OR i.author_id = u.id)
                 ) OR (r.visibility = 'public' AND i.author_id = u.id))
           )",
    ).bind(&[
        util::js_text(&base), util::js_text(&head), util::js_text(&merge_base),
        util::js_int(issue.id), util::js_int(snapshot.comparison_version), util::js_int(actor),
    ])?.run().await?;
    match result.meta()?.and_then(|meta| meta.changes) {
        Some(0) => error(state, 409, DiscussionText::ReviewConflict),
        Some(_) => util::redirect(&url),
        None => Err(Error::RustError("D1 did not report comparison changes".into())),
    }
}

#[derive(serde::Deserialize)]
struct ReviewId { id: i64 }

async fn record_merge(
    state: &AppState,
    database: &d1::D1Database,
    repo: &Repo,
    issue: &Issue,
    pull: &Pull,
    actor: i64,
) -> Result<Response> {
    let snapshot = &pull.snapshot;
    let Some((base, head)) = snapshot.base_commit.as_deref().zip(snapshot.head_commit.as_deref()) else {
        return error(state, 409, DiscussionText::ReviewConflict);
    };
    let verified = async {
        let handle = RepoHandle::open(&state.env, repo).await?;
        integrated_tip(&handle, &pull.base_branch, base, head).await
    }.await;
    let tip = match verified {
        Ok(Ok(tip)) => tip,
        Ok(Err(reason)) => return error(state, 409, reason),
        Err(storage_error) => {
            console_log!("pull request: failed to verify target history: {storage_error}");
            return error(state, 502, DiscussionText::MergeUnavailable);
        }
    };
    let now = util::now();
    let event = database.prepare(
        "INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit,
                                   merge_base, comparison_version, body, created_at)
         SELECT issue_id, ?1, 'merge_verified', base_commit, head_commit, merge_base, comparison_version, ?2, ?3
         FROM pulls WHERE issue_id = ?4 AND comparison_version = ?5 AND merged = 0
         RETURNING id",
    ).bind(&[
        util::js_int(actor), util::js_text(&tip), util::js_int(now), util::js_int(issue.id),
        util::js_int(snapshot.comparison_version),
    ])?;
    let merge = database.prepare(
        "UPDATE pulls SET merged = 1, merged_at = ?1, merge_commit = ?2
         WHERE issue_id = ?3 AND comparison_version = ?4 AND merged = 0",
    ).bind(&[util::js_int(now), util::js_text(&tip), util::js_int(issue.id), util::js_int(snapshot.comparison_version)])?;
    let close = database.prepare(
        "UPDATE issues SET state = 'closed', closed_at = COALESCE(closed_at, ?1), updated_at = ?1
         WHERE id = ?2 AND state <> 'closed' AND EXISTS (SELECT 1 FROM pulls WHERE issue_id = ?2 AND merged = 1)",
    ).bind(&[util::js_int(now), util::js_int(issue.id)])?;
    let results = database.batch(vec![event, merge, close]).await?;
    let Some(review) = results[0].results::<ReviewId>()?.into_iter().next() else {
        return error(state, 409, DiscussionText::ReviewConflict);
    };
    crate::handlers::issues::redirect_to_entry(database, repo, issue, Some(review.id)).await
}

async fn integrated_tip(
    repo: &RepoHandle,
    base_branch: &str,
    base: &str,
    head: &str,
) -> Result<std::result::Result<String, DiscussionText>> {
    if !is_object_id(base) || !is_object_id(head) || base == head {
        return Ok(Err(DiscussionText::MergeAbsent));
    }
    let Some(tip) = repo.head_commit(&format!("refs/heads/{base_branch}")).await? else {
        return Ok(Err(DiscussionText::MergeAbsent));
    };
    if !is_object_id(&tip.hash) || tip.hash == base {
        return Ok(Err(DiscussionText::MergeAbsent));
    }
    let mut seen = HashSet::from([tip.hash.clone()]);
    let mut queue = VecDeque::from([tip.hash.clone()]);
    let (mut found_base, mut found_head, mut missing) = (false, false, false);
    for _ in 0..MAX_HISTORY_READS {
        let Some(id) = queue.pop_front() else {
            return Ok(Err(if missing { DiscussionText::MergeUnavailable } else { DiscussionText::MergeAbsent }));
        };
        let Some(parents) = repo.commit_parents(&id).await? else {
            missing = true;
            continue;
        };
        found_base |= id == base;
        found_head |= id == head;
        if found_base && found_head {
            return Ok(Ok(tip.hash));
        }
        for parent in parents {
            if seen.len() == MAX_QUEUED_PARENTS {
                return Ok(Err(DiscussionText::MergeLimited));
            }
            if seen.insert(parent.clone()) {
                queue.push_back(parent);
            }
        }
    }
    Ok(Err(DiscussionText::MergeLimited))
}

fn error(state: &AppState, status: u16, message: DiscussionText) -> Result<Response> {
    error_response(&Chrome::new(state), status, state.locale.t(message))
}
