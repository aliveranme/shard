use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{
    forbidden, not_found, page_title, viewer_params, visible_repo, VISIBLE_REPO_SQL,
};
use crate::i18n::{CommonText, OrganizationText as Text};
use crate::models::Repo;
use crate::templates::{error_response, render, Chrome};
use crate::util::{
    form_bool, form_value, js_int, js_text, redirect, safe_next, url_path, valid_slug,
};
use askama::Template;
use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::{d1::D1Database, Error, FormData, Request, Response, Result, RouteContext};

const ORGANIZATIONS: &str = "/user/organizations";

#[derive(Deserialize)]
struct Organization {
    id: i64,
    slug: String,
    owner_id: i64,
}

impl Organization {
    fn path(&self) -> String {
        format!("{ORGANIZATIONS}/{}", self.slug)
    }
}

#[derive(Deserialize)]
struct Team {
    id: i64,
    slug: String,
}

#[derive(Deserialize)]
struct Member {
    id: i64,
    username: String,
}

#[derive(Deserialize)]
struct Link {
    id: i64,
    slug: String,
    accepted: i64,
}

#[derive(Deserialize)]
struct LinkedRepo {
    id: i64,
    owner: String,
    name: String,
    accepted: i64,
    can_view: i64,
}

#[derive(Deserialize)]
struct GrantedRepo {
    id: i64,
    owner: String,
    name: String,
    role: String,
    can_view: i64,
}

#[derive(Template)]
#[template(path = "organizations.html")]
struct IndexPage {
    chrome: Chrome,
    title: String,
    actor: i64,
    organizations: Vec<Organization>,
}

#[derive(Template)]
#[template(path = "organization.html")]
struct OrganizationPage {
    chrome: Chrome,
    title: String,
    organization: Organization,
    manager: bool,
    teams: Vec<Team>,
    members: Vec<Member>,
    repositories: Vec<LinkedRepo>,
}

#[derive(Template)]
#[template(path = "organization_team.html")]
struct TeamPage {
    chrome: Chrome,
    title: String,
    organization: Organization,
    team: Team,
    manager: bool,
    members: Vec<Member>,
    available_members: Vec<Member>,
    grants: Vec<GrantedRepo>,
    available_repositories: Vec<LinkedRepo>,
}

#[derive(Template)]
#[template(path = "repository_organization.html")]
struct RepositoryPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    link: Option<Link>,
    organizations: Vec<Organization>,
}

pub async fn index(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    render(&IndexPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::Organizations)]),
        actor,
        organizations: memberships(&state.db()?, actor).await?,
    })
}

pub async fn create(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let slug = form_value(state.form()?, "slug").unwrap_or_default();
    let slug = slug.trim();
    if !valid_slug(slug) {
        return error(state, 400, Text::InvalidSlug);
    }
    let created = change(
        &state.db()?,
        "INSERT INTO organizations (slug, owner_id)
         SELECT ?1, id FROM users WHERE id = ?2
           AND (SELECT COUNT(*) FROM organizations WHERE owner_id = ?2) < 50
         ON CONFLICT(slug) DO NOTHING",
        &[js_text(slug), js_int(actor)],
    )
    .await?;
    if !created {
        return error(state, 409, Text::SlugTaken);
    }
    redirect(&format!("{ORGANIZATIONS}/{slug}"))
}

pub async fn show(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let database = state.db()?;
    let Some(organization) = find_organization(&database, &ctx, actor).await? else {
        return not_found(state, state.locale.t(Text::OrganizationNotFound));
    };
    let manager = organization.owner_id == actor;
    let teams = db::all(
        &database,
        "SELECT t.id, t.slug FROM organization_teams t WHERE t.organization_id = ?1
           AND (?2 = 1 OR EXISTS (SELECT 1 FROM organization_team_members m
             WHERE m.organization_id = t.organization_id AND m.team_id = t.id AND m.user_id = ?3))
         ORDER BY t.slug LIMIT 200",
        &[
            js_int(organization.id),
            js_int(i64::from(manager)),
            js_int(actor),
        ],
    )
    .await?;
    let members = if manager {
        db::all(
            &database,
            "SELECT u.id, u.username FROM organization_members m JOIN users u ON u.id = m.user_id
             WHERE m.organization_id = ?1 ORDER BY u.username LIMIT 200",
            &[js_int(organization.id)],
        )
        .await?
    } else {
        Vec::new()
    };
    let repositories = linked_repositories(state, &database, &organization, false).await?;
    render(&OrganizationPage {
        chrome: Chrome::new(state),
        title: page_title(&[&organization.slug, state.locale.t(Text::Organizations)]),
        organization,
        manager,
        teams,
        members,
        repositories,
    })
}

pub async fn update(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let database = state.db()?;
    let Some(organization) = find_organization(&database, &ctx, actor).await? else {
        return not_found(state, state.locale.t(Text::OrganizationNotFound));
    };
    let form = state.form()?;
    let path = organization.path();
    let org = organization.id;
    let action = form_value(form, "action");
    if action.as_deref() == Some("leave") {
        if organization.owner_id == actor {
            return error(state, 409, Text::StaleAction);
        }
        let results = database.batch(vec![
            database.prepare(
                "DELETE FROM organization_repositories WHERE organization_id = ?1
                   AND EXISTS (SELECT 1 FROM repos r WHERE r.id = organization_repositories.repo_id AND r.owner_id = ?2)",
            ).bind(&[js_int(org), js_int(actor)])?,
            database.prepare(
                "DELETE FROM organization_members WHERE organization_id = ?1 AND user_id = ?2
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id <> ?2)
                 RETURNING user_id",
            ).bind(&[js_int(org), js_int(actor)])?,
        ]).await?;
        let left = !results[1].results::<serde::de::IgnoredAny>()?.is_empty();
        if !left {
            return error(state, 409, Text::StaleAction);
        }
        return redirect(ORGANIZATIONS);
    }
    if organization.owner_id != actor {
        return forbidden(state, state.locale.t(Text::ManageForbidden));
    }
    let changed = match action.as_deref() {
        Some("create_team") => {
            let slug = form_value(form, "slug").unwrap_or_default();
            let slug = slug.trim();
            if !valid_slug(slug) {
                return error(state, 400, Text::InvalidSlug);
            }
            let created = change(
                &database,
                "INSERT INTO organization_teams (organization_id, slug)
                 SELECT id, ?2 FROM organizations WHERE id = ?1 AND owner_id = ?3
                   AND (SELECT COUNT(*) FROM organization_teams WHERE organization_id = ?1) < 100
                 ON CONFLICT(organization_id, slug) DO NOTHING",
                &[js_int(org), js_text(slug), js_int(actor)],
            )
            .await?;
            if !created {
                return error(state, 409, Text::SlugTaken);
            }
            return redirect(&format!("{path}/teams/{slug}"));
        }
        Some("add_member") => {
            let username = form_value(form, "username").unwrap_or_default();
            if !valid_slug(username.trim()) {
                return error(state, 400, Text::MemberUnavailable);
            }
            let added = change(
                &database,
                "INSERT INTO organization_members (organization_id, user_id)
                 SELECT o.id, u.id FROM organizations o JOIN users u ON u.username = ?2 COLLATE NOCASE
                 WHERE o.id = ?1 AND o.owner_id = ?3
                 ON CONFLICT(organization_id, user_id) DO NOTHING",
                &[js_int(org), js_text(username.trim()), js_int(actor)],
            )
            .await?;
            if !added {
                return error(state, 409, Text::MemberUnavailable);
            }
            true
        }
        Some("remove_member") => {
            let Some(user) = form_id(form, "user_id") else {
                return invalid(state);
            };
            let results = database.batch(vec![
                database.prepare(
                    "DELETE FROM organization_repositories WHERE organization_id = ?1
                       AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3 AND owner_id <> ?2)
                       AND EXISTS (SELECT 1 FROM repos r WHERE r.id = organization_repositories.repo_id AND r.owner_id = ?2)",
                ).bind(&[js_int(org), js_int(user), js_int(actor)])?,
                database.prepare(
                    "DELETE FROM organization_members WHERE organization_id = ?1 AND user_id = ?2
                       AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3 AND owner_id <> ?2)
                     RETURNING user_id",
                ).bind(&[js_int(org), js_int(user), js_int(actor)])?,
            ]).await?;
            !results[1].results::<serde::de::IgnoredAny>()?.is_empty()
        }
        Some("accept") => {
            let Some(link) = form_id(form, "link_id") else {
                return invalid(state);
            };
            change(
                &database,
                "UPDATE organization_repositories SET accepted = 1
                 WHERE organization_id = ?1 AND id = ?2 AND accepted = 0
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3)
                   AND EXISTS (
                       SELECT 1 FROM repos r
                       JOIN organization_members m ON m.organization_id = organization_repositories.organization_id
                        AND m.user_id = r.owner_id
                       WHERE r.id = organization_repositories.repo_id
                   )",
                &[js_int(org), js_int(link), js_int(actor)],
            )
            .await?
        }
        Some("detach") => {
            let Some(link) = form_id(form, "link_id") else {
                return invalid(state);
            };
            change(
                &database,
                "DELETE FROM organization_repositories WHERE organization_id = ?1 AND id = ?2
                   AND (accepted = 0 OR ?4 = 1)
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3)",
                &[
                    js_int(org),
                    js_int(link),
                    js_int(actor),
                    js_int(i64::from(form_bool(form, "confirm"))),
                ],
            )
            .await?
        }
        _ => return invalid(state),
    };
    updated(state, changed, &path)
}

pub async fn team(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let database = state.db()?;
    let Some(organization) = find_organization(&database, &ctx, actor).await? else {
        return not_found(state, state.locale.t(Text::TeamNotFound));
    };
    let Some(team) = find_team(&database, &ctx, &organization, actor).await? else {
        return not_found(state, state.locale.t(Text::TeamNotFound));
    };
    let manager = organization.owner_id == actor;
    let members = db::all(
        &database,
        "SELECT u.id, u.username FROM organization_team_members m JOIN users u ON u.id = m.user_id
         WHERE m.organization_id = ?1 AND m.team_id = ?2 ORDER BY u.username LIMIT 200",
        &[js_int(organization.id), js_int(team.id)],
    )
    .await?;
    let available_members = if manager {
        db::all(
            &database,
            "SELECT u.id, u.username FROM organization_members m JOIN users u ON u.id = m.user_id
             WHERE m.organization_id = ?1 AND NOT EXISTS (
                 SELECT 1 FROM organization_team_members t
                 WHERE t.organization_id = m.organization_id AND t.team_id = ?2 AND t.user_id = m.user_id
             ) ORDER BY u.username LIMIT 200",
            &[js_int(organization.id), js_int(team.id)],
        )
        .await?
    } else {
        Vec::new()
    };
    let [viewer, admin] = viewer_params(state);
    let grants = db::all(
        &database,
        &format!(
            "SELECT l.id, u.username AS owner, r.name, g.role, {VISIBLE_REPO_SQL} AS can_view
             FROM organization_repo_grants g
             JOIN organization_repositories l ON l.id = g.link_id AND l.accepted = 1
             JOIN repos r ON r.id = l.repo_id JOIN users u ON u.id = r.owner_id
             WHERE g.organization_id = ?3 AND g.team_id = ?4 AND (?5 = 1 OR {VISIBLE_REPO_SQL})
             ORDER BY u.username, r.name COLLATE NOCASE, r.id LIMIT 200"
        ),
        &[
            viewer,
            admin,
            js_int(organization.id),
            js_int(team.id),
            js_int(i64::from(manager)),
        ],
    )
    .await?;
    let available_repositories = if manager {
        linked_repositories(state, &database, &organization, true).await?
    } else {
        Vec::new()
    };
    render(&TeamPage {
        chrome: Chrome::new(state),
        title: page_title(&[&team.slug, &organization.slug]),
        organization,
        team,
        manager,
        members,
        available_members,
        grants,
        available_repositories,
    })
}

pub async fn update_team(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let database = state.db()?;
    let Some(organization) = find_organization(&database, &ctx, actor).await? else {
        return not_found(state, state.locale.t(Text::TeamNotFound));
    };
    if organization.owner_id != actor {
        return forbidden(state, state.locale.t(Text::ManageForbidden));
    }
    let Some(team) = find_team(&database, &ctx, &organization, actor).await? else {
        return not_found(state, state.locale.t(Text::TeamNotFound));
    };
    let form = state.form()?;
    let args = [js_int(organization.id), js_int(team.id), js_int(actor)];
    let path = format!("{}/teams/{}", organization.path(), team.slug);
    let changed = match form_value(form, "action").as_deref() {
        Some("add_member") => {
            let Some(user) = form_id(form, "user_id") else {
                return invalid(state);
            };
            let [org, team, actor] = args;
            change(
                &database,
                "INSERT INTO organization_team_members (organization_id, team_id, user_id)
                 SELECT t.organization_id, t.id, m.user_id FROM organization_teams t
                 JOIN organizations o ON o.id = t.organization_id
                 JOIN organization_members m ON m.organization_id = o.id AND m.user_id = ?4
                 WHERE o.id = ?1 AND t.id = ?2 AND o.owner_id = ?3
                 ON CONFLICT(organization_id, team_id, user_id) DO NOTHING",
                &[org, team, actor, js_int(user)],
            )
            .await?
        }
        Some("remove_member") => {
            let Some(user) = form_id(form, "user_id") else {
                return invalid(state);
            };
            let [org, team, actor] = args;
            change(
                &database,
                "DELETE FROM organization_team_members WHERE organization_id = ?1 AND team_id = ?2 AND user_id = ?4
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3)",
                &[org, team, actor, js_int(user)],
            )
            .await?
        }
        Some("grant") => {
            let (Some(link), Some(role)) = (form_id(form, "link_id"), form_value(form, "role"))
            else {
                return invalid(state);
            };
            if !matches!(role.as_str(), "read" | "write") {
                return invalid(state);
            }
            let [org, team, actor] = args;
            change(
                &database,
                "INSERT INTO organization_repo_grants (organization_id, team_id, link_id, role)
                 SELECT t.organization_id, t.id, l.id, ?5 FROM organization_teams t
                 JOIN organizations o ON o.id = t.organization_id
                 JOIN organization_repositories l ON l.organization_id = o.id AND l.id = ?4 AND l.accepted = 1
                 JOIN repos r ON r.id = l.repo_id
                 JOIN organization_members om ON om.organization_id = o.id AND om.user_id = r.owner_id
                 WHERE o.id = ?1 AND t.id = ?2 AND o.owner_id = ?3
                 ON CONFLICT(organization_id, team_id, link_id) DO UPDATE SET role = excluded.role",
                &[org, team, actor, js_int(link), js_text(&role)],
            )
            .await?
        }
        Some("revoke") => {
            let Some(link) = form_id(form, "link_id") else {
                return invalid(state);
            };
            let [org, team, actor] = args;
            change(
                &database,
                "DELETE FROM organization_repo_grants WHERE organization_id = ?1 AND team_id = ?2 AND link_id = ?4
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3)",
                &[org, team, actor, js_int(link)],
            )
            .await?
        }
        Some("delete_team") if form_bool(form, "confirm") => {
            let deleted = change(
                &database,
                "DELETE FROM organization_teams WHERE organization_id = ?1 AND id = ?2
                   AND EXISTS (SELECT 1 FROM organizations WHERE id = ?1 AND owner_id = ?3)",
                &args,
            )
            .await?;
            return updated(state, deleted, &organization.path());
        }
        _ => return invalid(state),
    };
    updated(state, changed, &path)
}

pub async fn repository(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    manage_repository(req, ctx, false).await
}

pub async fn update_repository(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    manage_repository(req, ctx, true).await
}

async fn manage_repository(
    req: Request,
    ctx: RouteContext<AppState>,
    submit: bool,
) -> Result<Response> {
    let state = &ctx.data;
    let Some(actor) = state.user_id() else {
        return login(&req);
    };
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let Some(repo) = visible_repo(state, owner, name).await? else {
        return not_found(state, state.locale.t(CommonText::RepositoryNotFound));
    };
    if actor != repo.owner_id {
        return forbidden(state, state.locale.t(Text::RepositoryOwnerRequired));
    }
    let database = state.db()?;
    if submit {
        let form = state.form()?;
        let changed = match form_value(form, "action").as_deref() {
            Some("offer") => {
                let Some(org) = form_id(form, "organization_id") else {
                    return invalid(state);
                };
                change(
                    &database,
                    "INSERT INTO organization_repositories (organization_id, repo_id)
                     SELECT m.organization_id, r.id FROM repos r JOIN organization_members m ON m.user_id = r.owner_id
                     WHERE r.id = ?1 AND m.organization_id = ?2 AND r.owner_id = ?3
                     ON CONFLICT(repo_id) DO NOTHING",
                    &[js_int(repo.id), js_int(org), js_int(actor)],
                )
                .await?
            }
            Some("detach") => {
                let Some(link) = form_id(form, "link_id") else {
                    return invalid(state);
                };
                change(
                    &database,
                    "DELETE FROM organization_repositories WHERE repo_id = ?1 AND id = ?2
                       AND (accepted = 0 OR ?4 = 1)
                       AND EXISTS (SELECT 1 FROM repos WHERE id = ?1 AND owner_id = ?3)",
                    &[
                        js_int(repo.id),
                        js_int(link),
                        js_int(actor),
                        js_int(i64::from(form_bool(form, "confirm"))),
                    ],
                )
                .await?
            }
            _ => return invalid(state),
        };
        return updated(
            state,
            changed,
            &format!("/{}/settings/organization", repo.full_name()),
        );
    }

    let link = db::first(
        &database,
        "SELECT l.id, o.slug, l.accepted FROM organization_repositories l
         JOIN organizations o ON o.id = l.organization_id WHERE l.repo_id = ?1",
        &[js_int(repo.id)],
    )
    .await?;
    let organizations = if link.is_none() {
        memberships(&database, actor).await?
    } else {
        Vec::new()
    };
    render(&RepositoryPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(Text::OrganizationAccess)]),
        repo,
        link,
        organizations,
    })
}

async fn memberships(database: &D1Database, actor: i64) -> Result<Vec<Organization>> {
    db::all(
        database,
        "SELECT o.id, o.slug, o.owner_id FROM organization_members m JOIN organizations o ON o.id = m.organization_id
         WHERE m.user_id = ?1 ORDER BY o.slug LIMIT 200",
        &[js_int(actor)],
    )
    .await
}

async fn find_organization(
    database: &D1Database,
    ctx: &RouteContext<AppState>,
    actor: i64,
) -> Result<Option<Organization>> {
    let slug = ctx.param("org").map_or("", String::as_str);
    db::first(
        database,
        "SELECT o.id, o.slug, o.owner_id FROM organizations o JOIN organization_members m ON m.organization_id = o.id
         WHERE o.slug = ?1 COLLATE NOCASE AND m.user_id = ?2",
        &[js_text(slug), js_int(actor)],
    )
    .await
}

async fn find_team(
    database: &D1Database,
    ctx: &RouteContext<AppState>,
    organization: &Organization,
    actor: i64,
) -> Result<Option<Team>> {
    let slug = ctx.param("team").map_or("", String::as_str);
    db::first(
        database,
        "SELECT t.id, t.slug FROM organization_teams t
         JOIN organizations o ON o.id = t.organization_id
         WHERE o.id = ?1 AND t.slug = ?2 COLLATE NOCASE
           AND (o.owner_id = ?3 OR EXISTS (SELECT 1 FROM organization_team_members m
               WHERE m.organization_id = o.id AND m.team_id = t.id AND m.user_id = ?3))",
        &[js_int(organization.id), js_text(slug), js_int(actor)],
    )
    .await
}

async fn linked_repositories(
    state: &AppState,
    database: &D1Database,
    organization: &Organization,
    accepted_only: bool,
) -> Result<Vec<LinkedRepo>> {
    let [viewer, admin] = viewer_params(state);
    db::all(
        database,
        &format!(
            "SELECT l.id, u.username AS owner, r.name, l.accepted, {VISIBLE_REPO_SQL} AS can_view
             FROM organization_repositories l JOIN organizations o ON o.id = l.organization_id
             JOIN repos r ON r.id = l.repo_id JOIN users u ON u.id = r.owner_id
             WHERE o.id = ?3 AND (o.owner_id = ?1 OR (l.accepted = 1 AND {VISIBLE_REPO_SQL}))
               AND (?4 = 0 OR l.accepted = 1)
             ORDER BY l.accepted, u.username, r.name COLLATE NOCASE, r.id"
        ),
        &[
            viewer,
            admin,
            js_int(organization.id),
            js_int(i64::from(accepted_only)),
        ],
    )
    .await
}

async fn change(database: &D1Database, sql: &str, args: &[JsValue]) -> Result<bool> {
    let result = database.prepare(sql).bind(args)?.run().await?;
    result
        .meta()?
        .and_then(|meta| meta.changes)
        .map(|changes| changes > 0)
        .ok_or_else(|| Error::RustError("D1 did not report organization changes".into()))
}

fn form_id(form: &FormData, field: &str) -> Option<i64> {
    form_value(form, field)?
        .parse()
        .ok()
        .filter(|id| (1..=9_007_199_254_740_991).contains(id))
}

fn login(req: &Request) -> Result<Response> {
    redirect(&format!(
        "/user/login?next={}",
        url_path(&safe_next(Some(req.path())))
    ))
}

fn invalid(state: &AppState) -> Result<Response> {
    error(state, 400, Text::InvalidAction)
}

fn updated(state: &AppState, changed: bool, path: &str) -> Result<Response> {
    if changed {
        redirect(path)
    } else {
        error(state, 409, Text::StaleAction)
    }
}

fn error(state: &AppState, status: u16, message: Text) -> Result<Response> {
    error_response(&Chrome::new(state), status, state.locale.t(message))
}
