//! Git smart-HTTP proxy.
//!
//! Authenticate against Shard and stream Git requests to the recorded backend.
use super::common::{RepoRole, not_found, repo_role};
use crate::git_storage::{GitBackend, GitStorage};
use crate::auth;
use crate::ctx::AppState;
use crate::db;
use crate::i18n::{CollaboratorText, CommonText, RepoText};
use crate::util;
use worker::*;

#[derive(serde::Deserialize)]
struct GitRepo {
    id: i64,
    owner_id: i64,
    git_backend: GitBackend,
    storage_id: String,
    saved_remote: String,
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
        "SELECT r.id, r.owner_id, r.git_backend, r.artifacts_repo AS storage_id,
            r.artifacts_remote AS saved_remote, r.visibility
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

    let storage = match GitStorage::from_env(&state.env, repo.git_backend) {
        Ok(storage) => storage,
        Err(error) => return upstream_error(state, &error),
    };
    let handle = match storage.get(&repo.storage_id).await {
        Ok(handle) => handle,
        Err(error) => return upstream_error(state, &error),
    };

    let headers = Headers::new();
    for name in ["content-type", "content-encoding", "accept", "git-protocol", "user-agent"] {
        if let Some(value) = util::header(&req, name) {
            headers.set(name, &value)?;
        }
    }

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
        ..Default::default()
    };
    let upstream = match handle.git_request(service, query, write, &repo.saved_remote, init).await {
        Ok(response) => response,
        Err(error) => return upstream_error(state, &error),
    };
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
    Ok(upstream.with_headers(response_headers))
}

fn upstream_error(state: &AppState, error: &Error) -> Result<Response> {
    console_log!("git proxy: {error}");
    let mut response = Response::error(state.locale.t(RepoText::StorageUnavailable), 502)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
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

