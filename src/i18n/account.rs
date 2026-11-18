catalog! {
    AccountText {
        Account => "Account", "账号";
        AccountIntro => "Manage your email and password.", "管理邮箱和密码。";
        EmailAddress => "Email address", "电子邮箱";
        Email => "Email", "邮箱";
        CurrentPassword => "Current password", "当前密码";
        UpdateEmail => "Update email", "更新邮箱";
        ChangePassword => "Change password", "修改密码";
        PasswordHelp => "Use at least 8 characters. Changing your password signs out all other browser sessions.", "请至少使用 8 个字符。修改密码后，其他浏览器会话将退出登录。";
        NewPassword => "New password", "新密码";
        ConfirmPassword => "Confirm new password", "确认新密码";
        UpdatePassword => "Update password", "更新密码";
        EmailUpdated => "Your email address has been updated.", "邮箱已更新。";
        PasswordUpdated => "Your password has been updated. All other browser sessions have been signed out.", "密码已更新，其他浏览器会话已退出登录。";
        DeleteAccount => "Delete account", "删除账号";
        DeleteAccountHelp => "Transfer or delete repositories you own before deleting your account. Your contributions remain with anonymous attribution.", "删除账号前，请转让或删除你拥有的仓库。你的贡献将保留并显示为匿名。";
        ConfirmAccountDeletion => "Type your exact username to confirm account deletion.", "请输入你的完整用户名以确认删除账号。";
        AccountDeletionBlocked => "Account deletion requires a current password and no owned repositories or unfinished copies.", "删除账号需要当前密码，且不得拥有仓库或未完成的复制任务。";
        EmailVerified => "Email address verified.", "邮箱已验证。";
        EmailUnverified => "Email address is unverified.", "邮箱尚未验证。";
        VerifyEmail => "Verify email", "验证邮箱";
        SendVerification => "Send verification link", "发送验证链接";
        VerificationRequested => "Check your inbox for a verification link. If you just requested one, use the link already sent.", "请查收验证邮件。如果刚刚请求过，请使用已发送的链接。";
        VerifyEmailInstructions => "Open the link below to confirm your Shard email address. The link expires in 24 hours.", "请打开以下链接确认你的 Shard 邮箱地址。链接将在 24 小时后失效。";
        ForgotPassword => "Forgot password?", "忘记密码？";
        ResetPassword => "Reset password", "重置密码";
        SendResetLink => "Send reset link", "发送重置链接";
        ResetEmailInstructions => "Open the link below to reset your Shard password. The link expires in 30 minutes.", "请打开以下链接重置你的 Shard 密码。链接将在 30 分钟后失效。";
        IgnoreEmail => "If you did not request this email, you can ignore it.", "如果你没有请求此邮件，可以忽略它。";
        RecoveryRequested => "If this address is eligible for recovery, you will receive a password reset link.", "如果此邮箱可以用于找回账号，你将收到密码重置链接。";
        InvalidEmailLink => "This link is invalid, expired, or already used.", "此链接无效、已过期或已使用。";
        MailUnavailable => "Account email is unavailable on this instance.", "此站点未启用账号邮件功能。";
        MailSendFailed => "The verification email could not be sent. Try again in a few minutes.", "无法发送验证邮件，请几分钟后重试。";
        PasswordReset => "Your password has been reset. Sign in with your new password.", "密码已重置。请使用新密码登录。";
        Register => "Register", "注册";
        CreateYourAccount => "Create your account", "创建账号";
        SignInIntro => "Welcome back. Your repositories are waiting.", "欢迎回来，登录后即可访问你的仓库。";
        SignupIntro => "One account for every repository on this shard.", "使用一个账号访问此 Shard 站点的所有仓库。";
        Username => "Username", "用户名";
        UsernameOrEmail => "Username or email", "用户名或邮箱";
        Password => "Password", "密码";
        PasswordMinimum => "At least 8 characters.", "至少 8 个字符。";
        NewHere => "New to Shard?", "第一次使用 Shard？";
        AlreadyRegistered => "Already have an account?", "已有账号？";
        InvalidLoginLength => "Invalid login or password length.", "用户名、邮箱或密码长度不符合要求。";
        LoginThrottled => "Too many failed sign-in attempts. Try again later.", "登录失败次数过多，请稍后再试。";
        IncorrectCredentials => "Incorrect username or password.", "用户名或密码错误。";
        InvalidUsername => "Usernames must contain 2 to 100 letters, digits, dots, dashes or underscores.", "用户名必须包含 2 到 100 个字母、数字、点、连字符或下划线。";
        ReservedUsername => "That username is reserved. Choose another username.", "该用户名为保留名称，请另选一个。";
        InvalidEmail => "Enter a valid email address.", "请输入有效的邮箱地址。";
        InvalidPasswordLength => "Passwords need 8 to 1024 characters.", "密码长度必须为 8 到 1024 个字符。";
        SignupTaken => "That username or email is already registered.", "该用户名或邮箱已注册。";
        IncorrectCurrentPassword => "Your current password is incorrect.", "当前密码错误。";
        EmailTaken => "That email address is already registered.", "该邮箱地址已注册。";
        PasswordsMismatch => "The new passwords do not match.", "两次输入的新密码不一致。";
        InterfaceLanguage => "Interface language", "界面语言";
        LanguageIntro => "Choose the language used throughout the interface.", "选择整个站点界面使用的语言。";
        LanguageAccountStorage => "Your preference is saved to your account and used when you sign in.", "偏好将保存在账号中，并在登录后生效。";
        LanguageBrowserStorage => "Your preference is saved in this browser.", "偏好将保存在此浏览器中。";
        FollowBrowser => "Use browser language", "跟随浏览器语言";
        SaveLanguage => "Save language", "保存语言";
        LanguageSaved => "Language preference saved.", "语言偏好已保存。";
        InvalidLanguage => "Choose a supported interface language.", "请选择受支持的界面语言。";
        TokensIntro => "Use a token as the password when Git asks for credentials.", "Git 请求凭据时，请将令牌作为密码。";
        TokenCreated => "New token created.", "令牌已创建。";
        TokenCopyNow => "Copy it now — it will not be shown again.", "请立即复制，之后无法再查看。";
        GenerateAToken => "Generate a token", "生成访问令牌";
        TokenScopeHelp => "Write tokens can push to repositories you have write access to. Read tokens only clone and fetch.", "写入令牌可推送到你有写入权限的仓库。读取令牌仅可克隆和拉取。";
        TokenName => "Token name", "令牌名称";
        TokenNameExample => "laptop", "笔记本电脑";
        TokenScope => "Scope", "权限";
        TokenWriteOption => "Write — clone, fetch, push", "写入（克隆、拉取、推送）";
        TokenReadOption => "Read — clone, fetch", "读取（克隆、拉取）";
        ExpiresAfter => "Expires after", "有效期";
        GenerateToken => "Generate token", "生成令牌";
        GitQuickstart => "Git quickstart", "Git 快速入门";
        GitCredentials => "When Git asks for credentials, enter your username and use the token as the password.", "Git 请求凭据时，请输入用户名，并使用令牌作为密码。";
        YourTokens => "Your tokens", "你的令牌";
        NoTokens => "No tokens yet", "暂无令牌";
        GenerateFirstToken => "Generate one above to push from your machine.", "要从本机推送，请先在上方生成令牌。";
        WriteScope => "write scope", "写入权限";
        ReadScope => "read scope", "读取权限";
        NeverUsed => "never used", "从未使用";
        NoExpiry => "no expiry", "永不过期";
        Revoke => "Revoke", "撤销";
        InvalidTokenExpiry => "Choose a valid token expiry.", "请选择有效的令牌有效期。";
        InvalidTokenNameOrScope => "Use a name of 1 to 100 characters and choose read or write scope.", "请输入 1 到 100 个字符的名称，并选择读取或写入权限。";
    }
}

use super::Locale;
use crate::models::Ts;
use std::fmt::{self, Display};

impl Locale {
    pub fn token_days(self, count: u16) -> impl Display {
        fmt::from_fn(move |f| match self {
            Self::En if count == 1 => f.write_str("1 day"),
            Self::En => write!(f, "{count} days"),
            Self::ZhHans => write!(f, "{count} 天"),
        })
    }

    pub fn token_created(self, date: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "created {date}"),
            Self::ZhHans => write!(f, "创建于 {date}"),
        })
    }

    pub fn token_last_used(self, date: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "last used {}", date.relative(&self)),
            Self::ZhHans => write!(f, "上次使用：{}", date.relative(&self)),
        })
    }

    pub fn token_expiry<'a>(self, date: &'a Ts, expired: &bool) -> impl Display + 'a {
        let expired = *expired;
        fmt::from_fn(move |f| match (self, expired) {
            (Self::En, false) => write!(f, "expires {date}"),
            (Self::En, true) => write!(f, "expired {date}"),
            (Self::ZhHans, false) => write!(f, "将于 {date} 过期"),
            (Self::ZhHans, true) => write!(f, "已于 {date} 过期"),
        })
    }
}
