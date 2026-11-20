use crate::ctx::AppState;
use crate::i18n::Locale;
use crate::models::{Issue, Pull, PullSnapshot, Repo, Ts};
use crate::pagination::Pagination;
use crate::pull_diff::{ChangedFile, FileDiff};
use askama::Template;
use worker::*;

#[derive(Clone, Default)]
pub struct Avatar {
    pub id: Option<i64>,
    pub initial: String,
}

impl Avatar {
    pub fn new(name: &str, id: Option<i64>) -> Self {
        let initial = if id.is_none() { initial(name) } else { String::new() };
        Self { id, initial }
    }
}

#[derive(Clone)]
pub struct Chrome {
    pub signed_in: bool,
    pub user_name: String,
    pub avatar: Avatar,
    pub csrf_token: String,
    pub locale: Locale,
    pub registration_open: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            signed_in: false,
            user_name: String::new(),
            avatar: Avatar::default(),
            csrf_token: String::new(),
            locale: Locale::default(),
            registration_open: true,
        }
    }
}

impl Chrome {
    pub fn new(state: &AppState) -> Self {
        match &state.user {
            Some(u) => Self {
                signed_in: true,
                user_name: u.username.clone(),
                avatar: Avatar::new(&u.username, u.avatar_id),
                csrf_token: state.csrf_token.clone(),
                locale: state.locale,
                registration_open: state.registration_open,
            },
            None => Self {
                csrf_token: state.csrf_token.clone(),
                locale: state.locale,
                registration_open: state.registration_open,
                ..Self::default()
            },
        }
    }
}

pub fn render<T: Template>(page: &T) -> Result<Response> {
    let body = page
        .render()
        .map_err(|e| Error::RustError(format!("template: {e}")))?;
    Response::from_html(body)
}

#[derive(Clone)]
pub struct RepoRow {
    pub owner: String,
    pub name: String,
    pub description: String,
    pub updated: Ts,
    pub open_issues: i64,
    pub open_pulls: i64,
    pub private: bool,
}

impl RepoRow {
    pub fn from_repo(repo: &Repo) -> Self {
        Self {
            owner: repo.owner_name.clone(),
            name: repo.name.clone(),
            description: repo.description.clone(),
            updated: repo.updated_at,
            open_issues: repo.open_issues,
            open_pulls: repo.open_pulls,
            private: repo.is_private(),
        }
    }
}

#[derive(Clone)]
pub struct Crumb {
    pub name: String,
    pub url_path: String,
}

#[derive(Clone)]
pub struct FileRow {
    pub name: String,
    pub url_path: String,
    pub is_dir: bool,
    pub size: String,
}

#[derive(Clone)]
pub struct CommitRow {
    pub hash: String,
    pub short: String,
    pub message: String,
    pub author: String,
    pub avatar: Avatar,
    pub date: Option<Ts>,
}

#[derive(Clone)]
pub struct IssueRow {
    pub number: i64,
    pub title: String,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub date: Ts,
    pub comments: i64,
    pub is_pull: bool,
    pub state: String,
}

#[derive(Clone)]
pub struct GlobalIssueRow {
    pub repo_full: String,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub author: Option<String>,
    pub date: Ts,
    pub is_pull: bool,
    pub state: String,
}

pub struct DiscussionRow {
    pub id: i64,
    pub author: Option<String>,
    pub avatar: Avatar,
    pub date: Ts,
    pub html: String,
    pub action: String,
    pub is_comment: bool,
    pub review: Option<ReviewContext>,
}

pub struct ReviewContext {
    pub base: String,
    pub head: String,
    pub version: i64,
    pub line: Option<ReviewLine>,
}

pub struct ReviewLine {
    pub path: String,
    pub url_path: String,
    pub side: String,
    pub line: i64,
}

pub struct FilePreview {
    pub path: String,
    pub modes: Option<(&'static str, &'static str)>,
    pub diff: FileDiff,
}

#[derive(Clone)]
pub struct PullView {
    pub source_name: Option<String>,
    pub head_branch: String,
    pub base_branch: String,
    pub snapshot: PullSnapshot,
    pub merged: bool,
    pub merged_at: Option<Ts>,
    pub merge_commit: Option<String>,
}

impl PullView {
    pub fn from_pull(pull: Pull, source_name: Option<String>) -> Self {
        Self {
            source_name,
            head_branch: pull.head_branch,
            base_branch: pull.base_branch,
            snapshot: pull.snapshot,
            merged: pull.merged == 1,
            merged_at: pull.merged_at,
            merge_commit: pull.merge_commit,
        }
    }
}


#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePage {
    pub chrome: Chrome,
    pub title: String,
    pub repos: Vec<RepoRow>,
    pub is_explore: bool,
    pub query: String,
    pub pagination: Pagination,
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    pub chrome: Chrome,
    pub title: String,
    pub name: String,
    pub avatar: Avatar,
    pub joined: crate::models::Ts,
    pub repos: Vec<RepoRow>,
    pub is_self: bool,
    pub pagination: Pagination,
}

#[derive(Template)]
#[template(path = "global_issues.html")]
pub struct GlobalIssuesPage {
    pub chrome: Chrome,
    pub title: String,
    pub heading: &'static str,
    pub issues: Vec<GlobalIssueRow>,
    pub state: String,
    pub query: String,
    pub filters: crate::issue_filters::IssueFilters,
    pub open_url: String,
    pub closed_url: String,
    pub all_url: String,
    pub pagination: Pagination,
}

#[derive(Template)]
#[template(path = "auth.html")]
pub struct AuthPage {
    pub chrome: Chrome,
    pub title: String,
    pub mode: String,
    pub error: Option<&'static str>,
    pub username_value: String,
    pub email_value: String,
    pub next: String,
}

#[derive(Template)]
#[template(path = "new_repo.html")]
pub struct NewRepoPage {
    pub chrome: Chrome,
    pub title: String,
    pub error: Option<String>,
    pub name_value: String,
    pub description_value: String,
    pub private_checked: bool,
}

#[derive(Template)]
#[template(path = "repo_code.html")]
pub struct RepoCodePage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub branch: String,
    pub branch_url: String,
    pub branches: Vec<String>,
    pub crumbs: Vec<Crumb>,
    pub files: Vec<FileRow>,
    pub show_sizes: bool,
    pub readme: Option<String>,
    pub last_commit: Option<CommitRow>,
    pub storage_notice: Option<String>,
    pub clone_url: String,
    pub is_owner: bool,
}

#[derive(Template)]
#[template(path = "file.html")]
pub struct FilePage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub branch_url: String,
    pub path_url: String,
    pub crumbs: Vec<Crumb>,
    pub content: String,
    pub highlighted: Option<String>,
    pub markdown_html: Option<String>,
    pub image_url: Option<String>,
    pub is_binary: bool,
    pub truncated: bool,
    pub size: String,
    pub is_owner: bool,
}

#[derive(Template)]
#[template(path = "commits.html")]
pub struct CommitsPage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub branch: String,
    pub commits: Vec<CommitRow>,
    pub is_owner: bool,
    pub pagination: Pagination,
    pub latest_url: Option<String>,
}

#[derive(Template)]
#[template(path = "issues.html")]
pub struct IssuesPage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub state: String,
    pub query: String,
    pub filters: crate::issue_filters::IssueFilters,
    pub issues: Vec<IssueRow>,
    pub open_count: i64,
    pub closed_count: i64,
    pub is_pull: bool,
    pub is_owner: bool,
    pub open_url: String,
    pub closed_url: String,
    pub all_url: String,
    pub pagination: Pagination,
}

#[derive(Template)]
#[template(path = "issue.html")]
pub struct IssuePage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub issue: Issue,
    pub author_avatar: Avatar,
    pub body_html: String,
    pub discussion: Vec<DiscussionRow>,
    pub pagination: Pagination,
    pub pull: Option<PullView>,
    pub comparison_available: bool,
    pub changed: Vec<ChangedFile>,
    pub preview: Option<FilePreview>,
    pub preview_missing: bool,
    pub diff_error: Option<&'static str>,
    pub viewed_review: Option<i64>,
    pub comparison_notice: Option<&'static str>,
    pub can_refresh_comparison: bool,
    pub metadata: Option<crate::issue_metadata::IssueMetadata>,
    pub reactions: Vec<crate::repo_features::Reaction>,
    pub merge_workflow: Option<crate::branch_rules::Workflow>,
    pub can_review: bool,
    pub can_comment_on_diff: bool,
    pub can_verify_merge: bool,
    pub can_edit: bool,
    pub is_owner: bool,
}

#[derive(Template)]
#[template(path = "issue_new.html")]
pub struct IssueNewPage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub is_pull: bool,
    pub error: Option<String>,
    pub title_value: String,
    pub body_value: String,
    pub base_value: String,
    pub source_value: String,
    pub head_value: String,
    pub branches: Vec<String>,
    pub issue_templates: Vec<crate::repo_features::IssueTemplateChoice>,
    pub selected_issue_template: Option<i64>,
    pub can_classify: bool,
    pub labels: Vec<LabelChoice>,
    pub is_owner: bool,
}

#[derive(Clone)]
pub struct LabelChoice {
    pub id: i64,
    pub name: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsPage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub error: Option<String>,
    pub message: Option<String>,
    pub description_value: String,
    pub visibility_value: String,
    pub is_owner: bool,
}



/// Single uppercase character for an avatar bubble.
pub fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage<'a> {
    pub chrome: &'a Chrome,
    pub title: String,
    pub status: u16,
    pub message: &'static str,
}

pub fn error_response(chrome: &Chrome, status: u16, message: &'static str) -> Result<Response> {
    let page = ErrorPage {
        title: format!("{status}"),
        chrome,
        status,
        message,
    };
    Ok(render(&page)?.with_status(status))
}
