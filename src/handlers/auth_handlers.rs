use super::common::page_title;
use crate::auth;
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::models::AccessToken;
use crate::templates::{render, AuthPage, Chrome, TokensPage, TokenRow};
use crate::util;
use worker::*;

pub async fn login_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_some() {
        return util::redirect("/");
    }
    let next = util::safe_next(util::query_param(&req, "next"));
    let page = AuthPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["Sign in"]),
        mode: "login".into(),
        error: None,
        username_value: String::new(),
        email_value: String::new(),
        next,
    };
    render(&page)
}

pub async fn login_submit(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let form = req.form_data().await?;
    let login = util::form_value(&form, "login").unwrap_or_default();
    let password = util::form_value(&form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(&form, "next"));

    match auth::authenticate(&state.env, &login, &password).await? {
        Some(user) => {
            let token = auth::create_session(&state.env, user.id).await?;
            let cookie = util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?;
            let mut response = util::redirect(&next)?;
            response.headers_mut().append("Set-Cookie", &cookie)?;
            Ok(response)
        }
        None => {
            let page = AuthPage {
                chrome: Chrome::new(&state.user),
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
        chrome: Chrome::new(&state.user),
        title: page_title(&["Register"]),
        mode: "signup".into(),
        error: None,
        username_value: String::new(),
        email_value: String::new(),
        next,
    };
    render(&page)
}

pub async fn signup_submit(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let form = req.form_data().await?;
    let username = util::form_value(&form, "username").unwrap_or_default();
    let email = util::form_value(&form, "email").unwrap_or_default();
    let password = util::form_value(&form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(&form, "next"));

    let invalid = |message: &str| -> Result<Response> {
        let page = AuthPage {
            chrome: Chrome::new(&state.user),
            title: page_title(&["Register"]),
            mode: "signup".into(),
            error: Some(message.to_string()),
            username_value: username.clone(),
            email_value: email.clone(),
            next: next.clone(),
        };
        render(&page)
    };

    if !util::valid_slug(&username) || username.len() < 2 {
        return invalid("Usernames are 2+ characters: letters, digits, dot, dash or underscore.");
    }
    if !util::valid_email(&email) {
        return invalid("Enter a valid email address.");
    }
    if password.len() < 8 {
        return invalid("Passwords need at least 8 characters.");
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

pub async fn tokens_page(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings/tokens");
    };
    let database = state.db()?;
    let tokens: Vec<AccessToken> = db::all(
        &database,
        "SELECT id, name, scope, created_at, last_used_at FROM access_tokens WHERE user_id = ? ORDER BY id DESC",
        &[util::js_int(user.id)],
    )
    .await?;
    let new_token = util::query_param(&req, "token");
    let page = TokensPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["Access tokens"]),
        tokens: tokens.iter().map(TokenRow::from_token).collect(),
        new_token,
        error: None,
    };
    render(&page)
}

pub async fn create_token(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings/tokens");
    };
    let form = req.form_data().await?;
    let name = util::form_value(&form, "name").unwrap_or_else(|| "git".into());
    let scope = match util::form_value(&form, "scope").as_deref() {
        Some("read") => "read",
        _ => "write",
    };
    let plaintext = crypto::random_token()?;
    let hash = crypto::sha256_hex(&plaintext).await?;
    let database = state.db()?;
    db::run(
        &database,
        "INSERT INTO access_tokens (user_id, name, token_hash, scope, created_at) VALUES (?, ?, ?, ?, ?)",
        &[
            util::js_int(user.id),
            util::js_text(&name),
            util::js_text(&hash),
            util::js_text(scope),
            util::js_int(util::now()),
        ],
    )
    .await?;
    let tokens: Vec<AccessToken> = db::all(
        &database,
        "SELECT id, name, scope, created_at, last_used_at FROM access_tokens WHERE user_id = ? ORDER BY id DESC",
        &[util::js_int(user.id)],
    )
    .await?;
    let page = TokensPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["Access tokens"]),
        tokens: tokens.iter().map(TokenRow::from_token).collect(),
        new_token: Some(plaintext),
        error: None,
    };
    render(&page)
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
