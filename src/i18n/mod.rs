macro_rules! catalog {
    ($name:ident { $($key:ident => $en:literal, $zh:literal;)* }) => {
        #[derive(Clone, Copy)]
        pub enum $name { $($key,)* }

        impl super::Message for $name {
            fn text(self, locale: super::Locale) -> &'static str {
                match locale {
                    super::Locale::En => match self { $(Self::$key => $en,)* },
                    super::Locale::ZhHans => match self { $(Self::$key => $zh,)* },
                }
            }
        }
    };
}

mod account;
mod checks;
mod branches;
mod collaborators;
mod common;
mod discussion;
mod issue_metadata;
mod gpg;
mod lifecycle;
mod notifications;
mod organizations;
mod repo;
mod repo_features;
mod repo_objects;
mod search;
mod web;
mod webhooks;

pub use account::AccountText;
pub use branches::BranchText;
pub use checks::CheckText;
pub use collaborators::CollaboratorText;
pub use common::CommonText;
pub use discussion::DiscussionText;
pub use gpg::GpgText;
pub use issue_metadata::IssueMetadataText;
pub use lifecycle::LifecycleText;
pub use notifications::{NotificationAction, NotificationText};
pub use organizations::OrganizationText;
pub use repo::RepoText;
pub use repo_features::RepoFeatureText;
pub use repo_objects::RepoObjectText;
pub use search::SearchText;
pub use web::WebText;
pub use webhooks::WebhookText;

pub const LANGUAGE_COOKIE: &str = "shard_lang";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    En,
    ZhHans,
}

pub trait Message {
    fn text(self, locale: Locale) -> &'static str;
}

impl Locale {
    pub const SUPPORTED: [Self; 2] = [Self::En, Self::ZhHans];

    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhHans => "zh-Hans",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhHans => "简体中文",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::SUPPORTED.into_iter().find(|locale| locale.code() == code)
    }

    pub fn t(self, key: impl Message) -> &'static str {
        key.text(self)
    }

    fn accepts(self, tag: &str) -> bool {
        let mut parts = tag.split('-');
        let primary = parts.next().unwrap_or_default();
        match self {
            Self::En => primary.eq_ignore_ascii_case("en"),
            Self::ZhHans => primary.eq_ignore_ascii_case("zh")
                && parts.next().is_none_or(|part| {
                    ["Hans", "CN", "SG", "MY"].iter().any(|alias| part.eq_ignore_ascii_case(alias))
                }),
        }
    }

    /// A supported explicit range takes precedence over the wildcard, including q=0.
    /// Rank acceptable languages by quality, then by their order in the browser header.
    pub fn negotiate(header: &str) -> Self {
        let mut wildcard = None;
        let mut explicit = [None; Self::SUPPORTED.len()];
        for (position, range) in header.split(',').enumerate() {
            let (tag, quality) = match range.split_once(';') {
                Some((tag, field)) => {
                    let Some(q) = field.trim().strip_prefix("q=").and_then(quality) else { continue };
                    (tag.trim(), q)
                }
                None => (range.trim(), 1000),
            };
            let rank = (quality, std::cmp::Reverse(position));
            if tag == "*" {
                wildcard = wildcard.max(Some(rank));
            } else if let Some(index) = Self::SUPPORTED.iter().position(|locale| locale.accepts(tag)) {
                explicit[index] = explicit[index].max(Some(rank));
            }
        }
        Self::SUPPORTED
            .into_iter()
            .zip(explicit)
            .filter_map(|(locale, rank)| rank.or(wildcard).filter(|(q, _)| *q > 0).map(|rank| (locale, rank)))
            .enumerate()
            .max_by_key(|(index, (_, rank))| (*rank, std::cmp::Reverse(*index)))
            .map(|(_, (locale, _))| locale)
            .unwrap_or_default()
    }
}

fn quality(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    match whole {
        "1" if fraction.bytes().all(|digit| digit == b'0') => Some(1000),
        "0" => Some(fraction.bytes().zip([100, 10, 1]).map(|(digit, place)| {
            u16::from(digit - b'0') * place
        }).sum()),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub enum LanguagePreference {
    Browser,
    Explicit(Locale),
}

impl LanguagePreference {
    pub fn parse(value: &str) -> Option<Self> {
        if value == "auto" { Some(Self::Browser) } else { Locale::from_code(value).map(Self::Explicit) }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Browser => "auto",
            Self::Explicit(locale) => locale.code(),
        }
    }

    pub fn locale(self) -> Option<Locale> {
        match self {
            Self::Browser => None,
            Self::Explicit(locale) => Some(locale),
        }
    }
}

