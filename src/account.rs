use crate::models::{AccessToken, Ts};
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
    pub fn from_token(token: &AccessToken) -> Self {
        Self {
            id: token.id,
            name: token.name.clone(),
            scope: token.scope.clone(),
            created: token.created_at,
            last_used: token.last_used_at,
            expires: token.expires_at,
            expired: token.expires_at.is_some_and(|time| time.0 <= crate::util::now()),
        }
    }
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

#[derive(Template)]
#[template(path = "account.html")]
pub struct AccountPage {
    pub chrome: Chrome,
    pub title: String,
    pub email: String,
    pub success: Option<String>,
    pub error: Option<String>,
}
