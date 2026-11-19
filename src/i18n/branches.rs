catalog! {
    BranchText {
        Protection => "Branch protection", "分支保护";
        ProtectionHelp => "Protected branches accept updates through the merge queue. Choose the approvals and check contexts required for the reviewed commit.", "受保护分支通过合并队列更新。请选择审查提交所需的批准次数和检查名称。";
        BranchName => "Branch name", "分支名称";
        RequiredApprovals => "Required approvals", "必需批准次数";
        RequiredChecks => "Required checks", "必需检查";
        ChecksHelp => "Enter one CI context per line, such as ci/unit.", "每行输入一个 CI 检查名称，例如 ci/unit。";
        SaveRule => "Save rule", "保存规则";
        RemoveRule => "Remove branch protection", "移除分支保护";
        NoRules => "No protected branches", "暂无受保护分支";
        ConfirmName => "Type the branch name", "输入分支名称";
        InvalidRule => "Enter a valid branch, up to 20 approvals and up to 20 check contexts.", "请输入有效分支，批准次数和检查名称数量均不得超过 20。";
        RuleConflict => "The rule could not be changed. Wait for any Git write or merge to finish, then reload and try again.", "无法更改分支规则。请等待 Git 写入或合并结束，重新加载后重试。";
        InvalidRef => "Enter a valid Git reference and an existing commit ID.", "请输入有效的 Git 引用和已有提交 ID。";
        ManageRefs => "Manage references", "管理引用";
        RefKind => "Reference type", "引用类型";
        NewBranch => "New branch", "新建分支";
        NewTag => "New tag", "新建标签";
        RefName => "Reference name", "引用名称";
        FromCommit => "Commit ID", "提交 ID";
        CreateRef => "Create reference", "创建引用";
        DeleteRef => "Delete reference", "删除引用";
        DeleteRefName => "Type the reference name to confirm deletion", "输入引用名称以确认删除";
        CannotWriteRef => "This branch is protected or is the default branch. Check the branch settings.", "这是受保护分支或默认分支。请检查分支设置。";
        RefConflict => "The reference has changed or already exists. Reload the page and try again.", "引用已更改或已存在。请重新加载页面后重试。";
        InvalidPush => "The Git push command could not be read.", "无法读取 Git 推送命令。";
        ProtectedPush => "This push updates a protected branch or removes the default branch. Use a pull request for protected updates.", "此次推送会更新受保护分支或删除默认分支。请通过合并请求更新受保护分支。";
        QueueMerge => "Queue merge", "加入合并队列";
        CancelMerge => "Cancel queued merge", "取消排队合并";
        Checks => "Checks on the reviewed commit", "审查提交的检查";
        Queued => "Queued for merge", "等待合并";
        Running => "Merging", "正在合并";
        MergeComplete => "Merged by Shard", "已由 Shard 合并";
        MergeWaiting => "Waiting for required approvals or checks", "等待必需的批准或检查";
        MergeConflict => "Git reported a conflict. Resolve it on the source branch and queue the updated comparison.", "Git 报告冲突。请在源分支解决冲突，再将更新后的对比加入队列。";
        MergeStale => "The source or target changed. Review the current comparison before queuing again.", "源分支或目标分支已更改。请审查当前对比后重新排队。";
        MergeAccess => "The merge requester's access changed or the pull request is closed.", "合并发起者的权限已变更，或合并请求已关闭。";
        MergeUnsupported => "This merge needs a configured Git origin with merge support.", "此合并需要配置支持合并的 Git 源站。";
        MergeStorage => "Git storage could not complete the merge.", "Git 存储未能完成合并。";
        MergeForbidden => "Only a repository writer with access to the source can queue this merge.", "只有能访问源仓库的目标仓库写入者可以将此合并加入队列。";
    }
}
