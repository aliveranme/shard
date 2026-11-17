use crate::crypto;
use crate::db;
use crate::models::{GitIdentity, User, UserAuth};
use crate::util;
use base64::Engine;
use worker::*;

const SESSION_TTL: i64 = 30 * 24 * 3600;

pub async fn current_user(env: &Env, req: &Request) -> Result<Option<User>> {
    let Some(token) = util::cookie(req, "shard_session") else {
        return Ok(None);
    };
    let hash = crypto::sha256_hex(&token).await?;
    let database = env.d1("DB")?;
    db::first(
        &database,
        "SELECT u.id, u.username, u.email, u.is_admin, u.created_at
           FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = ? AND s.expires_at > ?",
        &[util::js_text(&hash), util::js_int(util::now())],
    )
    .await
}

pub async fn create_session(env: &Env, user_id: i64) -> Result<String> {
    let token = crypto::random_token()?;
    let hash = crypto::sha256_hex(&token).await?;
    let database = env.d1("DB")?;
    db::run(
        &database,
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        &[
            util::js_text(&hash),
            util::js_int(user_id),
            util::js_int(util::now()),
            util::js_int(util::now() + SESSION_TTL),
        ],
    )
    .await?;
    Ok(token)
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

pub async fn authenticate(env: &Env, login: &str, password: &str) -> Result<Option<User>> {
    let database = env.d1("DB")?;
    let found: Option<UserAuth> = db::first(
        &database,
        "SELECT id, username, email, password_hash, is_admin, created_at
           FROM users WHERE username = ? COLLATE NOCASE OR email = ? COLLATE NOCASE LIMIT 1",
        &[util::js_text(login), util::js_text(login)],
    )
    .await?;
    let Some(account) = found else {
        return Ok(None);
    };
    if crypto::verify_password(password, &account.password_hash).await? {
        Ok(Some(account.public()))
    } else {
        Ok(None)
    }
}

pub async fn register(
    env: &Env,
    username: &str,
    email: &str,
    password: &str,
) -> Result<User> {
    let hash = crypto::hash_password(password).await?;
    let database = env.d1("DB")?;
    db::run(
        &database,
        "INSERT INTO users (username, email, password_hash, is_admin, created_at)
         VALUES (?, ?, ?, 0, ?)",
        &[
            util::js_text(username),
            util::js_text(email),
            util::js_text(&hash),
            util::js_int(util::now()),
        ],
    )
    .await?;
    let account: UserAuth = db::first(
        &database,
        "SELECT id, username, email, password_hash, is_admin, created_at FROM users WHERE username = ?",
        &[util::js_text(username)],
    )
    .await?
    .ok_or_else(|| Error::RustError("user insert failed".into()))?;
    Ok(account.public())
}

/// Resolve HTTP basic (`username:token`) or bearer credentials to a git identity.
pub async fn git_identity(env: &Env, req: &Request) -> Result<Option<GitIdentity>> {
    let Some(header) = util::header(req, "authorization") else {
        return Ok(None);
    };
    let secret = if let Some(rest) = header.strip_prefix("Basic ").or_else(|| header.strip_prefix("basic ")) {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(rest.trim())
            .map_err(|_| Error::RustError("invalid basic credentials".into()))?;
        let decoded = String::from_utf8(decoded)
            .map_err(|_| Error::RustError("invalid basic credentials".into()))?;
        match decoded.split_once(':') {
            Some((_user, token)) if !token.is_empty() => token.to_string(),
            _ => return Ok(None),
        }
    } else if let Some(rest) = header.strip_prefix("Bearer ").or_else(|| header.strip_prefix("bearer ")) {
        rest.trim().to_string()
    } else {
        return Ok(None);
    };

    let hash = crypto::sha256_hex(&secret).await?;
    let database = env.d1("DB")?;
    #[derive(serde::Deserialize)]
    struct Row {
        id: i64,
        user_id: i64,
        username: String,
        scope: String,
        is_admin: i64,
    }
    let row: Option<Row> = db::first(
        &database,
        "SELECT t.id, t.user_id, t.scope, u.username, u.is_admin
           FROM access_tokens t JOIN users u ON u.id = t.user_id
          WHERE t.token_hash = ?",
        &[util::js_text(&hash)],
    )
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    db::run(
        &database,
        "UPDATE access_tokens SET last_used_at = ? WHERE id = ?",
        &[util::js_int(util::now()), util::js_int(row.id)],
    )
    .await?;
    Ok(Some(GitIdentity {
        user_id: row.user_id,
        username: row.username,
        scope: row.scope,
        is_admin: row.is_admin,
    }))
}
