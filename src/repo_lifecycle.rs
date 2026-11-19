use crate::ctx::AppState;
use crate::handlers::common::{not_found, page_title, visible_repo};
use crate::git_storage::{CopyError, GitBackend, GitStorage};
use crate::i18n::{CommonText, LifecycleText as Text, RepoText};
use crate::limits::ScheduledLimits;
use crate::lfs::{self, Namespace};
use crate::models::Repo;
use crate::native_git::ImportCredential;
use crate::pagination::{PageRequest, Pagination, PAGE_SIZE};
use crate::templates::{render, Chrome};
use crate::util::{form_bool, form_value, js_int, js_text, now, redirect, valid_slug};
use crate::{crypto, db};
use askama::Template;
use serde::Deserialize;
use worker::d1::D1Database;
use wasm_bindgen::{prelude::{wasm_bindgen, JsError}, JsValue};
use wasm_bindgen_futures::{future_to_promise, JsFuture};
use worker::*;

const TRANSFERS: &str = "/user/settings/transfers";
const IMPORT: &str = "/user/settings/repository-import";
const COPIES: &str = "/user/settings/repository-copies";
const COPY_CHECK_INTERVAL: i64 = 30;
const COPY_START_GRACE: i64 = 300;

#[wasm_bindgen(inline_js = r#"
export async function shard_copy_workflow_active(env, id, createIfMissing) {
    const workflow = env.REPOSITORY_COPY_WORKFLOW;
    if (workflow == null) return false;

    const copyId = id.toString();
    let instance;
    try {
        instance = await workflow.get(copyId);
    } catch {
        if (!createIfMissing) return false;
        try {
            await workflow.create({ id: copyId, params: { copyId } });
            return true;
        } catch {
            // The submission and cron may both try to create this instance.
            instance = await workflow.get(copyId);
        }
    }

    const { status } = await instance.status();
    return status === "queued" || status === "running" || status === "waiting" ||
        status === "paused" || status === "waitingForPause";
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_copy_workflow_active(
        env: &Env, id: i64, create_if_missing: bool,
    ) -> std::result::Result<bool, JsValue>;
}

#[derive(Deserialize)]
struct Identity {
    id: i64,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Transfer {
    id: i64,
    owner: String,
    name: String,
    recipient: String,
}

#[derive(Template)]
#[template(path = "repo_lifecycle.html")]
struct LifecyclePage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    pending: Option<Transfer>,
    error: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "repo_transfers.html")]
struct TransfersPage {
    chrome: Chrome,
    title: String,
    incoming: Vec<Transfer>,
    pagination: Pagination,
    declined: bool,
    error: Option<&'static str>,
}

pub(crate) fn login(path: &str) -> Result<Response> {
    redirect(&format!("/user/login?next={}", crate::util::url_path(path)))
}

pub(crate) fn repo_path(ctx: &RouteContext<AppState>) -> (&str, &str) {
    (
        ctx.param("owner").map_or("", String::as_str),
        ctx.param("repo").map_or("", String::as_str),
    )
}

pub(crate) fn repository_not_found(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(CommonText::RepositoryNotFound))
}

pub(crate) async fn owned_repo(ctx: &RouteContext<AppState>) -> Result<Option<Repo>> {
    let (owner, name) = repo_path(ctx);
    Ok(visible_repo(&ctx.data, owner, name).await?.filter(|repo| {
        ctx.data.user_id() == Some(repo.owner_id)
    }))
}

fn settings_path(repo: &Repo) -> String {
    format!("/{}/settings/lifecycle", repo.full_name())
}

pub async fn settings(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        let (owner, name) = repo_path(&ctx);
        return login(&format!("/{owner}/{name}/settings/lifecycle"));
    }
    let Some(repo) = owned_repo(&ctx).await? else {
        return repository_not_found(state);
    };
    render_settings(state, repo, None, 200).await
}

async fn render_settings(
    state: &AppState,
    repo: Repo,
    error: Option<&'static str>,
    status: u16,
) -> Result<Response> {
    let pending = db::first(
        &state.db()?,
        "SELECT t.id, s.username AS owner, r.name, u.username AS recipient
         FROM repo_transfers t JOIN repos r ON r.id = t.repo_id
         JOIN users s ON s.id = t.sender_id JOIN users u ON u.id = t.recipient_id
         WHERE t.repo_id = ? AND t.sender_id = ? AND r.owner_id = t.sender_id",
        &[js_int(repo.id), js_int(repo.owner_id)],
    )
    .await?;
    Ok(render(&LifecyclePage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::RepositoryLifecycle), &repo.full_name()]),
        repo,
        pending,
        error,
    })?
    .with_status(status))
}

pub async fn update(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = owned_repo(&ctx).await? else {
        return repository_not_found(state);
    };
    let form = state.form()?;
    let action = form_value(form, "action");
    let database = state.db()?;
    let failure = match action.as_deref() {
        Some("rename") => {
            let submitted = form_value(form, "name").unwrap_or_default();
            let name = submitted.trim();
            if !valid_slug(name) {
                return render_settings(state, repo, Some(state.locale.t(RepoText::InvalidName)), 400).await;
            }
            let renamed = db::first::<Identity>(
                &database,
                "UPDATE repos SET name = ?1, updated_at = ?2
                 WHERE id = ?3 AND owner_id = ?4 AND name = ?5 COLLATE BINARY
                   AND NOT EXISTS (SELECT 1 FROM repo_transfers WHERE repo_id = ?3)
                   AND NOT EXISTS (SELECT 1 FROM repo_paths WHERE owner_id = ?4 AND name = ?1
                       AND repo_id IS NOT ?3)
                 RETURNING id",
                &[js_text(name), js_int(now()), js_int(repo.id), js_int(repo.owner_id), js_text(&repo.name)],
            )
            .await?;
            if renamed.is_some() {
                return redirect(&format!("/{}/{name}/settings/lifecycle", repo.owner_name));
            }
            let transferring = db::first::<Identity>(
                &database,
                "SELECT id FROM repo_transfers WHERE repo_id = ?",
                &[js_int(repo.id)],
            )
            .await?;
            if transferring.is_some() {
                Text::PendingTransferBlocksRename
            } else {
                Text::NameTaken
            }
        }
        Some("request_transfer") => {
            let recipient = form_value(form, "recipient").unwrap_or_default();
            let recipient = db::first::<Identity>(
                &database,
                "SELECT id FROM users WHERE username = ? COLLATE NOCASE AND id <> ?",
                &[js_text(recipient.trim()), js_int(repo.owner_id)],
            )
            .await?;
            let Some(recipient) = recipient else {
                return render_settings(state, repo, Some(state.locale.t(Text::UnknownRecipient)), 400).await;
            };
            let requested = db::first::<Identity>(
                &database,
                "INSERT INTO repo_transfers (repo_id, sender_id, recipient_id, created_at)
                 SELECT id, owner_id, ?1, ?2 FROM repos
                 WHERE id = ?3 AND owner_id = ?4 AND name = ?5 COLLATE BINARY
                   AND EXISTS (SELECT 1 FROM users WHERE id = ?1)
                   AND NOT EXISTS (SELECT 1 FROM repo_transfers WHERE repo_id = ?3)
                 RETURNING id",
                &[js_int(recipient.id), js_int(now()), js_int(repo.id), js_int(repo.owner_id), js_text(&repo.name)],
            )
            .await?;
            if requested.is_some() {
                return redirect(&settings_path(&repo));
            }
            Text::RequestChanged
        }
        Some("cancel_transfer") => {
            if let Some(id) = form_id(form, "transfer_id") {
                let cancelled = db::first::<Identity>(
                    &database,
                    "DELETE FROM repo_transfers WHERE id = ? AND repo_id = ? AND sender_id = ?
                       AND sender_id = (SELECT owner_id FROM repos WHERE id = repo_id)
                     RETURNING id",
                    &[js_int(id), js_int(repo.id), js_int(repo.owner_id)],
                )
                .await?;
                if cancelled.is_some() {
                    return redirect(&settings_path(&repo));
                }
            }
            Text::RequestChanged
        }
        _ => Text::RequestChanged,
    };
    // Refresh ownership before rendering an error after a concurrent transfer.
    let Some(repo) = owned_repo(&ctx).await? else {
        return repository_not_found(state);
    };
    render_settings(state, repo, Some(state.locale.t(failure)), 409).await
}

fn positive_id(value: &str) -> Option<i64> {
    let id = value.parse::<i64>().ok()?;
    (1..=9_007_199_254_740_991).contains(&id).then_some(id)
}

fn form_id(form: &FormData, field: &str) -> Option<i64> {
    positive_id(&form_value(form, field)?)
}

pub async fn transfers(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = state.user.as_ref() else {
        return login(TRANSFERS);
    };
    render_transfers(&req, state, user.id, None).await
}

async fn render_transfers(
    req: &Request,
    state: &AppState,
    user_id: i64,
    error: Option<Text>,
) -> Result<Response> {
    let page = PageRequest::new(req, "page");
    let mut incoming = db::all(
        &state.db()?,
        "SELECT t.id, s.username AS owner, r.name, u.username AS recipient
         FROM repo_transfers t JOIN repos r ON r.id = t.repo_id AND r.owner_id = t.sender_id
         JOIN users s ON s.id = t.sender_id JOIN users u ON u.id = t.recipient_id
         WHERE t.recipient_id = ? ORDER BY t.id DESC LIMIT ? OFFSET ?",
        &[js_int(user_id), js_int(PAGE_SIZE as i64 + 1), js_int(page.offset())],
    )
    .await?;
    let pagination = page.finish(req, &mut incoming, None)?;
    Ok(render(&TransfersPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::Transfers)]),
        incoming,
        pagination,
        declined: error.is_none() && crate::util::query_param(req, "declined").as_deref() == Some("1"),
        error: error.map(|key| state.locale.t(key)),
    })?
    .with_status(if error.is_some() { 409 } else { 200 }))
}

pub async fn respond_transfer(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = state.user.as_ref() else {
        return login(TRANSFERS);
    };
    let form = state.form()?;
    let Some(id) = form_id(form, "transfer_id") else {
        return not_found(state, state.locale.t(Text::TransferNotFound));
    };
    let database = state.db()?;
    let args = [js_int(id), js_int(user.id)];
    match form_value(form, "action").as_deref() {
        Some("accept") => {
            let accepted = db::first::<Name>(
                &database,
                "UPDATE repos AS r SET owner_id = ?2, updated_at = ?3
                 WHERE id = (SELECT repo_id FROM repo_transfers WHERE id = ?1 AND recipient_id = ?2
                                AND sender_id = r.owner_id)
                   AND NOT EXISTS (SELECT 1 FROM repo_paths WHERE owner_id = ?2 AND name = r.name
                       AND repo_id IS NOT r.id)
                 RETURNING name",
                &[js_int(id), js_int(user.id), js_int(now())],
            )
            .await?;
            if let Some(accepted) = accepted {
                return redirect(&format!("/{}/{}", user.username, accepted.name));
            }
            let pending = db::first::<Identity>(
                &database,
                "SELECT id FROM repo_transfers WHERE id = ? AND recipient_id = ?",
                &args,
            )
            .await?;
            if pending.is_some() {
                return render_transfers(&req, state, user.id, Some(Text::TransferNameTaken)).await;
            }
        }
        Some("decline") => {
            let declined = db::first::<Identity>(
                &database,
                "DELETE FROM repo_transfers WHERE id = ? AND recipient_id = ? RETURNING id",
                &args,
            )
            .await?;
            if declined.is_some() {
                return redirect(&format!("{TRANSFERS}?declined=1"));
            }
        }
        _ => {}
    }
    not_found(state, state.locale.t(Text::TransferNotFound))
}

#[derive(Deserialize)]
struct CopyJob {
    id: i64,
    name: String,
    storage_id: String,
    git_backend: GitBackend,
    kind: String,
    state: String,
    created_at: i64,
    repo_id: Option<i64>,
}

const COPY_COLUMNS: &str = "id, name, artifacts_repo AS storage_id, git_backend, kind, state, created_at, repo_id";

impl CopyJob {
    fn is_active(&self) -> bool {
        matches!(self.state.as_str(), "starting" | "pending")
    }

    fn awaiting_submission(&self) -> bool {
        self.state == "starting" && now().saturating_sub(self.created_at) < COPY_START_GRACE
    }
}

enum CopyProgress {
    Done,
    Advanced,
    Waiting,
    Unknown,
}

async fn copy_workflow_active(env: &Env, id: i64, create_if_missing: bool) -> bool {
    match shard_copy_workflow_active(env, id, create_if_missing).await {
        Ok(active) => active,
        Err(_) => {
            console_error!("Unable to reach Workflow for repository copy {id}");
            false
        }
    }
}

#[wasm_bindgen]
pub struct RepositoryCopyStep {}

#[wasm_bindgen]
impl RepositoryCopyStep {
    pub async fn advance(env: Env, canonical_id: String) -> std::result::Result<JsValue, JsError> {
        let id = positive_id(&canonical_id)
            .filter(|id| id.to_string() == canonical_id)
            .ok_or_else(|| JsError::new("Invalid repository copy job ID"))?;
        let database = env.d1("DB")?;
        let Some(job) = db::first(
            &database,
            &format!("SELECT {COPY_COLUMNS} FROM repo_copies WHERE id = ?"),
            &[js_int(id)],
        ).await? else {
            return Ok(JsValue::from_str("done"));
        };
        let lfs_limit = ScheduledLimits::read(&env)?.lfs_copy_objects;
        let progress = reconcile_copy(&env, &database, &job, lfs_limit).await?;
        Ok(JsValue::from_str(match progress {
            CopyProgress::Done => "done",
            CopyProgress::Advanced => "advanced",
            CopyProgress::Waiting => "waiting",
            CopyProgress::Unknown => return Err(JsError::new("Repository copy storage unavailable")),
        }))
    }
}

#[derive(Template)]
#[template(path = "repo_copy_form.html")]
struct CopyForm<'a> {
    chrome: Chrome,
    title: String,
    heading: &'static str,
    help: &'static str,
    action: String,
    cancel: String,
    source: Option<&'a Repo>,
    private_source: bool,
    destination: Destination,
    source_url: &'a str,
    error: Option<&'static str>,
}

#[derive(Default)]
struct Destination {
    name: String,
    description: String,
    private: bool,
}

impl Destination {
    fn from_form(form: &FormData) -> Self {
        Self {
            name: form_value(form, "name").unwrap_or_default().trim().to_owned(),
            description: form_value(form, "description").unwrap_or_default(),
            private: form_bool(form, "private"),
        }
    }
}

#[derive(Template)]
#[template(path = "repo_copy_status.html")]
struct CopyStatus {
    chrome: Chrome,
    title: String,
    job: CopyJob,
    message: &'static str,
    check_again: bool,
}

#[derive(Template)]
#[template(path = "repo_copies.html")]
struct CopiesPage {
    chrome: Chrome,
    title: String,
    copies: Vec<CopyJob>,
    pagination: Pagination,
}

fn render_copy_form(
    state: &AppState,
    source: Option<&Repo>,
    destination: Destination,
    source_url: &str,
    error: Option<(Text, u16)>,
) -> Result<Response> {
    let (heading, help, action, cancel) = match source {
        Some(repo) => (Text::Fork, Text::ForkHelp, format!("/{}/fork", repo.full_name()), format!("/{}", repo.full_name())),
        None => (Text::Import, Text::ImportHelp, IMPORT.to_owned(), "/new".to_owned()),
    };
    let heading = state.locale.t(heading);
    Ok(render(&CopyForm {
        chrome: Chrome::new(state),
        title: page_title(&[heading]),
        heading,
        help: state.locale.t(help),
        action,
        cancel,
        source,
        private_source: source.is_some_and(Repo::is_private),
        destination,
        source_url,
        error: error.map(|(key, _)| state.locale.t(key)),
    })?
    .with_status(error.map_or(200, |(_, status)| status)))
}

pub async fn import_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    if ctx.data.user.is_none() {
        return login(IMPORT);
    }
    render_copy_form(&ctx.data, None, Destination::default(), "", None)
}

pub async fn import(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        return login(IMPORT);
    }
    let form = state.form()?;
    let destination = Destination::from_form(form);
    let source_url = form_value(form, "source_url").unwrap_or_default();
    let Some(source_url) = public_https(source_url.trim()) else {
        return render_copy_form(state, None, destination, "", Some((Text::InvalidSource, 400)));
    };
    let credential = match Credentials::from_form(form) {
        Ok(credential) if credential.is_none() || source_url.port().is_none() => credential,
        _ => return render_copy_form(state, None, destination, "", Some((Text::InvalidImportCredential, 400))),
    };
    start_copy(state, None, destination, source_url.as_str(), credential).await
}

pub(crate) fn public_https(raw: &str) -> Option<Url> {
    let (scheme, rest) = raw.split_once("://")?;
    let authority = rest.split('/').next()?;
    if raw.len() > 2048
        || !scheme.eq_ignore_ascii_case("https")
        || authority.is_empty()
        || authority.contains('@')
        || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = url.domain()?;
    let (domain, suffix) = host.rsplit_once('.')?;
    // Accept public DNS-style names. The Git provider applies its upstream policy.
    if domain.is_empty()
        || host.len() > 253
        || !host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        || url.as_str().len() > 2048
        || matches!(suffix, "localhost" | "local" | "localdomain" | "internal" | "lan" | "onion" | "home" | "arpa")
        || !suffix.bytes().any(|byte| byte.is_ascii_alphabetic())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().trim_matches('/').is_empty()
    {
        return None;
    }
    Some(url)
}

enum Credentials {
    Basic { username: String, password: String },
    Bearer(String),
}

impl Credentials {
    fn from_form(form: &FormData) -> std::result::Result<Option<Self>, ()> {
        let username = form_value(form, "source_username");
        let password = form_value(form, "source_password");
        let token = form_value(form, "source_token");
        match (username, password, token) {
            (None, None, None) => Ok(None),
            (Some(username), Some(password), None)
                if username.len() <= 128 && !username.contains(':')
                    && !username.chars().any(char::is_control)
                    && password.len() <= 1024 && !password.chars().any(char::is_control) =>
            {
                Ok(Some(Self::Basic { username, password }))
            }
            (None, None, Some(token))
                if token.len() <= 1024 && token.bytes().all(|byte| (b'!'..=b'~').contains(&byte)) =>
            {
                Ok(Some(Self::Bearer(token)))
            }
            _ => Err(()),
        }
    }

    fn for_origin(&self) -> ImportCredential<'_> {
        match self {
            Self::Basic { username, password } => ImportCredential::Basic { username, password },
            Self::Bearer(token) => ImportCredential::Bearer { token },
        }
    }
}

pub async fn fork_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let (owner, name) = repo_path(&ctx);
    if state.user.is_none() {
        return login(&format!("/{owner}/{name}/fork"));
    }
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return repository_not_found(state);
    };
    let destination = Destination {
        name: repo.name.clone(),
        description: repo.description.clone(),
        private: repo.is_private(),
    };
    render_copy_form(state, Some(&repo), destination, "", None)
}

pub async fn fork(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let (owner, name) = repo_path(&ctx);
    if state.user.is_none() {
        return login(&format!("/{owner}/{name}/fork"));
    }
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return repository_not_found(state);
    };
    start_copy(state, Some(&repo), Destination::from_form(state.form()?), "", None).await
}

async fn start_copy(
    state: &AppState,
    source: Option<&Repo>,
    mut destination: Destination,
    source_url: &str,
    credential: Option<Credentials>,
) -> Result<Response> {
    let Some(owner_id) = state.user_id() else {
        return login(IMPORT);
    };
    let authenticated_import = credential.is_some();
    let form_url = if authenticated_import { "" } else { source_url };
    if authenticated_import {
        destination.private = true;
    }
    if !valid_slug(&destination.name) {
        return render_copy_form(state, source, destination, form_url, Some((Text::InvalidName, 400)));
    }
    let backend = if authenticated_import {
        Ok(GitBackend::Native)
    } else {
        source.map_or_else(|| GitBackend::selected(&state.env), |repo| Ok(repo.git_backend))
    };
    let storage = backend.and_then(|backend| Ok((backend, GitStorage::from_env(&state.env, backend)?)));
    let Ok((backend, storage)) = storage else {
        let error = if authenticated_import { Text::ImportNativeRequired } else { Text::CopyUnknown };
        return render_copy_form(state, source, destination, form_url, Some((error, 503)));
    };
    let storage_id = format!("shard-copy-{}", &crypto::random_token()?[..32]);
    let database = state.db()?;
    let inserted = db::first::<Identity>(
        &database,
        "INSERT INTO repo_copies (owner_id, name, description, visibility, private_fork,
                                  artifacts_repo, kind, source_id, created_at, git_backend)
         SELECT ?1, ?2, ?3, ?4, COALESCE((SELECT visibility = 'private' FROM repos WHERE id = ?7), 0),
                ?5, ?6, ?7, ?8, ?9
         WHERE NOT EXISTS (SELECT 1 FROM repo_paths WHERE owner_id = ?1 AND name = ?2 COLLATE NOCASE)
           AND NOT EXISTS (SELECT 1 FROM repos WHERE artifacts_repo = ?5 AND git_backend = ?9)
           AND NOT EXISTS (SELECT 1 FROM repo_storage_cleanup WHERE artifacts_repo = ?5 AND git_backend = ?9)
           AND (?7 IS NULL OR EXISTS (
             SELECT 1 FROM repos s JOIN users u ON u.id = ?1 WHERE s.id = ?7
               AND s.git_backend = ?9
               AND (s.visibility = 'public' OR s.owner_id = ?1 OR u.is_admin = 1 OR EXISTS (
                 SELECT 1 FROM repo_member_roles rc WHERE rc.repo_id = s.id AND rc.user_id = ?1
               ))
           ))
         RETURNING id",
        &[
            js_int(owner_id),
            js_text(&destination.name),
            js_text(&destination.description),
            js_text(if destination.private { "private" } else { "public" }),
            js_text(&storage_id),
            js_text(if source.is_some() { "fork" } else { "import" }),
            source.map_or(JsValue::NULL, |repo| js_int(repo.id)),
            js_int(now()),
            js_text(backend.name()),
        ],
    )
    .await?;
    let Some(inserted) = inserted else {
        if let Some(source) = source {
            if visible_repo(state, &source.owner_name, &source.name).await?.is_none() {
                return repository_not_found(state);
            }
        }
        return render_copy_form(state, source, destination, form_url, Some((Text::NameTaken, 409)));
    };
    let id = inserted.id;
    let status_path = format!("{COPIES}/{id}");
    let source_repo = source.map(|repo| repo.storage_id.clone());
    let source_url = source_url.to_owned();
    let env = state.env.clone();
    let submission = future_to_promise(async move {
        let result = match (credential, &storage) {
            (Some(credential), GitStorage::Native(origin)) => {
                origin.start_copy_with_credential(&storage_id, None, &source_url, Some(&credential.for_origin())).await
            }
            (None, _) => storage.start_copy(&storage_id, &destination.description, source_repo.as_deref(), &source_url).await,
            (Some(_), _) => Err(CopyError::Rejected),
        };
        let next = match result {
            // Another repo owns this opaque identity. Never inspect or delete it.
            Err(CopyError::Collision) => "failed",
            Err(CopyError::Missing | CopyError::Rejected) => "discard",
            Err(CopyError::Unknown) => {
                console_error!("Git copy submission {id} returned an unknown storage result");
                "pending"
            }
            Ok(()) | Err(CopyError::Pending) => "pending",
        };
        db::run(
            &database,
            "UPDATE repo_copies SET state = ? WHERE id = ? AND state = 'starting'",
            &[js_text(next), js_int(id)],
        )
        .await
        .map_err(|_| JsValue::NULL)?;
        let managed = next == "pending" && copy_workflow_active(&env, id, true).await;
        Ok(JsValue::from_bool(managed))
    });
    // Keep recording the provider outcome if the initiating client disconnects.
    let background = submission.clone();
    state.worker_ctx.wait_until(async move {
        if JsFuture::from(background).await.is_err() {
            console_error!("Unable to record Git copy submission {id}");
        }
    });
    let Ok(managed) = JsFuture::from(submission).await else {
        return redirect(&status_path);
    };

    let Some(job) = load_copy(state, id).await? else {
        return copy_not_found(state);
    };
    let job = if managed.as_bool() != Some(true) && job.is_active() && !job.awaiting_submission() {
        let limit = ScheduledLimits::read(&state.env)?.lfs_copy_objects;
        reconcile_copy(&state.env, &state.db()?, &job, limit).await?;
        load_copy(state, id).await?
    } else {
        Some(job)
    };
    if let Some(url) = job.and_then(|job| ready_path(state, &job)) {
        return redirect(&url);
    }
    redirect(&status_path)
}

async fn load_copy(state: &AppState, id: i64) -> Result<Option<CopyJob>> {
    let Some(owner_id) = state.user_id() else {
        return Ok(None);
    };
    db::first(
        &state.db()?,
        &format!("SELECT {COPY_COLUMNS} FROM repo_copies WHERE id = ? AND owner_id = ?"),
        &[js_int(id), js_int(owner_id)],
    )
    .await
}

fn ready_path(state: &AppState, job: &CopyJob) -> Option<String> {
    (job.state == "ready").then_some(())?;
    job.repo_id?;
    // Resolve through the original path so later transfers still enforce current permissions.
    Some(format!("/{}/{}", state.user.as_ref()?.username, job.name))
}

fn copy_not_found(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(Text::CopyNotFound))
}

fn copy_response(state: &AppState, job: CopyJob, unknown: bool) -> Result<Response> {
    if job.state == "ready" {
        return match ready_path(state, &job) {
            Some(url) => redirect(&url),
            None => copy_not_found(state),
        };
    }
    let (message, status, check_again) = match job.state.as_str() {
        "failed" => (Text::CopyFailed, 422, false),
        "discard" => (Text::CopyCleanup, 202, true),
        _ if unknown => (Text::CopyUnknown, 503, true),
        _ => (Text::CopyPending, 202, true),
    };
    let mut response = render(&CopyStatus {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::CopyStatus)]),
        job,
        message: state.locale.t(message),
        check_again,
    })?
    .with_status(status);
    if check_again {
        response.headers_mut().set("Retry-After", "5")?;
    }
    Ok(response)
}

pub async fn copies(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(owner_id) = state.user_id() else {
        return login(COPIES);
    };
    let page = PageRequest::new(&req, "page");
    let mut copies = db::all(
        &state.db()?,
        &format!("SELECT {COPY_COLUMNS} FROM repo_copies WHERE owner_id = ? AND state <> 'ready' ORDER BY id DESC LIMIT ? OFFSET ?"),
        &[js_int(owner_id), js_int(PAGE_SIZE as i64 + 1), js_int(page.offset())],
    )
    .await?;
    let pagination = page.finish(&req, &mut copies, None)?;
    render(&CopiesPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::Copies)]),
        copies,
        pagination,
    })
}

pub async fn copy_status(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    copy_status_page(ctx, false).await
}

pub async fn poll_copy(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    ctx.data.form()?;
    copy_status_page(ctx, true).await
}

async fn copy_status_page(ctx: RouteContext<AppState>, poll: bool) -> Result<Response> {
    let state = &ctx.data;
    let Some(id) = ctx.param("id").and_then(|value| positive_id(value)) else {
        return copy_not_found(state);
    };
    if state.user.is_none() {
        return login(&format!("{COPIES}/{id}"));
    }
    let Some(job) = load_copy(state, id).await? else {
        return copy_not_found(state);
    };
    if poll && job.is_active() && !job.awaiting_submission() {
        let unknown = if copy_workflow_active(&state.env, id, false).await {
            false
        } else {
            let limit = ScheduledLimits::read(&state.env)?.lfs_copy_objects;
            let progress = reconcile_copy(&state.env, &state.db()?, &job, limit).await?;
            matches!(progress, CopyProgress::Unknown)
        };
        let Some(job) = load_copy(state, id).await? else {
            return copy_not_found(state);
        };
        return copy_response(state, job, unknown);
    }
    copy_response(state, job, false)
}

/// Poll a bounded rotation. Claiming a check advances even jobs whose storage is unavailable.
pub(crate) async fn reconcile_pending(env: &Env, limits: &ScheduledLimits) -> Result<()> {
    let database = env.d1("DB")?;
    let checked_at = now();
    let jobs: Vec<CopyJob> = db::all(
        &database,
        &format!(
            "UPDATE repo_copies SET last_checked_at = ?1
             WHERE id IN (
                SELECT id FROM repo_copies
                 WHERE state IN ('starting', 'pending') AND lfs_lease_until <= ?1
                   AND (last_checked_at IS NULL OR last_checked_at <= ?2)
                 ORDER BY COALESCE(last_checked_at, created_at), id LIMIT ?3
             ) RETURNING {COPY_COLUMNS}"
        ),
        &[js_int(checked_at), js_int(checked_at - COPY_CHECK_INTERVAL), js_int(limits.repo_copies)],
    )
    .await?;
    let mut failed = false;
    for job in jobs {
        if job.awaiting_submission() || copy_workflow_active(env, job.id, true).await {
            continue;
        }
        match reconcile_copy(env, &database, &job, limits.lfs_copy_objects).await {
            Ok(progress) => failed |= matches!(progress, CopyProgress::Unknown),
            Err(error) => {
                failed = true;
                console_error!("Unable to reconcile Git or LFS copy {}: {error}", job.id);
            }
        }
    }
    if let Err(error) = reconcile_cleanup(env, &database, now(), limits).await {
        failed = true;
        console_error!("Repository storage cleanup: {error}");
    }
    if failed {
        return Err(Error::RustError("Repository lifecycle reconciliation failed".into()));
    }
    Ok(())
}

async fn reconcile_copy(env: &Env, database: &D1Database, job: &CopyJob, lfs_limit: u32) -> Result<CopyProgress> {
    if !job.is_active() {
        return Ok(CopyProgress::Done);
    }
    if job.awaiting_submission() {
        return Ok(CopyProgress::Waiting);
    }
    let storage = match GitStorage::from_env(env, job.git_backend) {
        Ok(storage) => storage,
        Err(error) => {
            console_error!("Unable to open Git storage for copy {}: {error}", job.id);
            return Ok(CopyProgress::Unknown);
        }
    };
    let id = [js_int(job.id)];
    let info = match storage.copy_info(&job.storage_id).await {
        Err(CopyError::Missing) if now().saturating_sub(job.created_at) >= COPY_START_GRACE => {
            let discarded = db::first::<Identity>(
                database,
                "UPDATE repo_copies SET state = 'discard'
                 WHERE id = ? AND state = ? AND lfs_lease_until <= ? RETURNING id",
                &[js_int(job.id), js_text(&job.state), js_int(now())],
            ).await?;
            return Ok(if discarded.is_some() { CopyProgress::Done } else { CopyProgress::Waiting });
        }
        Err(CopyError::Missing | CopyError::Pending) => return Ok(CopyProgress::Waiting),
        Err(error) => {
            console_error!("Unable to inspect Git copy {}: {error:?}", job.id);
            return Ok(CopyProgress::Unknown);
        }
        Ok(info) if info.name != job.storage_id => {
            console_error!("Git storage returned a mismatched identity for copy {}", job.id);
            return Ok(CopyProgress::Unknown);
        }
        Ok(info) => info,
    };

    if job.state == "starting" {
        db::run(database, "UPDATE repo_copies SET state = 'pending' WHERE id = ? AND state = 'starting'", &id).await?;
    }
    let copied_page = if job.kind == "fork" {
        match copy_lfs_page(env, database, job.id, lfs_limit).await {
            Ok(copied) => copied,
            Err(error) => {
                console_error!("Unable to copy LFS objects for fork {}: {error}", job.id);
                discard_revoked_copy(database, job.id).await?;
                return Ok(CopyProgress::Unknown);
            }
        }
    } else {
        false
    };
    let published = db::first::<Identity>(
        database,
        "INSERT INTO repos (owner_id, name, description, default_branch, artifacts_repo, artifacts_remote,
                            visibility, forked_from, created_at, updated_at, private_fork, git_backend, lfs_copy_id)
         SELECT c.owner_id, c.name, c.description, ?2, c.artifacts_repo, ?3,
                CASE WHEN c.private_fork = 1 THEN 'private' ELSE c.visibility END,
                CASE WHEN c.private_fork = 0 THEN u.username || '/' || s.name END, ?4, ?4, c.private_fork, c.git_backend,
                CASE WHEN c.kind = 'fork' THEN c.id END
         FROM repo_copies c JOIN repo_copy_access a ON a.id = c.id
         LEFT JOIN repos s ON s.id = c.source_id LEFT JOIN users u ON u.id = s.owner_id
         WHERE c.id = ?1 AND c.state = 'pending' AND a.allowed = 1
           AND (c.kind = 'import' OR (c.lfs_complete = 1 AND c.lfs_lease_until = 0))
         RETURNING id",
        &[js_int(job.id), js_text(&info.default_branch), js_text(&info.remote), js_int(now())],
    ).await?;
    if published.is_some() || discard_revoked_copy(database, job.id).await? {
        Ok(CopyProgress::Done)
    } else if copied_page {
        Ok(CopyProgress::Advanced)
    } else {
        Ok(CopyProgress::Waiting)
    }
}

async fn discard_revoked_copy(database: &D1Database, id: i64) -> Result<bool> {
    db::first::<Identity>(
        database,
        "UPDATE repo_copies SET state = 'discard' WHERE id = ? AND state = 'pending'
           AND lfs_lease_until <= ? AND id IN (SELECT id FROM repo_copy_access WHERE allowed = 0)
         RETURNING id",
        &[js_int(id), js_int(now())],
    ).await.map(|row| row.is_some())
}

#[derive(Deserialize)]
struct CopyPage {
    source_id: i64,
    source_copy_id: Option<i64>,
    lfs_cursor: Option<String>,
}

async fn copy_lfs_page(env: &Env, database: &D1Database, id: i64, limit: u32) -> Result<bool> {
    let checked_at = now();
    let expires = checked_at + lfs::WRITE_LEASE;
    let Some(page) = db::first::<CopyPage>(
        database,
        "UPDATE repo_copies SET lfs_lease_until = ?2
         WHERE id = ?1 AND state = 'pending' AND kind = 'fork' AND lfs_complete = 0
           AND lfs_lease_until <= ?3 AND id IN (SELECT id FROM repo_copy_access WHERE allowed = 1)
         RETURNING source_id, lfs_cursor,
                   (SELECT s.lfs_copy_id FROM repos s WHERE s.id = repo_copies.source_id) AS source_copy_id",
        &[js_int(id), js_int(expires), js_int(checked_at)],
    ).await? else {
        return Ok(false);
    };

    let result = lfs::copy_page(
        env,
        Namespace::for_repo(page.source_id, page.source_copy_id),
        Namespace::Fork(id),
        page.lfs_cursor.as_deref(),
        expires,
        limit,
    ).await;
    match result {
        Ok(cursor) => db::first::<Identity>(
            database,
            "UPDATE repo_copies SET lfs_lease_until = 0, lfs_cursor = ?3, lfs_complete = ?4
             WHERE id = ?1 AND state = 'pending' AND lfs_lease_until = ?2 RETURNING id",
            &[js_int(id), js_int(expires), cursor.as_deref().map_or(JsValue::NULL, js_text), js_int(i64::from(cursor.is_none()))],
        ).await.map(|row| row.is_some()),
        Err(error) => {
            db::run(
                database,
                "UPDATE repo_copies SET lfs_lease_until = 0 WHERE id = ? AND lfs_lease_until = ?",
                &[js_int(id), js_int(expires)],
            ).await?;
            Err(error)
        }
    }
}

#[derive(Deserialize)]
struct Cleanup {
    id: i64,
    repo_id: Option<i64>,
    copy_id: Option<i64>,
    lfs_copy_id: Option<i64>,
    artifacts_repo: String,
    git_backend: GitBackend,
    git_deleted: i64,
}

async fn reconcile_cleanup(env: &Env, database: &D1Database, checked_at: i64, limits: &ScheduledLimits) -> Result<()> {
    let cutoff = [js_int(checked_at)];
    database.batch(vec![
        database.prepare(
            "DELETE FROM repo_lfs_uploads WHERE id IN (
                 SELECT id FROM repo_lfs_uploads WHERE expires_at <= ? ORDER BY expires_at LIMIT 256
             )",
        ).bind(&cutoff)?,
        database.prepare(
            "DELETE FROM repo_lfs_multipart WHERE attempt IN (
                 SELECT attempt FROM repo_lfs_multipart WHERE expires_at <= ? ORDER BY expires_at LIMIT 32
             )",
        ).bind(&cutoff)?,
    ]).await?;
    let jobs: Vec<Cleanup> = db::all(
        database,
        "UPDATE repo_storage_cleanup SET due_at = ?2
         WHERE id IN (
             SELECT q.id FROM repo_storage_cleanup q WHERE q.due_at <= ?1
               AND NOT EXISTS (SELECT 1 FROM repo_lfs_uploads w WHERE w.repo_id = q.repo_id AND w.expires_at > ?1)
               AND NOT EXISTS (SELECT 1 FROM repo_copies c WHERE c.id = q.copy_id AND c.lfs_lease_until > ?1)
             ORDER BY q.due_at, q.id LIMIT ?3
         ) RETURNING id, repo_id, copy_id, lfs_copy_id, artifacts_repo, git_backend, git_deleted",
        &[js_int(checked_at), js_int(checked_at + COPY_CHECK_INTERVAL), js_int(limits.repo_cleanups)],
    ).await?;
    let mut failed = false;
    for job in jobs {
        if let Err(error) = clean_storage(env, database, &job, limits.lfs_cleanup_objects).await {
            failed = true;
            console_error!("Unable to clean repository storage job {}: {error}", job.id);
        }
    }
    if failed {
        return Err(Error::RustError("One or more repository storage cleanup jobs failed".into()));
    }
    Ok(())
}

async fn clean_storage(env: &Env, database: &D1Database, job: &Cleanup, lfs_limit: u32) -> Result<()> {
    let id = [js_int(job.id)];
    if job.git_deleted == 0 {
        let live = db::first::<Identity>(
            database,
            "SELECT id FROM repos WHERE artifacts_repo = ?1 AND git_backend = ?2
             UNION ALL
             SELECT id FROM repo_copies WHERE artifacts_repo = ?1 AND git_backend = ?2
               AND state IN ('starting', 'pending') LIMIT 1",
            &[js_text(&job.artifacts_repo), js_text(job.git_backend.name())],
        ).await?;
        if live.is_some() {
            return Err(Error::RustError("Git storage is still referenced".into()));
        }
        GitStorage::from_env(env, job.git_backend)?
            .delete(&job.artifacts_repo).await
            .map_err(|error| Error::RustError(format!("Git deletion failed: {error:?}")))?;
        db::run(database, "UPDATE repo_storage_cleanup SET git_deleted = 1 WHERE id = ?", &id).await?;
    }

    let namespace = job.lfs_copy_id.map(Namespace::Fork).or_else(|| job.repo_id.map(Namespace::Repo));
    if let Some(namespace) = namespace {
        if !lfs::delete_page(env, namespace, lfs_limit).await? {
            return Ok(());
        }
    }
    if let Some(copy_id) = job.copy_id {
        let mark_failed = database.prepare(
            "UPDATE repo_copies SET state = 'failed' WHERE id = ? AND state = 'discard'",
        ).bind(&[js_int(copy_id)])?;
        let finish = database.prepare("DELETE FROM repo_storage_cleanup WHERE id = ?").bind(&id)?;
        database.batch(vec![mark_failed, finish]).await?;
        return Ok(());
    }
    db::run(database, "DELETE FROM repo_storage_cleanup WHERE id = ?", &id).await
}
