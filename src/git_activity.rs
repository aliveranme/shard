use crate::db;
use crate::git_storage::RepoHandle;
use crate::git_wire::{RefUpdate, ZERO_OID};
use crate::handlers::common::load_repo_by_id;
use crate::refs::{self, Inventory};
use crate::util::{js_int, js_null, js_text, now};
use serde::Deserialize;
use worker::d1::{D1Database, D1PreparedStatement};
use worker::{Env, Error, Result};

const CHECK_DELAY: i64 = 60;
const RETRY_DELAY: i64 = 300;
const ADMISSION_TTL: i64 = 300;
const INTENT_TTL: i64 = 3600;

#[derive(Deserialize)]
pub(crate) struct PushIntent {
    id: i64,
    repo_id: i64,
    updates: String,
    expires_at: i64,
}

impl PushIntent {
    pub(crate) fn id(&self) -> i64 {
        self.id
    }
}

fn observable(update: &RefUpdate) -> bool {
    update.before != update.after
        && (update.name.starts_with("refs/heads/") || update.name.starts_with("refs/tags/"))
        && !update.name.ends_with('/')
        && !update.name.bytes().any(|byte| byte <= 32 || byte == 127)
}

pub(crate) enum Admission {
    Blocked,
    Admitted(Option<PushIntent>),
}

// Both paths bind the proposed updates as ?2 and the deletion OID as ?3.
macro_rules! writable_refs {
    () => {
        "NOT EXISTS (
            SELECT 1 FROM json_each(?2) u
            LEFT JOIN repo_branch_rules b ON b.repo_id = r.id AND b.branch = substr(json_extract(u.value, '$.name'), 12)
            WHERE json_extract(u.value, '$.name') GLOB 'refs/heads/*'
              AND (b.repo_id IS NOT NULL OR (json_extract(u.value, '$.after') = ?3
                   AND r.default_branch = substr(json_extract(u.value, '$.name'), 12)))
        )"
    };
}

pub(crate) async fn blocks_push(
    database: &D1Database,
    repo_id: i64,
    updates: &[RefUpdate],
) -> Result<bool> {
    let branches: Vec<_> = updates
        .iter()
        .filter(|update| observable(update) && update.name.starts_with("refs/heads/"))
        .collect();
    if branches.is_empty() {
        return Ok(false);
    }
    Ok(database
        .prepare(concat!(
            "SELECT id FROM repos r WHERE r.id = ?1 AND ",
            writable_refs!()
        ))
        .bind(&[
            js_int(repo_id),
            js_text(&serde_json::to_string(&branches)?),
            js_text(ZERO_OID),
        ])?
        .first::<i64>(Some("id"))
        .await?
        .is_none())
}

pub(crate) async fn stage(
    database: &D1Database,
    repo_id: i64,
    updates: &[RefUpdate],
    require_admin: Option<i64>,
) -> Result<Admission> {
    let updates: Vec<_> = updates.iter().filter(|update| observable(update)).collect();
    if updates.is_empty() {
        return Ok(Admission::Admitted(None));
    }
    let time = now();
    let expires_at = time + INTENT_TTL;
    let admitted_until = if updates
        .iter()
        .any(|update| update.name.starts_with("refs/heads/"))
    {
        time + ADMISSION_TTL
    } else {
        0
    };
    let updates = serde_json::to_string(&updates)?;
    let id = database.prepare(concat!(
        "INSERT INTO git_push_intents (repo_id, updates, check_at, expires_at, admitted_until)
         SELECT r.id, ?2, ?4, ?5, ?6 FROM repos r WHERE r.id = ?1
          AND (?7 IS NULL OR EXISTS (SELECT 1 FROM users WHERE id = ?7 AND (id = r.owner_id OR is_admin = 1)))
          AND ", writable_refs!(), " RETURNING id",
    )).bind(&[js_int(repo_id), js_text(&updates), js_text(ZERO_OID), js_int(time + CHECK_DELAY),
              js_int(expires_at), js_int(admitted_until), require_admin.map_or_else(js_null, js_int)])?
        .first::<i64>(Some("id")).await?;
    Ok(match id {
        Some(id) => Admission::Admitted(Some(PushIntent {
            id,
            repo_id,
            updates,
            expires_at,
        })),
        None => Admission::Blocked,
    })
}

pub(crate) async fn confirm(
    env: &Env,
    mut intent: PushIntent,
    actor: Option<i64>,
    accepted: &[String],
) -> Result<()> {
    let database = env.d1("DB")?;
    // Reconciliation may settle some refs while an acknowledgement is arriving.
    for _ in 0..2 {
        let updates: Vec<RefUpdate> = serde_json::from_str(&intent.updates)?;
        let confirmed: Vec<_> = updates
            .iter()
            .filter(|update| accepted.contains(&update.name))
            .collect();
        match settle(&database, &intent, actor, &confirmed, None, false).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                // Git has finished. Keep observation work for cron and release the admission.
                let _ = db::run(
                    &database,
                    "UPDATE git_push_intents SET admitted_until = 0 WHERE id = ?",
                    &[js_int(intent.id)],
                )
                .await;
                return Err(error);
            }
        }
        let Some(current) = db::first(
            &database,
            "SELECT id, repo_id, updates, expires_at FROM git_push_intents WHERE id = ?",
            &[js_int(intent.id)],
        )
        .await?
        else {
            return Ok(());
        };
        intent = current;
    }
    Err(Error::RustError(
        "Git push confirmation raced with reconciliation".into(),
    ))
}

pub(crate) async fn confirm_direct(
    env: &Env,
    repo_id: i64,
    id: i64,
    actor: i64,
    accepted: &[String],
) -> Result<()> {
    let database = env.d1("DB")?;
    let intent = db::first(&database,
        "SELECT id, repo_id, updates, expires_at FROM git_push_intents WHERE id = ?1 AND repo_id = ?2",
        &[js_int(id), js_int(repo_id)],
    ).await?;
    if let Some(intent) = intent {
        confirm(env, intent, Some(actor), accepted).await?;
    }
    Ok(())
}

pub(crate) async fn release_admission(env: &Env, repo_id: i64, id: i64) {
    if let Ok(database) = env.d1("DB") {
        let _ = db::run(
            &database,
            "UPDATE git_push_intents SET admitted_until = 0, check_at = MIN(check_at, ?1) WHERE id = ?2 AND repo_id = ?3",
            &[js_int(now()), js_int(id), js_int(repo_id)],
        ).await;
    }
}

pub(crate) async fn record(env: &Env, repo_id: i64, actor: Option<i64>, updates: &[RefUpdate]) {
    let updates: Vec<_> = updates.iter().filter(|update| observable(update)).collect();
    if !updates.is_empty() && publish(env, repo_id, actor, &updates).await.is_err() {
        worker::console_log!("event=git_ref_observation_failed repo_id={repo_id}");
    }
}

async fn publish(
    env: &Env,
    repo_id: i64,
    actor: Option<i64>,
    updates: &[&RefUpdate],
) -> Result<()> {
    observations(&env.d1("DB")?, repo_id, actor, updates, None, false)?
        .run()
        .await?;
    Ok(())
}

fn observations(
    database: &D1Database,
    repo_id: i64,
    actor: Option<i64>,
    updates: &[&RefUpdate],
    intent: Option<&PushIntent>,
    recovered: bool,
) -> Result<D1PreparedStatement> {
    let json = serde_json::to_string(updates)?;
    database.prepare(
        "INSERT INTO git_ref_observations (repo_id, name, before_oid, after_oid, actor_id, observed_at, recovered)
         SELECT ?1, json_extract(j.value, '$.name'), NULLIF(json_extract(j.value, '$.before'), ?7),
                NULLIF(json_extract(j.value, '$.after'), ?7), ?2, ?3, ?8
         FROM json_each(?4) AS j
         WHERE ?5 IS NULL OR EXISTS (SELECT 1 FROM git_push_intents WHERE id = ?5 AND updates = ?6)",
    ).bind(&[
        js_int(repo_id), actor.map_or_else(js_null, js_int), js_int(now()), js_text(&json),
        intent.map_or_else(js_null, |intent| js_int(intent.id)),
        intent.map_or_else(js_null, |intent| js_text(&intent.updates)), js_text(ZERO_OID), js_int(i64::from(recovered)),
    ])
}

async fn settle(
    database: &D1Database,
    intent: &PushIntent,
    actor: Option<i64>,
    updates: &[&RefUpdate],
    remaining: Option<&str>,
    recovered: bool,
) -> Result<bool> {
    let mut statements = Vec::with_capacity(2);
    if !updates.is_empty() {
        statements.push(observations(
            database,
            intent.repo_id,
            actor,
            updates,
            Some(intent),
            recovered,
        )?);
    }
    let args = [js_int(intent.id), js_text(&intent.updates)];
    let finish = if let Some(remaining) = remaining {
        database
            .prepare("UPDATE git_push_intents SET updates = ?3 WHERE id = ?1 AND updates = ?2")
            .bind(&[args[0].clone(), args[1].clone(), js_text(remaining)])?
    } else {
        database
            .prepare("DELETE FROM git_push_intents WHERE id = ?1 AND updates = ?2")
            .bind(&args)?
    };
    statements.push(finish);
    let result = database
        .batch(statements)
        .await?
        .pop()
        .ok_or_else(|| Error::RustError("Missing Git push settlement result".into()))?;
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes)
        .is_some_and(|changes| changes != 0))
}

pub(crate) async fn reconcile(env: &Env, limit: i64) -> Result<()> {
    let database = env.d1("DB")?;
    let time = now();
    let intents = db::all::<PushIntent>(&database,
        "UPDATE git_push_intents SET check_at = ?2 WHERE id IN (
             SELECT id FROM git_push_intents WHERE check_at <= ?1 AND admitted_until <= ?1 ORDER BY check_at, id LIMIT ?3
         ) RETURNING id, repo_id, updates, expires_at",
        &[js_int(time), js_int(time + RETRY_DELAY), js_int(limit)],
    ).await?;
    for intent in intents {
        if reconcile_intent(env, &database, &intent, time)
            .await
            .is_err()
        {
            worker::console_log!("event=git_push_reconcile_failed intent_id={}", intent.id);
        }
    }
    Ok(())
}

async fn reconcile_intent(
    env: &Env,
    database: &D1Database,
    intent: &PushIntent,
    time: i64,
) -> Result<()> {
    let Some(repo) = load_repo_by_id(database, intent.repo_id).await? else {
        return Ok(());
    };
    let expired = time >= intent.expires_at;
    let receive_inventory = async {
        let handle = RepoHandle::open(env, &repo).await?;
        refs::load(&handle)
            .await
            .map_err(|_| Error::RustError("Unable to observe refs for an unfinished push".into()))
    };
    let inventory = match receive_inventory.await {
        Ok(inventory) => inventory,
        Err(_) if expired => return expire(database, intent, &[]).await,
        Err(error) => return Err(error),
    };
    let updates: Vec<RefUpdate> = serde_json::from_str(&intent.updates)?;
    let (observed, remaining): (Vec<_>, Vec<_>) = updates
        .iter()
        .partition(|update| ref_oid(&inventory, &update.name).unwrap_or(ZERO_OID) == update.after);
    if expired && !remaining.is_empty() {
        expire(database, intent, &observed).await?;
    } else if !observed.is_empty() {
        let remaining = (!remaining.is_empty())
            .then(|| serde_json::to_string(&remaining))
            .transpose()?;
        settle(
            database,
            intent,
            None,
            &observed,
            remaining.as_deref(),
            true,
        )
        .await?;
    }
    Ok(())
}

async fn expire(database: &D1Database, intent: &PushIntent, observed: &[&RefUpdate]) -> Result<()> {
    if settle(database, intent, None, observed, None, true).await? {
        worker::console_log!("event=git_push_intent_expired intent_id={}", intent.id);
    }
    Ok(())
}

fn ref_oid<'a>(inventory: &'a Inventory, name: &str) -> Option<&'a str> {
    if let Some(name) = name.strip_prefix("refs/heads/") {
        inventory
            .branches
            .iter()
            .find(|branch| branch.name == name)
            .map(|branch| branch.oid.as_str())
    } else {
        let name = name.strip_prefix("refs/tags/")?;
        inventory
            .tags
            .iter()
            .find(|tag| tag.name == name)
            .map(|tag| tag.oid.as_str())
    }
}
