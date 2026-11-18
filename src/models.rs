#![allow(dead_code)]
use serde::Deserialize;
use std::fmt;
use wasm_bindgen::JsValue;

/// Unix seconds, rendered as a date in templates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Ts(pub i64);

impl Ts {
    pub fn relative(&self) -> String {
        let now = (js_sys::Date::now() / 1000.0) as i64;
        let diff = now - self.0;
        match diff {
            d if d < 0 => "just now".into(),
            d if d < 60 => "just now".into(),
            d if d < 3600 => format!("{} min ago", d / 60),
            d if d < 86_400 => format!("{} h ago", d / 3600),
            d if d < 2_592_000 => {
                let days = d / 86_400;
                if days == 1 {
                    "1 day ago".into()
                } else {
                    format!("{days} days ago")
                }
            }
            _ => self.to_string(),
        }
    }
}

impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = js_sys::Date::new(&JsValue::from_f64((self.0 * 1000) as f64));
        let iso: String = date.to_iso_string().into();
        // "2026-11-17T12:34:56.000Z" -> "2026-11-17"
        write!(f, "{}", iso.split('T').next().unwrap_or(""))
    }
}

#[derive(Clone, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub is_admin: i64,
    pub created_at: Ts,
}

#[derive(Clone, Deserialize)]
pub struct UserAuth {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub is_admin: i64,
    pub created_at: Ts,
}

impl UserAuth {
    pub fn public(&self) -> User {
        User {
            id: self.id,
            username: self.username.clone(),
            email: self.email.clone(),
            is_admin: self.is_admin,
            created_at: self.created_at,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Repo {
    pub id: i64,
    pub owner_id: i64,
    pub owner_name: String,
    pub name: String,
    pub description: String,
    pub default_branch: String,
    pub artifacts_repo: String,
    pub visibility: String,
    pub next_issue_number: i64,
    pub forked_from: Option<String>,
    pub created_at: Ts,
    pub updated_at: Ts,
    pub open_issues: i64,
    pub open_pulls: i64,
}

impl Repo {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner_name, self.name)
    }

    pub fn is_private(&self) -> bool {
        self.visibility == "private"
    }

    pub fn number_of(&self) -> String {
        // Stable numeric suffix of the Artifacts repo name, useful for debugging.
        self.artifacts_repo.clone()
    }
}

#[derive(Clone, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author_id: Option<i64>,
    pub author_name: Option<String>,
    pub created_at: Ts,
    pub updated_at: Ts,
    pub closed_at: Option<Ts>,
    pub comment_count: i64,
}

impl Issue {
    pub fn is_pull(&self) -> bool {
        self.kind == "pull"
    }

    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    pub fn author(&self) -> &str {
        self.author_name.as_deref().unwrap_or("ghost")
    }
}


#[derive(Clone, Deserialize)]
pub struct Pull {
    pub issue_id: i64,
    pub head_branch: String,
    pub base_branch: String,
    pub head_commit: Option<String>,
    pub base_commit: Option<String>,
    pub merged: i64,
    pub merged_at: Option<Ts>,
    pub merge_commit: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct AccessToken {
    pub id: i64,
    pub name: String,
    pub scope: String,
    pub created_at: Ts,
    pub last_used_at: Option<Ts>,
    pub expires_at: Option<Ts>,
}

/// Authenticated git caller resolved from an HTTP basic/bearer credential.
pub struct GitIdentity {
    pub user_id: i64,
    pub username: String,
    pub scope: String,
    pub is_admin: i64,
}
