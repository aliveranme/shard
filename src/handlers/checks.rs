use super::common::{RepoRole, load_repo, repo_role};
use crate::artifacts::{Artifacts, is_object_id};
use crate::ctx::AppState;
use crate::i18n::{CheckText, Locale, Message, RepoText};
use crate::{auth, db, util};
use js_sys::{Reflect, Uint8Array};
use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::ReadableStreamDefaultReader;
use worker::*;

const DISPLAY_LIMIT: usize = 32;
const BODY_LIMIT: usize = 8 * 1024;
const CONTEXT_LIMIT: usize = 80;
const SUMMARY_LIMIT: usize = 1024;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CheckState {
    Pending,
    Success,
    Failure,
    Error,
}

impl CheckState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Error => "error",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitCheck {
    pub context: String,
    state: CheckState,
    pub summary: Option<String>,
}

impl CommitCheck {
    pub fn label(&self, locale: &Locale) -> &'static str {
        locale.t(match self.state {
            CheckState::Pending => CheckText::Pending,
            CheckState::Success => CheckText::Success,
            CheckState::Failure => CheckText::Failure,
            CheckState::Error => CheckText::Error,
        })
    }

    fn valid(&self) -> bool {
        !self.context.is_empty()
            && self.context.len() <= CONTEXT_LIMIT
            && self.context.trim() == self.context
            && !self.context.chars().any(char::is_control)
            && self.summary.as_ref().is_none_or(|summary| {
                summary.len() <= SUMMARY_LIMIT
                    && !summary.chars().any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
            })
    }
}

pub struct CommitChecks {
    pub rows: Vec<CommitCheck>,
    pub has_more: bool,
}

/// Call only after resolving a visible repository and its exact commit.
pub async fn load(state: &AppState, repo_id: i64, hash: &str) -> Result<CommitChecks> {
    let mut rows = db::all(
        &state.db()?,
        "SELECT context, state, summary FROM commit_checks
         WHERE repo_id = ?1 AND commit_hash = ?2 ORDER BY context LIMIT ?3",
        &[
            util::js_int(repo_id),
            util::js_text(hash),
            util::js_int((DISPLAY_LIMIT + 1) as i64),
        ],
    )
    .await?;
    let has_more = rows.len() > DISPLAY_LIMIT;
    rows.truncate(DISPLAY_LIMIT);
    Ok(CommitChecks { rows, has_more })
}

pub async fn record(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    // The Git identity helper supports Basic too; this endpoint only accepts Bearer.
    let bearer = util::header(&req, "authorization").is_some_and(|header| {
        header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .is_some_and(|token| !token.trim().is_empty())
    });
    let identity = if bearer { auth::git_identity(&state.env, &req).await? } else { None };
    let Some(identity) = identity else {
        let mut response = error(state, 401, CheckText::CredentialsRequired)?;
        response.headers_mut().set("WWW-Authenticate", "Bearer realm=\"shard\"")?;
        return Ok(response);
    };

    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let Some(repo) = load_repo(state, owner, name).await? else {
        return empty(404);
    };
    let database = state.db()?;
    let role = repo_role(&database, repo.id, repo.owner_id, identity.user_id, identity.is_admin).await?;
    if repo.visibility != "public" && role.is_none() {
        return empty(404);
    }
    if identity.scope != "write" || role != Some(RepoRole::Write) {
        return error(state, 403, CheckText::WriteRequired);
    }

    let hash = ctx.param("hash").map_or("", String::as_str);
    if !is_object_id(hash) {
        return error(state, 404, RepoText::CommitNotFound);
    }
    let json = util::header(&req, "content-type").is_some_and(|header| {
        header.split(';').next().is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
    });
    if !json {
        return error(state, 415, CheckText::JsonRequired);
    }
    let mut body = [0; BODY_LIMIT];
    let len = match read_body(&req, &mut body).await {
        Ok(len) => len,
        Err(PayloadError::Invalid) => return error(state, 400, CheckText::InvalidPayload),
        Err(PayloadError::TooLarge) => return error(state, 413, CheckText::PayloadTooLarge),
    };
    let check: CommitCheck = match serde_json::from_slice::<CommitCheck>(&body[..len]) {
        Ok(check) if check.valid() => check,
        _ => return error(state, 400, CheckText::InvalidPayload),
    };

    let commit = async {
        let handle = Artifacts::from_env(&state.env)?.get(&repo.artifacts_repo).await?;
        handle.read_commit(hash).await
    }
    .await;
    match commit {
        Ok(Some(_)) => (),
        Ok(None) => return error(state, 404, RepoText::CommitNotFound),
        Err(_) => return error(state, 502, RepoText::StorageUnavailable),
    }

    // Recheck the writer in the mutation because Artifacts calls can race a role change.
    let recorded = database
        .prepare(
            "INSERT INTO commit_checks (repo_id, commit_hash, context, state, summary)
             SELECT r.id, ?2, ?3, ?4, ?5 FROM repos r JOIN users u ON u.id = ?6
             WHERE r.id = ?1 AND (r.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
                 SELECT 1 FROM repo_collaborators c
                 WHERE c.repo_id = r.id AND c.user_id = u.id AND c.role = 'write'
             ))
             ON CONFLICT (repo_id, commit_hash, context) DO UPDATE SET
                 state = excluded.state, summary = excluded.summary
             RETURNING repo_id",
        )
        .bind(&[
            util::js_int(repo.id),
            util::js_text(hash),
            util::js_text(&check.context),
            util::js_text(check.state.as_str()),
            check.summary.as_deref().map_or_else(util::js_null, util::js_text),
            util::js_int(identity.user_id),
        ])?
        .first::<i64>(Some("repo_id"))
        .await?;
    empty(if recorded.is_some() { 204 } else { 404 })
}

enum PayloadError {
    Invalid,
    TooLarge,
}

async fn read_body(req: &Request, body: &mut [u8]) -> std::result::Result<usize, PayloadError> {
    let stream = req.inner().body().ok_or(PayloadError::Invalid)?;
    // Inspect chunk size before copying bytes from the Workers stream into Wasm.
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let done = JsValue::from_str("done");
    let value = JsValue::from_str("value");
    let mut len = 0;
    loop {
        let result = JsFuture::from(reader.read()).await.map_err(|_| PayloadError::Invalid)?;
        if Reflect::get(&result, &done).ok().and_then(|value| value.as_bool()) == Some(true) {
            reader.release_lock();
            return Ok(len);
        }
        let chunk = Reflect::get(&result, &value)
            .ok()
            .and_then(|value| value.dyn_into::<Uint8Array>().ok())
            .ok_or(PayloadError::Invalid)?;
        let chunk_len = chunk.length() as usize;
        if chunk_len > body.len() - len {
            let _ = JsFuture::from(reader.cancel()).await;
            return Err(PayloadError::TooLarge);
        }
        let end = len + chunk_len;
        chunk.copy_to(&mut body[len..end]);
        len = end;
    }
}

fn empty(status: u16) -> Result<Response> {
    Ok(Response::builder().with_status(status).with_header("Cache-Control", "no-store")?.empty())
}

fn error(state: &AppState, status: u16, message: impl Message) -> Result<Response> {
    let mut response = Response::error(state.locale.t(message), status)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    response.headers_mut().set("Content-Type", "text/plain; charset=utf-8")?;
    Ok(response)
}
