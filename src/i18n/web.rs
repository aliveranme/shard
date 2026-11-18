catalog! {
    WebText {
        Dashboard => "Dashboard", "工作台";
        ExploreRepositories => "Explore repositories", "探索仓库";
        ExploreIntro => "Public projects hosted on this Shard instance", "此 Shard 实例托管的公开项目";
        SearchRepositories => "Search repositories", "搜索仓库";
        SearchRepositoryHint => "Name, owner or description", "名称、所有者或描述";
        PublicRepositories => "Public repositories", "公开仓库";
        NoMatchingRepositories => "No matching repositories", "没有匹配的仓库";
        TryAnotherSearch => "Try a different search", "请尝试更换搜索关键词";
        WorkHeadline => "Your repositories", "你的仓库";
        WorkIntro => "Manage your repositories, issues and pull requests", "管理仓库、议题和合并请求";
        GuestEyebrow => "Runs on Cloudflare Workers", "运行于 Cloudflare Workers";
        GuestHeadline => "Self-hosted Git for small teams", "小团队的自托管 Git 平台";
        GuestIntro => "Host repositories, track issues and review changes on your own Shard instance", "在自己的 Shard 实例上托管仓库、跟踪议题和审查代码";
        ReviewExample => "Review example", "审查示例";
        ReviewExampleTitle => "Document the local setup", "补充本地搭建说明";
        ReviewExampleComment => "Install the pinned toolchain before starting the Worker", "启动 Worker 前先安装指定版本的工具链";
        RecentProjects => "Recently updated projects", "最近更新的项目";
        RecentRepositoriesIntro => "Your repositories and public projects, most recently updated first", "你的仓库和公开项目，按最近更新排序";
        PublicProjectsIntro => "Public repositories, most recently updated first", "公开仓库，按最近更新排序";
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
    pub fn member_since(self, joined: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Member since {joined}"),
            Self::ZhHans => write!(f, "加入于 {joined}"),
        })
    }
}
