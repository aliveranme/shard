use crate::ctx::AppState;
use crate::git_storage::RepoHandle;
use crate::handlers::common::page_title;
use crate::i18n::{AccountText, GpgText, Locale};
use crate::templates::{render, Chrome};
use crate::{auth, db, util};
use askama::Template;
use js_sys::Array;
use serde::Deserialize;
use std::ops::Range;
use wasm_bindgen::prelude::*;
use worker::{Request, Response, Result, RouteContext};

const KEY_BYTES: usize = 32 * 1024;
const SIGNATURE_BYTES: usize = 16 * 1024;
const MAX_KEYS: i64 = 8;
const SETTINGS: &str = "/user/settings/gpg";
const LOGIN: &str = "/user/login?next=/user/settings/gpg";

#[wasm_bindgen(module = "/src/gpg_verify.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_gpg_key(armor: &str, email: &str) -> std::result::Result<String, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_gpg_signature(armor: &str) -> std::result::Result<Array, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_git_commit_oid(bytes: &[u8]) -> std::result::Result<String, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_gpg_verify(
        binary: &[u8],
        signature: &JsValue,
        armor: &str,
        email: &str,
    ) -> std::result::Result<bool, JsValue>;
}

#[derive(Template)]
#[template(path = "gpg_keys.html")]
struct KeysPage {
    chrome: Chrome,
    title: String,
    keys: Vec<KeyRow>,
    name: String,
    success: Option<&'static str>,
    error: Option<&'static str>,
}

#[derive(Deserialize)]
struct KeyRow {
    fingerprint: String,
    name: String,
    email: String,
}

#[derive(Deserialize)]
struct InspectedKey {
    fingerprint: String,
    public_key: String,
    key_ids: Vec<String>,
}

fn valid_key_hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

async fn settings_page(
    state: &AppState,
    user_id: i64,
    name: String,
    success: Option<GpgText>,
    error: Option<&'static str>,
) -> Result<Response> {
    let keys: Vec<KeyRow> = db::all(
        &state.db()?,
        "SELECT fingerprint, name, email FROM gpg_keys WHERE user_id = ?
         ORDER BY created_at DESC, fingerprint",
        &[util::js_int(user_id)],
    )
    .await?;
    let mut response = render(&KeysPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(GpgText::Keys)]),
        keys,
        name,
        success: success.map(|message| state.locale.t(message)),
        error,
    })?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

pub(crate) async fn settings(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user_id) = state.user_id() else {
        return util::redirect(LOGIN);
    };
    let success = match util::query_param(&req, "updated").as_deref() {
        Some("saved") => Some(GpgText::Saved),
        Some("removed") => Some(GpgText::Removed),
        _ => None,
    };
    settings_page(state, user_id, String::new(), success, None).await
}

pub(crate) async fn update(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = &state.user else {
        return util::redirect(LOGIN);
    };
    let form = state.form()?;
    let action = util::form_value(form, "action").unwrap_or_default();
    let name = util::form_value(form, "name").unwrap_or_default();
    let reject = |status, message| {
        let mut echoed_name = name.trim().to_owned();
        if echoed_name.chars().count() > 100 {
            echoed_name.clear();
        }
        async move {
            Ok(
                settings_page(state, user.id, echoed_name, None, Some(message))
                    .await?
                    .with_status(status),
            )
        }
    };
    if !matches!(action.as_str(), "save" | "remove") {
        return reject(400, state.locale.t(GpgText::InvalidForm)).await;
    }
    let password = util::form_value(form, "current_password").unwrap_or_default();
    let Some(verified_hash) = auth::verified_password_hash(&state.env, user.id, &password).await?
    else {
        return reject(403, state.locale.t(AccountText::IncorrectCurrentPassword)).await;
    };
    let database = state.db()?;
    if action == "remove" {
        let fingerprint = util::form_value(form, "fingerprint").unwrap_or_default();
        if !valid_key_hex(&fingerprint, &[40, 64]) {
            return reject(400, state.locale.t(GpgText::InvalidForm)).await;
        }
        let removed: Option<KeyRow> = db::first(
            &database,
            "DELETE FROM gpg_keys WHERE user_id = ?1 AND fingerprint = ?2
               AND EXISTS (SELECT 1 FROM users WHERE id = ?1 AND password_hash = ?3)
             RETURNING fingerprint, name, email",
            &[
                util::js_int(user.id),
                util::js_text(&fingerprint),
                util::js_text(&verified_hash),
            ],
        )
        .await?;
        if removed.is_none() {
            return util::redirect(SETTINGS);
        }
        return util::redirect("/user/settings/gpg?updated=removed");
    }

    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 || trimmed.chars().any(char::is_control)
    {
        return reject(400, state.locale.t(GpgText::InvalidName)).await;
    }
    let armor = util::form_value(form, "public_key").unwrap_or_default();
    if armor.len() > KEY_BYTES {
        return reject(413, state.locale.t(GpgText::KeyTooLarge)).await;
    }
    let inspected =
        match shard_gpg_key(armor.trim().replace("\r\n", "\n").as_str(), &user.email).await {
            Ok(value) if value.is_empty() => {
                return reject(422, state.locale.t(GpgText::EmailMismatch)).await
            }
            Ok(value) => serde_json::from_str::<InspectedKey>(&value)?,
            Err(_) => return reject(422, state.locale.t(GpgText::InvalidKey)).await,
        };
    if inspected.public_key.len() > KEY_BYTES {
        return reject(413, state.locale.t(GpgText::KeyTooLarge)).await;
    }
    let key_ids = serde_json::to_string(&inspected.key_ids)?;
    let saved: Option<KeyRow> = db::first(
        &database,
        "INSERT INTO gpg_keys (user_id, fingerprint, email, name, public_key, key_ids, created_at)
         SELECT id, ?2, email, ?4, ?5, ?6, ?7 FROM users
         WHERE id = ?1 AND email = ?3 COLLATE NOCASE AND password_hash = ?9 AND (
           EXISTS (SELECT 1 FROM gpg_keys WHERE user_id = ?1 AND fingerprint = ?2) OR
           (SELECT COUNT(*) FROM gpg_keys WHERE user_id = ?1) < ?8
         ) ON CONFLICT (user_id, fingerprint) DO UPDATE SET email = excluded.email,
              name = excluded.name, public_key = excluded.public_key, key_ids = excluded.key_ids
         RETURNING fingerprint, name, email",
        &[
            util::js_int(user.id),
            util::js_text(&inspected.fingerprint),
            util::js_text(&user.email),
            util::js_text(trimmed),
            util::js_text(&inspected.public_key),
            util::js_text(&key_ids),
            util::js_int(util::now()),
            util::js_int(MAX_KEYS),
            util::js_text(&verified_hash),
        ],
    )
    .await?;
    if saved.is_none() {
        return reject(409, state.locale.t(GpgText::KeyLimit)).await;
    }
    util::redirect("/user/settings/gpg?updated=saved")
}

pub(crate) enum CommitSignature {
    Unsigned,
    Unsupported,
    MissingKey,
    Invalid,
    Unavailable,
    Valid {
        fingerprint: String,
        signer: String,
        email_verified: bool,
    },
}

impl CommitSignature {
    pub(crate) fn label(&self, locale: &Locale) -> &'static str {
        locale.t(match self {
            Self::Unsigned => GpgText::Unsigned,
            Self::Unsupported => GpgText::Unsupported,
            Self::MissingKey => GpgText::MissingKey,
            Self::Invalid => GpgText::InvalidSignature,
            Self::Unavailable => GpgText::Unavailable,
            Self::Valid { .. } => GpgText::ValidSignature,
        })
    }

    pub(crate) fn class(&self) -> &'static str {
        match self {
            Self::Valid { .. } => "badge badge--verified",
            Self::Invalid => "badge badge--deleted",
            _ => "badge",
        }
    }

    pub(crate) fn fingerprint(&self) -> Option<&str> {
        match self {
            Self::Valid { fingerprint, .. } => Some(fingerprint),
            _ => None,
        }
    }

    pub(crate) fn signer(&self) -> Option<&str> {
        match self {
            Self::Valid { signer, .. } => Some(signer),
            _ => None,
        }
    }

    pub(crate) fn email_verified(&self) -> bool {
        matches!(
            self,
            Self::Valid {
                email_verified: true,
                ..
            }
        )
    }
}

struct SignedCommit<'a> {
    armor: String,
    email: &'a str,
    signature: Range<usize>,
}

fn committer_email(line: &[u8]) -> Option<&str> {
    let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    let tz_split = trimmed.iter().rposition(|&b| b == b' ')?;
    let tz = &trimmed[tz_split + 1..];
    if tz.len() != 5 || !matches!(tz[0], b'+' | b'-') || !tz[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let before_tz = &trimmed[..tz_split];
    let ts_split = before_tz.iter().rposition(|&b| b == b' ')?;
    let ts = &before_tz[ts_split + 1..];
    let ts_digits = ts.strip_prefix(b"-").unwrap_or(ts);
    if ts_digits.is_empty() || !ts_digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let identity = &before_tz[..ts_split];
    if !identity.ends_with(b">") {
        return None;
    }
    let end = identity.len() - 1;
    let start = identity[..end].iter().rposition(|&byte| byte == b'<')?;
    let email = std::str::from_utf8(&identity[start + 1..end]).ok()?;
    util::valid_email(email).then_some(email)
}

fn signed_commit(bytes: &[u8]) -> std::result::Result<SignedCommit<'_>, CommitSignature> {
    let mut signature: Option<Range<usize>> = None;
    let mut other_signature = false;
    let mut committer = None;
    let mut has_tree = false;
    let mut has_author = false;
    let mut offset = 0;
    for (idx, line) in bytes.split_inclusive(|&byte| byte == b'\n').enumerate() {
        if line == b"\n" {
            break;
        }
        let end = offset + line.len();
        if idx == 0 {
            if !line.starts_with(b"tree ") {
                return Err(CommitSignature::Invalid);
            }
            has_tree = true;
        } else if line.starts_with(b"tree ") {
            return Err(CommitSignature::Invalid);
        }
        if line.starts_with(b"gpgsig ") {
            if signature.is_some() {
                return Err(CommitSignature::Invalid);
            }
            signature = Some(offset..end);
        } else if line.starts_with(b" ") {
            if let Some(range) = &mut signature {
                if range.end == offset {
                    range.end = end;
                }
            }
        } else if line.starts_with(b"author ") {
            if has_author {
                return Err(CommitSignature::Invalid);
            }
            has_author = true;
        } else if let Some(value) = line.strip_prefix(b"committer ") {
            if committer.replace(committer_email(value)).is_some() {
                return Err(CommitSignature::Invalid);
            }
        } else if line.starts_with(b"gpgsig-sha256 ") {
            other_signature = true;
        }
        offset = end;
    }
    let Some(signature) = signature else {
        return Err(if other_signature {
            CommitSignature::Unsupported
        } else {
            CommitSignature::Unsigned
        });
    };
    if !has_tree || !has_author {
        return Err(CommitSignature::Invalid);
    }
    if signature.len() > SIGNATURE_BYTES {
        return Err(CommitSignature::Unavailable);
    }
    let armor_lines =
        std::str::from_utf8(&bytes[signature.clone()]).map_err(|_| CommitSignature::Invalid)?;
    let (begin, armor_lines) = armor_lines
        .split_once('\n')
        .ok_or(CommitSignature::Invalid)?;
    if begin.trim_end_matches('\r') != "gpgsig -----BEGIN PGP SIGNATURE-----" {
        return Err(CommitSignature::Unsupported);
    }
    let mut armor = String::with_capacity(signature.len());
    armor.push_str("-----BEGIN PGP SIGNATURE-----\n");
    for line in armor_lines.split_inclusive('\n') {
        armor.push_str(line.strip_prefix(' ').ok_or(CommitSignature::Invalid)?);
    }
    if !armor.trim_end().ends_with("\n-----END PGP SIGNATURE-----") {
        return Err(CommitSignature::Invalid);
    }
    Ok(SignedCommit {
        armor,
        email: committer.flatten().ok_or(CommitSignature::MissingKey)?,
        signature,
    })
}

#[derive(Deserialize)]
struct VerificationKey {
    signer: String,
    fingerprint: String,
    public_key: String,
    email_verified: i64,
}

pub(crate) async fn verify_commit(
    state: &AppState,
    handle: &RepoHandle,
    hash: &str,
) -> CommitSignature {
    let result = async {
        let Some(bytes) = handle.read_commit_bytes(hash).await? else {
            return Ok(CommitSignature::Unavailable);
        };
        let signed = match signed_commit(&bytes) {
            Ok(signed) => signed,
            Err(status) => return Ok(status),
        };
        let signature = match shard_gpg_signature(&signed.armor).await {
            Ok(signature) => signature,
            Err(_) => return Ok(CommitSignature::Invalid),
        };
        let Some(key_id) = signature
            .get(0)
            .as_string()
            .filter(|id| valid_key_hex(id, &[16]))
        else {
            return Ok(CommitSignature::Invalid);
        };
        let keys: Vec<VerificationKey> = db::all(
            &state.db()?,
            "SELECT u.username AS signer, k.fingerprint, k.public_key,
                    u.email_verified_at IS NOT NULL AS email_verified
             FROM users u JOIN gpg_keys k ON k.user_id = u.id AND k.email = u.email
             WHERE u.email = ?1 COLLATE NOCASE AND EXISTS (
               SELECT 1 FROM json_each(k.key_ids) WHERE value = ?2
             ) ORDER BY k.fingerprint",
            &[util::js_text(signed.email), util::js_text(&key_id)],
        )
        .await?;
        if keys.is_empty() {
            return Ok(CommitSignature::MissingKey);
        }
        if shard_git_commit_oid(&bytes)
            .await
            .map_err(worker::Error::from)?
            != hash
        {
            return Ok(CommitSignature::Unavailable);
        }
        // The signed data is the original object with the full gpgsig header
        // removed, including its space-prefixed continuation lines.
        let mut payload = Vec::with_capacity(bytes.len() - signed.signature.len());
        payload.extend_from_slice(&bytes[..signed.signature.start]);
        payload.extend_from_slice(&bytes[signed.signature.end..]);
        let packet = signature.get(1);
        for key in keys {
            if matches!(
                shard_gpg_verify(&payload, &packet, &key.public_key, signed.email).await,
                Ok(true)
            ) {
                return Ok(CommitSignature::Valid {
                    fingerprint: key.fingerprint,
                    signer: key.signer,
                    email_verified: key.email_verified != 0,
                });
            }
        }
        Ok(CommitSignature::Invalid)
    }
    .await;
    result.unwrap_or_else(|_: worker::Error| {
        worker::console_log!("event=commit_signature_unavailable commit={hash}");
        CommitSignature::Unavailable
    })
}
