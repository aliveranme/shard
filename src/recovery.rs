use askama::Template;
use serde::Deserialize;
use worker::*;

use crate::ctx::AppState;
use crate::i18n::{AccountText, CommonText, Locale};
use crate::templates::{error_response, render, Chrome};
use crate::{crypto, db, util};

const REQUEST_PATH: &str = "/user/forgot_password";
const RESEND_SECONDS: i64 = 300;

#[derive(Clone, Copy)]
enum Purpose {
    Verify,
    Reset,
}

impl Purpose {
    fn key(self) -> &'static str {
        match self {
            Self::Verify => "verify",
            Self::Reset => "reset",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Verify => "/user/verify_email",
            Self::Reset => "/user/reset_password",
        }
    }

    fn heading(self) -> AccountText {
        match self {
            Self::Verify => AccountText::VerifyEmail,
            Self::Reset => AccountText::ResetPassword,
        }
    }

    fn ttl(self) -> i64 {
        match self {
            Self::Verify => 86400,
            Self::Reset => 1800,
        }
    }
}

struct Mail {
    origin: String,
    sender: String,
    binding: SendEmail,
}

impl Mail {
    fn from_env(env: &Env) -> Option<Self> {
        let origin = Url::parse(&env.var("PUBLIC_ORIGIN").ok()?.to_string()).ok()?;
        let loopback = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(origin.scheme() == "https" || origin.scheme() == "http" && loopback)
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return None;
        }
        let sender = env.var("EMAIL_FROM").ok()?.to_string();
        if !util::valid_email(&sender) {
            return None;
        }
        Some(Self {
            origin: origin.origin().ascii_serialization(),
            sender,
            binding: env.send_email("EMAIL").ok()?,
        })
    }
}

pub fn configured(env: &Env) -> bool {
    Mail::from_env(env).is_some()
}

#[derive(Deserialize)]
struct Recipient {
    email: String,
}

#[derive(Deserialize)]
struct ExistingLink {
    user_id: i64,
    email: String,
    token_hash: Option<String>,
    expires_at: i64,
}

async fn send_link(
    env: &Env,
    mail: Mail,
    locale: Locale,
    purpose: Purpose,
    user_id: Option<i64>,
    email: &str,
) -> Result<()> {
    let token = crypto::random_token()?;
    let hash = crypto::sha256_hex(&token).await?;
    let now = util::now();
    let database = env.d1("DB")?;
    let previous: Option<ExistingLink> = db::first(
        &database,
        "SELECT l.user_id, l.email, l.token_hash, l.expires_at
           FROM account_email_links l
           JOIN users u ON u.id = l.user_id
          WHERE l.purpose = ?1
            AND u.email = ?2 COLLATE NOCASE
            AND ((?1 = 'verify' AND u.id = ?3) OR ?1 = 'reset')",
        &[
            util::js_text(purpose.key()),
            util::js_text(email),
            util::js_int(user_id.unwrap_or(0)),
        ],
    )
    .await?;
    let recipient: Option<Recipient> = db::first(
        &database,
        "INSERT INTO account_email_links (user_id, purpose, email, token_hash, expires_at, next_send_at)
         SELECT id, ?1, email, ?2, ?3, ?4 FROM users
          WHERE email = ?5 COLLATE NOCASE
            AND ((?1 = 'verify' AND id = ?6 AND email_verified_at IS NULL)
                 OR (?1 = 'reset' AND email_verified_at IS NOT NULL))
         ON CONFLICT (user_id, purpose) DO UPDATE SET
           email = excluded.email, token_hash = excluded.token_hash,
           expires_at = excluded.expires_at, next_send_at = excluded.next_send_at
         WHERE account_email_links.next_send_at <= ?7
           AND (?1 = 'verify'
                OR account_email_links.token_hash IS NULL
                OR account_email_links.expires_at <= ?7)
         RETURNING email",
        &[
            util::js_text(purpose.key()),
            util::js_text(&hash),
            util::js_int(now + purpose.ttl()),
            util::js_int(now + RESEND_SECONDS),
            util::js_text(email),
            util::js_int(user_id.unwrap_or(0)),
            util::js_int(now),
        ],
    )
    .await?;
    if let Some(recipient) = recipient {
        let instructions = match purpose {
            Purpose::Verify => AccountText::VerifyEmailInstructions,
            Purpose::Reset => AccountText::ResetEmailInstructions,
        };
        let text = format!(
            "{}\n\n{}{}?token={}\n\n{}\n",
            locale.t(instructions),
            mail.origin,
            purpose.path(),
            token,
            locale.t(AccountText::IgnoreEmail),
        );
        let message =
            SendEmailBuilder::builder(&mail.sender, &recipient.email, locale.t(purpose.heading()))
                .text(&text)
                .build();
        if let Err(error) = mail.binding.send_with_builder(&message).await {
            // Restore any previous active link or clear the unsent token hash and release cooldown.
            let (restored_hash, restored_expires) = match previous {
                Some(prev)
                    if prev.email == recipient.email
                        && prev.token_hash.is_some()
                        && prev.expires_at > now =>
                {
                    let _ = prev.user_id;
                    (
                        prev.token_hash
                            .as_deref()
                            .map_or_else(util::js_null, util::js_text),
                        util::js_int(prev.expires_at),
                    )
                }
                _ => (util::js_null(), util::js_int(0)),
            };
            let _ = db::run(
                &database,
                "UPDATE account_email_links
                    SET token_hash = ?1, expires_at = ?2, next_send_at = 0
                  WHERE token_hash = ?3 AND purpose = ?4",
                &[
                    restored_hash,
                    restored_expires,
                    util::js_text(&hash),
                    util::js_text(purpose.key()),
                ],
            )
            .await;
            return Err(error.into());
        }
    }
    Ok(())
}

fn unavailable(state: &AppState) -> Result<Response> {
    error_response(
        &Chrome::new(state),
        503,
        state.locale.t(AccountText::MailUnavailable),
    )
}

#[derive(Template)]
#[template(path = "recovery.html")]
struct RecoveryPage<'a> {
    chrome: Chrome,
    title: &'static str,
    heading: &'static str,
    action: &'static str,
    button: &'static str,
    notice: Option<&'static str>,
    error: Option<&'static str>,
    request_email: bool,
    reset_password: bool,
    token: &'a str,
    back_url: &'static str,
    back_label: &'static str,
}

impl<'a> RecoveryPage<'a> {
    fn new(state: &AppState, heading: AccountText) -> Self {
        let heading = state.locale.t(heading);
        Self {
            chrome: Chrome::new(state),
            title: heading,
            heading,
            action: "",
            button: "",
            notice: None,
            error: None,
            request_email: false,
            reset_password: false,
            token: "",
            back_url: "/user/login",
            back_label: state.locale.t(CommonText::SignIn),
        }
    }

    fn link_form(mut self, purpose: Purpose, token: &'a str) -> Self {
        self.action = purpose.path();
        self.button = self.heading;
        self.token = token;
        self.reset_password = matches!(purpose, Purpose::Reset);
        self
    }

    fn response(&self, status: u16) -> Result<Response> {
        let mut response = render(self)?.with_status(status);
        // Suppress token-bearing paths without nulling Origin on form POSTs.
        response
            .headers_mut()
            .set("Referrer-Policy", "strict-origin")?;
        response.headers_mut().set("Cache-Control", "no-store")?;
        Ok(response)
    }
}

pub async fn request_form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if !configured(&state.env) {
        return unavailable(state);
    }
    let mut page = RecoveryPage::new(state, AccountText::ForgotPassword);
    page.action = REQUEST_PATH;
    page.request_email = true;
    page.button = state.locale.t(AccountText::SendResetLink);
    page.response(200)
}

pub async fn request_submit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(mail) = Mail::from_env(&state.env) else {
        return unavailable(state);
    };
    let email = util::form_value(state.form()?, "email").unwrap_or_default();
    if util::valid_email(email.trim()) {
        let env = state.env.clone();
        let locale = state.locale;
        // Complete the same HTTP response independently of lookup and delivery.
        state.worker_ctx.wait_until(async move {
            if send_link(&env, mail, locale, Purpose::Reset, None, email.trim())
                .await
                .is_err()
            {
                console_error!("Account recovery email failed");
            }
        });
    }
    let mut page = RecoveryPage::new(state, AccountText::ForgotPassword);
    page.notice = Some(state.locale.t(AccountText::RecoveryRequested));
    page.response(200)
}

pub async fn request_verification(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(user) = &state.user else {
        return util::redirect("/user/login?next=/user/settings");
    };
    let Some(mail) = Mail::from_env(&state.env) else {
        return unavailable(state);
    };
    if send_link(
        &state.env,
        mail,
        state.locale,
        Purpose::Verify,
        Some(user.id),
        &user.email,
    )
    .await
    .is_err()
    {
        console_error!("Email verification send failed");
        return error_response(
            &Chrome::new(state),
            503,
            state.locale.t(AccountText::MailSendFailed),
        );
    }
    util::redirect("/user/settings?updated=verification-requested")
}

// This predicate is shared by the read-only form view and the atomic consume
// statements. Email comparison is binary because case-only changes revoke links.
const ACTIVE_LINK: &str = "SELECT l.user_id FROM account_email_links l
    JOIN users u ON u.id = l.user_id AND u.email COLLATE BINARY = l.email
    WHERE l.token_hash = ?1 AND l.purpose = ?2 AND l.expires_at > ?3
      AND ((?2 = 'verify' AND u.email_verified_at IS NULL)
           OR (?2 = 'reset' AND u.email_verified_at IS NOT NULL))";

#[derive(Deserialize)]
struct LinkOwner {
    user_id: i64,
}

fn token_shape(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn active_owner(state: &AppState, hash: &str, purpose: Purpose) -> Result<Option<LinkOwner>> {
    db::first(
        &state.db()?,
        ACTIVE_LINK,
        &[
            util::js_text(hash),
            util::js_text(purpose.key()),
            util::js_int(util::now()),
        ],
    )
    .await
}

async fn consume(
    state: &AppState,
    hash: &str,
    purpose: Purpose,
    value: wasm_bindgen::JsValue,
) -> Result<Option<LinkOwner>> {
    let column = match purpose {
        Purpose::Verify => "email_verified_at",
        Purpose::Reset => "password_hash",
    };
    let database = state.db()?;
    let args = [
        util::js_text(hash),
        util::js_text(purpose.key()),
        util::js_int(util::now()),
    ];
    let update = database
        .prepare(format!(
            "UPDATE users SET {column} = ?4 WHERE id = ({ACTIVE_LINK}) RETURNING id AS user_id"
        ))
        .bind(&[args[0].clone(), args[1].clone(), args[2].clone(), value])?;
    if matches!(purpose, Purpose::Verify) {
        return update.first(None).await;
    }
    // Revoke Git tokens before the password trigger consumes this link and deletes browser sessions.
    let revoke = database
        .prepare(format!(
            "DELETE FROM access_tokens WHERE user_id = ({ACTIVE_LINK})"
        ))
        .bind(&args)?;
    let results = database.batch(vec![revoke, update]).await?;
    Ok(results[1].results::<LinkOwner>()?.into_iter().next())
}

fn invalid_link(state: &AppState, purpose: Purpose) -> Result<Response> {
    let mut page = RecoveryPage::new(state, purpose.heading());
    page.error = Some(state.locale.t(AccountText::InvalidEmailLink));
    if matches!(purpose, Purpose::Reset) {
        page.back_url = REQUEST_PATH;
        page.back_label = state.locale.t(AccountText::ForgotPassword);
    }
    page.response(400)
}

async fn show_link(req: Request, state: &AppState, purpose: Purpose) -> Result<Response> {
    let token = util::query_param(&req, "token").unwrap_or_default();
    if !token_shape(&token)
        || active_owner(state, &crypto::sha256_hex(&token).await?, purpose)
            .await?
            .is_none()
    {
        return invalid_link(state, purpose);
    }
    RecoveryPage::new(state, purpose.heading())
        .link_form(purpose, &token)
        .response(200)
}

pub async fn verify_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    show_link(req, &ctx.data, Purpose::Verify).await
}

pub async fn reset_form(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    show_link(req, &ctx.data, Purpose::Reset).await
}

pub async fn verify_submit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let purpose = Purpose::Verify;
    let token = util::form_value(state.form()?, "token").unwrap_or_default();
    if !token_shape(&token)
        || consume(
            state,
            &crypto::sha256_hex(&token).await?,
            purpose,
            util::js_int(util::now()),
        )
        .await?
        .is_none()
    {
        return invalid_link(state, purpose);
    }
    let mut page = RecoveryPage::new(state, purpose.heading());
    page.notice = Some(state.locale.t(AccountText::EmailVerified));
    if state.user_id().is_some() {
        page.back_url = "/user/settings";
        page.back_label = state.locale.t(CommonText::AccountSettings);
    }
    page.response(200)
}

pub async fn reset_submit(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let purpose = Purpose::Reset;
    let form = state.form()?;
    let token = util::form_value(form, "token").unwrap_or_default();
    if !token_shape(&token) {
        return invalid_link(state, purpose);
    }
    let hash = crypto::sha256_hex(&token).await?;
    if active_owner(state, &hash, purpose).await?.is_none() {
        return invalid_link(state, purpose);
    }
    let password = util::form_value(form, "password").unwrap_or_default();
    let confirmation = util::form_value(form, "password_confirm").unwrap_or_default();
    let error = if password.chars().count() < 8 || password.len() > 1024 {
        Some(AccountText::InvalidPasswordLength)
    } else if password != confirmation {
        Some(AccountText::PasswordsMismatch)
    } else {
        None
    };
    if let Some(error) = error {
        let mut page = RecoveryPage::new(state, purpose.heading()).link_form(purpose, &token);
        page.error = Some(state.locale.t(error));
        return page.response(400);
    }
    let password_hash = crypto::hash_password(&password).await?;
    let Some(owner) = consume(state, &hash, purpose, util::js_text(&password_hash)).await? else {
        return invalid_link(state, purpose);
    };
    let mut page = RecoveryPage::new(state, purpose.heading());
    let signed_out = state.user_id() == Some(owner.user_id);
    if signed_out {
        page.chrome.signed_in = false;
        page.chrome.user_name.clear();
        page.chrome.avatar = Default::default();
    }
    page.notice = Some(state.locale.t(AccountText::PasswordReset));
    let mut response = page.response(200)?;
    if signed_out {
        for cookie in util::clear_cookies(util::is_secure(&req)) {
            response.headers_mut().append("Set-Cookie", &cookie)?;
        }
    }
    Ok(response)
}
