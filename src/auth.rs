use crate::crypto;
use crate::db;
use crate::models::{GitIdentity, User};
use crate::util;
use base64::Engine;
use serde::de::IgnoredAny;
use worker::d1::{D1Database, D1PreparedStatement};
use worker::*;

const SESSION_TTL: i64 = 30 * 24 * 3600;
const TOKEN_USAGE_INTERVAL: i64 = 60;
const DUMMY_PASSWORD_HASH: &str = "pbkdf2_sha256$100000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

/// `SHARD_REGISTRATION=open|closed`; missing or `"open"` allows registration, while `"closed"` or any invalid value closes it.
pub fn registration_open(env: &Env) -> bool {
    match env
        .var("SHARD_REGISTRATION")
        .ok()
        .map(|value| value.to_string())
        .as_deref()
    {
        None | Some("open") => true,
        Some("closed") => false,
        Some(value) => {
            console_log!("event=registration_config_invalid value={value}");
            false
        }
    }
}

pub async fn current_user(env: &Env, req: &Request) -> Result<Option<User>> {
    let Some(token) = util::cookie(req, "shard_session") else {
        return Ok(None);
    };
    let hash = crypto::sha256_hex(&token).await?;
    let database = env.d1("DB")?;
    db::first(
        &database,
        "SELECT u.id, u.username, u.email, u.is_admin, u.created_at, u.locale, av.user_id AS avatar_id
           FROM sessions s JOIN users u ON u.id = s.user_id
           LEFT JOIN user_avatars av ON av.user_id = u.id
          WHERE s.token_hash = ? AND s.expires_at > ?",
        &[util::js_text(&hash), util::js_int(util::now())],
    )
    .await
}

fn session_insert(
    database: &D1Database,
    user_id: i64,
    password_hash: &str,
    token_hash: &str,
) -> Result<D1PreparedStatement> {
    let now = util::now();
    database
        .prepare(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at)
         SELECT ?1, id, ?3, ?4 FROM users WHERE id = ?2 AND password_hash = ?5 RETURNING user_id",
        )
        .bind(&[
            util::js_text(token_hash),
            util::js_int(user_id),
            util::js_int(now),
            util::js_int(now + SESSION_TTL),
            util::js_text(password_hash),
        ])
}

async fn create_session(
    database: &D1Database,
    user_id: i64,
    password_hash: &str,
) -> Result<Option<String>> {
    let token = crypto::random_token()?;
    let token_hash = crypto::sha256_hex(&token).await?;
    let inserted = session_insert(database, user_id, password_hash, &token_hash)?
        .first::<IgnoredAny>(None)
        .await?;
    Ok(inserted.map(|_| token))
}

pub async fn destroy_session(env: &Env, token: &str) -> Result<()> {
    let hash = crypto::sha256_hex(token).await?;
    let database = env.d1("DB")?;
    db::run(
        &database,
        "DELETE FROM sessions WHERE token_hash = ?",
        &[util::js_text(&hash)],
    )
    .await
}

#[derive(serde::Deserialize)]
struct Credential {
    id: i64,
    password_hash: String,
}

pub(crate) async fn login_session(
    env: &Env,
    login: &str,
    password: &str,
) -> Result<Option<String>> {
    let database = env.d1("DB")?;
    let found: Option<Credential> = db::first(
        &database,
        "SELECT id, password_hash FROM users
         WHERE username = ? COLLATE NOCASE OR email = ? COLLATE NOCASE LIMIT 1",
        &[util::js_text(login), util::js_text(login)],
    )
    .await?;
    let Some(account) = found else {
        // Equalize login latency when the account does not exist.
        let _ = crypto::verify_password(password, DUMMY_PASSWORD_HASH).await;
        return Ok(None);
    };
    if !crypto::verify_password(password, &account.password_hash).await? {
        return Ok(None);
    }
    create_session(&database, account.id, &account.password_hash).await
}

/// Returns `Some(token)` on success and `None` when a concurrent signup
/// claimed the username or email first. The UNIQUE constraint is the
/// authority; a caller that pre-checked availability can still lose this race.
pub async fn register(
    env: &Env,
    username: &str,
    email: &str,
    password: &str,
) -> Result<Option<String>> {
    let hash = crypto::hash_password(password).await?;
    let database = env.d1("DB")?;
    #[derive(serde::Deserialize)]
    struct Registered {
        id: i64,
    }
    let account = match db::first::<Registered>(
        &database,
        "INSERT INTO users (username, email, password_hash, is_admin, created_at)
         VALUES (?, ?, ?, 0, ?) RETURNING id",
        &[
            util::js_text(username),
            util::js_text(email),
            util::js_text(&hash),
            util::js_int(util::now()),
        ],
    )
    .await
    {
        Ok(account) => account,
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("unique constraint failed") =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    let Some(account) = account else {
        return Err(Error::RustError("user insert failed".into()));
    };
    match create_session(&database, account.id, &hash).await? {
        Some(token) => Ok(Some(token)),
        None => Err(Error::RustError(
            "account changed during registration".into(),
        )),
    }
}

/// Resolve HTTP basic (`username:token`) or bearer credentials to a git identity.
pub async fn git_identity(env: &Env, req: &Request) -> Result<Option<GitIdentity>> {
    let Some(header) = util::header(req, "authorization") else {
        return Ok(None);
    };
    let secret = if let Some(rest) = header
        .strip_prefix("Basic ")
        .or_else(|| header.strip_prefix("basic "))
    {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(rest.trim())
            .map_err(|_| Error::RustError("invalid basic credentials".into()))?;
        let decoded = String::from_utf8(decoded)
            .map_err(|_| Error::RustError("invalid basic credentials".into()))?;
        match decoded.split_once(':') {
            Some((_user, token)) if !token.is_empty() => token.to_string(),
            _ => return Ok(None),
        }
    } else if let Some(rest) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    {
        rest.trim().to_string()
    } else {
        return Ok(None);
    };

    let hash = crypto::sha256_hex(&secret).await?;
    let database = env.d1("DB")?;
    let now = util::now();
    #[derive(serde::Deserialize)]
    struct Row {
        id: i64,
        user_id: i64,
        username: String,
        scope: String,
        is_admin: i64,
        last_used_at: Option<i64>,
    }
    let row: Option<Row> = db::first(
        &database,
        "SELECT t.id, t.user_id, t.scope, u.username, u.is_admin, t.last_used_at
           FROM access_tokens t JOIN users u ON u.id = t.user_id
          WHERE t.token_hash = ? AND (t.expires_at IS NULL OR t.expires_at > ?)",
        &[util::js_text(&hash), util::js_int(now)],
    )
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let cutoff = now - TOKEN_USAGE_INTERVAL;
    if row.last_used_at.is_none_or(|last| last <= cutoff) {
        db::run(
            &database,
            "UPDATE access_tokens SET last_used_at = ?1 WHERE id = ?2
               AND (last_used_at IS NULL OR last_used_at <= ?3)",
            &[
                util::js_int(now),
                util::js_int(row.id),
                util::js_int(cutoff),
            ],
        )
        .await?;
    }
    Ok(Some(GitIdentity {
        user_id: row.user_id,
        username: row.username,
        scope: row.scope,
        is_admin: row.is_admin,
    }))
}

/// Remove expired credentials and completed login throttle windows.
pub async fn cleanup_expired(env: &Env) -> Result<()> {
    let database = env.d1("DB")?;
    let now = util::now();
    database
        .batch(vec![
            database
                .prepare("DELETE FROM sessions WHERE expires_at <= ?")
                .bind(&[util::js_int(now)])?,
            database
                .prepare("DELETE FROM access_tokens WHERE expires_at <= ?")
                .bind(&[util::js_int(now)])?,
            database
                .prepare("DELETE FROM login_throttle WHERE window_started <= ?")
                .bind(&[util::js_int(now - 900)])?,
            database
                .prepare(
                    "DELETE FROM account_email_links WHERE expires_at <= ? AND next_send_at <= ?",
                )
                .bind(&[util::js_int(now), util::js_int(now)])?,
        ])
        .await?;
    Ok(())
}

pub async fn login_key(req: &Request, login: &str) -> Result<String> {
    let source = util::header(req, "cf-connecting-ip")
        .map(|raw| util::normalize_client_ip(&raw))
        .unwrap_or_else(|| "unknown".into());
    let target = login.trim().to_lowercase();
    crypto::sha256_hex(&format!("{}:{}:{}", source.len(), source, target)).await
}

pub async fn login_ip_key(req: &Request) -> Result<String> {
    let source = util::header(req, "cf-connecting-ip")
        .map(|raw| util::normalize_client_ip(&raw))
        .unwrap_or_else(|| "unknown".into());
    crypto::sha256_hex(&format!("ip:{}:{}", source.len(), source)).await
}

pub async fn reserve_login_attempt(env: &Env, key: &str) -> Result<Option<i64>> {
    reserve_login_attempt_limit(env, key, 8).await
}

pub async fn reserve_login_attempt_limit(
    env: &Env,
    key: &str,
    max_failures: i64,
) -> Result<Option<i64>> {
    #[derive(serde::Deserialize)]
    struct Window {
        failures: i64,
        window_started: i64,
    }
    let database = env.d1("DB")?;
    let now = util::now();
    let row: Option<Window> = db::first(
        &database,
        "INSERT INTO login_throttle (key_hash, failures, window_started) VALUES (?, 1, ?)
         ON CONFLICT(key_hash) DO UPDATE SET
         failures = CASE WHEN window_started <= ? THEN 1 ELSE MIN(failures + 1, ?) END,
         window_started = CASE WHEN window_started <= ? THEN excluded.window_started ELSE window_started END
         RETURNING failures, window_started",
        &[
            util::js_text(key),
            util::js_int(now),
            util::js_int(now - 900),
            util::js_int(max_failures + 1),
            util::js_int(now - 900),
        ],
    )
    .await?;
    Ok(row
        .filter(|row| row.failures > max_failures && row.window_started + 900 > now)
        .map(|row| (row.window_started + 900 - now).max(1)))
}

pub async fn release_login_attempt(env: &Env, key: &str) -> Result<()> {
    let database = env.d1("DB")?;
    db::run(
        &database,
        "UPDATE login_throttle SET failures = MAX(failures - 1, 0) WHERE key_hash = ?",
        &[util::js_text(key)],
    )
    .await
}

pub async fn reset_login_failures(env: &Env, key: &str) -> Result<()> {
    let database = env.d1("DB")?;
    db::run(
        &database,
        "DELETE FROM login_throttle WHERE key_hash = ?",
        &[util::js_text(key)],
    )
    .await
}

pub(crate) async fn verified_password_hash(
    env: &Env,
    user_id: i64,
    password: &str,
) -> Result<Option<String>> {
    if password.is_empty() || password.len() > 1024 {
        return Ok(None);
    }
    let reauth_key = crypto::sha256_hex(&format!("reauth:{user_id}")).await?;
    if reserve_login_attempt(env, &reauth_key).await?.is_some() {
        return Ok(None);
    }
    let database = env.d1("DB")?;
    let row: Option<Credential> = db::first(
        &database,
        "SELECT id, password_hash FROM users WHERE id = ?",
        &[util::js_int(user_id)],
    )
    .await?;
    let Some(row) = row else { return Ok(None) };
    if crypto::verify_password(password, &row.password_hash).await? {
        reset_login_failures(env, &reauth_key).await?;
        Ok(Some(row.password_hash))
    } else {
        Ok(None)
    }
}

pub(crate) async fn replace_password(
    env: &Env,
    user_id: i64,
    verified_hash: &str,
    password: &str,
) -> Result<Option<String>> {
    let password_hash = crypto::hash_password(password).await?;
    let token = crypto::random_token()?;
    let token_hash = crypto::sha256_hex(&token).await?;
    let database = env.d1("DB")?;
    let results = database
        .batch(vec![
            database
                .prepare("UPDATE users SET password_hash = ? WHERE id = ? AND password_hash = ?")
                .bind(&[
                    util::js_text(&password_hash),
                    util::js_int(user_id),
                    util::js_text(verified_hash),
                ])?,
            session_insert(&database, user_id, &password_hash, &token_hash)?,
        ])
        .await?;
    Ok((!results[1].results::<IgnoredAny>()?.is_empty()).then_some(token))
}
