use super::common::{can_admin, not_found, page_title, visible_repo};
use crate::artifacts::{Artifacts, Commit, RepoHandle, TreeEntry};
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::i18n::{CommonText, RepoText};
use crate::models::Ts;
use crate::pagination::{query_url, Pagination, PAGE_SIZE};
use crate::refs::{self, Inventory, RefsError, RefsPage};
use crate::templates::{
    render, Chrome, CommitRow, CommitsPage, Crumb, FilePage, FileRow, NewRepoPage, RepoCodePage,
    SettingsPage,
};
use crate::util;
use js_sys::Uint8Array;
use std::borrow::Cow;
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::Blob;
use worker::*;

const PREVIEW_BYTES: usize = 256 * 1024;

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

    let artifacts = match Artifacts::from_env(&state.env) {
        Ok(a) => a,
        Err(error) => return invalid(log_failure("initialize repository storage", error, RepoText::CreateFailed)),
    };
    // Internal Artifacts name is opaque and immutable; the forge-facing name can change later.
    let internal = format!(
        "shard-{}-{}",
        name.to_lowercase().replace(['.', '_'], "-"),
        &crypto::random_token()?[..10]
    );
    let created = match artifacts.create(&internal, &description).await {
        Ok(created) => created,
        Err(error) => return invalid(log_failure("create repository storage", error, RepoText::CreateFailed)),
    };

    let inserted = db::run(
        &database,
        "INSERT INTO repos (owner_id, name, description, default_branch, artifacts_repo, artifacts_remote, visibility, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        ],
    )
    .await;

    if let Err(error) = inserted {
        if let Err(cleanup_error) = artifacts.delete(&created.name).await {
            console_log!("repository rollback failed: {cleanup_error}");
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

async fn storage_handle(state: &AppState, repository: &str) -> Result<RepoHandle> {
    Artifacts::from_env(&state.env)?.get(repository).await
}

async fn file_blob(state: &AppState, repository: &str, branch: &str, path: &str) -> Result<Option<Blob>> {
    storage_handle(state, repository).await?.read_file(branch, path).await
}

async fn blob_bytes(blob: &Blob) -> Result<Vec<u8>> {
    let buffer = JsFuture::from(blob.array_buffer()).await.map_err(Error::from)?;
    Ok(Uint8Array::new(&buffer).to_vec())
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
    let loaded = match storage_handle(state, &repo.artifacts_repo).await {
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
        let handle = storage_handle(&state, &repo.artifacts_repo).await?;
        let Some((entries, commit)) = load_dir(&handle, &branch, &path).await? else {
            return Ok(None);
        };
        let readme = match &commit {
            Some(commit) if path.is_empty() => load_readme(&handle, &commit.hash).await,
            _ => None,
        };
        Ok(Some((entries, commit, readme)))
    }
    .await;
    let (files, last_commit, readme, artifacts_error) = match loaded {
        Ok(Some((entries, commit, readme))) => (
            entries.into_iter().map(|entry| file_row_at(entry, &path)).collect(),
            commit.map(commit_row),
            readme,
            None,
        ),
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
    let clone_url = format!("{}/{}/{}.git", util::base_url(&req), repo.owner_name, repo.name);

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
        readme,
        last_commit,
        artifacts_error,
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
    let preview = async {
        let Some(blob) = file_blob(&state, &repo.artifacts_repo, &branch, &path).await? else {
            return Ok(None);
        };
        let size = blob.size() as u64;
        let truncated = size > PREVIEW_BYTES as u64;
        let prefix = if truncated {
            blob.slice_with_f64_and_f64(0.0, PREVIEW_BYTES as f64)
                .map_err(Error::from)?
        } else {
            blob
        };
        Ok(Some((blob_bytes(&prefix).await?, size, truncated)))
    }.await;
    let (bytes, size, truncated) = match preview {
        Ok(Some(preview)) => preview,
        Ok(None) => return not_found(&state, state.locale.t(RepoText::FileNotFound)),
        Err(error) => return storage_error(&state, "read file preview", error),
    };
    let is_binary = bytes.iter().take(8000).any(|byte| *byte == 0);
    let content = if is_binary {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let page = FilePage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), &path]),
        repo,
        tab: "code".into(),
        branch_url,
        path_url: util::url_path(&path),
        crumbs: crumbs(&path),
        content,
        is_binary,
        truncated: truncated && !is_binary,
        size: util::human_size(size),
        is_owner,
    };
    render(&page)
}

pub async fn raw(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
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
    let blob = match file_blob(&state, &repo.artifacts_repo, &branch, &path).await {
        Ok(Some(blob)) => blob,
        Ok(None) => return not_found(&state, state.locale.t(RepoText::FileNotFound)),
        Err(error) => return storage_error(&state, "read raw file", error),
    };
    let headers = Headers::new();
    headers.set("Content-Type", "application/octet-stream")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    let filename = path.rsplit('/').next().unwrap_or("file");
    let disposition = if filename.bytes().all(|b| (b' '..=b'~').contains(&b) && !matches!(b, b'"' | b'\\' | b'%')) {
        format!("inline; filename=\"{filename}\"")
    } else {
        format!("inline; filename*=UTF-8''{}", urlencoding::encode(filename))
    };
    headers.set("Content-Disposition", &disposition)?;
    Ok(Response::from_body(ResponseBody::Stream(blob.stream()))?.with_headers(headers))
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
    let handle = match storage_handle(&state, &repo.artifacts_repo).await {
        Ok(handle) => handle,
        Err(error) => return storage_error(&state, "open commit history", error),
    };
    // Artifacts log accepts a commit hash as its ref and walks first parents.
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
    let page = CommitsPage {
        chrome: Chrome::new(&state),
        title: page_title(&[&repo.full_name(), state.locale.t(CommonText::Commits)]),
        repo,
        tab: "commits".into(),
        branch,
        commits: history.into_iter().map(commit_row).collect(),
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
            Some("branch-not-found") => Some(RepoText::DefaultBranchNotFound),
            Some("branch-check-limit") => Some(RepoText::RefsLimit),
            Some("branch-check-unavailable") => Some(RepoText::StorageUnavailable),
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
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, state.locale.t(RepoText::SettingsForbidden));
    }
    let form = state.form()?;
    let description = util::form_value(form, "description").unwrap_or_default();
    let visibility = match util::form_value(form, "visibility").as_deref() {
        Some("private") => "private",
        _ => "public",
    };
    let default_branch = util::form_value(form, "default_branch")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| repo.default_branch.clone());
    if default_branch != repo.default_branch {
        let inventory = match storage_handle(&state, &repo.artifacts_repo).await {
            Ok(handle) => refs::load(&handle).await,
            Err(error) => {
                console_log!("repository: verify web default branch: {error}");
                Err(RefsError::Unavailable)
            }
        };
        let error = match inventory {
            Ok(refs) if refs.branches.iter().any(|branch| branch.name == default_branch) => None,
            Ok(_) => Some("branch-not-found"),
            Err(RefsError::Limit) => Some("branch-check-limit"),
            Err(RefsError::Unavailable) => Some("branch-check-unavailable"),
        };
        if let Some(error) = error {
            return util::redirect(&format!("/{}/{}/settings?error={error}", repo.owner_name, repo.name));
        }
    }

    let updated = async {
        let database = state.db()?;
        db::run(
            &database,
            "UPDATE repos SET description = ?, visibility = ?, default_branch = ?, updated_at = ? WHERE id = ?",
            &[
                util::js_text(&description),
                util::js_text(visibility),
                util::js_text(&default_branch),
                util::js_int(util::now()),
                util::js_int(repo.id),
            ],
        )
        .await
    }
    .await;
    if let Err(error) = updated {
        console_log!("repository: save settings: {error}");
        return util::redirect(&format!("/{}/{}/settings?error=save-failed", repo.owner_name, repo.name));
    }
    util::redirect(&format!("/{}/{}/settings?saved=1", repo.owner_name, repo.name))
}

pub async fn delete(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return repository_not_found(&state);
    };
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, state.locale.t(RepoText::DeleteForbidden));
    }
    let form = state.form()?;
    let confirmation = util::form_value(form, "confirm").unwrap_or_default();
    let settings_url = format!("/{}/{}/settings", repo.owner_name, repo.name);
    if confirmation != repo.name {
        return util::redirect(&format!("{settings_url}?error=confirm-deletion"));
    }
    let deleted = async {
        Artifacts::from_env(&state.env)?.delete(&repo.artifacts_repo).await
    }.await;
    if let Err(error) = deleted {
        console_log!("repository: delete Git storage: {error}");
        return util::redirect(&format!("{settings_url}?error=delete-storage-failed"));
    }
    // A false result means Git storage is already absent, allowing a retry
    // after storage deletion succeeded but a previous D1 deletion failed.
    let deleted = async {
        let database = state.db()?;
        db::run(&database, "DELETE FROM repos WHERE id = ?", &[util::js_int(repo.id)]).await
    }.await;
    if let Err(error) = deleted {
        console_log!("repository: delete metadata: {error}");
        return util::redirect(&format!("{settings_url}?error=delete-failed"));
    }
    util::redirect("/")
}

async fn load_dir(
    handle: &RepoHandle,
    branch: &str,
    path: &str,
) -> Result<Option<(Vec<TreeEntry>, Option<Commit>)>> {
    let commit = handle.head_commit(branch).await?;
    let Some(mut commit) = commit else {
        return Ok(path.is_empty().then(|| (Vec::new(), None)));
    };
    if commit.tree.is_none() {
        // `log` metadata is intentionally light: the tree id comes from readCommit.
        if let Some(full) = handle.read_commit(&commit.hash).await? {
            commit.tree = full.tree;
        }
    }
    let Some(tree) = commit.tree.clone() else {
        return Ok(path.is_empty().then(|| (Vec::new(), Some(commit))));
    };
    let mut tree = tree;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let entries = handle.read_tree(&tree).await?;
        match entries.into_iter().find(|e| e.name == segment) {
            Some(entry) if entry.is_dir => tree = entry.hash,
            _ => return Ok(None),
        }
    }
    let entries = handle.read_tree(&tree).await?;
    Ok(Some((entries, Some(commit))))
}

async fn load_readme(handle: &RepoHandle, commit: &str) -> Option<String> {
    for candidate in ["README.md", "readme.md", "Readme.md", "README"] {
        let content = match handle.read_file(commit, candidate).await {
            Ok(Some(blob)) if blob.size() <= PREVIEW_BYTES as f64 => blob_bytes(&blob).await,
            Ok(_) => continue,
            Err(error) => Err(error),
        };
        match content {
            Ok(bytes) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    return Some(crate::markdown::render(&text));
                }
            }
            Err(error) => console_log!("repository: read {candidate}: {error}"),
        }
    }
    None
}

fn commit_row(commit: Commit) -> CommitRow {
    CommitRow {
        short: util::short_hash(&commit.hash),
        hash: commit.hash,
        message: commit.message,
        author_initial: crate::templates::initial(&commit.author),
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

