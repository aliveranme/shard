use crate::artifacts::{is_object_id, Artifacts, RepoHandle};
use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{can_write, forbidden, not_found, visible_repo};
use crate::i18n::{CommonText, DiscussionText};
use crate::models::{Issue, Pull, Repo};
use crate::pull_diff::{self, Comparison, FileDiff};
use crate::templates::{error_response, Chrome};
use crate::util;
use std::collections::{HashSet, VecDeque};
use worker::*;

const MAX_HISTORY_READS: usize = 80;
const MAX_QUEUED_PARENTS: usize = 512;

pub async fn submit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
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
        "SELECT issue_id, head_branch, base_branch, head_commit, base_commit, merged, merged_at, merge_commit
         FROM pulls WHERE issue_id = ?",
        &[util::js_int(issue.id)],
    ).await?;
    let conflict = || error(state, 409, DiscussionText::ReviewConflict);
    let Some(pull) = pull else { return conflict() };
    let Some((base, head)) = pull.base_commit.as_deref().zip(pull.head_commit.as_deref()) else {
        return conflict();
    };
    let form = state.form()?;
    if util::form_value(form, "base_commit").as_deref() != Some(base)
        || util::form_value(form, "head_commit").as_deref() != Some(head)
        || pull.merged != 0
    {
        return conflict();
    }
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
            return record_merge(state, &database, &repo, &issue, &pull, actor, base, head).await;
        }
        _ => return error(state, 400, DiscussionText::ReviewInvalid),
    }
    if !issue.is_open() {
        return conflict();
    }
    let handle = Artifacts::from_env(&state.env)?.get(&repo.artifacts_repo).await?;
    let Comparison::Complete(files) = pull_diff::compare(&handle, base, head).await? else {
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
        "INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit, path, side, line, body, created_at)
         SELECT p.issue_id, ?1, ?2, p.base_commit, p.head_commit, ?3, ?4, ?5, ?6, ?7
         FROM pulls p JOIN issues i ON i.id = p.issue_id
         WHERE p.issue_id = ?8 AND p.base_commit = ?9 AND p.head_commit = ?10 AND p.merged = 0 AND i.state = 'open'
         RETURNING id",
        &[
            util::js_int(actor), util::js_text(&decision),
            path.as_deref().map_or_else(util::js_null, util::js_text),
            side.map_or_else(util::js_null, util::js_text),
            line.map_or_else(util::js_null, |n| util::js_int(n as i64)),
            util::js_text(comment), util::js_int(util::now()), util::js_int(issue.id),
            util::js_text(base), util::js_text(head),
        ],
    ).await?;
    match recorded {
        Some(review) => crate::handlers::issues::redirect_to_entry(&database, &repo, &issue, Some(review.id)).await,
        None => conflict(),
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
    base: &str,
    head: &str,
) -> Result<Response> {
    let verified = async {
        let handle = Artifacts::from_env(&state.env)?.get(&repo.artifacts_repo).await?;
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
        "INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit, body, created_at)
         SELECT issue_id, ?1, 'merge_verified', base_commit, head_commit, ?2, ?3 FROM pulls
          WHERE issue_id = ?4 AND base_commit = ?5 AND head_commit = ?6 AND merged = 0
         RETURNING id",
    ).bind(&[
        util::js_int(actor), util::js_text(&tip), util::js_int(now), util::js_int(issue.id),
        util::js_text(base), util::js_text(head),
    ])?;
    let merge = database.prepare(
        "UPDATE pulls SET merged = 1, merged_at = ?1, merge_commit = ?2
         WHERE issue_id = ?3 AND base_commit = ?4 AND head_commit = ?5 AND merged = 0",
    ).bind(&[util::js_int(now), util::js_text(&tip), util::js_int(issue.id), util::js_text(base), util::js_text(head)])?;
    let close = database.prepare(
        "UPDATE issues SET state = 'closed', closed_at = COALESCE(closed_at, ?1), updated_at = ?1
         WHERE id = ?2 AND state <> 'closed' AND EXISTS (SELECT 1 FROM pulls WHERE issue_id = ?2 AND merged = 1)",
    ).bind(&[util::js_int(now), util::js_int(issue.id)])?;
    let results = database.batch(vec![event, merge, close]).await?;
    let review = results[0].results::<ReviewId>()?.into_iter().next();
    crate::handlers::issues::redirect_to_entry(database, repo, issue, review.map(|row| row.id)).await
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
