use crate::ctx::AppState;
use crate::db;
use crate::git_storage::{RepoHandle, is_object_id};
use crate::handlers::common::{
    REPO_COLUMNS, VISIBLE_REPO_SQL, can_admin, can_write, forbidden, not_found, page_title,
    viewer_params, visible_repo,
};
use crate::i18n::{CommonText, Locale, RepoFeatureText as Text, RepoText};
use crate::models::{Repo, Ts};
use crate::pagination::{PAGE_SIZE, PageRequest, Pagination};
use crate::refs::{self, RefsError, Tag};
use crate::templates::{Chrome, RepoRow, error_response, render};
use crate::util::{form_value, js_int, js_text, now, query_param, redirect};
use askama::Template;
use serde::Deserialize;
use worker::*;

const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_TITLE_CHARS: usize = 200;
const MAX_TEMPLATE_NAME_CHARS: usize = 100;
const ADMIN_SQL: &str = "(r.owner_id = ?1 OR ?2 = 1)";
const WRITER_SQL: &str = "(r.owner_id = ?1 OR ?2 = 1 OR EXISTS (
    SELECT 1 FROM repo_member_roles m WHERE m.repo_id = r.id AND m.user_id = ?1 AND m.role = 'write'))";

async fn repository(ctx: &RouteContext<AppState>) -> Result<Option<Repo>> {
    let owner = ctx.param("owner").map_or("", String::as_str);
    let repo = ctx.param("repo").map_or("", String::as_str);
    visible_repo(&ctx.data, owner, repo).await
}

fn repo_missing(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(CommonText::RepositoryNotFound))
}

fn failure(state: &AppState, status: u16, message: Text) -> Result<Response> {
    error_response(&Chrome::new(state), status, state.locale.t(message))
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|id| (1..=9_007_199_254_740_991).contains(id))
}

fn route_id(ctx: &RouteContext<AppState>) -> Option<i64> {
    ctx.param("id").and_then(|id| parse_id(id))
}

#[derive(Deserialize)]
struct ReleaseSummary {
    id: i64,
    title: String,
    tag_name: String,
    published_at: Option<Ts>,
}

#[derive(Deserialize)]
struct Release {
    id: i64,
    tag_name: String,
    commit_hash: String,
    title: String,
    notes: String,
    published_at: Option<Ts>,
    version: i64,
}

#[derive(Template)]
#[template(path = "releases.html")]
struct ReleasesPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    writer: bool,
    releases: Vec<ReleaseSummary>,
    pagination: Pagination,
}

#[derive(Template)]
#[template(path = "release.html")]
struct ReleasePage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    writer: bool,
    release: Release,
    notes_html: String,
}

#[derive(Default)]
struct ReleaseInput {
    tag_name: String,
    title: String,
    notes: String,
}

impl ReleaseInput {
    fn read(form: &FormData) -> Self {
        Self {
            tag_name: form_value(form, "tag_name").unwrap_or_default(),
            title: form_value(form, "title").unwrap_or_default(),
            notes: form_value(form, "notes").unwrap_or_default(),
        }
    }

    fn error(&self) -> Option<Text> {
        let title = self.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS || title.chars().any(char::is_control) {
            Some(Text::InvalidReleaseTitle)
        } else if self.notes.len() > MAX_TEXT_BYTES {
            Some(Text::ReleaseNotesTooLong)
        } else {
            None
        }
    }
}

#[derive(Template)]
#[template(path = "release_edit.html")]
struct ReleaseEditor {
    chrome: Chrome,
    title: String,
    release_url: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    heading: &'static str,
    current: Option<Release>,
    published: bool,
    fields: ReleaseInput,
    tags: Vec<Tag>,
    missing_selection: bool,
    tag_notice: Option<&'static str>,
    error: Option<&'static str>,
}

pub async fn releases(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    let writer = can_write(state, &repo).await?;
    let page = PageRequest::new(&req, "page");
    let [viewer, admin] = viewer_params(state);
    let mut releases = db::all(
        &state.db()?,
        &format!(
            "SELECT l.id, l.title, l.tag_name, l.published_at FROM repo_releases l JOIN repos r ON r.id = l.repo_id
             WHERE r.id = ?3 AND {VISIBLE_REPO_SQL} AND (l.published_at IS NOT NULL OR {WRITER_SQL})
             ORDER BY COALESCE(l.published_at, l.created_at) DESC, l.id DESC LIMIT ?4 OFFSET ?5"
        ),
        &[viewer, admin, js_int(repo.id), js_int((PAGE_SIZE + 1) as i64), js_int(page.offset())],
    ).await?;
    let pagination = page.finish(&req, &mut releases, None)?;
    render(&ReleasesPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(Text::Releases)]),
        is_owner: can_admin(state, &repo),
        tab: "releases",
        repo,
        writer,
        releases,
        pagination,
    })
}

async fn load_release(state: &AppState, repo_id: i64, id: i64) -> Result<Option<Release>> {
    let [viewer, admin] = viewer_params(state);
    db::first(
        &state.db()?,
        &format!(
            "SELECT l.id, l.tag_name, l.commit_hash, l.title, l.notes, l.published_at, l.version
             FROM repo_releases l JOIN repos r ON r.id = l.repo_id
             WHERE r.id = ?3 AND l.id = ?4 AND {VISIBLE_REPO_SQL}
               AND (l.published_at IS NOT NULL OR {WRITER_SQL})"
        ),
        &[viewer, admin, js_int(repo_id), js_int(id)],
    ).await
}

pub async fn release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    let Some(id) = route_id(&ctx) else { return failure(state, 404, Text::MissingRelease) };
    let writer = can_write(state, &repo).await?;
    let Some(release) = load_release(state, repo.id, id).await? else {
        return failure(state, 404, Text::MissingRelease);
    };
    let notes_html = crate::markdown::render(&release.notes, &crate::markdown::Context::new(&repo, &release.commit_hash, ""));
    render(&ReleasePage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), &release.title]),
        is_owner: can_admin(state, &repo),
        tab: "releases",
        repo,
        writer,
        release,
        notes_html,
    })
}

async fn open_git(state: &AppState, repo: &Repo) -> std::result::Result<RepoHandle, RefsError> {
    RepoHandle::open(&state.env, repo).await.map_err(|error| {
        console_log!("releases: open Git repository: {error}");
        RefsError::Unavailable
    })
}

fn refs_message(error: RefsError) -> RepoText {
    match error {
        RefsError::Limit => RepoText::RefsLimit,
        RefsError::Unavailable => RepoText::StorageUnavailable,
    }
}

async fn resolve_tag(state: &AppState, repo: &Repo, tag_name: &str) -> std::result::Result<String, TagError> {
    if tag_name.is_empty() {
        return Err(TagError::Missing);
    }
    let handle = open_git(state, repo).await?;
    let inventory = refs::load(&handle).await?;
    let tag = inventory.tags.into_iter().find(|tag| tag.name == tag_name).ok_or(TagError::Missing)?;
    let hash = tag.peeled.unwrap_or(tag.oid);
    let commit = handle.read_commit(&hash).await.map_err(|error| {
        console_log!("releases: read tag target: {error}");
        RefsError::Unavailable
    })?;
    if commit.is_some_and(|commit| commit.hash == hash && commit.tree.as_deref().is_some_and(is_object_id)) {
        Ok(hash)
    } else {
        Err(TagError::Missing)
    }
}

enum TagError {
    Missing,
    Refs(RefsError),
}

impl From<RefsError> for TagError {
    fn from(error: RefsError) -> Self { Self::Refs(error) }
}

async fn release_editor(
    state: &AppState,
    repo: Repo,
    current: Option<Release>,
    fields: ReleaseInput,
    error: Option<(u16, Text)>,
) -> Result<Response> {
    let heading = if current.is_some() { Text::EditRelease } else { Text::NewRelease };
    let published = current.as_ref().is_some_and(|release| release.published_at.is_some());
    let (tags, tag_notice) = if published {
        (Vec::new(), None)
    } else {
        let inventory = match open_git(state, &repo).await {
            Ok(handle) => refs::load(&handle).await,
            Err(error) => Err(error),
        };
        match inventory {
            Ok(inventory) => (inventory.tags, None),
            Err(error) => (Vec::new(), Some(state.locale.t(refs_message(error)))),
        }
    };
    let missing_selection = !fields.tag_name.is_empty() && !tags.iter().any(|tag| tag.name == fields.tag_name);
    let page = ReleaseEditor {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(heading)]),
        release_url: format!("/{}/releases", repo.full_name()),
        is_owner: can_admin(state, &repo),
        tab: "releases",
        repo,
        heading: state.locale.t(heading),
        current,
        published,
        fields,
        tags,
        missing_selection,
        tag_notice,
        error: error.map(|(_, message)| state.locale.t(message)),
    };
    Ok(render(&page)?.with_status(error.map_or(200, |(status, _)| status)))
}

async fn tag_error(
    state: &AppState,
    repo: Repo,
    current: Option<Release>,
    fields: ReleaseInput,
    error: TagError,
) -> Result<Response> {
    match error {
        TagError::Missing => release_editor(state, repo, current, fields, Some((422, Text::TagMustPointToCommit))).await,
        TagError::Refs(error) => error_response(&Chrome::new(state), 502, state.locale.t(refs_message(error))),
    }
}

pub async fn new_release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_write(state, &repo).await? {
        return forbidden(state, state.locale.t(Text::ReleaseWriteRequired));
    }
    release_editor(state, repo, None, ReleaseInput::default(), None).await
}

pub async fn create_release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_write(state, &repo).await? {
        return forbidden(state, state.locale.t(Text::ReleaseWriteRequired));
    }
    let fields = ReleaseInput::read(state.form()?);
    if let Some(error) = fields.error() {
        return release_editor(state, repo, None, fields, Some((422, error))).await;
    }
    let commit = match resolve_tag(state, &repo, &fields.tag_name).await {
        Ok(commit) => commit,
        Err(error) => return tag_error(state, repo, None, fields, error).await,
    };
    let [viewer, admin] = viewer_params(state);
    let id = state.db()?.prepare(format!(
        "INSERT INTO repo_releases (repo_id, tag_name, commit_hash, title, notes, created_at)
         SELECT r.id, ?4, ?5, ?6, ?7, ?8 FROM repos r WHERE r.id = ?3 AND {WRITER_SQL}
         ON CONFLICT(repo_id, tag_name) DO NOTHING RETURNING id"
    )).bind(&[
        viewer, admin, js_int(repo.id), js_text(&fields.tag_name), js_text(&commit),
        js_text(fields.title.trim()), js_text(&fields.notes), js_int(now()),
    ])?.first::<i64>(Some("id")).await?;
    match id {
        Some(id) => redirect(&format!("/{}/releases/{id}/edit", repo.full_name())),
        None => release_editor(state, repo, None, fields, Some((409, Text::ReleaseTagUsed))).await,
    }
}

pub async fn edit_release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_write(state, &repo).await? {
        return forbidden(state, state.locale.t(Text::ReleaseWriteRequired));
    }
    let Some(id) = route_id(&ctx) else { return failure(state, 404, Text::MissingRelease) };
    let Some(mut current) = load_release(state, repo.id, id).await? else {
        return failure(state, 404, Text::MissingRelease);
    };
    let fields = ReleaseInput {
        tag_name: std::mem::take(&mut current.tag_name),
        title: std::mem::take(&mut current.title),
        notes: std::mem::take(&mut current.notes),
    };
    release_editor(state, repo, Some(current), fields, None).await
}

pub async fn update_release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_write(state, &repo).await? {
        return forbidden(state, state.locale.t(Text::ReleaseWriteRequired));
    }
    let Some(id) = route_id(&ctx) else { return failure(state, 404, Text::MissingRelease) };
    let Some(current) = load_release(state, repo.id, id).await? else {
        return failure(state, 404, Text::MissingRelease);
    };
    let form = state.form()?;
    let version = form_value(form, "version").and_then(|value| parse_id(&value));
    if version != Some(current.version) {
        return failure(state, 409, Text::ReleaseChanged);
    }
    let mut fields = ReleaseInput::read(form);
    let published = current.published_at.is_some();
    let publish = !published && form_value(form, "action").as_deref() == Some("publish");
    if published {
        fields.tag_name.clone_from(&current.tag_name);
    }
    if let Some(error) = fields.error() {
        return release_editor(state, repo, Some(current), fields, Some((422, error))).await;
    }
    let mut resolved = None;
    if publish || fields.tag_name != current.tag_name {
        match resolve_tag(state, &repo, &fields.tag_name).await {
            Ok(commit) => resolved = Some(commit),
            Err(error) => return tag_error(state, repo, Some(current), fields, error).await,
        }
    }
    let commit = resolved.as_deref().unwrap_or(&current.commit_hash);
    let [viewer, admin] = viewer_params(state);
    let database = state.db()?;
    let updated = database.prepare(format!(
        "UPDATE repo_releases
         SET tag_name = ?6, commit_hash = ?7, title = ?8, notes = ?9, version = version + 1,
             published_at = CASE WHEN ?10 = 1 THEN ?11 ELSE published_at END
         WHERE repo_id = ?3 AND id = ?4 AND version = ?5
           AND EXISTS (SELECT 1 FROM repos r WHERE r.id = ?3 AND {WRITER_SQL})
           AND NOT EXISTS (SELECT 1 FROM repo_releases other
                           WHERE other.repo_id = ?3 AND other.tag_name = ?6 AND other.id <> ?4)
         RETURNING id"
    )).bind(&[
        viewer, admin, js_int(repo.id), js_int(id), js_int(current.version), js_text(&fields.tag_name),
        js_text(commit), js_text(fields.title.trim()), js_text(&fields.notes), js_int(i64::from(publish)), js_int(now()),
    ])?.first::<i64>(Some("id")).await?;
    if updated.is_none() {
        let [viewer, admin] = viewer_params(state);
        let duplicate = database.prepare(format!(
            "SELECT l.id FROM repo_releases l JOIN repos r ON r.id = l.repo_id
             WHERE r.id = ?3 AND l.tag_name = ?4 AND l.id <> ?5 AND {WRITER_SQL}"
        )).bind(&[viewer, admin, js_int(repo.id), js_text(&fields.tag_name), js_int(id)])?.first::<i64>(Some("id")).await?;
        return if duplicate.is_some() {
            release_editor(state, repo, Some(current), fields, Some((409, Text::ReleaseTagUsed))).await
        } else {
            failure(state, 409, Text::ReleaseChanged)
        };
    }
    redirect(&format!("/{}/releases/{id}", repo.full_name()))
}

pub async fn delete_release(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_write(state, &repo).await? {
        return forbidden(state, state.locale.t(Text::ReleaseWriteRequired));
    }
    let Some(id) = route_id(&ctx) else { return failure(state, 404, Text::MissingRelease) };
    let form = state.form()?;
    let Some(version) = form_value(form, "version").and_then(|value| parse_id(&value)) else {
        return failure(state, 409, Text::ReleaseChanged);
    };
    let [viewer, admin] = viewer_params(state);
    let removed = state.db()?.prepare(format!(
        "DELETE FROM repo_releases WHERE repo_id = ?3 AND id = ?4 AND version = ?5
         AND EXISTS (SELECT 1 FROM repos r WHERE r.id = ?3 AND {WRITER_SQL}) RETURNING id"
    )).bind(&[viewer, admin, js_int(repo.id), js_int(id), js_int(version)])?.first::<i64>(Some("id")).await?;
    if removed.is_none() {
        return failure(state, 409, Text::ReleaseChanged);
    }
    redirect(&format!("/{}/releases", repo.full_name()))
}

#[derive(Deserialize)]
pub struct IssueTemplateChoice {
    pub id: i64,
    pub name: String,
}

impl IssueTemplateChoice {
    pub fn is_selected(&self, selected: Option<&i64>) -> bool {
        selected == Some(&self.id)
    }
}

#[derive(Deserialize)]
pub struct IssueTemplateDefaults {
    pub title: String,
    pub body: String,
}

pub async fn issue_template_choices(state: &AppState, repo_id: i64) -> Result<Vec<IssueTemplateChoice>> {
    db::all(
        &state.db()?,
        "SELECT id, name FROM issue_templates WHERE repo_id = ?1 ORDER BY name, id",
        &[js_int(repo_id)],
    ).await
}

pub async fn issue_template_defaults(state: &AppState, repo_id: i64, id: i64) -> Result<Option<IssueTemplateDefaults>> {
    db::first(
        &state.db()?,
        "SELECT title, body FROM issue_templates WHERE repo_id = ?1 AND id = ?2",
        &[js_int(repo_id), js_int(id)],
    ).await
}

#[derive(Default, Deserialize)]
struct IssueTemplateEditor {
    id: Option<i64>,
    version: i64,
    name: String,
    title: String,
    body: String,
}

impl IssueTemplateEditor {
    fn error(&self) -> Option<Text> {
        let name = self.name.trim();
        let title = self.title.trim();
        if name.is_empty() || name.chars().count() > MAX_TEMPLATE_NAME_CHARS || name.chars().any(char::is_control) {
            Some(Text::InvalidIssueTemplateName)
        } else if title.chars().count() > MAX_TITLE_CHARS || title.chars().any(char::is_control) {
            Some(Text::InvalidIssueTemplateTitle)
        } else if self.body.len() > MAX_TEXT_BYTES {
            Some(Text::IssueTemplateBodyTooLong)
        } else {
            None
        }
    }
}

#[derive(Template)]
#[template(path = "issue_templates.html")]
struct IssueTemplatesPage {
    chrome: Chrome,
    title: String,
    settings_url: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    choices: Vec<IssueTemplateChoice>,
    editor: Option<IssueTemplateEditor>,
    error: Option<&'static str>,
}

async fn issue_templates_page(
    state: &AppState,
    repo: Repo,
    editor: Option<IssueTemplateEditor>,
    error: Option<(u16, Text)>,
) -> Result<Response> {
    let choices = if editor.is_none() { issue_template_choices(state, repo.id).await? } else { Vec::new() };
    Ok(render(&IssueTemplatesPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(Text::IssueTemplates)]),
        settings_url: format!("/{}/settings", repo.full_name()),
        tab: "settings",
        is_owner: true,
        repo,
        choices,
        editor,
        error: error.map(|(_, message)| state.locale.t(message)),
    })?.with_status(error.map_or(200, |(status, _)| status)))
}

pub async fn issue_templates(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let editor = match query_param(&req, "edit").as_deref() {
        Some("new") => Some(IssueTemplateEditor::default()),
        Some(value) => {
            let Some(id) = parse_id(value) else { return failure(state, 404, Text::MissingIssueTemplate) };
            let editor = db::first(
                &state.db()?, "SELECT id, version, name, title, body FROM issue_templates WHERE repo_id = ?1 AND id = ?2",
                &[js_int(repo.id), js_int(id)],
            ).await?;
            if editor.is_none() {
                return failure(state, 404, Text::MissingIssueTemplate);
            }
            editor
        }
        None => None,
    };
    issue_templates_page(state, repo, editor, None).await
}

pub async fn save_issue_template(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let form = state.form()?;
    let id = match form_value(form, "id") {
        Some(value) => match parse_id(&value) {
            Some(id) => Some(id),
            None => return failure(state, 404, Text::MissingIssueTemplate),
        },
        None => None,
    };
    let editor = IssueTemplateEditor {
        id,
        version: form_value(form, "version").and_then(|value| parse_id(&value)).unwrap_or_default(),
        name: form_value(form, "name").unwrap_or_default(),
        title: form_value(form, "title").unwrap_or_default(),
        body: form_value(form, "body").unwrap_or_default(),
    };
    if let Some(error) = editor.error() {
        return issue_templates_page(state, repo, Some(editor), Some((422, error))).await;
    }
    let [viewer, admin] = viewer_params(state);
    let database = state.db()?;
    let mut args = vec![viewer, admin, js_int(repo.id), js_text(editor.name.trim()), js_text(editor.title.trim()), js_text(&editor.body)];
    let sql = if let Some(id) = editor.id {
        args.extend([js_int(id), js_int(editor.version)]);
        format!(
            "UPDATE issue_templates SET name = ?4, title = ?5, body = ?6, version = version + 1
             WHERE repo_id = ?3 AND id = ?7 AND version = ?8
               AND EXISTS (SELECT 1 FROM repos r WHERE r.id = ?3 AND {ADMIN_SQL})
               AND NOT EXISTS (SELECT 1 FROM issue_templates other WHERE other.repo_id = ?3 AND other.name = ?4 AND other.id <> ?7)
             RETURNING id"
        )
    } else {
        format!(
            "INSERT INTO issue_templates (repo_id, name, title, body)
             SELECT r.id, ?4, ?5, ?6 FROM repos r WHERE r.id = ?3 AND {ADMIN_SQL}
             ON CONFLICT(repo_id, name) DO NOTHING RETURNING id"
        )
    };
    let saved = database.prepare(&sql).bind(&args)?.first::<i64>(Some("id")).await?;
    if saved.is_none() {
        let [viewer, admin] = viewer_params(state);
        let duplicate = database.prepare(format!(
            "SELECT t.id FROM issue_templates t JOIN repos r ON r.id = t.repo_id
             WHERE r.id = ?3 AND t.name = ?4 AND t.id IS NOT ?5 AND {ADMIN_SQL}"
        )).bind(&[viewer, admin, js_int(repo.id), js_text(editor.name.trim()), editor.id.map_or_else(crate::util::js_null, js_int)])?
            .first::<i64>(Some("id")).await?;
        return if duplicate.is_some() {
            issue_templates_page(state, repo, Some(editor), Some((409, Text::IssueTemplateNameUsed))).await
        } else {
            failure(state, 409, Text::IssueTemplateChanged)
        };
    }
    redirect(&format!("/{}/settings/issue-templates", repo.full_name()))
}

pub async fn delete_issue_template(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    if !can_admin(state, &repo) {
        return forbidden(state, state.locale.t(RepoText::SettingsForbidden));
    }
    let Some(id) = route_id(&ctx) else { return failure(state, 404, Text::MissingIssueTemplate) };
    let form = state.form()?;
    let Some(version) = form_value(form, "version").and_then(|value| parse_id(&value)) else {
        return failure(state, 409, Text::IssueTemplateChanged);
    };
    let [viewer, admin] = viewer_params(state);
    let removed = state.db()?.prepare(format!(
        "DELETE FROM issue_templates WHERE repo_id = ?3 AND id = ?4 AND version = ?5
         AND EXISTS (SELECT 1 FROM repos r WHERE r.id = ?3 AND {ADMIN_SQL}) RETURNING id"
    )).bind(&[viewer, admin, js_int(repo.id), js_int(id), js_int(version)])?.first::<i64>(Some("id")).await?;
    if removed.is_none() {
        return failure(state, 409, Text::IssueTemplateChanged);
    }
    redirect(&format!("/{}/settings/issue-templates", repo.full_name()))
}

pub struct Reaction {
    pub(crate) kind: &'static str,
    label: Text,
    pub(crate) count: i64,
    pub(crate) active: bool,
}

impl Reaction {
    pub(crate) fn label(&self, locale: &Locale) -> &'static str {
        locale.t(self.label)
    }
}

pub async fn reactions(state: &AppState, issue_id: i64) -> Result<Vec<Reaction>> {
    #[derive(Deserialize)]
    struct Count {
        reaction: String,
        total: i64,
        active: i64,
    }
    let counts: Vec<Count> = db::all(
        &state.db()?,
        "SELECT reaction, COUNT(*) AS total, COALESCE(MAX(user_id = ?2), 0) AS active
         FROM issue_reactions WHERE issue_id = ?1 GROUP BY reaction",
        &[js_int(issue_id), state.user_id().map_or_else(crate::util::js_null, js_int)],
    ).await?;
    Ok([("up", Text::Upvote), ("down", Text::Downvote)].into_iter().map(|(kind, label)| {
        let count = counts.iter().find(|count| count.reaction == kind);
        Reaction { kind, label, count: count.map_or(0, |count| count.total), active: count.is_some_and(|count| count.active == 1) }
    }).collect())
}

fn active(form: &FormData) -> Option<bool> {
    match form_value(form, "active").as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

pub async fn react(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    let missing = || not_found(state, state.locale.t(CommonText::NotFound));
    let Some(number) = ctx.param("number").and_then(|number| parse_id(number)) else { return missing() };
    let path = format!("/{}/issues/{number}", repo.full_name());
    if state.user.is_none() {
        return redirect(&format!("/user/login?next={path}"));
    }
    let form = state.form()?;
    let kind = form_value(form, "reaction").unwrap_or_default();
    let Some(enabled) = active(form).filter(|_| matches!(kind.as_str(), "up" | "down")) else {
        return failure(state, 400, Text::InvalidReaction);
    };
    let [viewer, admin] = viewer_params(state);
    let eligible = format!(
        "SELECT i.id FROM issues i JOIN repos r ON r.id = i.repo_id
         WHERE r.id = ?3 AND i.number = ?4 AND i.kind = 'issue' AND {VISIBLE_REPO_SQL}"
    );
    let database = state.db()?;
    let args = [viewer, admin, js_int(repo.id), js_int(number), js_text(&kind)];
    if database.prepare(&eligible).bind(&args[..4])?.first::<i64>(Some("id")).await?.is_none() {
        return missing();
    }
    let sql = if enabled {
        format!(
            "INSERT INTO issue_reactions (issue_id, reaction, user_id)
             SELECT i.id, ?5, ?1 FROM ({eligible}) i
             WHERE true ON CONFLICT(issue_id, reaction, user_id) DO NOTHING"
        )
    } else {
        format!("DELETE FROM issue_reactions WHERE issue_id = ({eligible}) AND reaction = ?5 AND user_id = ?1")
    };
    db::run(&database, &sql, &args).await?;
    let page = PageRequest::new(&req, "comments_page").number;
    redirect(&format!("{path}?comments_page={page}#reactions"))
}

pub async fn watch(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    set_repo_preference(req, ctx, false).await
}

pub async fn star(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    set_repo_preference(req, ctx, true).await
}

async fn set_repo_preference(req: Request, ctx: RouteContext<AppState>, star: bool) -> Result<Response> {
    let state = &ctx.data;
    let Some(repo) = repository(&ctx).await? else { return repo_missing(state) };
    let path = format!("/{}", repo.full_name());
    if state.user.is_none() {
        return redirect(&format!("/user/login?next={path}"));
    }
    let Some(enabled) = active(state.form()?) else {
        return error_response(&Chrome::new(state), 400, state.locale.t(CommonText::UnreadableForm));
    };
    let [viewer, admin] = viewer_params(state);
    let mut args = vec![viewer, admin, js_int(repo.id)];
    let sql = match (star, enabled) {
        (true, true) => {
            args.push(js_int(now()));
            format!(
                "INSERT INTO repo_stars (user_id, repo_id, created_at)
                 SELECT ?1, r.id, ?4 FROM repos r WHERE r.id = ?3 AND {VISIBLE_REPO_SQL}
                 ON CONFLICT(user_id, repo_id) DO NOTHING"
            )
        }
        (false, true) => format!(
            "INSERT INTO repo_watches (repo_id, user_id)
             SELECT r.id, ?1 FROM repos r WHERE r.id = ?3 AND {VISIBLE_REPO_SQL}
             ON CONFLICT(repo_id, user_id) DO NOTHING"
        ),
        (_, false) => {
            let table = if star { "repo_stars" } else { "repo_watches" };
            format!("DELETE FROM {table} WHERE user_id = ?1 AND repo_id = ?3
                     AND EXISTS (SELECT 1 FROM repos r WHERE r.id = ?3 AND {VISIBLE_REPO_SQL})")
        }
    };
    db::run(&state.db()?, &sql, &args).await?;
    redirect_to_repo_page(&req, &path)
}

fn redirect_to_repo_page(req: &Request, path: &str) -> Result<Response> {
    let request_url = req.url()?;
    if let Some(source) = crate::util::header(req, "referer").and_then(|url| Url::parse(&url).ok()) {
        let source_path = source.path();
        let within_repo = source_path == path || source_path.strip_prefix(path).is_some_and(|suffix| suffix.starts_with('/'));
        if source.origin() == request_url.origin() && within_repo && source_path != request_url.path() {
            return if let Some(query) = source.query() {
                redirect(&format!("{source_path}?{query}"))
            } else {
                redirect(source_path)
            };
        }
    }
    redirect(path)
}

#[derive(Template)]
#[template(path = "starred.html")]
struct StarredPage {
    chrome: Chrome,
    title: String,
    repos: Vec<RepoRow>,
    pagination: Pagination,
}

pub async fn starred(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        return redirect("/user/login?next=/user/stars");
    }
    let [viewer, admin] = viewer_params(state);
    let request = PageRequest::new(&req, "page");
    let mut repos: Vec<Repo> = db::all(
        &state.db()?,
        &format!(
            "SELECT {REPO_COLUMNS} FROM repo_stars s JOIN repos r ON r.id = s.repo_id JOIN users u ON u.id = r.owner_id
             WHERE s.user_id = ?1 AND {VISIBLE_REPO_SQL}
             ORDER BY s.created_at DESC, s.repo_id DESC LIMIT ?3 OFFSET ?4"
        ),
        &[viewer, admin, js_int((PAGE_SIZE + 1) as i64), js_int(request.offset())],
    ).await?;
    let pagination = request.finish(&req, &mut repos, None)?;
    render(&StarredPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::StarredRepositories)]),
        repos: repos.iter().map(RepoRow::from_repo).collect(),
        pagination,
    })
}
