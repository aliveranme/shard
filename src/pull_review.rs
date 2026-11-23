use crate::ctx::AppState;
use crate::db;
use crate::git_storage::{is_object_id, RepoHandle};
use crate::handlers::common::{
    can_write, forbidden, load_repo_by_id, not_found, visible_repo, visible_repo_by_id,
};
use crate::i18n::{CommonText, DiscussionText};
use crate::models::{Issue, Pull, Repo};
use crate::pull_base::{self, Repositories};
use crate::pull_diff::{self, Comparison, FileDiff};
use crate::templates::{error_response, Chrome};
use crate::util;
use std::collections::{HashSet, VecDeque};
use worker::*;

const MAX_HISTORY_READS: usize = 80;
const MAX_QUEUED_PARENTS: usize = 512;
const REFRESH_RETRY_SECONDS: i64 = 60;

const TARGET_READ: &str = "(t.visibility = 'public' OR t.owner_id = u.id OR u.is_admin = 1
    OR EXISTS (SELECT 1 FROM repo_member_roles m WHERE m.repo_id = t.id AND m.user_id = u.id))";
const SOURCE_READ: &str = "(s.visibility = 'public' OR s.owner_id = u.id OR u.is_admin = 1
    OR EXISTS (SELECT 1 FROM repo_member_roles m WHERE m.repo_id = s.id AND m.user_id = u.id))";
const TARGET_WRITE: &str = "(t.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
    SELECT 1 FROM repo_member_roles m WHERE m.repo_id = t.id AND m.user_id = u.id AND m.role = 'write'))";

pub(crate) async fn load_pull(database: &d1::D1Database, id: i64) -> Result<Option<Pull>> {
    db::first(
        database,
        "SELECT issue_id, source_repo_id, head_branch, base_branch, head_commit, base_commit,
                merge_base, comparison_version, merged, merged_at, merge_commit
         FROM pulls WHERE issue_id = ?",
        &[util::js_int(id)],
    )
    .await
}

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
    let number = ctx
        .param("number")
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
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
    )
    .await?;
    let Some(issue) = issue else {
        return not_found(state, state.locale.t(CommonText::NotFound));
    };
    let Some(pull) = load_pull(&database, issue.id).await? else {
        return error(state, 409, DiscussionText::ReviewConflict);
    };
    let source_id = pull.source_repo_id(repo.id);
    let source_repo;
    let source = if source_id == repo.id {
        &repo
    } else {
        source_repo = visible_repo_by_id(state, source_id).await?;
        let Some(source) = source_repo.as_ref() else {
            return not_found(state, state.locale.t(CommonText::NotFound));
        };
        source
    };
    let conflict = || error(state, 409, DiscussionText::ReviewConflict);
    let form = state.form()?;
    let snapshot = &pull.snapshot;
    let version = snapshot.comparison_version;
    let submitted_version =
        util::form_value(form, "comparison_version").and_then(|value| value.parse().ok());
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
        return refresh_comparison(state, &database, &repo, source, &issue, &pull, actor).await;
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
    let handles = Repositories::open(&state.env, &repo, source).await?;
    let Comparison::Complete(files) = pull_diff::compare(
        &state.env,
        &handles.target,
        handles.source(),
        diff_base,
        head,
    )
    .await?
    else {
        return conflict();
    };
    if let (Some(path), Some(side), Some(line)) = (&path, side, line) {
        let Ok(index) = files.binary_search_by(|file| file.path.cmp(path)) else {
            return error(state, 400, DiscussionText::ReviewInvalid);
        };
        let FileDiff::Text(diff) = files[index]
            .lines(&handles.target, handles.source())
            .await?
        else {
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
        &format!("INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit,
                                    merge_base, comparison_version, path, side, line, body, created_at)
          SELECT p.issue_id, ?1, ?2, p.base_commit, p.head_commit, p.merge_base, p.comparison_version, ?3, ?4, ?5, ?6, ?7
          FROM pulls p JOIN issues i ON i.id = p.issue_id
          JOIN repos t ON t.id = i.repo_id JOIN repos s ON s.id = COALESCE(p.source_repo_id, i.repo_id)
          JOIN users u ON u.id = ?1
          WHERE p.issue_id = ?8 AND p.comparison_version = ?9 AND p.merged = 0 AND i.state = 'open'
            AND {TARGET_READ} AND {SOURCE_READ}
            AND (?2 = 'line_comment' OR (i.author_id IS NOT u.id AND {TARGET_WRITE}))
          RETURNING id"),
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
        Some(review) => {
            if decision == "approved" {
                let env = state.env.clone();
                let repo_id = repo.id;
                let base_branch = pull.base_branch.clone();
                let issue_id = issue.id;
                state.worker_ctx.wait_until(async move {
                    crate::branch_rules::wake_branch_issue(
                        &env,
                        repo_id,
                        &base_branch,
                        Some(issue_id),
                    )
                    .await;
                });
            }
            crate::handlers::issues::redirect_to_entry(
                state,
                &database,
                &repo,
                &issue,
                Some(review.id),
            )
            .await
        }
        None => conflict(),
    }
}

async fn refresh_comparison(
    state: &AppState,
    database: &d1::D1Database,
    target: &Repo,
    source: &Repo,
    issue: &Issue,
    pull: &Pull,
    actor: i64,
) -> Result<Response> {
    let handles = match Repositories::open(&state.env, target, source).await {
        Ok(handles) => handles,
        Err(cause) => {
            console_log!("pull comparison: could not open Git repository: {cause}");
            return error(state, 502, DiscussionText::GitUnavailable);
        }
    };
    match sample_comparison(&handles, pull, false).await {
        Ok(Some(tips)) => {
            if !save_comparison(database, pull, &tips, Some(actor), None).await? {
                return error(state, 409, DiscussionText::ReviewConflict);
            }
            let env = state.env.clone();
            let repo_id = target.id;
            let base_branch = pull.base_branch.clone();
            let issue_id = pull.issue_id;
            state.worker_ctx.wait_until(async move {
                crate::branch_rules::wake_branch_issue(&env, repo_id, &base_branch, Some(issue_id))
                    .await;
            });
        }
        Ok(None) => {}
        Err(message) => return error(state, 409, message),
    }
    util::redirect(&format!(
        "/{}/{}/pulls/{}#pull-comparison",
        target.owner_name, target.name, issue.number
    ))
}

struct ComparisonTips {
    base: String,
    head: String,
    merge_base: String,
}

async fn sample_comparison(
    handles: &Repositories,
    pull: &Pull,
    automatic: bool,
) -> std::result::Result<Option<ComparisonTips>, DiscussionText> {
    let (base, head) =
        pull_base::branch_tips(handles, &pull.base_branch, &pull.head_branch).await?;
    let snapshot = &pull.snapshot;
    if snapshot.merge_base.is_some()
        && snapshot.base_commit.as_deref() == Some(base.as_str())
        && snapshot.head_commit.as_deref() == Some(head.as_str())
    {
        return Ok(None);
    }
    let merge_base = pull_base::find(handles, &base, &head)
        .await
        .map_err(pull_base::BaseError::message)?;
    // Keep the saved commits available for external integration verification.
    if merge_base == head {
        return Err(DiscussionText::RefreshIntegrated);
    }
    if automatic {
        if let Some((saved_base, saved_head)) = snapshot
            .base_commit
            .as_deref()
            .zip(snapshot.head_commit.as_deref())
        {
            if saved_base != base
                && saved_head != head
                && saved_base != saved_head
                && is_object_id(saved_base)
                && is_object_id(saved_head)
                && handles
                    .target
                    .read_commit(saved_head)
                    .await
                    .map_err(storage_unavailable)?
                    .is_some()
            {
                match check_integration(&handles.target, &base, saved_base, saved_head)
                    .await
                    .map_err(storage_unavailable)?
                {
                    Ok(()) => return Err(DiscussionText::RefreshIntegrated),
                    Err(DiscussionText::MergeAbsent) => {}
                    Err(message) => return Err(message),
                }
            }
        }
    }
    Ok(Some(ComparisonTips {
        base,
        head,
        merge_base,
    }))
}

fn storage_unavailable(error: Error) -> DiscussionText {
    console_log!("pull comparison: could not read Git history: {error}");
    DiscussionText::GitUnavailable
}

async fn save_comparison(
    database: &d1::D1Database,
    pull: &Pull,
    tips: &ComparisonTips,
    actor: Option<i64>,
    generation: Option<i64>,
) -> Result<bool> {
    let result = database.prepare(format!(
        "UPDATE pulls AS p SET base_commit = ?1, head_commit = ?2, merge_base = ?3,
                              comparison_version = comparison_version + 1
         WHERE p.issue_id = ?4 AND p.comparison_version = ?5 AND p.merged = 0
           AND NOT EXISTS (SELECT 1 FROM merge_queue mq WHERE mq.issue_id = p.issue_id AND mq.state = 'running')
           AND (?7 IS NULL OR EXISTS (
               SELECT 1 FROM pull_comparison_updates q WHERE q.issue_id = p.issue_id AND q.generation = ?7
           ))
           AND EXISTS (
               SELECT 1 FROM issues i JOIN repos t ON t.id = i.repo_id
               JOIN repos s ON s.id = COALESCE(p.source_repo_id, i.repo_id)
               LEFT JOIN users u ON u.id = ?6
               WHERE i.id = p.issue_id AND i.state = 'open'
                 AND ((?7 IS NOT NULL AND i.author_id IS u.id AND (s.id = t.id OR ({TARGET_READ} AND {SOURCE_READ})))
                     OR (u.id IS NOT NULL AND {TARGET_READ} AND {SOURCE_READ} AND (i.author_id = u.id OR {TARGET_WRITE})))
           )"
    )).bind(&[
        util::js_text(&tips.base), util::js_text(&tips.head), util::js_text(&tips.merge_base),
        util::js_int(pull.issue_id), util::js_int(pull.snapshot.comparison_version),
        actor.map_or_else(util::js_null, util::js_int), generation.map_or_else(util::js_null, util::js_int),
    ])?.run().await?;
    let changed = result
        .meta()?
        .and_then(|meta| meta.changes)
        .map(|count| count != 0)
        .ok_or_else(|| Error::RustError("D1 did not report comparison changes".into()))?;
    if changed {
        let _ = db::run(
            database,
            "DELETE FROM merge_queue WHERE issue_id = ?1 AND state IN ('queued', 'failed') AND comparison_version <= ?2",
            &[util::js_int(pull.issue_id), util::js_int(pull.snapshot.comparison_version)],
        )
        .await;
    }
    Ok(changed)
}

#[derive(serde::Deserialize)]
struct PendingComparison {
    issue_id: i64,
    generation: i64,
}

#[derive(serde::Deserialize)]
struct ComparisonTarget {
    repo_id: i64,
    author_id: Option<i64>,
}

/// Re-sample refs on cron. A newly accepted push advances the queue generation
/// so work based on an older observation cannot consume the new signal.
pub(crate) async fn refresh_pending(env: &Env, limit: i64) -> Result<()> {
    let database = env.d1("DB")?;
    let pending: Vec<PendingComparison> = db::all(
        &database,
        "SELECT q.issue_id, q.generation FROM pull_comparison_updates q
         WHERE q.due_at <= ?1 AND NOT EXISTS (
             SELECT 1 FROM merge_queue mq WHERE mq.issue_id = q.issue_id AND mq.state = 'running'
         ) ORDER BY q.due_at ASC, q.issue_id ASC LIMIT ?2",
        &[util::js_int(util::now()), util::js_int(limit)],
    )
    .await?;
    for item in pending {
        let finished = match refresh_pending_comparison(env, &database, &item).await {
            Ok(finished) => finished,
            Err(error) => {
                console_log!("pull comparison: scheduled refresh failed: {error}");
                false
            }
        };
        let sql = if finished {
            "DELETE FROM pull_comparison_updates WHERE issue_id = ?1 AND generation = ?2"
        } else {
            "UPDATE pull_comparison_updates SET due_at = ?3 WHERE issue_id = ?1 AND generation = ?2"
        };
        let mut values = vec![util::js_int(item.issue_id), util::js_int(item.generation)];
        if !finished {
            values.push(util::js_int(util::now() + REFRESH_RETRY_SECONDS));
        }
        db::run(&database, sql, &values).await?;
    }
    Ok(())
}

async fn refresh_pending_comparison(
    env: &Env,
    database: &d1::D1Database,
    item: &PendingComparison,
) -> Result<bool> {
    let Some(pull) = load_pull(database, item.issue_id).await? else {
        return Ok(true);
    };
    if pull.merged != 0 {
        return Ok(true);
    }
    let target: Option<ComparisonTarget> = db::first(
        database,
        &format!("SELECT i.repo_id, i.author_id FROM issues i JOIN repos t ON t.id = i.repo_id
                  JOIN repos s ON s.id = COALESCE(?2, i.repo_id) LEFT JOIN users u ON u.id = i.author_id
                  WHERE i.id = ?1 AND i.state = 'open' AND (s.id = t.id OR ({TARGET_READ} AND {SOURCE_READ}))"),
        &[util::js_int(pull.issue_id), pull.source_repo_id.map_or_else(util::js_null, util::js_int)],
    ).await?;
    let Some(target) = target else {
        return Ok(true);
    };
    let Some(repo) = load_repo_by_id(database, target.repo_id).await? else {
        return Ok(true);
    };
    let source_id = pull.source_repo_id(repo.id);
    let source_repo;
    let source = if source_id == repo.id {
        &repo
    } else {
        source_repo = load_repo_by_id(database, source_id).await?;
        let Some(source) = source_repo.as_ref() else {
            return Ok(true);
        };
        source
    };
    let handles = Repositories::open(env, &repo, source).await?;
    match sample_comparison(&handles, &pull, true).await {
        Ok(Some(tips)) => {
            let changed = save_comparison(
                database,
                &pull,
                &tips,
                target.author_id,
                Some(item.generation),
            )
            .await?;
            if changed {
                crate::branch_rules::wake_branch_issue(
                    env,
                    repo.id,
                    &pull.base_branch,
                    Some(pull.issue_id),
                )
                .await;
            }
            Ok(changed)
        }
        Err(DiscussionText::GitUnavailable | DiscussionText::MergeUnavailable) => Ok(false),
        Ok(None) | Err(_) => Ok(true),
    }
}

#[derive(serde::Deserialize)]
struct ReviewId {
    id: i64,
}

async fn record_merge(
    state: &AppState,
    database: &d1::D1Database,
    repo: &Repo,
    issue: &Issue,
    pull: &Pull,
    actor: i64,
) -> Result<Response> {
    let snapshot = &pull.snapshot;
    let Some((base, head)) = snapshot
        .base_commit
        .as_deref()
        .zip(snapshot.head_commit.as_deref())
    else {
        return error(state, 409, DiscussionText::ReviewConflict);
    };
    let verified = async {
        let handle = RepoHandle::open(&state.env, repo).await?;
        integrated_tip(&handle, &pull.base_branch, base, head).await
    }
    .await;
    let tip = match verified {
        Ok(Ok(tip)) => tip,
        Ok(Err(reason)) => return error(state, 409, reason),
        Err(storage_error) => {
            console_log!("pull request: failed to verify target history: {storage_error}");
            return error(state, 502, DiscussionText::MergeUnavailable);
        }
    };
    let Some(review) =
        record_observed_merge(database, repo, issue, pull, Some(actor), &tip, true).await?
    else {
        return error(state, 409, DiscussionText::ReviewConflict);
    };
    crate::handlers::issues::redirect_to_entry(state, database, repo, issue, Some(review)).await
}

/// Record a target tip observed after an authorized native merge or Git verification.
pub(crate) async fn record_observed_merge(
    database: &d1::D1Database,
    repo: &Repo,
    issue: &Issue,
    pull: &Pull,
    actor: Option<i64>,
    tip: &str,
    verify_writer: bool,
) -> Result<Option<i64>> {
    if !is_object_id(tip) || issue.repo_id != repo.id || pull.issue_id != issue.id {
        return Ok(None);
    }
    let version = pull.snapshot.comparison_version;
    let now = util::now();
    let actor_val = actor.map_or_else(util::js_null, util::js_int);
    let writer_guard = if verify_writer {
        "AND EXISTS (
            SELECT 1 FROM repos r JOIN users u ON u.id = ?1
             WHERE r.id = i.repo_id AND (r.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
                 SELECT 1 FROM repo_member_roles c
                 WHERE c.repo_id = r.id AND c.user_id = u.id AND c.role = 'write'
             ))
         )"
    } else {
        ""
    };
    let event = database.prepare(format!(
        "INSERT INTO pull_reviews (issue_id, actor_id, decision, base_commit, head_commit,
                                   merge_base, comparison_version, body, created_at)
         SELECT p.issue_id, ?1, 'merge_verified', p.base_commit, p.head_commit, p.merge_base, p.comparison_version, ?2, ?3
         FROM pulls p JOIN issues i ON i.id = p.issue_id
         WHERE p.issue_id = ?4 AND p.comparison_version = ?5 AND p.merged = 0 AND i.repo_id = ?6
           {writer_guard}
         RETURNING id"
    )).bind(&[
        actor_val.clone(), util::js_text(tip), util::js_int(now), util::js_int(issue.id),
        util::js_int(version), util::js_int(repo.id),
    ])?;
    let merge = database
        .prepare(format!(
            "UPDATE pulls SET merged = 1, merged_at = ?3, merge_commit = ?2
         WHERE issue_id = ?4 AND comparison_version = ?5 AND merged = 0
           AND EXISTS (SELECT 1 FROM issues i WHERE i.id = ?4 AND i.repo_id = ?6 {writer_guard})"
        ))
        .bind(&[
            actor_val,
            util::js_text(tip),
            util::js_int(now),
            util::js_int(issue.id),
            util::js_int(version),
            util::js_int(repo.id),
        ])?;
    let close = database.prepare(
        "UPDATE issues SET state = 'closed', closed_at = COALESCE(closed_at, ?1), updated_at = ?1
         WHERE id = ?2 AND state <> 'closed' AND EXISTS (SELECT 1 FROM pulls WHERE issue_id = ?2 AND merged = 1)",
    ).bind(&[util::js_int(now), util::js_int(issue.id)])?;
    let clear_queue = database
        .prepare(
            "UPDATE merge_queue SET state = 'merged', reason = NULL, changed_at = ?1
         WHERE issue_id = ?2 AND state IN ('queued', 'failed')
           AND EXISTS (SELECT 1 FROM pulls WHERE issue_id = ?2 AND merged = 1)",
        )
        .bind(&[util::js_int(now), util::js_int(issue.id)])?;
    let results = database
        .batch(vec![event, merge, close, clear_queue])
        .await?;
    Ok(results[0]
        .results::<ReviewId>()?
        .into_iter()
        .next()
        .map(|review| review.id))
}

pub(crate) async fn integrated_tip(
    repo: &RepoHandle,
    base_branch: &str,
    base: &str,
    head: &str,
) -> Result<std::result::Result<String, DiscussionText>> {
    if !is_object_id(base) || !is_object_id(head) || base == head {
        return Ok(Err(DiscussionText::MergeAbsent));
    }
    let Some(tip) = repo
        .head_commit(&format!("refs/heads/{base_branch}"))
        .await?
    else {
        return Ok(Err(DiscussionText::MergeAbsent));
    };
    Ok(check_integration(repo, &tip.hash, base, head)
        .await?
        .map(|()| tip.hash))
}

async fn check_integration(
    repo: &RepoHandle,
    tip: &str,
    base: &str,
    head: &str,
) -> Result<std::result::Result<(), DiscussionText>> {
    if !is_object_id(tip) || tip == base {
        return Ok(Err(DiscussionText::MergeAbsent));
    }
    if let RepoHandle::Service(native) = repo {
        if !native.is_durable() {
            use crate::native_git::SnapshotRead;
            return Ok(match native.integrated(tip, base, head).await? {
                SnapshotRead::Found(true) => Ok(()),
                SnapshotRead::Found(false) => Err(DiscussionText::MergeAbsent),
                SnapshotRead::Missing => Err(DiscussionText::MergeUnavailable),
                SnapshotRead::Limited => Err(DiscussionText::MergeLimited),
            });
        }
    }
    let mut seen = HashSet::from([tip.to_owned()]);
    let mut queue = VecDeque::from([tip.to_owned()]);
    let (mut found_base, mut found_head, mut missing) = (false, false, false);
    for _ in 0..MAX_HISTORY_READS {
        let Some(id) = queue.pop_front() else {
            return Ok(Err(if missing {
                DiscussionText::MergeUnavailable
            } else {
                DiscussionText::MergeAbsent
            }));
        };
        let Some(parents) = repo.commit_parents(&id).await? else {
            missing = true;
            continue;
        };
        found_base |= id == base;
        found_head |= id == head;
        if found_base && found_head {
            return Ok(Ok(()));
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
