use super::common::page_title;
use crate::auth;
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::models::AccessToken;
use crate::templates::{render, AuthPage, Chrome};
use crate::account::{AccountPage, TokensPage, TokenRow};
use crate::util;
use worker::*;

pub async fn login_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_some() {
        return util::redirect("/");
    }
    let next = util::safe_next(util::query_param(&req, "next"));
    let page = AuthPage {
        chrome: Chrome::new(&state),
        title: page_title(&["Sign in"]),
        mode: "login".into(),
        error: None,
        username_value: String::new(),
        email_value: String::new(),
        next,
    };
    render(&page)
}

pub async fn login_submit(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let form = state.form()?;
    let login = util::form_value(&form, "login").unwrap_or_default().trim().to_string();
    let password = util::form_value(&form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(&form, "next"));
    if login.is_empty() || login.len() > 254 || password.is_empty() || password.len() > 1024 {
        return Response::error("Invalid login or password length.", 400);
    }
    let key = auth::login_key(&req, &login).await?;
    if let Some(seconds) = auth::login_retry_after(&state.env, &key).await? {
        let mut response = Response::error("Too many failed sign-in attempts. Try again later.", 429)?;
        response.headers_mut().set("Retry-After", &seconds.to_string())?;
        return Ok(response);
    }

    match auth::authenticate(&state.env, &login, &password).await? {
        Some(user) => {
            auth::reset_login_failures(&state.env, &key).await?;
            let token = auth::create_session(&state.env, user.id).await?;
            let cookie = util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?;
            let mut response = util::redirect(&next)?;
            response.headers_mut().append("Set-Cookie", &cookie)?;
            Ok(response)
        }
        None => {
            auth::record_login_failure(&state.env, &key).await?;
            let page = AuthPage {
                chrome: Chrome::new(&state),
                title: page_title(&["Sign in"]),
                mode: "login".into(),
                error: Some("Incorrect username or password.".into()),
                username_value: login,
                email_value: String::new(),
                next,
            };
            render(&page)
        }
    }
}

pub async fn signup_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_some() {
        return util::redirect("/");
    }
    let next = util::safe_next(util::query_param(&req, "next"));
    let page = AuthPage {
        chrome: Chrome::new(&state),
        title: page_title(&["Register"]),
        mode: "signup".into(),
        error: None,
        username_value: String::new(),
        email_value: String::new(),
        next,
    };
    render(&page)
}

pub async fn signup_submit(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let form = state.form()?;
    let username = util::form_value(&form, "username").unwrap_or_default();
    let email = util::form_value(&form, "email").unwrap_or_default();
    let password = util::form_value(&form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(&form, "next"));

    let invalid = |message: &str| -> Result<Response> {
        let page = AuthPage {
            chrome: Chrome::new(&state),
            title: page_title(&["Register"]),
            mode: "signup".into(),
            error: Some(message.to_string()),
            username_value: username.clone(),
            email_value: email.clone(),
            next: next.clone(),
        };
        render(&page)
    };

    if !util::valid_slug(&username) || username.len() < 2 || username.len() > 100 {
        return invalid("Usernames must contain 2 to 100 letters, digits, dots, dashes or underscores.");
    }
    if matches!(username.to_ascii_lowercase().as_str(), "user" | "explore" | "new" | "issues" | "pulls" | "static" | "api" | "favicon.ico" | "robots.txt") {
        return invalid("That username is reserved. Choose another username.");
    }
    if email.len() > 254 || !util::valid_email(&email) {
        return invalid("Enter a valid email address.");
    }
    if password.len() < 8 || password.len() > 1024 {
        return invalid("Passwords need 8 to 1024 characters.");
    }

    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct Count {
        n: i64,
    }
    let taken = db::first::<Count>(
        &database,
        "SELECT COUNT(*) AS n FROM users WHERE username = ? COLLATE NOCASE OR email = ? COLLATE NOCASE",
        &[util::js_text(&username), util::js_text(&email)],
    )
    .await?
    .map(|row| row.n)
    .unwrap_or(0);
    if taken > 0 {
        return invalid("That username or email is already registered.");
    }

    let user = auth::register(&state.env, &username, &email, &password).await?;
    let token = auth::create_session(&state.env, user.id).await?;
    let cookie = util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?;
    let mut response = util::redirect(&next)?;
    response.headers_mut().append("Set-Cookie", &cookie)?;
    Ok(response)
}

pub async fn logout(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if let Some(token) = util::cookie(&req, "shard_session") {
        let _ = auth::destroy_session(&state.env, &token).await;
    }
    let mut response = util::redirect("/")?;
    response
        .headers_mut()
        .append("Set-Cookie", &util::clear_cookie())?;
    Ok(response)
}

async fn render_tokens(state: &AppState, new_token: Option<String>, error: Option<String>) -> Result<Response> {
    let user = state.user.as_ref().ok_or_else(|| Error::RustError("authentication required".into()))?;
    let database = state.db()?;
    let tokens: Vec<AccessToken> = db::all(&database,
        "SELECT id, name, scope, created_at, last_used_at, expires_at FROM access_tokens WHERE user_id = ? ORDER BY id DESC",
        &[util::js_int(user.id)]).await?;
    let mut response = render(&TokensPage {
        chrome: Chrome::new(state), title: page_title(&["Access tokens"]),
        tokens: tokens.iter().map(TokenRow::from_token).collect(), new_token, error,
    })?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

pub async fn tokens_page(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_none() { return util::redirect("/user/login?next=/user/settings/tokens"); }
    render_tokens(&state, None, None).await
}

pub async fn create_token(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings/tokens");
    };
    let form = state.form()?;
    let name = util::form_value(&form, "name").unwrap_or_default().trim().to_string();
    let scope = util::form_value(&form, "scope").unwrap_or_default();
    let expiry = util::form_value(&form, "expires_in").unwrap_or_else(|| "30".into());
    let days = match expiry.as_str() {
        "7" => 7, "30" => 30, "90" => 90, "365" => 365,
        _ => return render_tokens(&state, None, Some("Choose a valid token expiry.".into())).await,
    };
    if name.is_empty() || name.chars().count() > 100 || !matches!(scope.as_str(), "read" | "write") {
        return render_tokens(&state, None, Some("Use a name of 1 to 100 characters and choose read or write scope.".into())).await;
    }
    let plaintext = format!("shard_{}", crypto::random_token()?);
    let hash = crypto::sha256_hex(&plaintext).await?;
    let database = state.db()?;
    let now = util::now();
    db::run(&database,
        "INSERT INTO access_tokens (user_id, name, token_hash, scope, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
        &[util::js_int(user.id), util::js_text(&name), util::js_text(&hash), util::js_text(&scope), util::js_int(now), util::js_int(now + days * 86400)]).await?;
    render_tokens(&state, Some(plaintext), None).await
}

pub async fn delete_token(
    _req: Request,
    ctx: RouteContext<AppState>,
) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login");
    };
    let id: i64 = ctx
        .param("id")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let database = state.db()?;
    db::run(
        &database,
        "DELETE FROM access_tokens WHERE id = ? AND user_id = ?",
        &[util::js_int(id), util::js_int(user.id)],
    )
    .await?;
    util::redirect("/user/settings/tokens")
}

fn render_account(state: &AppState, email: String, success: Option<String>, error: Option<String>) -> Result<Response> {
    let mut response = render(&AccountPage {
        chrome: Chrome::new(state), title: page_title(&["Account settings"]), email, success, error,
    })?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

pub async fn account_page(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let success = match util::query_param(&req, "updated").as_deref() {
        Some("email") => Some("Your email address has been updated.".into()),
        Some("password") => Some("Your password has been updated. All other browser sessions have been signed out.".into()),
        _ => None,
    };
    render_account(&state, user.email.clone(), success, None)
}

pub async fn update_email(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let form = state.form()?;
    let email = util::form_value(&form, "email").unwrap_or_default().trim().to_string();
    let password = util::form_value(&form, "current_password").unwrap_or_default();
    if email.len() > 254 || !util::valid_email(&email) {
        return render_account(&state, user.email.clone(), None, Some("Enter a valid email address.".into()));
    }
    if !auth::verify_current_password(&state.env, user.id, &password).await? {
        return render_account(&state, email, None, Some("Your current password is incorrect.".into()));
    }
    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct Count { n: i64 }
    let taken = db::first::<Count>(&database,
        "SELECT COUNT(*) AS n FROM users WHERE email = ? COLLATE NOCASE AND id != ?",
        &[util::js_text(&email), util::js_int(user.id)]).await?.map(|row| row.n).unwrap_or(0);
    if taken > 0 {
        return render_account(&state, email, None, Some("That email address is already registered.".into()));
    }
    match db::run(&database, "UPDATE users SET email = ? WHERE id = ?",
        &[util::js_text(&email), util::js_int(user.id)]).await {
        Ok(()) => util::redirect("/user/settings?updated=email"),
        Err(error) if error.to_string().contains("UNIQUE constraint failed: users.email") => {
            render_account(&state, email, None, Some("That email address is already registered.".into()))
        }
        Err(error) => Err(error),
    }
}

pub async fn update_password(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let form = state.form()?;
    let current = util::form_value(&form, "current_password").unwrap_or_default();
    let password = util::form_value(&form, "password").unwrap_or_default();
    let confirmation = util::form_value(&form, "password_confirm").unwrap_or_default();
    let error = if password.len() < 8 || password.len() > 1024 {
        Some("Passwords need 8 to 1024 characters.")
    } else if password != confirmation {
        Some("The new passwords do not match.")
    } else if !auth::verify_current_password(&state.env, user.id, &current).await? {
        Some("Your current password is incorrect.")
    } else { None };
    if let Some(error) = error {
        return render_account(&state, user.email.clone(), None, Some(error.into()));
    }
    let token = auth::replace_password(&state.env, user.id, &password).await?;
    let mut response = util::redirect("/user/settings?updated=password")?;
    response.headers_mut().append("Set-Cookie", &util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?)?;
    Ok(response)
}
