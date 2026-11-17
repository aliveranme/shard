use crate::models::{Comment, Issue, Pull, Repo, Ts, User};
use askama::Template;
use worker::*;

#[derive(Clone, Default)]
pub struct Chrome {
    pub signed_in: bool,
    pub user_name: String,
    pub user_initial: String,
}

impl Chrome {
    pub fn new(user: &Option<User>) -> Self {
        match user {
            Some(u) => Self {
                signed_in: true,
                user_name: u.username.clone(),
                user_initial: u
                    .username
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
            },
            None => Self::default(),
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
    pub author_initial: String,
    pub date: Option<Ts>,
}

#[derive(Clone)]
pub struct IssueRow {
    pub number: i64,
    pub title: String,
    pub author: String,
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
    pub author: String,
    pub date: Ts,
    pub is_pull: bool,
}

#[derive(Clone)]
pub struct CommentRow {
    pub id: i64,
    pub author: String,
    pub initial: String,
    pub date: Ts,
    pub html: String,
}

#[derive(Clone)]
pub struct ChangedRow {
    pub path: String,
    pub status: String,
}

#[derive(Clone)]
pub struct PullView {
    pub head_branch: String,
    pub base_branch: String,
    pub head_commit: Option<String>,
    pub base_commit: Option<String>,
    pub merged: bool,
    pub merged_at: Option<Ts>,
}

impl PullView {
    pub fn from_pull(pull: &Pull) -> Self {
        Self {
            head_branch: pull.head_branch.clone(),
            base_branch: pull.base_branch.clone(),
            head_commit: pull.head_commit.clone(),
            base_commit: pull.base_commit.clone(),
            merged: pull.merged == 1,
            merged_at: pull.merged_at,
        }
    }
}

#[derive(Clone)]
pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub scope: String,
    pub created: Ts,
    pub last_used: Option<Ts>,
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePage {
    pub chrome: Chrome,
    pub title: String,
    pub repos: Vec<RepoRow>,
    pub is_explore: bool,
    pub query: String,
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    pub chrome: Chrome,
    pub title: String,
    pub name: String,
    pub joined: crate::models::Ts,
    pub repos: Vec<RepoRow>,
    pub is_self: bool,
}

#[derive(Template)]
#[template(path = "global_issues.html")]
pub struct GlobalIssuesPage {
    pub chrome: Chrome,
    pub title: String,
    pub heading: String,
    pub issues: Vec<GlobalIssueRow>,
}

#[derive(Template)]
#[template(path = "auth.html")]
pub struct AuthPage {
    pub chrome: Chrome,
    pub title: String,
    pub mode: String,
    pub error: Option<String>,
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
    pub readme: Option<String>,
    pub last_commit: Option<CommitRow>,
    pub artifacts_error: Option<String>,
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
    pub issues: Vec<IssueRow>,
    pub open_count: i64,
    pub closed_count: i64,
    pub is_pull: bool,
    pub is_owner: bool,
}

#[derive(Template)]
#[template(path = "issue.html")]
pub struct IssuePage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub tab: String,
    pub issue: Issue,
    pub author_initial: String,
    pub body_html: String,
    pub comments: Vec<CommentRow>,
    pub pull: Option<PullView>,
    pub changed: Vec<ChangedRow>,
    pub diff_error: Option<String>,
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
    pub head_value: String,
    pub branches: Vec<String>,
    pub is_owner: bool,
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

#[derive(Template)]
#[template(path = "tokens.html")]
pub struct TokensPage {
    pub chrome: Chrome,
    pub title: String,
    pub tokens: Vec<TokenRow>,
    pub new_token: Option<String>,
    pub error: Option<String>,
}

impl TokenRow {
    pub fn from_token(token: &crate::models::AccessToken) -> Self {
        Self {
            id: token.id,
            name: token.name.clone(),
            scope: token.scope.clone(),
            created: token.created_at,
            last_used: token.last_used_at,
        }
    }
}

impl CommentRow {
    pub fn from_comment(comment: &Comment) -> Self {
        Self {
            id: comment.id,
            author: comment.author().to_string(),
            initial: initial(comment.author()),
            date: comment.created_at,
            html: crate::markdown::render(&comment.body),
        }
    }
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
pub struct ErrorPage {
    pub chrome: Chrome,
    pub title: String,
    pub status: u16,
    pub message: String,
}

pub fn error_response(chrome: Chrome, status: u16, message: &str) -> Result<Response> {
    let page = ErrorPage {
        title: format!("{status}"),
        chrome,
        status,
        message: message.to_string(),
    };
    Ok(render(&page)?.with_status(status))
}
