use crate::ctx::AppState;
use crate::git_storage::{is_object_id, RepoHandle};
use crate::git_wire::{valid_ref, RefUpdate, ZERO_OID};
use crate::handlers::common::{can_admin, can_write, forbidden, load_repo_by_id, not_found, page_title, repo_role, visible_repo, visible_repo_by_id, RepoRole};
use crate::i18n::{BranchText, CommonText, RepoText};
use crate::models::{Issue, Pull, Repo};
use crate::native_git::WriteError;
use crate::templates::{error_response, render, Chrome};
use crate::{db, git_activity, pull_review, util};
use askama::Template;
use serde::Deserialize;
use worker::{d1::D1Database, Env, Request, Response, Result, RouteContext};

const CLAIM_SECONDS: i64 = 300;

#[derive(Template)]
#[template(path = "branch_rules.html")]
struct Settings {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    branch: String,
    approvals: i64,
    checks: String,
}

impl Rule {
    fn contexts(&self) -> String {
        serde_json::from_str::<Vec<String>>(&self.checks).unwrap_or_default().join("\n")
    }
}

pub(crate) async fn settings(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else { return missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let database = state.db()?;
    let rules = db::all(&database,
        "SELECT branch, approvals, checks FROM repo_branch_rules WHERE repo_id = ? ORDER BY branch",
        &[util::js_int(repo.id)],
    ).await?;
    render(&Settings {
        chrome: Chrome::new(state), title: page_title(&[&repo.full_name(), state.locale.t(BranchText::Protection)]),
        repo, tab: "settings", is_owner: true, rules,
    })
}

pub(crate) async fn update(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else { return missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let form = state.form()?;
    let branch = util::form_value(form, "branch").unwrap_or_default();
    if !valid_ref(&branch) {
        return error(state, 400, BranchText::InvalidRule);
    }
    let database = state.db()?;
    let repo_id = util::js_int(repo.id);
    let branch_value = util::js_text(&branch);
    let changed = match util::form_value(form, "action").as_deref() {
        Some("save") => {
            let approvals = util::form_value(form, "approvals").and_then(|value| value.parse::<i64>().ok());
            let raw = util::form_value(form, "checks").unwrap_or_default();
            let mut checks: Vec<_> = raw.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
            checks.sort_unstable();
            checks.dedup();
            let Some(approvals) = approvals.filter(|n| (0..=20).contains(n)) else {
                return error(state, 400, BranchText::InvalidRule);
            };
            if checks.len() > 20 || checks.iter().any(|name| name.len() > 80 || name.chars().any(char::is_control)) {
                return error(state, 400, BranchText::InvalidRule);
            }
            let checks = serde_json::to_string(&checks)?;
            database.prepare(
                "INSERT INTO repo_branch_rules (repo_id, branch, approvals, checks)
                 SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS (
                     SELECT 1 FROM merge_queue WHERE repo_id = ?1 AND base_branch = ?2 AND state = 'running')
                 ON CONFLICT(repo_id, branch) DO UPDATE SET approvals = excluded.approvals, checks = excluded.checks
                 RETURNING repo_id",
            ).bind(&[repo_id, branch_value, util::js_int(approvals), util::js_text(&checks)])?.first::<i64>(Some("repo_id")).await?
        }
        Some("remove") if util::form_value(form, "confirm").as_deref() == Some(&branch) => {
            database.prepare(
                "DELETE FROM repo_branch_rules WHERE repo_id = ?1 AND branch = ?2 AND NOT EXISTS (
                     SELECT 1 FROM merge_queue WHERE repo_id = ?1 AND base_branch = ?2 AND state = 'running')
                 RETURNING repo_id",
            ).bind(&[repo_id, branch_value])?.first::<i64>(Some("repo_id")).await?
        }
        _ => return error(state, 400, BranchText::InvalidRule),
    };
    if changed.is_none() {
        return error(state, 409, BranchText::RefConflict);
    }
    util::redirect(&format!("/{}/settings/branches", repo.full_name()))
}

pub(crate) async fn blocks_push(database: &D1Database, repo_id: i64, updates: &[RefUpdate]) -> Result<bool> {
    let branches: Vec<_> = updates.iter().filter_map(|update| {
        let branch = update.name.strip_prefix("refs/heads/")?;
        (update.before != update.after).then_some((branch, update.after == ZERO_OID))
    }).collect();
    if branches.is_empty() { return Ok(false) }
    let changes = serde_json::to_string(&branches)?;
    Ok(database.prepare(
        "SELECT EXISTS (
            SELECT 1 FROM json_each(?2) c JOIN repos r ON r.id = ?1
            LEFT JOIN repo_branch_rules b ON b.repo_id = r.id AND b.branch = json_extract(c.value, '$[0]')
            WHERE b.repo_id IS NOT NULL OR (json_extract(c.value, '$[1]') AND r.default_branch = json_extract(c.value, '$[0]'))
         ) AS blocked",
    ).bind(&[util::js_int(repo_id), util::js_text(&changes)])?.first::<i64>(Some("blocked")).await? == Some(1))
}

pub(crate) async fn write_ref(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else { return missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let form = state.form()?;
    let name = util::form_value(form, "name").unwrap_or_default();
    let oid = util::form_value(form, "oid").unwrap_or_default();
    let kind = util::form_value(form, "kind").unwrap_or_default();
    if !valid_ref(&name) || !is_object_id(&oid) || oid == ZERO_OID || !matches!(kind.as_str(), "heads" | "tags") {
        return error(state, 400, BranchText::InvalidRef);
    }
    let (before, after) = match util::form_value(form, "action").as_deref() {
        Some("create") => (ZERO_OID.into(), oid),
        Some("delete") if util::form_value(form, "confirm").as_deref() == Some(&name) => (oid, ZERO_OID.into()),
        _ => return error(state, 400, BranchText::InvalidRef),
    };
    let update = RefUpdate { name: format!("refs/{kind}/{name}"), before, after };
    if blocks_push(&state.db()?, repo.id, std::slice::from_ref(&update)).await? {
        return error(state, 409, BranchText::CannotWriteRef);
    }
    let handle = RepoHandle::open(&state.env, &repo).await?;
    if update.after != ZERO_OID && handle.read_commit(&update.after).await?.is_none() {
        return error(state, 400, BranchText::InvalidRef);
    }
    if let Err(cause) = handle.write_ref(&update.name, &update.before, &update.after).await {
        return match cause {
            WriteError::Unavailable | WriteError::Unsupported => error_response(&Chrome::new(state), 502, state.locale.t(RepoText::StorageUnavailable)),
            _ => error(state, 409, BranchText::RefConflict),
        };
    }
    git_activity::record(&state.env, repo.id, state.user_id(), &[update]).await;
    util::redirect(&format!("/{}/refs", repo.full_name()))
}

#[derive(Deserialize)]
struct QueueEntry {
    issue_id: i64,
    repo_id: i64,
    base_branch: String,
    comparison_version: i64,
    actor_id: Option<i64>,
    state: String,
    reason: Option<String>,
    changed_at: i64,
}

pub(crate) struct Workflow {
    pub can_queue: bool,
    pub can_cancel: bool,
    pub status: Option<&'static str>,
    pub checks: crate::handlers::checks::CommitChecks,
}

pub(crate) async fn workflow(
    state: &AppState, target: &Repo, head: &str, issue_id: i64, can_write: bool,
) -> Result<Workflow> {
    let entry: Option<QueueEntry> = db::first(&state.db()?,
        "SELECT issue_id, repo_id, base_branch, comparison_version, actor_id, state, reason, changed_at
         FROM merge_queue WHERE issue_id = ?", &[util::js_int(issue_id)],
    ).await?;
    let (can_queue, can_cancel, status) = match entry.as_ref().map(|job| job.state.as_str()) {
        Some("queued") => (false, can_write, Some(if entry.as_ref().and_then(|job| job.reason.as_deref()) == Some("waiting") {
            BranchText::MergeWaiting
        } else { BranchText::Queued })),
        Some("running") => (false, false, Some(BranchText::Running)),
        Some("merged") => (false, false, Some(BranchText::MergeComplete)),
        Some("failed") => (can_write, false, entry.as_ref().and_then(|job| job.reason.as_deref()).map(failure_text)),
        _ => (can_write, false, None),
    };
    let checks = crate::handlers::checks::load(state, target.id, head).await?;
    Ok(Workflow { can_queue, can_cancel, status: status.map(|key| state.locale.t(key)), checks })
}

pub(crate) async fn request_merge(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else { return missing(state) };
    let Some(actor) = state.user_id() else { return util::redirect("/user/login") };
    if !can_write(state, &repo).await? {
        return error(state, 403, BranchText::MergeForbidden);
    }
    let number = ctx.param("number").and_then(|value| value.parse::<i64>().ok()).unwrap_or(0);
    let database = state.db()?;
    let issue_id = database.prepare("SELECT id FROM issues WHERE repo_id = ?1 AND number = ?2 AND kind = 'pull'")
        .bind(&[util::js_int(repo.id), util::js_int(number)])?.first::<i64>(Some("id")).await?;
    let Some(issue_id) = issue_id else { return not_found(state, state.locale.t(CommonText::NotFound)) };
    let Some((issue, pull)) = load_pull(&database, issue_id).await? else {
        return not_found(state, state.locale.t(CommonText::NotFound));
    };
    let form = state.form()?;
    let location = format!("/{}/pulls/{number}#merge", repo.full_name());
    if util::form_value(form, "action").as_deref() == Some("cancel") {
        db::run(&database, "DELETE FROM merge_queue WHERE issue_id = ? AND state = 'queued'", &[util::js_int(issue_id)]).await?;
        return util::redirect(&location);
    }
    let version = util::form_value(form, "comparison_version").and_then(|value| value.parse().ok());
    if !issue.is_open() || pull.merged != 0 || version != Some(pull.snapshot.comparison_version) || pull.snapshot.merge_base.is_none() {
        return error(state, 409, BranchText::MergeStale);
    }
    let source_id = pull.source_repo_id(repo.id);
    if source_id != repo.id {
        let Some(source) = visible_repo_by_id(state, source_id).await? else {
            return error(state, 403, BranchText::MergeForbidden);
        };
        if source.is_private() && !repo.is_private() {
            return error(state, 403, BranchText::MergeForbidden);
        }
    }
    let enqueued = database.prepare(
        "INSERT INTO merge_queue (issue_id, repo_id, base_branch, comparison_version, actor_id, state, queued_at, changed_at)
         SELECT p.issue_id, i.repo_id, p.base_branch, p.comparison_version, ?3, 'queued', ?4, ?4
         FROM pulls p JOIN issues i ON i.id = p.issue_id
         WHERE p.issue_id = ?1 AND p.comparison_version = ?2 AND p.merged = 0 AND i.state = 'open'
         ON CONFLICT(issue_id) DO UPDATE SET comparison_version = excluded.comparison_version,
             actor_id = excluded.actor_id, state = 'queued', reason = NULL,
             queued_at = excluded.queued_at, changed_at = excluded.changed_at
         WHERE merge_queue.state = 'failed'
         RETURNING issue_id",
    ).bind(&[util::js_int(issue_id), util::js_int(pull.snapshot.comparison_version), util::js_int(actor), util::js_int(util::now())])?
        .first::<i64>(Some("issue_id")).await?;
    if enqueued.is_some() {
        let env = state.env.clone();
        state.worker_ctx.wait_until(async move {
            if run_entry(&env, issue_id).await.is_err() {
                worker::console_log!("event=merge_queue_item_failed issue_id={issue_id}");
            }
        });
    }
    util::redirect(&location)
}

pub(crate) async fn process_queue(env: &Env, limit: i64) -> Result<()> {
    let database = env.d1("DB")?;
    let entries = db::all::<EntryId>(&database,
        "SELECT issue_id FROM merge_queue q
         WHERE ((q.state = 'queued' AND NOT EXISTS (
                  SELECT 1 FROM merge_queue earlier
                  WHERE earlier.repo_id = q.repo_id AND earlier.base_branch = q.base_branch AND
                      (earlier.state = 'running' OR (earlier.state = 'queued' AND
                       (earlier.queued_at, earlier.issue_id) < (q.queued_at, q.issue_id)))))
                OR (q.state = 'running' AND q.changed_at < ?1))
         ORDER BY changed_at, issue_id LIMIT ?2",
        &[util::js_int(util::now() - CLAIM_SECONDS), util::js_int(limit)],
    ).await?;
    for entry in entries {
        if run_entry(env, entry.issue_id).await.is_err() {
            worker::console_log!("event=merge_queue_item_failed issue_id={}", entry.issue_id);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct EntryId { issue_id: i64 }

#[derive(Deserialize)]
struct Actor { id: i64, is_admin: i64 }

async fn run_entry(env: &Env, issue_id: i64) -> Result<()> {
    let database = env.d1("DB")?;
    let Some(job) = db::first::<QueueEntry>(&database,
        "SELECT issue_id, repo_id, base_branch, comparison_version, actor_id, state, reason, changed_at
         FROM merge_queue WHERE issue_id = ?1 AND state IN ('queued', 'running')",
        &[util::js_int(issue_id)],
    ).await? else { return Ok(()) };
    if job.state == "running" && job.changed_at >= util::now() - CLAIM_SECONDS {
        return Ok(());
    }
    let Some((issue, pull)) = load_pull(&database, issue_id).await? else { return Ok(()) };
    let Some(target) = load_repo_by_id(&database, job.repo_id).await? else { return Ok(()) };
    if pull.merged != 0 {
        return update_job(&database, &job, "merged", None).await;
    }
    let Some((base, head)) = pull.snapshot.base_commit.as_deref().zip(pull.snapshot.head_commit.as_deref()) else {
        return fail(&database, &job, "stale").await;
    };
    if job.comparison_version != pull.snapshot.comparison_version || job.base_branch != pull.base_branch || !valid_ref(&job.base_branch) {
        return fail(&database, &job, "stale").await;
    }
    let Some(actor) = db::first::<Actor>(&database, "SELECT id, is_admin FROM users WHERE id = ?",
        &[job.actor_id.map_or_else(util::js_null, util::js_int)]).await? else {
        return fail(&database, &job, "access").await;
    };
    let target_handle = RepoHandle::open(env, &target).await?;
    let reference = format!("refs/heads/{}", job.base_branch);
    let actual_base = target_handle.head_commit(&reference).await?;
    if job.state == "running" {
        if actual_base.as_ref().is_some_and(|commit| commit.hash != base) {
            if let Ok(tip) = pull_review::integrated_tip(&target_handle, &job.base_branch, base, head).await? {
                return finish(env, &database, &job, &target, &issue, &pull, &tip).await;
            }
            return fail(&database, &job, "stale").await;
        }
        return update_job(&database, &job, "queued", None).await;
    }
    if !issue.is_open() || repo_role(&database, target.id, target.owner_id, actor.id, actor.is_admin).await? != Some(RepoRole::Write) {
        return fail(&database, &job, "access").await;
    }
    let source_id = pull.source_repo_id(target.id);
    let source = if source_id != target.id {
        let Some(source) = load_repo_by_id(&database, source_id).await? else { return fail(&database, &job, "access").await };
        if source.is_private() && (!target.is_private() || repo_role(&database, source.id, source.owner_id, actor.id, actor.is_admin).await?.is_none()) {
            return fail(&database, &job, "access").await;
        }
        Some((RepoHandle::open(env, &source).await?, source.storage_id))
    } else { None };
    let source_handle = source.as_ref().map_or(&target_handle, |(handle, _)| handle);
    let actual_head = source_handle.head_commit(&format!("refs/heads/{}", pull.head_branch)).await?;
    if actual_base.as_ref().is_none_or(|commit| commit.hash != base) || actual_head.as_ref().is_none_or(|commit| commit.hash != head) {
        return fail(&database, &job, "stale").await;
    }
    let claimed_at = util::now();
    let claimed = database.prepare(CLAIM).bind(&[
        util::js_int(job.issue_id), util::js_int(job.comparison_version), util::js_text(head),
        util::js_int(claimed_at), util::js_int(actor.id), util::js_int(job.changed_at),
    ])?.first::<i64>(Some("issue_id")).await?;
    let Some(_) = claimed else {
        return update_job(&database, &job, "queued", Some("waiting")).await;
    };
    let result = target_handle.merge_saved(env, &reference, base, head,
        pull.snapshot.merge_base.as_deref() == Some(base), source.as_ref().map(|(handle, id)| (handle, id.as_str())),
    ).await;
    let running = QueueEntry { state: "running".into(), changed_at: claimed_at, ..job };
    match result {
        Ok(tip) => finish(env, &database, &running, &target, &issue, &pull, &tip).await,
        Err(WriteError::Unavailable) => Ok(()), // The origin may already have advanced the ref. Recover after the claim expires.
        Err(WriteError::Conflict) => fail(&database, &running, "conflict").await,
        Err(WriteError::Unsupported) => fail(&database, &running, "unsupported").await,
        Err(WriteError::Missing | WriteError::Stale) => fail(&database, &running, "stale").await,
        Err(WriteError::Rejected) => fail(&database, &running, "storage").await,
    }
}

const CLAIM: &str = "WITH latest_reviews AS (
    SELECT p.actor_id, p.decision FROM pull_reviews p JOIN users u ON u.id = p.actor_id
    JOIN merge_queue q ON q.issue_id = p.issue_id JOIN repos r ON r.id = q.repo_id
    JOIN issues i ON i.id = p.issue_id
    WHERE p.issue_id = ?1 AND p.comparison_version = ?2 AND p.decision IN ('approved', 'changes_requested')
      AND p.actor_id IS NOT i.author_id
      AND (u.id = r.owner_id OR u.is_admin = 1 OR EXISTS (SELECT 1 FROM repo_member_roles m
           WHERE m.repo_id = r.id AND m.user_id = u.id AND m.role = 'write'))
      AND NOT EXISTS (SELECT 1 FROM pull_reviews newer WHERE newer.issue_id = p.issue_id AND newer.actor_id = p.actor_id
          AND newer.comparison_version = p.comparison_version AND newer.id > p.id
          AND newer.decision IN ('approved', 'changes_requested'))
) UPDATE merge_queue SET state = 'running', changed_at = ?4, reason = NULL
 WHERE issue_id = ?1 AND comparison_version = ?2 AND state = 'queued'
   AND actor_id = ?5 AND changed_at = ?6
   AND NOT EXISTS (SELECT 1 FROM merge_queue earlier
        WHERE earlier.repo_id = merge_queue.repo_id AND earlier.base_branch = merge_queue.base_branch AND
            (earlier.state = 'running' OR (earlier.state = 'queued' AND
             (earlier.queued_at, earlier.issue_id) < (merge_queue.queued_at, merge_queue.issue_id))))
   AND EXISTS (
       SELECT 1 FROM pulls p JOIN issues i ON i.id = p.issue_id
       JOIN repos t ON t.id = i.repo_id JOIN repos s ON s.id = COALESCE(p.source_repo_id, t.id)
       JOIN users u ON u.id = ?5
       WHERE p.issue_id = ?1 AND p.comparison_version = ?2 AND p.merged = 0 AND i.state = 'open'
         AND (t.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
             SELECT 1 FROM repo_member_roles m WHERE m.repo_id = t.id AND m.user_id = u.id AND m.role = 'write'))
         AND (s.id = t.id OR s.visibility = 'public' OR (t.visibility = 'private'
             AND (s.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
                 SELECT 1 FROM repo_member_roles m WHERE m.repo_id = s.id AND m.user_id = u.id))))
   )
   AND NOT EXISTS (SELECT 1 FROM repo_branch_rules b WHERE b.repo_id = merge_queue.repo_id AND b.branch = merge_queue.base_branch
       AND (b.approvals > (SELECT COUNT(*) FROM latest_reviews WHERE decision = 'approved')
        OR EXISTS (SELECT 1 FROM latest_reviews WHERE decision = 'changes_requested')
        OR EXISTS (SELECT 1 FROM json_each(b.checks) c WHERE NOT EXISTS (
            SELECT 1 FROM commit_checks k WHERE k.repo_id = b.repo_id AND k.commit_hash = ?3
                AND k.context = c.value AND k.state = 'success'))))
 RETURNING issue_id";

async fn finish(env: &Env, database: &D1Database, job: &QueueEntry, repo: &Repo, issue: &Issue, pull: &Pull, tip: &str) -> Result<()> {
    let (Some(actor), Some(base)) = (job.actor_id, pull.snapshot.base_commit.as_deref()) else {
        return fail(database, job, "stale").await;
    };
    if pull_review::record_observed_merge(database, repo, issue, pull, actor, tip).await?.is_some() {
        update_job(database, job, "merged", None).await?;
        let update = RefUpdate { name: format!("refs/heads/{}", job.base_branch), before: base.into(), after: tip.into() };
        git_activity::record(env, repo.id, Some(actor), &[update]).await;
    }
    Ok(())
}

async fn update_job(database: &D1Database, job: &QueueEntry, state: &str, reason: Option<&str>) -> Result<()> {
    db::run(database,
        "UPDATE merge_queue SET state = ?1, reason = ?2, changed_at = ?3
         WHERE issue_id = ?4 AND comparison_version = ?5 AND state = ?6 AND actor_id IS ?7 AND changed_at = ?8",
        &[util::js_text(state), reason.map_or_else(util::js_null, util::js_text), util::js_int(util::now()),
          util::js_int(job.issue_id), util::js_int(job.comparison_version), util::js_text(&job.state),
          job.actor_id.map_or_else(util::js_null, util::js_int), util::js_int(job.changed_at)],
    ).await
}

async fn fail(database: &D1Database, job: &QueueEntry, reason: &str) -> Result<()> {
    update_job(database, job, "failed", Some(reason)).await
}

fn failure_text(reason: &str) -> BranchText {
    match reason {
        "stale" => BranchText::MergeStale,
        "access" => BranchText::MergeAccess,
        "conflict" => BranchText::MergeConflict,
        "unsupported" => BranchText::MergeUnsupported,
        _ => BranchText::MergeStorage,
    }
}

async fn load_pull(database: &D1Database, id: i64) -> Result<Option<(Issue, Pull)>> {
    let args = [util::js_int(id)];
    let Some(issue) = db::first::<Issue>(database,
        "SELECT id, repo_id, number, kind, title, body, state, author_id, created_at, updated_at, closed_at,
                NULL AS author_name, NULL AS author_avatar_id, 0 AS comment_count FROM issues WHERE id = ? AND kind = 'pull'",
        &args,
    ).await? else { return Ok(None) };
    let pull = pull_review::load_pull(database, id).await?;
    Ok(pull.map(|pull| (issue, pull)))
}

async fn lookup(ctx: &RouteContext<AppState>) -> Result<Option<Repo>> {
    visible_repo(&ctx.data, ctx.param("owner").map_or("", String::as_str), ctx.param("repo").map_or("", String::as_str)).await
}

fn missing(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(CommonText::RepositoryNotFound))
}

fn error(state: &AppState, status: u16, message: BranchText) -> Result<Response> {
    error_response(&Chrome::new(state), status, state.locale.t(message))
}
