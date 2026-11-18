use crate::ctx::AppState;
use crate::db;
use crate::handlers::common::{VISIBLE_REPO_SQL, not_found, page_title, viewer_params};
use crate::i18n::{CommonText, Locale, NotificationAction, NotificationText};
use crate::models::Ts;
use crate::pagination::{PAGE_SIZE, PageRequest, Pagination};
use crate::templates::{Chrome, render};
use crate::util::{js_int, now, redirect};
use askama::Template;
use std::fmt;
use worker::*;

const INBOX_PATH: &str = "/user/notifications";
const LOGIN_PATH: &str = "/user/login?next=/user/notifications";
const THREAD_JOINS: &str = "JOIN issues i ON i.id = e.issue_id JOIN repos r ON r.id = i.repo_id";
const INBOX_FROM: &str = "FROM notifications n JOIN notification_events e ON e.id = n.event_id";

#[derive(serde::Deserialize)]
struct NotificationRow {
    id: i64,
    title: String,
    owner: String,
    repo: String,
    number: i64,
    kind: String,
    action: NotificationAction,
    actor: Option<String>,
    date: Ts,
    read_at: Option<Ts>,
    has_unread: i64,
}

impl NotificationRow {
    fn activity(&self, locale: &Locale) -> impl fmt::Display + '_ {
        let actor = self.actor.as_deref().unwrap_or(locale.t(CommonText::Ghost));
        locale.notification_activity(self.action, actor)
    }
}

#[derive(Template)]
#[template(path = "notifications.html")]
struct InboxPage {
    chrome: Chrome,
    title: String,
    notifications: Vec<NotificationRow>,
    pagination: Pagination,
    has_unread: bool,
}

pub async fn inbox(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        return redirect(LOGIN_PATH);
    }
    let page = PageRequest::new(&req, "page");
    let [recipient, admin] = viewer_params(state);
    // Resolve the canonical path and filter visibility before applying pagination.
    let mut notifications: Vec<NotificationRow> = db::all(
        &state.db()?,
        &format!(
            "SELECT n.event_id AS id, i.title, owner.username AS owner, r.name AS repo,
                    i.number, i.kind, e.action, actor.username AS actor, e.created_at AS date,
                    n.read_at, EXISTS (
                      SELECT 1 {INBOX_FROM} {THREAD_JOINS}
                      WHERE n.recipient_id = ?1 AND n.read_at IS NULL AND {VISIBLE_REPO_SQL}
                    ) AS has_unread
             {INBOX_FROM} {THREAD_JOINS}
             JOIN users owner ON owner.id = r.owner_id
             LEFT JOIN users actor ON actor.id = e.actor_id
             WHERE n.recipient_id = ?1 AND {VISIBLE_REPO_SQL}
             ORDER BY n.event_id DESC LIMIT ?3 OFFSET ?4"
        ),
        &[recipient, admin, js_int((PAGE_SIZE + 1) as i64), js_int(page.offset())],
    )
    .await?;
    let pagination = page.finish(&req, &mut notifications, None)?;
    let has_unread = notifications.first().is_some_and(|row| row.has_unread != 0);
    render(&InboxPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(NotificationText::Notifications)]),
        notifications,
        pagination,
        has_unread,
    })
}

fn inbox_redirect(req: &Request) -> Result<Response> {
    let page = PageRequest::new(req, "page");
    if page.number == 1 {
        redirect(INBOX_PATH)
    } else {
        redirect(&format!("{INBOX_PATH}?page={}", page.number))
    }
}

pub async fn read_one(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        return redirect(LOGIN_PATH);
    }
    let missing = || not_found(state, state.locale.t(NotificationText::NotFound));
    let Some(id) = ctx.param("id").and_then(|id| id.parse::<i64>().ok()).filter(|&id| id > 0) else {
        return missing();
    };
    let [recipient, admin] = viewer_params(state);
    // Check current access atomically with the mutation, including repeated reads.
    let marked = state.db()?.prepare(&format!(
        "UPDATE notifications SET read_at = COALESCE(read_at, ?3)
         WHERE recipient_id = ?1 AND event_id = ?4 AND EXISTS (
             SELECT 1 FROM notification_events e {THREAD_JOINS}
             WHERE e.id = notifications.event_id AND {VISIBLE_REPO_SQL}
         ) RETURNING event_id"
    ))
    .bind(&[recipient, admin, js_int(now()), js_int(id)])?
    .first::<i64>(Some("event_id"))
    .await?;
    if marked.is_none() {
        return missing();
    }
    inbox_redirect(&req)
}

pub async fn read_all(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        return redirect(LOGIN_PATH);
    }
    let [recipient, admin] = viewer_params(state);
    db::run(
        &state.db()?,
        &format!(
            "UPDATE notifications SET read_at = ?3
             WHERE recipient_id = ?1 AND read_at IS NULL AND EXISTS (
                 SELECT 1 FROM notification_events e {THREAD_JOINS}
                 WHERE e.id = notifications.event_id AND {VISIBLE_REPO_SQL}
             )"
        ),
        &[recipient, admin, js_int(now())],
    )
    .await?;
    inbox_redirect(&req)
}
