//! Git smart-HTTP proxy.
//!
//! Shard never stores objects itself: it authenticates the git client, mints a
//! short-lived Artifacts repo token and streams the request to the repository's
//! Artifacts remote. Clone URLs stay on the forge domain.
use super::common::not_found;
use crate::artifacts::Artifacts;
use crate::auth;
use crate::ctx::AppState;
use crate::db;
use crate::util;
use worker::*;

#[derive(serde::Deserialize)]
struct GitRepo {
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

async fn proxy(mut req: Request, ctx: RouteContext<AppState>, service: &str) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let raw_repo = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo_name) = raw_repo.strip_suffix(".git") else {
        return not_found(&state, "Git remotes use the `.git` suffix.");
    };

    let database = state.db()?;
    let repo: Option<GitRepo> = db::first(
        &database,
        "SELECT r.owner_id, r.artifacts_repo, r.artifacts_remote, r.visibility
           FROM repos r JOIN users u ON u.id = r.owner_id
          WHERE u.username = ? COLLATE NOCASE AND r.name = ? COLLATE NOCASE",
        &[util::js_text(&owner), util::js_text(repo_name)],
    )
    .await?;
    let Some(repo) = repo else {
        return not_found(&state, "Repository not found.");
    };

    let query = req
        .url()
        .ok()
        .and_then(|u| u.query().map(str::to_string))
        .unwrap_or_default();

    let identity = auth::git_identity(&state.env, &req).await?;
    // A push starts with GET info/refs?service=git-receive-pack; Artifacts
    // requires write scope for that advertisement too, not just the pack POST.
    let write = service == "git-receive-pack" || query.contains("service=git-receive-pack");
    if write {
        let allowed = match &identity {
            Some(id) => {
                id.scope == "write" && (id.user_id == repo.owner_id || id.is_admin == 1)
            }
            None => false,
        };
        if !allowed {
            return unauthorized("Pushing requires a shard access token with write scope.");
        }
    } else if repo.visibility != "public" {
        let allowed = match &identity {
            Some(id) => id.user_id == repo.owner_id,
            None => false,
        };
        if !allowed {
            return unauthorized("This repository is private.");
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
        url.push_str(&query);
    }

    let headers = Headers::new();
    for name in ["content-type", "accept", "git-protocol", "user-agent"] {
        if let Some(value) = util::header(&req, name) {
            headers.set(name, &value)?;
        }
    }
    headers.set("Authorization", &format!("Bearer {token}"))?;

    let body = if matches!(req.method(), Method::Post) {
        let bytes = req.bytes().await?;
        Some(js_sys::Uint8Array::from(bytes.as_slice()).into())
    } else {
        None
    };

    let init = RequestInit {
        body,
        headers,
        method: req.method(),
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
    let stream = upstream.stream()?;
    Ok(Response::from_stream(stream)?
        .with_status(status)
        .with_headers(response_headers))
}

fn upstream_error(stage: &str, error: &Error) -> Result<Response> {
    console_log!("git proxy: {stage}: {error}");
    Response::error(format!("{stage}: {error}"), 502)
}

fn unauthorized(message: &str) -> Result<Response> {
    let headers = Headers::new();
    headers.set("WWW-Authenticate", "Basic realm=\"shard\"")?;
    Ok(Response::error(message, 401)?.with_headers(headers))
}

