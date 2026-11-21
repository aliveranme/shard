use crate::i18n::{AccountText, LanguagePreference, Locale};
use crate::models::User;
use std::rc::Rc;
use worker::*;

pub const THEME_COOKIE: &str = "shard_theme";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    Auto,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn attr(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    pub const fn label_key(self) -> AccountText {
        match self {
            Self::Auto => AccountText::ThemeSystem,
            Self::Light => AccountText::ThemeLight,
            Self::Dark => AccountText::ThemeDark,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub env: Env,
    pub worker_ctx: Rc<Context>,
    pub user: Option<User>,
    pub locale: Locale,
    pub language_preference: LanguagePreference,
    pub theme_preference: ThemePreference,
    pub csrf_token: String,
    pub registration_open: bool,
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
