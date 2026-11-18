catalog! {
    WebText {
        Dashboard => "Dashboard", "工作台";
        ExploreRepositories => "Explore repositories", "探索仓库";
        ExploreIntro => "Public projects hosted on this shard.", "此站点托管的公开项目。";
        SearchRepositories => "Search repositories", "搜索仓库";
        SearchRepositoryHint => "Search by name, owner or description", "按名称、所有者或描述搜索";
        PublicRepositories => "Public repositories", "公开仓库";
        NoMatchingRepositories => "No matching repositories", "没有匹配的仓库";
        TryAnotherSearch => "Try a different search.", "请尝试更换搜索关键词。";
        WorkHeadline => "Your work, at the edge.", "边缘上的开发空间";
        WorkIntro => "Manage your repositories, issues and pull requests in one place.", "在同一处管理仓库、议题和合并请求。";
        GuestEyebrow => "Self-hosted Git on Cloudflare", "在 Cloudflare 自托管 Git";
        GuestHeadline => "Ship code. Keep the forge.", "自主托管代码";
        GuestIntro => "A self-hosted Git forge for repositories, issues and pull requests.", "支持仓库、议题和合并请求的自托管 Git 平台。";
        RecentProjects => "Recently updated projects", "最近更新的项目";
        RecentRepositoriesIntro => "Browse your repositories and public projects, most recently updated first.", "浏览你的仓库和公开项目，最近更新的项目在前。";
        PublicProjectsIntro => "Browse public projects, most recently updated first.", "浏览公开项目，最近更新的项目在前。";
        EmptyRepositoryPage => "No repositories on this page", "此页没有仓库";
        NothingHere => "Nothing here yet", "这里暂时没有内容";
        PreviousRepositoryPage => "Go to the previous page to keep browsing.", "返回上一页继续浏览。";
        CreateFirstRepository => "Create your first repository and push a branch.", "创建第一个仓库并推送分支。";
        SignInToCreateRepository => "Sign in to create the first repository.", "登录后即可创建第一个仓库。";
        You => "You", "你";
        Repositories => "Repositories", "仓库";
        NoRepositories => "No repositories yet", "暂无仓库";
        PushFirstRepository => "Create your first repository to make it appear here.", "创建第一个仓库后，它将显示在这里。";
        NothingPublished => "This person has not published anything yet.", "该用户还没有公开仓库。";
        UserNotFound => "User not found.", "未找到该用户。";
        GlobalIssues => "Issues across your projects", "项目议题";
        GlobalPulls => "Pull requests across your projects", "项目合并请求";
        GlobalDiscussionsIntro => "Work across the repositories you can read.", "查看有权访问的仓库中的讨论。";
        NoMatchingDiscussions => "No matching entries", "没有匹配的条目";
        TryAnotherFilter => "Try another state, search or page.", "请尝试更换状态、搜索词或页码。";
    }
}

use super::Locale;
use crate::models::Ts;
use std::fmt::{self, Display};

impl Locale {
    pub fn welcome<'a>(self, name: &'a str) -> impl Display + 'a {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Welcome back, {name}"),
            Self::ZhHans => write!(f, "欢迎回来，{name}"),
        })
    }

    pub fn member_since(self, joined: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Member since {joined}"),
            Self::ZhHans => write!(f, "加入于 {joined}"),
        })
    }
}
