//! Shard: a self-hosted, Cloudflare-native Git forge.
//!
//! Pages and metadata live in D1; repositories live in Artifacts. Git push and
//! fetch are proxied so users only ever see the forge domain.
use worker::*;

mod account;
mod artifacts;
mod auth;
mod crypto;
mod ctx;
mod db;
mod handlers;
mod i18n;
mod markdown;
mod models;
mod templates;
mod pagination;
mod security;
mod util;

use ctx::AppState;
use i18n::{CommonText, LANGUAGE_COOKIE, LanguagePreference, Locale};

fn routes(router: Router<AppState>) -> Router<AppState> {
    router
        .get_async("/", handlers::web::home)
        .get_async("/explore", handlers::web::explore)
        .get_async("/explore/repos", handlers::web::explore)
        .get_async("/issues", handlers::web::global_issues)
        .get_async("/pulls", handlers::web::global_pulls)
        // Authentication
        .get_async("/user/login", handlers::auth_handlers::login_form)
        .post_async("/user/login", handlers::auth_handlers::login_submit)
        .get_async("/user/sign_up", handlers::auth_handlers::signup_form)
        .post_async("/user/sign_up", handlers::auth_handlers::signup_submit)
        .post_async("/user/logout", handlers::auth_handlers::logout)
        .get_async("/user/settings", handlers::auth_handlers::account_page)
        .post_async("/user/settings/email", handlers::auth_handlers::update_email)
        .post_async("/user/settings/password", handlers::auth_handlers::update_password)
        .get_async("/user/settings/language", handlers::auth_handlers::language_page)
        .post_async("/user/settings/language", handlers::auth_handlers::update_language)
        .get_async(
            "/user/settings/tokens",
            handlers::auth_handlers::tokens_page,
        )
        .post_async(
            "/user/settings/tokens",
            handlers::auth_handlers::create_token,
        )
        .post_async(
            "/user/settings/tokens/:id/delete",
            handlers::auth_handlers::delete_token,
        )
        // Repository lifecycle
        .get_async("/new", handlers::repos::new_form)
        .get_async("/:owner", handlers::web::profile)
        .post_async("/repo/create", handlers::repos::create)
        .get_async("/:owner/:repo", handlers::repos::view)
        .get_async("/:owner/:repo/commits", handlers::repos::commits)
        .get_async("/:owner/:repo/src", handlers::repos::tree)
        .get_async("/:owner/:repo/src/*path", handlers::repos::tree)
        .get_async("/:owner/:repo/blob/*path", handlers::repos::file)
        .get_async("/:owner/:repo/raw/*path", handlers::repos::raw)
        .get_async("/:owner/:repo/settings", handlers::repos::settings)
        .post_async("/:owner/:repo/settings", handlers::repos::update_settings)
        .post_async("/:owner/:repo/settings/delete", handlers::repos::delete)
        // Issues and pull requests
        .get_async("/:owner/:repo/issues", handlers::issues::list_issues)
        .get_async("/:owner/:repo/issues/new", handlers::issues::new_issue_form)
        .post_async("/:owner/:repo/issues/new", handlers::issues::create_issue)
        .get_async("/:owner/:repo/issues/:number", handlers::issues::detail_issue)
        .post_async(
            "/:owner/:repo/issues/:number/comments",
            handlers::issues::comment,
        )
        .post_async(
            "/:owner/:repo/issues/:number/state",
            handlers::issues::set_state,
        )
        .get_async("/:owner/:repo/pulls", handlers::issues::list_pulls)
        .get_async("/:owner/:repo/pulls/new", handlers::issues::new_pull_form)
        .post_async("/:owner/:repo/pulls/new", handlers::issues::create_pull)
        .get_async("/:owner/:repo/pulls/:number", handlers::issues::detail_pull)
        .post_async(
            "/:owner/:repo/pulls/:number/comments",
            handlers::issues::comment,
        )
        .post_async(
            "/:owner/:repo/pulls/:number/state",
            handlers::issues::set_state,
        )
        // Git smart HTTP (repo name carries the `.git` suffix)
        .get_async("/:owner/:repo/info/refs", handlers::git::info_refs)
        .post_async(
            "/:owner/:repo/git-upload-pack",
            handlers::git::upload_pack,
        )
        .post_async(
            "/:owner/:repo/git-receive-pack",
            handlers::git::receive_pack,
        )
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _worker_ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let secure = util::is_secure(&req);
    let previous_csrf = util::cookie(&req, "shard_csrf");
    let is_git = security::is_git_request(&req);
    let guest_preference = if is_git {
        None
    } else {
        util::cookie(&req, LANGUAGE_COOKIE).as_deref().and_then(LanguagePreference::parse)
    };
    let browser_locale = || {
        if is_git {
            Locale::En
        } else {
            util::header(&req, "accept-language").as_deref().map_or(Locale::En, Locale::negotiate)
        }
    };
    let user = match auth::current_user(&env, &req).await {
        Ok(user) => user,
        Err(error) => {
            console_log!("session lookup failed: {error}");
            let token = security::csrf_token(&req, false).await?;
            let locale = guest_preference.and_then(LanguagePreference::locale).unwrap_or_else(browser_locale);
            let response = templates::error_response(
                templates::Chrome { locale, ..Default::default() },
                503,
                locale.t(CommonText::AccountUnavailable),
            )?;
            return security::finish(response, &token, previous_csrf.as_deref(), secure, locale);
        }
    };
    let language_preference = if is_git {
        LanguagePreference::Browser
    } else {
        user.as_ref().and_then(|user| user.locale.as_deref()).and_then(LanguagePreference::parse)
            .or(guest_preference)
            .unwrap_or(LanguagePreference::Browser)
    };
    let locale = language_preference.locale().unwrap_or_else(browser_locale);
    let csrf_token = security::csrf_token(&req, user.is_some()).await?;
    let mut state = AppState {
        env: env.clone(),
        user,
        locale,
        language_preference,
        csrf_token,
        form: None,
    };
    let chrome = templates::Chrome::new(&state);

    if req.method() == Method::Post && !is_git {
        let rejected = if !security::same_origin(&req) {
            Some((403, CommonText::FormOrigin))
        } else {
            match req.form_data().await {
                Ok(form) if security::valid_form(&form, &state.csrf_token) => {
                    state.form = Some(std::rc::Rc::new(form));
                    None
                }
                Ok(_) => Some((403, CommonText::FormExpired)),
                Err(_) => Some((400, CommonText::UnreadableForm)),
            }
        };
        if let Some((status, message)) = rejected {
            let response = templates::error_response(chrome, status, locale.t(message))?;
            return security::finish(response, &state.csrf_token, previous_csrf.as_deref(), secure, locale);
        }
    }

    let router = routes(Router::with_data(state));
    let response = match router.run(req, env).await {
        Ok(response) if !is_git && matches!(response.status_code(), 404 | 405)
            && !security::is_html(response.headers())? => {
                let status = response.status_code();
                let message = if status == 404 { CommonText::NotFound } else { CommonText::MethodNotAllowed };
                templates::error_response(chrome.clone(), status, locale.t(message))?
            }
        Ok(response) => response,
        Err(error) => {
            console_log!("unhandled worker error: {error}");
            templates::error_response(chrome.clone(), 500, locale.t(CommonText::WorkerError))?
        }
    };
    if is_git {
        Ok(response)
    } else {
        security::finish(response, &chrome.csrf_token, previous_csrf.as_deref(), secure, locale)
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    if let Err(error) = auth::cleanup_expired(&env).await {
        console_log!("credential cleanup failed: {error}");
    }
}
