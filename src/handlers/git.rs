//! Git smart-HTTP proxy.
//!
//! Authenticate against Shard and stream Git requests to the recorded backend.
use super::common::{RepoRole, not_found, repo_role};
use crate::auth;
use crate::git_storage::{is_object_id, GitBackend, GitStorage};
use crate::git_wire::{self, ReceiveRequest, RefUpdate};
use crate::ctx::AppState;
use crate::db;
use crate::models::GitIdentity;
use crate::i18n::{BranchText, CollaboratorText, CommonText, RepoText};
use crate::util;
use wasm_bindgen::JsCast;
use worker::*;

pub(crate) const ORIGIN_TOKEN: &str = "x-shard-git-origin-token";
const MAX_CONTROL_BYTES: usize = 512 * 1024;

#[derive(serde::Deserialize)]
struct GitRepo {
    id: i64,
    owner_id: i64,
    git_backend: GitBackend,
    storage_id: String,
    saved_remote: String,
    visibility: String,
    default_branch: String,
    git_head_override: i64,
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
    let Some(repo) = load_repo(&database, &owner, repo_name).await? else { return hidden_repo() };
    let request_url = req.url()?;
    let query = request_url.query().unwrap_or_default();
    // A push starts with a receive-pack advertisement, which also requires write access.
    let write = service == "git-receive-pack"
        || (service == "info/refs"
            && request_url.query_pairs().any(|(key, value)| {
                key == "service" && value == "git-receive-pack"
            }));
    let private = repo.visibility != "public";
    let identity = match authorize(state, &database, &repo, &req, write).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };

    let storage = match GitStorage::from_env(&state.env, repo.git_backend) {
        Ok(storage) => storage,
        Err(error) => return upstream_error(state, &error),
    };
    let handle = match storage.get(&repo.storage_id).await {
        Ok(handle) => handle,
        Err(error) => return upstream_error(state, &error),
    };

    let want_default = repo.git_head_override == 1 && !write;
    let receive = if service == "git-receive-pack" {
        let encoding = util::header(&req, "content-encoding").unwrap_or_default();
        let encoding = encoding.trim().to_ascii_lowercase();
        if !matches!(encoding.as_str(), "" | "identity" | "gzip") {
            return git_error(state, 415, BranchText::InvalidPush);
        }
        let push = match ReceiveRequest::inspect(&req, &encoding).await {
            Ok(push) => push,
            Err(_) => return git_error(state, 400, BranchText::InvalidPush),
        };
        if crate::branch_rules::blocks_push(&database, repo.id, &push.updates).await? {
            let _ = push.body.cancel();
            return git_error(state, 403, BranchText::ProtectedPush);
        }
        Some(push)
    } else {
        None
    };

    let headers = Headers::new();
    for name in ["content-type", "content-encoding", "accept", "git-protocol", "user-agent"] {
        if (want_default && name == "git-protocol") || (receive.is_some() && name == "content-encoding") {
            continue;
        }
        if let Some(value) = util::header(&req, name) {
            headers.set(name, &value)?;
        }
    }

    let method = req.method();
    // Inspect only the command prefix. Pack data still bypasses Wasm memory.
    let body = if let Some(push) = &receive {
        Some(push.body.clone().into())
    } else if method == Method::Post {
        req.inner().body().map(Into::into)
    } else {
        None
    };
    let intent = if let Some(push) = &receive {
        match crate::git_activity::stage(&state.env, repo.id, &push.updates).await {
            Ok(intent) => intent,
            Err(error) => {
                let _ = push.body.cancel();
                return Err(error);
            }
        }
    } else {
        None
    };

    let init = RequestInit {
        body,
        headers,
        method,
        ..Default::default()
    };
    let mut upstream = match handle.git_request(service, query, write, &repo.saved_remote, init).await {
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
    if upstream.status_code() == 200 {
        if let (Some(intent), Some(push)) = (intent, receive.filter(|push| push.report)) {
            if let ResponseBody::Stream(stream) = upstream.body() {
                let halves = stream.tee();
                let client = halves.get(0).unchecked_into();
                let observer = halves.get(1).unchecked_into();
                let env = state.env.clone();
                let actor = identity.as_ref().map(|id| id.user_id);
                state.worker_ctx.wait_until(async move {
                    if let Ok(accepted) = git_wire::receive_report(&observer, push.sideband).await {
                        if crate::git_activity::confirm(&env, intent, actor, &accepted).await.is_err() {
                            console_log!("event=git_push_confirmation_failed repo_id={}", repo.id);
                        }
                    }
                });
                upstream = git_wire::response_with_stream(upstream, client)?;
            }
        } else if want_default && service == "info/refs" && response_headers.get("content-type")?.as_deref()
            == Some("application/x-git-upload-pack-advertisement")
        {
            if let ResponseBody::Stream(stream) = upstream.body() {
                let stream = git_wire::default_head(stream, &repo.default_branch).await?;
                upstream = git_wire::response_with_stream(upstream, stream)?;
            }
        }
    }
    Ok(upstream.with_headers(response_headers))
}

async fn load_repo(database: &d1::D1Database, owner: &str, name: &str) -> Result<Option<GitRepo>> {
    db::first(database,
        "SELECT r.id, r.owner_id, r.git_backend, r.artifacts_repo AS storage_id,
            r.artifacts_remote AS saved_remote, r.visibility, r.default_branch, r.git_head_override
           FROM repo_paths p JOIN users pu ON pu.id = p.owner_id JOIN repos r ON r.id = p.repo_id
          WHERE pu.username = ? COLLATE NOCASE AND p.name = ? COLLATE NOCASE",
        &[util::js_text(owner), util::js_text(name)],
    ).await
}

async fn authorize(
    state: &AppState, database: &d1::D1Database, repo: &GitRepo, req: &Request, write: bool,
) -> Result<std::result::Result<Option<GitIdentity>, Response>> {
    let identity = match auth::git_identity(&state.env, req).await {
        Ok(identity) => identity,
        Err(Error::RustError(message)) if message == "invalid basic credentials" => None,
        Err(error) => return Err(error),
    };
    if !write && repo.visibility == "public" {
        return Ok(Ok(identity));
    }
    let Some(identity) = identity else {
        return Ok(Err(unauthorized(state.locale.t(CollaboratorText::GitCredentials))?));
    };
    let role = repo_role(database, repo.id, repo.owner_id, identity.user_id, identity.is_admin).await?;
    if repo.visibility != "public" && role.is_none() {
        return Ok(Err(hidden_repo()?));
    }
    if write && (role != Some(RepoRole::Write) || identity.scope != "write") {
        return Ok(Err(unauthorized(state.locale.t(CollaboratorText::GitWriteRequired))?));
    }
    Ok(Ok(Some(identity)))
}

pub(crate) fn direct_origin(env: &Env) -> Option<String> {
    crate::native_git::git_origin(&env.var("NATIVE_GIT_DIRECT_ORIGIN").ok()?.to_string())
}

pub(crate) fn trusted_direct_origin(env: &Env, req: &Request) -> Option<String> {
    let provided = util::header(req, ORIGIN_TOKEN)?;
    let expected = env.secret("NATIVE_GIT_TOKEN").ok()?.to_string();
    (!expected.is_empty() && crate::crypto::constant_time_eq(expected.as_bytes(), provided.as_bytes()))
        .then(|| direct_origin(env)).flatten()
}

pub(crate) fn direct_client_ip(env: &Env, req: &Request) -> Option<String> {
    let ip = util::header(req, "x-shard-git-client-ip")?.parse::<std::net::IpAddr>().ok()?;
    trusted_direct_origin(env, req).map(|_| ip.to_string())
}

#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum DirectCommand {
    Resolve { write: bool },
    Stage { storage: String, updates: Vec<RefUpdate> },
    Confirm { storage: String, intent: i64, actor: i64, accepted: Vec<String> },
}

pub(crate) async fn direct_control(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if trusted_direct_origin(&state.env, &req).is_none() {
        return hidden_repo();
    }
    let (Some(owner), Some(name)) = (ctx.param("owner"), ctx.param("repo").and_then(|name| name.strip_suffix(".git"))) else {
        return hidden_repo();
    };
    let invalid = |code| git_error(state, code, BranchText::InvalidPush);
    let length = util::header(&req, "content-length").and_then(|value| value.parse::<usize>().ok());
    let Some(length) = length.filter(|length| *length <= MAX_CONTROL_BYTES) else { return invalid(413) };
    if util::header(&req, "content-encoding").is_some() {
        return invalid(415);
    }
    let bytes = req.bytes().await?;
    let Ok(command) = serde_json::from_slice::<DirectCommand>(&bytes) else { return invalid(400) };
    if bytes.len() != length {
        return invalid(400);
    }
    let database = state.db()?;
    let Some(repo) = load_repo(&database, owner, name).await? else { return hidden_repo() };
    if !matches!(repo.git_backend, GitBackend::Native) {
        return hidden_repo();
    }
    let mut response = match command {
        DirectCommand::Resolve { write } => {
            if let Err(response) = authorize(state, &database, &repo, &req, write).await? {
                return Ok(response);
            }
            Response::from_json(&serde_json::json!({"storage": repo.storage_id}))?
        }
        DirectCommand::Stage { storage, updates } if storage == repo.storage_id => {
            let actor = match authorize(state, &database, &repo, &req, true).await? {
                Ok(Some(identity)) => identity.user_id,
                Err(response) => return Ok(response),
                Ok(None) => return invalid(400),
            };
            if updates.iter().any(|update| !is_object_id(&update.before) || !is_object_id(&update.after)
                || !update.name.starts_with("refs/") || update.name.chars().any(char::is_control))
            {
                return invalid(400);
            }
            if crate::branch_rules::blocks_push(&database, repo.id, &updates).await? {
                return git_error(state, 403, BranchText::ProtectedPush);
            }
            let intent = crate::git_activity::stage(&state.env, repo.id, &updates).await?;
            Response::from_json(&serde_json::json!({"intent": intent.as_ref().map(|intent| intent.id()), "actor": actor}))?
        }
        DirectCommand::Confirm { storage, intent, actor, accepted } if storage == repo.storage_id => {
            // Git has already committed these refs. Settle even if the user's token was just revoked.
            crate::git_activity::confirm_direct(&state.env, repo.id, intent, actor, &accepted).await?;
            Response::empty()?.with_status(204)
        }
        _ => return hidden_repo(),
    };
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

fn git_error(state: &AppState, status: u16, message: BranchText) -> Result<Response> {
    let mut response = Response::error(state.locale.t(message), status)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
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

