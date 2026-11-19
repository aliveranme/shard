use super::common::page_title;
use crate::auth;
use crate::avatar::{AvatarImage, UploadError};
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::i18n::{AccountText, CommonText, LANGUAGE_COOKIE, LanguagePreference, Locale};
use crate::models::AccessToken;
use crate::pagination::{PageRequest, PAGE_SIZE};
use crate::templates::{error_response, render, AuthPage, Chrome};
use crate::account::{AccountPage, LanguagePage, TokensPage, TokenRow};
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
        title: page_title(&[state.locale.t(CommonText::SignIn)]),
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
    let login = util::form_value(form, "login").unwrap_or_default().trim().to_string();
    let password = util::form_value(form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(form, "next"));
    if login.is_empty() || login.len() > 254 || password.is_empty() || password.len() > 1024 {
        return error_response(&Chrome::new(&state), 400, state.locale.t(AccountText::InvalidLoginLength));
    }
    let key = auth::login_key(&req, &login).await?;
    if let Some(seconds) = auth::login_retry_after(&state.env, &key).await? {
        let mut response = error_response(&Chrome::new(&state), 429, state.locale.t(AccountText::LoginThrottled))?;
        response.headers_mut().set("Retry-After", &seconds.to_string())?;
        return Ok(response);
    }

    match auth::login_session(&state.env, &login, &password).await? {
        Some(token) => {
            auth::reset_login_failures(&state.env, &key).await?;
            let cookie = util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?;
            let mut response = util::redirect(&next)?;
            response.headers_mut().append("Set-Cookie", &cookie)?;
            Ok(response)
        }
        None => {
            auth::record_login_failure(&state.env, &key).await?;
            let page = AuthPage {
                chrome: Chrome::new(&state),
                title: page_title(&[state.locale.t(CommonText::SignIn)]),
                mode: "login".into(),
                error: Some(state.locale.t(AccountText::IncorrectCredentials)),
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
        title: page_title(&[state.locale.t(AccountText::Register)]),
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
    if !state.registration_open {
        return error_response(&Chrome::new(&state), 403, state.locale.t(AccountText::RegistrationClosed));
    }
    let form = state.form()?;
    let username = util::form_value(form, "username").unwrap_or_default();
    let email = util::form_value(form, "email").unwrap_or_default();
    let password = util::form_value(form, "password").unwrap_or_default();
    let next = util::safe_next(util::form_value(form, "next"));

    let invalid = |message| -> Result<Response> {
        let page = AuthPage {
            chrome: Chrome::new(&state),
            title: page_title(&[state.locale.t(AccountText::Register)]),
            mode: "signup".into(),
            error: Some(state.locale.t(message)),
            username_value: username.clone(),
            email_value: email.clone(),
            next: next.clone(),
        };
        render(&page)
    };

    if !util::valid_slug(&username) || username.len() < 2 || username.len() > 100 {
        return invalid(AccountText::InvalidUsername);
    }
    if matches!(username.to_ascii_lowercase().as_str(), "user" | "explore" | "new" | "issues" | "pulls" | "static" | "api" | "favicon.ico" | "robots.txt") {
        return invalid(AccountText::ReservedUsername);
    }
    if email.len() > 254 || !util::valid_email(&email) {
        return invalid(AccountText::InvalidEmail);
    }
    if password.len() < 8 || password.len() > 1024 {
        return invalid(AccountText::InvalidPasswordLength);
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
        return invalid(AccountText::SignupTaken);
    }

    let token = auth::register(&state.env, &username, &email, &password).await?;
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

const TOKENS_PATH: &str = "/user/settings/tokens";

async fn render_tokens(req: &Request, state: &AppState, new_token: Option<String>, error: Option<&'static str>) -> Result<Response> {
    let user = state.user.as_ref().ok_or_else(|| Error::RustError("authentication required".into()))?;
    let database = state.db()?;
    let page = PageRequest::new(req, "page");
    let mut tokens: Vec<AccessToken> = db::all(&database,
        "SELECT id, name, scope, created_at, last_used_at, expires_at FROM access_tokens
         WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
        &[util::js_int(user.id), util::js_int((PAGE_SIZE + 1) as i64), util::js_int(page.offset())]).await?;
    let pagination = page.finish_at(TOKENS_PATH, &mut tokens)?;
    let now = util::now();
    let mut response = render(&TokensPage {
        chrome: Chrome::new(state), title: page_title(&[state.locale.t(CommonText::AccessTokens)]),
        tokens: tokens.into_iter().map(|token| TokenRow::from_token(token, now)).collect(),
        pagination, new_token, error,
    })?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

pub async fn tokens_page(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_none() { return util::redirect("/user/login?next=/user/settings/tokens"); }
    render_tokens(&req, &state, None, None).await
}

pub async fn create_token(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings/tokens");
    };
    let form = state.form()?;
    let name = util::form_value(form, "name").unwrap_or_default().trim().to_string();
    let scope = util::form_value(form, "scope").unwrap_or_default();
    let expiry = util::form_value(form, "expires_in").unwrap_or_else(|| "30".into());
    let days = match expiry.as_str() {
        "7" => 7, "30" => 30, "90" => 90, "365" => 365,
        _ => return render_tokens(&req, &state, None, Some(state.locale.t(AccountText::InvalidTokenExpiry))).await,
    };
    if name.is_empty() || name.chars().count() > 100 || !matches!(scope.as_str(), "read" | "write") {
        return render_tokens(&req, &state, None, Some(state.locale.t(AccountText::InvalidTokenNameOrScope))).await;
    }
    let password_error = || async {
        let message = state.locale.t(AccountText::IncorrectCurrentPassword);
        Ok(render_tokens(&req, &state, None, Some(message)).await?.with_status(403))
    };
    let password = util::form_value(form, "current_password").unwrap_or_default();
    let Some(verified_hash) = auth::verified_password_hash(&state.env, user.id, &password).await? else {
        return password_error().await;
    };
    let plaintext = format!("shard_{}", crypto::random_token()?);
    let hash = crypto::sha256_hex(&plaintext).await?;
    let database = state.db()?;
    let now = util::now();
    let created = db::first::<serde::de::IgnoredAny>(&database,
        "INSERT INTO access_tokens (user_id, name, token_hash, scope, created_at, expires_at)
         SELECT id, ?2, ?3, ?4, ?5, ?6 FROM users WHERE id = ?1 AND password_hash = ?7 RETURNING id",
        &[
            util::js_int(user.id), util::js_text(&name), util::js_text(&hash), util::js_text(&scope),
            util::js_int(now), util::js_int(now + days * 86400), util::js_text(&verified_hash),
        ]).await?;
    if created.is_none() { return password_error().await; }
    render_tokens(&req, &state, Some(plaintext), None).await
}

pub async fn delete_token(
    req: Request,
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
    let page = PageRequest::new(&req, "page");
    if page.number == 1 {
        util::redirect(TOKENS_PATH)
    } else {
        util::redirect(&format!("{TOKENS_PATH}?page={}", page.number))
    }
}

async fn render_account(state: &AppState, email: String, success: Option<&'static str>, error: Option<&'static str>) -> Result<Response> {
    #[derive(serde::Deserialize)]
    struct Verification { email_verified_at: Option<i64> }
    let user_id = state.user_id().ok_or_else(|| Error::RustError("authentication required".into()))?;
    let verification: Option<Verification> = db::first(
        &state.db()?,
        "SELECT email_verified_at FROM users WHERE id = ?",
        &[util::js_int(user_id)],
    ).await?;
    let mut response = render(&AccountPage {
        chrome: Chrome::new(state), title: page_title(&[state.locale.t(CommonText::AccountSettings)]), email, success, error,
        email_verified: verification.is_some_and(|row| row.email_verified_at.is_some()),
        mail_configured: crate::recovery::configured(&state.env),
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
        Some("email") => Some(state.locale.t(AccountText::EmailUpdated)),
        Some("password") => Some(state.locale.t(AccountText::PasswordUpdated)),
        Some("verification-requested") => Some(state.locale.t(AccountText::VerificationRequested)),
        Some("avatar") => Some(state.locale.t(AccountText::AvatarUpdated)),
        Some("avatar-removed") => Some(state.locale.t(AccountText::AvatarRemoved)),
        _ => None,
    };
    render_account(&state, user.email.clone(), success, None).await
}

pub async fn update_avatar(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = &state.user else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let image = match AvatarImage::from_form(state.form()?).await {
        Ok(image) => image,
        Err(error) => {
            let (status, message) = match error {
                UploadError::Missing => (400, AccountText::AvatarMissing),
                UploadError::Invalid => (400, AccountText::AvatarInvalid),
                UploadError::TooLarge => (413, AccountText::AvatarTooLarge),
                UploadError::Dimensions => (413, AccountText::AvatarDimensions),
            };
            let response = render_account(state, user.email.clone(), None, Some(state.locale.t(message))).await?;
            return Ok(response.with_status(status));
        }
    };
    image.save(state, user.id).await?;
    util::redirect("/user/settings?updated=avatar")
}

pub async fn remove_avatar(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user_id) = state.user_id() else {
        return util::redirect("/user/login?next=/user/settings");
    };
    db::run(
        &state.db()?,
        "DELETE FROM user_avatars WHERE user_id = ?",
        &[util::js_int(user_id)],
    )
    .await?;
    util::redirect("/user/settings?updated=avatar-removed")
}

pub async fn update_email(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let form = state.form()?;
    let email = util::form_value(form, "email").unwrap_or_default().trim().to_string();
    let password = util::form_value(form, "current_password").unwrap_or_default();
    if email.len() > 254 || !util::valid_email(&email) {
        return render_account(&state, user.email.clone(), None, Some(state.locale.t(AccountText::InvalidEmail))).await;
    }
    let Some(verified_hash) = auth::verified_password_hash(&state.env, user.id, &password).await? else {
        return render_account(&state, email, None, Some(state.locale.t(AccountText::IncorrectCurrentPassword))).await;
    };
    let database = state.db()?;
    match db::first::<serde::de::IgnoredAny>(&database,
        "UPDATE users SET email = ? WHERE id = ? AND password_hash = ? RETURNING id",
        &[util::js_text(&email), util::js_int(user.id), util::js_text(&verified_hash)]).await {
        Ok(Some(_)) => util::redirect("/user/settings?updated=email"),
        Ok(None) => render_account(&state, email, None, Some(state.locale.t(AccountText::IncorrectCurrentPassword))).await,
        Err(error) if error.to_string().contains("UNIQUE constraint failed: users.email") => {
            render_account(&state, email, None, Some(state.locale.t(AccountText::EmailTaken))).await
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
    let current = util::form_value(form, "current_password").unwrap_or_default();
    let password = util::form_value(form, "password").unwrap_or_default();
    let confirmation = util::form_value(form, "password_confirm").unwrap_or_default();
    let error = if password.len() < 8 || password.len() > 1024 {
        Some(AccountText::InvalidPasswordLength)
    } else if password != confirmation {
        Some(AccountText::PasswordsMismatch)
    } else { None };
    if let Some(error) = error {
        return render_account(&state, user.email.clone(), None, Some(state.locale.t(error))).await;
    }
    let Some(verified_hash) = auth::verified_password_hash(&state.env, user.id, &current).await? else {
        return render_account(&state, user.email.clone(), None, Some(state.locale.t(AccountText::IncorrectCurrentPassword))).await;
    };
    let Some(token) = auth::replace_password(&state.env, user.id, &verified_hash, &password).await? else {
        return render_account(&state, user.email.clone(), None, Some(state.locale.t(AccountText::IncorrectCurrentPassword))).await;
    };
    let mut response = util::redirect("/user/settings?updated=password")?;
    response.headers_mut().append("Set-Cookie", &util::set_cookie(&token, 30 * 24 * 3600, util::is_secure(&req))?)?;
    Ok(response)
}

pub async fn delete_account(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = &state.user else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let locale = state.locale;
    let error = |status, key| crate::templates::error_response(&Chrome::new(state), status, locale.t(key));
    let form = state.form()?;
    if util::form_value(form, "confirm").as_deref() != Some(user.username.as_str()) {
        return error(400, AccountText::ConfirmAccountDeletion);
    }
    let password = util::form_value(form, "current_password").unwrap_or_default();
    if password.is_empty() || password.len() > 1024 {
        return error(403, AccountText::IncorrectCurrentPassword);
    }
    #[derive(serde::Deserialize)]
    struct Credential { password_hash: String }
    let database = state.db()?;
    let args = [util::js_int(user.id)];
    let credential: Option<Credential> = db::first(&database,
        "SELECT password_hash FROM users WHERE id = ?", &args,
    ).await?;
    let Some(credential) = credential else {
        return util::redirect("/user/login");
    };
    if !crypto::verify_password(&password, &credential.password_hash).await? {
        return error(403, AccountText::IncorrectCurrentPassword);
    }
    #[derive(serde::Deserialize)]
    struct Deleted { id: i64 }
    let deleted: Option<Deleted> = db::first(&database,
        "DELETE FROM users WHERE id = ?1 AND password_hash = ?2
          AND NOT EXISTS (SELECT 1 FROM repos WHERE owner_id = ?1)
          AND NOT EXISTS (SELECT 1 FROM repo_copies WHERE owner_id = ?1 AND state IN ('starting', 'pending', 'discard'))
          RETURNING id",
        &[util::js_int(user.id), util::js_text(&credential.password_hash)],
    ).await?;
    if !deleted.is_some_and(|row| row.id == user.id) {
        return error(409, AccountText::AccountDeletionBlocked);
    }
    let mut response = util::redirect("/")?;
    response.headers_mut().append("Set-Cookie", &util::clear_cookie())?;
    Ok(response)
}

pub async fn language_page(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    render(&LanguagePage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(AccountText::InterfaceLanguage)]),
        preference: state.language_preference.code(),
        languages: &Locale::SUPPORTED,
        saved: util::query_param(&req, "updated").as_deref() == Some("1"),
    })
}

pub async fn update_language(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let value = util::form_value(state.form()?, "language");
    let Some(preference) = value.as_deref().and_then(LanguagePreference::parse) else {
        return error_response(&Chrome::new(state), 400, state.locale.t(AccountText::InvalidLanguage));
    };
    let mut response = util::redirect("/user/settings/language?updated=1")?;
    if let Some(user_id) = state.user_id() {
        db::run(
            &state.db()?,
            "UPDATE users SET locale = ? WHERE id = ?",
            &[util::js_text(preference.code()), util::js_int(user_id)],
        ).await?;
    } else {
        let secure = if util::is_secure(&req) { "; Secure" } else { "" };
        response.headers_mut().append(
            "Set-Cookie",
            &format!("{LANGUAGE_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{secure}", preference.code()),
        )?;
    }
    Ok(response)
}
