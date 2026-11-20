//! Shard: a self-hosted, Cloudflare-native Git forge.
//!
//! Pages and forge metadata live in D1. Git access through the forge domain
//! uses the storage backend recorded for each repository.
use worker::*;

mod account;
mod artifacts;
mod auth;
mod avatar;
mod collaborators;
mod crypto;
mod ctx;
mod db;
mod git_activity;
mod git_wire;
mod git_storage;
mod gpg;
mod handlers;
mod i18n;
mod issue_filters;
mod issue_metadata;
mod lfs;
mod limits;
mod markdown;
mod highlight;
mod branch_rules;
mod metadata_import;
mod models;
mod native_git;
mod notifications;
mod organizations;
mod templates;
mod pagination;
mod pull_base;
mod pull_diff;
mod pull_review;
mod recovery;
mod refs;
mod repo_features;
mod repo_lifecycle;
mod repo_objects;
mod repo_search;
mod security;
mod util;
mod webhooks;

use ctx::AppState;
use i18n::{AccountText, CommonText, LANGUAGE_COOKIE, LanguagePreference, Locale};

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
        .get_async("/user/forgot_password", recovery::request_form)
        .post_async("/user/forgot_password", recovery::request_submit)
        .get_async("/user/reset_password", recovery::reset_form)
        .post_async("/user/reset_password", recovery::reset_submit)
        .get_async("/user/verify_email", recovery::verify_form)
        .post_async("/user/verify_email", recovery::verify_submit)
        .post_async("/user/logout", handlers::auth_handlers::logout)
        .get_async("/user/notifications", notifications::inbox)
        .post_async("/user/notifications/:id/read", notifications::read_one)
        .post_async("/user/notifications/read-all", notifications::read_all)
        .get_async("/user/avatar/:id", avatar::show)
        .get_async("/user/stars", repo_features::starred)
        .get_async("/user/organizations", organizations::index)
        .post_async("/user/organizations", organizations::create)
        .get_async("/user/organizations/:org", organizations::show)
        .post_async("/user/organizations/:org", organizations::update)
        .get_async("/user/organizations/:org/teams/:team", organizations::team)
        .post_async("/user/organizations/:org/teams/:team", organizations::update_team)
        .get_async("/user/settings", handlers::auth_handlers::account_page)
        .post_async("/user/settings/avatar", handlers::auth_handlers::update_avatar)
        .post_async("/user/settings/avatar/remove", handlers::auth_handlers::remove_avatar)
        .post_async("/user/settings/delete", handlers::auth_handlers::delete_account)
        .post_async("/user/settings/email", handlers::auth_handlers::update_email)
        .post_async("/user/settings/email/verify", recovery::request_verification)
        .post_async("/user/settings/password", handlers::auth_handlers::update_password)
        .get_async("/user/settings/gpg", gpg::settings)
        .post_async("/user/settings/gpg", gpg::update)
        .get_async("/user/settings/language", handlers::auth_handlers::language_page)
        .post_async("/user/settings/language", handlers::auth_handlers::update_language)
        .get_async("/user/settings/transfers", repo_lifecycle::transfers)
        .post_async("/user/settings/transfers", repo_lifecycle::respond_transfer)
        .get_async("/user/settings/repository-import", repo_lifecycle::import_form)
        .post_async("/user/settings/repository-import", repo_lifecycle::import)
        .get_async("/user/settings/repository-copies", repo_lifecycle::copies)
        .get_async("/user/settings/repository-copies/:id", repo_lifecycle::copy_status)
        .post_async("/user/settings/repository-copies/:id", repo_lifecycle::poll_copy)
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
        .get_async("/:owner/:repo/commit/:hash", repo_objects::commit)
        .post_async("/:owner/:repo/commit/:hash/checks", handlers::checks::record)
        .get_async("/:owner/:repo/archive/:hash", repo_objects::archive)
        .get_async("/:owner/:repo/refs", handlers::repos::references)
        .post_async("/:owner/:repo/refs", branch_rules::write_ref)
        .get_async("/:owner/:repo/search", repo_search::search)
        .get_async("/:owner/:repo/releases", repo_features::releases)
        .get_async("/:owner/:repo/releases/new", repo_features::new_release)
        .post_async("/:owner/:repo/releases/new", repo_features::create_release)
        .get_async("/:owner/:repo/releases/:id", repo_features::release)
        .get_async("/:owner/:repo/releases/:id/edit", repo_features::edit_release)
        .post_async("/:owner/:repo/releases/:id/edit", repo_features::update_release)
        .post_async("/:owner/:repo/releases/:id/delete", repo_features::delete_release)
        .post_async("/:owner/:repo/watch", repo_features::watch)
        .post_async("/:owner/:repo/star", repo_features::star)
        .get_async("/:owner/:repo/fork", repo_lifecycle::fork_form)
        .post_async("/:owner/:repo/fork", repo_lifecycle::fork)
        .get_async("/:owner/:repo/src", handlers::repos::tree)
        .get_async("/:owner/:repo/src/*path", handlers::repos::tree)
        .get_async("/:owner/:repo/blob/*path", handlers::repos::file)
        .get_async("/:owner/:repo/raw/*path", handlers::repos::raw)
        .get_async("/:owner/:repo/media/*path", handlers::repos::media)
        .get_async("/:owner/:repo/settings", handlers::repos::settings)
        .post_async("/:owner/:repo/settings", handlers::repos::update_settings)
        .post_async("/:owner/:repo/settings/delete", handlers::repos::delete)
        .get_async("/:owner/:repo/settings/collaborators", collaborators::settings)
        .post_async("/:owner/:repo/settings/collaborators", collaborators::save)
        .post_async("/:owner/:repo/settings/collaborators/revoke", collaborators::revoke)
        .get_async("/:owner/:repo/settings/issues", issue_metadata::catalog)
        .post_async("/:owner/:repo/settings/issues", issue_metadata::catalog_action)
        .get_async("/:owner/:repo/settings/issue-templates", repo_features::issue_templates)
        .post_async("/:owner/:repo/settings/issue-templates", repo_features::save_issue_template)
        .post_async("/:owner/:repo/settings/issue-templates/:id/delete", repo_features::delete_issue_template)
        .get_async("/:owner/:repo/settings/organization", organizations::repository)
        .post_async("/:owner/:repo/settings/organization", organizations::update_repository)
        .get_async("/:owner/:repo/settings/webhooks", webhooks::page)
        .post_async("/:owner/:repo/settings/webhooks", webhooks::update)
        .get_async("/:owner/:repo/settings/metadata-import", metadata_import::form)
        .post_async("/:owner/:repo/settings/metadata-import", metadata_import::submit)
        .get_async("/:owner/:repo/settings/branches", branch_rules::settings)
        .post_async("/:owner/:repo/settings/branches", branch_rules::update)
        .get_async("/:owner/:repo/settings/lifecycle", repo_lifecycle::settings)
        .post_async("/:owner/:repo/settings/lifecycle", repo_lifecycle::update)
        // Issues and pull requests
        .get_async("/:owner/:repo/issues", handlers::issues::list_issues)
        .get_async("/:owner/:repo/issues/new", handlers::issues::new_issue_form)
        .post_async("/:owner/:repo/issues/new", handlers::issues::create_issue)
        .post_async("/:owner/:repo/issues/:number/reactions", repo_features::react)
        .get_async("/:owner/:repo/issues/:number", handlers::issues::detail_issue)
        .get_async("/:owner/:repo/issues/:number/edit", issue_metadata::edit_form)
        .post_async("/:owner/:repo/issues/:number/edit", issue_metadata::save_edit)
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
        .post_async("/:owner/:repo/pulls/:number/reviews", pull_review::submit)
        .post_async("/:owner/:repo/pulls/:number/comparison", pull_review::refresh)
        .post_async("/:owner/:repo/pulls/:number/merge", branch_rules::request_merge)
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
        .post_async("/:owner/:repo/shard-git-control", handlers::git::direct_control)
        .post_async("/:owner/:repo/shard-upload", handlers::git::multipart_upload)
        .post_async("/:owner/:repo/info/lfs/objects/batch", lfs::batch)
        .put_async("/:owner/:repo/info/lfs/objects/:repo_id/:oid/:size", lfs::upload)
        .get_async("/:owner/:repo/info/lfs/objects/:repo_id/:oid/:size", lfs::download)
        .post_async("/:owner/:repo/info/lfs/objects/:repo_id/:oid/:size/verify", lfs::verify)
        .post_async("/:owner/:repo/info/lfs/objects/:repo_id/:oid/:size/multipart", lfs::multipart)
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, worker_ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let request_id = match util::header(&req, "cf-ray").filter(|id| {
        (16..=32).contains(&id.len()) && id.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
    }) {
        Some(id) => id,
        None => crypto::random_token()?,
    };
    let machine = security::is_git_request(&req)
        || (req.method() == Method::Post && security::is_check_request(&req));
    let mut response = serve(req, env, worker_ctx, &request_id).await?;
    if machine {
        return Ok(response);
    }
    if response.headers_mut().set("X-Request-ID", &request_id).is_ok() {
        return Ok(response);
    }
    // A subrequest response can expose immutable headers; rebuild them so the
    // correlation header can still be set.
    let headers = Headers::new();
    for (name, value) in response.headers().entries() {
        headers.set(&name, &value)?;
    }
    headers.set("X-Request-ID", &request_id)?;
    Ok(response.with_headers(headers))
}

async fn serve(mut req: Request, env: Env, worker_ctx: Context, request_id: &str) -> Result<Response> {
    let secure = util::is_secure(&req);
    let is_git = security::is_git_request(&req);
    let is_check = req.method() == Method::Post && security::is_check_request(&req);
    let machine = is_git || is_check;
    let previous_csrf = if machine { None } else { util::cookie(&req, "shard_csrf") };
    let guest_preference = if machine {
        None
    } else {
        util::cookie(&req, LANGUAGE_COOKIE).as_deref().and_then(LanguagePreference::parse)
    };
    let browser_locale = || {
        if machine {
            Locale::En
        } else {
            util::header(&req, "accept-language").as_deref().map_or(Locale::En, Locale::negotiate)
        }
    };
    let source = is_git.then(|| handlers::git::direct_client_ip(&env, &req)).flatten()
        .or_else(|| util::header(&req, "cf-connecting-ip")).unwrap_or_else(|| "local".into());
    let class = if is_git { "git" } else if is_check { "checks" } else { "web" };
    if !env.rate_limiter("REQUEST_LIMITER")?.limit(format!("{class}:{source}")).await?.success {
        console_log!("request_id={request_id} event=rate_limit class={class}");
        let locale = guest_preference.and_then(LanguagePreference::locale).unwrap_or_else(browser_locale);
        let mut response = Response::error(locale.t(CommonText::RateLimit), 429)?;
        response.headers_mut().set("Retry-After", "60")?;
        response.headers_mut().set("Cache-Control", "no-store")?;
        if !machine {
            response.headers_mut().set("Content-Language", locale.code())?;
        }
        return Ok(response);
    }
    let user = match if machine { Ok(None) } else { auth::current_user(&env, &req).await } {
        Ok(user) => user,
        Err(error) => {
            console_log!("request_id={request_id} event=session_lookup_failed error={error}");
            let token = security::csrf_token(&req, false).await?;
            let locale = guest_preference.and_then(LanguagePreference::locale).unwrap_or_else(browser_locale);
            let response = templates::error_response(
                &templates::Chrome { locale, registration_open: auth::registration_open(&env), ..Default::default() },
                503,
                locale.t(CommonText::AccountUnavailable),
            )?;
            return security::finish(response, &token, previous_csrf.as_deref(), secure, locale);
        }
    };
    let language_preference = if machine {
        LanguagePreference::Browser
    } else {
        user.as_ref().and_then(|user| user.locale.as_deref()).and_then(LanguagePreference::parse)
            .or(guest_preference)
            .unwrap_or(LanguagePreference::Browser)
    };
    let locale = language_preference.locale().unwrap_or_else(browser_locale);
    let csrf_token = if machine { String::new() } else { security::csrf_token(&req, user.is_some()).await? };
    let mut state = AppState {
        env: env.clone(),
        worker_ctx: std::rc::Rc::new(worker_ctx),
        user,
        locale,
        language_preference,
        csrf_token,
        registration_open: auth::registration_open(&env),
        form: None,
    };
    let chrome = templates::Chrome::new(&state);

    if req.method() == Method::Post && !machine {
        let rejected = if !security::same_origin(&req) {
            Some((403, locale.t(CommonText::FormOrigin)))
        } else {
            let form = if req.path().trim_end_matches('/') == "/user/settings/avatar" {
                avatar::read_form(&mut req).await.map_err(|error| match error {
                    avatar::FormError::Invalid => (400, locale.t(AccountText::AvatarInvalidForm)),
                    avatar::FormError::TooLarge => (413, locale.t(AccountText::AvatarTooLarge)),
                })
            } else if req.path().trim_end_matches('/') == "/user/settings/gpg" {
                avatar::read_form(&mut req).await.map_err(|error| match error {
                    avatar::FormError::Invalid => (400, locale.t(i18n::GpgText::InvalidForm)),
                    avatar::FormError::TooLarge => (413, locale.t(i18n::GpgText::KeyTooLarge)),
                })
            } else if metadata_import::is_form_path(&req.path()) {
                avatar::read_form(&mut req).await.map_err(|error| match error {
                    avatar::FormError::Invalid => (400, locale.t(CommonText::UnreadableForm)),
                    avatar::FormError::TooLarge => (413, locale.t(i18n::LifecycleText::MetadataTooLarge)),
                })
            } else {
                req.form_data().await.map_err(|_| (400, locale.t(CommonText::UnreadableForm)))
            };
            match form {
                Ok(form) if security::valid_form(&form, &state.csrf_token) => {
                    state.form = Some(std::rc::Rc::new(form));
                    None
                }
                Ok(_) => Some((403, locale.t(CommonText::FormExpired))),
                Err(error) => Some(error),
            }
        };
        if let Some((status, message)) = rejected {
            let response = templates::error_response(&chrome, status, message)?;
            return security::finish(response, &state.csrf_token, previous_csrf.as_deref(), secure, locale);
        }
    }

    let router = routes(Router::with_data(state));
    let response = match router.run(req, env).await {
        Ok(response) if !machine && matches!(response.status_code(), 404 | 405)
            && !security::is_html(response.headers())? => {
                let status = response.status_code();
                let message = if status == 404 { CommonText::NotFound } else { CommonText::MethodNotAllowed };
                templates::error_response(&chrome, status, locale.t(message))?
            }
        Ok(response) => response,
        Err(error) => {
            console_log!("request_id={request_id} event=handler_error error={error}");
            if machine {
                Response::error(locale.t(CommonText::WorkerError), 500)?
            } else {
                templates::error_response(&chrome, 500, locale.t(CommonText::WorkerError))?
            }
        }
    };
    if machine {
        Ok(response)
    } else {
        security::finish(response, &chrome.csrf_token, previous_csrf.as_deref(), secure, locale)
    }
}

#[event(scheduled)]
async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    if event.cron() == "* * * * *" {
        let limits = match limits::ScheduledLimits::read(&env) {
            Ok(limits) => limits,
            Err(error) => {
                console_log!("event=scheduled_limits_invalid error={error}");
                return;
            }
        };
        if repo_lifecycle::reconcile_pending(&env, &limits).await.is_err() {
            console_log!("event=repository_copy_reconcile_failed");
        }
        if git_activity::reconcile(&env, limits.push_intents).await.is_err() {
            console_log!("event=git_push_reconcile_failed");
        }
        if pull_review::refresh_pending(&env, limits.pr_refreshes).await.is_err() {
            console_log!("event=pull_comparison_refresh_failed");
        }
        if branch_rules::process_queue(&env, limits.merges).await.is_err() {
            console_log!("event=merge_queue_failed");
        }
        if webhooks::deliver_pending(&env, limits.webhook_subscriptions, limits.webhook_legacy).await.is_err() {
            console_log!("event=webhook_delivery_failed");
        }
        return;
    }
    if let Err(error) = auth::cleanup_expired(&env).await {
        console_log!("credential cleanup failed: {error}");
    }
}
