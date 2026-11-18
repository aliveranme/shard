use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{can_admin, can_write, forbidden, not_found, page_title, visible_repo};
use crate::i18n::{CommonText, DiscussionText, IssueMetadataText};
use crate::models::{Issue, Repo};
use crate::templates::{Chrome, render};
use crate::util::{js_int, js_null, js_text, now, query_param, redirect};
use askama::Template;
use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::d1::D1Database;
use worker::{Error, FormData, FormEntry, Request, Response, Result, RouteContext};

const ELIGIBLE_ASSIGNEE: &str = "(u.id = r.owner_id OR u.is_admin = 1 OR EXISTS (
    SELECT 1 FROM repo_member_roles c WHERE c.repo_id = r.id AND c.user_id = u.id
))";
const WRITER: &str = "(u.id = r.owner_id OR u.is_admin = 1 OR EXISTS (
    SELECT 1 FROM repo_member_roles c WHERE c.repo_id = r.id AND c.user_id = u.id AND c.role = 'write'
))";

#[derive(Deserialize)]
pub(crate) struct Named {
    pub(crate) id: i64,
    pub(crate) name: String,
}

pub(crate) struct IssueMetadata {
    pub(crate) labels: Vec<Named>,
    pub(crate) milestone: Option<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) can_edit: bool,
}

#[derive(Default, Deserialize)]
struct Assignment {
    milestone_id: Option<i64>,
    milestone_name: Option<String>,
    assignee_id: Option<i64>,
    assignee_name: Option<String>,
}

async fn assigned(database: &D1Database, repo_id: i64, issue_id: i64) -> Result<(Vec<Named>, Assignment)> {
    let args = [js_int(repo_id), js_int(issue_id)];
    let labels = database.prepare(
        "SELECT l.id, l.name FROM issue_labels il
         JOIN repo_labels l ON l.repo_id = il.repo_id AND l.id = il.label_id
         WHERE il.repo_id = ? AND il.issue_id = ? ORDER BY l.name, l.id",
    ).bind(&args)?;
    let assignment = database.prepare(format!(
        "SELECT m.id AS milestone_id, m.name AS milestone_name,
                u.id AS assignee_id, u.username AS assignee_name
         FROM issues i
         JOIN repos r ON r.id = i.repo_id
         LEFT JOIN issue_milestones im ON im.repo_id = i.repo_id AND im.issue_id = i.id
         LEFT JOIN repo_milestones m ON m.repo_id = im.repo_id AND m.id = im.milestone_id
         LEFT JOIN users u ON u.id = i.assignee_id AND {ELIGIBLE_ASSIGNEE}
         WHERE i.repo_id = ? AND i.id = ? AND i.kind = 'issue'",
    )).bind(&args)?;
    let results = database.batch(vec![labels, assignment]).await?;
    Ok((results[0].results()?, results[1].results()?.pop().unwrap_or_default()))
}

pub(crate) async fn load(state: &AppState, repo: &Repo, issue: &Issue) -> Result<IssueMetadata> {
    let (labels, assignment) = assigned(&state.db()?, repo.id, issue.id).await?;
    Ok(IssueMetadata {
        labels,
        milestone: assignment.milestone_name,
        assignee: assignment.assignee_name,
        can_edit: is_author(state, issue.author_id) || can_write(state, repo).await?,
    })
}

fn is_author(state: &AppState, author_id: Option<i64>) -> bool {
    state.user_id().is_some_and(|id| Some(id) == author_id)
}

async fn route_repo(ctx: &RouteContext<AppState>) -> Result<Option<Repo>> {
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    visible_repo(&ctx.data, owner, name).await
}

fn issue_url(repo: &Repo, number: i64) -> String {
    format!("/{}/{}/issues/{number}", repo.owner_name, repo.name)
}

fn catalog_url(repo: &Repo) -> String {
    format!("/{}/{}/settings/issues", repo.owner_name, repo.name)
}

#[derive(Deserialize)]
struct IssueContent {
    id: i64,
    author_id: Option<i64>,
    title: String,
    body: String,
}

#[derive(Default)]
struct Selection {
    labels: Vec<i64>,
    milestone: Option<i64>,
    assignee: Option<i64>,
}

impl Selection {
    fn has_label(&self, id: &i64) -> bool {
        self.labels.binary_search(id).is_ok()
    }

    fn has_milestone(&self, id: &i64) -> bool {
        self.milestone == Some(*id)
    }

    fn has_assignee(&self, id: &i64) -> bool {
        self.assignee == Some(*id)
    }
}

#[derive(Template)]
#[template(path = "issue_edit.html")]
struct EditPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    number: i64,
    title_value: String,
    body_value: String,
    error: Option<&'static str>,
    can_classify: bool,
    selection: Selection,
    labels: Vec<Named>,
    milestones: Vec<Named>,
    assignees: Vec<Named>,
}

pub async fn edit_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    edit(&ctx, false).await
}

pub async fn save_edit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    edit(&ctx, true).await
}

async fn edit(ctx: &RouteContext<AppState>, submit: bool) -> Result<Response> {
    let state = &ctx.data;
    let locale = state.locale;
    let Some(repo) = route_repo(ctx).await? else {
        return not_found(state, locale.t(CommonText::RepositoryNotFound));
    };
    let Some(number) = ctx.param("number").and_then(|value| parse_id(value)) else {
        return not_found(state, locale.t(CommonText::NotFound));
    };
    let database = state.db()?;
    let Some(issue) = db::first::<IssueContent>(
        &database,
        "SELECT id, author_id, title, body FROM issues WHERE repo_id = ? AND number = ? AND kind = 'issue'",
        &[js_int(repo.id), js_int(number)],
    ).await? else {
        return not_found(state, locale.t(CommonText::NotFound));
    };
    let Some(actor_id) = state.user_id() else {
        return redirect(&format!("/user/login?next={}/edit", issue_url(&repo, number)));
    };
    let can_classify = can_write(state, &repo).await?;
    if !(is_author(state, issue.author_id) || can_classify) {
        return forbidden(state, locale.t(IssueMetadataText::EditForbidden));
    }

    let mut selection = None;
    let mut error = None;
    let mut title = None;
    let mut body = None;
    if submit {
        let form = state.form()?;
        let has_classification = ["classify", "label_id", "milestone_id", "assignee_id"]
            .iter().any(|field| form.has(field));
        if has_classification && !can_classify {
            return forbidden(state, locale.t(IssueMetadataText::ClassifyForbidden));
        }
        let mut valid = true;
        title = field(form, "title", &mut valid);
        body = field(form, "body", &mut valid);
        if has_classification {
            let marker = field(form, "classify", &mut valid);
            valid &= marker.as_deref().is_none_or(|value| value == "1");
            let (submitted, valid_selection) = read_selection(form, &mut valid);
            selection = Some(submitted);
            if !valid_selection {
                error = Some(locale.t(IssueMetadataText::InvalidSelection));
            }
        }
        if !valid {
            error = Some(locale.t(IssueMetadataText::InvalidForm));
        }
        if let Some(value) = title.as_deref().filter(|value| *value != issue.title) {
            if value.trim().is_empty() {
                error = Some(locale.t(DiscussionText::TitleRequired));
            } else if !valid_name(value, 300) {
                error = Some(locale.t(IssueMetadataText::InvalidTitle));
            }
        }
        if let Some(value) = body.as_deref().filter(|value| *value != issue.body) {
            if value.len() > 1_000_000 || value.chars().any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')) {
                error = Some(locale.t(IssueMetadataText::InvalidBody));
            }
        }
        if error.is_none() {
            let title_arg = title.as_deref().filter(|value| *value != issue.title).map_or_else(js_null, |value| js_text(value.trim()));
            let body_arg = body.as_deref().filter(|value| *value != issue.body).map_or_else(js_null, js_text);
            if let Some(selection) = &selection {
                if save_classification(&database, repo.id, issue.id, actor_id, selection, title_arg, body_arg).await? {
                    return redirect(&issue_url(&repo, number));
                }
                error = Some(locale.t(IssueMetadataText::InvalidSelection));
            } else {
                let saved = database.prepare(format!(
                    "UPDATE issues SET title = COALESCE(?4, title), body = COALESCE(?5, body), updated_at = ?6
                     WHERE repo_id = ?1 AND id = ?2 AND kind = 'issue'
                       AND EXISTS (
                         SELECT 1 FROM repos r JOIN users u ON u.id = ?3 WHERE r.id = issues.repo_id
                           AND ({WRITER} OR (issues.author_id = u.id AND (r.visibility = 'public' OR {ELIGIBLE_ASSIGNEE})))
                       ) RETURNING id",
                )).bind(&[js_int(repo.id), js_int(issue.id), js_int(actor_id), title_arg, body_arg, js_int(now())])?
                    .first::<i64>(Some("id")).await?;
                if saved.is_none() {
                    return forbidden(state, locale.t(IssueMetadataText::EditForbidden));
                }
                return redirect(&issue_url(&repo, number));
            }
        }
    }

    let mut page = EditPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), locale.t(IssueMetadataText::EditIssue)]),
        is_owner: can_admin(state, &repo),
        repo,
        tab: "issues",
        number,
        title_value: title.unwrap_or(issue.title),
        body_value: body.unwrap_or(issue.body),
        error,
        can_classify,
        selection: Selection::default(),
        labels: Vec::new(),
        milestones: Vec::new(),
        assignees: Vec::new(),
    };
    if can_classify {
        page.selection = match selection {
            Some(selection) => selection,
            None => {
                let (labels, assignment) = assigned(&database, page.repo.id, issue.id).await?;
                let mut labels: Vec<_> = labels.into_iter().map(|item| item.id).collect();
                labels.sort_unstable();
                Selection { labels, milestone: assignment.milestone_id, assignee: assignment.assignee_id }
            }
        };
        let [labels, milestones] = catalog_entries(&database, page.repo.id).await?;
        page.labels = labels;
        page.milestones = milestones;
        page.assignees = db::all(
            &database,
            "SELECT u.id, u.username AS name FROM users u
             JOIN (
                 SELECT owner_id AS id FROM repos WHERE id = ?1
                 UNION SELECT user_id FROM repo_member_roles WHERE repo_id = ?1
                 UNION SELECT id FROM users WHERE is_admin = 1
             ) eligible ON eligible.id = u.id
             ORDER BY u.username, u.id",
            &[js_int(page.repo.id)],
        ).await?;
    }
    let response = render(&page)?;
    Ok(if page.error.is_some() { response.with_status(400) } else { response })
}

// Every statement uses the same guard over rows that the batch never changes. A stale,
// foreign or unauthorized selection therefore writes nothing, including title and body.
async fn save_classification(
    database: &D1Database, repo_id: i64, issue_id: i64, actor_id: i64, selection: &Selection,
    title: JsValue, body: JsValue,
) -> Result<bool> {
    let labels = serde_json::to_string(&selection.labels).map_err(|error| Error::RustError(error.to_string()))?;
    let guard = format!(
        "WITH chosen_labels AS (SELECT value AS id FROM json_each(?6)),
          allowed AS (
            SELECT i.id, i.repo_id FROM issues i JOIN repos r ON r.id = i.repo_id
            WHERE i.repo_id = ?1 AND i.id = ?2 AND i.kind = 'issue'
              AND EXISTS (SELECT 1 FROM users u WHERE u.id = ?3 AND {WRITER})
              AND (?4 IS NULL OR EXISTS (SELECT 1 FROM repo_milestones m WHERE m.repo_id = r.id AND m.id = ?4))
              AND (?5 IS NULL OR EXISTS (SELECT 1 FROM users u WHERE u.id = ?5 AND {ELIGIBLE_ASSIGNEE}))
              AND NOT EXISTS (
                  SELECT 1 FROM chosen_labels c WHERE NOT EXISTS (
                      SELECT 1 FROM repo_labels l WHERE l.repo_id = r.id AND l.id = c.id
                  )
              )
          )",
    );
    let args = [
        js_int(repo_id), js_int(issue_id), js_int(actor_id),
        selection.milestone.map_or_else(js_null, js_int),
        selection.assignee.map_or_else(js_null, js_int), js_text(&labels),
        title, body, js_int(now()),
    ];
    let update = database.prepare(format!(
        "{guard} UPDATE issues SET title = COALESCE(?7, title), body = COALESCE(?8, body),
                    assignee_id = ?5, updated_at = ?9
                 WHERE id IN (SELECT id FROM allowed) RETURNING id",
    )).bind(&args)?;
    let remove_labels = database.prepare(format!(
        "{guard} DELETE FROM issue_labels WHERE issue_id IN (SELECT id FROM allowed)
                    AND label_id NOT IN (SELECT id FROM chosen_labels)",
    )).bind(&args[..6])?;
    let add_labels = database.prepare(format!(
        "{guard} INSERT INTO issue_labels(issue_id, repo_id, label_id)
                 SELECT a.id, a.repo_id, c.id FROM allowed a CROSS JOIN chosen_labels c WHERE 1
                 ON CONFLICT(issue_id, label_id) DO NOTHING",
    )).bind(&args[..6])?;
    let milestone_sql = if selection.milestone.is_some() {
        "INSERT INTO issue_milestones(issue_id, repo_id, milestone_id)
         SELECT id, repo_id, ?4 FROM allowed WHERE 1
         ON CONFLICT(issue_id) DO UPDATE SET milestone_id = excluded.milestone_id"
    } else {
        "DELETE FROM issue_milestones WHERE issue_id IN (SELECT id FROM allowed)"
    };
    let milestone = database.prepare(format!("{guard} {milestone_sql}")).bind(&args[..6])?;
    let results = database.batch(vec![update, remove_labels, add_labels, milestone]).await?;
    Ok(!results[0].results::<serde::de::IgnoredAny>()?.is_empty())
}

fn field(form: &FormData, name: &str, valid: &mut bool) -> Option<String> {
    let entries = form.get_all(name)?;
    if entries.is_empty() {
        return None;
    }
    *valid &= entries.len() == 1;
    let mut value = None;
    for entry in entries {
        match entry {
            FormEntry::Field(input) if value.is_none() => value = Some(input),
            _ => *valid = false,
        }
    }
    value
}

fn parse_id(value: &str) -> Option<i64> {
    const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|id| (1..=JS_MAX_SAFE_INTEGER).contains(id))
}

fn read_selection(form: &FormData, valid_form: &mut bool) -> (Selection, bool) {
    let mut selection = Selection::default();
    let mut valid = true;
    if let Some(entries) = form.get_all("label_id") {
        selection.labels.reserve(entries.len());
        for entry in entries {
            match entry {
                FormEntry::Field(value) => match parse_id(&value) {
                    Some(id) => selection.labels.push(id),
                    None => valid = false,
                },
                _ => valid = false,
            }
        }
        selection.labels.sort_unstable();
        selection.labels.dedup();
    }
    for (name, slot) in [("milestone_id", &mut selection.milestone), ("assignee_id", &mut selection.assignee)] {
        if let Some(value) = field(form, name, valid_form).filter(|value| !value.is_empty()) {
            *slot = parse_id(&value);
            valid &= slot.is_some();
        }
    }
    (selection, valid)
}

fn valid_name(value: &str, max_chars: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= max_chars
        && !value.chars().any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
}

#[derive(Clone, Copy)]
enum CatalogKind {
    Label,
    Milestone,
}

impl CatalogKind {
    fn table(self) -> &'static str {
        match self { Self::Label => "repo_labels", Self::Milestone => "repo_milestones" }
    }

    fn duplicate(self) -> IssueMetadataText {
        match self { Self::Label => IssueMetadataText::LabelExists, Self::Milestone => IssueMetadataText::MilestoneExists }
    }
}

async fn catalog_entries(database: &D1Database, repo_id: i64) -> Result<[Vec<Named>; 2]> {
    let entries = |kind: CatalogKind| {
        database.prepare(format!("SELECT id, name FROM {} WHERE repo_id = ? ORDER BY name, id", kind.table()))
            .bind(&[js_int(repo_id)])
    };
    let results = database.batch(vec![entries(CatalogKind::Label)?, entries(CatalogKind::Milestone)?]).await?;
    Ok([results[0].results()?, results[1].results()?])
}

#[derive(Template)]
#[template(path = "issue_catalog.html")]
struct CatalogPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    action_url: String,
    labels: Vec<Named>,
    milestones: Vec<Named>,
    label_value: String,
    milestone_value: String,
    saved: bool,
    error: Option<&'static str>,
}

pub async fn catalog(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    catalog_request(&req, &ctx, false).await
}

pub async fn catalog_action(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    catalog_request(&req, &ctx, true).await
}

enum CatalogAction {
    Create(CatalogKind),
    Rename(CatalogKind, i64),
    Remove(CatalogKind, i64),
}

impl CatalogAction {
    fn parse(form: &FormData) -> Option<Self> {
        let mut valid = true;
        let action = field(form, "action", &mut valid)?;
        let id = field(form, "id", &mut valid).and_then(|value| parse_id(&value));
        if !valid {
            return None;
        }
        Some(match action.as_str() {
            "create_label" => Self::Create(CatalogKind::Label),
            "create_milestone" => Self::Create(CatalogKind::Milestone),
            "rename_label" => Self::Rename(CatalogKind::Label, id?),
            "rename_milestone" => Self::Rename(CatalogKind::Milestone, id?),
            "remove_label" => Self::Remove(CatalogKind::Label, id?),
            "remove_milestone" => Self::Remove(CatalogKind::Milestone, id?),
            _ => return None,
        })
    }

    async fn apply(
        &self,
        database: &D1Database,
        repo_id: i64,
        actor_id: i64,
        name: &str,
    ) -> Result<Option<(u16, IssueMetadataText)>> {
        use IssueMetadataText::{CatalogEntryNotFound, CatalogForbidden, InvalidName};
        let (kind, id) = match *self {
            Self::Create(kind) => (kind, None),
            Self::Rename(kind, id) | Self::Remove(kind, id) => (kind, Some(id)),
        };
        if !matches!(self, Self::Remove(..)) && !valid_name(name, 80) {
            return Ok(Some((400, InvalidName)));
        }

        let table = kind.table();
        let guard = "SELECT r.id FROM repos r JOIN users u ON u.id = ?2
                     WHERE r.id = ?1 AND (r.owner_id = u.id OR u.is_admin = 1)";
        let status = database.prepare(format!(
            "SELECT EXISTS ({guard}) AS allowed,
                    EXISTS (SELECT 1 FROM {table} WHERE repo_id = ?1 AND id = ?3) AS found",
        ));
        let scope = [js_int(repo_id), js_int(actor_id), id.map_or_else(js_null, js_int)];
        let mutation = match self {
            Self::Create(_) => database.prepare(format!(
                "INSERT INTO {table}(repo_id, name) SELECT id, ?3 FROM ({guard}) WHERE 1
                 ON CONFLICT(repo_id, name) DO NOTHING RETURNING id",
            )).bind(&[scope[0].clone(), scope[1].clone(), js_text(name.trim())])?,
            Self::Rename(..) => database.prepare(format!(
                "UPDATE OR IGNORE {table} SET name = ?4 WHERE id = ?3 AND repo_id IN ({guard}) RETURNING id",
            )).bind(&[scope[0].clone(), scope[1].clone(), scope[2].clone(), js_text(name.trim())])?,
            Self::Remove(..) => database.prepare(format!(
                "DELETE FROM {table} WHERE id = ?3 AND repo_id IN ({guard}) RETURNING id",
            )).bind(&scope)?,
        };
        let results = database.batch(vec![
            status.bind(&scope)?,
            mutation,
        ]).await?;

        #[derive(Deserialize)]
        struct Status {
            allowed: i64,
            found: i64,
        }
        let status = results[0].results::<Status>()?.pop()
            .ok_or_else(|| Error::RustError("missing catalog mutation status".into()))?;
        if status.allowed == 0 {
            return Ok(Some((403, CatalogForbidden)));
        }
        if id.is_some() && status.found == 0 {
            return Ok(Some((404, CatalogEntryNotFound)));
        }
        Ok(results[1].results::<serde::de::IgnoredAny>()?.is_empty().then(|| (409, kind.duplicate())))
    }
}

async fn catalog_request(req: &Request, ctx: &RouteContext<AppState>, submit: bool) -> Result<Response> {
    let state = &ctx.data;
    let locale = state.locale;
    let Some(repo) = route_repo(ctx).await? else {
        return not_found(state, locale.t(CommonText::RepositoryNotFound));
    };
    let action_url = catalog_url(&repo);
    let Some(actor_id) = state.user_id() else {
        return redirect(&format!("/user/login?next={action_url}"));
    };
    if !can_admin(state, &repo) {
        return forbidden(state, locale.t(IssueMetadataText::CatalogForbidden));
    }
    let database = state.db()?;
    let mut error = None;
    let mut action = None;
    let mut name = String::new();
    if submit {
        let form = state.form()?;
        let mut valid = true;
        name = field(form, "name", &mut valid).unwrap_or_default();
        action = CatalogAction::parse(form).filter(|_| valid);
        error = match &action {
            Some(action) => action.apply(&database, repo.id, actor_id, &name).await?,
            None => Some((400, IssueMetadataText::InvalidAction)),
        };
        if error.is_none() {
            return redirect(&format!("{action_url}?saved=1"));
        }
        if let Some((403, message)) = error {
            return forbidden(state, locale.t(message));
        }
    }
    let [mut labels, mut milestones] = catalog_entries(&database, repo.id).await?;
    let mut label_value = String::new();
    let mut milestone_value = String::new();
    match action {
        Some(CatalogAction::Create(CatalogKind::Label)) => label_value = name,
        Some(CatalogAction::Create(CatalogKind::Milestone)) => milestone_value = name,
        Some(CatalogAction::Rename(kind, id)) => {
            let entries = match kind { CatalogKind::Label => &mut labels, CatalogKind::Milestone => &mut milestones };
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.name = name;
            }
        }
        _ => {}
    }
    let page = CatalogPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), locale.t(IssueMetadataText::Catalog)]),
        repo,
        tab: "settings",
        is_owner: true,
        action_url,
        labels,
        milestones,
        label_value,
        milestone_value,
        saved: !submit && query_param(req, "saved").as_deref() == Some("1"),
        error: error.map(|(_, key)| locale.t(key)),
    };
    let response = render(&page)?;
    Ok(if let Some((status, _)) = error { response.with_status(status) } else { response })
}
