use std::rc::Rc;
use crate::i18n::{LanguagePreference, Locale};
use crate::models::User;
use worker::*;

#[derive(Clone)]
pub struct AppState {
    pub env: Env,
    pub worker_ctx: Rc<Context>,
    pub user: Option<User>,
    pub locale: Locale,
    pub language_preference: LanguagePreference,
    pub csrf_token: String,
    pub form: Option<Rc<FormData>>,
}

impl AppState {
    pub fn db(&self) -> Result<d1::D1Database> {
        self.env.d1("DB")
    }

    pub fn user_id(&self) -> Option<i64> {
        self.user.as_ref().map(|u| u.id)
    }

    pub fn form(&self) -> Result<&FormData> {
        self.form
            .as_deref()
            .ok_or_else(|| Error::RustError("missing validated form data".into()))
    }

}
