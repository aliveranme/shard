//! Backend-independent Git LFS transfers.
use crate::ctx::AppState;
use crate::handlers::common::{repo_role, RepoRole};
use crate::{auth, db, limits, util};
use js_sys::{Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::ReadableStreamDefaultReader;
use worker::*;

#[path = "lfs_remote.rs"]
mod remote;

const MEDIA: &str = "application/vnd.git-lfs+json";
const BINARY: &str = "application/octet-stream";
const MAX_JSON: usize = 64 * 1024;
const MAX_OBJECTS: usize = 100;
// Leave time for the storage request to settle after its stream is cancelled.
const WRITE_GRACE: i64 = 30;
pub(crate) const WRITE_LEASE: i64 = 15 * 60 + WRITE_GRACE;

#[wasm_bindgen(inline_js = r#"
export function shard_lfs_stream(body, size, timeout) {
    const { readable, writable } = new FixedLengthStream(size);
    // A failed pipe errors the readable stream consumed by R2.
    body.pipeTo(writable, { signal: AbortSignal.timeout(timeout) }).catch(() => {});
    return readable;
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    fn shard_lfs_stream(
        body: worker::web_sys::ReadableStream,
        size: f64,
        timeout: u32,
    ) -> std::result::Result<worker::web_sys::ReadableStream, JsValue>;
}

type HttpError = (u16, &'static str);
const HIDDEN: HttpError = (404, "Repository not found");
const CREDENTIALS: HttpError = (401, "A Shard access token is required");
const STORAGE: HttpError = (503, "LFS storage unavailable");
const MISSING: HttpError = (404, "LFS object not found");
const INVALID_OBJECT: HttpError = (422, "Invalid SHA-256 object ID or size");
const WRONG_SIZE: HttpError = (422, "Object size does not match");
const TOO_LARGE: HttpError = (413, "LFS object exceeds the configured upload limit");
const BAD_DIGEST: HttpError = (422, "Object content does not match its SHA-256 ID");

#[derive(Deserialize)]
struct Repository {
    id: i64,
    lfs_copy_id: Option<i64>,
    owner_id: i64,
    visibility: String,
}

#[derive(Clone, Copy)]
pub(crate) enum Namespace {
    Repo(i64),
    Fork(i64),
}

impl Namespace {
    pub(crate) fn for_repo(repo_id: i64, copy_id: Option<i64>) -> Self {
        copy_id.map_or(Self::Repo(repo_id), Self::Fork)
    }

    fn prefix(self) -> String {
        match self {
            Self::Repo(id) => format!("lfs/{id}/"),
            Self::Fork(id) => format!("lfs/fork-{id}/"),
        }
    }
}

struct Transfer<'a> {
    bucket: Bucket,
    repo_id: i64,
    key: String,
    oid: &'a str,
    size: u64,
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
    let upload = batch.operation.writes().then(|| limits::LfsLimits::read(&ctx.data.env)).transpose()?;
    let mode = upload.as_ref().map(|settings| settings.mode);
    let adapter = if mode == Some(limits::LfsMode::Multipart) { remote::MULTIPART_ADAPTER } else { "basic" };
    if !batch.transfers.as_ref().map_or(adapter == "basic", |names| names.iter().any(|name| name == adapter)) {
        let message = if adapter == "basic" { "The basic transfer adapter is required" } else {
            "Configure the shard-r2-multipart Git LFS upload adapter"
        };
        return failure((422, message));
    }
    if batch.objects.len() > MAX_OBJECTS {
        return failure((413, "LFS batches accept at most 100 objects"));
    }
    let is_remote = matches!(mode, Some(limits::LfsMode::Direct | limits::LfsMode::Multipart));
    let remote_upload = if is_remote {
        let Some(authorization) = util::header(&req, "authorization") else { return failure(CREDENTIALS) };
        match remote::S3::read(&ctx.data.env) {
            Ok(s3) => Some((s3, authorization)),
            Err(error) => return failure(error),
        }
    } else {
        None
    };

    let bucket = match ctx.data.env.bucket("LFS_OBJECTS") {
        Ok(bucket) => bucket,
        Err(_) => return failure(STORAGE),
    };
    let url = req.url()?;
    let Some(path) = url.path().strip_suffix("/batch") else {
        return failure(HIDDEN);
    };
    let prefix = format!("{}{path}/{}", url.origin().ascii_serialization(), repo.id);
    let object_prefix = Namespace::for_repo(repo.id, repo.lfs_copy_id).prefix();
    let mut objects = Vec::with_capacity(batch.objects.len());
    for object in batch.objects {
        let mut entry = json!(object);
        let actions = match batch_object(&bucket, &object_prefix, &prefix, &object, upload.as_ref()).await {
            Ok(Some((href, size))) => match (mode, &remote_upload) {
                (Some(limits::LfsMode::Direct), Some((s3, authorization))) => {
                    remote::direct_actions(s3, repo.id, &object.oid, size, &href, authorization).await.map(Some)
                }
                (Some(limits::LfsMode::Multipart), Some((_, authorization))) => Ok(Some(json!({
                    "upload": {"href": format!("{href}/multipart"), "header": {"Authorization": authorization}},
                }))),
                (Some(limits::LfsMode::Proxy), _) => Ok(Some(json!({
                    "upload": {"href": href, "header": {"Content-Type": BINARY}},
                    "verify": {"href": format!("{href}/verify")},
                }))),
                (None, _) => Ok(Some(json!({"download": {"href": href}}))),
                _ => Err(STORAGE),
            },
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        match actions {
            Ok(Some(actions)) => {
                entry["actions"] = actions;
                if mode == Some(limits::LfsMode::Direct) {
                    entry["authenticated"] = json!(true);
                }
            }
            Ok(None) => {}
            Err((code, message)) => entry["error"] = json!({"code": code, "message": message}),
        }
        objects.push(entry);
    }
    json_response(200, &json!({"transfer": adapter, "hash_algo": "sha256", "objects": objects}))
}

async fn batch_object(
    bucket: &Bucket,
    object_prefix: &str,
    prefix: &str,
    spec: &ObjectSpec,
    upload: Option<&limits::LfsLimits>,
) -> std::result::Result<Option<(String, u64)>, HttpError> {
    let size = u64::try_from(spec.size).map_err(|_| INVALID_OBJECT)?;
    validate_oid(&spec.oid)?;
    let exists = match bucket.head(format!("{object_prefix}{}", spec.oid)).await.map_err(|_| STORAGE)? {
        Some(object) if object.size() != size => return Err(WRONG_SIZE),
        Some(_) => true,
        None => false,
    };
    match (upload, exists) {
        (Some(_), true) => return Ok(None),
        (Some(settings), false) if size > settings.max_bytes => return Err(TOO_LARGE),
        (None, false) => return Err(MISSING),
        _ => {}
    }
    Ok(Some((format!("{prefix}/{}/{size}", spec.oid), size)))
}

pub async fn upload(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let Transfer { bucket, repo_id, key, oid, size } = match transfer(&req, &ctx, Operation::Upload).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    let settings = limits::LfsLimits::read(&ctx.data.env)?;
    if settings.mode != limits::LfsMode::Proxy {
        return failure((405, "Use the upload action returned by the LFS Batch API"));
    }
    if size > settings.max_bytes {
        return failure(TOO_LARGE);
    }
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
    if body.is_none() && size != 0 {
        return failure(WRONG_SIZE);
    }
    let database = ctx.data.db()?;
    let expires = util::now() + WRITE_LEASE;
    let Some(lease) = acquire_write(&database, repo_id, expires).await? else {
        return failure(HIDDEN);
    };
    let put = async {
        let data = match body {
            Some(body) => bounded_stream(body, size, expires)?,
            None => Data::Empty,
        };
        put_object(&bucket, &key, data, oid).await
    }
    .await;
    release_write(&database, lease).await?;
    match put {
        Ok(Some(_)) => empty(),
        Ok(None) => check_stored(&bucket, key, size).await,
        Err(error) => {
            let message = error.to_string();
            if message.ends_with("(10037)") {
                failure(BAD_DIGEST)
            } else if message.ends_with("(10013)") || message.ends_with("(10054)") {
                failure(WRONG_SIZE)
            } else {
                failure(STORAGE)
            }
        }
    }
}

async fn acquire_write(database: &d1::D1Database, repo_id: i64, expires: i64) -> Result<Option<i64>> {
    #[derive(Deserialize)]
    struct Lease {
        id: i64,
    }
    Ok(db::first::<Lease>(
        database,
        "INSERT INTO repo_lfs_uploads (repo_id, expires_at)
         SELECT id, ?2 FROM repos WHERE id = ?1 RETURNING id",
        &[util::js_int(repo_id), util::js_int(expires)],
    ).await?.map(|lease| lease.id))
}

async fn release_write(database: &d1::D1Database, id: i64) -> Result<()> {
    db::run(database, "DELETE FROM repo_lfs_uploads WHERE id = ?", &[util::js_int(id)]).await
}

pub async fn download(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let Transfer { bucket, key, size, .. } = match transfer(&req, &ctx, Operation::Download).await {
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
    let target = match transfer(&req, &ctx, Operation::Upload).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    let spec: ObjectSpec = match read_json(&req).await {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    if spec.oid != target.oid || u64::try_from(spec.size).ok() != Some(target.size) {
        return failure(INVALID_OBJECT);
    }
    if let Some(stage) = util::query_param(&req, "stage") {
        return remote::verify_direct(&ctx.data, &target, &stage).await;
    }
    check_stored(&target.bucket, target.key, target.size).await
}

pub async fn multipart(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let target = match transfer(&req, &ctx, Operation::Upload).await {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    let settings = limits::LfsLimits::read(&ctx.data.env)?;
    if settings.mode != limits::LfsMode::Multipart {
        return failure((405, "Multipart LFS uploads are disabled"));
    }
    if target.size > settings.max_bytes {
        return failure(TOO_LARGE);
    }
    remote::multipart(&req, &ctx, &target).await
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
) -> std::result::Result<Transfer<'a>, HttpError> {
    let repo = authorized(req, ctx, operation).await?;
    if ctx.param("repo_id").and_then(|id| id.parse().ok()) != Some(repo.id) {
        return Err(HIDDEN);
    }
    let oid = ctx.param("oid").ok_or(INVALID_OBJECT)?;
    let size = ctx.param("size").and_then(|size| decimal(size)).ok_or(INVALID_OBJECT)?;
    validate_oid(oid)?;
    let bucket = ctx.data.env.bucket("LFS_OBJECTS").map_err(|_| STORAGE)?;
    let key = format!("{}{oid}", Namespace::for_repo(repo.id, repo.lfs_copy_id).prefix());
    Ok(Transfer { bucket, repo_id: repo.id, key, oid, size })
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
        "SELECT r.id, r.owner_id, r.visibility, r.lfs_copy_id
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

fn validate_oid(oid: &str) -> std::result::Result<(), HttpError> {
    if oid.len() != 64 || !oid.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(INVALID_OBJECT);
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

fn bounded_stream(body: worker::web_sys::ReadableStream, size: u64, expires: i64) -> Result<Data> {
    let seconds = expires - WRITE_GRACE - util::now();
    if seconds <= 0 {
        return Err(Error::RustError("LFS write lease expired".into()));
    }
    Ok(Data::from(shard_lfs_stream(body, size as f64, seconds as u32 * 1000)?))
}

async fn put_object(bucket: &Bucket, key: &str, data: Data, oid: &str) -> Result<Option<Object>> {
    // R2 checks the hash and prevents concurrent writes from replacing bytes.
    bucket
        .put(key, data)
        .sha256(digest(oid).to_vec())
        .only_if(Conditional {
            etag_does_not_match: Some("*".into()),
            ..Default::default()
        })
        .execute()
        .await
}

pub(crate) async fn copy_page(
    env: &Env,
    source: Namespace,
    target: Namespace,
    cursor: Option<&str>,
    expires: i64,
    limit: u32,
) -> Result<Option<String>> {
    let bucket = env.bucket("LFS_OBJECTS")?;
    let source = source.prefix();
    let target = target.prefix();
    if source == target {
        return Err(Error::RustError("LFS fork source and destination overlap".into()));
    }
    let list = bucket.list().prefix(&source).limit(limit);
    let list = match cursor {
        Some(cursor) => list.cursor(cursor),
        None => list,
    }.execute().await?;
    for entry in list.objects() {
        let source_key = entry.key();
        let size = entry.size();
        let oid = source_key.strip_prefix(&source)
            .filter(|oid| validate_oid(oid).is_ok())
            .ok_or_else(|| Error::RustError("Invalid object in LFS source prefix".into()))?;
        let target_key = format!("{target}{oid}");
        if let Some(existing) = bucket.head(&target_key).await? {
            if existing.size() == size {
                continue;
            }
            return Err(Error::RustError("LFS fork object size mismatch".into()));
        }
        if size > limits::R2_SINGLE_PUT_BYTES {
            let s3 = remote::S3::read(env).map_err(|_| Error::RustError(STORAGE.1.into()))?;
            let timeout = remote::time_left(expires - WRITE_GRACE)?;
            if !s3.copy(&source_key, &target_key, size, &entry.http_etag(), timeout).await?
                && !bucket.head(&target_key).await?.is_some_and(|object| object.size() == size)
            {
                return Err(Error::RustError("LFS fork object was not stored".into()));
            }
            continue;
        }
        let object = bucket.get(&source_key).execute().await?
            .filter(|object| object.size() == size)
            .ok_or_else(|| Error::RustError("LFS source object changed during copy".into()))?;
        let Some(body) = object.body() else {
            return Err(Error::RustError("LFS source returned no body".into()));
        };
        let ResponseBody::Stream(body) = body.response_body()? else {
            return Err(Error::RustError("LFS source returned a non-stream body".into()));
        };
        let data = bounded_stream(body, size, expires)?;
        if put_object(&bucket, &target_key, data, oid).await?.is_none()
            && !bucket.head(&target_key).await?.is_some_and(|object| object.size() == size)
        {
            return Err(Error::RustError("LFS fork object was not stored".into()));
        }
    }
    if list.truncated() {
        return list.cursor().map(Some)
            .ok_or_else(|| Error::RustError("LFS listing returned no continuation cursor".into()));
    }
    Ok(None)
}

pub(crate) async fn delete_page(env: &Env, namespace: Namespace, limit: u32) -> Result<bool> {
    let bucket = env.bucket("LFS_OBJECTS")?;
    let prefix = namespace.prefix();
    let list = bucket.list().prefix(&prefix).limit(limit).execute().await?;
    let keys: Vec<_> = list.objects().into_iter().map(|object| object.key()).collect();
    if keys.iter().any(|key| !key.starts_with(&prefix)) {
        return Err(Error::RustError("LFS cleanup listing escaped its prefix".into()));
    }
    if !keys.is_empty() {
        bucket.delete_multiple(keys).await?;
    }
    Ok(!list.truncated())
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
