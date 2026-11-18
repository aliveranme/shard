//! Git smart-HTTP proxy.
//!
//! Shard never stores objects itself: it authenticates the git client, mints a
//! short-lived Artifacts repo token and streams the request to the repository's
//! Artifacts remote. Clone URLs stay on the forge domain.
use super::common::{RepoRole, not_found, repo_role};
use crate::artifacts::Artifacts;
use crate::auth;
use crate::ctx::AppState;
use crate::db;
use crate::i18n::{CollaboratorText, CommonText};
use crate::util;
use worker::*;

#[derive(serde::Deserialize)]
struct GitRepo {
    id: i64,
    owner_id: i64,
    artifacts_repo: String,
    artifacts_remote: String,
    visibility: String,
}

pub async fn info_refs(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    proxy(req, ctx, "info/refs").await
}

pub async fn upload_pack(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    proxy(req, ctx, "git-upload-pack").await
}

pub async fn receive_pack(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    proxy(req, ctx, "git-receive-pack").await
}

async fn proxy(req: Request, ctx: RouteContext<AppState>, service: &str) -> Result<Response> {
    let state = &ctx.data;
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let raw_repo = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo_name) = raw_repo.strip_suffix(".git") else {
        return not_found(state, state.locale.t(CommonText::GitSuffixRequired));
    };

    let database = state.db()?;
    let repo: Option<GitRepo> = db::first(
        &database,
        "SELECT r.id, r.owner_id, r.artifacts_repo, r.artifacts_remote, r.visibility
           FROM repo_paths p JOIN users pu ON pu.id = p.owner_id JOIN repos r ON r.id = p.repo_id
          WHERE pu.username = ? COLLATE NOCASE AND p.name = ? COLLATE NOCASE",
        &[util::js_text(&owner), util::js_text(repo_name)],
    )
    .await?;
    let Some(repo) = repo else {
        return hidden_repo();
    };

    let request_url = req.url()?;
    let query = request_url.query().unwrap_or_default();
    // A push starts with a receive-pack advertisement, which also requires write access.
    let write = service == "git-receive-pack"
        || (service == "info/refs"
            && request_url.query_pairs().any(|(key, value)| {
                key == "service" && value == "git-receive-pack"
            }));
    let private = repo.visibility != "public";
    let identity = auth::git_identity(&state.env, &req).await?;
    if write || private {
        let Some(id) = identity else {
            return unauthorized(state.locale.t(CollaboratorText::GitCredentials));
        };
        let role = repo_role(&database, repo.id, repo.owner_id, id.user_id, id.is_admin).await?;
        if private && role.is_none() {
            return hidden_repo();
        }
        if write && (role != Some(RepoRole::Write) || id.scope != "write") {
            return unauthorized(state.locale.t(CollaboratorText::GitWriteRequired));
        }
    }

    let artifacts = match Artifacts::from_env(&state.env) {
        Ok(artifacts) => artifacts,
        Err(e) => return upstream_error("artifacts binding", &e),
    };
    let handle = match artifacts.get(&repo.artifacts_repo).await {
        Ok(handle) => handle,
        Err(e) => return upstream_error("artifacts get", &e),
    };
    let token = match handle
        .create_token(if write { "write" } else { "read" }, 3600)
        .await
    {
        Ok(token) => token,
        Err(e) => return upstream_error("artifacts token", &e),
    };
    let remote = match handle.remote().await {
        Ok(remote) => remote,
        Err(_) => repo.artifacts_remote.clone(),
    };
    if remote.is_empty() {
        return Response::error("Artifacts remote is unavailable", 503);
    }

    let mut url = format!("{}/{}", remote.trim_end_matches('/'), service);
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }

    let headers = Headers::new();
    for name in ["content-type", "accept", "git-protocol", "user-agent"] {
        if let Some(value) = util::header(&req, name) {
            headers.set(name, &value)?;
        }
    }
    headers.set("Authorization", &format!("Bearer {token}"))?;

    let method = req.method();
    // Forward the original Workers stream without copying pack data into Wasm.
    let body = if method == Method::Post {
        req.inner().body().map(Into::into)
    } else {
        None
    };

    let init = RequestInit {
        body,
        headers,
        method,
        redirect: RequestRedirect::Manual,
        ..Default::default()
    };
    let outbound = match Request::new_with_init(&url, &init) {
        Ok(request) => request,
        Err(e) => return upstream_error("git request", &e),
    };
    let mut upstream = match Fetch::Request(outbound).send().await {
        Ok(response) => response,
        Err(e) => return upstream_error("git fetch", &e),
    };

    let status = upstream.status_code();
    let response_headers = Headers::new();
    for name in [
        "content-type",
        "cache-control",
        "expires",
        "pragma",
        "www-authenticate",
    ] {
        if let Ok(Some(value)) = upstream.headers().get(name) {
            response_headers.set(name, &value)?;
        }
    }
    if private {
        response_headers.set("Cache-Control", "no-store")?;
    }
    let stream = upstream.stream()?;
    Ok(Response::from_stream(stream)?
        .with_status(status)
        .with_headers(response_headers))
}

fn upstream_error(stage: &str, error: &Error) -> Result<Response> {
    console_log!("git proxy: {stage}: {error}");
    Response::error(format!("{stage}: {error}"), 502)
}

fn hidden_repo() -> Result<Response> {
    Ok(Response::builder()
        .with_status(404)
        .with_header("Cache-Control", "no-store")?
        .empty())
}

fn unauthorized(message: &str) -> Result<Response> {
    let headers = Headers::new();
    headers.set("WWW-Authenticate", "Basic realm=\"shard\"")?;
    headers.set("Cache-Control", "no-store")?;
    Ok(Response::error(message, 401)?.with_headers(headers))
}

