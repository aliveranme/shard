catalog! {
    RepoText {
        Repository => "Repository", "仓库";
        Branch => "Branch", "分支";
        BrowseBranch => "Browse branch", "浏览分支";
        BranchesAndTags => "Branches and tags", "分支和标签";
        Branches => "Branches", "分支";
        Tags => "Tags", "标签";
        NoBranches => "No branches have been pushed", "尚未推送分支";
        NoTags => "No tags have been pushed", "尚未推送标签";
        TagObject => "Git object", "Git 对象";
        PeeledTarget => "Target object", "目标对象";
        RefsLimit => "The Git reference advertisement exceeds the size or record limit. Use Git to list the references.", "Git 引用公告超出大小或记录数量限制。请使用 Git 列出引用。";
        EmptyRepository => "This repository is empty", "此仓库为空";
        EmptyDirectory => "This directory is empty", "此目录为空";
        EmptyBranch => "No files on this branch", "此分支暂无文件";
        PushBranch => "Push a branch to get started:", "推送一个分支以开始使用：";
        Name => "Name", "名称";
        Size => "Size", "大小";
        About => "About", "关于";
        DefaultBranch => "Default branch", "默认分支";
        DefaultBranchHelp => "Used for browsing, new pull requests and Git clones through Shard.", "用于网页浏览、新合并请求以及通过 Shard 进行 Git 克隆。";
        DefaultBranchNotFound => "Push the branch to this repository before setting it as the default.", "请先将该分支推送到此仓库，再将它设为默认分支。";
        SyncGitHead => "Save and sync Git HEAD", "保存并同步 Git HEAD";
        GitHeadUnavailable => "Git storage could not update HEAD. Try saving the branch again.", "Git 存储未能更新 HEAD。请重试保存默认分支。";
        Visibility => "Visibility", "可见性";
        Updated => "Updated", "更新时间";
        Clone => "Clone", "克隆";
        CloneHelp => "Use your access token as the Git password. Pushing requires repository write access and a write token.", "请使用访问令牌作为 Git 密码。推送需要仓库写入权限及写入令牌。";
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
        NewRepositoryIntro => "Create a Git repository on this Shard instance.", "在此 Shard 实例上创建 Git 仓库。";
        RepositoryName => "Repository name", "仓库名称";
        NameHelp => "Letters, digits, dot, dash or underscore. Starts with a letter or digit.", "可使用字母、数字、点、连字符或下划线，须以字母或数字开头。";
        Description => "Description", "描述";
        DescriptionPlaceholder => "What is this repository for?", "此仓库的用途是什么？";
        PrivateHelp => "Private — accessible only to authorized users", "设为私有，仅授权用户可访问";
        CreateRepository => "Create repository", "创建仓库";
        Cancel => "Cancel", "取消";
        SettingsSaved => "Settings saved.", "设置已保存。";
        SaveSettings => "Save settings", "保存设置";
        RepositorySettings => "Repository settings", "仓库设置";
        General => "General", "常规";
        GeneralHelp => "Description, default branch and visibility.", "描述、默认分支和可见性。";
        GitStorage => "Git storage", "Git 存储";
        ArtifactsStorage => "Cloudflare Artifacts", "Cloudflare Artifacts";
        NativeGitStorage => "Native Git origin", "原生 Git 源站";
        DurableGitStorage => "Cloudflare Durable Objects", "Cloudflare Durable Objects";
        StorageHelp => "Git clients connect through this Shard instance.", "Git 客户端通过此 Shard 实例连接。";
        DangerZone => "Danger zone", "危险操作";
        DeleteThisRepository => "Delete this repository", "删除此仓库";
        DeleteHelp => "This permanently removes the repository, its Git data, issues and pull requests.", "此操作将永久删除仓库、Git 数据、议题和合并请求。";
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
