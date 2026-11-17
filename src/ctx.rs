use crate::models::User;
use worker::*;

#[derive(Clone)]
pub struct AppState {
    pub env: Env,
    pub user: Option<User>,
}

impl AppState {
    pub fn db(&self) -> Result<d1::D1Database> {
        self.env.d1("DB")
    }

    pub fn user_id(&self) -> Option<i64> {
        self.user.as_ref().map(|u| u.id)
    }

}
