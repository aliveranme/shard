use crate::i18n::Locale;
use crate::{crypto, util};
use worker::*;

/// Git clients use access-token authentication, never browser form credentials.
pub fn is_git_request(req: &Request) -> bool {
    let path = req.path();
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(owner), Some(repo), Some(endpoint)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if owner.is_empty() || !repo.ends_with(".git") {
        return false;
    }
    match endpoint {
        "git-upload-pack" | "git-receive-pack" | "shard-git-control" | "shard-upload" => parts.next().is_none(),
        "info" => match parts.next() {
            Some("refs") => parts.next().is_none(),
            Some("lfs") => match (parts.next(), parts.next()) {
                (Some("objects"), Some("batch")) => parts.next().is_none(),
                (Some("objects"), Some(_)) => match (parts.next(), parts.next(), parts.next()) {
                    (Some(_), Some(_), None) => true,
                    (Some(_), Some(_), Some("verify" | "multipart")) => parts.next().is_none(),
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        },
        _ => false,
    }
}

/// This machine-only endpoint requires a bearer token and never reads a session.
pub(crate) fn is_check_request(req: &Request) -> bool {
    let path = req.path();
    let mut parts = path.split('/');
    parts.next() == Some("")
        && parts.next().is_some_and(|owner| !owner.is_empty())
        && parts.next().is_some_and(|repo| !repo.is_empty())
        && parts.next() == Some("commit")
        && parts.next().is_some_and(|hash| !hash.is_empty())
        && parts.next() == Some("checks")
        && parts.next().is_none()
}

fn valid_token(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Authenticated form tokens are bound to the current session. Anonymous forms
/// use a host-only random cookie, including login and account registration.
pub async fn csrf_token(req: &Request, signed_in: bool) -> Result<String> {
    if signed_in {
        if let Some(session) = util::cookie(req, "shard_session") {
            return crypto::sha256_hex(&format!("shard-csrf:{session}")).await;
        }
    }
    match util::cookie(req, "shard_csrf").filter(|token| valid_token(token)) {
        Some(token) => Ok(token),
        None => crypto::random_token(),
    }
}

pub fn same_origin(req: &Request) -> bool {
    if util::header(req, "sec-fetch-site").as_deref() == Some("cross-site") {
        return false;
    }
    match util::header(req, "origin") {
        Some(origin) => req
            .url()
            .map(|url| origin == url.origin().ascii_serialization())
            .unwrap_or(false),
        None => true,
    }
}

pub fn valid_form(form: &FormData, expected: &str) -> bool {
    let Some(token) = form.get_field("csrf_token") else {
        return false;
    };
    if token.len() != expected.len() || !valid_token(&token) {
        return false;
    }
    token
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

pub fn is_html(headers: &Headers) -> Result<bool> {
    Ok(headers.get("Content-Type")?.is_some_and(|value| value.starts_with("text/html")))
}

pub fn finish(
    mut response: Response,
    token: &str,
    previous_token: Option<&str>,
    secure: bool,
    locale: Locale,
) -> Result<Response> {
    let headers = response.headers_mut();
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    if headers.get("Referrer-Policy")?.is_none() {
        headers.set("Referrer-Policy", "same-origin")?;
    }
    headers.set("Permissions-Policy", "camera=(), microphone=(), geolocation=()")?;
    if headers.get("Content-Security-Policy")?.is_none() {
        headers.set(
            "Content-Security-Policy",
            "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; connect-src 'self'",
        )?;
    }
    if headers.get("Cache-Control")?.is_none() {
        headers.set("Cache-Control", "no-store")?;
    }
    if secure {
        headers.set("Strict-Transport-Security", "max-age=31536000")?;
    }
    if is_html(headers)? {
        headers.set("Content-Language", locale.code())?;
        if previous_token != Some(token) {
            headers.append(
                "Set-Cookie",
                &format!(
                    "shard_csrf={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{}",
                    if secure { "; Secure" } else { "" },
                ),
            )?;
        }
    }
    Ok(response)
}
