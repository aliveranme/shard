use super::common::{can_admin, not_found, page_title, visible_repo};
use crate::git_storage::{is_object_id, Commit, GitBackend, GitStorage, RepoHandle, TreeEntry};
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::i18n::{CommonText, RepoText};
use crate::markdown;
use crate::models::{Repo, Ts};
use crate::native_git::WriteError;
use crate::pagination::{query_url, Pagination, PAGE_SIZE};
use crate::refs::{self, Inventory, RefsError, RefsPage};
use crate::templates::{
    render, Avatar, Chrome, CommitRow, CommitsPage, Crumb, FilePage, FileRow, NewRepoPage, RepoCodePage,
    SettingsPage,
};
use crate::util;
use std::borrow::Cow;
use worker::*;

const PREVIEW_BYTES: usize = 256 * 1024;
const HEAD_UPDATE_LEASE_SECONDS: i64 = 300;

pub async fn new_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_none() {
        return util::redirect("/user/login?next=/new");
    }
    let page = NewRepoPage {
        chrome: Chrome::new(&state),
        title: page_title(&[state.locale.t(CommonText::NewRepository)]),
        error: None,
        name_value: String::new(),
        description_value: String::new(),
        private_checked: false,
    };
    render(&page)
}

pub async fn create(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/new");
    };
    let form = state.form()?;
    let name = util::form_value(form, "name")
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = util::form_value(form, "description").unwrap_or_default();
    let private = util::form_bool(form, "private");

    let invalid = |message: RepoText| -> Result<Response> {
        let page = NewRepoPage {
            chrome: Chrome::new(&state),
            title: page_title(&[state.locale.t(CommonText::NewRepository)]),
            error: Some(state.locale.t(message).into()),
            name_value: name.clone(),
            description_value: description.clone(),
            private_checked: private,
        };
        render(&page)
    };

    if !util::valid_slug(&name) {
        return invalid(RepoText::InvalidName);
    }

    let database = state.db()?;
    #[derive(serde::Deserialize)]
    struct ReservedPath { taken: i64 }
    let reserved = db::first::<ReservedPath>(&database,
        "SELECT 1 AS taken FROM repo_paths WHERE owner_id = ? AND name = ? COLLATE NOCASE",
        &[util::js_int(user.id), util::js_text(&name)],
    ).await?;
    if reserved.is_some_and(|path| path.taken == 1) {
        return invalid(RepoText::DuplicateName);
    }

    let storage = GitBackend::selected(&state.env)
        .and_then(|backend| Ok((backend, GitStorage::from_env(&state.env, backend)?)));
    let (backend, storage) = match storage {
        Ok(storage) => storage,
        Err(error) => return invalid(log_failure("initialize repository storage", error, RepoText::CreateFailed)),
    };
    // Storage identity stays stable across repository renames and transfers.
    let internal = format!("shard-{}", &crypto::random_token()?[..32]);
    let created = match storage.create(&internal, &description).await {
        Ok(created) => created,
        Err(error) => return invalid(log_failure("create repository storage", error, RepoText::CreateFailed)),
    };

    let inserted = db::run(
        &database,
        "INSERT INTO repos (owner_id, name, description, default_branch, artifacts_repo, artifacts_remote, visibility, created_at, updated_at, git_backend)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            util::js_int(user.id),
            util::js_text(&name),
            util::js_text(&description),
            util::js_text(&created.default_branch),
            util::js_text(&created.name),
            util::js_text(&created.remote),
            util::js_text(if private { "private" } else { "public" }),
            util::js_int(util::now()),
            util::js_int(util::now()),
            util::js_text(backend.name()),
        ],
    )
    .await;

    if let Err(error) = inserted {
        if let Err(cleanup_error) = storage.delete(&created.name).await {
            console_log!("repository rollback failed: {cleanup_error:?}");
        }
        return invalid(log_failure("insert repository", error, RepoText::CreateFailed));
    }

    util::redirect(&format!("/{}/{}", user.username, name))
}

fn log_failure(operation: &str, error: Error, message: RepoText) -> RepoText {
    console_log!("repository: {operation}: {error}");
    message
}

fn repository_not_found(state: &AppState) -> Result<Response> {
    not_found(state, state.locale.t(CommonText::RepositoryNotFound))
}

fn storage_error(state: &AppState, operation: &str, error: Error) -> Result<Response> {
    let message = log_failure(operation, error, RepoText::StorageUnavailable);
    crate::templates::error_response(&Chrome::new(state), 500, state.locale.t(message))
}

fn branch_choices(default: &str, current: &str) -> Vec<String> {
    if default == current {
        vec![default.into()]
    } else {
        vec![default.into(), current.into()]
    }
}

fn requested_path(ctx: &RouteContext<AppState>) -> Option<Cow<'_, str>> {
    // Worker routing leaves wildcard path parameters percent-encoded.
    urlencoding::decode(ctx.param("path").map_or("", String::as_str)).ok()
}

pub async fn view(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    tree(req, ctx).await
}

pub async fn references(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let owner = ctx.param("owner").map(String::as_str).unwrap_or_default();
    let name = ctx.param("repo").map(String::as_str).unwrap_or_default();
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return repository_not_found(state);
    };
    let loaded = match RepoHandle::open(&state.env, &repo).await {
        Ok(handle) => refs::load(&handle).await,
        Err(_) => Err(RefsError::Unavailable),
    };
    let (inventory, notice, status) = match loaded {
        Ok(inventory) => (inventory, None, 200),
        Err(RefsError::Limit) => (Inventory::default(), Some(RepoText::RefsLimit), 200),
        Err(RefsError::Unavailable) => {
            (Inventory::default(), Some(RepoText::StorageUnavailable), 502)
        }
    };
    let page = RefsPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(RepoText::BranchesAndTags)]),
        is_owner: can_admin(state, &repo),
        repo,
        tab: "refs",
        inventory,
        notice: notice.map(|key| state.locale.t(key)),
    };
    Ok(render(&page)?.with_status(status))
}

pub async fn tree(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let Some(path) = requested_path(&ctx) else {
        return not_found(&state, state.locale.t(RepoText::DirectoryNotFound));
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let branch_url = util::url_path(&branch);
    let is_owner = can_admin(&state, &repo);
    let branches = branch_choices(&repo.default_branch, &branch);

    let loaded = async {
        let handle = RepoHandle::open(&state.env, &repo).await?;
        let Some(entry) = load_browse_entry(&handle, &branch, &path).await? else {
            return Ok(None);
        };
        let readme = match &entry {
            BrowseEntry::Directory(_, Some(commit)) if path.is_empty() => {
                load_readme(&handle, &repo, &commit.hash).await
            }
            _ => None,
        };
        Ok(Some((entry, readme)))
    }
    .await;
    let (files, last_commit, readme, storage_notice) = match loaded {
        Ok(Some((BrowseEntry::Directory(entries, commit), readme))) => {
            let last_commit = if let Some(commit) = commit {
                let avatars = crate::avatar::git_author_ids(&state, commit.author_email.iter().map(String::as_str)).await?;
                let id = commit.author_email.as_ref().and_then(|email| avatars.get(email)).copied();
                Some(commit_row(commit, id))
            } else {
                None
            };
            (entries.into_iter().map(|entry| file_row_at(entry, &path)).collect(), last_commit, readme, None)
        }
        Ok(Some((BrowseEntry::File, _))) => {
            let mut location = format!("/{}/{}/blob/{}", repo.owner_name, repo.name, util::url_path(&path));
            if let Some(query) = req.url()?.query() {
                location.push('?');
                location.push_str(query);
            }
            return util::redirect(&location);
        }
        Ok(None) => return not_found(&state, state.locale.t(RepoText::DirectoryNotFound)),
        Err(error) => {
            let message = log_failure("load repository directory", error, RepoText::StorageUnavailable);
            (Vec::new(), None, None, Some(state.locale.t(message).into()))
        }
    };
    let title = if path.is_empty() {
        page_title(&[&repo.full_name()])
    } else {
        page_title(&[&repo.full_name(), &path])
    };
    let direct_origin = match repo.git_backend {
        GitBackend::Native => super::git::direct_origin(&state.env),
        _ => None,
    };
    let clone_origin = direct_origin.unwrap_or_else(|| util::base_url(&req));
    let clone_url = format!("{clone_origin}/{}/{}.git", repo.owner_name, repo.name);

    let show_sizes = files.iter().any(|file| !file.size.is_empty());
    let page = RepoCodePage {
        chrome: Chrome::new(&state),
        title,
        repo,
        tab: "code".into(),
        branch,
        branch_url,
        branches,
        crumbs: crumbs(&path),
        files,
        show_sizes,
        readme,
        last_commit,
        storage_notice,
        clone_url,
        is_owner,
    };
    render(&page)
}

pub async fn file(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let Some(path) = requested_path(&ctx) else {
        return not_found(&state, state.locale.t(RepoText::FileNotFound));
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let branch_url = util::url_path(&branch);
    let is_owner = can_admin(&state, &repo);
    let is_image = image_mime(&path).is_some();
    let is_markdown = extension(&path).is_some_and(|extension| {
        ["md", "markdown", "mdown", "mkd", "mkdn"].iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    });
    let preview = async {
        let handle = RepoHandle::open(&state.env, &repo).await?;
        let commit = if (is_image || is_markdown) && !is_object_id(&branch) {
            handle.head_commit(&branch).await?.map(|commit| commit.hash)
        } else {
            None
        };
        let reference = commit.as_deref().unwrap_or(&branch);
        let limit = if is_image { 1 } else { PREVIEW_BYTES };
        let Some(file) = handle.read_file(reference, &path, Some(limit)).await? else {
            return Ok(None);
        };
        let size = file.size();
        let bytes = if is_image {
            file.discard();
            Vec::new()
        } else {
            file.bytes(limit).await?
        };
        Ok(Some((bytes, size, commit)))
    }.await;
    let (bytes, size, commit) = match preview {
        Ok(Some(preview)) => preview,
        Ok(None) => return not_found(&state, state.locale.t(RepoText::FileNotFound)),
        Err(error) => return storage_error(&state, "read file preview", error),
    };
    let is_binary = bytes.iter().take(8000).any(|byte| *byte == 0);
    let context = markdown::Context::new(&repo, commit.as_deref().unwrap_or(&branch), &path);
    let image_url = is_image.then(|| context.image_url(&path));
    let (content, markdown_html) = if is_image || is_binary {
        (String::new(), None)
    } else {
        let text = String::from_utf8_lossy(&bytes);
        if is_markdown {
            (String::new(), Some(markdown::render_document(&text, &context)))
        } else {
            (text.into_owned(), None)
        }
    };
    let highlighted = (!content.is_empty()).then(|| crate::highlight::render(&path, &content)).flatten();
    let page = FilePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), &path]),
        repo,
        tab: "code".into(),
        branch_url,
        path_url: util::url_path(&path),
        crumbs: crumbs(&path),
        content,
        highlighted,
        markdown_html,
        image_url,
        is_binary,
        truncated: size > PREVIEW_BYTES as u64 && !is_image && !is_binary,
        size: util::human_size(size),
        is_owner,
    };
    render(&page)
}

pub async fn raw(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    file_response(req, ctx, false).await
}

pub async fn media(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    file_response(req, ctx, true).await
}

async fn file_response(req: Request, ctx: RouteContext<AppState>, inline_image: bool) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let Some(path) = requested_path(&ctx) else {
        return not_found(&state, state.locale.t(RepoText::FileNotFound));
    };
    let mime = if inline_image {
        let Some(mime) = image_mime(&path) else {
            return not_found(&state, state.locale.t(RepoText::FileNotFound));
        };
        if path.contains('\\') || path.chars().any(char::is_control)
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return not_found(&state, state.locale.t(RepoText::FileNotFound));
        }
        mime
    } else {
        "application/octet-stream"
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let file = async {
        RepoHandle::open(&state.env, &repo).await?.read_file(&branch, &path, None).await
    }.await;
    let file = match file {
        Ok(Some(file)) => file,
        Ok(None) => return not_found(&state, state.locale.t(RepoText::FileNotFound)),
        Err(error) => return storage_error(&state, "stream repository file", error),
    };
    let headers = Headers::new();
    headers.set("Content-Type", mime)?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    if inline_image {
        headers.set("Content-Disposition", "inline")?;
        headers.set("Cache-Control", if repo.is_private() {
            "private, max-age=86400"
        } else {
            "public, max-age=86400, stale-while-revalidate=604800"
        })?;
        headers.set("Cross-Origin-Resource-Policy", "same-origin")?;
        headers.set("Referrer-Policy", "no-referrer")?;
        headers.set("Content-Security-Policy", "sandbox; default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'")?;
        return Ok(file.into_response()?.with_headers(headers));
    }
    let filename = path.rsplit('/').next().unwrap_or("file");
    let disposition = if filename.bytes().all(|b| (b' '..=b'~').contains(&b) && !matches!(b, b'"' | b'\\' | b'%')) {
        format!("inline; filename=\"{filename}\"")
    } else {
        format!("inline; filename*=UTF-8''{}", urlencoding::encode(filename))
    };
    headers.set("Content-Disposition", &disposition)?;
    Ok(file.into_response()?.with_headers(headers))
}

fn extension(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.rsplit_once('.').map(|(_, extension)| extension)
}

fn image_mime(path: &str) -> Option<&'static str> {
    let extension = extension(path)?;
    [
        ("png", "image/png"),
        ("apng", "image/apng"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("webp", "image/webp"),
        ("avif", "image/avif"),
        ("bmp", "image/bmp"),
        ("ico", "image/x-icon"),
        ("svg", "image/svg+xml"),
    ].into_iter().find_map(|(suffix, mime)| extension.eq_ignore_ascii_case(suffix).then_some(mime))
}

pub async fn commits(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let branch = util::query_param(&req, "branch").unwrap_or_else(|| repo.default_branch.clone());
    let start = util::query_param(&req, "start").map(|hash| hash.to_ascii_lowercase());
    if start.as_ref().is_some_and(|hash| hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return crate::templates::error_response(&Chrome::new(&state), 400, state.locale.t(RepoText::InvalidCursor));
    }
    let is_owner = can_admin(&state, &repo);
    let handle = match RepoHandle::open(&state.env, &repo).await {
        Ok(handle) => handle,
        Err(error) => return storage_error(&state, "open commit history", error),
    };
    // The Git backend accepts a commit hash as its ref and walks first parents.
    // The extra commit is the inclusive start of the next page, so every call
    // reads at most 21 commits regardless of how far back the history goes.
    let mut history = match handle.log(start.as_deref().unwrap_or(&branch), (PAGE_SIZE + 1) as u32).await {
        Ok(history) => history,
        Err(error) => return storage_error(&state, "load commit history", error),
    };
    let status = if start.is_some() && history.is_empty() { 404 } else { 200 };
    let next = if history.len() > PAGE_SIZE { history.pop().map(|commit| commit.hash) } else { None };
    let pagination = Pagination {
        page: 1,
        status: Some(state.locale.t(if start.is_some() { CommonText::OlderCommits } else { CommonText::LatestCommits })),
        previous: None,
        next: match next {
            Some(hash) => Some(query_url(&req, &[("start", Some(&hash)), ("branch", Some(&branch)), ("page", None)], None)?),
            None => None,
        },
    };
    let latest_url = if start.is_some() {
        Some(query_url(&req, &[("start", None), ("branch", Some(&branch)), ("page", None)], None)?)
    } else {
        None
    };
    let avatars = crate::avatar::git_author_ids(&state, history.iter().filter_map(|commit| commit.author_email.as_deref())).await?;
    let page = CommitsPage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), state.locale.t(CommonText::Commits)]),
        repo,
        tab: "commits".into(),
        branch,
        commits: history.into_iter().map(|commit| {
            let id = commit.author_email.as_ref().and_then(|email| avatars.get(email)).copied();
            commit_row(commit, id)
        }).collect(),
        is_owner,
        pagination,
        latest_url,
    };
    Ok(render(&page)?.with_status(status))
}

pub async fn settings(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, state.locale.t(RepoText::SettingsForbidden));
    }
    let page = SettingsPage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), state.locale.t(CommonText::Settings)]),
        description_value: repo.description.clone(),
        visibility_value: repo.visibility.clone(),
        repo,
        tab: "settings".into(),
        error: match util::query_param(&req, "error").as_deref() {
            Some("confirm-deletion") => Some(RepoText::ConfirmDeletion),
            Some("delete-storage-failed") => Some(RepoText::DeleteStorageFailed),
            Some("delete-failed") => Some(RepoText::DeleteFailed),
            Some("save-failed") => Some(RepoText::SaveFailed),
            Some("settings-changed") => Some(RepoText::SettingsChanged),
            Some("branch-not-found") => Some(RepoText::DefaultBranchNotFound),
            Some("branch-check-limit") => Some(RepoText::RefsLimit),
            Some("branch-check-unavailable") => Some(RepoText::StorageUnavailable),
            Some("git-head-unavailable") => Some(RepoText::GitHeadUnavailable),
            _ => None,
        }.map(|key| state.locale.t(key).into()),
        message: util::query_param(&req, "saved").filter(|value| value == "1"),
        is_owner: true,
    };
    render(&page)
}

pub async fn update_settings(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let Some(user_id) = state.user_id().filter(|_| can_admin(&state, &repo)) else {
        return super::common::forbidden(&state, state.locale.t(RepoText::SettingsForbidden));
    };
    let form = state.form()?;
    let description = util::form_value(form, "description").unwrap_or_default();
    let visibility = match util::form_value(form, "visibility").as_deref() {
        Some("private") => "private",
        _ => "public",
    };
    let default_branch = util::form_value(form, "default_branch")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| repo.default_branch.clone());
    let sync_head = default_branch != repo.default_branch || util::form_value(form, "sync_git_head").as_deref() == Some("1");
    let settings_url = format!("/{}/settings", repo.full_name());
    let database = state.db()?;
    let repo_id = util::js_int(repo.id);
    let actor_id = util::js_int(user_id);
    let saved_branch = util::js_text(&repo.default_branch);
    let mut lease = None;
    let mut override_head = None;
    if sync_head {
        let inventory = async {
            let handle = RepoHandle::open(&state.env, &repo).await.map_err(|_| RefsError::Unavailable)?;
            let refs = refs::load(&handle).await?;
            Ok::<_, RefsError>((handle, refs))
        }.await;
        let handle = match inventory {
            Ok((handle, refs)) if refs.branches.iter().any(|branch| branch.name == default_branch) => handle,
            result => {
                let error = match result {
                    Ok(_) => "branch-not-found",
                    Err(RefsError::Limit) => "branch-check-limit",
                    Err(RefsError::Unavailable) => "branch-check-unavailable",
                };
                return util::redirect(&format!("{settings_url}?error={error}"));
            }
        };
        if matches!(&handle, RepoHandle::Service(_)) {
            // Reserve before the storage write. Transfer and other settings saves
            // wait for this lease, and an expired attempt cannot publish settings.
            let claimed = database.prepare(
                "UPDATE repos SET git_head_lease_until = unixepoch() + ?4, git_head_lease_id = git_head_lease_id + 1
                  WHERE id = ?1 AND default_branch = ?3 AND git_head_lease_until <= unixepoch()
                    AND EXISTS (SELECT 1 FROM users WHERE id = ?2 AND (id = repos.owner_id OR is_admin = 1))
                  RETURNING git_head_lease_id",
            ).bind(&[repo_id.clone(), actor_id.clone(), saved_branch.clone(), util::js_int(HEAD_UPDATE_LEASE_SECONDS)])?
                .first::<i64>(Some("git_head_lease_id")).await?;
            let Some(claimed) = claimed else {
                return util::redirect(&format!("{settings_url}?error=settings-changed"));
            };
            lease = Some(claimed);
        }
        match handle.set_default_branch(&default_branch).await {
            Ok(advertise) => override_head = Some(i64::from(advertise)),
            Err(error) => {
                // A lost response may still commit HEAD. Keep the lease until its
                // deadline when storage cannot confirm that it rejected the write.
                if let Some(lease) = lease.filter(|_| error != WriteError::Unavailable) {
                    db::run(&database,
                        "UPDATE repos SET git_head_lease_until = 0 WHERE id = ?1 AND git_head_lease_id = ?2",
                        &[repo_id, util::js_int(lease)],
                    ).await?;
                }
                return util::redirect(&format!("{settings_url}?error=git-head-unavailable"));
            }
        }
    }

    let updated = database.prepare(
        "UPDATE repos SET description = ?1, visibility = ?2, default_branch = ?3,
             git_head_override = COALESCE(?4, git_head_override), updated_at = unixepoch(), git_head_lease_until = 0
          WHERE id = ?5 AND default_branch = ?6
            AND EXISTS (SELECT 1 FROM users WHERE id = ?7 AND (id = repos.owner_id OR is_admin = 1))
            AND CASE WHEN ?8 IS NULL THEN git_head_lease_until <= unixepoch()
                     ELSE git_head_lease_id = ?8 AND git_head_lease_until > unixepoch() END
          RETURNING id",
    ).bind(&[
        util::js_text(&description), util::js_text(visibility), util::js_text(&default_branch),
        override_head.map_or_else(util::js_null, util::js_int), repo_id, saved_branch, actor_id,
        lease.map_or_else(util::js_null, util::js_int),
    ])?.first::<i64>(Some("id")).await;
    match updated {
        Ok(Some(_)) => util::redirect(&format!("{settings_url}?saved=1")),
        Ok(None) => util::redirect(&format!("{settings_url}?error=settings-changed")),
        Err(error) => {
            console_log!("repository: save settings: {error}");
            util::redirect(&format!("{settings_url}?error=save-failed"))
        }
    }
}

pub async fn delete(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    let Some(user_id) = state.user_id().filter(|_| can_admin(&state, &repo)) else {
        return super::common::forbidden(&state, state.locale.t(RepoText::DeleteForbidden));
    };
    let form = state.form()?;
    let confirmation = util::form_value(form, "confirm").unwrap_or_default();
    let settings_url = format!("/{}/{}/settings", repo.owner_name, repo.name);
    if confirmation != repo.name {
        return util::redirect(&format!("{settings_url}?error=confirm-deletion"));
    }
    #[derive(serde::Deserialize)]
    struct Deleted {
        id: i64,
    }
    let deleted = async {
        let database = state.db()?;
        // The delete trigger records the Git identity and LFS namespace in the same transaction.
        db::first::<Deleted>(
            &database,
            "DELETE FROM repos WHERE id = ?1 AND name = ?2 COLLATE BINARY
               AND EXISTS (SELECT 1 FROM users WHERE id = ?3 AND (id = repos.owner_id OR is_admin = 1))
             RETURNING id",
            &[util::js_int(repo.id), util::js_text(&confirmation), util::js_int(user_id)],
        ).await
    }.await;
    match deleted {
        Ok(Some(deleted)) if deleted.id == repo.id => util::redirect("/"),
        Ok(_) => repository_not_found(&state),
        Err(error) => {
            console_log!("repository: delete metadata: {error}");
            util::redirect(&format!("{settings_url}?error=delete-failed"))
        }
    }
}

enum BrowseEntry {
    Directory(Vec<TreeEntry>, Option<Commit>),
    File,
}

async fn load_browse_entry(handle: &RepoHandle, branch: &str, path: &str) -> Result<Option<BrowseEntry>> {
    let commit = handle.head_commit(branch).await?;
    let Some(mut commit) = commit else {
        return Ok(path.is_empty().then(|| BrowseEntry::Directory(Vec::new(), None)));
    };
    if commit.tree.is_none() {
        // `log` metadata is intentionally light: the tree id comes from readCommit.
        if let Some(full) = handle.read_commit(&commit.hash).await? {
            commit.tree = full.tree;
        }
    }
    let Some(tree) = commit.tree.clone() else {
        return Ok(path.is_empty().then(|| BrowseEntry::Directory(Vec::new(), Some(commit))));
    };
    let mut tree = tree;
    let mut segments = path.split('/').filter(|s| !s.is_empty()).peekable();
    while let Some(segment) = segments.next() {
        let entries = handle.read_tree(&tree).await?;
        match entries.into_iter().find(|e| e.name == segment) {
            Some(entry) if entry.is_dir => tree = entry.hash,
            Some(_) if segments.peek().is_none() => return Ok(Some(BrowseEntry::File)),
            _ => return Ok(None),
        }
    }
    let entries = handle.read_tree(&tree).await?;
    Ok(Some(BrowseEntry::Directory(entries, Some(commit))))
}

async fn load_readme(handle: &RepoHandle, repo: &Repo, commit: &str) -> Option<String> {
    for candidate in ["README.md", "readme.md", "Readme.md", "README"] {
        let content = match handle.read_file(commit, candidate, Some(PREVIEW_BYTES)).await {
            Ok(Some(file)) if file.size() <= PREVIEW_BYTES as u64 => file.bytes(PREVIEW_BYTES).await,
            Ok(Some(file)) => {
                file.discard();
                continue;
            }
            Ok(None) => continue,
            Err(error) => Err(error),
        };
        match content {
            Ok(bytes) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    return Some(markdown::render_document(&text, &markdown::Context::new(repo, commit, candidate)));
                }
            }
            Err(error) => console_log!("repository: read {candidate}: {error}"),
        }
    }
    None
}

fn commit_row(commit: Commit, avatar_id: Option<i64>) -> CommitRow {
    CommitRow {
        short: util::short_hash(&commit.hash),
        hash: commit.hash,
        message: commit.message,
        avatar: Avatar::new(&commit.author, avatar_id),
        author: commit.author,
        date: commit.timestamp.map(Ts),
    }
}

fn file_row_at(entry: TreeEntry, parent: &str) -> FileRow {
    let url_path = if parent.is_empty() {
        util::url_path(&entry.name)
    } else {
        util::url_path(&format!("{parent}/{}", entry.name))
    };
    FileRow {
        name: entry.name,
        url_path,
        is_dir: entry.is_dir,
        size: entry.size.map(util::human_size).unwrap_or_default(),
    }
}

fn crumbs(path: &str) -> Vec<Crumb> {
    let mut out = Vec::new();
    let mut current = String::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        out.push(Crumb {
            name: segment.to_string(),
            url_path: util::url_path(&current),
        });
    }
    out
}

