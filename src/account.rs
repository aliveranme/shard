use crate::i18n::Locale;
use crate::models::{AccessToken, Ts};
use crate::pagination::Pagination;
use crate::templates::Chrome;
use askama::Template;

pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub scope: String,
    pub created: Ts,
    pub last_used: Option<Ts>,
    pub expires: Option<Ts>,
    pub expired: bool,
}

impl TokenRow {
    pub fn from_token(token: AccessToken, now: i64) -> Self {
        Self {
            id: token.id,
            name: token.name,
            scope: token.scope,
            created: token.created_at,
            last_used: token.last_used_at,
            expires: token.expires_at,
            expired: token.expires_at.is_some_and(|time| time.0 <= now),
        }
    }
}

#[derive(Template)]
#[template(path = "tokens.html")]
pub struct TokensPage {
    pub chrome: Chrome,
    pub title: String,
    pub tokens: Vec<TokenRow>,
    pub pagination: Pagination,
    pub new_token: Option<String>,
    pub error: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "account.html")]
pub struct AccountPage {
    pub chrome: Chrome,
    pub title: String,
    pub email: String,
    pub email_verified: bool,
    pub mail_configured: bool,
    pub success: Option<&'static str>,
    pub error: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "language.html")]
pub struct LanguagePage {
    pub chrome: Chrome,
    pub title: String,
    pub preference: &'static str,
    pub languages: &'static [Locale],
    pub saved: bool,
}

#[derive(Template)]
#[template(path = "appearance.html")]
pub struct AppearancePage {
    pub chrome: Chrome,
    pub title: String,
    pub preference: &'static str,
    pub saved: bool,
}
