//! Git smart-HTTP proxy.
//!
//! Authenticate against Shard and stream Git requests to the recorded backend.
use super::common::{not_found, repo_role, RepoRole};
use crate::auth;
use crate::ctx::AppState;
use crate::db;
use crate::git_activity::{self, Admission};
use crate::git_storage::{is_object_id, GitBackend, GitStorage};
use crate::git_wire::{self, ReceiveRequest, RefUpdate};
use crate::i18n::{BranchText, CollaboratorText, CommonText, RepoText};
use crate::models::GitIdentity;
use crate::native_git::{GitService, GitServiceRepo};
use crate::util;
use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::{ReadableStream, ReadableStreamDefaultReader};
use worker::*;

pub(crate) const ORIGIN_TOKEN: &str = "x-shard-git-origin-token";
const MAX_CONTROL_BYTES: usize = 512 * 1024;
const MAX_MULTIPART_COMMAND_BYTES: usize = 4096;

#[derive(serde::Deserialize)]
struct PreparedPush {
    prepared: String,
    updates: Vec<RefUpdate>,
    accepted: Vec<String>,
}

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
    let identity = resolve_git_identity(&state.env, &req).await?;
    let Some(repo) = load_repo(&database, &owner, repo_name).await? else {
        return missing_repo(state, identity.as_ref());
    };
    let request_url = req.url()?;
    let query = request_url.query().unwrap_or_default();
    // A push starts with a receive-pack advertisement, which also requires write access.
    let write = service == "git-receive-pack"
        || (service == "info/refs"
            && request_url
                .query_pairs()
                .any(|(key, value)| key == "service" && value == "git-receive-pack"));
    let private = repo.visibility != "public";
    let identity = match authorize_identity(state, &database, &repo, identity, write).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };

    let r2 = matches!(repo.git_backend, GitBackend::R2);
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
        let max_duration_ms = if r2 { 0 } else { 270_000 };
        let push = match ReceiveRequest::inspect(&req, &encoding, max_duration_ms).await {
            Ok(push) => push,
            Err(_) => return git_error(state, 400, BranchText::InvalidPush),
        };
        if r2 && git_activity::blocks_push(&database, repo.id, &push.updates).await? {
            let _ = push.body.cancel();
            return git_error(state, 403, BranchText::ProtectedPush);
        }
        Some(push)
    } else {
        None
    };

    let headers = Headers::new();
    for name in [
        "content-type",
        "content-encoding",
        "accept",
        "git-protocol",
        "user-agent",
    ] {
        if (want_default && name == "git-protocol")
            || (receive.is_some() && name == "content-encoding")
        {
            continue;
        }
        if let Some(value) = util::header(&req, name) {
            headers.set(name, &value)?;
        }
    }
    if r2 && receive.is_some() {
        let Some(identity) = &identity else {
            return hidden_repo();
        };
        headers.set("x-shard-git-actor", &identity.user_id.to_string())?;
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
    let intent = if let Some(push) = receive.as_ref().filter(|_| !r2) {
        match git_activity::stage(&database, repo.id, &push.updates, None).await {
            Ok(Admission::Admitted(intent)) => intent,
            Ok(Admission::Blocked) => {
                let _ = push.body.cancel();
                return git_error(state, 403, BranchText::ProtectedPush);
            }
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
    let mut upstream = match handle
        .git_request(service, query, write, &repo.saved_remote, init)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            if let Some(intent) = intent {
                let env = state.env.clone();
                let repo_id = repo.id;
                let intent_id = intent.id();
                state.worker_ctx.wait_until(async move {
                    git_activity::release_admission(&env, repo_id, intent_id).await;
                });
            }
            return upstream_error(state, &error);
        }
    };
    if r2 && receive.is_some() && upstream.status_code() == 202 {
        let (crate::git_storage::RepoHandle::Service(handle), Some(identity)) =
            (&handle, &identity)
        else {
            return hidden_repo();
        };
        upstream = publish_r2(&ctx, &req, &repo, handle, identity.user_id, upstream).await?;
    }
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
    if r2 {
        if let Some(value) = upstream.headers().get("retry-after")? {
            response_headers.set("Retry-After", &value)?;
        }
    }
    if private {
        response_headers.set("Cache-Control", "no-store")?;
    }
    if let (Some(intent), Some(push)) = (intent, receive) {
        let env = state.env.clone();
        let actor = identity.as_ref().map(|id| id.user_id);
        let repo_id = repo.id;
        let intent_id = intent.id();
        if upstream.status_code() == 200 && push.report {
            if let ResponseBody::Stream(stream) = upstream.body() {
                let halves = stream.tee();
                let client = halves.get(0).unchecked_into();
                let observer = halves.get(1).unchecked_into();
                state.worker_ctx.wait_until(async move {
                    match git_wire::receive_report(&observer, push.sideband).await {
                        Ok(accepted) => {
                            if git_activity::confirm(&env, intent, actor, &accepted)
                                .await
                                .is_err()
                            {
                                console_log!(
                                    "event=git_push_confirmation_failed repo_id={repo_id}"
                                );
                            }
                        }
                        Err(_) => {
                            git_activity::release_admission(&env, repo_id, intent_id).await;
                        }
                    }
                });
                upstream = git_wire::response_with_stream(upstream, client)?;
            } else {
                state.worker_ctx.wait_until(async move {
                    git_activity::release_admission(&env, repo_id, intent_id).await;
                });
            }
        } else if upstream.status_code() == 200 {
            if let ResponseBody::Stream(stream) = upstream.body() {
                let halves = stream.tee();
                let client = halves.get(0).unchecked_into();
                let observer = halves.get(1).unchecked_into();
                state.worker_ctx.wait_until(async move {
                    drain_stream(observer).await;
                    git_activity::release_admission(&env, repo_id, intent_id).await;
                });
                upstream = git_wire::response_with_stream(upstream, client)?;
            } else {
                state.worker_ctx.wait_until(async move {
                    git_activity::release_admission(&env, repo_id, intent_id).await;
                });
            }
        } else if (400..500).contains(&upstream.status_code()) {
            state.worker_ctx.wait_until(async move {
                if git_activity::confirm(&env, intent, actor, &[])
                    .await
                    .is_err()
                {
                    console_log!("event=git_push_confirmation_failed repo_id={repo_id}");
                }
            });
        } else {
            state.worker_ctx.wait_until(async move {
                git_activity::release_admission(&env, repo_id, intent_id).await;
            });
        }
    } else if upstream.status_code() == 200
        && want_default
        && service == "info/refs"
        && response_headers.get("content-type")?.as_deref()
            == Some("application/x-git-upload-pack-advertisement")
    {
        if let ResponseBody::Stream(stream) = upstream.body() {
            let stream = git_wire::default_head(stream, &repo.default_branch).await?;
            upstream = git_wire::response_with_stream(upstream, stream)?;
        }
    }
    Ok(upstream.with_headers(response_headers))
}

pub(crate) async fn multipart_upload(
    req: Request,
    ctx: RouteContext<AppState>,
) -> Result<Response> {
    let state = &ctx.data;
    let invalid = |code| git_error(state, code, BranchText::InvalidPush);
    let (Some(owner), Some(name)) = (
        ctx.param("owner"),
        ctx.param("repo").and_then(|name| name.strip_suffix(".git")),
    ) else {
        return hidden_repo();
    };
    let database = state.db()?;
    let identity = resolve_git_identity(&state.env, &req).await?;
    let Some(repo) = load_repo(&database, owner, name).await? else {
        return missing_repo(state, identity.as_ref());
    };
    let actor = match authorize_identity(state, &database, &repo, identity, true).await? {
        Ok(Some(identity)) => identity.user_id,
        Err(response) => return Ok(response),
        Ok(None) => return hidden_repo(),
    };
    if !matches!(repo.git_backend, GitBackend::R2) {
        return hidden_repo();
    }
    let length = util::header(&req, "content-length").and_then(|value| value.parse::<usize>().ok());
    let Some(length) = length.filter(|length| *length <= MAX_MULTIPART_COMMAND_BYTES) else {
        return invalid(413);
    };
    if util::header(&req, "content-encoding").is_some() {
        return invalid(415);
    }
    let bytes = match read_bounded_bytes(&req, length).await {
        Ok(bytes) => bytes,
        Err(code) => return invalid(code),
    };
    if bytes.len() != length {
        return invalid(400);
    }
    let Ok(command) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return invalid(400);
    };
    let Some(action @ ("start" | "part" | "finish" | "abort")) =
        command.get("action").and_then(|value| value.as_str())
    else {
        return invalid(400);
    };
    let handle = match GitService::r2(&state.env).and_then(|service| service.repo(&repo.storage_id))
    {
        Ok(handle) => handle,
        Err(error) => return upstream_error(state, &error),
    };
    let finish = action == "finish";
    let payload = serde_json::json!({"actor": actor.to_string(), "command": command});
    let response = match handle.r2_control("multipart", &payload).await {
        Ok(response) if finish && response.status_code() == 202 => {
            publish_r2(&ctx, &req, &repo, &handle, actor, response).await?
        }
        Ok(response) => response,
        Err(error) => return upstream_error(state, &error),
    };
    no_store(response)
}

async fn publish_r2(
    ctx: &RouteContext<AppState>,
    req: &Request,
    original: &GitRepo,
    handle: &GitServiceRepo,
    actor: i64,
    mut response: Response,
) -> Result<Response> {
    let state = &ctx.data;
    let prepared: PreparedPush = match response.json().await {
        Ok(prepared) => prepared,
        Err(error) => return upstream_error(state, &error),
    };
    let abort_prepared = |prepared_id: String| async move {
        let payload = serde_json::json!({
            "prepared": prepared_id,
            "actor": actor.to_string(),
            "abort": true,
        });
        let _ = handle.r2_control("publish", &payload).await;
    };
    let (Some(owner), Some(name)) = (
        ctx.param("owner"),
        ctx.param("repo").and_then(|name| name.strip_suffix(".git")),
    ) else {
        abort_prepared(prepared.prepared).await;
        return hidden_repo();
    };
    let database = state.db()?;
    let Some(repo) = load_repo(&database, owner, name).await? else {
        abort_prepared(prepared.prepared).await;
        return hidden_repo();
    };
    if repo.id != original.id
        || repo.storage_id != original.storage_id
        || !matches!(repo.git_backend, GitBackend::R2)
    {
        abort_prepared(prepared.prepared).await;
        return hidden_repo();
    }
    let identity = resolve_git_identity(&state.env, req).await?;
    match authorize_identity(state, &database, &repo, identity, true).await? {
        Ok(Some(identity)) if identity.user_id == actor => {}
        Err(response) => {
            abort_prepared(prepared.prepared).await;
            return Ok(response);
        }
        _ => {
            abort_prepared(prepared.prepared).await;
            return hidden_repo();
        }
    }
    if git_activity::blocks_push(&database, repo.id, &prepared.updates).await? {
        abort_prepared(prepared.prepared).await;
        return git_error(state, 403, BranchText::ProtectedPush);
    }
    // Reconciliation should only inspect refs Git accepted during preparation.
    let updates: Vec<_> = prepared
        .updates
        .into_iter()
        .filter(|update| prepared.accepted.contains(&update.name))
        .collect();
    let Admission::Admitted(intent) =
        git_activity::stage(&database, repo.id, &updates, None).await?
    else {
        abort_prepared(prepared.prepared).await;
        return git_error(state, 403, BranchText::ProtectedPush);
    };
    let payload = serde_json::json!({"prepared": prepared.prepared, "actor": actor.to_string()});
    let response = match handle.r2_control("publish", &payload).await {
        Ok(response) => response,
        Err(error) => {
            if let Some(intent) = intent {
                let env = state.env.clone();
                let repo_id = repo.id;
                let intent_id = intent.id();
                state.worker_ctx.wait_until(async move {
                    git_activity::release_admission(&env, repo_id, intent_id).await;
                });
            }
            return upstream_error(state, &error);
        }
    };
    if let Some(intent) = intent {
        let env = state.env.clone();
        if matches!(response.status_code(), 200 | 409) {
            let accepted = if response.status_code() == 200
                && response.headers().get("X-Shard-Replayed-Push")?.is_none()
            {
                prepared.accepted
            } else {
                Vec::new()
            };
            state.worker_ctx.wait_until(async move {
                if git_activity::confirm(&env, intent, Some(actor), &accepted)
                    .await
                    .is_err()
                {
                    console_log!("event=git_push_confirmation_failed repo_id={}", repo.id);
                }
            });
        } else {
            let repo_id = repo.id;
            let intent_id = intent.id();
            state.worker_ctx.wait_until(async move {
                git_activity::release_admission(&env, repo_id, intent_id).await;
            });
        }
    }
    Ok(response)
}

fn no_store(response: Response) -> Result<Response> {
    let headers = Headers::new();
    for name in ["content-type", "retry-after", "www-authenticate"] {
        if let Some(value) = response.headers().get(name)? {
            headers.set(name, &value)?;
        }
    }
    headers.set("Cache-Control", "no-store")?;
    Ok(response.with_headers(headers))
}

async fn load_repo(database: &d1::D1Database, owner: &str, name: &str) -> Result<Option<GitRepo>> {
    db::first(
        database,
        "SELECT r.id, r.owner_id, r.git_backend, r.artifacts_repo AS storage_id,
            r.artifacts_remote AS saved_remote, r.visibility, r.default_branch, r.git_head_override
           FROM repo_paths p JOIN users pu ON pu.id = p.owner_id JOIN repos r ON r.id = p.repo_id
          WHERE pu.username = ? COLLATE NOCASE AND p.name = ? COLLATE NOCASE",
        &[util::js_text(owner), util::js_text(name)],
    )
    .await
}

async fn resolve_git_identity(env: &Env, req: &Request) -> Result<Option<GitIdentity>> {
    match auth::git_identity(env, req).await {
        Ok(identity) => Ok(identity),
        Err(Error::RustError(message)) if message == "invalid basic credentials" => Ok(None),
        Err(error) => Err(error),
    }
}

fn missing_repo(state: &AppState, identity: Option<&GitIdentity>) -> Result<Response> {
    if identity.is_none() {
        unauthorized(state.locale.t(CollaboratorText::GitCredentials))
    } else {
        hidden_repo()
    }
}

async fn authorize_identity(
    state: &AppState,
    database: &d1::D1Database,
    repo: &GitRepo,
    identity: Option<GitIdentity>,
    write: bool,
) -> Result<std::result::Result<Option<GitIdentity>, Response>> {
    if !write && repo.visibility == "public" {
        return Ok(Ok(identity));
    }
    let Some(identity) = identity else {
        return Ok(Err(unauthorized(
            state.locale.t(CollaboratorText::GitCredentials),
        )?));
    };
    let role = repo_role(
        database,
        repo.id,
        repo.owner_id,
        identity.user_id,
        identity.is_admin,
    )
    .await?;
    if repo.visibility != "public" && role.is_none() {
        return Ok(Err(hidden_repo()?));
    }
    if write && (role != Some(RepoRole::Write) || identity.scope != "write") {
        return Ok(Err(unauthorized(
            state.locale.t(CollaboratorText::GitWriteRequired),
        )?));
    }
    Ok(Ok(Some(identity)))
}

pub(crate) fn direct_origin(env: &Env) -> Option<String> {
    crate::native_git::git_origin(&env.var("NATIVE_GIT_DIRECT_ORIGIN").ok()?.to_string())
}

pub(crate) fn trusted_direct_origin(env: &Env, req: &Request) -> Option<String> {
    let provided = util::header(req, ORIGIN_TOKEN)?;
    let expected = env.secret("NATIVE_GIT_TOKEN").ok()?.to_string();
    (!expected.is_empty()
        && crate::crypto::constant_time_eq(expected.as_bytes(), provided.as_bytes()))
    .then(|| direct_origin(env))
    .flatten()
}

pub(crate) fn direct_client_ip(env: &Env, req: &Request) -> Option<String> {
    let ip = util::header(req, "x-shard-git-client-ip")?
        .parse::<std::net::IpAddr>()
        .ok()?;
    trusted_direct_origin(env, req).map(|_| ip.to_string())
}

#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum DirectCommand {
    Resolve {
        write: bool,
    },
    Stage {
        storage: String,
        updates: Vec<RefUpdate>,
    },
    Confirm {
        storage: String,
        intent: i64,
        actor: i64,
        accepted: Vec<String>,
    },
    Release {
        storage: String,
        intent: i64,
    },
}

pub(crate) async fn direct_control(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if trusted_direct_origin(&state.env, &req).is_none() {
        return hidden_repo();
    }
    let (Some(owner), Some(name)) = (
        ctx.param("owner"),
        ctx.param("repo").and_then(|name| name.strip_suffix(".git")),
    ) else {
        return hidden_repo();
    };
    let invalid = |code| git_error(state, code, BranchText::InvalidPush);
    let length = util::header(&req, "content-length").and_then(|value| value.parse::<usize>().ok());
    let Some(length) = length.filter(|length| *length <= MAX_CONTROL_BYTES) else {
        return invalid(413);
    };
    if util::header(&req, "content-encoding").is_some() {
        return invalid(415);
    }
    let bytes = match read_bounded_bytes(&req, length).await {
        Ok(bytes) => bytes,
        Err(code) => return invalid(code),
    };
    if bytes.len() != length {
        return invalid(400);
    }
    let Ok(command) = serde_json::from_slice::<DirectCommand>(&bytes) else {
        return invalid(400);
    };
    let database = state.db()?;
    let identity = resolve_git_identity(&state.env, &req).await?;
    let Some(repo) = load_repo(&database, owner, name).await? else {
        return missing_repo(state, identity.as_ref());
    };
    if !matches!(repo.git_backend, GitBackend::Native) {
        return hidden_repo();
    }
    let mut response = match command {
        DirectCommand::Resolve { write } => {
            if let Err(response) =
                authorize_identity(state, &database, &repo, identity, write).await?
            {
                return Ok(response);
            }
            Response::from_json(&serde_json::json!({"storage": repo.storage_id}))?
        }
        DirectCommand::Stage { storage, updates } if storage == repo.storage_id => {
            let actor = match authorize_identity(state, &database, &repo, identity, true).await? {
                Ok(Some(identity)) => identity.user_id,
                Err(response) => return Ok(response),
                Ok(None) => return invalid(400),
            };
            if updates.iter().any(|update| {
                !is_object_id(&update.before)
                    || !is_object_id(&update.after)
                    || !update.name.starts_with("refs/")
                    || update.name.bytes().any(|byte| byte <= 32 || byte == 127)
                    || crate::git_wire::has_noncanonical_ref_namespace(&update.name)
                    || update
                        .name
                        .strip_prefix("refs/heads/")
                        .or_else(|| update.name.strip_prefix("refs/tags/"))
                        .is_some_and(|short| !crate::git_wire::valid_ref(short))
            }) {
                return invalid(400);
            }
            let Admission::Admitted(intent) =
                git_activity::stage(&database, repo.id, &updates, None).await?
            else {
                return git_error(state, 403, BranchText::ProtectedPush);
            };
            Response::from_json(
                &serde_json::json!({"intent": intent.as_ref().map(|intent| intent.id()), "actor": actor}),
            )?
        }
        DirectCommand::Confirm {
            storage,
            intent,
            actor,
            accepted,
        } if storage == repo.storage_id => {
            // Git has already committed these refs. Settle even if the user's token was just revoked.
            git_activity::confirm_direct(&state.env, repo.id, intent, actor, &accepted).await?;
            Response::empty()?.with_status(204)
        }
        DirectCommand::Release { storage, intent } if storage == repo.storage_id => {
            git_activity::release_admission(&state.env, repo.id, intent).await;
            Response::empty()?.with_status(204)
        }
        _ => return hidden_repo(),
    };
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

async fn read_bounded_bytes(req: &Request, limit: usize) -> std::result::Result<Vec<u8>, u16> {
    let Some(stream) = req.inner().body() else {
        return Ok(Vec::new());
    };
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let done = JsValue::from_str("done");
    let value = JsValue::from_str("value");
    let mut bytes = Vec::with_capacity(limit.min(8192));
    loop {
        let result = JsFuture::from(reader.read()).await.map_err(|_| 400u16)?;
        if Reflect::get(&result, &done).ok().and_then(|v| v.as_bool()) == Some(true) {
            reader.release_lock();
            return Ok(bytes);
        }
        let chunk = Reflect::get(&result, &value)
            .ok()
            .and_then(|v| v.dyn_into::<Uint8Array>().ok())
            .ok_or(400u16)?;
        let start = bytes.len();
        let len = chunk.length() as usize;
        if len > limit - start {
            let _ = JsFuture::from(reader.cancel()).await;
            reader.release_lock();
            return Err(413);
        }
        bytes.resize(start + len, 0);
        chunk.copy_to(&mut bytes[start..]);
    }
}

async fn drain_stream(stream: ReadableStream) {
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let done = JsValue::from_str("done");
    while let Ok(result) = JsFuture::from(reader.read()).await {
        if Reflect::get(&result, &done).ok().and_then(|v| v.as_bool()) == Some(true) {
            break;
        }
    }
    reader.release_lock();
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
