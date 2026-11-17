//! Shard: a self-hosted, Cloudflare-native Git forge.
//!
//! Pages and metadata live in D1; repositories live in Artifacts. Git push and
//! fetch are proxied so users only ever see the forge domain.
use worker::*;

mod artifacts;
mod auth;
mod crypto;
mod ctx;
mod db;
mod handlers;
mod markdown;
mod models;
mod templates;
mod util;

use ctx::AppState;

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
async fn fetch(req: Request, env: Env, _worker_ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let user = auth::current_user(&env, &req).await.unwrap_or(None);
    let state = AppState {
        env: env.clone(),
        user,
    };
    let chrome = templates::Chrome::new(&state.user);
    let router = routes(Router::with_data(state));

    match router.run(req, env).await {
        Ok(response) => Ok(response),
        Err(error) => {
            console_log!("unhandled worker error: {error}");
            templates::error_response(chrome, 500, "Something went wrong. The request was logged.")
        }
    }
}
