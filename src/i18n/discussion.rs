catalog! {
    DiscussionText {
        Issue => "issue", "议题";
        PullRequest => "pull request", "合并请求";
        IssueListIntro => "Track bugs, ideas and decisions next to the code.", "在代码仓库中跟踪缺陷、想法和决策。";
        PullListIntro => "Review changes before they reach the base branch.", "在变更合入目标分支前进行审查。";
        NewIssue => "New issue", "新建议题";
        NewPullRequest => "New pull request", "新建合并请求";
        NothingHere => "Nothing here", "暂无内容";
        NoMatchesOnPage => "No matching entries on this page.", "本页没有匹配的条目。";
        NoOpenIssues => "No open issues.", "没有未关闭的议题。";
        NoOpenPullRequests => "No open pull requests.", "没有未关闭的合并请求。";
        NoMatches => "No matching entries.", "没有匹配的条目。";
        NewIssueIntro => "Describe the problem or idea in Markdown.", "使用 Markdown 描述问题或想法。";
        NewPullIntro => "Compare two branches already pushed to this repository.", "比较已推送到此仓库的两个分支。";
        Title => "Title", "标题";
        IssueTitleExample => "Something is broken", "某个功能无法正常使用";
        PullTitleExample => "Improve the README", "改进 README";
        BaseBranch => "Base branch", "目标分支";
        HeadBranch => "Head branch", "源分支";
        BranchesMustExist => "Both branches must already exist in this repository.", "两个分支都必须已存在于此仓库中。";
        Description => "Description", "描述";
        CreateIssue => "Create issue", "创建议题";
        CreatePullRequest => "Create pull request", "创建合并请求";
        Cancel => "Cancel", "取消";
        TitleRequired => "A title is required.", "请填写标题。";
        ChooseHeadBranch => "Choose the branch that contains your changes.", "请选择包含你的变更的源分支。";
        BranchesMustDiffer => "Head and base branches must differ.", "源分支和目标分支不能相同。";
        GitUnavailable => "Git storage is unavailable.", "Git 存储暂不可用。";
        ReadHeadBranchFailed => "Could not read the head branch.", "无法读取源分支。";
        ReadBaseBranchFailed => "Could not read the base branch.", "无法读取目标分支。";
        AllocateNumberFailed => "Could not allocate an issue number.", "无法分配编号。";
        StateChangeForbidden => "Only the author or a repository owner can change this state.", "只有作者或仓库所有者可以更改此状态。";
        InvalidState => "State must be open or closed.", "状态只能是开启或已关闭。";
        Merged => "Merged", "已合并";
        CloseIssue => "Close issue", "关闭议题";
        ClosePullRequest => "Close pull request", "关闭合并请求";
        Reopen => "Reopen", "重新开启";
        ClosedAction => "closed", "关闭";
        ReopenedAction => "reopened", "重新开启";
        MergeFrom => "Merge", "将";
        MergeInto => "into", "合并到";
        HeadCommit => "Head commit", "源分支提交";
        BaseCommit => "Base commit", "目标分支提交";
        Discussion => "Discussion", "讨论";
        LeaveComment => "Leave a comment", "发表评论";
        Comment => "Comment", "发表评论";
        JoinDiscussion => "Join the discussion", "参与讨论";
        SignInToCommentOnIssue => "Sign in to comment on this issue.", "登录后即可在此议题下发表评论。";
        SignInToCommentOnPull => "Sign in to comment on this pull request.", "登录后即可在此合并请求下发表评论。";
        Details => "Details", "详情";
        Author => "Author", "作者";
        Opened => "Opened", "创建时间";
        Comments => "Comments", "评论";
        FilesChanged => "Files changed", "变更文件";
        CompareBranchesFailed => "Git storage could not read the saved comparison.", "Git 存储无法读取已保存的对比。";
        MissingSnapshot => "The saved comparison commits are unavailable.", "已保存的对比提交不可用。";
        ComparisonLimited => "This comparison exceeds the file or tree limits.", "此对比超出了文件或目录树的上限。";
        NoFileChanges => "No file changes between the saved commits.", "已保存的提交之间没有文件变更。";
        FileNotChanged => "This path is absent from the saved comparison.", "已保存的对比中没有此路径。";
        DiffLimited => "This file exceeds the line diff size limit.", "此文件超出了行级差异的大小上限。";
        DiffBinary => "Line diff is unavailable for binary or non-UTF-8 content.", "二进制或非 UTF-8 内容无法显示行级差异。";
        DiffSubmodule => "This change updates a Git submodule reference.", "此变更涉及 Git 子模块引用。";
        DiffUnchanged => "There are no text changes.", "没有文本变更。";
        DiffUnavailable => "Git storage could not read this file's contents.", "Git 存储无法读取此文件的内容。";
        OldLine => "Old line", "旧行号";
        NewLine => "New line", "新行号";
        FileChanges => "File changes", "文件差异";
        OmittedLines => "Unchanged lines omitted", "已省略未变更的行";
        NoFinalNewline => "No newline at end of file", "文件末尾没有换行符";
        FileMode => "File mode", "文件模式";
        Added => "added", "新增";
        Modified => "modified", "修改";
        Deleted => "deleted", "删除";
        UnknownStatus => "Unknown", "未知";
        Merging => "Merging", "合并方式";
        MergingHelp => "Merge locally and push the result to the base branch.", "请在本地完成合并，再将结果推送至目标分支。";
        Review => "Review this comparison", "审查此对比";
        Approve => "Approve", "批准";
        RequestChanges => "Request changes", "要求修改";
        Approved => "Approved this comparison", "批准了此对比";
        ChangesRequested => "Requested changes", "要求修改";
        ReviewComment => "Comment on a changed line", "对变更行发表评论";
        LineComment => "Line comment", "行评论";
        ReviewLine => "Changed line", "变更行";
        ReviewForbidden => "Only repository writers can record a decision on another author's pull request.", "只有仓库写入者可以审查其他作者的合并请求。";
        ReviewInvalid => "Select a review decision or a changed line, with a valid comment.", "请选择审查结论或变更行，并填写有效评论。";
        ReviewConflict => "This comparison can no longer be reviewed.", "此对比已无法审查。";
        VerifyMerge => "Verify merge on base branch", "验证目标分支中的合并";
        MergeForbidden => "Only repository writers can verify a merge.", "只有仓库写入者可以验证合并。";
        MergeVerified => "Verified integration in Git", "已验证提交合入 Git";
        MergeAbsent => "The target branch does not contain both saved commits, or its tip has not advanced.", "目标分支尚未包含两个已保存的提交，或分支顶端尚未推进。";
        MergeLimited => "The merge could not be verified within the Git history read limit.", "在 Git 历史读取上限内无法验证合并。";
        MergeUnavailable => "Git storage could not verify the target history.", "Git 存储无法验证目标分支历史。";
        MergeStateLocked => "A verified merge cannot be reopened.", "已验证合并的请求无法重新开启。";
        VerifiedTip => "Verified target tip", "已验证的目标分支顶端";
    }
}

use super::{CommonText, Locale};
use crate::models::Ts;
use std::fmt::{self, Display};

impl Locale {
    pub fn opened_by<'a>(self, date: &'a Ts, author: &'a str) -> impl Display + 'a {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "opened {} by {author}", date.relative(&self)),
            Self::ZhHans => write!(f, "{}由 {author} 创建", date.relative(&self)),
        })
    }

    pub fn opened_discussion<'a>(self, is_pull: bool, date: &'a Ts, author: &'a str) -> impl Display + 'a {
        let noun = self.discussion_noun(is_pull);
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "{author} opened this {noun} {}", date.relative(&self)),
            Self::ZhHans => write!(f, "{author} {}创建了此{noun}", date.relative(&self)),
        })
    }

    pub fn opened_when(self, date: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "opened {}", date.relative(&self)),
            Self::ZhHans => write!(f, "{}创建", date.relative(&self)),
        })
    }

    pub fn merged_when(self, date: &Ts) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "merged {}", date.relative(&self)),
            Self::ZhHans => write!(f, "{}合并", date.relative(&self)),
        })
    }

    pub fn comment_count(self, count: &i64) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En if *count == 1 => write!(f, "{count} comment"),
            Self::En => write!(f, "{count} comments"),
            Self::ZhHans => write!(f, "{count} 条评论"),
        })
    }

    pub fn discussion_event<'a>(
        self,
        is_pull: bool,
        action: &str,
        actor: Option<&'a str>,
    ) -> impl Display + 'a {
        let noun = self.discussion_noun(is_pull);
        let action = self.t(if action == "reopened" {
            DiscussionText::ReopenedAction
        } else {
            DiscussionText::ClosedAction
        });
        fmt::from_fn(move |f| match (self, actor) {
            (Self::En, Some(actor)) => write!(f, "{actor} {action} this {noun}"),
            (Self::En, None) => write!(f, "This {noun} was {action}"),
            (Self::ZhHans, Some(actor)) => write!(f, "{actor} {action}了此{noun}"),
            (Self::ZhHans, None) => write!(f, "此{noun}已{action}"),
        })
    }

    fn discussion_noun(self, is_pull: bool) -> &'static str {
        self.t(if is_pull { DiscussionText::PullRequest } else { DiscussionText::Issue })
    }

    pub fn discussion_state(self, state: &str) -> &'static str {
        self.t(if state == "open" { CommonText::Open } else { CommonText::Closed })
    }

    pub fn review_decision(self, value: &str) -> &'static str {
        self.t(match value {
            "approved" => DiscussionText::Approved,
            "changes_requested" => DiscussionText::ChangesRequested,
            "line_comment" => DiscussionText::LineComment,
            "merge_verified" => DiscussionText::MergeVerified,
            _ => DiscussionText::UnknownStatus,
        })
    }

    pub fn diff_status(self, status: &str) -> &'static str {
        self.t(match status {
            "added" => DiscussionText::Added,
            "modified" => DiscussionText::Modified,
            "deleted" => DiscussionText::Deleted,
            _ => DiscussionText::UnknownStatus,
        })
    }

    pub fn branch_has_no_commits(self, branch: &str) -> impl Display + '_ {
        fmt::from_fn(move |f| match self {
            Self::En => write!(f, "Branch `{branch}` has no commits in this repository."),
            Self::ZhHans => write!(f, "此仓库中的分支 `{branch}` 没有提交记录。"),
        })
    }
}
