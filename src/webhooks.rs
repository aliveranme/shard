//! Repository webhook settings and scheduled delivery of committed events.

use crate::ctx::AppState;
use crate::handlers::common::{can_admin, forbidden, not_found, page_title, visible_repo};
use crate::i18n::{CommonText, RepoText, WebhookText as Text};
use crate::models::{Repo, Ts};
use crate::templates::{render, Chrome};
use crate::{db, util};
use askama::Template;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use worker::d1::D1Database;
use worker::{Env, Request, Response, Result, RouteContext, Url};

const MAX_SUBSCRIPTIONS: i64 = 10;
const RECENT_DELIVERIES: i64 = 5;
const RETENTION_SECONDS: i64 = 30 * 86_400;
const CLEANUP_LIMIT: i64 = 64;
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

export function shard_webhook_encryption_key(hex) {
  const raw = Uint8Array.from(hex.match(/../g), pair => parseInt(pair, 16));
  return crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]);
}

function encryptionParameters(repoId, endpoint, iv) {
  return { name: "AES-GCM", iv, additionalData: encoder.encode(`shard-webhook-v1:${repoId}:${endpoint}`) };
}

export async function shard_webhook_seal(key, repoId, endpoint, secret) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
    encryptionParameters(repoId, endpoint, iv), key, encoder.encode(secret)
  ));
  return btoa(String.fromCharCode(...iv, ...ciphertext));
}

export async function shard_webhook_unseal(key, repoId, endpoint, sealed) {
  const bytes = Uint8Array.from(atob(sealed), char => char.charCodeAt(0));
  const raw = await crypto.subtle.decrypt(
    encryptionParameters(repoId, endpoint, bytes.subarray(0, 12)), key, bytes.subarray(12)
  );
  return crypto.subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
}

export async function shard_webhook_sign(key, json) {
  const body = encoder.encode(json);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, body));
  const hex = Array.from(mac, byte => byte.toString(16).padStart(2, "0")).join("");
  return { body, signature: `sha256=${hex}` };
}

export async function shard_webhook_post_signed(endpoint, id, deliveryId, { body, signature }) {
  const response = await fetch(endpoint, {
    method: "POST",
    redirect: "manual",
    signal: AbortSignal.timeout(5000),
    headers: {
      "Content-Type": "application/json",
      "X-Shard-Event-Id": id,
      "X-Shard-Signature": signature,
      ...(deliveryId ? { "X-Shard-Delivery-Id": deliveryId } : {}),
    },
    body,
  });
  void response.body?.cancel().catch(() => {});
  return response.status;
}

export async function shard_webhook_post(endpoint, key, id, deliveryId, json) {
  const signed = await shard_webhook_sign(key, json);
  return shard_webhook_post_signed(endpoint, id, deliveryId, signed);
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_webhook_key(secret: &str) -> std::result::Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_encryption_key(hex: &str) -> std::result::Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_seal(
        key: &JsValue,
        repo_id: &str,
        endpoint: &str,
        secret: &str,
    ) -> std::result::Result<String, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_unseal(
        key: &JsValue,
        repo_id: &str,
        endpoint: &str,
        sealed: &str,
    ) -> std::result::Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_post(
        endpoint: &str,
        key: &JsValue,
        id: &str,
        delivery_id: &str,
        json: &str,
    ) -> std::result::Result<u16, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_sign(key: &JsValue, json: &str)
        -> std::result::Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn shard_webhook_post_signed(
        endpoint: &str,
        id: &str,
        delivery_id: &str,
        signed: &JsValue,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_number: Option<i64>,
    kind: &'a str,
    action: &'a str,
    actor_id: Option<i64>,
    created_at: i64,
    #[serde(flatten)]
    reference: Option<RefPayload<'a>>,
}

#[derive(Serialize)]
struct RefPayload<'a> {
    #[serde(rename = "ref")]
    name: &'a str,
    before: Option<&'a str>,
    after: Option<&'a str>,
}

fn legacy_endpoint(raw: &str, local_http: bool) -> Option<Url> {
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
                .is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
                })
        {
            return Some(url);
        }
    }
    None
}

async fn deliver_legacy(env: &Env, database: &D1Database, limit: usize) -> Result<()> {
    let (Ok(url), Ok(repo), Ok(secret)) = (
        env.var("WEBHOOK_ENDPOINT"),
        env.var("WEBHOOK_REPO"),
        env.secret("WEBHOOK_SECRET"),
    ) else {
        return Ok(());
    };
    let local_http = env
        .var("WEBHOOK_LOCAL_HTTP")
        .is_ok_and(|v| v.to_string() == "1");
    let repo = repo.to_string();
    let secret = secret.to_string();
    let url = legacy_endpoint(&url.to_string(), local_http)
        .filter(|_| !repo.is_empty() && !secret.is_empty())
        .ok_or_else(|| worker::Error::RustError("invalid webhook configuration".into()))?;

    // Resolve one exact match. Ambiguous identifiers must never authorize a send.
    let repos = db::all::<RepoId>(
        database,
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
        database,
        "UPDATE webhook_deliveries SET state = 'failed', finished_at = ?2
          WHERE event_id IN (
            SELECT event_id FROM webhook_deliveries
             WHERE repo_id = ?1 AND next_attempt_at <= ?2
               AND state IN ('pending', 'sending') AND state = 'sending' AND attempts >= ?3
             ORDER BY next_attempt_at, event_id LIMIT ?4
          )",
        &[
            repo_id.clone(),
            util::js_int(util::now()),
            max_attempts.clone(),
            util::js_int(limit as i64),
        ],
    )
    .await?;

    for _ in 0..limit {
        let now = util::now();
        // Claim one event atomically before sending. An expired lease can be
        // reclaimed, so use the attempt number to fence stale completions.
        let Some(event) = db::first::<Delivery>(
            database,
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
            issue_number: Some(event.issue_number),
            kind: &event.kind,
            action: &event.action,
            actor_id: event.actor_id,
            created_at: event.created_at,
            reference: None,
        })
        .map_err(|_| worker::Error::RustError("webhook serialization failed".into()))?;
        let status = shard_webhook_post(url.as_str(), &key, &event.event_id.to_string(), "", &body)
            .await
            .ok();
        let now = util::now();
        let (state, next_attempt) = delivery_result(status, event.attempts, now);
        let finished_at = if state == "pending" {
            util::js_null()
        } else {
            util::js_int(now)
        };
        db::run(
            database,
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

fn encryption_secret(env: &Env) -> Option<String> {
    let key = env.secret("WEBHOOK_ENCRYPTION_KEY").ok()?.to_string();
    (key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(key)
}

fn subscription_endpoint(raw: &str, env: &Env) -> Option<Url> {
    let (_, suffix) = raw.split_once("://")?;
    let authority = suffix.split(['/', '?', '#']).next()?;
    if raw.len() > 2048
        || authority.is_empty()
        || authority.contains('@')
        || raw.contains('\\')
        || raw
            .chars()
            .any(|char| char.is_whitespace() || char.is_control())
    {
        return None;
    }
    let url = legacy_endpoint(raw, true)?;
    if url.as_str().len() > 2048 {
        return None;
    }
    if url.scheme() == "http" {
        if !env
            .var("WEBHOOK_LOCAL_HTTP")
            .is_ok_and(|var| var.to_string() == "1")
        {
            return None;
        }
        let configured = env.var("WEBHOOK_LOCAL_HTTP_ORIGIN").ok()?.to_string();
        let origin = legacy_endpoint(&configured, true)?;
        return (origin.scheme() == "http" && origin.origin() == url.origin()).then_some(url);
    }
    // Worker fetch uses public egress. Reject IP literals and special-use names
    // locally, and leave redirects disabled for every delivery and retry.
    let host = url.host_str()?.trim_end_matches('.');
    let (_, suffix) = host.rsplit_once('.')?;
    if url.port().is_some()
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        || !(suffix.bytes().all(|byte| byte.is_ascii_alphabetic()) || suffix.starts_with("xn--"))
        || matches!(
            suffix,
            "alt"
                | "arpa"
                | "corp"
                | "example"
                | "home"
                | "internal"
                | "invalid"
                | "lan"
                | "local"
                | "localdomain"
                | "localhost"
                | "onion"
                | "svc"
                | "test"
        )
    {
        return None;
    }
    Some(url)
}

#[derive(Deserialize)]
struct Subscription {
    id: i64,
    label: String,
    endpoint_origin: String,
    disabled_at: Option<i64>,
    #[serde(skip)]
    recent: Vec<RecentDelivery>,
}

#[derive(Deserialize)]
struct RecentDelivery {
    id: i64,
    subscription_id: i64,
    kind: String,
    issue_number: Option<i64>,
    ref_name: Option<String>,
    state: String,
    attempts: i64,
    last_status: Option<i64>,
    created_at: Ts,
}

impl RecentDelivery {
    fn state_text(&self) -> Text {
        match self.state.as_str() {
            "pending" => Text::Pending,
            "sending" => Text::Sending,
            "sent" => Text::Sent,
            "canceled" => Text::Canceled,
            _ => Text::Failed,
        }
    }

    fn kind_text(&self) -> Text {
        match self.kind.as_str() {
            "pull" => Text::PullRequest,
            "tag" => Text::Tag,
            "push" => Text::Push,
            _ => Text::Issue,
        }
    }
}

#[derive(Template)]
#[template(path = "webhooks.html")]
struct WebhookPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    subscriptions: Vec<Subscription>,
    can_create: bool,
    error: Option<&'static str>,
}

const SETTINGS_ACCESS: &str = "r.id = ?1 AND (r.owner_id = u.id OR u.is_admin = 1)";

async fn render_settings(
    state: &AppState,
    repo: Repo,
    error: Option<Text>,
    status: u16,
) -> Result<Response> {
    let database = state.db()?;
    let args = [
        util::js_int(repo.id),
        state.user_id().map_or_else(util::js_null, util::js_int),
    ];
    let mut subscriptions = db::all::<Subscription>(
        &database,
        &format!(
            "SELECT s.id, s.label, s.endpoint_origin, s.disabled_at
               FROM repos r JOIN users u ON u.id = ?2
               JOIN webhook_subscriptions s ON s.repo_id = r.id
              WHERE {SETTINGS_ACCESS} ORDER BY s.id DESC"
        ),
        &args,
    )
    .await?;
    if !subscriptions.is_empty() {
        let recent = db::all::<RecentDelivery>(
            &database,
            &format!(
                "SELECT s.id AS subscription_id, d.id, d.attempts, d.last_status,
                        CASE WHEN s.disabled_at IS NOT NULL AND d.state IN ('pending', 'sending')
                             THEN 'canceled' ELSE d.state END AS state,
                        e.kind, e.issue_number, e.ref AS ref_name, e.created_at
                   FROM repos r JOIN users u ON u.id = ?2
                   JOIN webhook_subscriptions s ON s.repo_id = r.id
                   JOIN webhook_subscription_deliveries d ON d.id IN (
                        SELECT id FROM webhook_subscription_deliveries
                         WHERE repo_id = r.id AND subscription_id = s.id ORDER BY id DESC LIMIT ?3
                   )
                   JOIN webhook_events e ON e.repo_id = d.repo_id AND e.id = d.event_id
                  WHERE {SETTINGS_ACCESS} ORDER BY s.id DESC, d.id DESC"
            ),
            &[
                args[0].clone(),
                args[1].clone(),
                util::js_int(RECENT_DELIVERIES),
            ],
        )
        .await?;
        for delivery in recent {
            if let Some(subscription) = subscriptions
                .iter_mut()
                .find(|s| s.id == delivery.subscription_id)
            {
                subscription.recent.push(delivery);
            }
        }
    }
    let can_create = encryption_secret(&state.env).is_some();
    Ok(render(&WebhookPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::Webhooks), &repo.full_name()]),
        repo,
        tab: "settings",
        is_owner: true,
        subscriptions,
        can_create,
        error: error.map(|message| state.locale.t(message)),
    })?
    .with_status(status))
}

async fn settings_repo(ctx: &RouteContext<AppState>) -> Result<Option<Repo>> {
    visible_repo(
        &ctx.data,
        ctx.param("owner").map_or("", String::as_str),
        ctx.param("repo").map_or("", String::as_str),
    )
    .await
}

pub async fn page(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = settings_repo(&ctx).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    render_settings(state, repo, None, 200).await
}

pub async fn update(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = settings_repo(&ctx).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let Some(user_id) = state.user_id().filter(|_| can_admin(state, &repo)) else {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    };
    let form = state.form()?;
    let database = state.db()?;
    let repo_id = util::js_int(repo.id);
    let user_id = util::js_int(user_id);
    let changed = match util::form_value(form, "action").as_deref() {
        Some("create") => {
            let raw = util::form_value(form, "endpoint").unwrap_or_default();
            let Some(endpoint) = subscription_endpoint(raw.trim(), &state.env) else {
                return render_settings(state, repo, Some(Text::InvalidEndpoint), 400).await;
            };
            let label = util::form_value(form, "label").unwrap_or_default();
            let label = label.trim();
            if label.chars().count() > 100 || label.chars().any(char::is_control) {
                return render_settings(state, repo, Some(Text::InvalidLabel), 400).await;
            }
            let Some(secret) = util::form_value(form, "secret")
                .filter(|value| value.len() <= 1024 && !value.chars().any(char::is_control))
            else {
                return render_settings(state, repo, Some(Text::InvalidSecret), 400).await;
            };
            let Some(key) = encryption_secret(&state.env) else {
                return render_settings(state, repo, Some(Text::KeyUnavailable), 503).await;
            };
            let key = shard_webhook_encryption_key(&key)
                .await
                .map_err(|_| webhook_error("webhook encryption key import failed"))?;
            let sealed = shard_webhook_seal(&key, &repo.id.to_string(), endpoint.as_str(), &secret)
                .await
                .map_err(|_| webhook_error("webhook secret encryption failed"))?;
            db::first::<RepoId>(
                &database,
                &format!(
                    "INSERT INTO webhook_subscriptions (repo_id, endpoint, endpoint_origin, label, encrypted_secret)
                     SELECT r.id, ?3, ?4, ?5, ?6 FROM repos r JOIN users u ON u.id = ?2
                      WHERE {SETTINGS_ACCESS}
                        AND (SELECT COUNT(*) FROM webhook_subscriptions WHERE repo_id = r.id) < ?7
                     ON CONFLICT (repo_id, endpoint) WHERE disabled_at IS NULL DO NOTHING
                     RETURNING id"
                ),
                &[
                    repo_id,
                    user_id,
                    util::js_text(endpoint.as_str()),
                    util::js_text(&endpoint.origin().ascii_serialization()),
                    util::js_text(label),
                    util::js_text(&sealed),
                    util::js_int(MAX_SUBSCRIPTIONS),
                ],
            )
            .await?
        }
        Some(action @ ("disable" | "delete")) => {
            let id = util::form_value(form, "subscription_id")
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|id| (1..=9_007_199_254_740_991).contains(id));
            let Some(id) = id else {
                return not_found(state, state.locale.t(Text::SubscriptionNotFound));
            };
            let sql = if action == "delete" {
                "DELETE FROM webhook_subscriptions"
            } else {
                "UPDATE webhook_subscriptions SET disabled_at = COALESCE(disabled_at, unixepoch()), encrypted_secret = NULL"
            };
            let changed = db::first::<RepoId>(
                &database,
                &format!(
                    "{sql} WHERE repo_id = ?1 AND id = ?3 AND EXISTS (
                        SELECT 1 FROM repos r JOIN users u ON u.id = ?2 WHERE {SETTINGS_ACCESS}
                     ) RETURNING id"
                ),
                &[repo_id.clone(), user_id, util::js_int(id)],
            )
            .await?;
            if changed.is_none() {
                return not_found(state, state.locale.t(Text::SubscriptionNotFound));
            }
            if action == "disable" {
                let _ = db::run(
                    &database,
                    "UPDATE webhook_subscription_deliveries
                        SET state = 'canceled'
                      WHERE repo_id = ?1 AND subscription_id = ?2 AND state IN ('pending', 'sending')",
                    &[repo_id, util::js_int(id)],
                )
                .await;
            }
            changed
        }
        _ => return render_settings(state, repo, Some(Text::InvalidAction), 400).await,
    };
    if changed.is_none() {
        return render_settings(state, repo, Some(Text::CreateConflict), 409).await;
    }
    util::redirect(&format!("/{}/settings/webhooks", repo.full_name()))
}

fn webhook_error(message: &str) -> worker::Error {
    worker::Error::RustError(message.into())
}

#[derive(Deserialize)]
struct ClaimedDelivery {
    id: i64,
    attempts: i64,
}

#[derive(Deserialize)]
struct SubscriptionDelivery {
    repo_id: i64,
    event_id: i64,
    repo: String,
    issue_number: Option<i64>,
    kind: String,
    action: String,
    actor_id: Option<i64>,
    created_at: i64,
    ref_name: Option<String>,
    before_oid: Option<String>,
    after_oid: Option<String>,
    endpoint: String,
    encrypted_secret: String,
}

async fn clean_subscriptions(database: &D1Database) -> Result<()> {
    let now = util::now();
    let limit = util::js_int(CLEANUP_LIMIT);
    database.batch(vec![
        database.prepare(
            "DELETE FROM webhook_events WHERE id IN (
                 SELECT id FROM webhook_events WHERE created_at < ?1 ORDER BY created_at, id LIMIT ?2
             )",
        ).bind(&[util::js_int(now - RETENTION_SECONDS), limit.clone()])?,
        database.prepare(
            "DELETE FROM webhook_deliveries WHERE event_id IN (
                 SELECT event_id FROM webhook_deliveries WHERE created_at < ?1 ORDER BY created_at, event_id LIMIT ?2
             )",
        ).bind(&[util::js_int(now - RETENTION_SECONDS), limit.clone()])?,
        database.prepare(
            "UPDATE webhook_subscription_deliveries SET state = 'canceled' WHERE id IN (
                 SELECT d.id FROM webhook_subscription_deliveries d
                 JOIN webhook_subscriptions s ON s.repo_id = d.repo_id AND s.id = d.subscription_id
                 WHERE d.state IN ('pending', 'sending') AND s.disabled_at IS NOT NULL
                 ORDER BY d.next_attempt_at, d.id LIMIT ?1
             )",
        ).bind(std::slice::from_ref(&limit))?,
        database.prepare(
            "UPDATE webhook_subscription_deliveries SET state = 'failed', last_status = NULL WHERE id IN (
                 SELECT id FROM webhook_subscription_deliveries
                  WHERE state IN ('pending', 'sending') AND state = 'sending'
                    AND next_attempt_at <= ?1 AND attempts >= ?2 ORDER BY next_attempt_at, id LIMIT ?3
             )",
        ).bind(&[util::js_int(now), util::js_int(MAX_ATTEMPTS), limit])?,
    ]).await?;
    Ok(())
}

enum SubscriptionOutcome {
    Finished(Option<u16>),
    Canceled,
}

async fn try_subscription(
    env: &Env,
    database: &D1Database,
    encryption_key: &JsValue,
    claim: &ClaimedDelivery,
    event: SubscriptionDelivery,
) -> Result<SubscriptionOutcome> {
    let Some(endpoint) = subscription_endpoint(&event.endpoint, env) else {
        return Ok(SubscriptionOutcome::Finished(None));
    };
    let key = shard_webhook_unseal(
        encryption_key,
        &event.repo_id.to_string(),
        endpoint.as_str(),
        &event.encrypted_secret,
    )
    .await;
    let Ok(key) = key else {
        return Ok(SubscriptionOutcome::Finished(None));
    };
    let body = serde_json::to_string(&Payload {
        version: 1,
        event_id: event.event_id,
        repo: &event.repo,
        issue_number: event.issue_number,
        kind: &event.kind,
        action: &event.action,
        actor_id: event.actor_id,
        created_at: event.created_at,
        reference: event.ref_name.as_deref().map(|name| RefPayload {
            name,
            before: event.before_oid.as_deref(),
            after: event.after_oid.as_deref(),
        }),
    })
    .map_err(|_| webhook_error("webhook serialization failed"))?;
    let Ok(signed) = shard_webhook_sign(&key, &body).await else {
        return Ok(SubscriptionOutcome::Finished(None));
    };
    // A transfer or disable can cancel this claim while its key is being
    // decrypted or the payload is being signed. Check again immediately before fetching.
    let active = db::first::<RepoId>(
        database,
        "SELECT d.id FROM webhook_subscription_deliveries d
           JOIN webhook_subscriptions s ON s.repo_id = d.repo_id AND s.id = d.subscription_id
          WHERE d.id = ?1 AND d.attempts = ?2 AND d.state = 'sending'
            AND d.next_attempt_at > unixepoch() AND s.disabled_at IS NULL",
        &[util::js_int(claim.id), util::js_int(claim.attempts)],
    )
    .await?;
    if active.is_none() {
        return Ok(SubscriptionOutcome::Canceled);
    }
    Ok(SubscriptionOutcome::Finished(
        shard_webhook_post_signed(
            endpoint.as_str(),
            &event.event_id.to_string(),
            &claim.id.to_string(),
            &signed,
        )
        .await
        .ok()
        .filter(|&status| status != 0),
    ))
}

async fn deliver_subscriptions(env: &Env, database: &D1Database, limit: usize) -> Result<()> {
    clean_subscriptions(database).await?;
    let Some(secret) = encryption_secret(env) else {
        return Ok(());
    };
    let key = shard_webhook_encryption_key(&secret)
        .await
        .map_err(|_| webhook_error("webhook encryption key import failed"))?;
    let max_attempts = util::js_int(MAX_ATTEMPTS);
    let deadline = util::now() + 20;
    let max_per_repo = (limit / 4).max(1);
    const MAX_REPO_MS: f64 = 4_500.0;
    let mut repo_counts = std::collections::HashMap::<i64, usize>::new();
    let mut repo_ms = std::collections::HashMap::<i64, f64>::new();
    let mut saturated = Vec::<i64>::new();
    for _ in 0..limit {
        if util::now() >= deadline {
            break;
        }
        let now = util::now();
        let excluded_json = serde_json::to_string(&saturated)
            .map_err(|_| webhook_error("webhook filter serialization failed"))?;
        let Some(claim) = db::first::<ClaimedDelivery>(
            database,
            "UPDATE webhook_subscription_deliveries
                SET state = 'sending', attempts = attempts + 1, next_attempt_at = ?2
              WHERE id = (
                SELECT d.id FROM webhook_subscription_deliveries d
                JOIN webhook_subscriptions s ON s.repo_id = d.repo_id AND s.id = d.subscription_id
                 WHERE d.next_attempt_at <= ?1 AND d.state IN ('pending', 'sending')
                   AND d.attempts < ?3 AND s.disabled_at IS NULL
                   AND d.repo_id NOT IN (SELECT value FROM json_each(?4))
                 ORDER BY d.next_attempt_at, d.id LIMIT 1
              ) RETURNING id, attempts",
            &[
                util::js_int(now),
                util::js_int(now + LEASE_SECONDS),
                max_attempts.clone(),
                util::js_text(&excluded_json),
            ],
        )
        .await?
        else {
            break;
        };
        let id = util::js_int(claim.id);
        let attempts = util::js_int(claim.attempts);
        // Check the lease and active subscription again before accessing its key.
        let Some(event) = db::first::<SubscriptionDelivery>(
            database,
            "SELECT d.repo_id, d.event_id, e.repo, e.issue_number, e.kind, e.action, e.actor_id,
                    e.created_at, e.ref AS ref_name, e.before_oid, e.after_oid, s.endpoint, s.encrypted_secret
               FROM webhook_subscription_deliveries d
               JOIN webhook_events e ON e.repo_id = d.repo_id AND e.id = d.event_id
               JOIN webhook_subscriptions s ON s.repo_id = d.repo_id AND s.id = d.subscription_id
              WHERE d.id = ?1 AND d.attempts = ?2 AND d.state = 'sending'
                AND d.next_attempt_at > ?3 AND s.disabled_at IS NULL",
            &[id.clone(), attempts.clone(), util::js_int(util::now())],
        ).await? else {
            continue;
        };
        let repo_id = event.repo_id;
        let count = repo_counts.entry(repo_id).or_insert(0);
        *count += 1;
        let started = js_sys::Date::now();
        let outcome = try_subscription(env, database, &key, &claim, event).await?;
        let elapsed = (js_sys::Date::now() - started).max(0.0);
        let spent = repo_ms.entry(repo_id).or_insert(0.0);
        *spent += elapsed;
        if (*count >= max_per_repo
            || *spent >= MAX_REPO_MS
            || matches!(outcome, SubscriptionOutcome::Finished(None)))
            && !saturated.contains(&repo_id)
        {
            saturated.push(repo_id);
        }
        let SubscriptionOutcome::Finished(status) = outcome else {
            continue;
        };
        let (state, next_attempt_at) = delivery_result(status, claim.attempts, util::now());
        db::run(
            database,
            "UPDATE webhook_subscription_deliveries SET state = ?3, next_attempt_at = ?4, last_status = ?5
              WHERE id = ?1 AND attempts = ?2 AND state = 'sending'",
            &[
                id,
                attempts,
                util::js_text(state),
                util::js_int(next_attempt_at),
                status.map_or_else(util::js_null, |status| util::js_int(i64::from(status))),
            ],
        ).await?;
    }
    Ok(())
}

fn delivery_result(status: Option<u16>, attempts: i64, now: i64) -> (&'static str, i64) {
    if matches!(status, Some(200..=299)) {
        ("sent", now)
    } else if attempts >= MAX_ATTEMPTS {
        ("failed", now)
    } else {
        ("pending", now + (FIRST_RETRY_SECONDS << (attempts - 1)))
    }
}

pub(crate) async fn deliver_pending(
    env: &Env,
    subscriptions_limit: usize,
    legacy_limit: usize,
) -> Result<()> {
    let database = env.d1("DB")?;
    let subscriptions = deliver_subscriptions(env, &database, subscriptions_limit).await;
    deliver_legacy(env, &database, legacy_limit)
        .await
        .and(subscriptions)
}
