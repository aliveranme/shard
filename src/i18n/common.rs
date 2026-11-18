use super::Locale;
use crate::models::Ts;
use std::fmt::{self, Display};

catalog! {
    CommonText {
        SkipToContent => "Skip to content", "跳转到正文";
        ShardHome => "Shard home", "Shard 首页";
        Primary => "Primary", "主导航";
        Explore => "Explore", "探索";
        Issues => "Issues", "议题";
        PullRequests => "Pull requests", "合并请求";
        NewRepository => "New repository", "新建仓库";
        AccountSettings => "Account settings", "账号设置";
        AccessTokens => "Access tokens", "访问令牌";
        SignOut => "Sign out", "退出登录";
        SignIn => "Sign in", "登录";
        CreateAccount => "Create account", "创建账号";
        HostedOn => "Self-hosted on Cloudflare Workers, D1 and Artifacts", "基于 Cloudflare Workers、D1 与 Artifacts 自托管";
        Language => "Language", "语言";
        Search => "Search", "搜索";
        SearchByTitle => "Search by title", "按标题搜索";
        Private => "Private", "私有";
        Public => "Public", "公开";
        Open => "Open", "开启";
        Closed => "Closed", "已关闭";
        All => "All", "全部";
        State => "State", "状态";
        NoDescription => "No description yet.", "暂无描述。";
        MarkdownSupported => "Markdown is supported", "支持 Markdown";
        Ghost => "ghost", "已删除的用户";
        Commits => "Commits", "提交";
        Settings => "Settings", "设置";
        Code => "Code", "代码";
        LatestCommits => "Latest commits", "最新提交";
        OlderCommits => "Older commits", "更早的提交";
        Pagination => "Pagination", "分页";
        Previous => "Previous", "上一页";
        Next => "Next", "下一页";
        JustNow => "just now", "刚刚";
        RepositoryNotFound => "Repository not found.", "未找到仓库。";
        NotFound => "Not found.", "找不到此页面。";
        MethodNotAllowed => "This page does not support that request method.", "此页面不支持该请求方式。";
        ErrorHeading => "Something went sideways", "请求未能完成";
        BackToDashboard => "Back to your dashboard", "返回首页";
        AccountUnavailable => "Account storage is temporarily unavailable. Please try again.", "账号服务暂时不可用，请稍后重试。";
        FormOrigin => "This form must be submitted from this shard.", "请在当前 Shard 站点提交此表单。";
        FormExpired => "This form has expired. Reload the page and try again.", "此表单已过期，请刷新页面后重试。";
        UnreadableForm => "The submitted form could not be read.", "无法读取提交的表单。";
        WorkerError => "Something went wrong. The request was logged.", "服务发生错误，请求已记录。";
    }
}

impl Locale {
    pub fn page_number(self, page: &u32) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Page {page}"),
            Self::ZhHans => write!(f, "第 {page} 页"),
        })
    }

    pub fn open_issue_count(self, count: &i64) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En if *count == 1 => f.write_str("1 open issue"),
            Self::En => write!(f, "{count} open issues"),
            Self::ZhHans => write!(f, "{count} 个开启的议题"),
        })
    }

    pub fn open_pull_count(self, count: &i64) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En if *count == 1 => f.write_str("1 open pull request"),
            Self::En => write!(f, "{count} open pull requests"),
            Self::ZhHans => write!(f, "{count} 个开启的合并请求"),
        })
    }

    pub fn open_count(self, count: &i64) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "{count} open"),
            Self::ZhHans => write!(f, "{count} 个开启"),
        })
    }

    pub fn updated(self, date: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Updated {}", date.relative(&self)),
            Self::ZhHans => write!(f, "更新于{}", date.relative(&self)),
        })
    }
}
