use crate::ctx::AppState;
use crate::git_storage::{is_object_id, BlobRead, Commit, GitBackend, RepoHandle, TreeEntry};
use crate::handlers::common::{can_admin, not_found, page_title, visible_repo};
use crate::i18n::{CommonText, RepoText, SearchText};
use crate::limits::Profile;
use crate::models::Repo;
use crate::templates::{render, Chrome};
use crate::util::{query_param, url_path};
use askama::Template;
use std::borrow::Cow;
use worker::{Error, Request, Response, Result, RouteContext};

const MAX_QUERY_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_DEPTH: usize = 16;
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_RESULTS: usize = 40;

struct SearchLimits {
    reads: usize,
    entries: u32,
    content_bytes: usize,
    commits: usize,
}

impl SearchLimits {
    fn for_backend(profile: Profile, backend: GitBackend) -> Self {
        match profile {
            Profile::Free => Self {
                // Native reads leave room for authentication and snapshot lookup within 50 external calls.
                reads: if matches!(backend, GitBackend::Durable) {
                    64
                } else {
                    36
                },
                entries: 2_048,
                content_bytes: 512 * 1024,
                commits: 24,
            },
            Profile::Paid => Self {
                reads: 128,
                entries: 4_096,
                content_bytes: 2 * 1024 * 1024,
                commits: 64,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Path,
    Code,
    Commits,
}

impl SearchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Code => "code",
            Self::Commits => "commits",
        }
    }
}

#[derive(Template)]
#[template(path = "repository_search.html")]
struct SearchPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    branch: String,
    branch_url: String,
    query: String,
    mode: SearchMode,
    max_commits: usize,
    results: Option<SearchResults>,
    error: Option<&'static str>,
}

struct Hit {
    title: String,
    url: String,
    detail: Option<String>,
    line: Option<usize>,
}

#[derive(Default)]
struct Coverage {
    scan_limited: bool,
    large_files: bool,
    long_messages: bool,
    result_limited: bool,
    read_failed: bool,
}

impl Coverage {
    fn partial(&self) -> bool {
        self.scan_limited
            || self.large_files
            || self.long_messages
            || self.result_limited
            || self.read_failed
    }
}

struct SearchResults {
    snapshot: String,
    hits: Vec<Hit>,
    coverage: Coverage,
}

pub async fn search(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    let branch = query_param(&req, "branch").unwrap_or_else(|| repo.default_branch.clone());
    let query = query_param(&req, "q").unwrap_or_default();
    let mode = match query_param(&req, "type").as_deref() {
        Some("code") => SearchMode::Code,
        Some("commits") => SearchMode::Commits,
        _ => SearchMode::Path,
    };
    let locale = state.locale;
    let limits = SearchLimits::for_backend(Profile::read(&state.env)?, repo.git_backend);
    let mut page = SearchPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), locale.t(SearchText::RepositorySearch)]),
        tab: "code",
        is_owner: can_admin(state, &repo),
        branch_url: url_path(&branch),
        branch,
        query,
        mode,
        max_commits: limits.commits,
        repo,
        results: None,
        error: None,
    };
    let query = page.query.as_str();
    let (status, error) = if query.trim().is_empty() {
        (200, None)
    } else if query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control) {
        (400, Some(locale.t(SearchText::InvalidQuery)))
    } else if !valid_ref(&page.branch) {
        (400, Some(locale.t(SearchText::InvalidRef)))
    } else {
        let loaded = async {
            let handle = RepoHandle::open(&state.env, &page.repo).await?;
            load_results(&handle, &page.repo, &page.branch, query, mode, limits).await
        }
        .await;
        match loaded {
            Ok(Some(results)) => {
                page.results = Some(results);
                (200, None)
            }
            Ok(None) => (404, Some(locale.t(SearchText::RefNotFound))),
            Err(error) => {
                worker::console_log!("repository search: {error}");
                (502, Some(locale.t(RepoText::StorageUnavailable)))
            }
        }
    };
    page.error = error;
    let mut response = render(&page)?.with_status(status);
    response
        .headers_mut()
        .set("Cache-Control", "private, no-store")?;
    Ok(response)
}

fn valid_ref(reference: &str) -> bool {
    crate::handlers::repos::valid_browse_ref(reference)
}

async fn load_results(
    handle: &RepoHandle,
    repo: &Repo,
    branch: &str,
    query: &str,
    mode: SearchMode,
    limits: SearchLimits,
) -> Result<Option<SearchResults>> {
    let history_limit = if mode == SearchMode::Commits {
        limits.commits as u32 + 1
    } else {
        1
    };
    let history = handle.log(branch, history_limit).await?;
    let Some(head) = history.first() else {
        return Ok(None);
    };
    if !is_object_id(&head.hash) {
        return Err(Error::RustError("Invalid Git snapshot ID".into()));
    }
    let mut results = SearchResults {
        snapshot: head.hash.clone(),
        hits: Vec::new(),
        coverage: Coverage::default(),
    };
    let repo_url = format!("/{}/{}", repo.owner_name, repo.name);
    if mode == SearchMode::Commits {
        scan_commits(
            handle,
            &repo_url,
            query,
            history,
            limits.commits,
            &mut results,
        )
        .await;
    } else {
        let tree = if let Some(tree) = &head.tree {
            Cow::Borrowed(tree.as_str())
        } else {
            Cow::Owned(
                handle
                    .read_commit(&head.hash)
                    .await?
                    .and_then(|commit| commit.tree)
                    .unwrap_or_default(),
            )
        };
        if !is_object_id(&tree) {
            return Err(Error::RustError("Invalid Git snapshot tree ID".into()));
        }
        let mut scan = FileScan {
            handle,
            repo_url: &repo_url,
            query,
            code: mode == SearchMode::Code,
            results: &mut results,
            reads_left: limits.reads,
            entries_left: limits.entries,
            bytes_left: limits.content_bytes,
            path: String::new(),
        };
        scan.tree(&tree, 0).await;
    }
    Ok(Some(results))
}

async fn scan_commits(
    handle: &RepoHandle,
    repo_url: &str,
    query: &str,
    history: Vec<Commit>,
    max_commits: usize,
    results: &mut SearchResults,
) {
    let full_in_log = handle.log_includes_full_message();
    results.coverage.scan_limited = history.len() > max_commits;
    for commit in history.into_iter().take(max_commits) {
        if !is_object_id(&commit.hash) {
            results.coverage.read_failed = true;
            continue;
        }
        let detail = if commit.hash.starts_with(query) {
            Some(excerpt(&commit.message, 0))
        } else if let Some(detail) = commit_excerpt(&commit.message, query, &mut results.coverage) {
            Some(detail)
        } else if full_in_log {
            None
        } else {
            match handle.read_commit(&commit.hash).await {
                Ok(Some(full)) => commit_excerpt(&full.message, query, &mut results.coverage),
                Ok(None) | Err(_) => {
                    results.coverage.read_failed = true;
                    None
                }
            }
        };
        if let Some(detail) = detail {
            if results.hits.len() == MAX_RESULTS {
                results.coverage.result_limited = true;
                break;
            }
            results.hits.push(Hit {
                url: format!(
                    "{repo_url}/commit/{}?branch={}",
                    commit.hash, results.snapshot
                ),
                title: commit.hash,
                detail: Some(detail),
                line: None,
            });
        }
    }
}

fn commit_excerpt(message: &str, query: &str, coverage: &mut Coverage) -> Option<String> {
    let prefix = if message.len() <= MAX_MESSAGE_BYTES {
        message
    } else {
        coverage.long_messages = true;
        &message[..message.floor_char_boundary(MAX_MESSAGE_BYTES)]
    };
    prefix.find(query).map(|offset| excerpt(prefix, offset))
}

struct FileScan<'a> {
    handle: &'a RepoHandle,
    repo_url: &'a str,
    query: &'a str,
    code: bool,
    results: &'a mut SearchResults,
    reads_left: usize,
    entries_left: u32,
    bytes_left: usize,
    path: String,
}

impl FileScan<'_> {
    fn take_read(&mut self) -> bool {
        if self.reads_left == 0 {
            self.results.coverage.scan_limited = true;
            return false;
        }
        self.reads_left -= 1;
        true
    }

    async fn tree(&mut self, hash: &str, depth: usize) {
        if depth == MAX_DEPTH || self.entries_left == 0 {
            self.results.coverage.scan_limited = true;
            return;
        }
        if !self.take_read() {
            return;
        }
        let mut entries = match self.handle.read_tree_bounded(hash, self.entries_left).await {
            Ok(Some(entries)) => entries,
            Ok(None) => {
                self.results.coverage.scan_limited = true;
                return;
            }
            Err(_) => {
                self.results.coverage.read_failed = true;
                return;
            }
        };
        let Some(remaining) = self.entries_left.checked_sub(entries.len() as u32) else {
            self.results.coverage.scan_limited = true;
            return;
        };
        self.entries_left = remaining;
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        let parent_len = self.path.len();
        for entry in entries {
            if self.code && self.bytes_left == 0 {
                self.results.coverage.scan_limited = true;
                break;
            }
            if parent_len + (parent_len != 0) as usize + entry.name.len() > MAX_PATH_BYTES {
                self.results.coverage.scan_limited = true;
                continue;
            }
            if parent_len != 0 {
                self.path.push('/');
            }
            self.path.push_str(&entry.name);
            match entry.mode {
                "040000" => Box::pin(self.tree(&entry.hash, depth + 1)).await,
                "100644" | "100755" if self.code => self.file(&entry).await,
                "100644" | "100755" | "120000" if !self.code && self.path.contains(self.query) => {
                    self.file_hit(None, None);
                }
                _ => {}
            }
            self.path.truncate(parent_len);
            if self.results.coverage.result_limited {
                break;
            }
        }
    }

    async fn file(&mut self, entry: &TreeEntry) {
        if entry.size.is_some_and(|size| size > MAX_FILE_BYTES as u64) {
            self.results.coverage.large_files = true;
            return;
        }
        if self.bytes_left == 0 || entry.size.is_some_and(|size| size > self.bytes_left as u64) {
            self.results.coverage.scan_limited = true;
            return;
        }
        if !self.take_read() {
            return;
        }
        let limit = self.bytes_left.min(MAX_FILE_BYTES);
        let bytes = match self.handle.read_blob_bounded(&entry.hash, limit).await {
            Ok(BlobRead::Bytes(bytes)) => bytes,
            Ok(BlobRead::TooLarge) => {
                if limit == MAX_FILE_BYTES {
                    self.results.coverage.large_files = true;
                } else {
                    self.results.coverage.scan_limited = true;
                }
                return;
            }
            Err(_) => {
                self.results.coverage.read_failed = true;
                return;
            }
        };
        self.bytes_left -= bytes.len();
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return;
        };
        let Some(offset) = text.find(self.query) else {
            return;
        };
        if bytes.contains(&0) {
            return;
        }
        // Queries contain no line breaks. Only locate the line after finding a match.
        let before = &text[..offset];
        let start = before.rfind('\n').map_or(0, |at| at + 1);
        let line = text[start..].lines().next().unwrap_or_default();
        let number = before.bytes().filter(|&byte| byte == b'\n').count() + 1;
        self.file_hit(Some(excerpt(line, offset - start)), Some(number));
    }

    fn file_hit(&mut self, detail: Option<String>, line: Option<usize>) {
        if self.results.hits.len() == MAX_RESULTS {
            self.results.coverage.result_limited = true;
            return;
        }
        self.results.hits.push(Hit {
            url: format!(
                "{}/blob/{}?branch={}",
                self.repo_url,
                url_path(&self.path),
                self.results.snapshot
            ),
            title: self.path.clone(),
            detail,
            line,
        });
    }
}

fn excerpt(text: &str, offset: usize) -> String {
    let start = text.ceil_char_boundary(offset.saturating_sub(72));
    let end = text.floor_char_boundary((start + 240).min(text.len()));
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    format!("{prefix}{}{suffix}", text[start..end].trim())
}
