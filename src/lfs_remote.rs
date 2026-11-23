//! Private staging and publication for S3 uploads.
use super::{
    acquire_write, bounded_stream, empty, failure, json_response, put_object, read_json,
    release_write, validate_oid, Transfer, BAD_DIGEST, HIDDEN, INVALID_OBJECT, MISSING, STORAGE,
    TOO_LARGE, WRITE_GRACE, WRITE_LEASE, WRONG_SIZE,
};
use crate::ctx::AppState;
use crate::limits::{LfsLimits, LfsMode, R2_SINGLE_PUT_BYTES};
use crate::util::{js_int, js_text, now};
use crate::{crypto, db};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};
use worker::{
    d1::D1Database, Data, Env, Error, Request, Response, ResponseBody, Result, RouteContext,
    UploadedPart, Url,
};

pub(super) const MULTIPART_ADAPTER: &str = "shard-r2-multipart";
const SESSION_SECONDS: i64 = 6 * 24 * 60 * 60;
const MIB: u64 = 1024 * 1024;
const MAX_PARTS: u64 = 10_000;
const STALE: super::HttpError = (409, "Start the LFS multipart upload again");
const INCOMPLETE: super::HttpError = (409, "LFS multipart upload is missing part receipts");

#[wasm_bindgen(module = "/src/lfs_s3.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = lfsPresign)]
    async fn presign(
        endpoint: &str,
        bucket: &str,
        access: &str,
        secret: &str,
        key: &str,
        upload_id: &str,
        part: u16,
        length: f64,
        checksum: &str,
    ) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = lfsCopy)]
    async fn copy_parts(
        endpoint: &str,
        bucket: &str,
        access: &str,
        secret: &str,
        source: &str,
        target: &str,
        length: f64,
        etag: &str,
        timeout: u32,
    ) -> std::result::Result<bool, JsValue>;
    #[wasm_bindgen(catch, js_name = lfsListParts)]
    async fn list_parts_js(
        endpoint: &str,
        bucket: &str,
        access: &str,
        secret: &str,
        key: &str,
        upload_id: &str,
    ) -> std::result::Result<JsValue, JsValue>;
}

#[wasm_bindgen(inline_js = r#"
export async function shard_lfs_sha256(body, timeout) {
    const digest = new crypto.DigestStream("SHA-256");
    const [hash] = await Promise.all([
        digest.digest,
        body.pipeTo(digest, { signal: AbortSignal.timeout(timeout) }),
    ]);
    return Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, "0")).join("");
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_lfs_sha256(
        body: worker::web_sys::ReadableStream,
        timeout: u32,
    ) -> std::result::Result<String, JsValue>;
}

pub(super) struct S3 {
    endpoint: String,
    bucket: String,
    access: String,
    secret: String,
}

impl S3 {
    pub(super) fn read(env: &Env) -> std::result::Result<Self, super::HttpError> {
        const CONFIG: super::HttpError = (
            503,
            "Configure the LFS R2 endpoint, bucket and S3 credentials",
        );
        let endpoint = env
            .var("SHARD_LFS_R2_ENDPOINT")
            .map_err(|_| CONFIG)?
            .to_string();
        let endpoint = Url::parse(&endpoint).map_err(|_| CONFIG)?;
        let host = endpoint.host_str().ok_or(CONFIG)?;
        let account = host.split('.').next().unwrap_or_default();
        let s3 = endpoint.scheme() == "https"
            && endpoint.port().is_none()
            && endpoint.path() == "/"
            && account.len() == 32
            && account.bytes().all(|byte| byte.is_ascii_hexdigit())
            && host.ends_with(".r2.cloudflarestorage.com");
        let local = endpoint.scheme() == "http"
            && matches!(host, "127.0.0.1" | "localhost" | "[::1]")
            && endpoint.path().trim_end_matches('/') == "/cdn-cgi/local/r2/s3";
        if !(s3 || local)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(CONFIG);
        }
        let bucket = env
            .var("SHARD_LFS_R2_BUCKET")
            .map_err(|_| CONFIG)?
            .to_string();
        let access = env
            .secret("SHARD_LFS_R2_ACCESS_KEY_ID")
            .map_err(|_| CONFIG)?
            .to_string();
        let secret = env
            .secret("SHARD_LFS_R2_SECRET_ACCESS_KEY")
            .map_err(|_| CONFIG)?
            .to_string();
        if !(3..=63).contains(&bucket.len())
            || bucket.starts_with('-')
            || bucket.ends_with('-')
            || !bucket
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || access.is_empty()
            || secret.is_empty()
        {
            return Err(CONFIG);
        }
        Ok(Self {
            endpoint: endpoint.as_str().trim_end_matches('/').to_owned(),
            bucket,
            access,
            secret,
        })
    }

    async fn sign(
        &self,
        key: &str,
        upload_id: &str,
        part: u16,
        length: u64,
        checksum: &str,
    ) -> std::result::Result<Value, super::HttpError> {
        let value = presign(
            &self.endpoint,
            &self.bucket,
            &self.access,
            &self.secret,
            key,
            upload_id,
            part,
            length as f64,
            checksum,
        )
        .await
        .map_err(|_| STORAGE)?;
        let json = js_sys::JSON::stringify(&value)
            .map_err(|_| STORAGE)?
            .as_string()
            .ok_or(STORAGE)?;
        serde_json::from_str(&json).map_err(|_| STORAGE)
    }

    pub(super) async fn copy(
        &self,
        source: &str,
        target: &str,
        size: u64,
        etag: &str,
        timeout: u32,
    ) -> Result<bool> {
        copy_parts(
            &self.endpoint,
            &self.bucket,
            &self.access,
            &self.secret,
            source,
            target,
            size as f64,
            etag,
            timeout,
        )
        .await
        .map_err(|_| Error::RustError("R2 multipart copy failed".into()))
    }

    async fn list_parts(
        &self,
        key: &str,
        upload_id: &str,
    ) -> std::result::Result<Option<Vec<ListedPart>>, super::HttpError> {
        #[derive(Deserialize)]
        struct Listing {
            status: String,
            parts: Vec<ListedPart>,
        }
        let value = list_parts_js(
            &self.endpoint,
            &self.bucket,
            &self.access,
            &self.secret,
            key,
            upload_id,
        )
        .await
        .map_err(|_| STORAGE)?;
        let json = js_sys::JSON::stringify(&value)
            .map_err(|_| STORAGE)?
            .as_string()
            .ok_or(STORAGE)?;
        let listing: Listing = serde_json::from_str(&json).map_err(|_| STORAGE)?;
        if listing.status == "missing" {
            Ok(None)
        } else {
            Ok(Some(listing.parts))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedPart {
    part_number: u16,
    size: u64,
    etag: String,
}

fn stage_key(target: i64, oid: &str, attempt: &str) -> String {
    format!("lfs-incoming/{target}/{oid}/{attempt}")
}

pub(super) async fn direct_actions(
    s3: &S3,
    repo_id: i64,
    oid: &str,
    size: u64,
    href: &str,
    authorization: &str,
) -> std::result::Result<Value, super::HttpError> {
    let attempt = crypto::random_token().map_err(|_| STORAGE)?;
    let checksum = STANDARD.encode(super::digest(oid));
    let upload = s3
        .sign(&stage_key(repo_id, oid, &attempt), "", 0, size, &checksum)
        .await?;
    Ok(json!({
        "upload": upload,
        "verify": {"href": format!("{href}/verify?stage={attempt}"), "header": {"Authorization": authorization}},
    }))
}

pub(super) async fn verify_direct(
    state: &AppState,
    target: &Transfer<'_>,
    attempt: &str,
) -> Result<Response> {
    let settings = LfsLimits::read(&state.env)?;
    if settings.mode != LfsMode::Direct {
        return failure((405, "Direct LFS uploads are disabled"));
    }
    if target.size > settings.max_bytes {
        return failure(TOO_LARGE);
    }
    if validate_oid(attempt).is_err() || target.size > R2_SINGLE_PUT_BYTES {
        return failure(INVALID_OBJECT);
    }
    let stage = stage_key(target.repo_id, target.oid, attempt);
    if let Some(response) = already_stored(target).await? {
        let _ = target.bucket.delete(&stage).await;
        return Ok(response);
    }
    let response = publish(state, target, &stage).await?;
    if matches!(response.status_code(), 200 | 422) {
        let _ = target.bucket.delete(&stage).await;
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Command {
    Start,
    Part {
        upload_id: String,
        part_number: u16,
    },
    Record {
        upload_id: String,
        part_number: u16,
        etag: String,
    },
    Finish {
        upload_id: String,
    },
    Abort {
        upload_id: String,
    },
}

#[derive(Deserialize)]
struct Session {
    size: u64,
    attempt: String,
    upload_id: String,
}

#[derive(Deserialize)]
struct Part {
    part_number: u16,
    etag: String,
}

pub(super) async fn multipart(
    req: &Request,
    ctx: &RouteContext<AppState>,
    target: &Transfer<'_>,
) -> Result<Response> {
    let command: Command = match read_json(req).await {
        Ok(command) => command,
        Err(error) => return failure(error),
    };
    let database = ctx.data.db()?;
    match command {
        Command::Start => start(&ctx.data, &database, target).await,
        Command::Part {
            upload_id,
            part_number,
        } => {
            let Some(session) = session(&database, target, &upload_id).await? else {
                return failure(STALE);
            };
            let Some(length) = part_length(target.size, part_number) else {
                return failure(INVALID_OBJECT);
            };
            let s3 = match S3::read(&ctx.data.env) {
                Ok(s3) => s3,
                Err(error) => return failure(error),
            };
            let stage = stage_key(target.repo_id, target.oid, &session.attempt);
            match s3.sign(&stage, &upload_id, part_number, length, "").await {
                Ok(action) => json_response(200, &action),
                Err(error) => failure(error),
            }
        }
        Command::Record {
            upload_id,
            part_number,
            etag,
        } => {
            let Some(session) = session(&database, target, &upload_id).await? else {
                return failure(STALE);
            };
            let etag = etag
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(&etag);
            if part_length(target.size, part_number).is_none()
                || etag.is_empty()
                || etag.len() > 256
                || !etag
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'"')
            {
                return failure((422, "Invalid LFS multipart receipt"));
            }
            db::run(
                &database,
                "INSERT INTO repo_lfs_parts (attempt, part_number, etag)
                 SELECT attempt, ?2, ?3 FROM repo_lfs_multipart WHERE attempt = ?1
                 ON CONFLICT (attempt, part_number) DO UPDATE SET etag = excluded.etag
                 WHERE etag <> excluded.etag",
                &[
                    js_text(&session.attempt),
                    js_int(i64::from(part_number)),
                    js_text(etag),
                ],
            )
            .await?;
            empty()
        }
        Command::Finish { upload_id } => {
            if let Some(response) = already_stored(target).await? {
                if let Some(session) = session(&database, target, &upload_id).await? {
                    let stage = stage_key(target.repo_id, target.oid, &session.attempt);
                    if let Ok(upload) = target
                        .bucket
                        .resume_multipart_upload(&stage, &session.upload_id)
                    {
                        let _ = upload.abort().await;
                    }
                    let _ = target.bucket.delete(&stage).await;
                    let _ = drop_session(&database, &session.attempt).await;
                }
                return Ok(response);
            }
            let Some(session) = session(&database, target, &upload_id).await? else {
                return failure(STALE);
            };
            finish(&ctx.data, &database, target, session).await
        }
        Command::Abort { upload_id } => {
            let Some(session) = session(&database, target, &upload_id).await? else {
                return failure(STALE);
            };
            let stage = stage_key(target.repo_id, target.oid, &session.attempt);
            if let Ok(upload) = target
                .bucket
                .resume_multipart_upload(&stage, &session.upload_id)
            {
                let _ = upload.abort().await;
            }
            let _ = target.bucket.delete(&stage).await;
            drop_session(&database, &session.attempt).await?;
            empty()
        }
    }
}

async fn active(database: &D1Database, target: &Transfer<'_>) -> Result<Option<Session>> {
    db::first(database,
        "SELECT size, attempt, upload_id FROM repo_lfs_multipart WHERE repo_id = ? AND oid = ? AND expires_at > ?",
        &[js_int(target.repo_id), js_text(target.oid), js_int(now())],
    ).await
}

async fn session(
    database: &D1Database,
    target: &Transfer<'_>,
    upload_id: &str,
) -> Result<Option<Session>> {
    Ok(active(database, target)
        .await?
        .filter(|s| s.size == target.size && s.upload_id == upload_id))
}

async fn start(state: &AppState, database: &D1Database, target: &Transfer<'_>) -> Result<Response> {
    if let Some(response) = already_stored(target).await? {
        return if response.status_code() == 200 {
            json_response(200, &json!({"complete": true}))
        } else {
            Ok(response)
        };
    }
    if let Err(error) = S3::read(&state.env) {
        return failure(error);
    }
    if target.size == 0 {
        let expires = now() + WRITE_LEASE;
        let Some(lease) = acquire_write(database, target.repo_id, expires).await? else {
            return failure(HIDDEN);
        };
        let put = put_object(&target.bucket, &target.key, Data::Empty, target.oid).await;
        release_write(database, lease).await?;
        return match put {
            Ok(_) => json_response(200, &json!({"complete": true})),
            Err(error) if bad_digest(&error) => failure(BAD_DIGEST),
            Err(_) => failure(STORAGE),
        };
    }
    let existing = active(database, target).await?;
    if let Some(mismatched) = existing.as_ref().filter(|s| s.size != target.size) {
        let old_stage = stage_key(target.repo_id, target.oid, &mismatched.attempt);
        let has_staged = match target.bucket.head(&old_stage).await {
            Ok(object) => object.is_some(),
            Err(_) => return failure(STORAGE),
        };
        let has_parts = db::first::<serde::de::IgnoredAny>(
            database,
            "SELECT 1 FROM repo_lfs_parts WHERE attempt = ? LIMIT 1",
            &[js_text(&mismatched.attempt)],
        )
        .await?
        .is_some();
        if has_staged || has_parts {
            return failure(WRONG_SIZE);
        }
        // Replace an unused wrong-size session so a mistyped start does not lock out the OID.
        if let Ok(upload) = target
            .bucket
            .resume_multipart_upload(&old_stage, &mismatched.upload_id)
        {
            let _ = upload.abort().await;
        }
        let _ = target.bucket.delete(&old_stage).await;
        drop_session(database, &mismatched.attempt).await?;
    }
    let existing = existing.filter(|s| s.size == target.size);
    let session = if let Some(session) = existing {
        session
    } else {
        let time = now();
        #[derive(Deserialize)]
        struct ExpiredSession {
            attempt: String,
            upload_id: String,
        }
        let expired: Vec<ExpiredSession> = db::all(
            database,
            "DELETE FROM repo_lfs_multipart WHERE repo_id = ? AND oid = ? AND expires_at <= ?
             RETURNING attempt, upload_id",
            &[js_int(target.repo_id), js_text(target.oid), js_int(time)],
        )
        .await?;
        for old in expired {
            let old_stage = stage_key(target.repo_id, target.oid, &old.attempt);
            if let Ok(upload) = target
                .bucket
                .resume_multipart_upload(&old_stage, &old.upload_id)
            {
                let _ = upload.abort().await;
            }
            let _ = target.bucket.delete(&old_stage).await;
        }
        let attempt = crypto::random_token()?;
        let stage = stage_key(target.repo_id, target.oid, &attempt);
        let upload = match target.bucket.create_multipart_upload(stage).execute().await {
            Ok(upload) => upload,
            Err(_) => return failure(STORAGE),
        };
        let upload_id = upload.upload_id().await;
        let inserted = db::first::<Session>(
            database,
            "INSERT INTO repo_lfs_multipart (repo_id, oid, size, attempt, upload_id, expires_at)
             SELECT id, ?2, ?3, ?4, ?5, ?6 FROM repos WHERE id = ?1
             ON CONFLICT (repo_id, oid) DO NOTHING RETURNING size, attempt, upload_id",
            &[
                js_int(target.repo_id),
                js_text(target.oid),
                js_int(target.size as i64),
                js_text(&attempt),
                js_text(&upload_id),
                js_int(time + SESSION_SECONDS),
            ],
        )
        .await;
        match inserted {
            Ok(Some(created)) => created,
            Ok(None) => {
                let _ = upload.abort().await;
                let Some(concurrent) = active(database, target).await? else {
                    return failure(HIDDEN);
                };
                concurrent
            }
            Err(error) => {
                let _ = upload.abort().await;
                return Err(error);
            }
        }
    };
    if session.size != target.size {
        return failure(WRONG_SIZE);
    }
    let stage = stage_key(target.repo_id, target.oid, &session.attempt);
    let staged = match target.bucket.head(stage).await {
        Ok(object) => object.is_some(),
        Err(_) => return failure(STORAGE),
    };
    #[derive(Deserialize)]
    struct Receipt {
        part_number: u16,
    }
    let uploaded: Vec<_> = if staged {
        Vec::new()
    } else {
        db::all::<Receipt>(
            database,
            "SELECT part_number FROM repo_lfs_parts WHERE attempt = ? ORDER BY part_number",
            &[js_text(&session.attempt)],
        )
        .await?
        .into_iter()
        .map(|part| part.part_number)
        .collect()
    };
    json_response(
        200,
        &json!({
            "uploadId": session.upload_id, "partBytes": part_bytes(target.size), "uploaded": uploaded, "staged": staged,
        }),
    )
}

async fn finish(
    state: &AppState,
    database: &D1Database,
    target: &Transfer<'_>,
    session: Session,
) -> Result<Response> {
    let stage = stage_key(target.repo_id, target.oid, &session.attempt);
    let staged = match target.bucket.head(&stage).await {
        Ok(object) => object.is_some(),
        Err(_) => return failure(STORAGE),
    };
    if !staged {
        let expected_parts = target.size.div_ceil(part_bytes(target.size));
        let parts = db::all::<Part>(
            database,
            "SELECT part_number, etag FROM repo_lfs_parts WHERE attempt = ? ORDER BY part_number",
            &[js_text(&session.attempt)],
        )
        .await?;
        if parts.len() as u64 != expected_parts {
            return failure(INCOMPLETE);
        }
        let upload = target
            .bucket
            .resume_multipart_upload(&stage, &session.upload_id)?;
        if let Err(error) = upload
            .complete(
                parts
                    .into_iter()
                    .map(|p| UploadedPart::new(p.part_number, p.etag)),
            )
            .await
        {
            // One completion can succeed while another request gets NoSuchUpload.
            if target.bucket.head(&stage).await?.is_none() {
                if let Some(response) = already_stored(target).await? {
                    return Ok(response);
                }
                let message = error.to_string();
                if message.ends_with("(10024)") || message.ends_with("(10025)") {
                    drop_session(database, &session.attempt).await?;
                    return failure(STALE);
                }
                // Reconcile against R2's authoritative uploaded parts to recover from stale/poisoned receipts.
                if let Ok(s3) = S3::read(&state.env) {
                    if let Ok(listing) = s3.list_parts(&stage, &session.upload_id).await {
                        let Some(actual) = listing else {
                            drop_session(database, &session.attempt).await?;
                            return failure(STALE);
                        };
                        let valid: Vec<_> = actual
                            .into_iter()
                            .filter(|part| {
                                part_length(target.size, part.part_number) == Some(part.size)
                            })
                            .collect();
                        if valid.len() as u64 == expected_parts {
                            let retry = target
                                .bucket
                                .resume_multipart_upload(&stage, &session.upload_id)?;
                            if retry
                                .complete(
                                    valid
                                        .into_iter()
                                        .map(|p| UploadedPart::new(p.part_number, p.etag)),
                                )
                                .await
                                .is_ok()
                                || target.bucket.head(&stage).await?.is_some()
                            {
                                // Reconciled and assembled successfully; continue to publish.
                            } else {
                                if let Ok(aborted) = target
                                    .bucket
                                    .resume_multipart_upload(&stage, &session.upload_id)
                                {
                                    let _ = aborted.abort().await;
                                }
                                drop_session(database, &session.attempt).await?;
                                return failure(STALE);
                            }
                        } else {
                            let parts_json = serde_json::to_string(
                                &valid
                                    .iter()
                                    .map(|part| {
                                        json!({
                                            "part_number": part.part_number,
                                            "etag": part.etag,
                                        })
                                    })
                                    .collect::<Vec<_>>(),
                            )
                            .map_err(|_| {
                                Error::RustError("LFS part serialization failed".into())
                            })?;
                            let clear = database
                                .prepare("DELETE FROM repo_lfs_parts WHERE attempt = ?")
                                .bind(&[js_text(&session.attempt)])?;
                            let restore = database
                                .prepare(
                                    "INSERT INTO repo_lfs_parts (attempt, part_number, etag)
                                     SELECT m.attempt, json_extract(p.value, '$.part_number'), json_extract(p.value, '$.etag')
                                       FROM repo_lfs_multipart m, json_each(?2) p
                                      WHERE m.attempt = ?1
                                     ON CONFLICT (attempt, part_number) DO UPDATE SET etag = excluded.etag",
                                )
                                .bind(&[js_text(&session.attempt), js_text(&parts_json)])?;
                            database.batch(vec![clear, restore]).await?;
                            return failure(INCOMPLETE);
                        }
                    } else {
                        return failure(STORAGE);
                    }
                } else {
                    return failure(STORAGE);
                }
            }
        }
    }
    let response = publish(state, target, &stage).await?;
    if matches!(response.status_code(), 200 | 422) {
        drop_session(database, &session.attempt).await?;
        let _ = target.bucket.delete(&stage).await;
    }
    Ok(response)
}

async fn drop_session(database: &D1Database, attempt: &str) -> Result<()> {
    db::run(
        database,
        "DELETE FROM repo_lfs_multipart WHERE attempt = ?",
        &[js_text(attempt)],
    )
    .await
}

fn part_bytes(size: u64) -> u64 {
    (size.div_ceil(MAX_PARTS * MIB)).max(32) * MIB
}

fn part_length(size: u64, part: u16) -> Option<u64> {
    let bytes = part_bytes(size);
    let offset = u64::from(part.checked_sub(1)?) * bytes;
    (offset < size && u64::from(part) <= MAX_PARTS).then(|| (size - offset).min(bytes))
}

async fn already_stored(target: &Transfer<'_>) -> Result<Option<Response>> {
    match target.bucket.head(&target.key).await {
        Ok(Some(object)) if object.size() == target.size => empty().map(Some),
        Ok(Some(_)) => failure(WRONG_SIZE).map(Some),
        Ok(None) => Ok(None),
        Err(_) => failure(STORAGE).map(Some),
    }
}

pub(super) fn time_left(deadline: i64) -> Result<u32> {
    let seconds = deadline - now();
    if seconds <= 0 {
        return Err(Error::RustError("LFS verification timed out".into()));
    }
    Ok(seconds as u32 * 1000)
}

fn bad_digest(error: &Error) -> bool {
    error.to_string().ends_with("(10037)")
}

async fn publish(state: &AppState, target: &Transfer<'_>, stage: &str) -> Result<Response> {
    let object = match target.bucket.get(stage).execute().await {
        Ok(Some(object)) => object,
        Ok(None) => {
            return already_stored(target)
                .await?
                .map_or_else(|| failure(MISSING), Ok)
        }
        Err(_) => return failure(STORAGE),
    };
    let Some(body) = object.body() else {
        return failure(STORAGE);
    };
    let ResponseBody::Stream(body) = body.response_body()? else {
        return failure(STORAGE);
    };
    if object.size() != target.size {
        let _ = body.cancel();
        return failure(WRONG_SIZE);
    }
    let database = state.db()?;
    let data = if target.size <= R2_SINGLE_PUT_BYTES {
        // The final conditional PUT verifies SHA-256 as R2 reads every byte from staging.
        Some(body)
    } else {
        // A multipart checksum is composite. Hash the actual assembled bytes once.
        let timeout = time_left(now() + WRITE_LEASE - WRITE_GRACE)?;
        let sha = shard_lfs_sha256(body, timeout).await.map_err(Error::from)?;
        if sha != target.oid {
            return failure(BAD_DIGEST);
        }
        None
    };
    // Start the write lease after verification so hashing does not consume the copy's time.
    let expires = now() + WRITE_LEASE;
    let Some(lease) = acquire_write(&database, target.repo_id, expires).await? else {
        if let Some(body) = data {
            let _ = body.cancel();
        }
        return failure(HIDDEN);
    };
    // Release the lease on every outcome: an R2 or S3 failure must not delay
    // the repository's storage cleanup until the lease expires.
    let result: Result<std::result::Result<bool, Response>> = async {
        if let Some(body) = data {
            let data = bounded_stream(body, target.size, expires)?;
            return Ok(
                match put_object(&target.bucket, &target.key, data, target.oid).await {
                    Ok(object) => Ok(object.is_some()),
                    Err(error) if bad_digest(&error) => Err(failure(BAD_DIGEST)?),
                    Err(_) => Err(failure(STORAGE)?),
                },
            );
        }
        let s3 = match S3::read(&state.env) {
            Ok(s3) => s3,
            Err(error) => return Ok(Err(failure(error)?)),
        };
        let timeout = time_left(expires - WRITE_GRACE)?;
        Ok(
            match s3
                .copy(
                    stage,
                    &target.key,
                    target.size,
                    &object.http_etag(),
                    timeout,
                )
                .await
            {
                Ok(copied) => Ok(copied),
                Err(_) => Err(failure(STORAGE)?),
            },
        )
    }
    .await;
    release_write(&database, lease).await?;
    match result? {
        Ok(true) => empty(),
        Ok(false) => already_stored(target)
            .await?
            .map_or_else(|| failure(STORAGE), Ok),
        Err(response) => Ok(response),
    }
}
