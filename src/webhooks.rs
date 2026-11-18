//! Scheduled delivery of committed issue and review events.

use crate::{db, util};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use worker::{Env, Result, Url};

const BATCH_SIZE: usize = 8;
const MAX_ATTEMPTS: i64 = 6;
const LEASE_SECONDS: i64 = 30;
const FIRST_RETRY_SECONDS: i64 = 60;

#[wasm_bindgen(inline_js = r#"
const encoder = new TextEncoder();

export function shard_webhook_key(secret) {
  return crypto.subtle.importKey(
    "raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]
  );
}

export async function shard_webhook_post(endpoint, key, id, json) {
  const body = encoder.encode(json);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, body));
  const hex = Array.from(mac, byte => byte.toString(16).padStart(2, "0")).join("");
  const response = await fetch(endpoint, {
    method: "POST",
    redirect: "manual",
    signal: AbortSignal.timeout(5000),
    headers: {
      "Content-Type": "application/json",
      "X-Shard-Event-Id": id,
      "X-Shard-Signature": `sha256=${hex}`,
    },
    body,
  });
  void response.body?.cancel().catch(() => {});
  return response.status;
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_webhook_key(secret: &str) -> std::result::Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_post(
        endpoint: &str,
        key: &JsValue,
        id: &str,
        json: &str,
    ) -> std::result::Result<u16, JsValue>;
}

#[derive(Deserialize)]
struct RepoId {
    id: i64,
}

#[derive(Deserialize)]
struct Delivery {
    event_id: i64,
    issue_number: i64,
    kind: String,
    action: String,
    actor_id: Option<i64>,
    created_at: i64,
    attempts: i64,
}

#[derive(Serialize)]
struct Payload<'a> {
    version: u8,
    event_id: i64,
    repo: &'a str,
    issue_number: i64,
    kind: &'a str,
    action: &'a str,
    actor_id: Option<i64>,
    created_at: i64,
}

fn endpoint(raw: &str, local_http: bool) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if url.scheme() == "https" {
        return Some(url);
    }
    if !local_http || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]")) {
        return None;
    }
    // URL parsers normalize aliases such as 127.1 and hexadecimal IPv4.
    // Permit only the literal forms in the simulator configuration.
    let authority = raw.strip_prefix("http://")?.split(['/', '?']).next()?;
    for literal in ["127.0.0.1", "[::1]"] {
        if authority == literal
            || authority
                .strip_prefix(literal)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Some(url);
        }
    }
    None
}

pub(crate) async fn deliver_pending(env: &Env) -> Result<()> {
    let (Ok(url), Ok(repo), Ok(secret)) = (
        env.var("WEBHOOK_ENDPOINT"),
        env.var("WEBHOOK_REPO"),
        env.secret("WEBHOOK_SECRET"),
    ) else {
        return Ok(());
    };
    let local_http = env.var("WEBHOOK_LOCAL_HTTP").is_ok_and(|v| v.to_string() == "1");
    let repo = repo.to_string();
    let secret = secret.to_string();
    let url = endpoint(&url.to_string(), local_http)
        .filter(|_| !repo.is_empty() && !secret.is_empty())
        .ok_or_else(|| worker::Error::RustError("invalid webhook configuration".into()))?;

    let database = env.d1("DB")?;
    // Resolve one exact match. Ambiguous identifiers must never authorize a send.
    let repos = db::all::<RepoId>(
        &database,
        "SELECT id FROM repos WHERE artifacts_repo = ?1 LIMIT 2",
        &[util::js_text(&repo)],
    )
    .await?;
    let [scope] = repos.as_slice() else {
        return Ok(());
    };
    let repo_id = util::js_int(scope.id);
    let key = shard_webhook_key(&secret)
        .await
        .map_err(|_| worker::Error::RustError("webhook signing key import failed".into()))?;
    let max_attempts = util::js_int(MAX_ATTEMPTS);

    // Finalize abandoned final attempts in a bounded sweep. They may have reached
    // the receiver before the Worker stopped, so leave the HTTP status unknown.
    db::run(
        &database,
        "UPDATE webhook_deliveries SET state = 'failed', finished_at = ?2
          WHERE event_id IN (
            SELECT event_id FROM webhook_deliveries
             WHERE repo_id = ?1 AND next_attempt_at <= ?2
               AND state = 'sending' AND attempts >= ?3
             ORDER BY next_attempt_at, event_id LIMIT ?4
          )",
        &[
            repo_id.clone(),
            util::js_int(util::now()),
            max_attempts.clone(),
            util::js_int(BATCH_SIZE as i64),
        ],
    )
    .await?;

    for _ in 0..BATCH_SIZE {
        let now = util::now();
        // Claim one event atomically before sending. An expired lease can be
        // reclaimed, so use the attempt number to fence stale completions.
        let Some(event) = db::first::<Delivery>(
            &database,
            "UPDATE webhook_deliveries
                SET state = 'sending', attempts = attempts + 1, next_attempt_at = ?3,
                    last_status = NULL
              WHERE event_id = (
                SELECT event_id FROM webhook_deliveries
                 WHERE repo_id = ?1 AND next_attempt_at <= ?2
                   AND state IN ('pending', 'sending') AND attempts < ?4
                 ORDER BY next_attempt_at, event_id LIMIT 1
              )
              RETURNING event_id, issue_number, kind, action, actor_id, created_at, attempts",
            &[
                repo_id.clone(),
                util::js_int(now),
                util::js_int(now + LEASE_SECONDS),
                max_attempts.clone(),
            ],
        )
        .await?
        else {
            break;
        };
        let body = serde_json::to_string(&Payload {
            version: 1,
            event_id: event.event_id,
            repo: &repo,
            issue_number: event.issue_number,
            kind: &event.kind,
            action: &event.action,
            actor_id: event.actor_id,
            created_at: event.created_at,
        })
        .map_err(|_| worker::Error::RustError("webhook serialization failed".into()))?;
        let status = shard_webhook_post(url.as_str(), &key, &event.event_id.to_string(), &body)
            .await
            .ok();
        let now = util::now();
        let (state, next_attempt, finished_at) = if matches!(status, Some(200..=299)) {
            ("sent", now, util::js_int(now))
        } else if event.attempts >= MAX_ATTEMPTS {
            ("failed", now, util::js_int(now))
        } else {
            let delay = FIRST_RETRY_SECONDS << (event.attempts - 1);
            ("pending", now + delay, util::js_null())
        };
        db::run(
            &database,
            "UPDATE webhook_deliveries SET state = ?3, next_attempt_at = ?4,
                    finished_at = ?5, last_status = ?6
              WHERE event_id = ?1 AND attempts = ?2 AND state = 'sending'",
            &[
                util::js_int(event.event_id),
                util::js_int(event.attempts),
                util::js_text(state),
                util::js_int(next_attempt),
                finished_at,
                status
                    .filter(|&status| status != 0)
                    .map_or_else(util::js_null, |status| util::js_int(i64::from(status))),
            ],
        )
        .await?;
    }
    Ok(())
}
