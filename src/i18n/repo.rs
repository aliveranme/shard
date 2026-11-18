catalog! {
    RepoText {
        Repository => "Repository", "仓库";
        Branch => "Branch", "分支";
        BrowseBranch => "Browse branch", "浏览分支";
        EmptyRepository => "This repository is empty", "此仓库为空";
        EmptyDirectory => "This directory is empty", "此目录为空";
        EmptyBranch => "No files on this branch", "此分支暂无文件";
        PushBranch => "Push a branch to get started:", "推送一个分支以开始使用：";
        Name => "Name", "名称";
        Size => "Size", "大小";
        About => "About", "关于";
        DefaultBranch => "Default branch", "默认分支";
        Visibility => "Visibility", "可见性";
        Updated => "Updated", "更新时间";
        Clone => "Clone", "克隆";
        CloneHelp => "Authenticate with your username and an access token. Write tokens can push.", "使用用户名和访问令牌进行身份验证。具有写入权限的令牌可以推送。";
        ManageTokens => "Manage access tokens", "管理访问令牌";
        File => "File", "文件";
        RawFile => "Raw file", "原始文件";
        BinaryFile => "Binary file", "二进制文件";
        NoPreview => "Preview is not available for this file.", "此文件无法预览。";
        DownloadRaw => "Download raw file", "下载原始文件";
        TruncatedPreview => "Preview truncated to the first 256 KiB.", "仅预览文件的前 256 KiB。";
        BackToCode => "Back to code", "返回代码";
        CommitNotFound => "Commit not found", "未找到提交";
        NoCommits => "No commits yet", "暂无提交";
        ReturnLatest => "Return to the latest commits to browse this branch.", "返回最新提交以浏览此分支。";
        PushFirstCommit => "Push a branch to create the first commit.", "推送一个分支以创建首次提交。";
        NewRepositoryIntro => "A repository becomes an Artifacts-backed Git remote on this shard.", "新仓库将由 Cloudflare Artifacts 提供 Git 存储。";
        RepositoryName => "Repository name", "仓库名称";
        NameHelp => "Letters, digits, dot, dash or underscore. Starts with a letter or digit.", "可使用字母、数字、点、连字符或下划线，须以字母或数字开头。";
        Description => "Description", "描述";
        DescriptionPlaceholder => "What is this repository for?", "此仓库的用途是什么？";
        PrivateHelp => "Private — only you can see it", "设为私有，仅你可见";
        CreateRepository => "Create repository", "创建仓库";
        Cancel => "Cancel", "取消";
        SettingsSaved => "Settings saved.", "设置已保存。";
        SaveSettings => "Save settings", "保存设置";
        RepositorySettings => "Repository settings", "仓库设置";
        General => "General", "常规";
        GeneralHelp => "Description, default branch and visibility.", "描述、默认分支和可见性。";
        GitStorage => "Git storage", "Git 存储";
        StorageHelp => "Cloudflare Artifacts stores this repository. Git clients connect through this shard's proxy.", "此仓库由 Cloudflare Artifacts 存储。Git 客户端通过本站点的代理连接。";
        DangerZone => "Danger zone", "危险操作";
        DeleteThisRepository => "Delete this repository", "删除此仓库";
        DeleteHelp => "This removes the metadata, issues, pull requests and the Artifacts repository. It cannot be undone.", "此操作将删除元数据、议题、合并请求和 Artifacts 仓库，且无法撤销。";
        DeleteRepository => "Delete repository", "删除仓库";
        InvalidName => "Repository names use letters, digits, dot, dash or underscore and start with a letter or digit.", "仓库名称只能使用字母、数字、点、连字符或下划线，并且必须以字母或数字开头。";
        DuplicateName => "You already have a repository with that name.", "你已有一个同名仓库。";
        CreateFailed => "Could not create the repository. Please try again later.", "无法创建仓库。请稍后重试。";
        StorageUnavailable => "Git storage is unavailable. Please try again later.", "Git 存储暂不可用。请稍后重试。";
        DirectoryNotFound => "Directory not found.", "未找到目录。";
        FileNotFound => "File not found.", "未找到文件。";
        InvalidCursor => "The commit cursor must be a full Git commit hash.", "提交游标必须是完整的 Git 提交哈希。";
        SettingsForbidden => "Only the repository owner can change settings.", "只有仓库所有者可以更改设置。";
        DeleteForbidden => "Only the repository owner can delete it.", "只有仓库所有者可以删除此仓库。";
        ConfirmDeletion => "Type the repository name to confirm deletion.", "请输入仓库名称以确认删除。";
        DeleteStorageFailed => "Could not delete Git storage. Please try again later.", "无法删除 Git 存储。请稍后重试。";
        DeleteFailed => "Could not delete the repository. Please try again later.", "无法删除仓库。请稍后重试。";
        SaveFailed => "Could not save settings. Please try again later.", "无法保存设置。请稍后重试。";
    }
}

use super::Locale;
use std::fmt::{self, Display};

impl Locale {
    pub fn readme_from_branch(self, branch: &str) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Rendered from the {branch} branch"),
            Self::ZhHans => write!(f, "呈现自 {branch} 分支"),
        })
    }

    pub fn first_parent_history(self, branch: &str) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "First-parent history on {branch}"),
            Self::ZhHans => write!(f, "{branch} 分支的第一父提交历史"),
        })
    }

    pub fn repository_settings_intro(self, repo: &str) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Name, visibility and storage for {repo}."),
            Self::ZhHans => write!(f, "{repo} 的名称、可见性和存储信息。"),
        })
    }

    pub fn confirm_repository_name(self, repo: &str) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Type {repo} to confirm"),
            Self::ZhHans => write!(f, "输入 {repo} 以确认"),
        })
    }
}
