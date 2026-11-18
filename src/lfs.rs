//! Backend-independent Git LFS basic transfers.
use crate::ctx::AppState;
use crate::handlers::common::{repo_role, RepoRole};
use crate::{auth, db, util};
use js_sys::{Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::ReadableStreamDefaultReader;
use worker::*;

const MEDIA: &str = "application/vnd.git-lfs+json";
const BINARY: &str = "application/octet-stream";
const MAX_JSON: usize = 64 * 1024;
const MAX_OBJECTS: usize = 100;
// Stay within the smallest Workers plan's incoming request size limit.
const MAX_OBJECT_BYTES: u64 = 100_000_000;

type HttpError = (u16, &'static str);
const HIDDEN: HttpError = (404, "Repository not found");
const CREDENTIALS: HttpError = (401, "A Shard access token is required");
const STORAGE: HttpError = (503, "LFS storage unavailable");
const MISSING: HttpError = (404, "LFS object not found");
const INVALID_OBJECT: HttpError = (422, "Invalid SHA-256 object ID or size");
const WRONG_SIZE: HttpError = (422, "Object size does not match");
const TOO_LARGE: HttpError = (413, "LFS object exceeds 100 MB");

#[derive(Deserialize)]
struct Repository {
    id: i64,
    owner_id: i64,
    visibility: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Download,
    Upload,
}

impl Operation {
    fn writes(self) -> bool {
        matches!(self, Self::Upload)
    }
}

#[derive(Deserialize)]
struct Batch {
    operation: Operation,
    transfers: Option<Vec<String>>,
    hash_algo: Option<String>,
    objects: Vec<ObjectSpec>,
}

#[derive(Deserialize, Serialize)]
struct ObjectSpec {
    oid: String,
    size: i64,
}

pub async fn batch(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let batch: Batch = match read_json(&req).await {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    let repo = match authorized(&req, &ctx, batch.operation).await {
        Ok(repo) => repo,
        Err(error) => return failure(error),
    };
    if batch.hash_algo.as_deref().is_some_and(|hash| hash != "sha256") {
        return failure((409, "Only SHA-256 object IDs are supported"));
    }
    if batch.transfers.is_some_and(|names| !names.iter().any(|name| name == "basic")) {
        return failure((422, "The basic transfer adapter is required"));
    }
    if batch.objects.len() > MAX_OBJECTS {
        return failure((413, "LFS batches accept at most 100 objects"));
    }

    let bucket = match ctx.data.env.bucket("LFS_OBJECTS") {
        Ok(bucket) => bucket,
        Err(_) => return failure(STORAGE),
    };
    let url = req.url()?;
    let Some(path) = url.path().strip_suffix("/batch") else {
        return failure(HIDDEN);
    };
    // Keep actions on the batch origin and leave credentials to the LFS client.
    let prefix = format!("{}{path}/{}", url.origin().ascii_serialization(), repo.id);
    let mut objects = Vec::with_capacity(batch.objects.len());
    for object in batch.objects {
        let mut entry = json!(object);
        match batch_object(&bucket, repo.id, &prefix, &object, batch.operation).await {
            Ok(Some(actions)) => entry["actions"] = actions,
            Ok(None) => {}
            Err((code, message)) => entry["error"] = json!({"code": code, "message": message}),
        }
        objects.push(entry);
    }
    json_response(200, &json!({"transfer": "basic", "hash_algo": "sha256", "objects": objects}))
}

async fn batch_object(
    bucket: &Bucket,
    repo_id: i64,
    prefix: &str,
    spec: &ObjectSpec,
    operation: Operation,
) -> std::result::Result<Option<serde_json::Value>, HttpError> {
    let size = u64::try_from(spec.size).map_err(|_| INVALID_OBJECT)?;
    validate_object(&spec.oid, size)?;
    let exists = match bucket.head(object_key(repo_id, &spec.oid)).await.map_err(|_| STORAGE)? {
        Some(object) if object.size() != size => return Err(WRONG_SIZE),
        Some(_) => true,
        None => false,
    };
    let href = format!("{prefix}/{}/{size}", spec.oid);
    match (operation, exists) {
        (Operation::Upload, true) => Ok(None),
        (Operation::Upload, false) => Ok(Some(json!({
            "upload": {"href": href, "header": {"Content-Type": BINARY}},
            "verify": {"href": format!("{href}/verify")},
        }))),
        (Operation::Download, true) => Ok(Some(json!({"download": {"href": href}}))),
        (Operation::Download, false) => Err(MISSING),
    }
}

pub async fn upload(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let (bucket, key, oid, size) = match transfer(&req, &ctx, Operation::Upload).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    let body = req.inner().body();
    if util::header(&req, "content-encoding").is_some_and(|value| !value.eq_ignore_ascii_case("identity")) {
        return failure((415, "LFS uploads require an unencoded body"));
    }
    match util::header(&req, "content-length") {
        Some(length) if decimal(&length) == Some(size) => {}
        None if size == 0 && body.is_none() => {}
        Some(_) => return failure(WRONG_SIZE),
        None => return failure((411, "LFS uploads require Content-Length")),
    }
    let data = match body {
        Some(stream) => Data::from(stream),
        None if size == 0 => Data::Empty,
        None => return failure(WRONG_SIZE),
    };
    // R2 consumes the original known-length Workers stream and validates the
    // digest before storing it. The conditional PUT keeps objects immutable.
    let put = bucket
        .put(&key, data)
        .sha256(digest(oid).to_vec())
        .only_if(Conditional {
            etag_does_not_match: Some("*".into()),
            ..Default::default()
        })
        .execute()
        .await;
    match put {
        Ok(Some(_)) => empty(),
        Ok(None) => check_stored(&bucket, key, size).await,
        Err(error) => {
            let message = error.to_string();
            if message.ends_with("(10037)") {
                failure((422, "Object content does not match its SHA-256 ID"))
            } else if message.ends_with("(10013)") || message.ends_with("(10054)") {
                failure(WRONG_SIZE)
            } else {
                failure(STORAGE)
            }
        }
    }
}

pub async fn download(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let (bucket, key, _, size) = match transfer(&req, &ctx, Operation::Download).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    match bucket.get(key).execute().await {
        Ok(Some(object)) if object.size() == size => {
            let Some(body) = object.body() else {
                return failure(STORAGE);
            };
            let mut response = Response::from_body(body.response_body()?)?;
            response.headers_mut().set("Content-Type", BINARY)?;
            response.headers_mut().set("Content-Length", &size.to_string())?;
            response.headers_mut().set("Cache-Control", "no-store")?;
            Ok(response)
        }
        Ok(Some(_)) => failure(WRONG_SIZE),
        Ok(None) => failure(MISSING),
        Err(_) => failure(STORAGE),
    }
}

pub async fn verify(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let (bucket, key, oid, size) = match transfer(&req, &ctx, Operation::Upload).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    let spec: ObjectSpec = match read_json(&req).await {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    if spec.oid != oid || u64::try_from(spec.size).ok() != Some(size) {
        return failure(INVALID_OBJECT);
    }
    check_stored(&bucket, key, size).await
}

async fn check_stored(bucket: &Bucket, key: String, size: u64) -> Result<Response> {
    match bucket.head(key).await {
        Ok(Some(object)) if object.size() == size => empty(),
        Ok(Some(_)) => failure(WRONG_SIZE),
        Ok(None) => failure(MISSING),
        Err(_) => failure(STORAGE),
    }
}

async fn transfer<'a>(
    req: &Request,
    ctx: &'a RouteContext<AppState>,
    operation: Operation,
) -> std::result::Result<(Bucket, String, &'a str, u64), HttpError> {
    let repo = authorized(req, ctx, operation).await?;
    if ctx.param("repo_id").and_then(|id| id.parse().ok()) != Some(repo.id) {
        return Err(HIDDEN);
    }
    let oid = ctx.param("oid").ok_or(INVALID_OBJECT)?;
    let size = ctx.param("size").and_then(|size| decimal(size)).ok_or(INVALID_OBJECT)?;
    validate_object(oid, size)?;
    let bucket = ctx.data.env.bucket("LFS_OBJECTS").map_err(|_| STORAGE)?;
    Ok((bucket, object_key(repo.id, oid), oid, size))
}

async fn authorized(
    req: &Request,
    ctx: &RouteContext<AppState>,
    operation: Operation,
) -> std::result::Result<Repository, HttpError> {
    let owner = ctx.param("owner").ok_or(HIDDEN)?;
    let name = ctx.param("repo").and_then(|repo| repo.strip_suffix(".git")).ok_or(HIDDEN)?;
    let database = ctx.data.db().map_err(|_| STORAGE)?;
    let repo: Repository = db::first(
        &database,
        "SELECT r.id, r.owner_id, r.visibility
           FROM repo_paths p JOIN users pu ON pu.id = p.owner_id JOIN repos r ON r.id = p.repo_id
          WHERE pu.username = ? COLLATE NOCASE AND p.name = ? COLLATE NOCASE",
        &[util::js_text(owner), util::js_text(name)],
    )
    .await
    .map_err(|_| STORAGE)?
    .ok_or(HIDDEN)?;
    let private = repo.visibility != "public";
    if !private && !operation.writes() {
        return Ok(repo);
    }
    let identity = auth::git_identity(&ctx.data.env, req)
        .await
        .map_err(|error| match error {
            Error::RustError(message) if message == "invalid basic credentials" => CREDENTIALS,
            _ => STORAGE,
        })?
        .ok_or(CREDENTIALS)?;
    let role = repo_role(&database, repo.id, repo.owner_id, identity.user_id, identity.is_admin)
        .await
        .map_err(|_| STORAGE)?;
    if private && role.is_none() {
        return Err(HIDDEN);
    }
    if operation.writes() && (role != Some(RepoRole::Write) || identity.scope != "write") {
        return Err((403, "LFS upload requires repository write access and a write token"));
    }
    Ok(repo)
}

fn validate_object(oid: &str, size: u64) -> std::result::Result<(), HttpError> {
    if oid.len() != 64 || !oid.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(INVALID_OBJECT);
    }
    if size > MAX_OBJECT_BYTES {
        return Err(TOO_LARGE);
    }
    Ok(())
}

fn digest(oid: &str) -> [u8; 32] {
    fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            _ => byte - b'a' + 10,
        }
    }
    let mut digest = [0; 32];
    let (pairs, _) = oid.as_bytes().as_chunks::<2>();
    for (byte, pair) in digest.iter_mut().zip(pairs) {
        *byte = nibble(pair[0]) << 4 | nibble(pair[1]);
    }
    digest
}

fn object_key(repo_id: i64, oid: &str) -> String {
    format!("lfs/{repo_id}/{oid}")
}

fn decimal(text: &str) -> Option<u64> {
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

async fn read_json<T: serde::de::DeserializeOwned>(req: &Request) -> std::result::Result<T, HttpError> {
    const INVALID: HttpError = (422, "Invalid LFS JSON payload");
    const LIMIT: HttpError = (413, "LFS JSON exceeds 64 KiB");
    if !util::header(req, "content-type")
        .and_then(|value| value.split(';').next().map(|media| media.trim().eq_ignore_ascii_case(MEDIA)))
        .unwrap_or(false)
    {
        return Err((415, "Content-Type must be application/vnd.git-lfs+json"));
    }
    if let Some(header) = util::header(req, "content-length") {
        if decimal(&header).ok_or(INVALID)? > MAX_JSON as u64 {
            return Err(LIMIT);
        }
    }
    let stream = req.inner().body().ok_or(INVALID)?;
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let done = JsValue::from_str("done");
    let value = JsValue::from_str("value");
    let mut bytes = Vec::new();
    loop {
        let result = JsFuture::from(reader.read()).await.map_err(|_| INVALID)?;
        if Reflect::get(&result, &done).ok().and_then(|value| value.as_bool()) == Some(true) {
            reader.release_lock();
            return serde_json::from_slice(&bytes).map_err(|_| INVALID);
        }
        let chunk = Reflect::get(&result, &value)
            .ok()
            .and_then(|value| value.dyn_into::<Uint8Array>().ok())
            .ok_or(INVALID)?;
        let start = bytes.len();
        let len = chunk.length() as usize;
        if len > MAX_JSON - start {
            let _ = JsFuture::from(reader.cancel()).await;
            return Err(LIMIT);
        }
        bytes.resize(start + len, 0);
        chunk.copy_to(&mut bytes[start..]);
    }
}

fn empty() -> Result<Response> {
    Ok(Response::builder().with_header("Cache-Control", "no-store")?.empty())
}

fn json_response(status: u16, value: &serde_json::Value) -> Result<Response> {
    let mut response = Response::from_json(value)?.with_status(status);
    response.headers_mut().set("Content-Type", MEDIA)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

fn failure((status, message): HttpError) -> Result<Response> {
    let mut response = json_response(status, &json!({"message": message}))?;
    if status == 401 {
        response.headers_mut().set("LFS-Authenticate", "Basic realm=\"shard\"")?;
        response.headers_mut().set("WWW-Authenticate", "Basic realm=\"shard\"")?;
    }
    Ok(response)
}
