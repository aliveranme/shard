use crate::ctx::AppState;
use crate::git_activity::Admission;
use crate::git_storage::{is_object_id, RepoHandle};
use crate::git_wire::{valid_ref, RefUpdate, ZERO_OID};
use crate::handlers::common::{
    can_admin, can_write, forbidden, load_repo_by_id, not_found, page_title, repo_role,
    visible_repo, visible_repo_by_id, RepoRole,
};
use crate::i18n::{BranchText, CommonText, RepoText};
use crate::models::{Issue, Pull, Repo};
use crate::native_git::WriteError;
use crate::templates::{error_response, render, Chrome};
use crate::{db, git_activity, pull_review, util};
use askama::Template;
use serde::Deserialize;
use worker::{
    d1::D1Database, durable_object, DurableObject, Env, Error, Method, ObjectNamespace, Request,
    RequestInit, Response, Result, RouteContext, State,
};

const CLAIM_SECONDS: i64 = 300;
const MERGE_COORDINATOR: &str = "MERGE_COORDINATOR";
const WAIT_SECONDS: i64 = 30;
const MAX_WAIT_SECONDS: i64 = 300;
const MAX_WAIT_ATTEMPTS: u32 = 12;
const MAX_ACTIVE_QUEUE_PER_REPO: i64 = 64;

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
        serde_json::from_str::<Vec<String>>(&self.checks)
            .unwrap_or_default()
            .join("\n")
    }
}

pub(crate) async fn settings(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else {
        return missing(state);
    };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let database = state.db()?;
    let rules = db::all(
        &database,
        "SELECT branch, approvals, checks FROM repo_branch_rules WHERE repo_id = ? ORDER BY branch",
        &[util::js_int(repo.id)],
    )
    .await?;
    render(&Settings {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(BranchText::Protection)]),
        repo,
        tab: "settings",
        is_owner: true,
        rules,
    })
}

pub(crate) async fn update(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else {
        return missing(state);
    };
    let Some(user_id) = state.user_id().filter(|_| can_admin(state, &repo)) else {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    };
    let form = state.form()?;
    let branch = util::form_value(form, "branch").unwrap_or_default();
    if !valid_ref(&branch) {
        return error(state, 400, BranchText::InvalidRule);
    }
    let database = state.db()?;
    let repo_id = util::js_int(repo.id);
    let branch_value = util::js_text(&branch);
    let actor_id = util::js_int(user_id);
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
                 SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (
                     SELECT 1 FROM repos r JOIN users actor ON actor.id = ?5
                      WHERE r.id = ?1 AND (actor.id = r.owner_id OR actor.is_admin = 1))
                 AND NOT EXISTS (
                     SELECT 1 FROM merge_queue WHERE repo_id = ?1 AND base_branch = ?2 COLLATE NOCASE AND state = 'running')
                 AND NOT EXISTS (SELECT 1 FROM pending_branch_writes WHERE repo_id = ?1 AND branch = ?2 COLLATE NOCASE)
                 ON CONFLICT(repo_id, branch) DO UPDATE SET approvals = excluded.approvals, checks = excluded.checks
                 RETURNING repo_id",
            ).bind(&[repo_id, branch_value, util::js_int(approvals), util::js_text(&checks), actor_id])?.first::<i64>(Some("repo_id")).await?
        }
        Some("remove") if util::form_value(form, "confirm").as_deref() == Some(&branch) => {
            database.prepare(
                "DELETE FROM repo_branch_rules WHERE repo_id = ?1 AND branch = ?2
                 AND EXISTS (SELECT 1 FROM repos r JOIN users actor ON actor.id = ?3
                              WHERE r.id = ?1 AND (actor.id = r.owner_id OR actor.is_admin = 1))
                 AND NOT EXISTS (
                     SELECT 1 FROM merge_queue WHERE repo_id = ?1 AND base_branch = ?2 COLLATE NOCASE AND state = 'running')
                 AND NOT EXISTS (SELECT 1 FROM pending_branch_writes WHERE repo_id = ?1 AND branch = ?2 COLLATE NOCASE)
                 RETURNING repo_id",
            ).bind(&[repo_id, branch_value, actor_id])?.first::<i64>(Some("repo_id")).await?
        }
        _ => return error(state, 400, BranchText::InvalidRule),
    };
    if changed.is_none() {
        return error(state, 409, BranchText::RuleConflict);
    }
    util::redirect(&format!("/{}/settings/branches", repo.full_name()))
}

pub(crate) async fn write_ref(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else {
        return missing(state);
    };
    let Some(actor) = state.user_id().filter(|_| can_admin(state, &repo)) else {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    };
    let form = state.form()?;
    let name = util::form_value(form, "name").unwrap_or_default();
    let oid = util::form_value(form, "oid").unwrap_or_default();
    let kind = util::form_value(form, "kind").unwrap_or_default();
    if !valid_ref(&name)
        || !is_object_id(&oid)
        || oid == ZERO_OID
        || !matches!(kind.as_str(), "heads" | "tags")
    {
        return error(state, 400, BranchText::InvalidRef);
    }
    let (before, after) = match util::form_value(form, "action").as_deref() {
        Some("create") => (ZERO_OID.into(), oid),
        Some("delete") if util::form_value(form, "confirm").as_deref() == Some(&name) => {
            (oid, ZERO_OID.into())
        }
        _ => return error(state, 400, BranchText::InvalidRef),
    };
    let update = RefUpdate {
        name: format!("refs/{kind}/{name}"),
        before,
        after,
    };
    let updates = std::slice::from_ref(&update);
    let handle = RepoHandle::open(&state.env, &repo).await?;
    if update.after != ZERO_OID && handle.read_commit(&update.after).await?.is_none() {
        return error(state, 400, BranchText::InvalidRef);
    }
    let Admission::Admitted(Some(intent)) =
        git_activity::stage(&state.db()?, repo.id, updates, Some(actor)).await?
    else {
        return error(state, 409, BranchText::CannotWriteRef);
    };
    match handle
        .write_ref(&update.name, &update.before, &update.after)
        .await
    {
        Ok(()) => {
            if git_activity::confirm(&state.env, intent, Some(actor), &[update.name])
                .await
                .is_err()
            {
                worker::console_log!("event=git_push_confirmation_failed repo_id={}", repo.id);
            }
        }
        Err(cause) => {
            if cause == WriteError::Unavailable {
                let _ = git_activity::release_admission(&state.env, repo.id, intent.id()).await;
            } else {
                git_activity::confirm(&state.env, intent, Some(actor), &[]).await?;
            }
            return match cause {
                WriteError::Unavailable | WriteError::Unsupported => error_response(
                    &Chrome::new(state),
                    502,
                    state.locale.t(RepoText::StorageUnavailable),
                ),
                _ => error(state, 409, BranchText::RefConflict),
            };
        }
    }
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
    state: &AppState,
    target: &Repo,
    head: &str,
    issue_id: i64,
    can_write: bool,
) -> Result<Workflow> {
    let entry: Option<QueueEntry> = db::first(&state.db()?,
        "SELECT issue_id, repo_id, base_branch, comparison_version, actor_id, state, reason, changed_at
         FROM merge_queue WHERE issue_id = ?", &[util::js_int(issue_id)],
    ).await?;
    let (can_queue, can_cancel, status) = match entry.as_ref().map(|job| job.state.as_str()) {
        Some("queued") => (
            false,
            can_write,
            Some(
                if entry.as_ref().and_then(|job| job.reason.as_deref()) == Some("waiting") {
                    BranchText::MergeWaiting
                } else {
                    BranchText::Queued
                },
            ),
        ),
        Some("running") => (false, false, Some(BranchText::Running)),
        Some("merged") => (false, false, Some(BranchText::MergeComplete)),
        Some("failed") => (
            can_write,
            false,
            entry
                .as_ref()
                .and_then(|job| job.reason.as_deref())
                .map(failure_text),
        ),
        _ => (can_write, false, None),
    };
    let checks = crate::handlers::checks::load(state, target.id, head).await?;
    Ok(Workflow {
        can_queue,
        can_cancel,
        status: status.map(|key| state.locale.t(key)),
        checks,
    })
}

pub(crate) async fn request_merge(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = lookup(&ctx).await? else {
        return missing(state);
    };
    let Some(actor) = state.user_id() else {
        return util::redirect("/user/login");
    };
    if !can_write(state, &repo).await? {
        return error(state, 403, BranchText::MergeForbidden);
    }
    let number = ctx
        .param("number")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let database = state.db()?;
    let issue_id = database
        .prepare("SELECT id FROM issues WHERE repo_id = ?1 AND number = ?2 AND kind = 'pull'")
        .bind(&[util::js_int(repo.id), util::js_int(number)])?
        .first::<i64>(Some("id"))
        .await?;
    let Some(issue_id) = issue_id else {
        return not_found(state, state.locale.t(CommonText::NotFound));
    };
    let Some((issue, pull)) = load_pull(&database, issue_id).await? else {
        return not_found(state, state.locale.t(CommonText::NotFound));
    };
    let form = state.form()?;
    let location = format!("/{}/pulls/{number}#merge", repo.full_name());
    if util::form_value(form, "action").as_deref() == Some("cancel") {
        let removed = database
            .prepare("DELETE FROM merge_queue WHERE issue_id = ? AND state = 'queued'")
            .bind(&[util::js_int(issue_id)])?
            .run()
            .await?
            .meta()?
            .and_then(|meta| meta.changes)
            .unwrap_or(0);
        if removed > 0 {
            let env = state.env.clone();
            let repo_id = repo.id;
            let base_branch = pull.base_branch.clone();
            state.worker_ctx.wait_until(async move {
                wake_branch_issue(&env, repo_id, &base_branch, Some(issue_id)).await;
            });
        }
        return util::redirect(&location);
    }
    let version = util::form_value(form, "comparison_version").and_then(|value| value.parse().ok());
    if !issue.is_open()
        || pull.merged != 0
        || version != Some(pull.snapshot.comparison_version)
        || pull.snapshot.merge_base.is_none()
    {
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
           AND (SELECT COUNT(*) FROM merge_queue active
                WHERE active.repo_id = i.repo_id AND active.state IN ('queued', 'running')) < ?5
         ON CONFLICT(issue_id) DO UPDATE SET comparison_version = excluded.comparison_version,
             actor_id = excluded.actor_id, state = 'queued', reason = NULL,
             queued_at = excluded.queued_at, changed_at = excluded.changed_at
         WHERE merge_queue.state = 'failed'
         RETURNING issue_id",
    ).bind(&[
        util::js_int(issue_id), util::js_int(pull.snapshot.comparison_version), util::js_int(actor),
        util::js_int(util::now()), util::js_int(MAX_ACTIVE_QUEUE_PER_REPO),
    ])?
        .first::<i64>(Some("issue_id")).await?;
    if enqueued.is_some() {
        let env = state.env.clone();
        state.worker_ctx.wait_until(async move {
            if signal_or_run(&env, repo.id, &pull.base_branch, issue_id)
                .await
                .is_err()
            {
                worker::console_log!("event=merge_queue_item_failed issue_id={issue_id}");
            }
        });
    }
    util::redirect(&location)
}

pub(crate) async fn process_queue(env: &Env, limit: i64) -> Result<()> {
    let database = env.d1("DB")?;
    let entries = db::all::<EntryId>(
        &database,
        "SELECT issue_id, repo_id, base_branch FROM merge_queue q
         WHERE q.state IN ('queued', 'running')
           AND ((q.state = 'queued' AND NOT EXISTS (
                  SELECT 1 FROM merge_queue earlier
                  WHERE earlier.repo_id = q.repo_id AND earlier.base_branch = q.base_branch
                    AND earlier.state IN ('queued', 'running')
                    AND (earlier.state = 'running' OR (earlier.state = 'queued' AND
                         (earlier.queued_at, earlier.issue_id) < (q.queued_at, q.issue_id)))))
                OR (q.state = 'running' AND q.changed_at < ?1))
         ORDER BY changed_at, issue_id LIMIT ?2",
        &[
            util::js_int(util::now() - CLAIM_SECONDS),
            util::js_int(limit),
        ],
    )
    .await?;
    for entry in entries {
        if recover_or_run(env, entry.repo_id, &entry.base_branch, entry.issue_id)
            .await
            .is_err()
        {
            worker::console_log!("event=merge_queue_item_failed issue_id={}", entry.issue_id);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct EntryId {
    issue_id: i64,
    repo_id: i64,
    base_branch: String,
}

async fn signal_or_run(env: &Env, repo_id: i64, branch: &str, issue_id: i64) -> Result<()> {
    if let Ok(namespace) = env.durable_object(MERGE_COORDINATOR) {
        if signal_path(&namespace, repo_id, branch, "wake", Some(issue_id))
            .await
            .is_ok()
        {
            return Ok(());
        }
        worker::console_log!("event=merge_coordinator_unavailable repo_id={repo_id}");
    }
    run_entry(env, issue_id).await
}

async fn recover_or_run(env: &Env, repo_id: i64, branch: &str, issue_id: i64) -> Result<()> {
    if let Ok(namespace) = env.durable_object(MERGE_COORDINATOR) {
        if signal_path(&namespace, repo_id, branch, "recover", None)
            .await
            .is_ok()
        {
            return Ok(());
        }
        worker::console_log!("event=merge_coordinator_unavailable repo_id={repo_id}");
    }
    run_entry(env, issue_id).await
}

async fn signal(
    namespace: &ObjectNamespace,
    repo_id: i64,
    branch: &str,
    issue_id: Option<i64>,
) -> Result<()> {
    signal_path(namespace, repo_id, branch, "wake", issue_id).await
}

async fn signal_path(
    namespace: &ObjectNamespace,
    repo_id: i64,
    branch: &str,
    endpoint: &str,
    issue_id: Option<i64>,
) -> Result<()> {
    let name = format!("{repo_id}:refs/heads/{branch}");
    let stub = namespace.get_by_name(&name)?;
    let init = RequestInit {
        method: Method::Post,
        body: Some(name.into()),
        ..Default::default()
    };
    let url = match issue_id {
        Some(id) => format!("https://internal/{endpoint}?issue={id}"),
        None => format!("https://internal/{endpoint}"),
    };
    let response = stub
        .fetch_with_request(Request::new_with_init(&url, &init)?)
        .await?;
    if response.status_code() == 202 {
        Ok(())
    } else {
        Err(Error::RustError(
            "Merge coordinator did not accept the wake request".into(),
        ))
    }
}

pub(crate) async fn wake_branch_issue(
    env: &Env,
    repo_id: i64,
    branch: &str,
    issue_id: Option<i64>,
) {
    if let Ok(namespace) = env.durable_object(MERGE_COORDINATOR) {
        let _ = signal(&namespace, repo_id, branch, issue_id).await;
    }
}

pub(crate) async fn wake_for_check(env: &Env, repo_id: i64, head: &str) -> Result<()> {
    let Ok(namespace) = env.durable_object(MERGE_COORDINATOR) else {
        return Ok(());
    };
    let database = env.d1("DB")?;
    let limit = crate::limits::ScheduledLimits::read(env)?.merges;
    #[derive(Deserialize)]
    struct BranchIssue {
        issue_id: i64,
        base_branch: String,
    }
    let items = db::all::<BranchIssue>(
        &database,
        "SELECT q.issue_id, q.base_branch FROM merge_queue q JOIN pulls p ON p.issue_id = q.issue_id
         WHERE q.repo_id = ?1 AND q.state = 'queued' AND p.head_commit = ?2
         ORDER BY q.base_branch, q.queued_at, q.issue_id LIMIT ?3",
        &[
            util::js_int(repo_id),
            util::js_text(head),
            util::js_int(limit),
        ],
    )
    .await?;
    for item in items {
        signal(&namespace, repo_id, &item.base_branch, Some(item.issue_id)).await?;
    }
    Ok(())
}

#[durable_object]
pub struct MergeQueueCoordinator {
    state: State,
    env: Env,
}

#[derive(Deserialize)]
struct BranchJob {
    issue_id: i64,
    state: String,
    changed_at: i64,
}

impl MergeQueueCoordinator {
    async fn arm(&self, delay_seconds: i64) -> Result<()> {
        let at = (util::now() + delay_seconds) * 1000;
        let storage = self.state.storage();
        if storage
            .get_alarm()
            .await?
            .is_none_or(|current| current > at)
        {
            storage
                .set_alarm(std::time::Duration::from_secs(delay_seconds as u64))
                .await?;
        }
        Ok(())
    }
}

fn branch_key(key: &str) -> Option<(i64, &str)> {
    let (id, reference) = key.split_once(':')?;
    let id: i64 = id.parse().ok()?;
    let branch = reference.strip_prefix("refs/heads/")?;
    (id > 0 && valid_ref(branch)).then_some((id, branch))
}

async fn first_branch_job(
    database: &D1Database,
    repo_id: i64,
    branch: &str,
) -> Result<Option<BranchJob>> {
    db::first(
        database,
        "SELECT issue_id, state, changed_at FROM merge_queue
         WHERE repo_id = ?1 AND base_branch = ?2 AND state IN ('running', 'queued')
         ORDER BY state = 'running' DESC, queued_at, issue_id LIMIT 1",
        &[util::js_int(repo_id), util::js_text(branch)],
    )
    .await
}

impl DurableObject for MergeQueueCoordinator {
    fn new(state: State, env: Env) -> Self {
        Self { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let path = req.path();
        if req.method() != Method::Post || !matches!(path.as_str(), "/wake" | "/recover") {
            return Ok(Response::empty()?.with_status(404));
        }
        let key = req.text().await?;
        if branch_key(&key).is_none() {
            return Ok(Response::empty()?.with_status(400));
        }
        let storage = self.state.storage();
        match storage.get::<String>("branch").await? {
            Some(current) if current != key => return Ok(Response::empty()?.with_status(409)),
            Some(_) => (),
            None => storage.put("branch", key).await?,
        }
        if path == "/wake" {
            let woken_issue = util::query_param(&req, "issue")
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|&id| id > 0);
            let waiting_issue = storage.get::<i64>("wait_issue").await?;
            if waiting_issue.is_none() || woken_issue.is_none() || waiting_issue == woken_issue {
                let _ = storage.delete("wait_attempts").await;
                let _ = storage.delete("wait_issue").await;
                self.arm(0).await?;
            } else if storage.get_alarm().await?.is_none() {
                self.arm(0).await?;
            }
        } else if storage.get_alarm().await?.is_none() {
            self.arm(0).await?;
        }
        Ok(Response::empty()?.with_status(202))
    }

    async fn alarm(&self) -> Result<Response> {
        let storage = self.state.storage();
        let key = storage.get::<String>("branch").await?;
        let Some((repo_id, branch)) = key.as_deref().and_then(branch_key) else {
            return Response::empty();
        };
        let database = self.env.d1("DB")?;
        let Some(job) = first_branch_job(&database, repo_id, branch).await? else {
            let _ = storage.delete("wait_attempts").await;
            let _ = storage.delete("wait_issue").await;
            return Response::empty();
        };
        let recovery_delay = job.changed_at + CLAIM_SECONDS + 1 - util::now();
        if job.state == "running" && recovery_delay > 0 {
            self.arm(recovery_delay).await?;
            return Response::empty();
        }
        run_entry(&self.env, job.issue_id).await?;
        if let Some(next) = first_branch_job(&database, repo_id, branch).await? {
            if next.state == "running" {
                let _ = storage.delete("wait_attempts").await;
                let _ = storage.delete("wait_issue").await;
                let delay = (next.changed_at + CLAIM_SECONDS + 1 - util::now()).max(0);
                self.arm(delay).await?;
            } else if next.issue_id == job.issue_id && job.state == "queued" {
                let previous_issue = storage.get::<i64>("wait_issue").await?;
                let attempts = if previous_issue == Some(next.issue_id) {
                    storage
                        .get::<u32>("wait_attempts")
                        .await?
                        .unwrap_or(0)
                        .saturating_add(1)
                } else {
                    1
                };
                if attempts <= MAX_WAIT_ATTEMPTS {
                    storage.put("wait_issue", next.issue_id).await?;
                    storage.put("wait_attempts", attempts).await?;
                    let shift = (attempts - 1).min(4);
                    let delay = (WAIT_SECONDS << shift).min(MAX_WAIT_SECONDS);
                    self.arm(delay).await?;
                } else {
                    let _ = storage.delete("wait_attempts").await;
                    let _ = storage.delete("wait_issue").await;
                    let _ = db::run(
                        &database,
                        "UPDATE merge_queue SET state = 'failed', reason = 'checks', changed_at = ?2
                         WHERE issue_id = ?1 AND state = 'queued'",
                        &[util::js_int(next.issue_id), util::js_int(util::now())],
                    )
                    .await;
                    self.arm(0).await?;
                }
            } else {
                let _ = storage.delete("wait_attempts").await;
                let _ = storage.delete("wait_issue").await;
                self.arm(0).await?;
            }
        } else {
            let _ = storage.delete("wait_attempts").await;
            let _ = storage.delete("wait_issue").await;
        }
        Response::empty()
    }
}

#[derive(Deserialize)]
struct Actor {
    id: i64,
    is_admin: i64,
}

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
    let Some((issue, pull)) = load_pull(&database, issue_id).await? else {
        return Ok(());
    };
    let Some(target) = load_repo_by_id(&database, job.repo_id).await? else {
        return Ok(());
    };
    if pull.merged != 0 {
        return update_job(&database, &job, "merged", None).await;
    }
    let Some((base, head)) = pull
        .snapshot
        .base_commit
        .as_deref()
        .zip(pull.snapshot.head_commit.as_deref())
    else {
        return fail(&database, &job, "stale").await;
    };
    if job.comparison_version != pull.snapshot.comparison_version
        || job.base_branch != pull.base_branch
        || !valid_ref(&job.base_branch)
    {
        return fail(&database, &job, "stale").await;
    }
    let target_handle = RepoHandle::open(env, &target).await?;
    let reference = format!("refs/heads/{}", job.base_branch);
    let actual_base = target_handle.head_commit(&reference).await?;
    if job.state == "running" {
        if actual_base
            .as_ref()
            .is_some_and(|commit| commit.hash != base)
        {
            if let Ok(tip) =
                pull_review::integrated_tip(&target_handle, &job.base_branch, base, head).await?
            {
                return finish(env, &database, &job, &target, &issue, &pull, &tip).await;
            }
            return fail(&database, &job, "stale").await;
        }
        return update_job(&database, &job, "queued", None).await;
    }
    let Some(actor) = db::first::<Actor>(
        &database,
        "SELECT id, is_admin FROM users WHERE id = ?",
        &[job.actor_id.map_or_else(util::js_null, util::js_int)],
    )
    .await?
    else {
        return fail(&database, &job, "access").await;
    };
    if !issue.is_open()
        || repo_role(
            &database,
            target.id,
            target.owner_id,
            actor.id,
            actor.is_admin,
        )
        .await?
            != Some(RepoRole::Write)
    {
        return fail(&database, &job, "access").await;
    }
    let source_id = pull.source_repo_id(target.id);
    let source = if source_id != target.id {
        let Some(source) = load_repo_by_id(&database, source_id).await? else {
            return fail(&database, &job, "access").await;
        };
        if source.is_private()
            && (!target.is_private()
                || repo_role(
                    &database,
                    source.id,
                    source.owner_id,
                    actor.id,
                    actor.is_admin,
                )
                .await?
                .is_none())
        {
            return fail(&database, &job, "access").await;
        }
        Some((RepoHandle::open(env, &source).await?, source.storage_id))
    } else {
        None
    };
    let source_handle = source.as_ref().map_or(&target_handle, |(handle, _)| handle);
    let actual_head = source_handle
        .head_commit(&format!("refs/heads/{}", pull.head_branch))
        .await?;
    if actual_base
        .as_ref()
        .is_none_or(|commit| commit.hash != base)
        || actual_head
            .as_ref()
            .is_none_or(|commit| commit.hash != head)
    {
        return fail(&database, &job, "stale").await;
    }
    let claimed_at = util::now();
    let claimed = database
        .prepare(CLAIM)
        .bind(&[
            util::js_int(job.issue_id),
            util::js_int(job.comparison_version),
            util::js_text(head),
            util::js_int(claimed_at),
            util::js_int(actor.id),
            util::js_int(job.changed_at),
        ])?
        .first::<i64>(Some("issue_id"))
        .await?;
    let Some(_) = claimed else {
        return update_job(&database, &job, "queued", Some("waiting")).await;
    };
    let result = target_handle
        .merge_saved(
            env,
            &reference,
            base,
            head,
            pull.snapshot.merge_base.as_deref() == Some(base),
            source.as_ref().map(|(handle, id)| (handle, id.as_str())),
        )
        .await;
    let running = QueueEntry {
        state: "running".into(),
        changed_at: claimed_at,
        ..job
    };
    match result {
        Ok(tip) => finish(env, &database, &running, &target, &issue, &pull, &tip).await,
        Err(WriteError::Unavailable) => Ok(()), // The origin may already have advanced the ref. Recover after the claim expires.
        Err(WriteError::Conflict) => fail(&database, &running, "conflict").await,
        Err(WriteError::Unsupported) => fail(&database, &running, "unsupported").await,
        Err(WriteError::Missing | WriteError::Stale) => fail(&database, &running, "stale").await,
        Err(WriteError::Rejected) => fail(&database, &running, "storage").await,
    }
}

// Match Native branch rules with the same NOCASE semantics as push admission.
// NTFS/APFS repositories may fold ref names, so a case-only rule mismatch must
// never let the merge queue skip approvals or checks.
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
   AND NOT EXISTS (
       SELECT 1 FROM repo_branch_rules b JOIN repos rule_repo ON rule_repo.id = b.repo_id
       WHERE b.repo_id = merge_queue.repo_id
         AND ((rule_repo.git_backend = 'native' AND b.branch = merge_queue.base_branch COLLATE NOCASE)
              OR b.branch = merge_queue.base_branch)
         AND (b.approvals > (SELECT COUNT(*) FROM latest_reviews WHERE decision = 'approved')
          OR EXISTS (SELECT 1 FROM latest_reviews WHERE decision = 'changes_requested')
          OR EXISTS (SELECT 1 FROM json_each(b.checks) c WHERE NOT EXISTS (
              SELECT 1 FROM commit_checks k WHERE k.repo_id = b.repo_id AND k.commit_hash = ?3
                  AND k.context = c.value AND k.state = 'success'))))
 RETURNING issue_id";

async fn finish(
    env: &Env,
    database: &D1Database,
    job: &QueueEntry,
    repo: &Repo,
    issue: &Issue,
    pull: &Pull,
    tip: &str,
) -> Result<()> {
    let Some(base) = pull.snapshot.base_commit.as_deref() else {
        return fail(database, job, "stale").await;
    };
    if pull_review::record_observed_merge(database, repo, issue, pull, job.actor_id, tip, false)
        .await?
        .is_some()
    {
        update_job(database, job, "merged", None).await?;
        let update = RefUpdate {
            name: format!("refs/heads/{}", job.base_branch),
            before: base.into(),
            after: tip.into(),
        };
        git_activity::record(env, repo.id, job.actor_id, &[update]).await;
    } else if pull_review::load_pull(database, issue.id)
        .await?
        .is_some_and(|current| current.merged != 0)
    {
        update_job(database, job, "merged", None).await?;
    } else {
        fail(database, job, "stale").await?;
    }
    Ok(())
}

async fn update_job(
    database: &D1Database,
    job: &QueueEntry,
    state: &str,
    reason: Option<&str>,
) -> Result<()> {
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
        "checks" => BranchText::MergeChecks,
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
    visible_repo(
        &ctx.data,
        ctx.param("owner").map_or("", String::as_str),
        ctx.param("repo").map_or("", String::as_str),
    )
    .await
}

fn missing(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(CommonText::RepositoryNotFound))
}

fn error(state: &AppState, status: u16, message: BranchText) -> Result<Response> {
    error_response(&Chrome::new(state), status, state.locale.t(message))
}
