use super::common::{
    can_admin, load_repo, not_found, page_title, visible_repo,
};
use crate::artifacts::{Artifacts, Commit, RepoHandle, TreeEntry};
use crate::ctx::AppState;
use crate::crypto;
use crate::db;
use crate::models::Ts;
use crate::templates::{
    render, Chrome, CommitRow, CommitsPage, Crumb, FilePage, FileRow, NewRepoPage, RepoCodePage,
    SettingsPage,
};
use crate::util;
use worker::*;

pub async fn new_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    if state.user.is_none() {
        return util::redirect("/user/login?next=/new");
    }
    let page = NewRepoPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&["New repository"]),
        error: None,
        name_value: String::new(),
        description_value: String::new(),
        private_checked: false,
    };
    render(&page)
}

pub async fn create(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let Some(user) = state.user.as_ref() else {
        return util::redirect("/user/login?next=/new");
    };
    let form = req.form_data().await?;
    let name = util::form_value(&form, "name")
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = util::form_value(&form, "description").unwrap_or_default();
    let private = util::form_bool(&form, "private");

    let invalid = |message: &str| -> Result<Response> {
        let page = NewRepoPage {
            chrome: Chrome::new(&state.user),
            title: page_title(&["New repository"]),
            error: Some(message.to_string()),
            name_value: name.clone(),
            description_value: description.clone(),
            private_checked: private,
        };
        render(&page)
    };

    if !util::valid_slug(&name) {
        return invalid("Repository names use letters, digits, dot, dash or underscore and start with a letter or digit.");
    }

    let database = state.db()?;
    if load_repo(&state, &user.username, &name).await?.is_some() {
        return invalid("You already have a repository with that name.");
    }

    let artifacts = match Artifacts::from_env(&state.env) {
        Ok(a) => a,
        Err(e) => return invalid(&failure_message(&e)),
    };
    // Internal Artifacts name is opaque and immutable; the forge-facing name can change later.
    let internal = format!(
        "shard-{}-{}",
        name.to_lowercase().replace(['.', '_'], "-"),
        &crypto::random_token()?[..10]
    );
    let created = match artifacts.create(&internal, &description).await {
        Ok(created) => created,
        Err(e) => return invalid(&failure_message(&e)),
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

    if let Err(e) = inserted {
        let _ = artifacts.delete(&created.name).await;
        return invalid(&failure_message(&e));
    }

    util::redirect(&format!("/{}/{}", user.username, name))
}

fn failure_message(error: &Error) -> String {
    format!("Could not create the repository: {error}")
}

pub async fn view(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let is_owner = can_admin(&state, &repo);
    let branch = util::query_param(&req, "branch").unwrap_or_else(|| repo.default_branch.clone());
    let mut branches = vec![repo.default_branch.clone()];
    if branch != repo.default_branch {
        branches.push(branch.clone());
    }

    let mut files = Vec::new();
    let mut readme = None;
    let mut last_commit = None;
    let mut artifacts_error = None;

    match Artifacts::from_env(&state.env) {
        Ok(artifacts) => match artifacts.get(&repo.artifacts_repo).await {
            Ok(handle) => {
                match load_dir(&handle, &branch, "").await {
                    Ok((entries, commit)) => {
                        files = entries.iter().map(file_row).collect();
                        last_commit = commit.map(commit_row);
                        readme = load_readme(&handle, &branch).await;
                    }
                    Err(e) => artifacts_error = Some(format!("{e}")),
                }
            }
            Err(e) => artifacts_error = Some(format!("{e}")),
        },
        Err(e) => artifacts_error = Some(failure_message(&e)),
    }

    let page = RepoCodePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name()]),
        repo: repo.clone(),
        tab: "code".into(),
        branch_url: util::url_path(&branch),
        branch,
        branches,
        crumbs: Vec::new(),
        files,
        readme,
        last_commit,
        artifacts_error,
        clone_url: format!(
            "{}/{}/{}.git",
            util::base_url(&req),
            repo.owner_name,
            repo.name
        ),
        is_owner,
    };
    render(&page)
}

pub async fn tree(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let path = ctx.param("path").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let branch_url = util::url_path(&branch);
    let is_owner = can_admin(&state, &repo);

    let mut files = Vec::new();
    let mut last_commit = None;
    let mut artifacts_error = None;
    match Artifacts::from_env(&state.env) {
        Ok(artifacts) => match artifacts.get(&repo.artifacts_repo).await {
            Ok(handle) => match load_dir(&handle, &branch, &path).await {
                Ok((entries, commit)) => {
                    files = entries.iter().map(|e| file_row_at(e, &path)).collect();
                    last_commit = commit.map(commit_row);
                }
                Err(e) => artifacts_error = Some(format!("{e}")),
            },
            Err(e) => artifacts_error = Some(format!("{e}")),
        },
        Err(e) => artifacts_error = Some(failure_message(&e)),
    }

    let page = RepoCodePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), &path]),
        repo: repo.clone(),
        tab: "code".into(),
        branch: branch.clone(),
        branch_url: branch_url.clone(),
        branches: vec![repo.default_branch.clone()],
        crumbs: crumbs(&path),
        files,
        readme: None,
        last_commit,
        artifacts_error,
        clone_url: format!(
            "{}/{}/{}.git",
            util::base_url(&req),
            repo.owner_name,
            repo.name
        ),
        is_owner,
    };
    render(&page)
}

pub async fn file(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let path = ctx.param("path").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let branch_url = util::url_path(&branch);
    let is_owner = can_admin(&state, &repo);
    let artifacts = Artifacts::from_env(&state.env)?;
    let handle = artifacts.get(&repo.artifacts_repo).await?;
    let bytes = handle
        .read_file(&branch, &path)
        .await?
        .ok_or_else(|| Error::RustError("file not found".into()))?;
    let size = bytes.len();
    let (content, is_binary, truncated) = render_blob(&bytes);
    let page = FilePage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), &path]),
        repo,
        tab: "code".into(),
        branch_url,
        path_url: util::url_path(&path),
        crumbs: crumbs(&path),
        content,
        is_binary,
        truncated,
        size: util::human_size(size as u64),
        is_owner,
    };
    render(&page)
}

pub async fn raw(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let path = ctx.param("path").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let branch = util::query_param(&req, "branch")
        .unwrap_or_else(|| repo.default_branch.clone());
    let artifacts = Artifacts::from_env(&state.env)?;
    let handle = artifacts.get(&repo.artifacts_repo).await?;
    let Some(bytes) = handle.read_file(&branch, &path).await? else {
        return not_found(&state, "File not found.");
    };
    let headers = Headers::new();
    headers.set("Content-Type", "application/octet-stream")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set(
        "Content-Disposition",
        &format!("inline; filename=\"{}\"", path.rsplit('/').next().unwrap_or("file")),
    )?;
    Ok(Response::from_bytes(bytes)?.with_headers(headers))
}

pub async fn commits(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    let branch = util::query_param(&req, "branch").unwrap_or_else(|| repo.default_branch.clone());
    let is_owner = can_admin(&state, &repo);
    let artifacts = Artifacts::from_env(&state.env)?;
    let handle = artifacts.get(&repo.artifacts_repo).await?;
    let commits = handle
        .log(&branch, 50)
        .await?
        .into_iter()
        .map(commit_row)
        .collect();
    let page = CommitsPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), "Commits"]),
        repo,
        tab: "commits".into(),
        branch,
        commits,
        is_owner,
    };
    render(&page)
}

pub async fn settings(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, "Only the repository owner can change settings.");
    }
    let page = SettingsPage {
        chrome: Chrome::new(&state.user),
        title: page_title(&[&repo.full_name(), "Settings"]),
        description_value: repo.description.clone(),
        visibility_value: repo.visibility.clone(),
        repo,
        tab: "settings".into(),
        error: util::query_param(&req, "error"),
        message: util::query_param(&req, "saved"),
        is_owner: true,
    };
    render(&page)
}

pub async fn update_settings(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = visible_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, "Only the repository owner can change settings.");
    }
    let form = req.form_data().await?;
    let description = util::form_value(&form, "description").unwrap_or_default();
    let visibility = match util::form_value(&form, "visibility").as_deref() {
        Some("private") => "private",
        _ => "public",
    };
    let default_branch = util::form_value(&form, "default_branch")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| repo.default_branch.clone());

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
    .await?;
    util::redirect(&format!("/{}/{}/settings?saved=1", repo.owner_name, repo.name))
}

pub async fn delete(mut req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = ctx.data.clone();
    let owner = ctx.param("owner").cloned().unwrap_or_default();
    let name = ctx.param("repo").cloned().unwrap_or_default();
    let Some(repo) = load_repo(&state, &owner, &name).await? else {
        return not_found(&state, "Repository not found.");
    };
    if !can_admin(&state, &repo) {
        return super::common::forbidden(&state, "Only the repository owner can delete it.");
    }
    let form = req.form_data().await?;
    let confirmation = util::form_value(&form, "confirm").unwrap_or_default();
    if confirmation != repo.name {
        return util::redirect_with_flash(
            &format!("/{}/{}/settings", repo.owner_name, repo.name),
            "error",
            "Type the repository name to confirm deletion.",
        );
    }
    let database = state.db()?;
    db::run(
        &database,
        "DELETE FROM repos WHERE id = ?",
        &[util::js_int(repo.id)],
    )
    .await?;
    if let Ok(artifacts) = Artifacts::from_env(&state.env) {
        let _ = artifacts.delete(&repo.artifacts_repo).await;
    }
    util::redirect("/")
}

async fn load_dir(
    handle: &RepoHandle,
    branch: &str,
    path: &str,
) -> Result<(Vec<TreeEntry>, Option<Commit>)> {
    let commit = handle.head_commit(branch).await?;
    let Some(mut commit) = commit else {
        return Ok((Vec::new(), None));
    };
    if commit.tree.is_none() {
        // `log` metadata is intentionally light: the tree id comes from readCommit.
        if let Some(full) = handle.read_commit(&commit.hash).await? {
            commit.tree = full.tree;
        }
    }
    let Some(tree) = commit.tree.clone() else {
        return Ok((Vec::new(), Some(commit)));
    };
    let mut tree = tree;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let entries = handle.read_tree(&tree).await?;
        match entries.into_iter().find(|e| e.name == segment) {
            Some(entry) if entry.is_dir => tree = entry.hash,
            _ => return Ok((Vec::new(), Some(commit))),
        }
    }
    let entries = handle.read_tree(&tree).await?;
    Ok((entries, Some(commit)))
}

async fn load_readme(handle: &RepoHandle, branch: &str) -> Option<String> {
    for candidate in ["README.md", "readme.md", "Readme.md", "README"] {
        if let Ok(Some(bytes)) = handle.read_file(branch, candidate).await {
            if let Ok(text) = String::from_utf8(bytes) {
                return Some(crate::markdown::render(&text));
            }
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

fn file_row(entry: &TreeEntry) -> FileRow {
    FileRow {
        name: entry.name.clone(),
        url_path: util::url_path(&entry.name),
        is_dir: entry.is_dir,
        size: entry.size.map(util::human_size).unwrap_or_default(),
    }
}

fn file_row_at(entry: &TreeEntry, parent: &str) -> FileRow {
    let path = if parent.is_empty() {
        entry.name.clone()
    } else {
        format!("{parent}/{}", entry.name)
    };
    FileRow {
        name: entry.name.clone(),
        url_path: util::url_path(&path),
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

fn render_blob(bytes: &[u8]) -> (String, bool, bool) {
    const LIMIT: usize = 256 * 1024;
    let binary = bytes.iter().take(8000).any(|b| *b == 0);
    if binary {
        return (String::new(), true, false);
    }
    let truncated = bytes.len() > LIMIT;
    let slice = &bytes[..bytes.len().min(LIMIT)];
    let text = String::from_utf8_lossy(slice).to_string();
    (text, false, truncated)
}
